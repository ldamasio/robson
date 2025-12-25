# ADR-0012: Event-Sourced Stop-Loss Monitor with Rust WebSocket Service

**Status**: Draft (REVISED)
**Date**: 2024-12-25
**Deciders**: System Architect
**Related**: ADR-0007 (Strategy Semantics), ADR-0011 (GitOps)

---

## Context

### Current State Analysis

**Stop-Loss Monitoring** (`apps/backend/monolith/api/application/stop_monitor.py`):
- Kubernetes CronJob running every **60 seconds**
- Uses **percentage-based stops** (`operation.stop_loss_percent` - line 108)
- Executes via `place_market()` adapter (CORRECT: market orders, not exchange stops)
- Currently in **DRY-RUN mode** in production (`infra/k8s/prod/rbs-stop-monitor-cronjob.yml:35`)

**Critical Gaps Identified**:

#### 1. **Stop Calculated from Percentage, Not Absolute Price**

**Problem** (`stop_monitor.py:108-112`):
```python
if operation.stop_loss_percent:
    if operation.side == "BUY":
        stop_loss_price = entry_price * (1 - operation.stop_loss_percent / 100)
    else:
        stop_loss_price = entry_price * (1 + operation.stop_loss_percent / 100)
```

**Issue**:
- Stop level **recalculated** on every check based on `average_entry_price`
- If entry price changes (e.g., adding to position), stop level **shifts**
- Violates GOLDEN RULE: stop should be a **fixed technical level**, not a moving target

**Correct Approach**:
- Store `stop_price` as **absolute value** (e.g., $93,500.00) at operation creation
- Never recalculate; always compare current price vs. fixed `stop_price`

#### 2. **No Idempotency Mechanism**

**Problem**:
- If CronJob runs twice due to k8s pod restart, could execute stop twice
- No deduplication token to prevent duplicate orders
- Race conditions between multiple monitor instances

#### 3. **No Audit Trail (Mutable State)**

**Problem** (`trading.py:232-237`):
```python
STATUS_CHOICES = [
    ("PLANNED", "Planned"),
    ("ACTIVE", "Active"),
    ("CLOSED", "Closed"),
    ("CANCELLED", "Cancelled"),
]
```

**Issues**:
- State transitions **overwrite** previous status (no history)
- Can't answer: "When exactly was stop triggered?" "How many times did we retry?"
- No source attribution (was it WS or CronJob that executed?)

#### 4. **No Risk Guardrails**

**Missing**:
- Slippage limits (what if price gaps 10% past stop?)
- Circuit breaker (pause trading if market crashes)
- Kill switch per tenant (emergency stop all trading)
- Stale price handling (what if WebSocket disconnects?)

#### 5. **Technical Stop Logic Disconnected**

- `api/domain/technical_stop.py`: Sophisticated stop calculation EXISTS ✅
- `user_operations.py:306-312`: Operation created WITHOUT `stop_price` field ❌
- Monitor can't use technical stops because they're not persisted ❌

---

## Decision

### Architecture: Event-Sourced Stop-Loss Monitor

**Core Principles**:

1. **Event Sourcing**: All state changes are **append-only events** (immutable audit trail)
2. **Absolute Price Stops**: Persist `stop_price` as fixed technical level (never recalculate)
3. **Idempotency**: Global unique token prevents duplicate executions
4. **Risk Guardrails**: Slippage limits, circuit breakers, kill switches
5. **Stale Price Handling**: Explicit policy for WebSocket disconnection
6. **Market Orders**: Always MARKET, never exchange stop-orders (rationale below)

---

### Why Market Orders for Stop-Loss Execution

**Decision**: The system uses market orders for stop-loss execution by design.

**This decision is justified by**:

1. **Isolated Margin Architecture**: Positions operate under isolated margin, preventing cross-position contagion.

2. **Small Position Sizing**: Position sizes are intentionally small (1% risk rule), making worst-case slippage financially negligible.

3. **Execution Risk > Slippage Risk**: The primary risk is **missed execution** (position left exposed), not adverse price impact from slippage.

4. **Order Book Privacy**: Market orders ensure deterministic execution and avoid intent disclosure in the order book (limit orders telegraph stop levels to market makers).

5. **Operational Simplicity**: Market orders eliminate edge cases (partial fills, price gaps past limit, order rejection).

**Given the above, market orders provide the lowest operational risk profile for automated stop execution.**

---

## Architecture Design

### Component Diagram

```
┌─────────────────────────────────────────────────────────────────┐
│                     Binance WebSocket API                       │
│                 (Market Data Stream - Real-Time)                │
└────────────────────────────┬────────────────────────────────────┘
                             │
                    ┌────────▼────────┐
                    │  Rust Service   │ ◄──── CRITICAL PATH
                    │  rbs-stop-exec  │       (Event Producer)
                    │                 │
                    │  - Consumes WS  │
                    │  - Detects      │
                    │  - Emits Events │
                    │  - Executes     │
                    └────┬────────┬───┘
                         │        │
             ┌───────────┘        └─────────────┐
             │                                   │
    ┌────────▼────────┐                ┌────────▼────────┐
    │   PostgreSQL    │                │   RabbitMQ      │
    │                 │                │   (Outbox)      │
    │  - stop_events  │                │                 │
    │  - stop_exec    │                │  Routing:       │
    │  - operations   │                │   stop.trigger  │
    │  (Event Store)  │                │   stop.executed │
    └─────────────────┘                └────────┬────────┘
                                                 │
                                        ┌────────┴────────┐
                                        │                 │
                                ┌───────▼──────┐  ┌──────▼──────┐
                                │  Redis Cache │  │ Go Service  │
                                │              │  │ rbs-ws-     │
                                │ - Last Price │  │ notify      │
                                │ - Staleness  │  │             │
                                │ - Kill Switch│  │ - Subscribe │
                                └──────────────┘  │ - Broadcast │
                                                  └──────┬──────┘
                                                         │
                                                  ┌──────▼──────┐
                                                  │  Frontend   │
                                                  │  Dashboard  │
                                                  └─────────────┘

┌─────────────────────────────────────────────────────────────────┐
│           Backstop/Reconciliation (Every 1 minute)              │
│                                                                 │
│   Python CronJob (monitor_stops.py - NO --dry-run)             │
│   - Same idempotency mechanism (reads stop_events)             │
│   - Detects missed executions (if Rust crashed)                │
│   - Publishes to same Outbox (source=cron)                     │
└─────────────────────────────────────────────────────────────────┘
```

---

### RabbitMQ vs Redis: Separation of Responsibilities

| Component | Role | Technology | Rationale |
|-----------|------|------------|-----------|
| **Event Bus** | Durable async messaging, retries, dead-letter queues | **RabbitMQ** | AMQP guarantees, message persistence, retry logic |
| **Cache/Fast Lookup** | Last known price, staleness flags, kill switches | **Redis** | Sub-ms latency, TTL for staleness, atomic flags |
| **Event Store** | Immutable audit trail, source of truth | **PostgreSQL** | ACID, event replay, complex queries |

**RabbitMQ Exchanges/Queues**:
```
Exchange: stop_events (topic)
├── Queue: stop_events.critical  (Rust consumer)
├── Queue: stop_events.backstop  (CronJob consumer)
├── Queue: stop_events.notify    (Go WS consumer)
└── DLQ:   stop_events.dlq       (failed events)

Routing Keys:
- stop.trigger.{tenant_id}.{symbol}
- stop.executed.{tenant_id}.{symbol}
- stop.failed.{tenant_id}.{symbol}
- stop.blocked.{tenant_id}.{symbol}
```

**Redis Keys**:
```
price:{symbol}         → Last known price (SET with EX 30s)
price_staleness:{symbol} → Timestamp of last update
kill_switch:{tenant_id}  → 1 = trading paused, 0 = active
slippage_breaker:{symbol} → Circuit breaker state (OPEN/CLOSED)
```

---

## Event Sourcing Schema

### Table: `stop_events` (Append-Only Event Store)

**Purpose**: Immutable audit trail of ALL stop-loss/stop-gain events.

```sql
CREATE TABLE stop_events (
    -- Event identity
    event_id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    event_seq BIGSERIAL,  -- Global sequence for ordering
    occurred_at TIMESTAMP WITH TIME ZONE DEFAULT NOW() NOT NULL,

    -- Operation context
    operation_id BIGINT NOT NULL REFERENCES operation(id),
    tenant_id BIGINT NOT NULL REFERENCES client(id),  -- Multi-tenant isolation
    symbol VARCHAR(20) NOT NULL,

    -- Event type (state transition)
    event_type VARCHAR(50) NOT NULL,
    -- Values: STOP_TRIGGERED, EXECUTION_SUBMITTED, EXECUTED,
    --         FAILED, BLOCKED, STALE_PRICE, KILL_SWITCH, SLIPPAGE_BREACH

    -- Stop-loss parameters (captured at event time)
    trigger_price NUMERIC(20, 8),  -- Price that triggered stop
    stop_price NUMERIC(20, 8),     -- Configured stop level (absolute)
    quantity NUMERIC(20, 8),
    side VARCHAR(10),  -- BUY/SELL (closing direction)

    -- Idempotency
    execution_token VARCHAR(64) UNIQUE,  -- Global dedup key

    -- Payload
    payload_json JSONB,  -- Full context (entry_price, slippage_limit, etc.)
    request_payload_hash VARCHAR(64),  -- Hash for deduplication

    -- Execution results
    exchange_order_id VARCHAR(100),  -- Binance order ID (if executed)
    fill_price NUMERIC(20, 8),       -- Actual fill price
    slippage_pct NUMERIC(10, 4),     -- Calculated slippage

    -- Source attribution
    source VARCHAR(20) NOT NULL,  -- 'ws', 'cron', 'manual'

    -- Error tracking
    error_message TEXT,
    retry_count INT DEFAULT 0,

    -- Indexes for queries
    CONSTRAINT check_event_type CHECK (
        event_type IN (
            'STOP_TRIGGERED', 'EXECUTION_SUBMITTED', 'EXECUTED',
            'FAILED', 'BLOCKED', 'STALE_PRICE', 'KILL_SWITCH',
            'SLIPPAGE_BREACH', 'CIRCUIT_BREAKER'
        )
    )
);

-- Indexes
CREATE INDEX idx_stop_events_operation ON stop_events(operation_id, event_seq);
CREATE INDEX idx_stop_events_tenant ON stop_events(tenant_id, occurred_at);
CREATE INDEX idx_stop_events_token ON stop_events(execution_token) WHERE execution_token IS NOT NULL;
CREATE INDEX idx_stop_events_type ON stop_events(event_type, occurred_at);
CREATE INDEX idx_stop_events_source ON stop_events(source, occurred_at);
```

**Key Design Decisions**:

1. **Immutable**: Never UPDATE events, only INSERT
2. **Global Sequence**: `event_seq` for total ordering (event replay)
3. **Unique Token**: `execution_token` prevents duplicate executions
4. **Source Attribution**: Track if execution came from WS, CronJob, or manual intervention
5. **Full Payload**: `payload_json` stores complete context for debugging

---

### Table: `stop_executions` (Execution State Projection)

**Purpose**: Materialized view of latest execution state per operation (derived from `stop_events`).

```sql
CREATE TABLE stop_executions (
    -- Primary key
    execution_id UUID PRIMARY KEY DEFAULT gen_random_uuid(),

    -- Operation reference
    operation_id BIGINT NOT NULL REFERENCES operation(id),
    tenant_id BIGINT NOT NULL REFERENCES client(id),

    -- Idempotency token (unique across ALL executions)
    execution_token VARCHAR(64) UNIQUE NOT NULL,

    -- Execution state (derived from latest event)
    status VARCHAR(50) NOT NULL DEFAULT 'PENDING',
    -- Values: PENDING, SUBMITTED, EXECUTED, FAILED, BLOCKED

    -- Stop parameters (absolute price)
    stop_price NUMERIC(20, 8) NOT NULL,  -- Fixed technical level
    trigger_price NUMERIC(20, 8),        -- Price at detection
    quantity NUMERIC(20, 8) NOT NULL,
    side VARCHAR(10) NOT NULL,

    -- Timestamps (from events)
    triggered_at TIMESTAMP WITH TIME ZONE,
    submitted_at TIMESTAMP WITH TIME ZONE,
    executed_at TIMESTAMP WITH TIME ZONE,
    failed_at TIMESTAMP WITH TIME ZONE,

    -- Execution results
    exchange_order_id VARCHAR(100),
    fill_price NUMERIC(20, 8),
    slippage_pct NUMERIC(10, 4),

    -- Source and error tracking
    source VARCHAR(20) NOT NULL,  -- 'ws', 'cron'
    error_message TEXT,
    retry_count INT DEFAULT 0,

    -- Audit
    created_at TIMESTAMP WITH TIME ZONE DEFAULT NOW() NOT NULL,
    updated_at TIMESTAMP WITH TIME ZONE DEFAULT NOW() NOT NULL,

    CONSTRAINT check_status CHECK (
        status IN ('PENDING', 'SUBMITTED', 'EXECUTED', 'FAILED', 'BLOCKED')
    )
);

-- Indexes
CREATE UNIQUE INDEX idx_stop_exec_token ON stop_executions(execution_token);
CREATE INDEX idx_stop_exec_operation ON stop_executions(operation_id, status);
CREATE INDEX idx_stop_exec_tenant ON stop_executions(tenant_id, status);
```

**Relationship to `stop_events`**:
```
stop_events (append-only source of truth)
    ↓ (aggregate/project)
stop_executions (current state view)
```

**Update Pattern** (triggered by new event):
```python
def on_stop_event(event: StopEvent):
    """Update stop_executions based on new event."""
    if event.event_type == 'STOP_TRIGGERED':
        # Create new execution record
        StopExecution.objects.create(
            execution_token=event.execution_token,
            operation_id=event.operation_id,
            status='PENDING',
            triggered_at=event.occurred_at,
            # ... other fields
        )
    elif event.event_type == 'EXECUTION_SUBMITTED':
        # Update status
        StopExecution.objects.filter(
            execution_token=event.execution_token
        ).update(
            status='SUBMITTED',
            submitted_at=event.occurred_at,
        )
    elif event.event_type == 'EXECUTED':
        # Final state
        StopExecution.objects.filter(
            execution_token=event.execution_token
        ).update(
            status='EXECUTED',
            executed_at=event.occurred_at,
            exchange_order_id=event.exchange_order_id,
            fill_price=event.fill_price,
        )
```

---

### Table: `operation` (Schema Changes)

**Add fields for absolute price stops**:

```sql
ALTER TABLE operation
ADD COLUMN stop_price NUMERIC(20, 8),          -- ⭐ Absolute stop level
ADD COLUMN target_price NUMERIC(20, 8),        -- ⭐ Absolute target level
ADD COLUMN stop_execution_token VARCHAR(64),   -- Current execution token (if triggered)
ADD COLUMN last_stop_check_at TIMESTAMP WITH TIME ZONE,
ADD COLUMN stop_check_count INT DEFAULT 0;

-- Keep stop_loss_percent for reference/calculation, but monitor uses stop_price
-- DEPRECATE using stop_loss_percent in monitor logic
```

**Migration Strategy**:
```python
# Backfill stop_price from stop_loss_percent for existing operations
for op in Operation.objects.filter(stop_price__isnull=True):
    if op.stop_loss_percent and op.average_entry_price:
        if op.side == 'BUY':
            op.stop_price = op.average_entry_price * (
                Decimal('1') - op.stop_loss_percent / Decimal('100')
            )
        else:
            op.stop_price = op.average_entry_price * (
                Decimal('1') + op.stop_loss_percent / Decimal('100')
            )
        op.save(update_fields=['stop_price'])
```

---

### Table: `tenant_config` (Risk Guardrails)

**Purpose**: Per-tenant risk controls (kill switches, limits).

```sql
CREATE TABLE tenant_config (
    tenant_id BIGINT PRIMARY KEY REFERENCES client(id),

    -- Kill switch
    trading_enabled BOOLEAN DEFAULT TRUE,
    trading_paused_reason TEXT,
    trading_paused_at TIMESTAMP WITH TIME ZONE,

    -- Slippage limits
    max_slippage_pct NUMERIC(10, 4) DEFAULT 5.0,  -- Max 5% slippage
    slippage_pause_threshold_pct NUMERIC(10, 4) DEFAULT 10.0,  -- Circuit breaker at 10%

    -- Rate limits
    max_executions_per_minute INT DEFAULT 10,
    max_executions_per_hour INT DEFAULT 100,

    -- Audit
    created_at TIMESTAMP WITH TIME ZONE DEFAULT NOW() NOT NULL,
    updated_at TIMESTAMP WITH TIME ZONE DEFAULT NOW() NOT NULL
);
```

---

### Table: `circuit_breaker_state` (Per-Symbol Circuit Breaker)

**Purpose**: Track circuit breaker state per trading pair.

```sql
CREATE TABLE circuit_breaker_state (
    symbol VARCHAR(20) PRIMARY KEY,

    -- State
    state VARCHAR(20) DEFAULT 'CLOSED',  -- CLOSED, OPEN, HALF_OPEN
    -- CLOSED = normal trading
    -- OPEN = circuit tripped, no trading
    -- HALF_OPEN = testing if market recovered

    -- Metrics
    failure_count INT DEFAULT 0,
    last_failure_at TIMESTAMP WITH TIME ZONE,
    opened_at TIMESTAMP WITH TIME ZONE,
    will_retry_at TIMESTAMP WITH TIME ZONE,

    -- Thresholds
    failure_threshold INT DEFAULT 3,  -- Trip after 3 consecutive failures
    retry_delay_seconds INT DEFAULT 300,  -- 5 minutes

    -- Audit
    updated_at TIMESTAMP WITH TIME ZONE DEFAULT NOW() NOT NULL
);
```

---

## Idempotency Flow (End-to-End)

### Token Generation & Deduplication

**Rule**: One `execution_token` per stop trigger, globally unique.

**Format**: `{operation_id}:{stop_price}:{timestamp_ms}`

**Example**: `12345:93500.00:1703520934123`

**Why This Format?**:
- `operation_id`: Scope to specific operation
- `stop_price`: Prevent duplicate triggers at different levels (if stop changes)
- `timestamp_ms`: Uniqueness across retries

---

### Flow Diagram: Event-Sourced Execution

```
┌──────────────────────────────────────────────────────────────────┐
│ 1. Price Update (Binance WS → Rust Service)                     │
│    BTCUSDC: $94,800                                              │
└────────────────────────┬─────────────────────────────────────────┘
                         │
    ┌────────────────────▼──────────────────────┐
    │ 2. Query Active Operations (Rust)         │
    │    SELECT id, stop_price, side, qty       │
    │    FROM operation                         │
    │    WHERE status = 'ACTIVE'                │
    │    AND symbol_id = (SELECT id FROM symbol │
    │                     WHERE name = 'BTCUSDC')│
    └────────────────────┬──────────────────────┘
                         │
    ┌────────────────────▼──────────────────────────────┐
    │ 3. Trigger Detection (Per Operation)              │
    │    Op #456:                                       │
    │      stop_price: $95,000 (ABSOLUTE, from DB)      │
    │      current_price: $94,800                       │
    │      Triggered? YES ✅ (current <= stop)          │
    └────────────────────┬──────────────────────────────┘
                         │
    ┌────────────────────▼──────────────────────────────┐
    │ 4. Generate Idempotency Token (Rust)              │
    │    token = "456:95000.00:1703520934123"           │
    └────────────────────┬──────────────────────────────┘
                         │
    ┌────────────────────▼──────────────────────────────┐
    │ 5. Pre-Execution Guardrails (Rust)                │
    │    ✅ Check kill switch (Redis)                   │
    │    ✅ Check circuit breaker (Redis)               │
    │    ✅ Check price staleness (Redis)               │
    │    ✅ Estimate slippage (current vs stop)         │
    │    ⚠️  If ANY check fails → Emit BLOCKED event    │
    └────────────────────┬──────────────────────────────┘
                         │
    ┌────────────────────▼──────────────────────────────┐
    │ 6. Emit STOP_TRIGGERED Event (Append-Only)        │
    │    INSERT INTO stop_events (                      │
    │      event_type = 'STOP_TRIGGERED',               │
    │      execution_token = token,                     │
    │      operation_id = 456,                          │
    │      trigger_price = 94800,                       │
    │      stop_price = 95000,                          │
    │      source = 'ws'                                │
    │    )                                              │
    │    ON CONFLICT (execution_token) DO NOTHING       │
    │    RETURNING event_id;                            │
    │                                                   │
    │    IF event_id IS NULL:                           │
    │      → Token already exists, ABORT (idempotent)   │
    └────────────────────┬──────────────────────────────┘
                         │
    ┌────────────────────▼──────────────────────────────┐
    │ 7. Publish to Outbox (Rust → RabbitMQ)            │
    │    INSERT INTO outbox (                           │
    │      event_id,                                    │
    │      routing_key = 'stop.trigger.tenant1.BTCUSDC',│
    │      payload = {...}                              │
    │    )                                              │
    │                                                   │
    │    Outbox Worker (separate process):             │
    │      PUBLISH to RabbitMQ → DELETE from outbox     │
    └────────────────────┬──────────────────────────────┘
                         │
    ┌────────────────────▼──────────────────────────────┐
    │ 8. Place Market Order (Rust → Binance)            │
    │    Emit EXECUTION_SUBMITTED event BEFORE call     │
    │                                                   │
    │    POST /api/v3/order                             │
    │    {                                              │
    │      "symbol": "BTCUSDC",                         │
    │      "side": "SELL",                              │
    │      "type": "MARKET",                            │
    │      "quantity": 0.055                            │
    │    }                                              │
    │                                                   │
    │    Response: { "orderId": 7891234, ... }          │
    └────────────────────┬──────────────────────────────┘
                         │
    ┌────────────────────▼──────────────────────────────┐
    │ 9. Emit EXECUTED Event (Append-Only)              │
    │    INSERT INTO stop_events (                      │
    │      event_type = 'EXECUTED',                     │
    │      execution_token = token,  (SAME TOKEN!)      │
    │      exchange_order_id = 7891234,                 │
    │      fill_price = 94810,                          │
    │      slippage_pct = -0.01,  (better than stop!)   │
    │      source = 'ws'                                │
    │    )                                              │
    └────────────────────┬──────────────────────────────┘
                         │
    ┌────────────────────▼──────────────────────────────┐
    │ 10. Update Operation Status (Django/Rust)         │
    │     UPDATE operation                              │
    │     SET status = 'CLOSED',                        │
    │         stop_execution_token = token              │
    │     WHERE id = 456;                               │
    │                                                   │
    │     INSERT INTO order (exit order details)        │
    └────────────────────┬──────────────────────────────┘
                         │
    ┌────────────────────▼──────────────────────────────┐
    │ 11. Publish EXECUTED Event (RabbitMQ)             │
    │     Routing Key: stop.executed.tenant1.BTCUSDC    │
    │                                                   │
    │     Consumers:                                    │
    │     - Go WS: Broadcast to frontend dashboard      │
    │     - Audit Logger: Archive to cold storage       │
    └───────────────────────────────────────────────────┘
```

---

### Retry & Failure Handling

**Scenario 1: Binance API 503 (Temporary Failure)**

```
1. EXECUTION_SUBMITTED event emitted (token: ABC123)
2. Binance API call → 503 Service Unavailable
3. Emit FAILED event (retry_count=1, error='503')
4. Retry after 2s (exponential backoff)
5. EXECUTION_SUBMITTED event (SAME token: ABC123)
6. Binance API call → 200 OK
7. Emit EXECUTED event (token: ABC123)
```

**Key**: Same token across retries = idempotent.

**Scenario 2: Duplicate Detection (CronJob runs after WS)**

```
1. Rust WS emits STOP_TRIGGERED (token: ABC123) ✅
2. Rust executes successfully
3. 30s later, CronJob runs (backstop)
4. CronJob generates same token: ABC123
5. CronJob tries: INSERT INTO stop_events ... (token: ABC123)
6. DB returns: ON CONFLICT (execution_token) DO NOTHING
7. CronJob logs: "Already executed by ws, skipping" ✅
```

**Scenario 3: Kill Switch Activated**

```
1. Admin activates kill switch for tenant_id=1
   UPDATE tenant_config SET trading_enabled=FALSE
   SET kill_switch:tenant_1 = 1 (Redis)

2. Rust detects trigger for op #456 (tenant_id=1)
3. Pre-execution check: Redis GET kill_switch:tenant_1 → 1 (paused)
4. Emit KILL_SWITCH event (status=BLOCKED, no execution)
5. Log: "Trading paused for tenant 1, stop not executed"
```

**Scenario 4: Stale Price (WebSocket Disconnected)**

```
1. Binance WebSocket disconnects at 10:00:00
2. Redis key price:BTCUSDC expires after 30s (TTL)
3. At 10:00:35, trigger detected
4. Check staleness:
   last_update = Redis GET price_staleness:BTCUSDC → 10:00:00
   now = 10:00:35
   stale_duration = 35s > max_staleness (30s)
5. Emit STALE_PRICE event (status=BLOCKED)
6. Log: "Price stale for BTCUSDC (35s), execution paused"
7. Optional: Fallback to REST API for fresh price
   GET /api/v3/ticker/price?symbol=BTCUSDC
   If fresh price confirms trigger → execute
   Else → wait for WS reconnect
```

---

## Risk Guardrails Implementation

### 1. Slippage Limit

**Rule**: Reject execution if estimated slippage exceeds tenant's `max_slippage_pct`.

**Calculation**:
```rust
fn estimate_slippage(
    current_price: Decimal,
    stop_price: Decimal,
    side: &str,
) -> Decimal {
    let slippage = if side == "BUY" {
        // Closing a short: buying at current_price vs stop_price
        (current_price - stop_price) / stop_price
    } else {
        // Closing a long: selling at current_price vs stop_price
        (stop_price - current_price) / stop_price
    };

    slippage * Decimal::from(100)  // Convert to percentage
}

fn check_slippage_limit(
    slippage_pct: Decimal,
    tenant_id: i64,
    db: &PgPool,
) -> Result<(), GuardrailError> {
    let config = get_tenant_config(tenant_id, db).await?;

    if slippage_pct > config.max_slippage_pct {
        return Err(GuardrailError::SlippageBreach {
            actual: slippage_pct,
            limit: config.max_slippage_pct,
        });
    }

    Ok(())
}
```

**Event on Breach**:
```sql
INSERT INTO stop_events (
    event_type = 'SLIPPAGE_BREACH',
    execution_token = token,
    payload_json = {
        "estimated_slippage_pct": 7.5,
        "limit_pct": 5.0,
        "reason": "Estimated slippage exceeds tenant limit"
    }
)
```

### 2. Circuit Breaker

**Pattern**: Fail-fast if multiple consecutive failures for a symbol.

**State Machine**:
```
CLOSED ─(3 failures)→ OPEN ─(wait 5min)→ HALF_OPEN ─(success)→ CLOSED
                        │                              │
                        │                         (failure)
                        └──────────────────────────────┘
```

**Rust Implementation**:
```rust
async fn check_circuit_breaker(
    symbol: &str,
    redis: &mut RedisConnection,
) -> Result<(), GuardrailError> {
    let state_key = format!("circuit:state:{}", symbol);
    let state: Option<String> = redis.get(&state_key).await?;

    match state.as_deref() {
        Some("OPEN") => {
            // Check if retry window has passed
            let retry_key = format!("circuit:retry:{}", symbol);
            let retry_time: Option<i64> = redis.get(&retry_key).await?;

            if let Some(retry_ts) = retry_time {
                let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() as i64;
                if now < retry_ts {
                    return Err(GuardrailError::CircuitOpen {
                        symbol: symbol.to_string(),
                        will_retry_at: retry_ts,
                    });
                } else {
                    // Transition to HALF_OPEN
                    redis.set(&state_key, "HALF_OPEN").await?;
                }
            }
        }
        Some("HALF_OPEN") => {
            // Allow one test execution
            warn!("Circuit breaker in HALF_OPEN state for {}", symbol);
        }
        _ => {
            // CLOSED or not set: normal operation
        }
    }

    Ok(())
}

async fn record_execution_result(
    symbol: &str,
    success: bool,
    redis: &mut RedisConnection,
) {
    let failure_key = format!("circuit:failures:{}", symbol);
    let state_key = format!("circuit:state:{}", symbol);

    if success {
        // Reset failure count, close circuit
        redis.del(&failure_key).await.ok();
        redis.set(&state_key, "CLOSED").await.ok();
    } else {
        // Increment failure count
        let failures: i64 = redis.incr(&failure_key, 1).await.unwrap_or(1);

        if failures >= 3 {
            // Trip circuit breaker
            redis.set(&state_key, "OPEN").await.ok();

            let retry_ts = SystemTime::now().duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs() as i64 + 300;  // 5 minutes

            let retry_key = format!("circuit:retry:{}", symbol);
            redis.set(&retry_key, retry_ts).await.ok();

            error!("Circuit breaker OPENED for {} after {} failures", symbol, failures);

            // Emit event
            emit_circuit_breaker_event(symbol, failures).await;
        }
    }
}
```

### 3. Kill Switch (Per-Tenant)

**Redis Key**: `kill_switch:{tenant_id}` → `0` (active) or `1` (paused)

**Check Before Every Execution**:
```rust
async fn check_kill_switch(
    tenant_id: i64,
    redis: &mut RedisConnection,
) -> Result<(), GuardrailError> {
    let key = format!("kill_switch:{}", tenant_id);
    let paused: Option<i32> = redis.get(&key).await?;

    if paused == Some(1) {
        return Err(GuardrailError::TradingPaused { tenant_id });
    }

    Ok(())
}
```

**Admin Endpoint** (Django):
```python
@api_view(['POST'])
@permission_classes([IsAdminUser])
def activate_kill_switch(request):
    """
    Emergency stop: pause all trading for a tenant.

    POST /api/admin/kill-switch/
    {
        "tenant_id": 1,
        "reason": "Suspected account compromise"
    }
    """
    tenant_id = request.data.get('tenant_id')
    reason = request.data.get('reason')

    # Update DB
    TenantConfig.objects.filter(tenant_id=tenant_id).update(
        trading_enabled=False,
        trading_paused_reason=reason,
        trading_paused_at=timezone.now(),
    )

    # Set Redis flag (immediate effect)
    redis_client.set(f"kill_switch:{tenant_id}", 1)

    # Emit event
    StopEvent.objects.create(
        event_type='KILL_SWITCH',
        tenant_id=tenant_id,
        payload_json={'reason': reason, 'activated_by': request.user.id},
    )

    return Response({'status': 'kill_switch_activated', 'tenant_id': tenant_id})
```

---

## Stale Price Handling

### Policy: Pause Execution if Price Stale

**Definition**: Price is "stale" if not updated within `MAX_STALENESS` seconds (default: 30s).

**Redis Tracking**:
```python
# On every WS price update
redis.setex(f"price:{symbol}", 30, current_price)  # 30s TTL
redis.set(f"price_staleness:{symbol}", int(time.time()))
```

**Rust Check**:
```rust
async fn check_price_freshness(
    symbol: &str,
    redis: &mut RedisConnection,
    max_staleness_secs: u64,
) -> Result<(), GuardrailError> {
    let staleness_key = format!("price_staleness:{}", symbol);
    let last_update: Option<i64> = redis.get(&staleness_key).await?;

    if let Some(last_ts) = last_update {
        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() as i64;
        let stale_duration = now - last_ts;

        if stale_duration > max_staleness_secs as i64 {
            return Err(GuardrailError::StalePrice {
                symbol: symbol.to_string(),
                stale_duration_secs: stale_duration,
            });
        }
    } else {
        // No price data at all
        return Err(GuardrailError::NoPriceData {
            symbol: symbol.to_string(),
        });
    }

    Ok(())
}
```

**Fallback Strategy** (Optional):
```rust
async fn execute_with_fallback(
    trigger: &TriggerEvent,
    redis: &mut RedisConnection,
    binance: &BinanceClient,
) -> Result<(), ExecutionError> {
    // Step 1: Check if WS price is fresh
    match check_price_freshness(&trigger.symbol, redis, 30).await {
        Ok(_) => {
            // Fresh price: proceed normally
            place_market_order(trigger, binance).await?;
        }
        Err(GuardrailError::StalePrice { stale_duration_secs, .. }) => {
            warn!(
                "Stale WS price for {} ({}s old), falling back to REST API",
                trigger.symbol, stale_duration_secs
            );

            // Step 2: Fetch fresh price via REST API
            let fresh_price = binance.get_ticker_price(&trigger.symbol).await?;

            // Step 3: Verify trigger still valid with fresh price
            if is_trigger_still_valid(trigger, fresh_price) {
                info!("Fresh price confirms trigger, executing");
                place_market_order(trigger, binance).await?;
            } else {
                info!("Fresh price invalidates trigger, aborting");
                emit_event(StopEvent {
                    event_type: EventType::StalePriceAborted,
                    reason: "Trigger invalidated by fresh REST price",
                    ..
                }).await?;
            }
        }
        Err(e) => {
            // Other errors (no price data, Redis down)
            emit_event(StopEvent {
                event_type: EventType::StalePriceBlocked,
                error: e.to_string(),
                ..
            }).await?;
            return Err(ExecutionError::Guardrail(e));
        }
    }

    Ok(())
}
```

**Event on Stale Price**:
```sql
INSERT INTO stop_events (
    event_type = 'STALE_PRICE',
    execution_token = token,
    payload_json = {
        "stale_duration_secs": 45,
        "max_staleness_secs": 30,
        "action": "execution_paused"
    }
)
```

---

## Outbox Pattern for Event Publishing

### Table: `outbox`

**Purpose**: Reliable event publishing to RabbitMQ (transactional outbox pattern).

```sql
CREATE TABLE outbox (
    outbox_id UUID PRIMARY KEY DEFAULT gen_random_uuid(),

    -- Event reference
    event_id UUID NOT NULL REFERENCES stop_events(event_id),

    -- Routing
    routing_key VARCHAR(255) NOT NULL,  -- e.g., 'stop.executed.tenant1.BTCUSDC'
    exchange VARCHAR(100) DEFAULT 'stop_events',

    -- Payload
    payload JSONB NOT NULL,

    -- Publishing state
    published BOOLEAN DEFAULT FALSE,
    published_at TIMESTAMP WITH TIME ZONE,
    retry_count INT DEFAULT 0,
    last_error TEXT,

    -- Audit
    created_at TIMESTAMP WITH TIME ZONE DEFAULT NOW() NOT NULL
);

CREATE INDEX idx_outbox_unpublished ON outbox(published, created_at) WHERE NOT published;
```

### Outbox Worker (Background Process)

**Rust Implementation**:
```rust
async fn outbox_worker(
    db: PgPool,
    rabbitmq: RabbitMQConnection,
) -> Result<(), Box<dyn std::error::Error>> {
    loop {
        // Fetch unpublished messages
        let messages: Vec<OutboxMessage> = sqlx::query_as(
            "SELECT * FROM outbox WHERE published = FALSE ORDER BY created_at LIMIT 100"
        )
        .fetch_all(&db)
        .await?;

        for msg in messages {
            match publish_to_rabbitmq(&msg, &rabbitmq).await {
                Ok(_) => {
                    // Mark as published
                    sqlx::query(
                        "UPDATE outbox SET published = TRUE, published_at = NOW() WHERE outbox_id = $1"
                    )
                    .bind(msg.outbox_id)
                    .execute(&db)
                    .await?;

                    info!("Published event {} to RabbitMQ", msg.event_id);
                }
                Err(e) => {
                    // Increment retry count
                    sqlx::query(
                        "UPDATE outbox SET retry_count = retry_count + 1, last_error = $1 WHERE outbox_id = $2"
                    )
                    .bind(e.to_string())
                    .bind(msg.outbox_id)
                    .execute(&db)
                    .await?;

                    error!("Failed to publish event {}: {}", msg.event_id, e);
                }
            }
        }

        // Sleep before next poll
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

async fn publish_to_rabbitmq(
    msg: &OutboxMessage,
    rabbitmq: &RabbitMQConnection,
) -> Result<(), RabbitMQError> {
    rabbitmq.publish(
        &msg.exchange,
        &msg.routing_key,
        &msg.payload,
        PublishOptions {
            mandatory: true,
            persistent: true,
        },
    ).await
}
```

**Transactional Guarantee**:
```rust
async fn emit_and_publish_event(
    event: StopEvent,
    db: &PgPool,
) -> Result<Uuid, Box<dyn std::error::Error>> {
    let mut tx = db.begin().await?;

    // Step 1: Insert event (within transaction)
    let event_id = sqlx::query_scalar(
        "INSERT INTO stop_events (...) VALUES (...) RETURNING event_id"
    )
    .fetch_one(&mut *tx)
    .await?;

    // Step 2: Insert outbox entry (within same transaction)
    sqlx::query(
        "INSERT INTO outbox (event_id, routing_key, payload) VALUES ($1, $2, $3)"
    )
    .bind(event_id)
    .bind(format!("stop.{}.{}.{}", event.event_type, event.tenant_id, event.symbol))
    .bind(serde_json::to_value(&event)?)
    .execute(&mut *tx)
    .await?;

    // Step 3: Commit transaction (atomic)
    tx.commit().await?;

    Ok(event_id)  // Outbox worker will publish asynchronously
}
```

---

## CronJob Backstop Integration

### Updated `monitor_stops.py` Command

**File**: `apps/backend/monolith/api/management/commands/monitor_stops.py`

**Changes**:
1. Remove `--dry-run` flag from production deployment
2. Use same idempotency token mechanism
3. Check `stop_events` table before executing
4. Emit events to same `stop_events` table

**Python Implementation**:
```python
# monitor_stops.py (revised)

import hashlib
import time
from django.core.management.base import BaseCommand
from django.db import transaction
from api.models import Operation, StopEvent

class Command(BaseCommand):
    help = "Monitor active operations and execute stops (BACKSTOP)"

    def add_arguments(self, parser):
        parser.add_argument(
            "--continuous",
            action="store_true",
            help="Run continuously (loop)",
        )
        parser.add_argument(
            "--interval",
            type=int,
            default=60,
            help="Check interval in seconds (default: 60)",
        )

    def handle(self, *args, **options):
        continuous = options["continuous"]
        interval = options["interval"]

        self.stdout.write("🔍 Starting stop monitor BACKSTOP...")
        self.stdout.write(f"   Interval: {interval}s")
        self.stdout.write("   Source: cron")

        try:
            while True:
                self._run_check()

                if not continuous:
                    break

                time.sleep(interval)

        except KeyboardInterrupt:
            self.stdout.write("\n👋 Monitor stopped")

    def _run_check(self):
        """Run a single check cycle."""
        from api.application.stop_monitor import PriceMonitor
        from api.services.binance_service import BinanceService

        monitor = PriceMonitor()
        binance = BinanceService()

        # Fetch active operations
        active_ops = Operation.objects.filter(status='ACTIVE')

        for op in active_ops:
            try:
                # Get current price
                current_price = binance.get_current_price(op.symbol.name)

                # Check if stop triggered (using ABSOLUTE stop_price)
                if self._is_triggered(op, current_price):
                    # Generate idempotency token
                    token = self._generate_token(op, current_price)

                    # Check if already executed
                    if self._already_executed(token):
                        self.stdout.write(
                            f"⏭️  Op#{op.id}: Already executed (token: {token[:16]}...)"
                        )
                        continue

                    # Execute stop-loss
                    self._execute_stop(op, current_price, token)

            except Exception as e:
                self.stderr.write(f"❌ Error checking op#{op.id}: {e}")

    def _is_triggered(self, op: Operation, current_price: Decimal) -> bool:
        """Check if stop is triggered using ABSOLUTE stop_price."""
        if not op.stop_price:
            return False  # No stop configured

        if op.side == "BUY":
            # Long position: stop if current price <= stop_price
            return current_price <= op.stop_price
        else:
            # Short position: stop if current price >= stop_price
            return current_price >= op.stop_price

    def _generate_token(self, op: Operation, current_price: Decimal) -> str:
        """Generate idempotency token."""
        timestamp_ms = int(time.time() * 1000)
        return f"{op.id}:{op.stop_price}:{timestamp_ms}"

    def _already_executed(self, token: str) -> bool:
        """Check if token already exists in stop_events."""
        return StopEvent.objects.filter(execution_token=token).exists()

    @transaction.atomic
    def _execute_stop(self, op: Operation, current_price: Decimal, token: str):
        """Execute stop-loss with idempotency."""
        from api.models import StopEvent, Order
        from api.services.binance_service import BinanceService

        # Emit STOP_TRIGGERED event (idempotency check via unique constraint)
        try:
            event = StopEvent.objects.create(
                event_type='STOP_TRIGGERED',
                execution_token=token,  # Unique constraint
                operation_id=op.id,
                tenant_id=op.client_id,
                symbol=op.symbol.name,
                trigger_price=current_price,
                stop_price=op.stop_price,
                quantity=op.total_entry_quantity,
                side='SELL' if op.side == 'BUY' else 'BUY',
                source='cron',  # Attribution
            )
        except IntegrityError:
            # Token already exists (executed by WS)
            self.stdout.write(f"⏭️  Token collision: {token[:16]}... (already executed)")
            return

        # Place market order
        binance = BinanceService()
        try:
            result = binance.client.create_order(
                symbol=op.symbol.name,
                side='SELL' if op.side == 'BUY' else 'BUY',
                type='MARKET',
                quantity=str(op.total_entry_quantity),
            )

            # Emit EXECUTED event
            StopEvent.objects.create(
                event_type='EXECUTED',
                execution_token=token,
                operation_id=op.id,
                tenant_id=op.client_id,
                symbol=op.symbol.name,
                exchange_order_id=result['orderId'],
                fill_price=Decimal(result['fills'][0]['price']),
                source='cron',
            )

            # Update operation
            op.status = 'CLOSED'
            op.stop_execution_token = token
            op.save()

            # Create exit order
            Order.objects.create(
                client=op.client,
                symbol=op.symbol,
                side='SELL' if op.side == 'BUY' else 'BUY',
                order_type='MARKET',
                quantity=op.total_entry_quantity,
                status='FILLED',
                binance_order_id=result['orderId'],
            )

            self.stdout.write(
                self.style.SUCCESS(f"✅ Executed: Op#{op.id} (CronJob)")
            )

        except Exception as e:
            # Emit FAILED event
            StopEvent.objects.create(
                event_type='FAILED',
                execution_token=token,
                operation_id=op.id,
                error_message=str(e),
                source='cron',
            )

            self.stderr.write(
                self.style.ERROR(f"❌ Failed: Op#{op.id}: {e}")
            )
```

---

## Rust Service Implementation

### Crate Structure

```
apps/backend/stop-executor/
├── Cargo.toml
├── src/
│   ├── main.rs
│   ├── config.rs
│   ├── models/
│   │   ├── mod.rs
│   │   ├── operation.rs
│   │   ├── stop_event.rs
│   │   └── tenant_config.rs
│   ├── binance/
│   │   ├── mod.rs
│   │   ├── websocket.rs
│   │   └── rest.rs
│   ├── database/
│   │   ├── mod.rs
│   │   ├── queries.rs
│   │   └── pool.rs
│   ├── redis/
│   │   ├── mod.rs
│   │   └── cache.rs
│   ├── rabbitmq/
│   │   ├── mod.rs
│   │   ├── publisher.rs
│   │   └── consumer.rs
│   ├── monitor/
│   │   ├── mod.rs
│   │   ├── trigger_detector.rs
│   │   ├── executor.rs
│   │   └── guardrails.rs
│   └── outbox/
│       ├── mod.rs
│       └── worker.rs
└── Dockerfile
```

### `main.rs`

```rust
use anyhow::Result;
use tracing::{info, error};

mod config;
mod models;
mod binance;
mod database;
mod redis;
mod rabbitmq;
mod monitor;
mod outbox;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter("rbs_stop_executor=info")
        .init();

    info!("🚀 Robson Stop Executor (Event-Sourced) starting...");

    // Load configuration
    let config = config::Settings::new()?;

    // Initialize database pool
    let db_pool = database::init_pool(&config.database_url).await?;

    // Initialize Redis connection
    let redis_client = redis::init_client(&config.redis_url)?;

    // Initialize RabbitMQ connection
    let rabbitmq = rabbitmq::init_connection(&config.rabbitmq_url).await?;

    // Start Outbox worker (background task)
    let outbox_db = db_pool.clone();
    let outbox_rmq = rabbitmq.clone();
    tokio::spawn(async move {
        if let Err(e) = outbox::worker::run(outbox_db, outbox_rmq).await {
            error!("Outbox worker error: {}", e);
        }
    });

    // Initialize Binance WebSocket client
    let ws_url = format!("{}@ticker", config.binance_ws_url);
    let ws_client = binance::websocket::BinanceWsClient::new(&ws_url);

    // Start monitoring loop
    let monitor = monitor::StopMonitor::new(
        db_pool,
        redis_client,
        rabbitmq,
        ws_client,
    );

    match monitor.run().await {
        Ok(_) => info!("Monitor stopped gracefully"),
        Err(e) => error!("Monitor error: {}", e),
    }

    Ok(())
}
```

### `monitor/trigger_detector.rs`

```rust
use rust_decimal::Decimal;
use sqlx::PgPool;
use tracing::{info, warn};

use crate::models::operation::Operation;

pub struct TriggerDetector {
    db: PgPool,
}

impl TriggerDetector {
    pub fn new(db: PgPool) -> Self {
        Self { db }
    }

    pub async fn check_operations(
        &self,
        symbol: &str,
        current_price: Decimal,
    ) -> Result<Vec<Operation>, sqlx::Error> {
        // Query active operations with ABSOLUTE stop_price
        let operations = sqlx::query_as::<_, Operation>(
            r#"
            SELECT o.*
            FROM operation o
            JOIN symbol s ON o.symbol_id = s.id
            WHERE o.status = 'ACTIVE'
            AND o.stop_price IS NOT NULL
            AND s.name = $1
            "#
        )
        .bind(symbol)
        .fetch_all(&self.db)
        .await?;

        let mut triggered = Vec::new();

        for op in operations {
            if self.is_triggered(&op, current_price) {
                info!(
                    "🚨 Stop triggered: Op#{} at {} (stop: {})",
                    op.id, current_price, op.stop_price.unwrap()
                );
                triggered.push(op);
            }
        }

        Ok(triggered)
    }

    fn is_triggered(&self, op: &Operation, current_price: Decimal) -> bool {
        let stop_price = match op.stop_price {
            Some(price) => price,
            None => return false,
        };

        // ⭐ CRITICAL: Use ABSOLUTE stop_price, never recalculate
        if op.side == "BUY" {
            // Long position: trigger if current <= stop
            current_price <= stop_price
        } else {
            // Short position: trigger if current >= stop
            current_price >= stop_price
        }
    }
}
```

### `monitor/guardrails.rs`

```rust
use rust_decimal::Decimal;
use redis::aio::ConnectionManager;
use sqlx::PgPool;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum GuardrailError {
    #[error("Trading paused for tenant {tenant_id}")]
    TradingPaused { tenant_id: i64 },

    #[error("Circuit breaker OPEN for {symbol} (will retry at {will_retry_at})")]
    CircuitOpen { symbol: String, will_retry_at: i64 },

    #[error("Slippage breach: {actual}% > {limit}%")]
    SlippageBreach { actual: Decimal, limit: Decimal },

    #[error("Stale price for {symbol} ({stale_duration_secs}s old)")]
    StalePrice { symbol: String, stale_duration_secs: i64 },

    #[error("No price data available for {symbol}")]
    NoPriceData { symbol: String },

    #[error("Redis error: {0}")]
    Redis(#[from] redis::RedisError),

    #[error("Database error: {0}")]
    Database(#[from] sqlx::Error),
}

pub struct Guardrails {
    db: PgPool,
    redis: ConnectionManager,
}

impl Guardrails {
    pub fn new(db: PgPool, redis: ConnectionManager) -> Self {
        Self { db, redis }
    }

    pub async fn check_all(
        &mut self,
        tenant_id: i64,
        symbol: &str,
        current_price: Decimal,
        stop_price: Decimal,
        side: &str,
    ) -> Result<(), GuardrailError> {
        // 1. Kill switch
        self.check_kill_switch(tenant_id).await?;

        // 2. Circuit breaker
        self.check_circuit_breaker(symbol).await?;

        // 3. Price staleness
        self.check_price_freshness(symbol, 30).await?;

        // 4. Slippage limit
        let slippage_pct = self.estimate_slippage(current_price, stop_price, side);
        self.check_slippage_limit(slippage_pct, tenant_id).await?;

        Ok(())
    }

    async fn check_kill_switch(&mut self, tenant_id: i64) -> Result<(), GuardrailError> {
        let key = format!("kill_switch:{}", tenant_id);
        let paused: Option<i32> = redis::cmd("GET")
            .arg(&key)
            .query_async(&mut self.redis)
            .await?;

        if paused == Some(1) {
            return Err(GuardrailError::TradingPaused { tenant_id });
        }

        Ok(())
    }

    async fn check_circuit_breaker(&mut self, symbol: &str) -> Result<(), GuardrailError> {
        let state_key = format!("circuit:state:{}", symbol);
        let state: Option<String> = redis::cmd("GET")
            .arg(&state_key)
            .query_async(&mut self.redis)
            .await?;

        if state.as_deref() == Some("OPEN") {
            let retry_key = format!("circuit:retry:{}", symbol);
            let retry_ts: Option<i64> = redis::cmd("GET")
                .arg(&retry_key)
                .query_async(&mut self.redis)
                .await?;

            if let Some(ts) = retry_ts {
                return Err(GuardrailError::CircuitOpen {
                    symbol: symbol.to_string(),
                    will_retry_at: ts,
                });
            }
        }

        Ok(())
    }

    async fn check_price_freshness(
        &mut self,
        symbol: &str,
        max_staleness_secs: u64,
    ) -> Result<(), GuardrailError> {
        let staleness_key = format!("price_staleness:{}", symbol);
        let last_update: Option<i64> = redis::cmd("GET")
            .arg(&staleness_key)
            .query_async(&mut self.redis)
            .await?;

        if let Some(last_ts) = last_update {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs() as i64;

            let stale_duration = now - last_ts;

            if stale_duration > max_staleness_secs as i64 {
                return Err(GuardrailError::StalePrice {
                    symbol: symbol.to_string(),
                    stale_duration_secs: stale_duration,
                });
            }
        } else {
            return Err(GuardrailError::NoPriceData {
                symbol: symbol.to_string(),
            });
        }

        Ok(())
    }

    fn estimate_slippage(
        &self,
        current_price: Decimal,
        stop_price: Decimal,
        side: &str,
    ) -> Decimal {
        let slippage = if side == "SELL" {
            // Closing long: selling at current vs stop
            (stop_price - current_price) / stop_price
        } else {
            // Closing short: buying at current vs stop
            (current_price - stop_price) / stop_price
        };

        slippage * Decimal::from(100)
    }

    async fn check_slippage_limit(
        &mut self,
        slippage_pct: Decimal,
        tenant_id: i64,
    ) -> Result<(), GuardrailError> {
        let config: TenantConfig = sqlx::query_as(
            "SELECT * FROM tenant_config WHERE tenant_id = $1"
        )
        .bind(tenant_id)
        .fetch_one(&self.db)
        .await?;

        if slippage_pct > config.max_slippage_pct {
            return Err(GuardrailError::SlippageBreach {
                actual: slippage_pct,
                limit: config.max_slippage_pct,
            });
        }

        Ok(())
    }
}
```

---

## Database Migrations

### Migration 1: Add Event Sourcing Tables

**File**: `apps/backend/monolith/api/migrations/0042_event_sourcing_stop_monitor.py`

```python
from django.db import migrations, models
import django.db.models.deletion

class Migration(migrations.Migration):
    dependencies = [
        ('api', '0041_previous_migration'),
    ]

    operations = [
        # stop_events table
        migrations.CreateModel(
            name='StopEvent',
            fields=[
                ('event_id', models.UUIDField(primary_key=True, default=uuid.uuid4)),
                ('event_seq', models.BigAutoField(unique=True)),
                ('occurred_at', models.DateTimeField(auto_now_add=True)),
                ('operation', models.ForeignKey(on_delete=models.CASCADE, to='api.operation')),
                ('tenant', models.ForeignKey(on_delete=models.CASCADE, to='api.client')),
                ('symbol', models.CharField(max_length=20)),
                ('event_type', models.CharField(max_length=50, choices=[
                    ('STOP_TRIGGERED', 'Stop Triggered'),
                    ('EXECUTION_SUBMITTED', 'Execution Submitted'),
                    ('EXECUTED', 'Executed'),
                    ('FAILED', 'Failed'),
                    ('BLOCKED', 'Blocked'),
                    ('STALE_PRICE', 'Stale Price'),
                    ('KILL_SWITCH', 'Kill Switch'),
                    ('SLIPPAGE_BREACH', 'Slippage Breach'),
                    ('CIRCUIT_BREAKER', 'Circuit Breaker'),
                ])),
                ('trigger_price', models.DecimalField(max_digits=20, decimal_places=8, null=True)),
                ('stop_price', models.DecimalField(max_digits=20, decimal_places=8, null=True)),
                ('quantity', models.DecimalField(max_digits=20, decimal_places=8, null=True)),
                ('side', models.CharField(max_length=10, null=True)),
                ('execution_token', models.CharField(max_length=64, unique=True, null=True)),
                ('payload_json', models.JSONField(default=dict)),
                ('request_payload_hash', models.CharField(max_length=64, null=True)),
                ('exchange_order_id', models.CharField(max_length=100, null=True)),
                ('fill_price', models.DecimalField(max_digits=20, decimal_places=8, null=True)),
                ('slippage_pct', models.DecimalField(max_digits=10, decimal_places=4, null=True)),
                ('source', models.CharField(max_length=20, choices=[
                    ('ws', 'WebSocket'),
                    ('cron', 'CronJob'),
                    ('manual', 'Manual'),
                ])),
                ('error_message', models.TextField(null=True, blank=True)),
                ('retry_count', models.IntegerField(default=0)),
            ],
            options={
                'db_table': 'stop_events',
                'ordering': ['event_seq'],
            },
        ),

        # Indexes
        migrations.AddIndex(
            model_name='stopevent',
            index=models.Index(fields=['operation', 'event_seq'], name='idx_stop_events_operation'),
        ),
        migrations.AddIndex(
            model_name='stopevent',
            index=models.Index(fields=['tenant', 'occurred_at'], name='idx_stop_events_tenant'),
        ),
        migrations.AddIndex(
            model_name='stopevent',
            index=models.Index(fields=['event_type', 'occurred_at'], name='idx_stop_events_type'),
        ),

        # stop_executions table
        migrations.CreateModel(
            name='StopExecution',
            fields=[
                ('execution_id', models.UUIDField(primary_key=True, default=uuid.uuid4)),
                ('operation', models.ForeignKey(on_delete=models.CASCADE, to='api.operation')),
                ('tenant', models.ForeignKey(on_delete=models.CASCADE, to='api.client')),
                ('execution_token', models.CharField(max_length=64, unique=True)),
                ('status', models.CharField(max_length=50, default='PENDING', choices=[
                    ('PENDING', 'Pending'),
                    ('SUBMITTED', 'Submitted'),
                    ('EXECUTED', 'Executed'),
                    ('FAILED', 'Failed'),
                    ('BLOCKED', 'Blocked'),
                ])),
                ('stop_price', models.DecimalField(max_digits=20, decimal_places=8)),
                ('trigger_price', models.DecimalField(max_digits=20, decimal_places=8, null=True)),
                ('quantity', models.DecimalField(max_digits=20, decimal_places=8)),
                ('side', models.CharField(max_length=10)),
                ('triggered_at', models.DateTimeField(null=True)),
                ('submitted_at', models.DateTimeField(null=True)),
                ('executed_at', models.DateTimeField(null=True)),
                ('failed_at', models.DateTimeField(null=True)),
                ('exchange_order_id', models.CharField(max_length=100, null=True)),
                ('fill_price', models.DecimalField(max_digits=20, decimal_places=8, null=True)),
                ('slippage_pct', models.DecimalField(max_digits=10, decimal_places=4, null=True)),
                ('source', models.CharField(max_length=20, choices=[
                    ('ws', 'WebSocket'),
                    ('cron', 'CronJob'),
                ])),
                ('error_message', models.TextField(null=True, blank=True)),
                ('retry_count', models.IntegerField(default=0)),
                ('created_at', models.DateTimeField(auto_now_add=True)),
                ('updated_at', models.DateTimeField(auto_now=True)),
            ],
            options={
                'db_table': 'stop_executions',
            },
        ),

        # tenant_config table
        migrations.CreateModel(
            name='TenantConfig',
            fields=[
                ('tenant', models.OneToOneField(primary_key=True, on_delete=models.CASCADE, to='api.client')),
                ('trading_enabled', models.BooleanField(default=True)),
                ('trading_paused_reason', models.TextField(null=True, blank=True)),
                ('trading_paused_at', models.DateTimeField(null=True, blank=True)),
                ('max_slippage_pct', models.DecimalField(max_digits=10, decimal_places=4, default=Decimal('5.0'))),
                ('slippage_pause_threshold_pct', models.DecimalField(max_digits=10, decimal_places=4, default=Decimal('10.0'))),
                ('max_executions_per_minute', models.IntegerField(default=10)),
                ('max_executions_per_hour', models.IntegerField(default=100)),
                ('created_at', models.DateTimeField(auto_now_add=True)),
                ('updated_at', models.DateTimeField(auto_now=True)),
            ],
            options={
                'db_table': 'tenant_config',
            },
        ),

        # outbox table
        migrations.CreateModel(
            name='Outbox',
            fields=[
                ('outbox_id', models.UUIDField(primary_key=True, default=uuid.uuid4)),
                ('event', models.ForeignKey(on_delete=models.CASCADE, to='api.stopevent')),
                ('routing_key', models.CharField(max_length=255)),
                ('exchange', models.CharField(max_length=100, default='stop_events')),
                ('payload', models.JSONField()),
                ('published', models.BooleanField(default=False)),
                ('published_at', models.DateTimeField(null=True, blank=True)),
                ('retry_count', models.IntegerField(default=0)),
                ('last_error', models.TextField(null=True, blank=True)),
                ('created_at', models.DateTimeField(auto_now_add=True)),
            ],
            options={
                'db_table': 'outbox',
            },
        ),
        migrations.AddIndex(
            model_name='outbox',
            index=models.Index(fields=['published', 'created_at'], name='idx_outbox_unpublished', condition=models.Q(published=False)),
        ),
    ]
```

### Migration 2: Add `stop_price` to Operation

**File**: `apps/backend/monolith/api/migrations/0043_operation_stop_price.py`

```python
from django.db import migrations, models
from decimal import Decimal

class Migration(migrations.Migration):
    dependencies = [
        ('api', '0042_event_sourcing_stop_monitor'),
    ]

    operations = [
        # Add stop_price field
        migrations.AddField(
            model_name='operation',
            name='stop_price',
            field=models.DecimalField(
                max_digits=20,
                decimal_places=8,
                null=True,
                blank=True,
                help_text='Absolute technical stop price (not percentage)'
            ),
        ),

        # Add target_price field
        migrations.AddField(
            model_name='operation',
            name='target_price',
            field=models.DecimalField(
                max_digits=20,
                decimal_places=8,
                null=True,
                blank=True,
                help_text='Absolute target price (not percentage)'
            ),
        ),

        # Add execution tracking fields
        migrations.AddField(
            model_name='operation',
            name='stop_execution_token',
            field=models.CharField(max_length=64, null=True, blank=True),
        ),
        migrations.AddField(
            model_name='operation',
            name='last_stop_check_at',
            field=models.DateTimeField(null=True, blank=True),
        ),
        migrations.AddField(
            model_name='operation',
            name='stop_check_count',
            field=models.IntegerField(default=0),
        ),
    ]


def backfill_stop_price(apps, schema_editor):
    """Backfill stop_price from stop_loss_percent for existing operations."""
    Operation = apps.get_model('api', 'Operation')

    for op in Operation.objects.filter(stop_price__isnull=True, stop_loss_percent__isnull=False):
        if op.average_entry_price:
            if op.side == 'BUY':
                op.stop_price = op.average_entry_price * (
                    Decimal('1') - op.stop_loss_percent / Decimal('100')
                )
            else:
                op.stop_price = op.average_entry_price * (
                    Decimal('1') + op.stop_loss_percent / Decimal('100')
                )
            op.save(update_fields=['stop_price'])


class Migration(migrations.Migration):
    # ... (previous operations)

    operations = [
        # ... (previous operations)

        # Run data migration
        migrations.RunPython(backfill_stop_price, reverse_code=migrations.RunPython.noop),
    ]
```

---

## Summary of Revisions

### **Critical Changes from Original Plan**

| Aspect | Original Plan | REVISED Plan |
|--------|---------------|---------------|
| **Stop Calculation** | Recalculate from `stop_loss_percent` | ✅ Use ABSOLUTE `stop_price` (fixed technical level) |
| **State Management** | Mutable status field | ✅ **Event Sourcing** (append-only `stop_events`) |
| **Idempotency** | Optimistic locking (`version` field) | ✅ Global unique `execution_token` + INSERT constraint |
| **Audit Trail** | Limited (status changes overwrite) | ✅ Complete (every event preserved with source attribution) |
| **Risk Controls** | Basic | ✅ Slippage limits, circuit breakers, kill switches |
| **Stale Price** | Not addressed | ✅ Explicit policy (pause if stale, optional REST fallback) |
| **Event Publishing** | Direct RabbitMQ | ✅ **Outbox Pattern** (transactional guarantee) |
| **CronJob Role** | Reduce to 5-minute backstop | ✅ Keep 1-minute (unless justified otherwise), same idempotency |
| **WebSocket Separation** | Dual WS (Rust + Go) | ✅ Rust WS for critical path, Go for dashboard only |
| **RabbitMQ vs Redis** | Not clearly separated | ✅ RabbitMQ = events, Redis = cache/flags |

---

## Next Steps

1. **Review this ADR** for approval
2. **Create database migrations** (event sourcing tables)
3. **Implement Rust service** (WebSocket consumer + event emitter)
4. **Update CronJob** (`monitor_stops.py` with idempotency)
5. **Deploy to staging** (test event replay, idempotency, guardrails)
6. **Gradual production rollout**

**Do you approve this revised architecture?** I can proceed with implementation once confirmed.
