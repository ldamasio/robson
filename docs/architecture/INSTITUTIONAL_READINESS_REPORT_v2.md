# Robson Institutional Readiness Report (v2 Snapshot)

## 1. Context

This report evaluates Robson during a transitional phase between the current `v2` runtime track and the intended `v3` runtime architecture.

The assessment is about the system as a governed execution runtime. It is not a model evaluation. The runtime is the system. Any model, detector, or future LLM component is a dependency that must remain subordinate to runtime control, audit, and risk policy.

Repository reality is split across two layers:

- Legacy application surfaces at the repository root, still documented in the top-level `README.md`.
- The Rust runtime migration track under `v2/`, centered on `robsond`, `robson-exec`, `robson-eventlog`, `robson-projector`, and the `docs/architecture` migration documents.

This report therefore treats `v2/robsond` and its adjacent runtime crates as the primary assessment boundary.

## 2. System Classification

### Current state: Research Runtime

Repository-verified status: the system is best classified as a research runtime with serious architectural intent.

Reasons:

- `v2/README.md` explicitly marks the Rust runtime as `Development (Alpha)`.
- `v2/robsond/src/main.rs` boots `Daemon::new_stub_with_recovery(...)` or `Daemon::new_stub(...)`, not a production exchange-backed runtime.
- `v2/robson-exec/src/stub.rs` confirms that the active daemon path uses a stub exchange with simulated fills.
- Several governance and durability mechanisms exist, but they remain partial, in-memory, or migration-scoped.

### Target state: Governed Financial Decision System

The target state is clearly described in:

- `docs/architecture/v3-runtime-spec.md`
- `docs/architecture/v3-query-query-engine.md`
- `docs/architecture/v3-control-loop.md`
- `docs/architecture/v3-migration-plan.md`

Those documents describe a governed financial decision system in which:

- runtime authority is explicit,
- risk is blocking,
- event persistence is durable,
- projections are derived,
- operator actions are part of the control model,
- and external dependencies operate under runtime supervision.

That target state is not yet the repository-verified current implementation.

## 3. Architectural Assessment

### Auditability

**What exists**

- Typed domain lifecycle events exist in `v2/robson-domain/src/events.rs` for arming, signal receipt, order placement, fills, trailing stop updates, exits, closure, disarm, and error conditions.
- A structured PostgreSQL event envelope exists in `v2/robson-eventlog/src/types.rs` with tenant scope, stream key, sequence number, idempotency key, correlation metadata, actor fields, and reserved hash-chain fields.
- Event append logic in `v2/robson-eventlog/src/append.rs` provides ordered stream persistence plus global idempotency.
- Query lifecycle persistence exists through `v2/robsond/src/query_engine.rs`, `v2/migrations/20240101000007_query_audit_phase4.sql`, and `v2/robson-projector/src/handlers/queries.rs`.
- Replayability for query audit projections is repository-verified by `v2/robsond/tests/replay_test.rs`.

**What is partial**

- Position-domain events are durably bridged from runtime execution through `v2/robsond/src/position_manager.rs`, but this is a synchronous fail-fast bridge, not a unified transactional state model.
- `positions_current` is updated synchronously on the runtime write path when PostgreSQL is configured, while query audit projection is worker-driven through `v2/robsond/src/projection_worker.rs`.
- Restart handling explicitly invalidates in-memory pending approvals by appending `restart_invalidated` query events in `v2/robsond/src/daemon.rs`.

**What is missing**

- Position append and projection apply are not a single atomic guarantee; `docs/architecture/v3-migration-plan.md` states this directly.
- The projection worker does not catch up `position:{id}` streams; it watches only the configured stream key, typically the daemon query-audit stream.
- Pending approvals are intentionally non-durable and are dropped on restart.
- Tamper-evident hashing is reserved in schema but not implemented as an active chain.

### Runtime Governance

**What exists**

- `v2/robsond/src/query_engine.rs` implements a real blocking governance path for entry execution via `check_risk()`, `check_approval()`, `revalidate_risk()`, and `authorize()`.
- `v2/robsond/src/query.rs` defines an explicit query lifecycle: `Accepted -> Processing -> RiskChecked -> AwaitingApproval -> Authorized -> Acting -> Completed`, plus `Failed`, `Denied`, and `Expired`.
- `v2/robsond/src/position_manager.rs` routes entry-signal handling through the query lifecycle and governance gate before executor dispatch.
- The architecture docs correctly frame the runtime as the control authority and any future model as subordinate dependency.

**What is partial**

- Governance is strongest on the entry path. The executor boundary remains transitional, as explicitly documented in `docs/architecture/v3-runtime-spec.md`.
- Approval gating exists, but pending approvals live in runtime memory and require REST bootstrap because SSE is ephemeral.
- The runtime documents pause, halt, circuit breaker, and full control-loop governance, but those controls are not yet represented as an implemented `RuntimeState`.

**What is missing**

- `v2/robsond/src/api.rs` exposes mutating routes for arm, signal injection, approval, disarm, and panic with no repository-visible authentication or authorization layer.
- Operator authority is therefore not attributable at the API boundary.
- `v2/robsond/src/config.rs` defaults the API bind to `0.0.0.0:8080`, expanding exposure of an unauthenticated control surface.
- The documented pause/resume and circuit-breaker control model in `docs/architecture/v3-control-loop.md` is not implemented as current runtime authority.

### Risk Controls

**What exists**

- `v2/robson-engine/src/risk.rs` implements explicit portfolio checks for maximum open positions, total exposure, single-position concentration, duplicate position prevention, and daily loss limit logic.
- `v2/robsond/src/position_manager.rs` builds a risk context using `find_risk_open()` from `v2/robson-store/src/repository.rs`, which correctly counts `Entering` and `Active` positions for exposure.
- `v2/robson-exec/src/executor.rs` validates isolated margin and fixed leverage before both entry and exit orders.
- `v2/robsond/src/position_monitor.rs` implements a rogue-position Safety Net with retries, cooldown, and panic-mode escalation.

**What is partial**

- Risk governance is explicit for entry approval, but it is not yet represented as a complete runtime-wide risk state with pause, halt, circuit-breaker, drawdown, and order-rate constraints.
- Approval revalidation exists, which is a strong governance property, but only for the pending-approval path.
- Safety Net controls are more mature than some core runtime controls; this is an inversion of institutional priorities.

**What is missing**

- `v2/robsond/src/position_manager.rs` explicitly states that daily realized and unrealized PnL default to zero, which makes the daily loss breaker effectively inactive.
- `v2/robsond/src/api.rs` accepts `capital` and `risk_percent`, but `v2/robsond/src/position_manager.rs` ignores the supplied `RiskConfig`, while `v2/robsond/src/daemon.rs` initializes the engine with a hardcoded `$10,000 / 1%` configuration.
- `v2/robson-domain/src/entities.rs` hardcodes fixed 10x leverage at the domain layer.
- The documented circuit-breaker ladder in `docs/architecture/v3-migration-plan.md` and `docs/architecture/v3-control-loop.md` is still pending.
- Panic close is incomplete because `Entering` positions are explicitly skipped until cancel-order logic exists.

### Failure Handling

**What exists**

- Query lifecycle failures are explicit and typed in `v2/robsond/src/query.rs`.
- Runtime restart invalidates unsafe pending approvals in `v2/robsond/src/daemon.rs`.
- The event-log persistence bridge in `v2/robsond/src/position_manager.rs` is fail-fast when PostgreSQL is configured.
- Safety Net failure handling in `v2/robsond/src/position_monitor.rs` includes transient retry logic, cooldown windows, and panic mode.
- Recovery from projection is present in `v2/robsond/src/daemon.rs` when the store is empty and projection recovery is configured.

**What is partial**

- Recovery logic is split between store replay, projection fallback, and query invalidation rather than a single runtime recovery model.
- `docs/architecture/v3-query-query-engine.md` correctly documents several failure modes, but part of that document still describes target recovery properties rather than fully implemented runtime behavior.
- Exchange error handling quality differs materially between Safety Net and the core executor.

**What is missing**

- `v2/robson-exec/src/executor.rs` performs one exchange call per order and does not implement the retry semantics described in the architecture docs.
- `v2/robsond/src/api.rs` reports readiness even when PostgreSQL is absent and without a real Binance connectivity check.
- `v2/robsond/src/position_manager.rs` leaves `Entering` positions unmanaged during panic if the order must be canceled.
- Because the daemon currently boots `MemoryStore` plus `StubExchange`, the repository does not provide a credible live failure-handling path for the main runtime binary.

### Observability

**What exists**

- `v2/robsond/src/main.rs` initializes structured tracing.
- `v2/robsond/src/query_engine.rs` emits structured logs for query-state transitions.
- Query lifecycle snapshots are durable when PostgreSQL is configured.
- `v2/robsond/src/api.rs` exposes `/events`, `/status`, `/health`, `/healthz`, `/readyz`, and Safety Net status endpoints.
- SSE event mapping exists in `v2/robsond/src/sse.rs` for operator-facing runtime updates.

**What is partial**

- SSE is useful for operator visibility, but it is explicitly ephemeral and non-replayable.
- Query observability is materially better than position/risk/latency observability.
- Health endpoints exist, but their semantics are weaker than the deployment documentation suggests.

**What is missing**

- The runtime does not expose a real `/metrics` endpoint, despite `v2/k8s/prod/robsond-deployment.yml` advertising Prometheus scraping on `/metrics`.
- No repository-visible Prometheus counters, histograms, or OpenTelemetry instrumentation exist in the runtime crates.
- Operator identity is not attached to API-originated actions, so audit metadata cannot support institutional accountability.
- There is no repository-verified operator-grade inspection surface for replay, drift analysis, or runtime watermark visibility.

### State Architecture

**What exists**

- The docs explicitly distinguish operational truth, durable truth, and derived projections in `docs/architecture/v3-query-query-engine.md`.
- Current operational state lives in `Store`, accessed by `PositionManager`, as documented in the same file.
- Durable audit state exists in `robson-eventlog`.
- Projection tables such as `positions_current` and `queries_current` exist and are usable for recovery and inspection.

**What is partial**

- The implemented model is transitional: `Store` is operational truth during runtime, `EventLog` is durable truth when wired, and projections are partly synchronous and partly worker-updated.
- `v2/robsond/src/daemon.rs` rebuilds from `store.events().get_all_events()` and falls back to projection recovery when the store is empty.
- `docs/architecture/v3-runtime-spec.md` and `docs/architecture/v3-query-query-engine.md` correctly state that explicit `RuntimeState` is a v3 target, not current implementation.

**What is missing**

- The current daemon uses `MemoryStore`, not a durable runtime state store.
- There is no implemented `RuntimeState` structure carrying paused/halted flags, circuit-breaker state, risk snapshot, or cycle watermark as described in `docs/architecture/v3-runtime-spec.md`.
- The repository does not yet justify the claim `state = source of truth, stream = projection` in a clean institutional sense for the active daemon path, because state, stream, projection, and recovery are still split across transitional mechanisms.

## 4. Credibility Gaps

- Main daemon boots stub exchange and `MemoryStore` -> no credible institutional execution path exists in the active runtime binary.
- Operator authority is not attributable -> breaks institutional accountability.
- Mutating runtime API is exposed without repository-visible access control -> execution authority is not governed at the boundary.
- Risk configuration is not authoritative -> operator intent and runtime behavior can diverge.
- Daily loss breaker is structurally inactive -> portfolio loss controls are overstated.
- Circuit-breaker and pause/halt model are documented but not implemented as runtime authority -> governance claims exceed code reality.
- Panic handling cannot safely unwind all non-terminal states -> emergency control is incomplete.
- Event persistence and projection update are not atomic -> recovery confidence is weaker than institutional standards require.
- Position-stream projection catch-up is absent -> durability and replay semantics are inconsistent across subsystems.
- Health and observability claims overstate runtime truth -> operators can receive false confidence.

## 5. Gap Classification

### Transitional

- Split architecture between legacy root application surfaces and `v2/` Rust runtime track.
- Query audit projection implemented while position projection still relies on synchronous write-path bridging.
- Pending approvals intentionally non-durable during the migration period.
- `Store`-based operational truth still stands in for the explicit `RuntimeState` planned for v3.
- Frontend and operator control surface remain partially coupled to interim REST plus ephemeral SSE patterns.

### Structural

- Unauthenticated runtime control surface.
- Non-attributable operator actions.
- Stub runtime boot path as the default daemon composition.
- Ignored operator-supplied risk configuration and hardcoded engine risk values.
- Inactive daily-loss control due to zeroed PnL inputs.
- Incomplete panic and order-cancel handling.
- Missing institutional telemetry and truthful readiness semantics.
- Non-atomic persistence/projection path for execution-critical state.

## 6. Institutional Impact

The current repository would not be accepted for real capital deployment by a regulated financial institution because the control problem is not closed.

The issue is not missing polish. The issue is that the runtime cannot yet prove, from repository evidence alone, that:

- every privileged action is attributable to an authenticated operator or governed subsystem,
- runtime state is durable and authoritative during live execution,
- risk limits configured by operators are the limits actually enforced,
- emergency controls can reliably unwind risk across all live states,
- and health or observability signals correspond to operational truth.

An institution allocating real capital does not fund intent. It funds control evidence. In the current snapshot, Robson contains good design work and some defensible primitives, but it does not yet present a repository-verified governed execution system.

## 7. Maturity Snapshot Table

| Domain | Snapshot | Repository-Verified Maturity | Institutional Readiness |
|--------|----------|------------------------------|-------------------------|
| Auditability | Strong typed events, durable query audit, replay-tested query projection | Partial | Not sufficient |
| Governance | Real entry gating via QueryEngine and approvals | Partial | Not sufficient |
| Risk | Explicit pre-trade checks and Safety Net controls | Partial | Not sufficient |
| Execution | Stub daemon path, idempotent executor, margin validation | Weak | Not acceptable |
| Observability | Tracing, SSE, status endpoints | Weak | Not acceptable |
| State | Transitional split across Store, EventLog, and projections | Partial | Not sufficient |

## 8. Conditions for Institutional Acceptance

For Robson to be taken seriously by a regulated financial institution, all of the following must be true:

1. The active runtime boot path must use a real exchange adapter and durable operational state, not stub composition.
2. Every privileged runtime action must pass through authenticated and authorized operator or system identity, with durable actor attribution in audit records.
3. Risk configuration must be authoritative, persistent, and provably identical between operator intent and runtime enforcement.
4. Daily PnL, exposure, drawdown, and rate-limit controls must be active inputs to runtime governance rather than documented intentions.
5. Circuit-breaker, pause, halt, and panic controls must exist as implemented runtime state transitions with durable evidence.
6. Emergency handling must cover all non-terminal execution states, including submitted-but-unfilled entry orders.
7. Event persistence, projection updates, and recovery semantics must be made defensible for execution-critical state, either transactionally or through an equally strong design with verified catch-up.
8. Health checks and telemetry must reflect real dependency state, not placeholder assumptions.
9. The runtime must expose operator-grade observability: metrics, structured logs, replay support, state inspection, and drift visibility.
10. End-to-end staging evidence must exist for the live execution path, recovery path, reconciliation path, and risk-blocking path.

Until those conditions are met, model quality is secondary. The runtime is still the system, and the runtime is not yet institution-ready.

## 9. Final Note

This report captures a transitional `v2` snapshot.

It reflects repository-verified current reality, not v3 target architecture. It must be revisited after `v3` stabilization, after `robsond` becomes the primary runtime without stub composition, and after the governance, risk, state, and observability gaps identified here have been closed with repository evidence.
