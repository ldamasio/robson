# ROBSON v3 — RUNTIME SYSTEM SPECIFICATION

**Date**: 2026-04-03  
**Status**: APPROVED  
**Owner**: robsond (Rust daemon)

---

## Identity

The Runtime is the single most important software component in Robson. It is NOT "the thing that manages context." It is the sole guardian of execution. Every action in the system — market observation, risk evaluation, order placement, state transition, event persistence — flows through the Runtime.

**The Runtime IS the agent.** The LLM (if integrated in v3+) is a reasoning tool that the Runtime governs. The exchange is an execution tool that the Runtime governs. The EventLog is a persistence tool that the Runtime governs. Nothing operates outside the Runtime's control.

### Execution Core: QueryEngine

The Runtime's execution pipeline is implemented by the **QueryEngine** (`robsond/src/query_engine.rs`). Every trigger becomes a typed **ExecutionQuery** that flows through the Control Loop. The QueryEngine is internal to the Runtime — it is NOT a separate service or crate.

**Architectural premise**: `state = source of truth, stream = projection`. RuntimeState is the operational authority for real-time decisions. EventLog is the durable authority for replay and audit. Projections are always derived from EventLog, never consulted for decisions.

**v2.5 transitional note (implemented 2026-04-05)**: `robsond` now exposes a minimal SSE endpoint at `/events` for operator-facing runtime updates. This stream is ephemeral and derived for UI/monitoring only. It does not provide replay or `Last-Event-ID` resume; clients bootstrap current state via REST and then subscribe for incremental updates.

**QE-P3 transitional note (implemented 2026-04-05)**: high-notional entry queries now pause in `AwaitingApproval` after `RiskChecked`. The approval decision is made inside `QueryEngine`; pending approvals are held in runtime memory by `PositionManager`; the operator authorizes via `POST /queries/{id}/approve`; and pending approvals expire after 300 seconds if not approved. Approval is not a risk override: pending queries reserve risk while waiting, and `approve` revalidates the current risk context before acting. `disarm` invalidates pending approvals for the same position. REST bootstrap now exposes pending approvals via `/status`, because `/events` remains ephemeral and non-replayable. This is intentionally non-durable in v2.5/v3 minimum scope: restart drops pending approvals and clients must re-bootstrap from REST + SSE.

**Current v2.5 implementation note**: governance is already enforced inside `robsond`, but the executor boundary is still transitional. `QueryEngine` uses an internal `GovernedAction` token after risk approval; `Executor` still accepts `Vec<EngineAction>`; query lifecycle audit events append directly to `robson-eventlog`; and executor domain events still persist through `Store`.

See **[v3-query-query-engine.md](v3-query-query-engine.md)** for the full specification.

---

## Architecture

```
                    ┌──────────────────────────────────┐
                    │         RUNTIME (robsond)        │
                    │                                  │
                    │  ┌────────────┐ ┌─────────────┐  │
Observation ──────>│  │ Control    │ │ Position    │  │
Queue              │  │ Loop      │ │ Manager     │  │
                    │  └─────┬──────┘ └──────┬──────┘  │
                    │        │               │         │
                    │  ┌─────▼──────┐ ┌──────▼──────┐  │
                    │  │ Risk       │ │ Engine      │  │
                    │  │ Engine     │ │ (pure)      │  │
                    │  └─────┬──────┘ └──────┬──────┘  │
                    │        │               │         │
                    │  ┌─────▼───────────────▼──────┐  │
                    │  │   Governance Layer          │  │
                    │  │   (GovernedAction factory)  │  │
                    │  └─────────────┬──────────────┘  │
                    │               │                  │
                    └───────────────┼──────────────────┘
                                    │
                    ┌───────────────▼──────────────────┐
                    │        EXECUTOR (robson-exec)     │
                    │   ExchangePort -> Binance         │
                    └──────────────────────────────────┘
                                    │
                    ┌───────────────▼──────────────────┐
                    │        EVENTLOG (robson-eventlog) │
                    │   PostgreSQL append-only           │
                    └──────────────────────────────────┘
```

---

## Contracts

### Input Contract

```rust
/// Everything that enters the Runtime
pub enum RuntimeInput {
    /// Market data from WebSocket or REST fallback
    MarketTick {
        symbol: Symbol,
        bid: Price,
        ask: Price,
        timestamp: DateTime<Utc>,
    },
    /// Entry signal from detector or manual injection
    DetectorSignal {
        signal_id: Ulid,
        symbol: Symbol,
        side: Side,
        entry_price: Price,
        tech_stop: Price,
        confidence: Decimal,
    },
    /// Operator command from CLI or API
    OperatorCommand {
        command: Command,
        params: CommandParams,
        issued_at: DateTime<Utc>,
        source: CommandSource,  // CLI, API, UI
    },
    /// Order fill notification from exchange
    OrderFill {
        order_id: OrderId,
        fill_price: Price,
        fill_quantity: Quantity,
        commission: Decimal,
        timestamp: DateTime<Utc>,
    },
    /// Periodic timer
    TimerFire {
        interval_id: String,
        fired_at: DateTime<Utc>,
    },
}
```

**Validation**: Every RuntimeInput is validated at the boundary:
- Prices must be > 0
- Quantities must be > 0
- Timestamps must not be in the future (with 5s tolerance for clock skew)
- Signal IDs must be valid ULIDs
- Symbols must be in the allowed set (configurable)

Invalid inputs are rejected with `InputRejected` event and do not enter the control loop.

### Output Contract

```rust
/// Everything that leaves the Runtime
pub enum RuntimeOutput {
    /// Events produced and persisted in EventLog
    EventsProduced {
        events: Vec<EventEnvelope>,
        cycle_id: Ulid,
    },
    /// Actions requested to Executor after runtime governance clearance
    ActionRequested {
        actions: Vec<EngineAction>,
        cycle_id: Ulid,
    },
    /// State change notification (for SSE consumers)
    StateChanged {
        position_id: PositionId,
        old_state: PositionState,
        new_state: PositionState,
    },
    /// Alert for operator
    Alert {
        kind: AlertKind,
        message: String,
        severity: AlertSeverity,
    },
}
```

---

## State Representation

```rust
pub struct RuntimeState {
    /// All positions by ID (armed, active, entering, exiting)
    pub positions: HashMap<PositionId, Position>,
    
    /// Active orders pending fill
    pub active_orders: HashMap<OrderId, OrderState>,
    
    /// Current risk snapshot (exposure, daily P&L, drawdown)
    pub risk_snapshot: RiskSnapshot,
    
    /// Circuit breaker state
    pub circuit_breaker: CircuitBreakerState,
    
    /// Runtime configuration (risk limits, timing, allowed symbols)
    pub config: RuntimeConfig,
    
    /// Control flags
    pub paused: bool,
    pub halted: bool,
    
    /// Last persisted event sequence (for projection watermark)
    pub last_event_sequence: u64,
    
    /// Cycle counter (monotonically increasing)
    pub cycle_count: u64,
}

// PnL fields follow the canonical model defined in v3-risk-engine-spec.md § PnL Model.
// monthly_drawdown_pct = (realized_pnl_gross - fees_paid + unrealized_pnl) / capital
pub struct RiskSnapshot {
    pub total_exposure_pct: Decimal,
    pub daily_pnl_pct: Decimal,
    pub monthly_drawdown_pct: Decimal,
    pub open_position_count: usize,
    pub daily_order_count: usize,
    pub last_updated: DateTime<Utc>,
}

pub enum CircuitBreakerState {
    Active,
    MonthlyHalt {
        triggered_at: DateTime<Utc>,
        trigger_reason: String,
    },
}
```

---

## Internal Stages

Each runtime cycle processes a trigger through 9 sequential stages.
These are **pipeline stages within a single execution tick**, not project phases.
For project-level identifiers see v3-migration-plan.md §1.1 (`MIG-*`, `QE-P*`).

### Stage 1: Input Validation

Every `RuntimeInput` passes through validation before entering the observation queue:

```rust
impl RuntimeInput {
    pub fn validate(&self) -> Result<(), ValidationError> {
        match self {
            Self::MarketTick { bid, ask, .. } => {
                ensure!(bid.value() > Decimal::ZERO, "bid must be positive");
                ensure!(ask.value() > Decimal::ZERO, "ask must be positive");
                ensure!(ask >= bid, "ask must be >= bid");
            }
            Self::DetectorSignal { entry_price, tech_stop, side, .. } => {
                match side {
                    Side::Long => ensure!(tech_stop < entry_price, "long: stop must be below entry"),
                    Side::Short => ensure!(tech_stop > entry_price, "short: stop must be above entry"),
                }
            }
            // ... other variants
        }
        Ok(())
    }
}
```

### Stage 2: Inspection (v3 — when LLM is integrated)

For v2.5/v3 launch: NO LLM, this stage is a no-op pass-through.

Future (v3+): When LLM reasoning is added for thesis evaluation:
- Strip sensitive data (API keys, credentials) from context
- Validate schema of any data entering LLM prompt
- Log what context was provided to the model for audit

### Stage 3: Risk Pre-Check

Before the Engine makes any decision, the Risk Engine performs a pre-check:
- Is the circuit breaker CLOSED? If OPEN, only `PanicClose` and `AdjustConfig` actions are allowed.
- Is the system paused? If paused, only operator commands are processed.
- Is the daily order count below the rate limit?

### Stage 4: Engine Decision

The Engine receives the interpretation and produces an `EngineAction`. This is a pure function — no I/O, no side effects, deterministic.

### Stage 5: Risk Post-Check

The specific `EngineAction` is evaluated:
- `PlaceEntryOrder`: Does the new position violate max exposure? Max position count? Max single position size?
- `TriggerExit`: Always allowed (reducing exposure is always safe).
- `UpdateTrailingStop`: Always allowed (tightening stop is always safe).
- `PanicClose`: Always allowed (emergency).

### Stage 6: Governance (GovernedAction Construction)

If Risk Engine approves:
```rust
let governed = GovernedAction::new(actions);
let cleared_actions = governed.into_actions();
```

Current v2.5/QE-P2 reality: `GovernedAction` is an internal `robsond` token proving that `QueryEngine` cleared the action set. It is `pub(crate)` and not part of the executor API. The executor boundary still accepts `Vec<EngineAction>` today; moving the proof all the way to the executor signature remains target architecture / follow-up work.

### Stage 7: Execution

Risk-cleared actions are sent to Executor as `Vec<EngineAction>`. Result returned.

### Stage 8: Evaluation

Action result applied to RuntimeState. Domain events produced.

### Stage 9: Persistence

Events appended to EventLog. Cycle complete.

---

## Zero-Bypass Guarantee

### How the Runtime prevents bypass

1. **Current runtime-level enforcement**: `GovernedAction` can only be constructed inside `robsond` after Risk Engine clearance. It acts as an internal proof token before executor dispatch, even though the executor signature still accepts `Vec<EngineAction>` today.

```rust
// In robsond:
let governed = query_engine.check_risk(query, actions, proposed, context).await?;

// Current executor boundary:
executor.execute(governed.into_actions()).await?;

// In robson-exec:
pub async fn execute(&self, actions: Vec<EngineAction>) -> ExecResult<Vec<ActionResult>> { ... }
```

Cross-crate type-level enforcement remains a v3 target, not a property of the current executor API.

2. **Module visibility**: `GovernedAction::new()` is `pub(crate)` within the runtime crate. External crates cannot construct it.

3. **No direct exchange access from the decision path**: `ExchangePort` is held behind `Executor`. Runtime entry points are wrapped by `QueryEngine` before dispatch, so the governance gate remains on the hot path.

4. **Current persistence reality**: query lifecycle audit events are appended via `EventLogQueryRecorder`; executor domain events are still appended via `Store::events().append()`. Convergence to a single persistence boundary is architectural direction, not current-state behavior.

### What this prevents today

| Attack/Bug | Prevention |
|-----------|------------|
| Engine produces action, bypasses Risk Engine | `QueryEngine` must create the internal `GovernedAction` token before executor dispatch |
| Component writes query lifecycle events outside the audit trail | Query lifecycle audit is centralized in `EventLogQueryRecorder` |
| Component calls exchange directly from the decision path | ExchangePort instance is private to Executor; runtime owns executor dispatch |
| LLM output triggers direct execution | LLM output is typed as `Suggestion` — only `Observation` and `ThesisUpdate` variants exist. No `PlaceOrder` variant. |

### Target-state strengthening

The intended v3 strengthening is to push this proof to the executor boundary as well, so the executor accepts a governed type rather than raw `Vec<EngineAction>`. That is not yet implemented.

---

## Robson-Authored Position Invariant

Reference: [ADR-0022](../adr/ADR-0022-robson-authored-position-invariant.md),
[docs/policies/UNTRACKED-POSITION-RECONCILIATION.md](../policies/UNTRACKED-POSITION-RECONCILIATION.md).

Every open position on the operated Binance account MUST be the direct result of an
entry authored by `robsond` under a `GovernedAction`. The Zero-Bypass Guarantee above
covers the **write side** (nothing leaves the Runtime without governance). This
invariant adds the **read side**: nothing sits on the exchange that did not leave the
Runtime.

Operationally, the Runtime owns a long-lived **Position Reconciliation Worker** that:

- Scans Binance for open positions across **all account types** (spot, isolated margin,
  cross margin, futures) and **all symbols** — not gated by `allowed_symbols` and not
  gated by `ROBSON_POSITION_MONITOR_ENABLED`.
- Looks up each open position by exchange order id in `event_log`.
- Classifies positions without a matching `entry_order_placed` event as **UNTRACKED**.
- Closes UNTRACKED positions at market via the Safety Net close path (exit reason
  `UNTRACKED_ON_EXCHANGE`) and alerts the operator at severity `CRITICAL`.

Follow-up required (target architecture):

- Exchange-order-id index on `event_log` for O(1) lookup by the reconciliation worker.
- Dedicated `safety_net` close path for UNTRACKED positions.
- Startup reconciliation gate: the daemon must enter `StartupReconciling` before
  accepting observations; new entries blocked until the UNTRACKED set is empty.
- Operator panic override endpoint (`POST /reconciliation/suspend`, max TTL 300 s).

---

## Symbol-Agnostic Policy Invariant

Reference: [ADR-0023](../adr/ADR-0023-symbol-agnostic-policy-invariant.md),
[docs/policies/SYMBOL-AGNOSTIC-POLICIES.md](../policies/SYMBOL-AGNOSTIC-POLICIES.md).

The Runtime treats **symbol as a variable**, not as a fixed parameter. Every rule in
this specification applies to every trading pair the operator configures.
`allowed_symbols` in `RuntimeConfig` is a **scope selector** (which symbols are
currently active) — it is not a policy exception.

Symbol-specific constants (tick size, lot step, minimum notional, max leverage, fee
rate) are loaded from `ExchangePort::exchange_info()` at runtime and cached per
symbol. They MUST NOT appear as numeric literals in this specification or any
component spec.

Examples in this document that use `BTCUSDT` are illustrative. Replace with
`{symbol}` when reading the specification abstractly.

---

## Model Agnosticism

The Runtime has ZERO coupling to any specific LLM:

1. No LLM SDK imports in runtime crate
2. No model-specific prompt templates
3. No API key management for LLM providers
4. Engine decisions are pure Rust functions

If LLM is added (v3+), it enters through:
```rust
#[async_trait]
pub trait ReasoningPort: Send + Sync {
    async fn evaluate_thesis(&self, context: ThesisContext) -> Result<ThesisEvaluation>;
    async fn suggest_observation(&self, market_state: MarketSnapshot) -> Result<Suggestion>;
}
```

The Runtime governs what context enters the ReasoningPort and what outputs are accepted. The ReasoningPort implementation (Claude, GPT, Llama, local model) is an adapter that can be swapped without touching the Runtime.

---

## Configuration

```toml
# robsond.toml

[runtime]
observation_queue_capacity = 1000
cycle_timeout_ms = 5000
health_check_interval_s = 30

[risk]
max_open_positions = 3
max_total_exposure_pct = 30
max_single_position_pct = 15
max_daily_loss_pct = 3
max_monthly_drawdown_pct = 4
max_orders_per_minute = 10
max_slippage_pct = 5
risk_engine_timeout_ms = 200

[circuit_breaker]
# v3: binary MonthlyHalt — no escalation ladder
# 4% monthly drawdown → MonthlyHalt (close all, block new entries)
# No automatic reset. No Warning/SoftHalt/HardHalt levels.

[execution]
retry_attempts = 3
retry_backoff_ms = [100, 500, 2000]
intent_journal_enabled = true

[market_data]
websocket_reconnect_delay_ms = 1000
websocket_max_reconnect_delay_ms = 30000
rest_fallback_poll_interval_ms = 1000

[api]
host = "0.0.0.0"
port = 8080
sse_batch_interval_ms = 100
sse_critical_immediate = true

[persistence]
eventlog_database_url = "postgres://robson:***@jaguar:5432/robson"
projection_reconciliation_interval_s = 300
```

---

## Recovery Procedures

### Scenario 1: Runtime Crash Mid-Cycle

1. Kubernetes detects liveness probe failure (30s timeout)
2. Pod restarted automatically
3. On startup: `RuntimeState::replay_from_log(pool)` reconstructs state from EventLog
4. Reconcile with Binance: `reconcile_with_exchange(state, exchange)` compares positions
5. If match: resume normal operation
6. If mismatch: persist `ReconciliationEvent`, adopt exchange state, alert operator

**Time to recover**: <60s (Kubernetes restart + replay)

### Scenario 2: EventLog Write Failure (Postgres Down)

1. Runtime detects write failure
2. Events buffered in memory (bounded: 100 events max)
3. Alert operator: "EventLog write failure — operating on memory buffer"
4. Retry every 5 seconds
5. If buffer fills: HALT all activity (losing events is worse than missing trades)
6. When Postgres recovers: flush buffer, resume normal operation

### Scenario 3: Exchange Connection Lost

1. WebSocket disconnect detected
2. Attempt reconnection with exponential backoff (1s, 2s, 4s, ... 30s max)
3. If disconnect >10s: switch to REST polling (1s interval) for active position prices
4. If disconnect >5min: alert operator, enter defensive mode (no new positions, existing stops remain on exchange)
5. When reconnected: reconcile state, resume normal operation

### Scenario 4: State Corruption (Detected via Reconciliation)

1. Periodic reconciliation detects mismatch between RuntimeState and exchange for a
   position that IS tracked in `event_log` (e.g., a missed fill, a slippage outside
   tolerance).
2. Persist `StateCorruptionDetected` event with full diff.
3. For **tracked** positions: adopt exchange state as truth (exchange is the real
   world), rebuild RuntimeState from EventLog + exchange reconciliation.
4. Alert operator with details.
5. If corruption source is identifiable (e.g., missed fill event): log as
   `MissedEvent` for investigation.

**Important**: adoption applies ONLY to positions whose entry is already recorded in
`event_log` with a matching exchange order id. A position without a matching
`entry_order_placed` event is UNTRACKED and handled by Scenario 5 — not by adoption.
Back-filling an `entry_order_placed` event for an UNTRACKED position is a policy
violation (ADR-0022).

### Scenario 5: UNTRACKED Position Detected

Triggered when the Position Reconciliation Worker finds an open exchange position
for which no matching `entry_order_placed` event exists in `event_log`. See
[ADR-0022](../adr/ADR-0022-robson-authored-position-invariant.md) and
[docs/policies/UNTRACKED-POSITION-RECONCILIATION.md](../policies/UNTRACKED-POSITION-RECONCILIATION.md).

1. Persist `position_untracked_detected` with evidence: exchange order id, symbol,
   side, quantity, open timestamp.
2. Emit a CRITICAL operator alert.
3. Close the position at market via the Safety Net close path, tagged exit reason
   `UNTRACKED_ON_EXCHANGE`.
4. Persist `untracked_position_closed` with the resulting fill.
5. Do NOT reconstruct an `entry_order_placed` event. Do NOT adopt the position into
   RuntimeState before closing it.

This path is unconditional — not gated by `ROBSON_POSITION_MONITOR_ENABLED` or
`allowed_symbols`. The only permitted override is an operator-issued
`POST /reconciliation/suspend` (max TTL 300 s, fully audited — v3 target).

At daemon startup, the Runtime MUST enter `StartupReconciling` and resolve all
UNTRACKED positions before accepting non-critical observations.
