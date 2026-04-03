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
