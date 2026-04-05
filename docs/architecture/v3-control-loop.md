# ROBSON v3 — CONTROL LOOP SPECIFICATION

**Date**: 2026-04-03  
**Status**: APPROVED  
**Owner**: Runtime (robsond)

---

## Overview

The Control Loop is the heartbeat of Robson. It is the single execution path through which ALL system behavior flows. The Runtime (robsond) is the EXCLUSIVE owner of the Control Loop. No other component may initiate, pause, interrupt, or restart a cycle outside the Runtime.

```
Observe -> Interpret -> Decide -> Act -> Evaluate -> Persist
```

### Implementation: QueryEngine

The Control Loop is implemented by the **QueryEngine** (`robsond/src/query_engine.rs`). Every trigger that enters the Runtime becomes a typed **ExecutionQuery** (`robsond/src/query.rs`) that progresses through the pipeline above. The QueryEngine is the ONLY path to mutate RuntimeState.

See **[v3-query-query-engine.md](v3-query-query-engine.md)** for the full specification, including state machine, phased implementation, and the `state = source of truth, stream = projection` architectural premise.

---

## Stage Specification

Each control loop cycle progresses through 6 sequential stages.
These are **pipeline stages within a single execution tick**, not project phases.
For project-level identifiers see v3-migration-plan.md §1.1 (`MIG-*`, `QE-P*`).

### Stage 1: Observe

**Owner**: Runtime (MarketDataManager, API Server, Timer)

**Input**: Raw external event from one of:
- Binance WebSocket (market tick)
- Binance WebSocket (order fill notification)
- HTTP API (operator command)
- HTTP API (detector signal injection)
- Internal timer (periodic health check)

**Output**: Typed `Observation` enum:
```rust
pub enum Observation {
    MarketTick {
        symbol: Symbol,
        bid: Price,
        ask: Price,
        timestamp: DateTime<Utc>,
    },
    DetectorSignal {
        signal_id: Ulid,
        symbol: Symbol,
        side: Side,
        entry_price: Price,
        tech_stop: Price,
        confidence: Decimal,
    },
    OperatorCommand {
        command: Command,  // Arm, Disarm, Panic, Pause, Resume, AdjustRisk
        params: CommandParams,
        issued_at: DateTime<Utc>,
    },
    OrderFill {
        order_id: OrderId,
        fill_price: Price,
        fill_quantity: Quantity,
        commission: Decimal,
        timestamp: DateTime<Utc>,
    },
    TimerFire {
        interval_id: String,
        fired_at: DateTime<Utc>,
    },
}
```

**Deterministic**: Yes. Raw bytes are parsed into typed observations. Parsing is deterministic.

**Error handling**: Malformed input is logged as `ObservationError` event and dropped. Cycle does not proceed.

---

### Stage 2: Interpret

**Owner**: Runtime (PositionManager)

**Input**: `Observation` + current `RuntimeState` (positions, orders, risk snapshot)

**Output**: Typed `Interpretation` enum:
```rust
pub enum Interpretation {
    StopBreached {
        position_id: PositionId,
        trigger_price: Price,
        stop_price: Price,
        distance: Decimal,
    },
    SignalValid {
        signal_id: Ulid,
        position_id: PositionId,
        calculated_size: Quantity,
    },
    SignalRejected {
        signal_id: Ulid,
        reason: String,
    },
    TrailingStopUpdate {
        position_id: PositionId,
        new_favorable_extreme: Price,
        new_stop: Price,
        previous_stop: Price,
    },
    OrderFillProcessed {
        order_id: OrderId,
        position_id: PositionId,
        new_position_state: PositionState,
    },
    CommandAccepted {
        command: Command,
    },
    RiskAlert {
        kind: RiskAlertKind,  // ApproachingLimit, ThresholdBreached, CircuitBreakerTriggered
        details: String,
    },
    NoAction,
}
```

**Deterministic**: Yes. Given the same observation and state, interpretation is identical.

**Logic**:
- `MarketTick` -> check all active positions for trailing stop breach -> `StopBreached` or `TrailingStopUpdate` or `NoAction`
- `DetectorSignal` -> validate signal matches armed position -> calculate position size via Golden Rule -> `SignalValid` or `SignalRejected`
- `OperatorCommand` -> validate command is valid for current state -> `CommandAccepted`
- `OrderFill` -> update position state machine -> `OrderFillProcessed`
- `TimerFire` -> health check, reconciliation -> `NoAction` (or `RiskAlert` if anomaly detected)

---

### Stage 3: Decide

**Owner**: Engine (robson-engine) — PURE, NO I/O

**Input**: `Interpretation` + `RiskLimits` + `PositionState`

**Output**: `EngineAction` enum:
```rust
pub enum EngineAction {
    PlaceEntryOrder {
        position_id: PositionId,
        symbol: Symbol,
        side: Side,
        quantity: Quantity,
        signal_id: Ulid,
    },
    UpdateTrailingStop {
        position_id: PositionId,
        previous_stop: Price,
        new_stop: Price,
        trigger_price: Price,
    },
    TriggerExit {
        position_id: PositionId,
        exit_reason: ExitReason,
        trigger_price: Price,
        stop_price: Price,
    },
    RejectTrade {
        reason: RiskVerdict,
    },
    AdjustConfig {
        changes: ConfigDelta,
    },
    PanicClose {
        position_ids: Vec<PositionId>,
    },
    NoOp,
}
```

**Deterministic**: Yes. The Engine is a pure function with zero side effects.

**Risk Gate (sub-stage)**:
Before the Engine decision is accepted, the Risk Engine evaluates:
1. Is the system in a state where this action is allowed? (circuit breaker check)
2. Does this specific action violate any limit? (exposure check, position count, daily loss)

If denied, `EngineAction` is replaced with `RejectTrade { reason: RiskVerdict }`.

---

### Stage 4: Act

**Owner**: Executor (robson-exec) via GovernedAction

**Input**: `GovernedAction` (constructed by Runtime with Risk Engine clearance proof)

**Output**: `ActionResult` enum:
```rust
pub enum ActionResult {
    OrderPlaced {
        order_id: OrderId,
        exchange_order_id: String,
        status: OrderStatus,
    },
    OrderFailed {
        reason: String,
        retryable: bool,
    },
    TrailingStopUpdated {
        position_id: PositionId,
    },
    Blocked {
        guard: String,
        reason: String,
    },
    NoAction,
}
```

**Deterministic**: NO. This stage interacts with external systems (Binance API). Network latency, exchange state, and market conditions introduce non-determinism.

**Audit**: Full request/response logged. Exchange order ID captured for reconciliation.

**Retry policy**: 3 attempts with exponential backoff (100ms, 500ms, 2s). If all retries fail: `ActionResult::OrderFailed { retryable: false }`, cycle continues to Evaluate.

---

### Stage 5: Evaluate

**Owner**: Runtime (PositionManager)

**Input**: `ActionResult` + current `PositionState`

**Output**: 
- New `PositionState` (state machine transition)
- `Vec<DomainEvent>` (events to persist)

**Deterministic**: Yes. Given the same ActionResult and state, the new state and events are identical.

**State transitions**:
```
OrderPlaced (entry) -> PositionState::Entering
OrderPlaced (exit)  -> PositionState::Exiting
OrderFailed         -> PositionState::Error (if retries exhausted)
TrailingStopUpdated -> PositionState::Active (updated trailing stop)
Blocked             -> PositionState unchanged (action was prevented)
```

---

### Stage 6: Persist

**Owner**: EventLog (robson-eventlog)

**Input**: `Vec<DomainEvent>` from Evaluate stage

**Output**: `Vec<EventEnvelope>` (persisted events with assigned event_id, sequence, timestamp)

**Deterministic**: Yes. Append-only, idempotent (SHA256 dedup key).

**Events persisted per cycle**:
1. `CycleStarted { cycle_id, trigger, timestamp }`
2. Domain events from Evaluate (0 to N events)
3. `RiskDecision { verdict, limits_applied, exposure_snapshot }`
4. `PermissionCheck { action, decision, override }`
5. `CycleCompleted { cycle_id, duration, events_produced }`

**Failure handling**: If EventLog append fails (Postgres down), cycle enters error state. Runtime retries persist 3 times. If still failing: halt all activity, alert operator, cache events in memory (bounded buffer, 100 events). Resume when Postgres recovers.

---

## Cycle Triggers

| Priority | Trigger | Source | Queuing |
|----------|---------|--------|---------|
| Critical | Circuit breaker activation | Risk Engine | Preempts current cycle at next safe point |
| High | Operator command (panic, pause) | CLI / API | Front of queue |
| High | Risk alert | Risk Engine monitoring | Front of queue |
| Normal | Detector signal | External / manual | FIFO queue |
| Normal | Market tick | Binance WebSocket | FIFO queue, droppable if queue >800 |
| Normal | Order fill | Binance WebSocket | FIFO queue, never dropped |
| Low | Timer fire | Internal | FIFO queue, droppable |

**Queue specification**:
- Bounded channel, capacity 1000
- Priority levels: Critical > High > Normal > Low
- When queue >800: drop oldest Normal/Low observations (not fills, not operator commands)
- `QueueOverflow` event logged when dropping occurs

---

## Interruption Protocol

### Pause

```
Operator issues PAUSE command
-> Current cycle completes normally
-> After Persist stage: set RuntimeState.paused = true
-> Observe stage checks paused flag: skip all non-critical observations
-> Only Critical and High priority triggers processed
-> PauseActivated event persisted
```

### Resume

```
Operator issues RESUME command
-> RuntimeState.paused = false
-> Normal queue processing resumes
-> ResumeActivated event persisted
```

### Circuit Breaker

```
Risk Engine detects threshold breach
-> CircuitBreakerActivated event persisted
-> Current cycle completes Evaluate + Persist
-> Next cycle: all PlaceEntryOrder actions automatically denied
-> Escalation timer starts (see Risk Engine specification)
-> Only operator CircuitBreakerReset command restores normal operation
```

### Crash Recovery

```
Runtime restarts (Kubernetes pod restart)
-> Load RuntimeState from EventLog replay
-> Query Binance for actual position state
-> Compare: EventLog state vs Exchange state
-> If match: resume normal operation
-> If mismatch: 
   - Adopt exchange state as truth
   - Persist ReconciliationEvent with discrepancy details
   - Alert operator
   - Resume with reconciled state
```

---

## Timing Constraints

| Metric | Target | Measurement |
|--------|--------|-------------|
| Full cycle duration (tick -> persist) | < 50ms (no exchange interaction) | Histogram metric: `robson_cycle_duration_ms` |
| Full cycle duration (with exchange) | < 500ms | Histogram metric: `robson_cycle_with_exchange_duration_ms` |
| Observation queue latency | < 10ms at p99 | Histogram metric: `robson_queue_latency_ms` |
| Risk Engine evaluation | < 10ms | Histogram metric: `robson_risk_eval_duration_ms` |
| EventLog append | < 5ms | Histogram metric: `robson_eventlog_append_ms` |

---

## Observability

### Metrics (Prometheus)

```
robson_cycles_total{trigger_type}           # Counter: total cycles by trigger
robson_cycle_duration_ms{stage}             # Histogram: duration per stage
robson_queue_depth                          # Gauge: current observation queue depth
robson_risk_decisions_total{verdict}        # Counter: approved/denied
robson_circuit_breaker_state                # Gauge: 0=closed, 1=open, 2=half_open
robson_positions_active                     # Gauge: active positions count
robson_eventlog_sequence                    # Gauge: latest event sequence number
```

### Logs (structured JSON via tracing)

Every cycle produces a structured log entry:
```json
{
  "cycle_id": "01HXYZ...",
  "trigger": "market_tick",
  "symbol": "BTCUSDT",
  "phases": {
    "observe_ms": 1,
    "interpret_ms": 2,
    "decide_ms": 1,
    "act_ms": 0,
    "evaluate_ms": 1,
    "persist_ms": 3
  },
  "events_produced": 1,
  "risk_verdict": "approved",
  "queue_depth_at_start": 12
}
```

### Traces (OpenTelemetry)

Each cycle is a trace span with child spans per stage. Trace ID = cycle_id for correlation.
