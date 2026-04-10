# ROBSON v3 — COMPLETE MIGRATION PLAN

**Author**: RBX Systems Architecture  
**Date**: 2026-04-03  
**Status**: APPROVED — Living Document  
**Classification**: Internal / Confidential

---

## 0. PRODUCT DEFINITION

### What Robson v3 Is

Robson v3 is an institutional-grade AI risk management assistant for leveraged cryptocurrency trading, operated by a single operator (Leandro) from Baar, Canton Zug, Switzerland. It enforces position sizing via the Golden Rule (`Position Size = (Capital x 1%) / |Entry - Technical Stop|`), manages position lifecycle (arm -> enter -> trail -> exit), monitors risk in real-time, and provides a complete audit trail of every decision, execution, and risk evaluation. It does NOT generate signals, predict prices, or execute trades autonomously. The operator decides WHEN to trade; Robson decides HOW MUCH and enforces THAT the risk rules are never violated.

### Core Capabilities at Launch

1. **Position lifecycle management**: Arm position with risk parameters -> receive entry signal -> calculate position size -> place entry -> trail stop -> exit on stop/target/manual
2. **Risk enforcement**: Hard limits (max position size, max open positions, max daily loss, max drawdown, max exposure) and circuit breakers that HALT execution on breach
3. **Real-time market data**: Binance WebSocket price feeds with REST fallback
4. **Event-sourced audit trail**: Every state transition, risk decision, and execution produces an immutable event in PostgreSQL
5. **Operator control surface**: Web UI for monitoring, intervention (pause/resume/override/panic), risk adjustment, and replay
6. **CLI**: Terminal commands for all operational actions (arm, disarm, status, panic, credentials)
7. **Isolated margin trading**: Binance isolated margin with automatic transfer management

### What Robson v3 is NOT

- NOT an auto-trader or signal generator (per ADR-0007)
- NOT multi-user (single operator, single tenant for v3)
- NOT a public API or SaaS product
- NOT a high-frequency trading system (human-speed decisions, machine-speed execution)
- NOT a backtesting platform (deferred to v4)
- NOT integrated with TRON/TRC-20 or any blockchain settlement layer (deferred — see Section 10)

### Operator Workflows

**Typical session**:
1. Review market context (funding rates, open interest, volatility regime)
2. Identify opportunity and form thesis (manual, possibly aided by pattern engine)
3. Arm position via CLI or UI: specify symbol, side, capital, risk%, technical stop level
4. Robson calculates position size, validates against risk limits, arms the position
5. When entry signal triggers (detector or manual injection): Robson places entry order
6. Position becomes active: trailing stop monitors in real-time via WebSocket
7. On stop hit or operator intervention: exit order placed, position closed, P&L recorded
8. Full audit trail available for review and replay

**Emergency workflow**: `robson panic` or UI circuit breaker -> all positions closed immediately

### Success Criteria

1. **Zero unaudited executions**: Every order placement traceable to an event chain with risk approval
2. **Risk containment**: No single trade loses more than 1% of capital; no month loses more than 4% drawdown
3. **Execution latency**: Stop-loss exit within 500ms of trailing stop breach (WebSocket path)
4. **System availability**: 99.5% uptime during market hours (24/7 for crypto)
5. **Replay determinism**: Given the same EventLog, system state reconstructs identically

### Regulatory Posture

Robson operates from Baar/Zug under Swiss jurisdiction:

- **DLT Act (2021)**: Mature framework, but Robson v3 does NOT interact with DLT/blockchain — not applicable for v3
- **FINMA classification**: Robson is a private risk management tool for the operator's own account. No third-party assets, no custody, no advisory services to others. This falls OUTSIDE FINMA licensing requirements (no FinTech license, no banking license, no VT Trading Facility license needed)
- **Tax obligations**: Standard Swiss income/capital gains reporting on trading profits. Crypto-AEOI (automatic exchange of information) mandatory since January 2026 — exchange account data already reported by Binance
- **Key requirement before live operations**: Written legal opinion from a Zug-based fintech lawyer confirming that single-operator, own-account trading with AI-assisted risk management does not trigger FINMA licensing. Estimated cost: CHF 2,000-5,000. Must be obtained BEFORE v3 goes live with real capital.

**Failure mode**: Building a technically perfect system that solves no real problem is mitigated by the Golden Rule being the core value proposition — it is a proven risk management technique used at institutional desks. The operator has been trading without it and losing more than 1% per trade. Shipping without regulatory clarity is mitigated by obtaining the legal opinion pre-launch; the risk is LOW given own-account classification, but non-zero if the system is later offered to others.

---

## 1. EXECUTIVE SUMMARY

Robson currently exists as two parallel implementations: a Django monolith (v1, live in production on k3s) and a Rust system (v2, architecturally superior, 12 crates, ~21K LOC). The v1 system has accumulated 31 Django migrations, event-sourced stop monitoring, a 3-level audit trail, margin trading, pattern detection, and an agentic workflow (PLAN->VALIDATE->EXECUTE). The v2 system implements the correct architecture — pure domain layer, event sourcing with ULID ordering, trailing stop engine, risk gate, idempotent executor with intent journal, Binance REST+WS connectors, and a daemon with HTTP API — but still has incomplete projections (40%), no backtesting, and the robsond k3s rollout is not treated as complete in this repository. The migration path is: v2.5 deploys the Rust daemon alongside the Django monolith, with Django continuing to serve the API, frontend, and pattern engine while the Rust runtime assumes execution responsibilities. Once the Rust path is validated in production, the v1 execution CronJobs are disabled and removed from desired state. v3 then promotes the Rust daemon to the primary runtime, replaces Django with a thin API gateway, and the React frontend connects directly to the daemon's event stream. The single most important architectural decision is that the Rust Runtime (robsond) becomes the sole guardian of execution — no context reaches the model, no tool executes, no order places without passing through its governance pipeline. Every other decision flows from this.

In the v3 desired state there are no Django execution CronJobs. Critical monitoring moves to long-lived Rust runtime components such as the Control Loop, Safety Net, and reconciliation workers. Kubernetes CronJobs remain acceptable only for non-critical maintenance jobs such as retention, backfills, or report generation.

---

## 1.1. NOMENCLATURE — AXES AND IDENTIFIERS

This document covers multiple parallel axes of work. To avoid ambiguity,
every reference uses a canonical identifier with a prefix:

| Axis | Prefix | Identifiers | What it is |
|------|--------|-------------|------------|
| **Migration v2 → v2.5** | `MIG-v2.5#` | MIG-v2.5#1 … MIG-v2.5#10 | Migration steps to deploy Rust daemon alongside Django |
| **Migration v2.5 → v3** | `MIG-v3#` | MIG-v3#1 … MIG-v3#8 | Migration steps to promote Rust daemon as primary runtime |
| **QueryEngine phases** | `QE-P` | QE-P1 … QE-P5 | Internal implementation phases of the QueryEngine subsystem (see v3-query-query-engine.md) |
| **Pipeline stages** | `Stage` | Stage 1 … Stage N | Sequential stages within a single control loop cycle (see v3-control-loop.md, v3-runtime-spec.md) |

**Rules**:
- Never use bare "Phase 5" or "step 3" without prefix. Always use the canonical identifier.
- `MIG-v2.5` and `MIG-v3` are sequential migration blocks. `QE-P1`…`QE-P5` are subsystem workstreams.
- `QE-P5` is NOT a migration step; it is a deferred QueryEngine phase (Context Governance, v3+ with LLM).
- `Stage N` is a pipeline stage within a single execution tick (e.g., Stage 1: Observe). Not a project milestone.

**Quick status reference** (as of 2026-04-10, repository-verified):

Status rule for this table: code-backed items may be marked done from repository evidence; operational rollout items stay pending unless the repository contains explicit rollout confirmation.

| ID | Description | Status |
|----|-------------|--------|
| MIG-v2.5#1 | Deploy robsond to k3s | Pending (operational rollout) |
| MIG-v2.5#2 | Complete projector handlers + runtime persistence | Pending (code hardened; awaiting real PostgreSQL execution) |
| MIG-v2.5#3 | Migrate stop monitoring to WebSocket | Pending |
| MIG-v2.5#4 | GovernedAction + Risk Engine blocking gate | ✅ Done (2026-04-04) |
| MIG-v2.5#5 | Circuit breaker escalation ladder | ✅ Done (2026-04-10) |
| MIG-v2.5#6 | SSE endpoint for frontend | ✅ Done (2026-04-05) |
| MIG-v2.5#7 | SOPS secrets management | ✅ Done (2026-04-10) |
| MIG-v2.5#8 | Deploy Prometheus + Grafana + Loki | ✅ Done (2026-04-10) — manifests+ArgoCD; pending cluster sync |
| MIG-v2.5#9 | Contract tests for daemon API | ✅ Done (2026-04-10) — 29 tests |
| MIG-v2.5#10 | EventLog replay determinism test | ✅ Done (2026-04-05) |
| MIG-v3#1 | Promote robsond as primary runtime | Pending |
| MIG-v3#2 | Replace Django API with thin gateway | Pending |
| MIG-v3#3 | Frontend direct connection to SSE | Pending |
| MIG-v3#4 | Dynamic risk limits | Pending |
| MIG-v3#5 | Operator control surface in UI | Pending |
| MIG-v3#6 | Hash-chained EventLog | Pending |
| MIG-v3#7 | PaymentRail trait | Pending |
| MIG-v3#8 | Chaos testing suite | Pending |
| QE-P1 | Passive Wrapper (Non-Breaking) | ✅ Done |
| QE-P2 | Blocking Governance | ✅ Done (2026-04-04) |
| QE-P3 | Approval Gates | ✅ Done (2026-04-05) |
| QE-P4 | Full Audit & Replay | ✅ Done (2026-04-05) |
| QE-P5 | Context Governance (LLM) | Deferred (v3+) |

### MIG-v2.5#2 Technical Notes (2026-04-05)

**Status**: Pending (needs staging validation with real PostgreSQL — see remaining gaps below)

**What was corrected in this session**:

1. **Centralized event persistence in `execute_and_persist()`**: The wrapper now persists events from both `ActionResult::EventEmitted` AND `ActionResult::OrderPlaced { event: Some(_), .. }`. This eliminates the gap where `ExitOrderPlaced` events were lost in the panic_close path.

2. **Correct event ordering**: `ExitOrderPlaced` is persisted BEFORE `PositionClosed` in the exit path. The `execute_and_persist()` function handles this centrally, removing manual persistence logic from call sites.

3. **Removed duplicate manual persistence**: Call sites in `execute_signal_query()` and `process_market_data()` no longer have manual `if let Some(evt) = event { persist_event_to_log(&evt) }` blocks.

4. **Projector fails on unknown events**: `apply_event_to_projections()` now returns `MissingHandler` error instead of `warn+skip`. This prevents silent checkpoint advancement over unhandled events.

5. **Fail-fast write path for position events**: `persist_event_to_log()` now returns `DaemonError::EventLog` on any failure (append or projection apply). `execute_and_persist()` propagates these errors to callers. When `event_log_pool` is configured, failures in append or projection apply abort the current execution cycle. Silent failures are no longer accepted.

6. **Idempotent retry now re-applies projection**: when `append_event()` returns `IdempotentDuplicate`, `persist_event_to_log()` no longer assumes the projection was already updated. It fetches the stored envelope and re-runs `apply_event_to_projections()`. This closes the retry hole where append could succeed and projection apply could fail on the first attempt.

**Consistency mechanism (explicit design decision)**:

The chosen mechanism for MIG-v2.5#2 is the **synchronous fail-fast write path**:

- Position events are written to `stream_key = position:{position_id}` (per-position streams)
- On every `execute_and_persist()` call with `event_log_pool` configured:
  1. Event is appended to `event_log` — failure aborts execution cycle
  2. Projection `positions_current` is updated synchronously — failure aborts execution cycle
  3. If the append already happened on a prior attempt (`IdempotentDuplicate`), the stored envelope is fetched and projection apply is retried
- The ProjectionWorker reads from `config.projection.stream_key` (e.g., `robson:daemon`) for QueryEngine audit events only
- **Position streams are NOT watched by ProjectionWorker today** — this remains a limitation of the current design, not an invisible background repair path
- **Recovery**: reads from `positions_current`, which is updated on the synchronous write path and is no longer allowed to fail silently
- **Important limit**: append and projection apply are still separate steps, so this is fail-fast + retry-healing on duplicate, not a fully atomic guarantee
- **v3 follow-up**: evaluate whether to unify stream strategy or add per-position ProjectionWorker catch-up, or move append+projection into a single transaction-aware path

**Remaining gaps (explicit)**:

1. Staging validation with real PostgreSQL (`DATABASE_URL`) has not been run in this repository
2. ProjectionWorker does not perform catch-up for `position:{id}` streams
3. Runtime e2e test (`test_runtime_arm_position_persists_to_projection`) is compiled and present but marked `#[ignore]` pending `DATABASE_URL` in CI
4. The write path is fail-fast and retry-healing on duplicate, but append + projection apply are still not a single atomic transaction

**2026-04-05 Session Update**:

6. **Added `entry_signal_received` handler to projector**: The engine emits `EntrySignalReceived` as an audit event during entry signal processing. Previously, the projector had no handler for this event type, which would cause a `MissingHandler` error when `event_log_pool` is configured (fail-fast mode). Added `handle_entry_signal_received()` which acknowledges the event without modifying projection state (it's an audit event, state transition is done by `entry_order_placed`).

7. **Added regression test for `entry_signal_received`**: New test `test_entry_signal_received_handled_without_error` in `crash_recovery.rs` verifies that the event is handled without `MissingHandler` error and doesn't change position state.

---

## 2. ARCHITECTURAL DECISIONS TABLE

| # | Decision | Chose | Rejected | Why | Breaks-if-wrong | Version |
|---|----------|-------|----------|-----|-----------------|---------|
| 1 | Runtime language | Rust (robsond) | Keep Django as runtime | Rust gives zero-cost abstractions, type safety on financial types (Decimal), 500ms stop-loss guarantee impossible with Django CronJob (60s granularity). v2 already implements this. | If Rust codebase becomes unmaintainable for solo operator, fallback: freeze Rust binary, add features via Python sidecar | v2.5 |
| 2 | Event store | PostgreSQL append-only (robson-eventlog) | Dedicated event store (EventStoreDB), Kafka | Postgres already deployed (ParadeDB on jaguar), team knows it, single database reduces failure surface. EventStoreDB adds operational complexity for zero benefit at current scale. | If event volume exceeds Postgres capacity (unlikely for single operator), migrate to partitioned tables or EventStoreDB | v2.5 |
| 3 | Risk Engine role | CENTRAL, BLOCKING gate | Advisory/logging only | Financial system. Silent risk violations = financial loss. Engine must BLOCK, not suggest. | If Risk Engine has false positives blocking valid trades, operator loses opportunity. Mitigation: operator override with mandatory audit event | v2.5 |
| 4 | Agent model | Single-agent with tool delegation | Multi-agent with coordination | Single operator, single strategy at a time. Multi-agent adds coordination complexity with zero benefit. Revisit when parallel strategy execution needed. | If operator needs parallel strategies, single-agent becomes bottleneck. Trigger: >3 concurrent armed positions regularly | v2.5 |
| 5 | Frontend architecture | React + Vite (existing) evolved into control surface | Rewrite in Svelte/SolidJS | Frontend exists, works, follows hexagonal pattern. Rewriting is engineering theater. Add SSE/WebSocket for real-time, add operator controls. | If React bundle size or performance becomes issue, unlikely for single-user control surface | v3 |
| 6 | Database consolidation | PostgreSQL only (ParadeDB with pgvector) | PostgreSQL + MongoDB | ParadeDB already deployed with pgvector extension. MongoDB adds a second database with separate failure modes, backup strategy, and operational burden. Nothing in Robson requires document-store semantics that Postgres JSONB cannot handle. | If vector search performance degrades, ParadeDB's pg_search (BM25) and pgvector are purpose-built for this. Fallback: dedicated pgvector index tuning | v2.5 |
| 7 | Message broker | Redis Streams (existing Redis 7) | RabbitMQ, Kafka, SQS | Redis already deployed in production. Redis Streams provides consumer groups, acknowledgment, and persistence. RabbitMQ exists in staging but adds operational complexity. Kafka absurd for single operator. | If Redis Streams loses messages under load, detection: consumer lag monitoring. Mitigation: EventLog is source of truth, replay from log. | v2.5 |
| 8 | Kubernetes | KEEP k3s | Drop to docker-compose/systemd | k3s already running 4 nodes with ArgoCD, cert-manager, Traefik. Dropping it means rebuilding TLS, GitOps, health checks, rolling deploys. The infrastructure investment is already made. Cost: ~EUR 60/mo for 4 VPS nodes. | If k3s becomes operationally burdensome, single-node k3s with local-path storage is the minimum viable cluster | v2.5 |
| 9 | Secrets management | SOPS + age (encrypted in git) | Vault, sealed-secrets, env vars | Single operator. Vault requires its own HA cluster — absurd overhead. SOPS encrypts secrets in git with age keys, decrypts at deploy time. Simple, auditable, no extra infrastructure. | If secrets leak from git (age key compromised), rotate all secrets + API keys. Blast radius: contained to Robson's own accounts | v2.5 |
| 10 | CI/CD | GitHub Actions + ArgoCD (existing) | Jenkins, Tekton, Flux | Already working. Image builds on push, ArgoCD syncs manifests. Zero reason to change. | If GitHub Actions has outage, manual docker build + kubectl apply as fallback. Not worth engineering around. | v2.5 |
| 11 | Observability | OpenTelemetry + Prometheus + Grafana + Loki (self-hosted on k3s) | Datadog, New Relic, managed | Budget-conscious. Self-hosted stack costs EUR 0 beyond existing node resources. Managed observability starts at $50-200/mo and sends financial system telemetry to third parties. | If observability stack consumes too many node resources, reduce retention (7 days for metrics, 30 days for logs) or move Grafana/Loki to jaguar | v2.5 |
| 12 | TRON/TRC-20 | DEFER — architecture TRON-ready but not TRON-active | Adopt for v3, Reject entirely | Regulatory uncertainty (FINMA stablecoin rules 2026), engineering distraction from core product, single dependency on Zero Hash. BUT: $1B AI fund is worth pursuing as funding, and payment rail abstraction costs nothing to build. | If TRON ecosystem becomes dominant for AI agent payments and Robson missed the window, trigger reconsideration when: (a) FINMA issues clear stablecoin guidance, (b) Zero Hash obtains Swiss-compatible license, (c) $1B fund opens applications | DEFERRED |
| 13 | Django monolith fate | Sunset incrementally (v2.5: keep for API/frontend; v3: replace with thin gateway) | Keep Django forever, or kill it immediately | Django served well for v1 but cannot meet 500ms stop-loss SLA (CronJob = 60s). Keeping it as API layer during v2.5 avoids big-bang migration. Killing it immediately breaks the live frontend. | If Django sunset takes too long, it becomes permanent. Mitigation: hard deadline — Django must be fully replaced by v3 launch | v2.5->v3 |
| 14 | Concurrency model | Single control loop, no concurrent cycles | Concurrent cycles with isolation | Single operator, one position lifecycle at a time simplifies reasoning about state. Queue additional signals. | If market moves require simultaneous position management, the queue introduces latency. Trigger: operator regularly has >3 positions that need simultaneous attention | v2.5 |

---

## 3. CONTROL LOOP

The Control Loop is the heartbeat of Robson. It is owned EXCLUSIVELY by the Runtime (robsond). No other component may initiate, pause, or restart a cycle.

### Loop Specification

```
Observe -> Interpret -> Decide -> Act -> Evaluate -> Persist
```

| Step | Owning Component | Input | Output | Deterministic? |
|------|-----------------|-------|--------|----------------|
| **Observe** | Runtime (MarketDataManager) | WebSocket tick, operator command, detector signal, timer event | Typed observation: `MarketTick(symbol, price, ts)`, `OperatorCommand(cmd)`, `DetectorSignal(signal_id, ...)`, `TimerFire(interval_id)` | Deterministic (event parsing) |
| **Interpret** | Runtime (PositionManager) | Observation + current PositionState | Interpretation: `StopBreached(position_id, price)`, `SignalValid(signal_id, position_id)`, `NoAction`, `RiskAlert(kind)` | Deterministic (pure function of state + observation) |
| **Decide** | Engine (robson-engine) | Interpretation + RiskLimits + PositionState | EngineAction: `PlaceEntryOrder`, `UpdateTrailingStop`, `TriggerExit`, `RejectTrade` | Deterministic (pure, no I/O) |
| **Act** | Executor (robson-exec) via ExchangePort | EngineAction + Permission check | ActionResult: `OrderPlaced(order_id)`, `OrderFailed(reason)`, `Blocked(guard)` | Probabilistic (exchange interaction) |
| **Evaluate** | Runtime (PositionManager) | ActionResult + PositionState | New PositionState + domain events | Deterministic (state machine transition) |
| **Persist** | EventLog (robson-eventlog) | Domain events | EventEnvelope(event_id, stream_key, sequence, payload, timestamp) | Deterministic (append-only) |

### Cycle Triggers

| Trigger | Source | Priority |
|---------|--------|----------|
| Market tick (price update) | Binance WebSocket | Normal |
| Detector signal (entry trigger) | External detector or manual injection | Normal |
| Operator command (arm, disarm, panic) | CLI/API | High |
| Risk alert (threshold approach) | Risk Engine monitoring | High |
| Timer (periodic health check) | Internal timer (30s) | Low |
| Circuit breaker activation | Risk Engine | Critical (preempts all) |

### Cycle Interruption

- **Risk threshold breach**: Risk Engine raises `CircuitBreaker` event. Current cycle completes its Evaluate step, then ALL subsequent cycles are blocked until operator acknowledges and resets.
- **System failure**: If any step panics/errors, the cycle logs `CycleError` event with full context and transitions to a safe state (no new orders, existing stops remain on exchange).
- **Manual override**: Operator `pause` command sets a flag checked at the Observe step. Cycles drain gracefully — current cycle completes, no new cycles start until `resume`.

### Concurrency Model

**No concurrent cycles.** Each cycle runs to completion before the next begins. Incoming observations are queued (bounded channel, capacity 1000). If the queue fills (extreme market volatility), oldest non-critical observations are dropped; operator commands and risk alerts are never dropped.

**Rationale**: Single operator, single strategy. Concurrent cycles introduce race conditions on PositionState that require distributed locking — complexity with zero benefit for the use case.

### Cycle Audit

Every completed cycle generates an immutable `CycleCompleted` event in EventLog:

```rust
CycleCompleted {
    cycle_id: Ulid,
    started_at: DateTime<Utc>,
    completed_at: DateTime<Utc>,
    trigger: CycleTrigger,
    observation: Observation,
    interpretation: Interpretation,
    decision: EngineAction,
    action_result: ActionResult,
    risk_state: RiskSnapshot,  // all limits + current values at cycle end
    position_states: Vec<(PositionId, PositionState)>,
}
```

### Failure Modes

| Failure | Impact | Detection | Recovery |
|---------|--------|-----------|----------|
| Loop executes with corrupted state | Wrong position size, wrong trailing stop | State hash comparison against EventLog replay | Halt loop, replay events from last known-good sequence, resume |
| Risk Engine slow (>100ms) | Loop blocks, price moves | Timeout on Risk Engine check (200ms hard limit) | If timeout: DENY action (safe default), log `RiskEngineTimeout` event, alert operator |
| Two cycles race on same state | Impossible by design (sequential model) | N/A | N/A |
| Queue overflow (>1000 pending) | Market observations lost | Queue depth metric, alert at 800 | Acceptable: EventLog has exchange order fills as source of truth for position state. Lost ticks only affect trailing stop granularity temporarily |

---

## 4. RUNTIME

### Runtime Contract

| Aspect | Specification |
|--------|--------------|
| **Input contract** | `RuntimeInput` enum: `MarketTick { symbol, bid, ask, ts }`, `DetectorSignal { signal_id, symbol, side, entry_price, tech_stop }`, `OperatorCommand { cmd, params }`, `OrderFill { order_id, fill_price, fill_qty, ts }`, `Timer { interval_id }` |
| **Output contract** | `RuntimeOutput` enum: `EventsProduced(Vec<DomainEvent>)`, `ActionRequested(Vec<EngineAction>)` after runtime governance clearance, `StateChanged(PositionId, PositionState)`, `Alert(AlertKind, String)` |
| **Internal stages** | Observation -> Inspection -> Risk Gate -> Engine Decision -> Action Governance -> Execution -> Evaluation -> Persistence |
| **State representation** | `RuntimeState { positions: HashMap<PositionId, Position>, risk_snapshot: RiskSnapshot, active_orders: HashMap<OrderId, OrderState>, config: RuntimeConfig, circuit_breaker: CircuitBreakerState }` |
| **Model-agnostic** | Runtime has ZERO coupling to any LLM. The Engine is pure Rust functions. If/when LLM integration is added (v3+), it enters through a `ReasoningPort` trait that the Runtime governs. |

### Zero-Bypass Enforcement

The Runtime enforces governance at the `robsond` boundary today. Stronger cross-crate type-level enforcement remains a target architecture item, not current executor reality:

1. **No raw exchange access from the decision path**: `ExchangePort` remains behind `Executor`. Runtime entry points are wrapped by `QueryEngine` before dispatch.
2. **No ungoverned actions inside `robsond`**: `EngineAction` must pass through `QueryEngine` risk evaluation before executor dispatch. `QueryEngine` uses an internal `GovernedAction` token (`pub(crate)`) to represent "risk-cleared" actions inside the crate.
3. **Current persistence reality**: query lifecycle audit events are appended via `EventLogQueryRecorder`; executor domain events are still persisted via `Store::events().append()`. Convergence to a single durable persistence boundary is follow-up work.

```rust
// Current QE-P2 implementation inside robsond.
pub(crate) struct GovernedAction {
    actions: Vec<EngineAction>,
    _proof: (),  // private token proving QueryEngine approval
}

impl GovernedAction {
    // Private constructor — only QueryEngine can create
    fn new(actions: Vec<EngineAction>) -> Self { ... }
    pub(crate) fn into_actions(self) -> Vec<EngineAction> { ... }
}

// Executor API remains unchanged today.
pub async fn execute(&self, actions: Vec<EngineAction>) -> ExecResult<Vec<ActionResult>> { ... }
```

### Context Management (v3 — when LLM integration is added)

For v3, if the Runtime integrates LLM reasoning (for thesis evaluation, not execution):

- **Inspection**: All context passes through `ContextInspector` which strips sensitive data (API keys, raw credentials) and validates schema
- **Compaction**: `ContextCompactor` summarizes historical events into a bounded window. Strategy: keep last N events verbatim, summarize older events into structured summaries. N = configurable, default 50.
- **Governance**: LLM output is parsed into typed `Suggestion` enum. Only `Suggestion::Observation` and `Suggestion::ThesisUpdate` are accepted. `Suggestion::PlaceOrder` or `Suggestion::OverrideRisk` are REJECTED and logged as `GovernanceViolation` events.

**Note for v2.5**: No LLM integration. The Engine is pure deterministic Rust. This is correct for launch — add LLM reasoning only after the core system is proven.

### Runtime Recovery

If the Runtime crashes mid-cycle:

1. On restart, load last persisted `RuntimeState` from EventLog replay
2. Query Binance for actual position state (reconciliation)
3. If discrepancy: log `ReconciliationEvent`, adopt exchange state as truth (exchange is the real world)
4. Resume control loop from clean state

### Failure Modes

| Failure | Impact | Detection | Recovery |
|---------|--------|-----------|----------|
| Raw context leaks to model | N/A for v2.5/v3 (no LLM) | N/A | N/A — type system prevents at compile time |
| Compaction removes critical context | N/A for v2.5/v3 | N/A | N/A |
| Runtime crashes mid-cycle | Incomplete cycle, potentially orphaned exchange order | Kubernetes liveness probe fails, restarts pod | Reconciliation on restart: compare EventLog state vs exchange state |

---

## 5. BACKEND ARCHITECTURE

### Component Boundaries

| Layer | Responsibility | MUST NOT |
|-------|---------------|----------|
| **Orchestrator** (robsond main loop) | Bootstrap components, manage lifecycle, route inputs to Runtime | Must NOT execute tools, hold mutable state outside Runtime, or bypass Risk Engine |
| **Runtime** (PositionManager + Control Loop) | Own the Control Loop, manage context, enforce governance, coordinate state | Must NOT make trading decisions (that's Engine), access exchange directly (that's Executor via ExchangePort) |
| **Engine** (robson-engine) | Pure decision logic: position sizing, trailing stop, risk gate | Must NOT perform I/O, access database, call exchange. ZERO side effects. |
| **Executor** (robson-exec) | Execute actions cleared by Runtime/QueryEngine on exchange, manage intent journal | Must NOT reason, decide, or self-authorize. The signature still accepts `Vec<EngineAction>` today; governance is enforced before dispatch |
| **Risk Engine** (robson-engine::risk) | Evaluate limits, enforce constraints, trigger circuit breakers | Must NOT be bypassed, overridden by any component, or made advisory-only |
| **EventLog** (robson-eventlog) | Persist immutable events, provide replay | Must NOT mutate events, allow deletion, or serve as query engine (that's projections) |
| **Store** (robson-store) | Persist/recall volatile projections from EventLog | Must NOT be treated as source of truth. Always rebuildable from EventLog |

### Permission System

**Default policy: DENY ALL.**

Every action requires explicit permission. Permissions are scoped:

| Scope | Description | Example |
|-------|-------------|---------|
| Per-session | Valid for current daemon lifetime | `session:read_positions` |
| Per-action | One-time approval for specific action | `action:place_order:BTCUSDT:0.01` |
| Per-risk-level | Elevated permissions for high-risk contexts | `elevated:override_trailing_stop` |

**Human confirmation gates** (actions requiring operator confirmation before execution):

1. `PlaceEntryOrder` when position value > 5% of capital
2. `TriggerExit` when manual (not stop-hit)
3. `AdjustRiskLimits` (any change to hard limits)
4. `CircuitBreakerReset` after activation
5. `PanicClose` (always requires confirmation, even from CLI)

This list is configurable via `robsond.toml`.

**Implementation note (QE-P3 minimum, 2026-04-05)**: the current `robsond`
implementation wires the minimal production path only:
- `PlaceEntryOrder` above 5% of capital is gated
- approval state is kept in memory only for the current daemon lifetime
- operator approval happens via `POST /queries/{id}/approve`
- approval is not a risk override: pending approvals reserve risk, and `approve`
  revalidates the current context before execution
- `disarm` invalidates pending approvals for the same position
- TTL is fixed at 300 seconds
- public SSE exposes `query.awaiting_approval`, `query.authorized`, and `query.expired`
- REST bootstrap exposes pending approvals on `/status`

The broader configurable permission matrix remains the v3 target architecture,
but it is not fully implemented in QE-P3.

**Escalation path**: When permission is denied:
1. Action is BLOCKED (not silently dropped)
2. `PermissionDenied` event logged with full context
3. Alert sent to operator (UI notification + optional webhook)
4. Cycle continues with `ActionResult::Blocked(guard_name)`

**Audit**: Every permission check (granted AND denied) generates an event:
```rust
PermissionCheck {
    cycle_id: Ulid,
    action: String,
    risk_level: RiskLevel,
    decision: PermissionDecision,  // Granted | Denied { reason }
    operator_override: Option<OverrideRecord>,
    timestamp: DateTime<Utc>,
}
```

**Failure mode — permission system crashes**: System HALTS. No actions proceed with last-known permissions. This is the safe default.

**Failure mode — operator grants permission violating risk constraint**: Risk Engine check runs AFTER permission check. Even with operator permission, Risk Engine can still DENY. Both decisions are logged. If operator overrides Risk Engine, this requires a separate `RiskOverride` permission with mandatory audit event containing the operator's stated reason.

### Agent Execution Model

**v2.5 and v3: Single-agent with tool delegation.**

The "agent" is robsond (the daemon). It delegates to:
- Engine (pure decision logic)
- Executor (exchange interaction)
- EventLog (persistence)
- Connectors (market data)

No multi-agent coordination needed. The operator is the strategist; Robson is the executor.

**Trigger for reconsideration**: If the operator regularly runs >3 concurrent strategies requiring independent risk budgets, multi-agent with per-strategy isolation would be warranted. This is not the v3 use case.

### Event Loop Design

**State machine**: The PositionState FSM from robson-domain:

```
Armed -> Entering -> Active -> Exiting -> Closed
  |         |          |         |
  +-> Disarmed   +-> Error  +-> Error
```

**Transitions and guards**:

| From | To | Trigger | Guard |
|------|----|---------|-------|
| Armed | Entering | DetectorSignal received | Signal matches position, Risk Engine approves |
| Armed | Disarmed | Operator disarm command | None (always allowed) |
| Entering | Active | Entry order filled | Fill price within acceptable slippage |
| Entering | Error | Entry order failed/timeout | Retries exhausted (3 attempts) |
| Active | Exiting | Trailing stop breached OR operator exit | Risk Engine confirms (for manual exit) |
| Active | Error | Exchange connection lost >5min | Fallback: REST poll for position state |
| Exiting | Closed | Exit order filled | P&L calculated and recorded |
| Exiting | Error | Exit order failed | CRITICAL: retry immediately, then panic if persistent |

### Failure Modes

| Failure | Blast Radius | Detection | Recovery |
|---------|-------------|-----------|----------|
| Tool execution failure (Binance API error) | Single action blocked | HTTP error code from connector | Retry with exponential backoff (3 attempts). If persistent: log, alert operator, block action |
| Risk Engine slow (>200ms) | Current cycle delayed | Timeout metric | DENY action (safe default), alert operator |
| Orchestrator/Runtime boundary blur | Governance bypass | Code review + trait-based separation at compile time | Rust's type system prevents this if boundaries are enforced via module visibility |

---

## 6. DATA & MEMORY

### EventLog (Immutable, Append-Only)

**Storage**: PostgreSQL (ParadeDB on jaguar:5432)

**Schema**:
```sql
CREATE TABLE event_log (
    event_id        ULID PRIMARY KEY,     -- sortable, time-based
    stream_key      TEXT NOT NULL,         -- partition key (e.g., "position:uuid")
    sequence        BIGINT NOT NULL,       -- per-stream sequence number
    event_type      TEXT NOT NULL,         -- e.g., "entry_filled"
    payload         JSONB NOT NULL,        -- event data
    timestamp       TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    cycle_id        ULID,                  -- links to control loop cycle
    component       TEXT NOT NULL,         -- originating component
    idempotency_key TEXT UNIQUE,           -- SHA256 hash for dedup
    -- CONSTRAINT: no UPDATE or DELETE triggers, append-only enforced at app level
    UNIQUE(stream_key, sequence)
);

CREATE INDEX idx_event_log_stream ON event_log (stream_key, sequence);
CREATE INDEX idx_event_log_type ON event_log (event_type, timestamp);
CREATE INDEX idx_event_log_cycle ON event_log (cycle_id);
```

**Retention**: 7 years minimum (Swiss financial record-keeping, Art. 958f OR). Practical: partition by month, archive to S3 (Contabo) after 1 year, keep hot data for 90 days.

**Replay**: Full system state reconstructable from EventLog alone:
```rust
pub async fn replay_from_log(pool: &PgPool) -> Result<RuntimeState> {
    let events = query_events(pool, QueryOptions::all()).await?;
    let mut state = RuntimeState::empty();
    for event in events {
        state.apply(event)?;
    }
    Ok(state)
}
```

**Integrity**: Hash-chaining deferred to v3. For v2.5, idempotency_key (SHA256 of payload) provides tamper detection for individual events. Full hash chain (`prev_hash` column) adds tamper detection for event ordering — implement when audit requirements are formalized.

### MemoryStore (Volatile Projection)

**What is projected**:
- Active positions (from position lifecycle events)
- Open orders (from order events)
- Current risk snapshot (from risk evaluation events)
- Recent market metrics (from market data events, last 24h)

**Rebuild trigger**: On daemon restart (event-driven replay). Periodic reconciliation: every 5 minutes, compare projection against EventLog tail.

**Staleness detection**: Each projection carries a `last_event_sequence` watermark. If the watermark lags behind EventLog head by >100 events, the projection is flagged stale and triggers rebuild.

### Vector Store (pgvector via ParadeDB)

**What goes in**: Semantic embeddings for market context retrieval, historical pattern signatures, thesis text for similarity search.

**What does NOT go in**: Transactional data, risk state, permissions, EventLog data.

**Retrieval strategy**: Runtime queries vector store when constructing context for thesis evaluation (v3 LLM integration). For v2.5, vector store is populated by pattern engine but not actively queried by Runtime.

**Staleness**: Vector search results include `indexed_at` timestamp. If result is >24h old, it is flagged `potentially_stale` in the response. Runtime can discard or weight accordingly.

### Document Store (MongoDB)

**Decision: REJECT.** Nothing in Robson justifies a second database engine. PostgreSQL JSONB handles all semi-structured data (event payloads, configuration, pattern metadata). ParadeDB adds full-text search (pg_search with BM25) and vector search (pgvector). Adding MongoDB means: second backup strategy, second connection pool, second failure mode, second monitoring target — all for zero capability gain.

### Relational Store (PostgreSQL / ParadeDB)

**Schema boundaries**:

| Schema/Namespace | Content |
|-----------------|---------|
| `public` | EventLog, projections (positions, orders, risk snapshots) |
| `config` | Runtime configuration, risk limits, operator preferences |
| `audit` | Audit trail views, compliance queries |
| `vector` | pgvector indexes for semantic search |

### Failure Modes

| Failure | Detection | Recovery |
|---------|-----------|----------|
| Projections drift from EventLog | Sequence watermark lag >100, periodic reconciliation mismatch | Full projection rebuild from EventLog. Takes <30s for <100K events |
| PostgreSQL fails | Connection pool errors, health check failure | Runtime enters read-only mode (cached state), alerts operator. Recovery: Postgres restart, reconnect, reconcile. RPO: 0 (synchronous writes). RTO: <5min (pod restart) |
| Vector index corrupted | Query returns errors or zero results for known-good queries | Rebuild pgvector index from source data. Not critical path — does not affect trading |

---

## 7. FRONTEND — OPERATOR CONTROL SURFACE

### Architecture

**Stack**: React 18 + Vite (existing). No framework change.

**Evolution from v1 to v3**:
- v2.5: Add SSE endpoint in `robsond` for real-time operator event streaming; frontend still bootstraps via REST
- v3: Frontend connects directly to robsond's SSE/WebSocket endpoint through the gateway; Django is removed from the execution path

**Real-time model**: Server-Sent Events (SSE) for event stream (unidirectional, simpler than WebSocket for event push). WebSocket retained only for Binance market data relay to frontend.

**v2.5 SSE MVP contract (implemented 2026-04-05)**:
```typescript
interface EventStreamMessage {
    schema_version: 1;
    event_id: string;             // UUID v7, uniqueness/dedup only (NOT replay cursor)
    event_type: string;           // "position.changed", "position.opened", etc.
    occurred_at: string;          // Projection timestamp (ISO 8601)
    payload: Record<string, any>; // Event-specific data
}
```

**Current endpoint**: `GET /events`

**Current behavior**:
- REST remains the bootstrap path for snapshots (`/status`, `/positions`, etc.)
- SSE carries incremental operator-facing updates only
- Public stream is mapped from internal daemon events; it does NOT expose `DaemonEvent` directly
- QE-P3 adds approval observability on the same stream via
  `query.awaiting_approval`, `query.authorized`, and `query.expired`
- Keepalive heartbeat is enabled
- On broadcast lag, the daemon emits `system.resync_required` and closes the stream
- `Last-Event-ID` replay/resume is NOT implemented in v2.5

**Current public event types**:
- `position.changed`
- `position.opened`
- `position.closed`
- `safety.rogue_position_detected`
- `safety.exit_executed`
- `safety.exit_failed`
- `safety.panic`
- `system.resync_required`

### Operator Capabilities

| Capability | Description | Latency | Implementation |
|-----------|-------------|---------|----------------|
| **Pause/Resume** | Pause control loop (drain current cycle, block new ones) | < 100ms ack | `POST /api/v1/control/pause` -> sets flag in Runtime |
| **Override** | Override pending decision before execution | Before Act stage | `POST /api/v1/control/override/{cycle_id}` with operator decision |
| **Risk adjustment** | Adjust risk parameters in real-time | Immediate on next cycle | `PUT /api/v1/config/risk-limits` -> updates RuntimeConfig |
| **Event injection** | Inject events (manual observations, corrections) | Persisted in current cycle | `POST /api/v1/events/inject` with event type + payload |
| **Replay** | Replay past events/cycles for audit | On-demand, <30s for 100K events | `GET /api/v1/replay?from={seq}&to={seq}` |
| **Inspect** | View current state: memory, context, risk, permissions | Real-time via SSE | Dashboard widgets bound to SSE stream |
| **Circuit breaker** | Emergency stop (close all positions) | < 100ms | `POST /api/v1/control/panic` (requires confirmation) |

### Observability Surface

**Always visible** (dashboard main view):
- Active positions with current P&L, trailing stop level, distance to stop
- Risk snapshot: current exposure, daily P&L, drawdown, limits headroom
- System health: daemon uptime, WebSocket connection status, last cycle timestamp
- Circuit breaker status: CLOSED (green), OPEN (red), HALF_OPEN (yellow)

**On-demand** (expandable panels):
- Event stream (filterable by type, position, time range)
- Cycle inspector (click any cycle to see full Observe->Persist chain)
- Permission log (recent grants/denials)
- Agent reasoning chain (v3, when LLM is integrated)

### Degradation

| Scenario | UI Behavior | Automated Failsafe |
|----------|------------|-------------------|
| SSE stream drops | Yellow banner: "Connection lost, reconnecting..." Auto-retry with exponential backoff (1s, 2s, 4s, max 30s) | None needed — Runtime continues independently |
| SSE drops during risk event | Red banner: "CONNECTION LOST DURING RISK EVENT — MANUAL CHECK REQUIRED" | If operator unreachable for >5 min during active circuit breaker: Runtime automatically escalates to Level 2 (reduce exposure) |
| WebSocket disconnect (market data) | "Market data delayed" indicator. Prices shown from last known value with stale timestamp | Runtime falls back to REST polling (1s interval) for price data |
| Full backend unreachable | "System Offline" screen with last known state cached in browser localStorage | Runtime continues operating on k8s. Operator can use CLI as fallback |

**Automated failsafe when operator unreachable**: If a risk event (circuit breaker Level 1+) fires and no operator acknowledgment within 5 minutes (no SSE heartbeat from frontend, no CLI command), Runtime escalates one level automatically: L1->L2 (reduce exposure), L2->L3 (close all), L3->L4 (halt + alert via external webhook to operator's phone).

---

## 8. RISK ENGINE

### Architecture Decision: CENTRAL, BLOCKING

The Risk Engine is a mandatory gate in the control loop. Every `EngineAction` passes through the Risk Engine before reaching the Executor. The Risk Engine returns `RiskClearance::Approved` or `RiskClearance::Denied(reason)`. Denied actions are logged but NOT executed.

**Defense of this decision**: In a financial system operated by one person, the Risk Engine is the only thing between a bug and a margin call. Making it advisory ("log and continue") means bugs in the Engine or Executor can drain the account. Making it blocking means the worst case of a Risk Engine bug is a missed trade — annoying but not financially devastating.

**Challenge scenario**: Risk Engine has false positive, blocks a valid trade during a fast move. The operator misses a profitable entry. **Mitigation**: Operator can issue `RiskOverride` which is: (a) logged as an immutable event, (b) requires explicit confirmation, (c) still subject to HARD limits (circuit breakers cannot be overridden, only reset).

### Relationships

```
                    ┌──────────────┐
                    │  Risk Engine │ (robson-engine::risk)
                    │  (CENTRAL)   │
                    └──────┬───────┘
                           │
              ┌────────────┼────────────┐
              │            │            │
              v            v            v
    ┌─────────────┐ ┌───────────┐ ┌──────────┐
    │Safety Net   │ │Control    │ │ EventLog │
    │(independent)│ │Loop       │ │(audit)   │
    └─────────────┘ └───────────┘ └──────────┘
```

**Safety Net**: INDEPENDENT from Risk Engine (defense in depth). Safety Net monitors Binance for rogue positions not opened by Robson and closes them. It does NOT share state with Risk Engine. If Risk Engine fails, Safety Net still operates. If Safety Net fails, Risk Engine still operates. Neither can disable the other.

**Control Loop integration**: Risk Engine evaluates at:
1. **Before Decide**: Pre-check — is the system in a state where ANY action is allowed? (circuit breaker check)
2. **Before Act**: Specific action validation — does THIS action violate any limit?
3. **After Evaluate**: Post-check — did the completed action push any metric close to a limit? (early warning)

### Circuit Breaker Escalation Ladder

| Level | Trigger | Action | Auto-Escalation |
|-------|---------|--------|-----------------|
| **L1: Block New** | Daily loss > 2% OR single position loss approaching 1% | Block new entry orders. Existing positions continue with trailing stops. | Escalate to L2 after 30 min without operator ack |
| **L2: Reduce** | Daily loss > 3% OR max drawdown > 3% | Close 50% of exposure (largest positions first). Block all new orders. | Escalate to L3 after 15 min without operator ack |
| **L3: Close All** | Daily loss > 4% OR any single position loss > 1.5% OR operator unreachable for 45 min during L1/L2 | Emergency close ALL positions at market. System enters L4. | Immediate |
| **L4: Halt** | After L3 execution OR operator `panic` command | System halted. No cycles run. Full audit dump generated. | Requires manual operator reset with typed confirmation |

### Hard Limits (v2.5)

| Limit | Default | Configurable? | Override? |
|-------|---------|---------------|-----------|
| Max position size | 15% of capital | Yes (operator, via config) | Yes, with RiskOverride event |
| Max daily loss | 3% of capital | Yes | NO — hard circuit breaker |
| Max drawdown (monthly) | 4% of capital | Yes | NO — hard circuit breaker |
| Max open positions | 3 | Yes | Yes, with RiskOverride event |
| Max total exposure | 30% of capital | Yes | Yes, with RiskOverride event |
| Max execution frequency | 10 orders/minute | Yes | NO — rate limit |
| Max slippage | 5% per order | Yes | NO — order rejected |

### Dynamic Limits (v3)

| Dynamic Input | Effect on Limits | Fallback |
|--------------|-----------------|----------|
| Realized volatility (20-day) | High vol -> reduce max position size by up to 50% | If vol calculation fails: use hard limits |
| Funding rate extremes | Funding > 0.1%: reduce max exposure for longs | If funding data stale: use hard limits |
| Portfolio correlation | Correlated positions -> reduce combined exposure | If correlation calc fails: treat positions as independent (conservative) |

**Who can override dynamic limits**: Only the operator, with a `DynamicLimitOverride` event that logs: which limit, old value, new value, reason, timestamp. Dynamic overrides expire after 24h (force re-evaluation).

**Fallback**: If dynamic computation fails for ANY reason, system reverts to hard limits immediately. This is logged as `DynamicLimitFallback` event.

### Auditability

Every risk decision is logged:
```rust
RiskDecision {
    cycle_id: Ulid,
    action_evaluated: EngineAction,
    limits_applied: RiskLimits,           // snapshot of current limits
    dynamic_adjustments: Option<DynamicAdjustments>, // if any
    current_exposure: ExposureSnapshot,    // portfolio state at decision time
    verdict: RiskVerdict,                  // Approved | Denied(reason)
    operator_override: Option<OverrideRecord>,
    timestamp: DateTime<Utc>,
}
```

### Failure Modes

| Failure | Decision | Rationale |
|---------|----------|-----------|
| Risk Engine crashes | System HALTS all activity | Continuing without risk checks is equivalent to no seatbelt. Halt is safe; continuing is dangerous. |
| Dynamic limits from stale data | Revert to hard limits | Hard limits are conservative by design. Stale dynamic data could be manipulated or wrong. |
| Max staleness window for dynamic data | 5 minutes | Beyond 5 min, market conditions may have changed materially. Hard limits take over. |
| Operator overrides risk constraint | Logged with full context, allowed for soft limits, BLOCKED for hard limits (daily loss, drawdown, slippage, rate limit) | Some limits exist to protect the operator from themselves. Daily loss and drawdown limits are non-negotiable even for the operator. |

---

## 9. CLOUD & INFRASTRUCTURE

### Compute: KEEP k3s

**Decision**: Keep the existing 4-node k3s cluster.

**Justification**: The cluster is already provisioned, running production workloads, with ArgoCD, cert-manager, Traefik, and Let's Encrypt all configured. Dropping k3s to docker-compose saves approximately EUR 0/month (same VPS costs) while losing: automated TLS renewal, GitOps deploys, health checks with auto-restart, rolling updates, namespace isolation, resource limits. The investment is sunk; the operational benefit is real.

**Minimal viable cluster**:
- tiger (control plane + workloads): robsond, frontend, Redis
- jaguar (database): ParadeDB (Postgres 16 with pgvector)
- altaica + sumatrae: available for redundancy or additional workloads

**Cost**: ~EUR 60/month total (4x Contabo VPS at ~EUR 15/mo each).

### Observability Stack

| Component | Purpose | Self-hosted | Version |
|-----------|---------|-------------|---------|
| OpenTelemetry Collector | Instrumentation ingress | Yes (k3s DaemonSet) | v2.5 |
| Prometheus | Metrics storage + alerting | Yes (k3s StatefulSet on jaguar) | v2.5 |
| Grafana | Dashboards + alerting UI | Yes (k3s Deployment) | v2.5 |
| Loki | Log aggregation | Yes (k3s StatefulSet on jaguar) | v2.5 |
| Tempo | Distributed tracing | Yes (k3s Deployment) | v3 (deferred — add when multi-component tracing becomes necessary) |

**Resource budget**: Prometheus + Grafana + Loki together should consume <1GB RAM, <500m CPU. Deployed on jaguar alongside Postgres.

**Monitoring model**: Critical runtime monitoring is continuous, not scheduled. robsond owns the Control Loop, Safety Net, exchange reconciliation, and market-data failover as long-lived processes. New Kubernetes CronJobs are allowed only for non-critical housekeeping such as retention, backfills, and offline reports.

### CI/CD

**Decision**: GitHub Actions + ArgoCD. Already working. No changes needed.

ArgoCD is justified because it is ALREADY DEPLOYED and configured. If starting fresh, a simpler `kubectl apply` from GitHub Actions would suffice. But removing ArgoCD now would be work for negative value.

### Secrets: SOPS + age

**v2.5**: SOPS with age encryption. Secrets encrypted in git, decrypted by ArgoCD or CI/CD pipeline using age private key stored on the k3s control plane.

**Setup**:
1. Generate age key pair: `age-keygen -o key.txt`
2. Store private key on tiger: `/etc/sops/age/keys.txt`
3. Create `.sops.yaml` in repo with age public key
4. Encrypt secrets: `sops --encrypt --age <public-key> secrets.yaml > secrets.enc.yaml`
5. ArgoCD decrypts using KSOPS plugin or helm-secrets

**Rotation**: API keys rotated quarterly. Age key rotated annually.

### Cost Estimate

| Item | v2.5 (monthly) | v3 (monthly) |
|------|----------------|--------------|
| 4x Contabo VPS (tiger, altaica, sumatrae, jaguar) | EUR 60 | EUR 60 |
| Contabo S3 (backups, datalake) | EUR 5 | EUR 10 |
| Domain names (rbxsystems.ch, rbx.ia.br) | EUR 3 | EUR 3 |
| GitHub (free tier) | EUR 0 | EUR 0 |
| Let's Encrypt certificates | EUR 0 | EUR 0 |
| **Total** | **EUR 68** | **EUR 73** |

**Cost ceiling**: EUR 150/month. Beyond this, reassess infrastructure choices.

**Top 3 cost drivers**: VPS nodes (87%), S3 storage (7%), domains (4%).

### Single Points of Failure

| SPOF | Impact | Mitigation |
|------|--------|------------|
| jaguar (Postgres) | All data lost if disk fails | Daily pg_dump to S3. WAL archiving for point-in-time recovery. RTO: 1h (restore from backup). RPO: 24h (daily backup) or 5min (with WAL). |
| tiger (k3s control plane) | Cluster management lost, workloads continue running but no rescheduling | etcd backup daily. Recovery: reinstall k3s, restore etcd, rejoin workers. RTO: 2h. |
| Binance API | No trading, no market data | Fallback: read-only mode. Positions remain on exchange with their stop orders. No Robson action needed. |
| Single operator | System needs human for circuit breaker reset, risk overrides | Automated escalation ladder (see Risk Engine). System can self-protect for hours without operator. |

### Monitoring Blind Spots

| Blind Spot | What Could Happen | Detection Gap | Mitigation |
|-----------|-------------------|--------------|------------|
| Disk space on jaguar | Postgres WAL fills disk, database crashes | Add Prometheus node_exporter disk alert at 80% | v2.5: deploy node_exporter on all nodes |
| Certificate expiry | TLS cert expires, HTTPS fails | cert-manager handles renewal, but monitor for renewal failures | Add Prometheus cert-manager alert |
| Binance API rate limits | 429 responses, degraded service | Add metric for Binance API response codes | v2.5: add rate limit tracking in robson-connectors |

---

## 10. EXTERNAL SERVICES & TRON DECISION

### Service Decisions

| Category | Candidates | Decision | Rationale |
|----------|-----------|----------|-----------|
| Transactional email | Postmark / SES | **DEFER** | No user-facing email needed for single operator. Trigger: if Robson becomes multi-user or needs alerting beyond webhook |
| Message queues | Redis Streams / RabbitMQ | **ADOPT Redis Streams (v2.5)** | Redis already deployed. RabbitMQ in staging is over-engineered for single operator. Remove RabbitMQ from staging. |
| Data pipelines | Airflow | **REJECT** | Airflow is heavy infrastructure for batch jobs that can be K8s CronJobs. The existing bronze-ingest and silver-transform jobs are simple enough as CronJobs. |
| Model lifecycle | MLFlow | **DEFER** | No ML models in v3. Trigger: when Robson adds ML-based signal generation or dynamic risk models |
| Stablecoin rails | TRON TRC-20 (USDT) | **DEFER** — see below | Regulatory + engineering complexity exceeds v3 scope |
| Agentic finance funding | TRON DAO $1B AI fund | **PURSUE AS FUNDING** — separate from product integration | Fund application is a business activity, not an engineering dependency |

### TRON / TRC-20 — DECISION: NO for v3 product, YES for architecture readiness, PURSUE as funding

#### Product Integration: NO

TRON/TRC-20 is NOT part of Robson v3 architecture. Reasons:

1. **Regulatory uncertainty**: FINMA's 2026 stablecoin regulation requires 100% backing + licensing for stablecoin issuers and potentially for platforms that integrate stablecoin settlement rails. Robson operating stablecoin rails (even via Zero Hash) could trigger FinTech licensing requirements, transforming a simple own-account tool into a regulated entity. The legal opinion needed (see Section 0) would need to cover this, adding cost and delay.

2. **Engineering distraction**: TRON integration (TronWeb, Solidity contracts, wallet management, key custody) is a full workstream that does not contribute to the core value proposition (risk management for trading). Every hour spent on TRON is an hour not spent on the Risk Engine, EventLog, or control surface.

3. **Zero Hash dependency**: Zero Hash's Swiss regulatory status is unclear. Building on Zero Hash creates a dependency on a third party's licensing timeline. If Zero Hash doesn't obtain Swiss-compatible coverage, the integration is worthless.

4. **No user need**: The operator trades on Binance. Binance handles settlement. There is no settlement problem to solve in v3.

#### Architecture Readiness: YES

The architecture should include a `PaymentRail` trait (port) that abstracts settlement:

```rust
#[async_trait]
pub trait PaymentRail: Send + Sync {
    async fn transfer(&self, amount: Decimal, destination: &str) -> Result<TransferReceipt>;
    async fn balance(&self) -> Result<Decimal>;
    async fn status(&self, transfer_id: &str) -> Result<TransferStatus>;
}
```

This costs nothing to define and allows future implementation of `TronPaymentRail`, `BankTransferRail`, `BinanceSettlementRail`, etc. without architectural changes.

#### Funding: PURSUE

The TRON DAO $1B AI fund (March 2026) targets AI agent infrastructure. Robson fits the "AI agent for finance" category. Applying for a grant is a business activity with no engineering dependency. The application can reference Robson's architecture (event-sourced, runtime-governed AI agent) without requiring TRON integration in the product.

**Action item**: Prepare grant application highlighting Robson's agentic architecture. Mention TRON-readiness (PaymentRail abstraction). Apply by Q3 2026.

#### Trigger Conditions for Reconsideration

Reconsider TRON integration when ALL of these are true:
1. FINMA issues clear guidance on stablecoin integration for own-account trading tools
2. Zero Hash obtains Swiss-compatible license (or equivalent regulated bridge)
3. Operator has a concrete use case for stablecoin settlement (e.g., treasury management, cross-exchange transfers)
4. v3 core system is stable and operational for >3 months

#### Failure Modes

| Failure | Impact | Mitigation |
|---------|--------|------------|
| FINMA restricts TRC-20 after adoption | Would need to rip out integration | Not adopted — no impact for v3 |
| Zero Hash changes terms/shuts down | Would need alternative bridge | Not adopted — no impact for v3 |
| Missed $1B fund window | Lost potential funding | Apply for grant regardless of product integration decision |

---

## 11. TESTING & VALIDATION

### Testing Layers

| Layer | What | Tool | When | Status |
|-------|------|------|------|--------|
| **Unit** | Domain entities, Engine logic, Risk calculations | `cargo test` (Rust), `pytest` (Django during v2.5) | Every commit | v2 domain+engine: good coverage. v1: decent coverage. |
| **Integration** | Component interactions, EventLog append/replay, Executor+Exchange | `cargo test --features integration` + testcontainers (Postgres) | Every PR | v2: partial (projector stubs). Needs completion. |
| **Contract** | API contracts (CLI<->daemon, frontend<->backend), event schemas | JSON Schema validation + OpenAPI spec tests | Every PR | Not yet implemented. **v2.5 priority.** |
| **Risk Engine** | Threshold behavior, circuit breakers, escalation ladder, edge cases | Dedicated `risk_engine_tests` module with property-based testing | Every PR + nightly | v2: RiskLimits tested. Circuit breaker ladder: NOT YET. **v2.5 priority.** |
| **Replay** | EventLog replay produces identical state | Custom harness: insert events, replay, compare state hash | Nightly | v2: basic replay exists. Determinism assertion: NOT YET. **v2.5 priority.** |
| **Chaos** | Component failures, slow Risk Engine, exchange timeouts | tokio::test with injected delays + mock failures | Weekly / pre-release | NOT YET. **v3 priority.** |
| **Regulatory** | Audit trail completeness, permission logging, data retention | Custom validators: for each event type, verify all required fields present | Pre-release | NOT YET. **v3 priority.** |

### Specific Requirements

1. **Risk Engine**: 100% branch coverage on all threshold and circuit breaker logic. Tested via property-based testing (`proptest` crate): for random portfolio states and random market conditions, verify that risk limits are NEVER violated.

2. **EventLog replay determinism**: `replay_test` harness: insert 10,000 events, replay from scratch, compare resulting state byte-for-byte with state produced during original insert. Must pass 100% of the time.

3. **Permission denial testing**: For each permission gate, a test case where: given context X and risk level Y, action Z is denied. Negative tests are as important as positive tests.

4. **Minimum coverage threshold for v3 ship**:
   - Domain + Engine: 95%
   - Risk Engine (including circuit breakers): 100% branch coverage
   - EventLog: 90%
   - Executor: 80% (exchange interactions are inherently non-deterministic)
   - Frontend: 60% (UI components are lower priority than backend correctness)

### Failure Mode: Risk Engine Bug Under Specific Market Conditions

**Detection**: Property-based testing explores edge cases (zero price, negative funding, max leverage). Nightly replay tests with historical market data detect divergence. Circuit breaker monitoring in production detects unexpected behavior.

**Recovery**: If a bug is found in production:
1. Activate L4 circuit breaker (halt all activity)
2. Export EventLog for the affected period
3. Identify the bug via replay with instrumentation
4. Fix, test, deploy via normal CI/CD
5. Replay events to verify fix produces correct state
6. Resume operations after operator confirmation

---

## 12. MIGRATION PLAN

### MIG-v2.5: v2 → v2.5 (Incremental, Non-Breaking, De-Risks v3)

| ID | Change | Why It Cannot Wait | Depends On | Effort | Reversible? | Rollback | Breaks If Skipped |
|----|--------|--------------------|-----------|--------|-------------|----------|-------------------|
| MIG-v2.5#1 | **Deploy robsond to k3s alongside Django** | Rust daemon must be running before it can take over stop monitoring. Deploy first, verify stability. | K8s manifests (exist in v2/k8s/) | S | Yes — undeploy pod | `kubectl delete deployment robsond` | v3 has no production-proven daemon |
| MIG-v2.5#2 | **Complete projector handlers** (robson-projector, currently 40%) | Without projections, Runtime cannot reconstruct state on restart | MIG-v2.5#1 | M | Yes — code change only | Revert commit | Runtime loses state on restart, requires full EventLog replay every time |
| MIG-v2.5#3 | **Migrate stop monitoring from Django CronJob to robsond WebSocket** | CronJob has 60s granularity. WebSocket achieves <500ms. This is the core latency improvement. | MIG-v2.5#1, MIG-v2.5#2 | M | Yes — re-enable CronJob | Re-enable `rbs-stop-monitor-cronjob`, disable robsond stop monitor | Stop-loss latency stays at 60s, unacceptable for leveraged trading |
| MIG-v2.5#4 | **Implement GovernedAction + Risk Engine as blocking gate** | Risk Engine is currently in robson-engine but not wired as mandatory gate. Must block before any v3 feature relies on it. | MIG-v2.5#1 | M | Yes — revert to advisory mode | Config flag: `risk_engine_mode: advisory` | Risk Engine bypass possible, defeating the entire safety architecture |
| MIG-v2.5#5 | **Implement circuit breaker escalation ladder (L1-L4)** | Without escalation, a single circuit breaker trip halts the system with no graceful degradation | MIG-v2.5#4 | M | Yes — revert to simple halt | Remove escalation, revert to single-level circuit breaker | Operator must manually intervene for every risk event, no automated protection |
| MIG-v2.5#6 | **Add SSE endpoint to robsond for frontend event streaming** — ✅ DONE 2026-04-05 | Frontend needs real-time data from daemon, not Django polling | MIG-v2.5#1 | S | Yes — frontend falls back to REST polling | Remove SSE route, frontend uses REST | Frontend has no real-time capability, operator flies blind |
| MIG-v2.5#7 | **Implement SOPS for secrets management** | Current secrets are K8s secrets (base64, not encrypted at rest in git). SOPS encrypts in git. | None | S | Yes — revert to plain K8s secrets | Remove .sops.yaml, restore template secrets | Secrets remain unencrypted in git templates (security risk) |
| MIG-v2.5#8 | **Deploy Prometheus + Grafana + Loki on k3s** | Observability is non-negotiable before v3. Cannot debug production issues without metrics/logs. | k3s cluster (exists) | M | Yes — undeploy | `kubectl delete namespace monitoring` | Flying blind in production. Debugging via `kubectl logs` only. |
| MIG-v2.5#9 | **Contract tests for daemon API** | CLI and frontend depend on daemon API stability. Breaking changes must be caught in CI. | MIG-v2.5#1 | S | Yes — remove test suite | Revert CI config | API breaks silently, CLI/frontend stop working after daemon update |
| MIG-v2.5#10 | **EventLog replay determinism test** — ✅ DONE 2026-04-05 | Must prove replay works before relying on it for state recovery | EventLog implemented (exists in v2) | S | Yes — remove test | N/A | Cannot guarantee state recovery correctness |

### MIG-v3: v2.5 → v3 (Architectural Evolution)

| ID | Change | Replaces from v2.5 | Precondition | Effort | Reversible? | Rollback | Breaks If Done Wrong |
|----|--------|-------------------|-------------|--------|-------------|----------|---------------------|
| MIG-v3#1 | **Promote robsond as primary runtime** (all execution goes through daemon) | Django stop monitor CronJob | MIG-v2.5#1–#4 complete, daemon stable for >2 weeks in prod | M | Yes — rollback to legacy runtime | Suspend robsond execution path, restore legacy runtime, redirect frontend to Django API if needed | Execution path broken if daemon has undiscovered bugs |
| MIG-v3#2 | **Replace Django API with thin gateway** (FastAPI or axum) that proxies to robsond | Django REST API | MIG-v3#1, MIG-v2.5#6 | L | Partially — can re-enable Django | Redeploy Django, update ingress routing | Frontend/CLI break if gateway has bugs; both have robsond as direct fallback |
| MIG-v3#3 | **Frontend direct connection to robsond SSE** | SSE via Django proxy | MIG-v3#2 | S | Yes — revert to Django proxy | Update frontend VITE_API_BASE_URL to Django endpoint | Frontend loses real-time if SSE path fails; graceful degradation to REST |
| MIG-v3#4 | **Dynamic risk limits** (volatility-adjusted, funding-rate-aware) | Hard limits only | MIG-v2.5#4, market data pipeline working | M | Yes — disable dynamic, use hard limits | Config: `dynamic_limits_enabled: false` | False sense of security if dynamic computation is wrong; fallback to hard limits is safe |
| MIG-v3#5 | **Operator control surface** (pause/resume/override in UI) | CLI-only operator interaction | MIG-v3#3, MIG-v2.5#6 | M | Yes — controls are additive | Remove UI controls, operator uses CLI | Operator must use CLI for all interventions, slower response in emergencies |
| MIG-v3#6 | **Hash-chained EventLog** for tamper detection | Plain EventLog | EventLog stable, MIG-v2.5#10 ✅ DONE 2026-04-05 | S | Yes — stop computing hashes | Remove hash column, revert to plain append | Audit trail tampering undetectable; acceptable for single operator, problematic if audited |
| MIG-v3#7 | **PaymentRail trait** (architecture readiness for future settlement) | None | None (pure interface definition) | S | Yes — delete trait | Remove trait definition | No impact on v3; delays TRON readiness if ever needed |
| MIG-v3#8 | **Chaos testing suite** | No chaos testing | All components deployed and stable | M | Yes — disable tests | Remove chaos test suite from CI | Undiscovered failure modes in production; acceptable risk if monitoring is good |

### Migration Rules

1. **Every migration step generates an immutable event**: `MigrationStepStarted`, `MigrationStepCompleted`, `MigrationStepRolledBack` in EventLog.
2. **Data migration risk**: Steps #1-#6 in v2.5 do NOT migrate data. The Django database remains untouched. robsond creates its own EventLog in Postgres. The two systems coexist during v2.5, but only one execution path may be active for live stop/trailing responsibilities at a time.
3. **Build order** (foundation first):
   - FOUNDATION: MIG-v2.5#1 + MIG-v2.5#7 + MIG-v2.5#8 + **QE-P1** (passive wrapper)
   - GOVERNANCE: MIG-v2.5#2 + MIG-v2.5#4 + **QE-P2** (blocking governance)
   - EXECUTION CUTOVER: MIG-v2.5#3 + MIG-v2.5#5
   - OPERATOR FEEDBACK: MIG-v2.5#6 + **QE-P3** (approval gates)
   - VERIFICATION: MIG-v2.5#9 + MIG-v2.5#10 / **QE-P4** (full audit & replay)
   - See v3-query-query-engine.md for complete QueryEngine implementation timeline (QE-P1…QE-P5)
4. **Parallelizable**: MIG-v2.5#1 + MIG-v2.5#7 + MIG-v2.5#8 + QE-P1 can run in parallel. MIG-v2.5#2 + MIG-v2.5#4 + QE-P2 can run in parallel after MIG-v2.5#1. MIG-v2.5#9 + MIG-v2.5#10 can run in parallel after MIG-v2.5#1.
5. **No parallel execution authorities**: v3 never runs Django execution CronJobs in parallel with robsond. Rollback is mutually exclusive.
6. **Deferred to post-v3**:
   - Backtesting (robson-sim): Trigger — operator wants to validate strategies before arming
   - Multi-user support: Trigger — second operator joins
   - ML-based signals: Trigger — operator has validated ML model with backtesting
   - TRON integration: Trigger — regulatory clarity + concrete use case
7. **Explicitly out of scope**:
   - Auto-trading (ADR-0007 — Robson is an assistant, not a bot)
   - Public API / SaaS (single operator system)
   - Mobile app (web UI is sufficient for desk trading)

---

## 13. CRITICAL RISKS & UNKNOWNS

| # | Risk | Likelihood | Impact | Mitigation | Owner |
|---|------|-----------|--------|------------|-------|
| 1 | **Risk Engine bug allows trade exceeding limits** | Low | Critical (financial loss) | 100% branch coverage, property-based testing, circuit breaker as independent safety net, Safety Net monitors exchange directly | Leandro (architect + operator) |
| 2 | **Rust daemon instability in production** (memory leak, panic, deadlock) | Medium | High (system downtime) | Deploy alongside Django first (v2.5), monitor for 2+ weeks before promoting. Kubernetes auto-restart on liveness failure. | Leandro |
| 3 | **Single-operator bus factor** | High | Critical (system unmaintained if operator unavailable) | Automated escalation ladder (L1->L4). System self-protects for hours. Documentation sufficient for another engineer to operate. | Leandro |
| 4 | **Binance API changes break connectors** | Medium | High (trading halted) | Pin Binance API version, monitor deprecation notices. REST+WS fallback pattern. Connector is isolated behind trait — swap implementation without changing Runtime. | Leandro |
| 5 | **FINMA regulatory surprise** (own-account trading with AI requires licensing) | Low | High (must halt operations or obtain license) | Obtain legal opinion BEFORE v3 goes live. Budget CHF 2,000-5,000. If licensing required, evaluate cost/benefit of FinTech license (CHF 30K+ setup). | Leandro + legal counsel |

---

## 14. FINAL RECOMMENDATIONS

### Current Recommended Next Steps

The repository already shows `MIG-v2.5#4`, `MIG-v2.5#6`, `MIG-v2.5#10`, and `QE-P1` through `QE-P4` implemented. The next actions should stay inside the pending `MIG-v2.5` items:

1. **MIG-v2.5#1 — Operational rollout of robsond to k3s**: create the production overlay, deploy the daemon, and verify `healthz`/`readyz` under the real namespace.
2. **MIG-v2.5#2 — Complete projector handlers**: close the projection/recovery gaps so restart semantics match the real runtime event flow.
3. **MIG-v2.5#3 — Migrate stop monitoring to robsond WebSocket**: only after `MIG-v2.5#1` and `MIG-v2.5#2` are accepted.
4. **MIG-v2.5#5 — Implement the circuit breaker escalation ladder**: build on top of the already-implemented blocking gate.
5. **MIG-v2.5#7, MIG-v2.5#8, MIG-v2.5#9 — Finish the ops baseline**: SOPS, observability, and daemon API contract tests.

### Explicitly NOT Building Yet

1. **TRON/TRC-20 integration** — Trigger: regulatory clarity + concrete settlement need
2. **Backtesting (robson-sim)** — Trigger: operator wants pre-arm strategy validation
3. **Dynamic risk limits** — Trigger: hard limits proven stable for >1 month in production
4. **LLM integration for thesis evaluation** — Trigger: core system stable for >3 months, clear value proposition defined
5. **Multi-agent coordination** — Trigger: >3 concurrent strategies needed regularly

### The 1 Decision That Determines Whether v3 Succeeds or Fails

**The Rust Runtime (robsond) must become the sole guardian of execution.**

Once robsond is deployed, stable, and enforcing the Risk Engine as a blocking gate, everything else follows: the EventLog captures truth, the projections derive from truth, the frontend displays truth, the operator intervenes on truth. If robsond fails to become the authority — if Django remains in the execution path, if the Risk Engine remains advisory, if events bypass the log — then v3 is a collection of components, not a system. The entire migration hinges on this one transition: from Django CronJob executing stops every 60 seconds, to robsond governing every action in real-time with full audit.

Ship this. Everything else is sequencing.
