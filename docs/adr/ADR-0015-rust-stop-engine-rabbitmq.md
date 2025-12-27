# ADR-0015: Rust Stop Engine with RabbitMQ for Critical Stop-Loss Execution

**Status**: APPROVED
**Date**: 2024-12-26
**Deciders**: Product Owner, System Architect
**Related**: ADR-0007 (Risk Assistant), ADR-0011 (GitOps), ADR-0002 (Hexagonal Architecture)

---

## Context

### Current State (Before This ADR)

**Stop-Loss Monitoring** runs as Kubernetes CronJob with 60-second polling interval:

```python
# apps/backend/monolith/api/management/commands/monitor_stops.py
# Current implementation (Python/Django)
- Polls PostgreSQL every 60 seconds
- Detects triggers by comparing current price vs. stop_price
- Executes market orders DIRECTLY on Binance REST API
- Currently in DRY-RUN mode (--dry-run flag)
```

**Problems**:
1. **Latency**: Up to 59 seconds delay between trigger and execution
2. **No durability**: Direct execution without message queue
3. **Single point of failure**: If CronJob crashes during execution, trigger is lost
4. **No event-driven architecture**: Polling-based, not reactive
5. **No separation of concerns**: Trigger detection + execution in same process

**Event Sourcing Infrastructure** (already implemented):
- ✅ `StopEvent` model (append-only event store)
- ✅ `StopExecution` model (materialized view)
- ✅ Idempotency via `execution_token`
- ✅ Absolute `stop_price` fields on Operation model
- ❌ **NOT CONNECTED** to durable message transport

---

## Decision

### ⚠️ HARD CONSTRAINTS (Non-Negotiable)

These constraints define the entire system architecture and MUST NOT be violated:

1. **WebSocket is NEVER part of critical stop-loss execution**
   - WebSocket exists ONLY for UI updates
   - WebSocket failures do NOT affect execution
   - NO WebSocket code in Rust Stop Engine

2. **RabbitMQ + Outbox + PostgreSQL form the durable backbone**
   - PostgreSQL is source of truth (event sourcing)
   - Outbox Pattern ensures transactional consistency
   - RabbitMQ is ONLY transport for critical commands/events

3. **Rust Stop Engine is a RabbitMQ consumer, NOT a WebSocket-based executor**
   - Consumes from `stop_commands.critical` queue
   - Publishes to `stop_events` exchange
   - NO WebSocket listening code

4. **UI notifications are separate, non-critical path**
   - Go WebSocket Server subscribes to Redis pub/sub
   - Redis pub/sub is ephemeral (no persistence)
   - UI notification failures are tolerated

---

### Architecture: Event-Driven Execution with RabbitMQ

**Replace**: Polling-based CronJob (60s interval)
**With**: Event-driven Rust Stop Engine consuming from RabbitMQ

**Critical Path** (DURABLE, RELIABLE):

```
┌─────────────────────────────────────────────────────────────────┐
│                        CRITICAL PATH                             │
│                                                                  │
│  1. Python Stop Monitor (CronJob backstop, 5-min interval)      │
│     - Detects triggers                                          │
│     - Writes to PostgreSQL + Outbox (same transaction)          │
│                                                                  │
│          ↓                                                       │
│                                                                  │
│  2. Outbox Publisher (Django service)                           │
│     - Polls outbox table every 1 second                         │
│     - Publishes stop commands to RabbitMQ                       │
│     - Marks as published in DB                                  │
│                                                                  │
│          ↓                                                       │
│                                                                  │
│  3. RabbitMQ (stop_commands exchange)                           │
│     - Durable queues with persistence                           │
│     - Acknowledgements, retries, DLQ                            │
│     - Guaranteed delivery                                       │
│                                                                  │
│          ↓                                                       │
│                                                                  │
│  4. Rust Stop Engine (RabbitMQ Consumer) ⭐ CRITICAL COMPONENT  │
│     - Consumes from stop_commands.critical queue                │
│     - Enforces guardrails:                                      │
│       • Kill switch (tenant-level emergency stop)               │
│       • Circuit breaker (per-symbol failure protection)         │
│       • Price staleness detection                               │
│       • Slippage limit validation                               │
│     - Executes market orders on Binance REST API                │
│     - Writes results to PostgreSQL (event sourcing)             │
│     - Publishes events to RabbitMQ (stop_events exchange)       │
│                                                                  │
│          ↓                                                       │
│                                                                  │
│  5. PostgreSQL (Event Store + Projections)                      │
│     - Source of truth for all executions                        │
│     - Complete audit trail (StopEvent table)                    │
│     - Materialized views (StopExecution table)                  │
│                                                                  │
└─────────────────────────────────────────────────────────────────┘
```

**UI Notification Path** (EPHEMERAL, NON-CRITICAL):

```
┌─────────────────────────────────────────────────────────────────┐
│                     NON-CRITICAL PATH                            │
│                                                                  │
│  RabbitMQ (stop_events.notify queue)                            │
│          ↓                                                       │
│  Fanout Service (Python, optional)                              │
│  - Consumes from RabbitMQ                                       │
│  - Publishes to Redis pub/sub (ephemeral)                       │
│          ↓                                                       │
│  Redis Pub/Sub (stop_notifications channel)                     │
│          ↓                                                       │
│  Go WebSocket Server                                            │
│  - Broadcasts to frontend clients                               │
│          ↓                                                       │
│  Frontend Dashboard (React)                                     │
│  - Real-time UI updates                                         │
│                                                                  │
│  ⚠️  THIS PATH CAN FAIL WITHOUT AFFECTING EXECUTION             │
│                                                                  │
└─────────────────────────────────────────────────────────────────┘
```

---

### Component Naming (Explicit and Mandatory)

**Critical Components**:
- ✅ **"Rust Stop Engine"** (RabbitMQ consumer, NOT WebSocket server)
- ✅ **"Outbox Publisher"** (Django service)
- ✅ **"RabbitMQ"** (durable message broker)
- ✅ **"PostgreSQL"** (event store + projections)

**Non-Critical Components**:
- ⚪ **"Go WebSocket Server"** (UI notifications only)
- ⚪ **"Fanout Service"** (RabbitMQ → Redis bridge)
- ⚪ **"Redis Pub/Sub"** (ephemeral, UI-only)

**Forbidden Names** (architecturally incorrect):
- ❌ "Rust WebSocket Server" (misleading - implies WS is critical path)
- ❌ "WebSocket-based execution" (wrong - execution is RabbitMQ-based)
- ❌ "Critical WebSocket" (contradiction - WS is never critical)
- ❌ "Stop Executor" without "Engine" (less precise, avoids "executor" confusion with thread executors)

---

## Rationale

### Why Rust Stop Engine (Not Python/Go)?

| Requirement | Rust | Python (Current) | Go |
|-------------|------|------------------|-----|
| **Latency** (ms) | ✅ < 10ms | ❌ 60,000ms (poll) | ✅ < 50ms |
| **Memory Safety** | ✅ Compile-time | ❌ Runtime errors | ⚠️ Runtime GC pauses |
| **Concurrency** | ✅ Fearless (Send/Sync) | ❌ GIL bottleneck | ✅ Goroutines |
| **Financial Precision** | ✅ rust_decimal | ✅ Decimal | ⚠️ float64 pitfalls |
| **Type Safety** | ✅ Strong static | ⚠️ Optional (mypy) | ✅ Strong static |
| **Ecosystem (AMQP)** | ✅ lapin (mature) | ✅ pika | ✅ amqp091-go |
| **Critical Path Fit** | ✅ Best | ❌ Current (slow) | ✅ Good alternative |

**Decision**: Rust for critical path. Python stays as backstop (proven, simple).

---

### Why RabbitMQ (Not Redis/Kafka)?

| Feature | RabbitMQ | Redis Pub/Sub | Kafka |
|---------|----------|---------------|-------|
| **Durability** | ✅ Persisted queues | ❌ Ephemeral | ✅ Persisted topics |
| **Acknowledgements** | ✅ Per-message ACK | ❌ Fire-and-forget | ✅ Offset-based |
| **Dead Letter Queue** | ✅ Built-in DLQ | ❌ None | ⚠️ Manual setup |
| **Retry Logic** | ✅ Built-in NACK requeue | ❌ Manual | ⚠️ Manual |
| **Complexity** | ⚪ Medium | ✅ Low | ❌ High (Zookeeper) |
| **Operational Overhead** | ⚪ StatefulSet | ✅ Simple | ❌ Cluster mgmt |

**Decision**: RabbitMQ. Redis pub/sub is used ONLY for UI fanout (non-critical).

**Why NOT Kafka**:
- Over-engineered for stop-loss use case (designed for streaming logs, not transactional commands)
- Operational complexity too high for single-tenant deployment
- No immediate need for multi-partition scalability (hundreds of users, not millions)

---

### Why Outbox Pattern (Not Direct Publishing)?

**Problem with Direct Publishing**:
```python
# ❌ WRONG (not transactional)
stop_event = StopEvent.objects.create(...)
rabbitmq.publish(command)  # If this fails, DB has event but no message sent
```

**Outbox Pattern Solution**:
```python
# ✅ CORRECT (transactional consistency)
with transaction.atomic():
    StopEvent.objects.create(...)
    Outbox.objects.create(payload=command, ...)  # Same transaction!

# Separate process polls outbox and publishes
```

**Guarantees**:
1. **Atomicity**: Event + outbox entry committed together (or both rolled back)
2. **Durability**: If system crashes after commit, outbox will retry
3. **Idempotency**: Outbox prevents duplicate publishes via correlation_id

---

## Implementation Details

### RabbitMQ Topology

**Exchanges**:
```
1. stop_commands (topic exchange, durable)
   Purpose: Inbound commands for stop execution

2. stop_events (topic exchange, durable)
   Purpose: Outbound execution result events
```

**Queues** (Critical Path):
```
stop_commands.critical
  Routing: stop.command.#
  Durable: true
  Consumer: Rust Stop Engine
  Arguments:
    x-dead-letter-exchange: stop_commands.dlx
    x-message-ttl: 300000 (5 minutes)

stop_commands.dlq
  Purpose: Failed commands after max retries
  Requires manual intervention

stop_commands.backstop
  Routing: stop.command.#
  Consumer: Python CronJob (fallback)
  Purpose: Reconciliation if Rust fails
```

**Queues** (Events Fan-out):
```
stop_events.audit
  Routing: stop.event.#
  Purpose: Long-term audit logging
  TTL: 90 days

stop_events.notify
  Routing: stop.event.executed.#, stop.event.failed.#
  Consumer: Fanout Service → Redis pub/sub → Go WS
  Purpose: UI notifications

stop_events.metrics
  Routing: stop.event.#
  Consumer: Prometheus exporter
  Purpose: Real-time metrics aggregation
```

**Routing Keys**:
```
Commands:
  stop.command.{operation_id}.{symbol}
  Example: stop.command.123.BTCUSDC

Events:
  stop.event.triggered.{operation_id}.{symbol}
  stop.event.executed.{operation_id}.{symbol}
  stop.event.failed.{operation_id}.{symbol}
  stop.event.blocked.{operation_id}.{symbol}.{reason}
```

---

### Outbox Table Schema

```sql
CREATE TABLE outbox (
    outbox_id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    aggregate_type VARCHAR(50) NOT NULL,     -- 'stop_command'
    aggregate_id BIGINT NOT NULL,            -- operation_id
    event_type VARCHAR(50) NOT NULL,         -- 'COMMAND_ISSUED'
    routing_key VARCHAR(255) NOT NULL,       -- RabbitMQ routing key
    exchange VARCHAR(100) NOT NULL,          -- 'stop_commands'
    payload JSONB NOT NULL,                  -- Full command payload
    correlation_id VARCHAR(64) NOT NULL UNIQUE,  -- Idempotency key
    published BOOLEAN DEFAULT FALSE,
    published_at TIMESTAMP WITH TIME ZONE,
    retry_count INT DEFAULT 0,
    last_error TEXT,
    created_at TIMESTAMP WITH TIME ZONE DEFAULT NOW() NOT NULL
);

CREATE INDEX idx_outbox_unpublished ON outbox(published, created_at)
WHERE NOT published;

CREATE INDEX idx_outbox_correlation ON outbox(correlation_id);
```

---

### Rust Stop Engine Architecture

**Crate Structure**:
```
apps/backend/stop-engine/
├── Cargo.toml
├── Dockerfile
├── src/
│   ├── main.rs                 # RabbitMQ consumer loop
│   ├── config.rs               # Environment config
│   ├── models/
│   │   ├── stop_command.rs     # Incoming command payload
│   │   ├── stop_event.rs       # Outgoing event payload
│   │   └── tenant_config.rs    # Risk guardrails config
│   ├── binance/
│   │   └── rest.rs             # Binance REST API (NO WebSocket)
│   ├── database/
│   │   ├── queries.rs          # PostgreSQL queries
│   │   └── pool.rs             # Connection pool
│   ├── redis/
│   │   └── cache.rs            # Kill switches, circuit breakers
│   ├── rabbitmq/
│   │   ├── consumer.rs         # AMQP consumer logic
│   │   └── publisher.rs        # Publish events back
│   ├── engine/
│   │   ├── executor.rs         # Stop-loss execution
│   │   └── guardrails.rs       # Pre-execution checks
│   └── metrics/
│       └── prometheus.rs       # Metrics exporter
```

**Key Dependencies**:
```toml
[dependencies]
tokio = { version = "1", features = ["full"] }
sqlx = { version = "0.7", features = ["postgres", "decimal"] }
rust_decimal = "1.33"
redis = { version = "0.23", features = ["tokio-comp"] }
lapin = "2.3"                    # RabbitMQ client (AMQP)
serde = { version = "1.0", features = ["derive"] }
tracing = "0.1"
prometheus = "0.13"
reqwest = { version = "0.11", features = ["json"] }  # Binance REST
```

**NO tokio-tungstenite** (no WebSocket dependency).

---

### Guardrails (Pre-Execution Checks)

Executed IN ORDER before placing Binance order:

1. **Idempotency Check**
   ```sql
   SELECT EXISTS(
     SELECT 1 FROM stop_events
     WHERE execution_token = $1
   )
   ```
   If exists → SKIP (already executed)

2. **Kill Switch** (tenant-level emergency stop)
   ```redis
   GET kill_switch:{tenant_id}
   ```
   If = 1 → BLOCK with reason: KILL_SWITCH

3. **Circuit Breaker** (per-symbol failure protection)
   ```redis
   GET circuit:state:{symbol}
   ```
   If = "OPEN" → BLOCK with reason: CIRCUIT_BREAKER

   State Machine:
   ```
   CLOSED ─(3 failures)→ OPEN ─(wait 5min)→ HALF_OPEN ─(success)→ CLOSED
                           │                              │
                           │                         (failure)
                           └──────────────────────────────┘
   ```

4. **Price Staleness Detection**
   ```redis
   GET price_staleness:{symbol}
   ```
   If timestamp > 30 seconds old → BLOCK with reason: STALE_PRICE

5. **Slippage Limit**
   ```rust
   let estimated_fill_price = get_current_market_price(symbol);
   let slippage = ((estimated_fill_price - stop_price).abs() / stop_price) * 100;

   if slippage > tenant_config.max_slippage_pct {
       return Err(BlockedReason::SlippageBreach);
   }
   ```

If ALL checks pass → Execute market order on Binance.

---

### Event Sourcing Integration

**Event Types** (StopEvent table):
```python
EVENT_TYPE_CHOICES = [
    ('STOP_TRIGGERED', 'Stop Triggered'),       # Monitor detected trigger
    ('EXECUTION_SUBMITTED', 'Submitted'),        # Rust sent order to Binance
    ('EXECUTED', 'Executed'),                    # Binance confirmed fill
    ('FAILED', 'Failed'),                        # Execution error
    ('BLOCKED', 'Blocked'),                      # Guardrail blocked execution
]
```

**Execution Flow** (Rust Stop Engine):
```rust
async fn execute_stop(cmd: StopCommand) -> Result<()> {
    // 1. Run guardrails
    if let Some(block_reason) = check_guardrails(&cmd).await? {
        emit_blocked_event(&cmd, block_reason).await?;
        return Ok(()); // ACK message (blocked is not an error)
    }

    // 2. Place market order on Binance
    let order_response = binance.place_market_order(&cmd).await?;

    // 3. Write event to PostgreSQL
    insert_stop_event(StopEvent {
        event_type: "EXECUTED",
        operation_id: cmd.operation_id,
        exchange_order_id: order_response.order_id,
        execution_token: cmd.correlation_id,
        ...
    }).await?;

    // 4. Update operation status
    update_operation_status(cmd.operation_id, "CLOSED").await?;

    // 5. Publish result event to RabbitMQ
    publish_event(StopEventPayload {
        event_type: "executed",
        operation_id: cmd.operation_id,
        ...
    }).await?;

    Ok(())
}
```

---

## Consequences

### Positive

✅ **Sub-second execution latency** (vs. 60s polling)
✅ **Durable message transport** (RabbitMQ persistence)
✅ **Transactional consistency** (Outbox Pattern)
✅ **Idempotency guarantees** (correlation_id)
✅ **Failure resilience** (DLQ, retries, circuit breaker)
✅ **Complete audit trail** (event sourcing)
✅ **Separation of concerns** (trigger detection ≠ execution)
✅ **Backstop safety net** (Python CronJob as fallback)
✅ **UI notifications decoupled** (can fail without affecting execution)

### Negative

❌ **Increased operational complexity** (RabbitMQ + Rust + Outbox)
❌ **New technology stack** (team needs Rust knowledge)
❌ **More moving parts** (Outbox Publisher, Rust Engine, Fanout Service)
❌ **RabbitMQ operational burden** (StatefulSet, monitoring, backups)

### Mitigation

- **Complexity**: Comprehensive documentation (runbooks, ADRs)
- **Rust knowledge**: Focused on single component (Stop Engine), not entire stack
- **Operational burden**: Managed RabbitMQ option (CloudAMQP) as alternative
- **Gradual rollout**: Run in dual mode (Rust + Python CronJob) for 2 weeks before backstop mode

---

## Alternatives Considered

### Alternative 1: Keep Python, Add WebSocket

**Approach**: Python async with Binance WebSocket for real-time prices.

**Pros**:
- Familiar tech stack (Python/Django)
- Reuses existing event sourcing code

**Cons**:
- GIL limits concurrency for hundreds of operations
- Python WebSocket libraries less mature than Rust
- Still need message queue for durability

**Rejected**: Performance ceiling too low for scale.

---

### Alternative 2: Use Binance Native Stop-Loss Orders

**Approach**: Place STOP_LOSS_LIMIT orders on Binance exchange.

**Pros**:
- Zero infrastructure (Binance handles execution)
- No latency (exchange-side trigger)

**Cons**:
- **No control**: Can't enforce guardrails (kill switch, circuit breaker)
- **No audit trail**: Execution happens in Binance's black box
- **No risk management**: Can't validate drawdown limits before execution
- **STOP_LOSS_LIMIT may not fill**: If price gaps past limit, order sits unfilled

**Rejected**: Violates core principle of Robson as Risk Management Assistant (user wants control).

---

### Alternative 3: Go-Based Stop Engine

**Approach**: Replace Rust with Go for RabbitMQ consumer.

**Pros**:
- Simpler language (easier to hire for)
- Good concurrency (goroutines)
- Mature AMQP library (amqp091-go)

**Cons**:
- GC pauses (not ideal for latency-critical execution)
- float64 precision issues (Go's decimal library less mature than rust_decimal)
- Less compile-time safety than Rust

**Rejected**: Rust's guarantees (memory safety, precision) justify learning curve for critical component.

---

### Alternative 4: Redis Streams Instead of RabbitMQ

**Approach**: Use Redis Streams with consumer groups.

**Pros**:
- Simpler than RabbitMQ (already have Redis)
- Good performance

**Cons**:
- No native DLQ (manual implementation)
- No built-in retry with exponential backoff
- Less mature than RabbitMQ for message durability

**Rejected**: RabbitMQ's built-in features (DLQ, retries, ACKs) reduce custom code.

---

## Validation & Testing Strategy

### Unit Tests (Rust)

```rust
#[tokio::test]
async fn test_idempotency() {
    let engine = StopEngine::new(mock_db(), mock_binance());
    let cmd = StopCommand { correlation_id: "test_123", ... };

    // Execute twice
    let result1 = engine.execute(cmd.clone()).await;
    let result2 = engine.execute(cmd.clone()).await;

    assert!(result1.is_ok());
    assert_matches!(result2, Ok(ExecutionResult::AlreadyProcessed));
}

#[tokio::test]
async fn test_kill_switch_blocks_execution() {
    let redis = mock_redis_with_kill_switch(tenant_id = 1);
    let engine = StopEngine::new(mock_db(), redis, mock_binance());

    let result = engine.execute(cmd).await;

    assert_matches!(result, Ok(ExecutionResult::Blocked(BlockReason::KillSwitch)));
}
```

### Integration Tests (Python)

```python
@pytest.mark.django_db
def test_outbox_publishes_to_rabbitmq(rabbitmq_connection):
    """Verify Outbox Publisher sends commands to RabbitMQ."""
    operation = create_test_operation_with_stop()

    # Trigger detection (writes to outbox)
    monitor = PriceMonitor()
    monitor.check_all_operations()

    # Outbox publisher should publish
    time.sleep(2)  # Allow publisher to poll and publish

    # Verify message in RabbitMQ
    channel = rabbitmq_connection.channel()
    method, properties, body = channel.basic_get('stop_commands.critical')

    assert method is not None
    command = json.loads(body)
    assert command['operation_id'] == operation.id
```

### End-to-End Test (Staging)

1. Create test operation with stop-loss in staging
2. Manually adjust market price (testnet Binance or mock)
3. Verify:
   - Outbox entry created
   - RabbitMQ message published
   - Rust Stop Engine consumes message
   - Binance order placed
   - StopEvent written to DB
   - Operation status = CLOSED
   - Frontend receives WebSocket notification

**Acceptance Criteria**:
- ✅ Execution latency < 1 second (P95)
- ✅ Zero duplicate executions
- ✅ Complete audit trail in PostgreSQL

---

## Rollout Plan

### Phase 1: Staging Validation (Week 6)

1. Deploy RabbitMQ StatefulSet
2. Deploy Outbox Publisher
3. Deploy Rust Stop Engine (2 replicas)
4. Deploy Fanout Service + Go WS
5. Run end-to-end tests
6. Monitor for 1 week

**Success Criteria**: 100% of test triggers executed within 1 second.

---

### Phase 2: Production Dual Mode (Week 7)

1. Deploy Rust Stop Engine to production (2 replicas)
2. Keep Python CronJob at 1-minute interval
3. Both detect triggers → idempotency prevents duplicates
4. Monitor logs for race conditions
5. Verify Rust executes 99%+ of stops

**Success Criteria**: Rust handles >99% of executions, Python executes <1%.

---

### Phase 3: Production Backstop Mode (Week 8)

1. Reduce Python CronJob to 5-minute interval
2. Rust is primary execution path
3. CronJob is safety net for missed triggers
4. Monitor for 1 week

**Success Criteria**: CronJob triggers <1% of events (only when Rust unavailable).

---

## Monitoring & Observability

### Prometheus Metrics (Rust Stop Engine)

```
robson_stop_commands_consumed_total     (counter)
robson_stop_executions_success_total    (counter)
robson_stop_executions_failed_total     (counter)
robson_stop_executions_blocked_total    (counter, by reason)
robson_stop_execution_latency_seconds   (histogram)
robson_circuit_breaker_state            (gauge, by symbol)
robson_rabbitmq_connected               (gauge)
```

### AlertManager Rules

```yaml
- alert: StopEngineRabbitMQDisconnected
  expr: robson_rabbitmq_connected == 0
  for: 1m
  severity: critical
  annotations:
    summary: "Rust Stop Engine disconnected from RabbitMQ"

- alert: HighStopExecutionFailureRate
  expr: rate(robson_stop_executions_failed_total[5m]) / rate(robson_stop_commands_consumed_total[5m]) > 0.05
  for: 5m
  severity: warning
  annotations:
    summary: "Stop execution failure rate >5%"
```

---

## References

- [Outbox Pattern](https://microservices.io/patterns/data/transactional-outbox.html)
- [RabbitMQ Reliability Guide](https://www.rabbitmq.com/reliability.html)
- [Rust lapin AMQP Client](https://github.com/amqp-rs/lapin)
- [ADR-0007: Robson is Risk Assistant, Not Auto-Trader](./ADR-0007-robson-is-risk-assistant-not-autotrader.md)
- [ADR-0002: Hexagonal Architecture](./ADR-0002-hexagonal-architecture.md)

---

**Status**: APPROVED
**Approved By**: Product Owner, System Architect
**Implementation Start**: 2024-12-26
**Target Completion**: 2025-02-14 (8 weeks)
