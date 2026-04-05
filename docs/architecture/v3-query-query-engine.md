# ROBSON v3 — QUERY & QUERYENGINE SPECIFICATION

**Date**: 2026-04-04  
**Revised**: 2026-04-04 (post-review: fan-out model, persistence ownership, type corrections)  
**Status**: APPROVED — Ready for Implementation  
**Owner**: Runtime (robsond)  
**Companion to**: v3-migration-plan.md, v3-control-loop.md, v3-runtime-spec.md

---

## 0. ARCHITECTURAL PREMISE

### state = source of truth, stream = projection

This document formalizes the introduction of `ExecutionQuery` and `QueryEngine` as the governed execution core within the Runtime (robsond). The design rests on one non-negotiable principle:

**`state = source of truth`** — The canonical persisted execution state owned by the Runtime is the operational authority for all real-time decisions.

**`stream = projection`** — Every derived view (projections, SSE events, audit streams, UI state) is computed from the durable truth, never the reverse.

### How This Resolves the EventLog Tension

The v3 architecture defines two truth planes that are complementary, not competing:

| Plane | Authority | Mechanism | Purpose |
|-------|-----------|-----------|---------|
| **Operational** | `Store` (v2) / `RuntimeState` (v3 target) | MemoryStore or PgStore, queried by PositionManager | Real-time decisions (<500ms) |
| **Durable** | `EventLog` | PostgreSQL append-only (robson-eventlog) | Replay, recovery, audit, compliance |
| **Derived** | `Projections / Streams` | Computed from EventLog | UI, monitoring, SSE, read queries |

The relationship is unidirectional and strict:

```
Store / RuntimeState (operational truth)
  -> EventLog (durable truth, persisted from state transitions)
    -> Projections / Streams (derived, always rebuildable)
```

On restart, state is reconstructed from EventLog replay (v3-runtime-spec.md, Recovery Scenario 1). During runtime, state is the authority. EventLog is the durability mechanism. Projections are always derived.

**v2 reality vs v3 target**: The v2 codebase does NOT yet have a `RuntimeState` struct. Operational state lives in `Store` (MemoryStore/PgStore) queried via repository traits. The Executor persists events AND applies them to Store internally via `store.events().append()` + `store.positions().apply_event()`. Phase 1 of QueryEngine works with this existing model. `RuntimeState` as an explicit in-memory struct is a Phase 2+ concern.

This is NOT a departure from event sourcing. It is a clarification: events are the durable persistence format, but the runtime does not query the EventLog to make decisions. It acts on state.

---

## 1. PROBLEM STATEMENT

### Current Fragmentation in robsond

Today, execution lifecycle is distributed across multiple concerns without a unified typed unit:

1. **Business lifecycle** lives in `PositionState` (Armed -> Entering -> Active -> Exiting -> Closed) — `robson-domain/src/entities.rs`
2. **Idempotency lifecycle** lives in `IntentStatus` (Pending -> Executing -> Completed) — `robson-exec/src/intent.rs`
3. **Runtime-cycle state** is implicit in `PositionManager` call stacks — `robsond/src/position_manager.rs`
4. **Risk evaluation** is in `robson-engine/src/risk.rs` but not yet wired as a mandatory blocking gate
5. **Two parallel event models** exist: `robson_domain::Event` via Store, and `EventEnvelope` via robson-eventlog
6. **API handlers** can arm, inject signals, and panic-close directly into runtime methods with no explicit governance boundary — `robsond/src/api.rs`

There is no single typed execution unit that:
- Represents one complete trigger-to-completion lifecycle
- Carries governance proof (risk clearance, approval status)
- Is auditable end-to-end
- Can be replayed deterministically

### What QueryEngine Solves

The QueryEngine is the **implementation of the Control Loop** (v3-control-loop.md). It formalizes the `Observe -> Interpret -> Decide -> Act -> Evaluate -> Persist` pipeline as a concrete Rust module with:

- A single entry point for ALL runtime triggers
- A typed lifecycle unit (`ExecutionQuery`) for each trigger
- Explicit state machine progression
- Mandatory risk gate (when wired in Phase 2)
- Structured audit trail per query
- Clear ownership: QueryEngine sits INSIDE the Runtime, not alongside it

---

## 2. DESIGN

### 2.1 ExecutionQuery — The Typed Lifecycle Unit

**File**: `v2/robsond/src/query.rs`

The name `ExecutionQuery` is intentional — it avoids collision with `robson-eventlog/src/query.rs` (database read queries) and communicates that this is an execution lifecycle concept, not a data retrieval concept.

```rust
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use uuid::Uuid;

/// A typed execution lifecycle unit.
/// Every trigger that enters the Runtime becomes one or more ExecutionQueries.
/// For single-position triggers (signal, disarm): one query per trigger.
/// For fan-out triggers (market tick, panic): one query PER POSITION affected.
/// This is the control-loop unit: it tracks one complete
/// Observe -> Interpret -> Decide -> Act -> Evaluate -> Persist cycle.
pub struct ExecutionQuery {
    /// Unique identifier (UUID v7, time-ordered)
    pub id: Uuid,
    /// What triggered this query
    pub kind: QueryKind,
    /// Current lifecycle state
    pub state: QueryState,
    /// Who/what initiated this query
    pub actor: ActorKind,
    /// Associated position (if applicable)
    pub position_id: Option<PositionId>,
    /// When this query was created
    pub started_at: DateTime<Utc>,
    /// When this query reached a terminal state
    pub finished_at: Option<DateTime<Utc>>,
    /// Final outcome (set when Completed or Failed)
    pub outcome: Option<QueryOutcome>,
    /// Lightweight context summary (for audit, not for decisions)
    pub context_summary: Option<ContextSummary>,
}

/// What triggered the execution query.
///
/// IMPORTANT: Fan-out triggers (ProcessMarketTick, PanicClose) do NOT become
/// a single query. The PositionManager creates one ExecutionQuery PER POSITION
/// affected. This preserves per-position auditability and outcome tracking.
///
/// Types match v2 codebase exactly:
/// - signal_id: Uuid (not Ulid — v2 uses uuid::Uuid everywhere)
/// - PositionId: Uuid
/// - Price, Symbol, Side, Quantity: from robson_domain
pub enum QueryKind {
    /// Detector fired an entry signal (one query, one position)
    ProcessSignal {
        signal_id: Uuid,
        symbol: Symbol,
        side: Side,
        entry_price: Price,
        stop_loss: Price,
    },
    /// Market price update for ONE active position
    /// (PositionManager creates one query per active position matching the symbol)
    ProcessMarketTick {
        symbol: Symbol,
        price: Price,
    },
    /// Operator requests position arming
    ArmPosition {
        symbol: Symbol,
        side: Side,
        tech_stop_distance: TechnicalStopDistance,
        account_id: Uuid,
    },
    /// Operator requests position disarming
    DisarmPosition {
        position_id: PositionId,
    },
    /// Emergency close ONE position (PanicClose creates one query per position)
    PanicClosePosition {
        position_id: PositionId,
    },
    /// Safety Net detected rogue position
    SafetyNetExit {
        position_id: PositionId,
        reason: String,
    },
    /// Order fill received from exchange
    ProcessOrderFill {
        order_id: OrderId,
        fill_price: Price,
        fill_quantity: Quantity,
    },
    /// Periodic health/reconciliation check
    HealthCheck,
}

/// Lifecycle state machine for an ExecutionQuery.
///
///   Accepted -> Processing -> Acting -> Completed
///                  |            |
///                  v            v
///               Failed       Failed
///
/// Phase 2+ adds: RiskChecked, AwaitingApproval, Authorized
/// between Processing and Acting.
pub enum QueryState {
    /// Query created, validated, queued
    Accepted,
    /// Engine + Risk are evaluating (Interpret + Decide phases)
    Processing,
    /// Executor is executing governed actions (Act phase)
    Acting,
    /// Successfully completed.
    /// In Phase 1: set after Executor returns (Executor persists internally).
    /// In Phase 2+: set after EventLog append confirmed.
    Completed,
    /// Terminal failure
    Failed {
        reason: String,
        phase: String,
    },
}

/// The result of a completed query.
pub enum QueryOutcome {
    /// Actions were executed via Executor.
    /// Note: persistence happens INSIDE Executor (store.events().append +
    /// store.positions().apply_event). By the time Executor returns,
    /// events are already persisted. This is why Completed is safe to set
    /// after executor.execute() returns.
    ActionsExecuted {
        actions_count: usize,
    },
    /// Query evaluated but no action was needed (e.g., price tick didn't trigger stop)
    NoAction {
        reason: String,
    },
    /// Risk Engine or governance denied the action (Phase 2+)
    Denied {
        reason: String,
    },
}

/// Who or what initiated the query.
pub enum ActorKind {
    /// Operator via CLI or API
    Operator {
        source: CommandSource,
    },
    /// Signal detector (external or manual injection)
    Detector,
    /// Market data feed (WebSocket or REST fallback)
    MarketData,
    /// Safety Net (rogue position monitor)
    SafetyNet,
    /// Internal system (timer, recovery, reconciliation)
    System {
        subsystem: String,
    },
}

/// Lightweight summary of context at query time.
/// Logged via tracing for observability, NOT used for decisions.
/// Phase 1: populated from Store queries.
/// Phase 2+: may include risk snapshot when RuntimeState exists.
pub struct ContextSummary {
    pub active_positions_count: usize,
}

impl ExecutionQuery {
    /// Create a new query. Always starts in Accepted state.
    pub fn new(kind: QueryKind, actor: ActorKind) -> Self {
        Self {
            id: Uuid::now_v7(),
            kind,
            state: QueryState::Accepted,
            actor,
            position_id: None,
            started_at: Utc::now(),
            finished_at: None,
            outcome: None,
            context_summary: None,
        }
    }

    /// Transition to a new state. Returns error if transition is invalid.
    pub fn transition(&mut self, new_state: QueryState) -> Result<(), QueryError> {
        // Enforce valid transitions
        match (&self.state, &new_state) {
            (QueryState::Accepted, QueryState::Processing) => Ok(()),
            (QueryState::Processing, QueryState::Acting) => Ok(()),
            (QueryState::Acting, QueryState::Completed) => Ok(()),
            // Any non-terminal state can fail
            (QueryState::Accepted, QueryState::Failed { .. }) => Ok(()),
            (QueryState::Processing, QueryState::Failed { .. }) => Ok(()),
            (QueryState::Acting, QueryState::Failed { .. }) => Ok(()),
            // Processing can skip Acting (NoAction / Denied)
            (QueryState::Processing, QueryState::Completed) => Ok(()),
            _ => Err(QueryError::InvalidTransition {
                from: format!("{:?}", self.state),
                to: format!("{:?}", new_state),
            }),
        }?;
        self.state = new_state;
        if matches!(self.state, QueryState::Completed | QueryState::Failed { .. }) {
            self.finished_at = Some(Utc::now());
        }
        Ok(())
    }

    /// Convenience: mark as completed with outcome.
    pub fn complete(&mut self, outcome: QueryOutcome) -> Result<(), QueryError> {
        self.outcome = Some(outcome);
        match &self.state {
            QueryState::Acting => self.transition(QueryState::Completed),
            QueryState::Processing => self.transition(QueryState::Completed),
            _ => Err(QueryError::InvalidTransition {
                from: format!("{:?}", self.state),
                to: "Completed".to_string(),
            }),
        }
    }

    /// Convenience: mark as failed.
    pub fn fail(&mut self, reason: String, phase: String) {
        self.outcome = Some(QueryOutcome::Denied {
            reason: reason.clone(),
            risk_verdict: None,
        });
        // Transition is best-effort for failure
        let _ = self.transition(QueryState::Failed { reason, phase });
    }

    /// Duration of query execution (None if not yet completed).
    pub fn duration(&self) -> Option<chrono::Duration> {
        self.finished_at.map(|f| f - self.started_at)
    }
}
```

### 2.2 QueryEngine — The Governed Execution Core

**File**: `v2/robsond/src/query_engine.rs`

The QueryEngine tracks the lifecycle of ExecutionQueries. It sits inside `robsond` (the Runtime crate).

**CRITICAL DESIGN DECISION FOR PHASE 1**: QueryEngine is a **lifecycle tracker**, not a dispatcher. It does NOT own Engine or Executor. It does NOT call them. PositionManager retains its existing Engine+Executor logic unchanged. QueryEngine records state transitions AROUND that logic.

**Why**: The v2 codebase has no `RuntimeState` struct. The Executor persists events internally via `store.events().append()` + `store.positions().apply_event()`. Persistence ownership lives in Executor, not in a caller-managed state object. Trying to move dispatch into QueryEngine in Phase 1 would require changing Executor's persistence model — that is a Phase 2 concern.

**Phase 2 evolution**: QueryEngine becomes the dispatcher. Engine+Executor calls move from PositionManager into QueryEngine.process(). GovernedAction wraps Executor calls. RuntimeState is introduced as an explicit struct.

```rust
use crate::query::{ExecutionQuery, QueryState, QueryOutcome};

/// Phase 1: Lifecycle tracker.
/// Records query state transitions via QueryRecorder.
/// Does NOT dispatch to Engine/Executor (PositionManager still does that).
///
/// Phase 2+: Becomes the governed dispatcher.
///
/// Ownership: lives INSIDE robsond crate. Not a separate crate.
pub struct QueryEngine<R: QueryRecorder> {
    recorder: R,
}

impl<R: QueryRecorder> QueryEngine<R> {
    pub fn new(recorder: R) -> Self {
        Self { recorder }
    }

    /// Record that a query has been accepted.
    pub fn on_accepted(&self, query: &ExecutionQuery) {
        self.recorder.on_state_change(query);
    }

    /// Record a state transition.
    pub fn on_state_change(&self, query: &ExecutionQuery) {
        self.recorder.on_state_change(query);
    }

    /// Record an error. Caller is responsible for calling query.fail() first.
    pub fn on_error(&self, query: &ExecutionQuery, error: &str) {
        self.recorder.on_error(query, error);
    }
}
```

### 2.3 QueryRecorder — Audit and Observability

**File**: Part of `v2/robsond/src/query_engine.rs`

```rust
/// Records query lifecycle events for observability and audit.
/// Phase 1: TracingQueryRecorder (structured logs via tracing crate).
/// Phase 2+: EventLogQueryRecorder (persists to robson-eventlog).
pub trait QueryRecorder: Send + Sync {
    fn on_state_change(&self, query: &ExecutionQuery);
    fn on_error(&self, query: &ExecutionQuery, error: &str);
}

/// Default implementation: structured tracing logs.
/// Zero persistence overhead. Full observability via tracing subscribers.
pub struct TracingQueryRecorder;

impl QueryRecorder for TracingQueryRecorder {
    fn on_state_change(&self, query: &ExecutionQuery) {
        tracing::info!(
            query_id = %query.id,
            kind = ?query.kind,
            state = ?query.state,
            actor = ?query.actor,
            position_id = ?query.position_id,
            duration_ms = ?query.duration().map(|d| d.num_milliseconds()),
            "query state transition"
        );
    }

    fn on_error(&self, query: &ExecutionQuery, error: &str) {
        tracing::error!(
            query_id = %query.id,
            kind = ?query.kind,
            state = ?query.state,
            error = %error,
            "query engine error"
        );
    }
}
```

### 2.4 Integration: PositionManager Wraps Existing Logic

**Phase 1**: PositionManager retains ALL existing Engine+Executor logic. It wraps each method with query lifecycle tracking. The internal calls remain identical.

**Phase 2**: Engine+Executor dispatch moves into QueryEngine.process(). PositionManager becomes a thin translator (EventBus events -> ExecutionQuery).

**Phase 1 pattern** (handle_signal as example):
```rust
// PositionManager wraps existing logic with query lifecycle
async fn handle_signal(&self, signal: DetectorSignal) {
    let mut query = ExecutionQuery::new(
        QueryKind::ProcessSignal {
            signal_id: signal.signal_id,
            symbol: signal.symbol.clone(),
            side: signal.side,
            entry_price: signal.entry_price,
            stop_loss: signal.stop_loss,
        },
        ActorKind::Detector,
    );
    query.position_id = Some(signal.position_id);
    self.query_engine.on_accepted(&query);

    query.transition(QueryState::Processing)?;
    self.query_engine.on_state_change(&query);

    // --- EXISTING LOGIC UNCHANGED ---
    let position = self.store.positions().find_by_id(position_id).await?...;
    self.kill_detector(position_id).await;
    let decision = self.engine.decide_entry(&position, &signal)?;

    query.transition(QueryState::Acting)?;
    self.query_engine.on_state_change(&query);

    let results = self.executor.execute(decision.actions).await?;
    // ... handle results as before ...
    // --- END EXISTING LOGIC ---

    query.complete(QueryOutcome::ActionsExecuted { actions_count: results.len() })?;
    self.query_engine.on_state_change(&query);
}

// On ANY error path:
// query.fail(format!("{}", e), "processing".to_string());
// self.query_engine.on_error(&query, &format!("{}", e));
// return Err(e);
```

**Fan-out pattern** (process_market_data):
```rust
// One ExecutionQuery PER POSITION processed, not one per tick
pub async fn process_market_data(&self, data: MarketData) -> DaemonResult<()> {
    let open_positions = self.store.positions().find_active().await?;

    for position in open_positions {
        if position.symbol != data.symbol { continue; }
        if !matches!(position.state, PositionState::Active { .. }) { continue; }

        let mut query = ExecutionQuery::new(
            QueryKind::ProcessMarketTick {
                symbol: data.symbol.clone(),
                price: data.price,
            },
            ActorKind::MarketData,
        );
        query.position_id = Some(position.id);
        self.query_engine.on_accepted(&query);

        // ... existing engine + executor logic per position, wrapped with transitions ...

        query.complete(outcome)?;
        self.query_engine.on_state_change(&query);
    }
    Ok(())
}
```

**Fan-out pattern** (panic_close_all):
```rust
// find_active() returns open core-lifecycle positions:
// Armed, Entering, Active, Exiting.
// Each state receives appropriate handling — not blindly "close everything".
pub async fn panic_close_all(&self) -> DaemonResult<Vec<PositionId>> {
    let open_positions = self.store.positions().find_active().await?;
    let mut closed_ids = Vec::new();

    for position in open_positions {
        match &position.state {
            PositionState::Active { .. } => {
                // One PanicClosePosition query per Active position
                let mut query = ExecutionQuery::new(
                    QueryKind::PanicClosePosition { position_id: position.id },
                    ActorKind::Operator { source: CommandSource::Cli },
                );
                query.position_id = Some(position.id);
                self.query_engine.on_accepted(&query);
                match self.panic_close_position_internal(position.id, &mut query).await {
                    Ok(_) => { closed_ids.push(position.id); }
                    Err(e) => { query.fail(format!("{}", e), "acting".to_string()); }
                }
            }
            PositionState::Armed => { self.disarm_position(position.id).await.ok(); }
            PositionState::Entering { .. } => { /* cancel not yet implemented — log */ }
            PositionState::Exiting { .. } => { /* skip: exit already in progress */ }
            _ => {}
        }
    }
    Ok(closed_ids)
}
```

This pattern applies to ALL PositionManager entry points: `arm_position`, `disarm_position`, `handle_signal`, `process_market_data`, `panic_close_all`.

**Methods NOT wrapped separately**: `handle_entry_fill` and `handle_exit_fill` are internal methods called within `handle_signal` and `process_market_data`. They are covered by the parent method's query lifecycle.

---

## 3. STATE MACHINE EVOLUTION

### Phase 1 (Now) — Minimal, Non-Breaking

```
Accepted -> Processing -> Acting -> Completed
                |            |
                v            v
             Failed        Failed
                |
                v
           Completed (NoAction/Denied)
```

5 states. Enough for typed lifecycle tracking and structured audit.

### Phase 2 (v2.5 #4 — GovernedAction) — Add Risk Gate

```
Accepted -> Processing -> RiskChecked -> Acting -> Completed
                |             |             |
                v             v             v
             Failed        Denied        Failed
```

`RiskChecked` is the proof that Risk Engine approved. `Denied` is a terminal state (not Failed — denial is intentional, not an error).

### Phase 3 (v3 — Approval Gates) — Add Human Confirmation

```
Accepted -> Processing -> RiskChecked -> AwaitingApproval -> Authorized -> Acting -> Completed
                |             |                |                              |
                v             v                v                              v
             Failed        Denied          Expired                         Failed
```

`AwaitingApproval` blocks until operator confirms (for actions requiring human gate). `Expired` if operator does not respond within TTL. `Authorized` carries the approval token.

---

## 4. RELATIONSHIP TO EXISTING v3 SPECIFICATIONS

### v3-control-loop.md Mapping

The Control Loop phases map directly to QueryEngine processing:

| Control Loop Phase | Phase 1 Responsibility | Phase 2+ Responsibility | QueryState |
|---|---|---|---|
| **Observe** | PositionManager creates `ExecutionQuery` | Same | `Accepted` |
| **Interpret** | PositionManager calls Engine (unchanged) | QueryEngine dispatches to Engine | `Processing` |
| **Decide** | Engine returns `EngineDecision` (pure) | Same | `Processing` |
| **Risk Gate** | N/A (pass-through) | QueryEngine evaluates via RiskGate | `RiskChecked` (Phase 2) |
| **Act** | PositionManager calls Executor (unchanged) | QueryEngine calls Executor with GovernedAction | `Acting` |
| **Evaluate + Persist** | Executor persists internally (unchanged) | QueryEngine manages persist sequence | `Completed` |

### v3-runtime-spec.md Mapping

| Runtime Concept | Phase 1 Equivalent | Phase 2+ Equivalent |
|---|---|---|
| `RuntimeInput` | `QueryKind` | Same |
| `RuntimeOutput` | `QueryOutcome` | Same + emitted events |
| `GovernedAction` | N/A | Constructed inside QueryEngine after RiskChecked |
| `RuntimeState` | `Store` (MemoryStore/PgStore) | Explicit in-memory RuntimeState |
| Zero-Bypass Guarantee | Partial (all entry points wrapped) | Full (Executor accepts only GovernedAction) |

### v3-migration-plan.md Alignment

| Migration Step | QueryEngine Impact |
|---|---|
| v2.5 #1: Deploy robsond | QueryEngine ships as part of robsond |
| v2.5 #3: Migrate stop monitoring | Stop monitoring goes through QueryEngine via `ProcessMarketTick` |
| v2.5 #4: Wire Risk Engine as blocking gate | Risk gate added to QueryEngine Phase 2 |
| v2.5 #5: Circuit breaker ladder | Circuit breaker check in QueryEngine before Processing |
| v3 #1: Promote robsond as primary | QueryEngine IS the primary execution path |

**No conflicts with existing migration steps.** QueryEngine is additive in Phase 1 (wrapper) and becomes the enforcement mechanism in Phase 2+.

---

## 5. PHASED IMPLEMENTATION PLAN

### Phase 1: Passive Wrapper (Non-Breaking)

**Goal**: Introduce ExecutionQuery and QueryEngine without changing any external behavior. Tracing-only audit. All existing tests pass.

**Files created**:
- `v2/robsond/src/query.rs` — ExecutionQuery, QueryKind, QueryState, QueryOutcome, ActorKind, ContextSummary, scaffolding enums
- `v2/robsond/src/query_engine.rs` — QueryEngine, QueryRecorder trait, TracingQueryRecorder

**Files modified**:
- `v2/robsond/src/lib.rs` — add `mod query; mod query_engine;` and re-exports
- `v2/robsond/src/position_manager.rs` — wrap `arm_position`, `disarm_position`, `handle_signal`, `process_market_data`, `panic_close_all` with query lifecycle tracking

**Files NOT touched**:
- `robson-engine/*` — Engine remains pure, unchanged
- `robson-exec/*` — Executor signature unchanged, persistence ownership unchanged
- `robson-eventlog/*` — EventLog unchanged (especially `query.rs`)
- `robson-store/*` — Store unchanged
- `robsond/src/api.rs` — API handlers unchanged
- `robsond/src/daemon.rs` — Daemon unchanged
- Any Django code

**Acceptance criteria**:
- `cargo test -p robsond` passes with zero regressions
- `cargo test --all` passes
- Every wrapped execution path produces structured tracing with `query_id`
- State machine transitions are validated (invalid transitions return error)
- Every error path calls `query.fail()` before returning
- Fan-out methods (`process_market_data`, `panic_close_all`) create one query per position
- `cargo clippy --all-targets -- -D warnings` clean
- `cargo fmt --all --check` clean

**What is intentionally deferred**:
- No DB persistence for queries (tracing only)
- No blocking risk gate (pass-through)
- No approval gates
- No GovernedAction construction
- No changes to Executor signature or persistence ownership
- No RuntimeState struct (v2 uses Store)
- `handle_entry_fill` and `handle_exit_fill` not separately wrapped (covered by parent query)

### Phase 2: Blocking Governance (Aligns with v2.5 #4) — IMPLEMENTED 2026-04-04

**Goal**: Wire Risk Engine as mandatory blocking gate inside QueryEngine. Introduce GovernedAction.

**Implemented**:
- `QueryEngine` gains `risk_gate: RiskGate` field
- New state: `RiskChecked` between Processing and Acting
- New terminal state: `Denied { reason, check }` (governance denial — distinct from `Failed`)
- `GovernedAction` constructed inside `QueryEngine::check_risk()` with `pub(crate)` visibility
- `QueryRecorder` records both `RiskChecked` and `Denied` transitions via tracing
- `CheckRiskError` enum separates governed denial (`Denied`) from operational state-machine
  error (`InvalidState(QueryError)`). Callers re-arm the detector on `Denied` and propagate
  as hard error on `InvalidState`. Prevents a state machine bug from being silently treated
  as a governed denial.
- `PositionRepository::find_risk_open()` added with explicit Entering+Active semantics.
  `build_risk_context()` calls this — not `find_active()` — so concurrent Entering positions
  (order submitted, fill pending) block new entries as expected.
- `PositionRepository::find_active()` contract fixed to return open core-lifecycle states only:
  `Armed`, `Entering`, `Active`, `Exiting`. It explicitly excludes `Closed` and `Error`.
  `MemoryStore` implementation now matches these states explicitly instead of relying on
  `!is_closed()`, which accidentally included `Error`.
- PostgreSQL projection recovery was aligned to the same contract. `positions_current` now stores
  recovery metadata (`entry_signal_id`, `exit_reason`) so `find_active_from_projection()` can
  reconstruct `Entering` and `Exiting` rows instead of silently dropping them during fallback recovery.
- `panic_close_all()` refactored with per-state dispatch: Active→exit order, Armed→disarm,
  Entering→warning (cancel not yet implemented), Exiting→skip.

**Architectural decision (Option A — recorded)**:
- `GovernedAction` lives in `robsond` (`pub(crate)`), NOT in `robson-exec`
- `Executor` continues accepting `Vec<EngineAction>` — signature unchanged
- Governance enforcement is **runtime-level inside the crate**, not type-level across crate boundary
- Rationale: the crate graph `robsond → robson-exec` makes `GovernedAction` in `robson-exec` require
  either a circular dependency or a new shared contracts crate. Both are out of scope for this phase.
  Governance and Risk Gate belong to the Runtime/QueryEngine layer, not to I/O execution.
- `GovernedAction` can only be constructed by `QueryEngine::check_risk()` (private constructor),
  enforcing the governance rule within the crate. Type-level enforcement across the crate boundary
  is explicitly deferred as a follow-up architectural concern.

**Phase 2 limitation**: Daily PnL (daily_realized_pnl, daily_unrealized_pnl) in `RiskContext` defaults
to zero. The daily loss circuit breaker (`DailyLossLimit` check in RiskGate) is therefore not active.
Proper PnL tracking in the store is deferred to a follow-up task.

**Depends on**: Phase 1 complete

### Phase 3: Approval Gates (v3)

**Goal**: Add human confirmation for high-risk actions.

**Changes**:
- New states: `AwaitingApproval`, `Authorized`, `Expired`
- API endpoint: `POST /api/v1/queries/{id}/approve`
- Approval token with TTL, bound to query_id + action hash
- Permission system wired into QueryEngine
- Circuit breaker reset requires approval
- Precondition status: SSE working (v2.5 #6) is satisfied by the `robsond` `/events`
  endpoint implemented on 2026-04-05

**Depends on**: Phase 2 complete, SSE working (v2.5 #6)

### Phase 4: Full Audit & Replay

**Goal**: Complete audit trail. Query lifecycle fully persisted. Replay determinism proven.

**Changes**:
- Query lifecycle events in EventLog (QueryAccepted, QueryProcessing, QueryCompleted, etc.)
- Projection handler for query lifecycle (robson-projector)
- Projection worker checkpoint moves from /tmp to DB
- Replay test: insert N queries, replay from EventLog, compare state byte-for-byte

**Depends on**: Phase 3 complete, v2.5 #10 (replay determinism)

### Phase 5: Context Governance (v3+ with LLM)

**Goal**: Bounded context for LLM reasoning, if ever added.

**Changes**:
- ContextWindow: redacted, bounded view of state for ReasoningPort
- LLM output typed as Suggestion (Observation | ThesisUpdate only)
- Action-bearing suggestions rejected at type level

**Depends on**: v3 stable for >3 months, concrete LLM value proposition

---

## 6. SCAFFOLDING ENUMS (Phase 1 — Permissive, Future-Ready)

Include these in Phase 1 for type stability, but keep them permissive (always allow/approve):

```rust
/// Classification of side effects. Used by governance layers.
/// Phase 1: informational only (logged via tracing).
/// Phase 2+: determines approval requirements.
pub enum ActionClass {
    /// Reads state, no side effects
    ReadOnly,
    /// Writes to exchange (orders, cancellations)
    ExchangeWrite,
    /// Modifies runtime configuration or risk limits
    ControlPlaneWrite,
    /// Overrides a risk denial
    RiskOverride,
    /// Replay or recovery operation
    ReplayRepair,
}

/// Whether an action requires human approval.
/// Phase 1: always NotRequired.
/// Phase 3+: determined by action class, risk level, and configuration.
pub enum ApprovalRequirement {
    NotRequired,
    Required {
        reason: String,
        ttl_seconds: u64,
    },
}

/// Result of a permission check.
/// Phase 1: always Granted.
/// Phase 2+: determined by Risk Engine and governance rules.
pub enum PermissionDecision {
    Granted,
    Denied { reason: String },
    RequiresElevation { scope: String },
}
```

---

## 7. stream = projection — FORMAL CONTRACT

With QueryEngine as the execution core, the stream derivation contract becomes explicit:

**Phase 1 / v2.5 current model**:
```
ExecutionQuery (trigger)
  -> PositionManager (Engine + Executor calls, wrapped with lifecycle)
    -> Executor.execute() (persists events + applies to Store internally)
      -> Store (MemoryStore/PgStore) = operational truth
      -> EventLog.append() = durable truth (when robson-eventlog is wired)
        -> Runtime EventBus / public mapper (derived, ephemeral)
          -> SSE stream (push to consumers)
```

**Phase 2+ (v3 target)**:
```
ExecutionQuery (trigger)
  -> QueryEngine.process() (owns the pipeline)
    -> RuntimeState mutation (operational truth)
      -> EventLog.append() (durable truth)
        -> ProjectionWorker.apply() (derived)
          -> SSE stream (push to consumers)
```

**Rules** (apply to both phases):
1. **Derived streams NEVER feed back into decisions.** Decisions use Store (Phase 1/v2.5) or RuntimeState (Phase 2+/v3 target).
2. **v2.5 SSE is an ephemeral derived stream**, mapped from runtime events for operator/UI use. It is not a source of truth, does not provide replay, and requires REST bootstrap on connect.
3. **v3 target SSE is derived from durable projections**, not from Store/RuntimeState directly.
4. **If projections drift from EventLog** (watermark lag >100), they are rebuilt from EventLog. This is safe because projections are always derived.
5. **If Store/RuntimeState drifts from exchange** (reconciliation mismatch), exchange state wins. A ReconciliationEvent is appended to EventLog.

This formalizes what v3-control-loop.md and v3-runtime-spec.md already describe, but makes the derivation chain explicit and enforceable.

---

## 8. FAILURE MODES

| Failure | Impact | Detection | Recovery |
|---------|--------|-----------|----------|
| Executor fails mid-execution (exchange order placed, store update fails) | State/store may be inconsistent | ActionResult::OrderFailed or ExecError | Intent journal provides idempotency. On restart: check intent status, reconcile with exchange. |
| Query fails mid-Processing (Engine error) | No side effects (Engine is pure) | QueryState::Failed with phase="processing" | Safe: no exchange interaction occurred. query.fail() called, recorded via on_error(). |
| Query fails mid-Acting (Executor error) | Exchange order may be placed | QueryState::Failed with phase="acting" | Intent journal dedup. On restart: reconcile with exchange. |
| Error path skips query.fail() | Query stuck in non-terminal state | Missing on_error() tracing entry | Implementation MUST call query.fail() in every error branch before returning Err. |
| Fan-out partial failure (2 of 3 positions fail) | Some positions processed, some not | Per-position query has own Failed/Completed state | Each position's query tracks independently. Operator sees which succeeded/failed. |
| Duplicate execution after restart | Same trigger processed twice | IntentJournal dedup, EventLog idempotency_key | Safe retries guaranteed by existing Executor idempotency. |
| Projection drift from EventLog | UI shows stale data | Watermark lag monitoring (>100 events) | Automatic projection rebuild from EventLog. No impact on execution. |

---

## 9. OBSERVABILITY

### Metrics (Prometheus)

```
robson_queries_total{kind, actor, outcome}          # Counter: queries by type and result
robson_query_duration_ms{kind, phase}               # Histogram: duration per query phase
robson_query_state{state}                           # Gauge: queries in each state
robson_query_denied_total{reason}                   # Counter: risk/governance denials
robson_query_failed_total{phase, kind}              # Counter: failures by phase
```

### Structured Logs (tracing)

Every query state transition produces a structured log entry:
```json
{
  "query_id": "01926a3b-...",
  "kind": "process_signal",
  "state": "completed",
  "actor": "detector",
  "position_id": "01926a2f-...",
  "duration_ms": 47,
  "outcome": "actions_executed",
  "actions_count": 1,
  "events_count": 2,
  "context": {
    "active_positions": 1,
    "exposure_pct": "12.5",
    "circuit_breaker": "closed"
  }
}
```

---

## 10. CLAUDE CODE IMPLEMENTATION PROMPT — PHASE 1

**IMPORTANT**: The prompt below has been superseded by a more detailed, code-anchored prompt provided separately. Use the external prompt document, not this abbreviated version. This section is kept for reference only.

The external prompt includes:
- Exact v2 type signatures (PositionManager<E: ExchangePort, S: Store>, Executor.execute(Vec<EngineAction>))
- Correct fan-out pattern for process_market_data and panic_close_all
- Error path requirements (query.fail() on every error branch)
- Phase 1 lifecycle tracker design (QueryEngine does NOT dispatch, only records)
- Complete test requirements

---

## 11. OPEN QUESTIONS

These require human architectural decision before Phase 2:

1. **Query persistence granularity**: Should every query (including high-frequency market ticks) be persisted to EventLog, or only queries that produce actions? Market ticks at 100/s would generate significant EventLog volume.

2. **GovernedAction location**: ~~Should it stay in robsond or move to robson-exec?~~ **DECIDED 2026-04-04**: Stays in robsond as `pub(crate)`. Executor unchanged. See Phase 2 architectural decision above.

3. **Approval persistence**: Should pending approvals be persisted to PostgreSQL (survives restart) or kept in-memory (lost on restart, operator must re-approve)? Persistent approvals add complexity but prevent lost approval state during rolling deploys.

4. **Query timeout**: Should there be a hard timeout on queries in AwaitingApproval state? The v3-risk-engine-spec.md defines circuit breaker escalation timers (30min L1->L2), but individual query approval TTL is not specified.

5. **Event model unification**: The repository has two event models (robson_domain::Event and EventEnvelope). Should QueryEngine produce one, the other, or both? Unification is desirable but risky if done in Phase 1.

---

**This document is the authoritative specification for Query/QueryEngine in Robson v3. Implementation follows the phased plan above. Phase 1 is safe, non-breaking, and ready to build.**
