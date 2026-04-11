# ROBSON v3 — ARCHITECTURAL DECISIONS RECORD

**Date**: 2026-04-03  
**Status**: APPROVED  
**Companion to**: v3-migration-plan.md

---

## Decision Register

### ADR-v3-001: Runtime Language — Rust (robsond)

**Context**: Robson v1 runs stop monitoring via a Django CronJob (60s granularity). Leveraged trading requires <500ms stop-loss execution. The Rust v2 codebase (~21K LOC, 12 crates) already implements the correct architecture with type-safe financial calculations, zero-cost async, and compiled performance.

**Decision**: Rust (robsond daemon) is the production runtime for v3.

**Chose**: Rust via robsond  
**Rejected**: Keep Django as execution runtime; Rewrite in Go; Rewrite in Python with asyncio  

**Rationale**:
- Django CronJob cannot achieve <500ms latency (minimum 60s cycle)
- Python asyncio could achieve it but loses Rust's type safety on Decimal arithmetic and ownership guarantees on concurrent state
- Go is viable but the v2 Rust codebase is already 85% complete — rewriting is waste
- Rust's type system enforces the GovernedAction pattern at compile time (no runtime bypass possible)
- v3 needs a single execution authority; leaving Django CronJobs in parallel creates ambiguity and double-execution risk

**Breaks if wrong**: If the Rust codebase becomes unmaintainable for a solo operator (Rust's learning curve, borrow checker friction on rapid iteration), development velocity drops. Mitigation: freeze the Rust binary at a stable point, add new features via Python sidecar communicating over gRPC. This preserves the performance-critical execution path while allowing rapid iteration on non-critical features.

**Reversibility**: Partially reversible. Rollback means restoring the legacy runtime as a mutually exclusive path, not running Django CronJobs in parallel with robsond. Cannot easily port Rust type system guarantees back to Python.

---

### ADR-v3-002: Event Store — PostgreSQL

**Context**: EventLog is the sole source of truth (Axiom 4). Options: dedicated event store (EventStoreDB), PostgreSQL, Kafka, SQLite.

**Decision**: PostgreSQL (ParadeDB on jaguar:5432) as the event store.

**Chose**: PostgreSQL with append-only table, ULID ordering, idempotency key  
**Rejected**: EventStoreDB (operational overhead for single operator), Kafka (absurd at this scale), SQLite (no concurrent access from multiple components)

**Rationale**:
- PostgreSQL already deployed and operational (ParadeDB on jaguar)
- Single database reduces failure surface (one backup strategy, one monitoring target)
- ParadeDB adds pgvector for future vector search, pg_search for full-text
- PostgreSQL JSONB handles event payloads with indexing
- ULID provides time-sorted, globally unique event IDs
- Idempotency via SHA256 hash prevents duplicate events

**Breaks if wrong**: If event volume exceeds PostgreSQL's write throughput. At current scale (single operator, ~100 events/hour during active trading), PostgreSQL handles this trivially. Even at 10x growth, table partitioning by month solves it. The trigger for reconsideration: >100K events/day sustained.

**Schema**:
```sql
CREATE TABLE event_log (
    event_id        ULID PRIMARY KEY,
    stream_key      TEXT NOT NULL,
    sequence        BIGINT NOT NULL,
    event_type      TEXT NOT NULL,
    payload         JSONB NOT NULL,
    timestamp       TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    cycle_id        ULID,
    component       TEXT NOT NULL,
    idempotency_key TEXT UNIQUE,
    UNIQUE(stream_key, sequence)
);
```

---

### ADR-v3-003: Risk Engine — Central, Blocking

**Context**: The Risk Engine can be advisory (log violations, continue) or blocking (deny violating actions). Financial systems literature universally recommends blocking for pre-trade risk checks.

**Decision**: Risk Engine is a mandatory blocking gate. No action reaches the Executor without Risk Engine clearance.

**Chose**: Blocking gate with GovernedAction type  
**Rejected**: Advisory mode (log-only), Hybrid (advisory for low-risk, blocking for high-risk)

**Rationale**:
- A single trade exceeding 1% risk can wipe 1% of capital. This is not a "log and investigate" scenario.
- Advisory mode creates a false sense of safety — risk violations are detected but not prevented
- Hybrid mode (advisory for low-risk) introduces complexity in categorizing risk levels, with the dangerous possibility of miscategorization letting a high-risk action through as "low-risk"
- The GovernedAction pattern (Rust type that can only be constructed by Runtime after Risk Engine approval) makes bypass impossible at compile time

**Breaks if wrong**: False positives block valid trades. The operator misses profitable entries during fast moves. Mitigation: operator override with mandatory audit event. Hard limits (daily loss, drawdown) cannot be overridden even by operator.

---

### ADR-v3-004: Agent Model — Single-Agent with Tool Delegation

**Context**: Multi-agent architectures enable parallel strategy execution but add coordination complexity (shared state management, message passing, failure isolation between agents).

**Decision**: Single-agent (robsond) with tool delegation to Engine, Executor, EventLog.

**Chose**: Single-agent  
**Rejected**: Multi-agent with coordinator, Actor model (Actix/Tokio actors)

**Rationale**:
- Single operator running one strategy at a time. No parallelism needed.
- Single-agent eliminates: distributed state consistency, agent-to-agent communication protocol, partial failure handling
- Tool delegation through Rust traits (ExchangePort, Store, etc.) is sufficient
- The control loop is inherently sequential (Observe->Persist)

**Trigger for reconsideration**: Operator regularly has >3 concurrent armed positions requiring independent risk budgets and simultaneous attention.

---

### ADR-v3-005: Frontend — Evolve Existing React + Vite

**Context**: Frontend exists (React 18 + Vite), follows hexagonal architecture, has agentic workflow UI, pattern dashboard. Options: keep and evolve, rewrite in another framework, build native desktop app.

**Decision**: Keep React + Vite, evolve into operator control surface.

**Chose**: React 18 + Vite with SSE for real-time  
**Rejected**: Rewrite in Svelte/SolidJS (no benefit for single-user app), Electron desktop app (unnecessary for web-accessible control surface), Terminal UI (loses visual richness needed for trading)

**Rationale**:
- Codebase exists and works. Rewriting is engineering theater.
- React's component model maps well to the control surface paradigm (widgets bound to SSE event streams)
- Vite provides fast builds and HMR for development
- SSE (Server-Sent Events) for unidirectional event push is simpler than WebSocket for the event stream use case

**Breaks if wrong**: React bundle size impacts load time. Extremely unlikely for a single-user control surface. If it happens, code-split lazy-load non-critical panels.

---

### ADR-v3-006: Database Consolidation — PostgreSQL Only

**Context**: v1 uses PostgreSQL (via Django ORM). Some architectures add MongoDB for document storage, Redis for caching, separate vector databases. Each additional database is a separate failure mode, backup strategy, and monitoring target.

**Decision**: PostgreSQL only (ParadeDB with pgvector). Redis for caching/pub-sub only (not durable data).

**Chose**: PostgreSQL (ParadeDB) for all durable data + Redis for volatile cache/messaging  
**Rejected**: PostgreSQL + MongoDB (no document-store need that JSONB doesn't handle), PostgreSQL + dedicated vector DB (pgvector is sufficient), PostgreSQL + Redis as durable store (Redis persistence is not equivalent to PostgreSQL durability)

**Rationale**:
- ParadeDB = PostgreSQL 16 + pgvector + pg_search (BM25). Covers relational, vector, and full-text search in one engine.
- JSONB columns handle all semi-structured data (event payloads, configuration, pattern metadata)
- Fewer databases = fewer things that can fail independently
- One backup strategy (pg_dump + WAL archiving) covers all data

**Breaks if wrong**: Vector search performance degrades under load. ParadeDB's pgvector is optimized for this. At current scale (<100K vectors), performance is not a concern. Trigger: if vector queries exceed 100ms p99.

---

### ADR-v3-007: Message Broker — Redis Streams

**Context**: v1 staging deploys RabbitMQ (StatefulSet, management UI, AMQP). v1 production uses Django Outbox pattern with RabbitMQ as the intended consumer. Options: keep RabbitMQ, switch to Redis Streams, use Postgres NOTIFY/LISTEN.

**Decision**: Redis Streams for internal messaging. Remove RabbitMQ.

**Chose**: Redis Streams (Redis 7, already deployed)  
**Rejected**: RabbitMQ (operational overhead: StatefulSet, Erlang runtime, management UI, credentials), Kafka (absurd at this scale), Postgres NOTIFY/LISTEN (no persistence, no consumer groups)

**Rationale**:
- Redis 7 already running in production (robson-redis)
- Redis Streams provides: persistence, consumer groups, acknowledgment, backpressure
- Operational overhead: zero (Redis is already there)
- RabbitMQ adds: separate StatefulSet, Erlang runtime, management UI with auth, separate monitoring, separate credentials. All for a single-operator system.

**Breaks if wrong**: Redis Streams loses messages under load. Detection: consumer lag metric. Mitigation: EventLog is the source of truth. Any lost message can be replayed from EventLog. Redis Streams is a convenience, not a durability layer.

---

### ADR-v3-008: Kubernetes — Keep k3s

**Context**: 4-node k3s cluster (tiger/altaica/sumatrae/jaguar) running production workloads with ArgoCD, cert-manager, Traefik, Let's Encrypt. Question: is k3s justified for a single operator?

**Decision**: Keep k3s.

**Chose**: k3s (existing cluster)  
**Rejected**: Docker Compose (loses GitOps, auto-TLS, health checks, rolling updates, resource limits), systemd (loses container isolation, declarative config), Nomad (learning curve for zero benefit over existing k3s)

**Rationale**:
- Infrastructure investment is sunk. The cluster works. Removing it means rebuilding everything.
- k3s provides: automated TLS renewal, GitOps via ArgoCD, health checks with auto-restart, rolling deployments, namespace isolation, resource limits, Traefik ingress
- Cost: ~EUR 60/month (4x Contabo VPS). Dropping to 2 nodes saves EUR 30/month but loses redundancy.
- docker-compose would require manual TLS management, manual restarts on failure, no GitOps, no resource limits

**Breaks if wrong**: k3s becomes operationally burdensome. Fallback: single-node k3s on tiger with local-path storage is the minimum viable cluster.

---

### ADR-v3-009: Secrets — SOPS + age

**Context**: Current secrets are Kubernetes Secrets (base64 in manifests, templates in git). Options: HashiCorp Vault, SOPS + age, sealed-secrets, environment variables.

**Decision**: SOPS with age encryption.

**Chose**: SOPS + age  
**Rejected**: HashiCorp Vault (requires its own HA cluster — absurd for single operator), sealed-secrets (requires controller on cluster, less portable), raw environment variables (no encryption at rest, no audit trail)

**Rationale**:
- SOPS encrypts secrets in git with age public key
- Decryption requires age private key (stored on tiger at /etc/sops/age/keys.txt)
- Auditable: encrypted secrets are versioned in git
- Simple: single binary (sops), single key pair (age)
- No additional infrastructure
- ArgoCD integration via KSOPS plugin or helm-secrets

**Breaks if wrong**: Age private key compromised. Blast radius: all Robson secrets exposed (API keys, database passwords). Mitigation: rotate all secrets immediately. Recovery time: <1 hour.

---

### ADR-v3-010: Observability — Self-Hosted Stack

**Context**: Options: managed (Datadog, New Relic, Grafana Cloud) or self-hosted (Prometheus + Grafana + Loki).

**Decision**: Self-hosted on k3s.

**Chose**: OpenTelemetry + Prometheus + Grafana + Loki (self-hosted)  
**Rejected**: Datadog ($50-200/month, sends telemetry to third party), Grafana Cloud (free tier limited, paid tier unnecessary), New Relic (same concerns as Datadog)

**Rationale**:
- Budget: EUR 0 additional cost (runs on existing nodes)
- Privacy: financial system telemetry stays on own infrastructure
- Control: retention policies, alerting rules, dashboards fully configurable
- Sufficient: for single operator, Prometheus + Grafana covers all monitoring needs

**Breaks if wrong**: Self-hosted stack consumes too many resources. Mitigation: reduce retention (7 days metrics, 30 days logs), move to jaguar if needed. Nuclear option: switch to Grafana Cloud free tier.

---

### ADR-v3-011: CI/CD — GitHub Actions + ArgoCD

**Context**: Already deployed and working. Question: is ArgoCD justified?

**Decision**: Keep GitHub Actions + ArgoCD.

**Rationale**: ArgoCD is already deployed, configured, and working. Removing it would be work for negative value. If starting fresh, `kubectl apply` from GitHub Actions would suffice. But we are not starting fresh.

---

### ADR-v3-012: TRON/TRC-20 — Defer

See v3-tron-evaluation.md for full analysis.

**Decision**: No product integration in v3. Architecture-ready via PaymentRail trait. Pursue $1B AI fund as funding opportunity.

---

### ADR-v3-013: Django Sunset — Incremental

**Context**: Django monolith is live in production. Rust daemon is architecturally superior but not yet deployed. Options: big-bang replacement, incremental migration, keep both forever.

**Decision**: Incremental sunset. Django serves API/frontend during v2.5. Replaced by thin gateway in v3.

**Chose**: Incremental sunset  
**Rejected**: Big-bang replacement (too risky — both systems have unique capabilities), Keep both forever (maintenance burden of two systems in two languages)

**Rationale**:
- Big-bang migration risks breaking the live system. The operator is trading real capital.
- Keeping both forever means maintaining Python + Rust codebases indefinitely
- Incremental: deploy Rust daemon alongside Django, migrate responsibilities one at a time, verify at each step, remove Django when fully replaced
- Hard deadline: Django must be fully replaced by v3 launch (prevents "temporary" becoming permanent)

**Breaks if wrong**: Migration takes too long, Django becomes permanent second system. Mitigation: hard deadline + clear migration sequencing (Section 12 of migration plan).

---

### ADR-v3-015: Execution Core — QueryEngine with state-first Architecture

**Context**: The Control Loop (Observe -> Interpret -> Decide -> Act -> Evaluate -> Persist) is currently stitched together by PositionManager, which directly calls Engine + Executor. Execution lifecycle state is fragmented: PositionState tracks business state, IntentStatus tracks idempotency state, and runtime-cycle state is implicit in call stacks. There is no single typed execution unit. Additionally, the relationship between RuntimeState (operational truth), EventLog (durable truth), and Projections (derived views) is implicit.

**Decision**: Introduce QueryEngine as the governed execution core inside the Runtime, with ExecutionQuery as the typed lifecycle unit. Formalize the architectural premise: `state = source of truth, stream = projection`.

**Chose**: QueryEngine inside robsond with phased rollout (passive wrapper -> blocking governance -> approval gates -> full audit)
**Rejected**: (a) Rewrite PositionManager from scratch (too risky, breaks everything), (b) Separate QueryEngine crate (violates Runtime exclusivity — Control Loop must be owned by robsond), (c) Keep implicit stitching (fragmentation grows with each new feature)

**Rationale**:
- Every trigger becomes a typed ExecutionQuery with explicit state machine (Accepted -> Processing -> Acting -> Completed / Failed)
- Single entry point: `QueryEngine.process()` is the ONLY path to mutate RuntimeState
- Formalizes what PositionManager does informally, without rewriting it
- The `state = source of truth, stream = projection` premise clarifies: RuntimeState is the operational authority, EventLog is the durable authority, Projections are always derived
- QE-P1 is non-breaking (wrapper + tracing), QE-P2 wires GovernedAction and Risk Engine as blocking gate
- Aligns with GovernedAction pattern (v3-runtime-spec.md) — GovernedAction is constructed inside QueryEngine after risk clearance

**Breaks if wrong**: QueryEngine adds indirection that slows development. Mitigation: QE-P1 is a thin wrapper with zero behavior change. If indirection proves harmful, remove it — the underlying Engine + Executor remain unchanged.

See [v3-query-query-engine.md](v3-query-query-engine.md) for the full specification.

---

### ADR-v3-014: Concurrency — Sequential Control Loop

**Context**: The control loop can run cycles concurrently (with isolation guarantees) or sequentially (one at a time, queued).

**Decision**: Sequential. No concurrent cycles.

**Chose**: Sequential with bounded observation queue (capacity 1000)  
**Rejected**: Concurrent with per-position locking (complexity without benefit for single operator)

**Rationale**:
- Single operator, one strategy at a time. No parallelism needed.
- Sequential eliminates: race conditions on PositionState, distributed locking, partial cycle failures
- Queue handles burst observations. Critical events (operator commands, risk alerts) have priority and are never dropped.

**Breaks if wrong**: Market moves require simultaneous management of multiple positions and the queue introduces unacceptable latency. Trigger: operator regularly has >3 positions requiring simultaneous attention AND measured queue latency exceeds 1 second.

---

### ADR-v3-015: Query Persistence Granularity — Governance-Relevant Only

**Date**: 2026-04-05
**Status**: DECIDED — implementation alignment is follow-up

**Context**: QE-P4 introduced durable query lifecycle persistence via `QUERY_STATE_CHANGED` events in EventLog. The question was whether ALL queries (including high-frequency market ticks that terminate in `NoAction`) should be durably persisted.

**Decision**: Persist only governance-relevant and operationally-relevant queries.

**Chose**: Selective persistence based on query outcome and governance significance
**Rejected**: Persist all queries (including NoAction market ticks)

**Criteria for durable persistence**:
- Queries that produce actions (`ActionsExecuted`)
- Queries entering approval/authorization/expiration flow
- Queries resulting in `Denied`, `Failed`, or auditable terminal states
- Queries crossing governance boundaries (risk gate evaluation)

**Excluded from durable persistence**:
- High-frequency `NoAction` queries (market ticks without effect) — tracing/metrics only

**Rationale**: Market ticks at 100/s would generate ~8.6M EventLog rows/day with zero audit value. Tracing via `TracingQueryRecorder` provides full observability for debugging. The EventLog must remain a meaningful audit trail, not a firehose.

**Breaks if wrong**: If a NoAction query later turns out to have been governance-relevant (e.g., a tick that *should* have triggered a stop but didn't due to a bug), the EventLog won't have it. Mitigation: tracing logs retain full query lifecycle for debugging; only durable audit is selective.

**Follow-up**: Verify that `EventLogQueryRecorder` applies this filter. If it currently persists all queries regardless of outcome, adjust the implementation.

See [v3-query-query-engine.md §11](v3-query-query-engine.md) for details.

---

### ADR-v3-016: Event Model Convergence — EventEnvelope as Canonical Durable Format

**Date**: 2026-04-05
**Status**: DECIDED — full convergence is follow-up

**Context**: The repository has two event models: `robson_domain::Event` (internal domain events used by Engine/Executor within crate boundaries) and `EventEnvelope` (the `robson-eventlog` durable format with ULID, stream_key, sequence, JSONB payload). Both currently participate in persistence — Executor writes via `store.events().append()` using domain events, while QueryEngine writes `QUERY_STATE_CHANGED` as `EventEnvelope` directly. This dual-model state is transitional, not architectural target.

**Decision**: `robson_eventlog` / `EventEnvelope` / `event_log` table is the single canonical durable event format.

**Chose**: One canonical durable format (`EventEnvelope`), with `robson_domain::Event` as internal-only domain representation
**Rejected**: Two parallel durable models; QueryEngine producing both formats as independent durable outputs

**Convergence plan**:
1. `robson_domain::Event` remains as the internal business-logic representation within crate boundaries
2. All durable persistence converges to `EventEnvelope` as the single canonical format
3. Adaptation between `robson_domain::Event` and `EventEnvelope` happens at persistence boundaries
4. No component should maintain a separate durable event store outside the canonical `event_log` table

**Breaks if wrong**: If `EventEnvelope` schema is too rigid for future domain event types, adaptation at the boundary becomes complex. Mitigation: `EventEnvelope.payload` is JSONB — schema flexibility is inherent.

**Follow-up**: Migrate Executor persistence to emit `EventEnvelope` directly, or introduce an adapter at the Store boundary. This is a code change, not an architectural decision.

See [v3-query-query-engine.md §11](v3-query-query-engine.md) for details.

---

### ADR-v3-017: Namespace Consolidation — robsond moves to namespace `robson`

**Date**: 2026-04-11
**Status**: DECIDED — implementation in MIG-v3#2

**Context**: robsond runs in namespace `robson-v2` while the Django stack (backend, frontend, Redis, CronJobs) runs in namespace `robson`. With MIG-v3#1 complete, robsond is the sole execution authority. The Django backend is being removed (MIG-v3#2). Two namespaces for a single application increase routing complexity (cross-namespace service references in Ingress/HTTPRoute) without any remaining benefit.

**Decision**: robsond deployment moves to namespace `robson`. Namespace `robson-v2` is archived and its ArgoCD Application removed.

**Chose**: Single namespace (`robson`) for all Robson resources
**Rejected**: Keep two namespaces (adds cross-namespace routing, no isolation benefit for single-operator system); Create new namespace `robson-v3` (unnecessary churn)

**Rationale**:
- Single operator, single application. Namespace isolation was a transitional safeguard during v2.5 coexistence.
- Cross-namespace HTTPRoute/Ingress references require ExternalName Services or gateway-level workarounds.
- ArgoCD manages one Application instead of two, reducing sync complexity.

**Breaks if wrong**: If robsond needs to be isolated from frontend/Redis for security or resource reasons, re-introducing a namespace is straightforward (move manifests, update kustomization). At current scale (single replica, single operator), this is unnecessary.

**Rollback**: git revert + ArgoCD sync.

---

### ADR-v3-018: Gateway API as sole routing layer (Traefik)

**Date**: 2026-04-11
**Status**: DECIDED — implementation in MIG-v3#2

**Context**: The cluster runs both Kubernetes Ingress (`networking.k8s.io/v1`) and Gateway API HTTPRoute (`gateway.networking.k8s.io/v1`) for the same hosts. Both point to the same backends via Traefik. This duplication creates confusion about which layer is authoritative and can cause conflicting routing decisions.

**Decision**: Remove all `Ingress` resources. Only `HTTPRoute` (Gateway API) remains for external routing.

**Chose**: Gateway API (HTTPRoute) exclusively
**Rejected**: Keep both layers (duplication, confusion); Ingress only (backwards-looking, no benefit)

**Rationale**:
- Gateway API is the forward-looking standard. Traefik supports it natively.
- One routing mechanism per host eliminates ambiguity.
- The `robson-gateway` Gateway resource already exists in the cluster.

**Breaks if wrong**: If Gateway API support in Traefik has regressions, re-adding Ingress resources is a git revert. Low risk — Gateway API is stable (v1 since Kubernetes 1.28).

**Rollback**: git revert + ArgoCD sync.

---

### ADR-v3-019: Django backend removed (not scaled to zero)

**Date**: 2026-04-11
**Status**: DECIDED — implementation in MIG-v3#2

**Context**: The Django backend Deployment (`robson-backend`) has been superseded by robsond as the sole runtime (MIG-v3#1). The question is whether to scale it to zero (`replicas: 0`) or remove the manifests entirely.

**Decision**: Remove the Deployment and Service manifests from git. Do not scale to zero.

**Chose**: Full removal from manifests
**Rejected**: `replicas: 0` (ambiguous state — "suspended but present" sends mixed signal about what is active)

**Rationale**:
- `replicas: 0` keeps the Deployment in desired state, suggesting it might come back. This is ambiguous after MIG-v3#1 committed to robsond as sole runtime.
- Removal records the decision clearly in git history.
- Rollback is a git revert — no data loss, no irreversible state change.
- Django database tables remain untouched; only the runtime is removed.

**Breaks if wrong**: If Django needs to be reactivated urgently, git revert restores manifests and ArgoCD re-deploys. The Django image and configuration remain in git history permanently.

**Rollback**: git revert + ArgoCD sync.

---

### ADR-v3-020: Django CronJobs removed

**Date**: 2026-04-11
**Status**: DECIDED — implementation in MIG-v3#2

**Context**: Three CronJobs exist in namespace `robson`:
- `rbs-stop-monitor-cronjob` — suspended since MIG-v3#1 (2026-04-10)
- `rbs-trailing-stop-cronjob` — suspended since MIG-v3#1 (2026-04-10)
- `rbs-pattern-scan-cronjob` — still active, runs every 15 minutes with `BINANCE_USE_TESTNET=True`

**Decision**: Remove all three CronJob manifests from git.

**Chose**: Full removal
**Rejected**: Keep suspended (ambiguous); Keep pattern-scan (testnet-only, Django ecosystem, no v3 role)

**Rationale**:
- Stop monitor and trailing stop are already replaced by robsond's WebSocket runtime.
- Pattern scan runs against testnet and belongs to the Django pattern engine — not part of v3 scope.
- Keeping suspended CronJobs in manifests creates noise and confusion about what is active.

**Rollback**: git revert + ArgoCD sync.

---

### ADR-v3-021: Frontend adaptation deferred

**Date**: 2026-04-11
**Status**: DECIDED — separate work item post MIG-v3#2

**Context**: The React frontend (`app.robson.rbx.ia.br`) currently calls `api.robson.rbx.ia.br` expecting the Django API contract. After MIG-v3#2, `api.robson.rbx.ia.br` will point to robsond's Axum API, which has a different (simpler) contract. The frontend will break for any endpoint not present in robsond.

**Decision**: Do not modify the frontend in MIG-v3#2. Accept temporary breakage. Frontend reconnection is MIG-v3#3.

**Chose**: Defer frontend changes to MIG-v3#3
**Rejected**: Adapt frontend now (scope creep, MIG-v3#2 is infrastructure-only)

**Rationale**:
- MIG-v3#2 scope is infrastructure routing (ingress) and Django sunset.
- Frontend changes require understanding the exact API contract differences and are a separate work item.
- robsond's API already covers all trading endpoints. Non-trading endpoints (patterns, indicators) are deferred by design.
- Operator has CLI as fallback for all critical operations.

**Breaks if wrong**: Frontend is temporarily non-functional for trading operations. Operator uses CLI. Risk is acceptable — operator confirmed "frontend breaking is acceptable."
