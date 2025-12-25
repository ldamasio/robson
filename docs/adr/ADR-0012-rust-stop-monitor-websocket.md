# ADR-0012: Rust-Based Critical Stop-Loss Monitor with Dual WebSocket Architecture

**Status**: Draft
**Date**: 2024-12-25
**Deciders**: System Architect
**Related**: ADR-0007 (Strategy Semantics), ADR-0011 (GitOps)

---

## Context

### Current State Analysis

**Stop-Loss Monitoring** (`apps/backend/monolith/api/application/stop_monitor.py`):
- Kubernetes CronJob running every **60 seconds**
- Uses **percentage-based stops** (`operation.stop_loss_percent`)
- Executes via `place_market()` adapter ✅ (CORRECT: market orders, not exchange stops)
- Currently in **DRY-RUN mode** in production (`--dry-run` flag)

**Critical Gap Identified**:
```python
# trading.py line 244: Operation has stop_loss_percent field
stop_loss_percent = models.DecimalField(...)

# stop_monitor.py line 108: Monitor reads this field
if operation.stop_loss_percent:
    stop_loss_price = entry_price * (1 - operation.stop_loss_percent / 100)

# user_operations.py line 306-312: Operation created WITHOUT setting stop_loss_percent!
operation = Operation.objects.create(
    # ... fields ...
    # ❌ stop_loss_percent NOT SET!
)

# user_operations.py line 324: Stop price stored in ORDER, not Operation
entry_order = Order.objects.create(
    stop_loss_price=stop_price,  # ✅ Price here, but monitor doesn't use this
)
```

**Technical Stop Logic** (`api/domain/technical_stop.py`):
- ✅ Exists: Sophisticated technical analysis for stop placement
- ❌ **NOT CONNECTED** to stop monitor
- Calculates position size from technical stop (GOLDEN RULE implementation)
- Converts technical stop to percentage **at operation creation time** (if implemented)

**WebSocket Server** (`cli/cmd/server.go`):
- Go-based, broadcasts market data from Redis
- Simulates price updates (1-second ticker)
- **Not used for critical stop-loss logic** (only for dashboard updates)

**ASGI Server** (`apps/backend/monolith/backend/asgi.py`):
- Vanilla Django ASGI (no Channels)
- No WebSocket integration on Django side

---

## Decision

### Architecture: Dual-Service Real-Time Stop Monitor

Replace 60-second CronJob polling with a **dual-service architecture**:

1. **Rust Critical Service** (Deployment): Real-time stop-loss/stop-gain execution
2. **Go Notification Service** (Deployment): Dashboard updates for end users
3. **Python CronJob** (Backstop): Reconciliation & failsafe (5-minute polling)

#### Why Rust for Critical Path?

| Requirement | Rust | Python/Django | Go |
|-------------|------|---------------|-----|
| **Latency** (ms execution) | ✅ Excellent | ⚠️ GIL limits | ✅ Excellent |
| **Memory Safety** | ✅ Compile-time | ❌ Runtime | ⚠️ Runtime GC |
| **Concurrency** | ✅ Async + fearless | ⚠️ GIL + asyncio | ✅ Goroutines |
| **DB Access** | ✅ Diesel/SQLx | ✅ Django ORM | ✅ GORM |
| **Financial Precision** | ✅ rust_decimal | ✅ Decimal | ⚠️ float64 |
| **Observability** | ✅ tracing | ✅ logging | ✅ log |

**Decision: Rust** for stop-loss (critical, requires precision & speed), **Go** for notifications (good enough for broadcasts).

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
                    │  Rust Service   │ ◄──── Critical Path
                    │  rbs-stop-exec  │
                    │                 │
                    │  - Consumes WS  │
                    │  - Filters ops  │
                    │  - Executes SL  │
                    │  - Publishes    │
                    └────┬────────┬───┘
                         │        │
             ┌───────────┘        └─────────────┐
             │                                   │
    ┌────────▼────────┐                ┌────────▼────────┐
    │   PostgreSQL    │                │  Redis Pub/Sub  │
    │                 │                │                 │
    │  - Read Ops     │                │  - Publish SL   │
    │  - Write Orders │                │  - Notify       │
    │  - Locks        │                │                 │
    └─────────────────┘                └────────┬────────┘
                                                 │
                                        ┌────────▼────────┐
                                        │   Go Service    │ ◄──── Notification
                                        │  rbs-ws-notify  │
                                        │                 │
                                        │  - Subscribe    │
                                        │  - Broadcast WS │
                                        └────────┬────────┘
                                                 │
                                        ┌────────▼────────┐
                                        │  Frontend Apps  │
                                        │  (Dashboard)    │
                                        └─────────────────┘

┌─────────────────────────────────────────────────────────────────┐
│                    Backstop (Every 5 minutes)                   │
│                                                                 │
│   Python CronJob (monitor_stops.py without --dry-run)          │
│   - Detects missed executions (if Rust crashed)                │
│   - Reconciles state inconsistencies                           │
│   - Alerts on failures                                         │
└─────────────────────────────────────────────────────────────────┘
```

---

### State Machine: Operation Status Flow

**Current State** (`trading.py` line 232-237):
```python
STATUS_CHOICES = [
    ("PLANNED", "Planned"),
    ("ACTIVE", "Active"),
    ("CLOSED", "Closed"),
    ("CANCELLED", "Cancelled"),
]
```

**Problem**: No intermediate states for stop-loss execution.

**Proposed Enhancement**:
```python
STATUS_CHOICES = [
    ("PLANNED", "Planned"),                # Created, not executed
    ("ACTIVE", "Active"),                  # Entry filled, monitoring
    ("STOP_TRIGGERED", "Stop Triggered"),  # ⭐ NEW: Price hit stop level
    ("CLOSING", "Closing"),                # ⭐ NEW: Exit order placed
    ("CLOSED", "Closed"),                  # Exit filled
    ("CANCELLED", "Cancelled"),            # User cancelled
    ("FAILED", "Failed"),                  # ⭐ NEW: Execution failed
]
```

**State Transition Diagram**:
```
PLANNED ──(entry filled)──> ACTIVE
                              │
        ┌─────────────────────┼─────────────────────┐
        │                     │                     │
   (stop hit)          (target hit)         (user cancel)
        │                     │                     │
        ▼                     ▼                     ▼
STOP_TRIGGERED         STOP_TRIGGERED          CANCELLED
        │                     │
   (order placed)       (order placed)
        │                     │
        ▼                     ▼
     CLOSING              CLOSING
        │                     │
   (order filled)       (order filled)
        │                     │
        └─────────────────────┴─────────────────> CLOSED

   (any error)
        │
        ▼
     FAILED
```

---

### Idempotency & Concurrency Control

#### Database-Level Locks

**Problem**: Multiple processes (Rust + CronJob backstop) could trigger same stop.

**Solution**: Optimistic locking with version field + atomic CAS.

**Schema Change** (`trading.py` - Operation model):
```python
class Operation(BaseModel):
    # ... existing fields ...

    # ⭐ NEW: Version for optimistic locking
    version = models.IntegerField(default=1, db_index=True)

    # ⭐ NEW: Track execution attempts
    stop_execution_attempts = models.IntegerField(default=0)
    last_stop_check_at = models.DateTimeField(null=True, blank=True)

    # ⭐ NEW: Deduplication token
    stop_execution_token = models.CharField(max_length=64, null=True, blank=True, unique=True)
```

**Atomic State Transition** (pseudo-code):
```python
def trigger_stop_loss(operation_id: int, current_price: Decimal) -> Result:
    """
    Atomically transition operation to STOP_TRIGGERED.

    Uses optimistic locking to prevent race conditions.
    """
    with transaction.atomic():
        # Lock row with SELECT FOR UPDATE
        op = Operation.objects.select_for_update().get(id=operation_id)

        # Check if already processing
        if op.status != "ACTIVE":
            return Result.AlreadyProcessed

        # Generate idempotency token
        token = f"stop_{operation_id}_{int(time.time())}"

        # Atomic CAS (Compare-And-Swap)
        updated = Operation.objects.filter(
            id=operation_id,
            status="ACTIVE",
            version=op.version,  # ⭐ Optimistic lock
            stop_execution_token__isnull=True,  # ⭐ Not already claimed
        ).update(
            status="STOP_TRIGGERED",
            version=op.version + 1,  # Increment version
            stop_execution_token=token,
            last_stop_check_at=timezone.now(),
        )

        if updated == 0:
            # Race condition: another process claimed it
            return Result.RaceCondition

        # Proceed with execution
        return Result.Success(token)
```

#### Redis-Based Distributed Lock (Alternative/Supplementary)

```rust
// Rust implementation using redis crate
async fn acquire_stop_lock(
    redis: &mut RedisConnection,
    operation_id: i64,
) -> Result<Option<String>, RedisError> {
    let lock_key = format!("lock:stop:{}", operation_id);
    let lock_value = Uuid::new_v4().to_string();

    // SET NX EX (set if not exists, expire after 30 seconds)
    let acquired: bool = redis::cmd("SET")
        .arg(&lock_key)
        .arg(&lock_value)
        .arg("NX")  // Only set if not exists
        .arg("EX")  // Expiration
        .arg(30)    // 30 seconds
        .query_async(redis)
        .await?;

    if acquired {
        Ok(Some(lock_value))
    } else {
        Ok(None)  // Lock held by another process
    }
}
```

---

### Data Flow: Stop-Loss Execution

#### Sequence Diagram

```
Binance WS ─┐
            │
            ▼
      ┌─────────────────┐
      │  Rust Service   │
      │  (rbs-stop-exec)│
      └────────┬────────┘
               │
    ┌──────────▼──────────┐
    │ 1. Receive Price    │
    │    BTCUSDC: $94,800 │
    └──────────┬──────────┘
               │
    ┌──────────▼──────────────────────────┐
    │ 2. Query Active Operations          │
    │    SELECT * FROM operation          │
    │    WHERE status = 'ACTIVE'          │
    │    AND symbol = 'BTCUSDC'           │
    │    AND client_id IN (tenant_filter) │
    └──────────┬──────────────────────────┘
               │
    ┌──────────▼──────────────────────────┐
    │ 3. Calculate Trigger (Per Op)       │
    │    Op #123:                         │
    │      Entry: $95,000                 │
    │      Stop%: 2.0%                    │
    │      Stop Price: $93,100            │
    │      Current: $94,800               │
    │      Triggered? NO                  │
    │                                     │
    │    Op #456:                         │
    │      Entry: $95,500                 │
    │      Stop%: 1.5%                    │
    │      Stop Price: $94,067            │
    │      Current: $94,800               │
    │      Triggered? YES ✅              │
    └──────────┬──────────────────────────┘
               │
    ┌──────────▼──────────────────────────┐
    │ 4. Acquire Lock (Op #456)           │
    │    BEGIN TRANSACTION;               │
    │    SELECT FOR UPDATE;               │
    │    UPDATE ... WHERE version = N;    │
    │    COMMIT;                          │
    │    Result: LOCK ACQUIRED ✅         │
    └──────────┬──────────────────────────┘
               │
    ┌──────────▼──────────────────────────┐
    │ 5. Transition State                 │
    │    ACTIVE → STOP_TRIGGERED          │
    └──────────┬──────────────────────────┘
               │
    ┌──────────▼──────────────────────────┐
    │ 6. Place Market Order (Binance)     │
    │    POST /api/v3/order               │
    │    symbol=BTCUSDC                   │
    │    side=SELL (closing long)         │
    │    type=MARKET                      │
    │    quantity=0.055                   │
    └──────────┬──────────────────────────┘
               │
    ┌──────────▼──────────────────────────┐
    │ 7. Update State                     │
    │    STOP_TRIGGERED → CLOSING         │
    │    (if order accepted)              │
    └──────────┬──────────────────────────┘
               │
    ┌──────────▼──────────────────────────┐
    │ 8. Save Exit Order to DB            │
    │    INSERT INTO order (...)          │
    │    UPDATE operation SET status=...  │
    └──────────┬──────────────────────────┘
               │
    ┌──────────▼──────────────────────────┐
    │ 9. Publish Event (Redis)            │
    │    PUBLISH stop_events              │
    │    {"op_id": 456, "status": "..."}  │
    └──────────┬──────────────────────────┘
               │
               ▼
        ┌─────────────┐
        │ Go Service  │──> Frontend WebSocket
        │ (broadcast) │    "Stop-loss executed!"
        └─────────────┘
```

---

### Handling Edge Cases

#### 1. **Price Gap (Slippage)**

**Scenario**: Stop at $94,000, market jumps from $94,500 → $93,200 in one tick.

**Solution**:
- ✅ Use MARKET orders (guaranteed execution, accept slippage)
- ❌ Never use LIMIT orders at stop price (may not fill)
- Log actual fill price vs. expected stop price for analysis

```rust
if actual_fill_price < expected_stop_price {
    let slippage_pct = ((expected_stop_price - actual_fill_price) / expected_stop_price) * 100;
    warn!("Slippage detected: {}% worse than expected", slippage_pct);
    // Log to audit trail
}
```

#### 2. **Binance API Failure**

**Scenario**: Order placement returns 503 (service unavailable).

**Solution**:
- Retry with exponential backoff (3 attempts)
- If all retries fail: transition to `FAILED` status
- Alert via monitoring (Prometheus + AlertManager)
- **Backstop CronJob** will retry on next run (5-minute interval)

```rust
async fn place_order_with_retry(
    client: &BinanceClient,
    order: &OrderRequest,
) -> Result<OrderResponse, OrderError> {
    let mut attempts = 0;
    let max_attempts = 3;

    while attempts < max_attempts {
        match client.place_order(order).await {
            Ok(response) => return Ok(response),
            Err(e) if e.is_retryable() => {
                attempts += 1;
                let delay = Duration::from_millis(100 * 2_u64.pow(attempts));
                tokio::time::sleep(delay).await;
            }
            Err(e) => return Err(e),  // Non-retryable error
        }
    }

    Err(OrderError::MaxRetriesExceeded)
}
```

#### 3. **Database Transaction Rollback**

**Scenario**: Market order succeeds on Binance, but DB update fails.

**Solution**: **Two-Phase Protocol** (Exchange → DB, not DB → Exchange)

```rust
// WRONG (dangerous):
// 1. Update DB to STOP_TRIGGERED
// 2. Place order on Binance  ← If this fails, DB is inconsistent!

// ✅ CORRECT:
// 1. Transition to STOP_TRIGGERED (intent)
// 2. Place order on Binance ✓
// 3. If success: Update DB with CLOSING + order details
// 4. If DB update fails: Log orphaned order, manual reconciliation

async fn execute_stop_loss(op_id: i64) -> Result<(), ExecutionError> {
    // Step 1: Claim operation (optimistic lock)
    let token = claim_operation_for_stop(op_id).await?;

    // Step 2: Place order on exchange FIRST
    let order_response = binance.place_market_order(...).await?;

    // Step 3: Update DB with exchange response
    match save_exit_order_to_db(op_id, &order_response).await {
        Ok(_) => {
            info!("Stop-loss executed successfully for op {}", op_id);
            Ok(())
        }
        Err(e) => {
            // ⚠️ ORPHANED ORDER: Binance order exists, DB doesn't know
            error!(
                "CRITICAL: Order {} placed on Binance but DB update failed: {}",
                order_response.order_id, e
            );
            // Log to dead-letter queue for manual reconciliation
            log_orphaned_order(op_id, order_response, e).await;
            Err(ExecutionError::OrphanedOrder)
        }
    }
}
```

#### 4. **Concurrent Executions (Race Condition)**

**Scenario**: Rust service and CronJob backstop both detect trigger simultaneously.

**Solution**: Optimistic locking + idempotency token (already covered above).

**Test**:
```python
# Simulate race condition in test
import threading

def trigger_stop_loss(op_id):
    # Attempt to execute
    result = execute_stop_loss(op_id)
    return result

# Run two threads simultaneously
t1 = threading.Thread(target=trigger_stop_loss, args=(123,))
t2 = threading.Thread(target=trigger_stop_loss, args=(123,))

t1.start()
t2.start()
t1.join()
t2.join()

# Assertion: Exactly ONE order created, other thread gets AlreadyProcessed
orders = Order.objects.filter(operation__id=123, side="SELL")
assert orders.count() == 1  # ✅ Idempotent
```

---

### Observability & Monitoring

#### Metrics (Prometheus)

```rust
use prometheus::{Counter, Histogram, IntGauge};

lazy_static! {
    static ref STOP_TRIGGERS_TOTAL: Counter = register_counter!(
        "robson_stop_triggers_total",
        "Total number of stop-loss/stop-gain triggers detected"
    ).unwrap();

    static ref STOP_EXECUTIONS_SUCCESS: Counter = register_counter!(
        "robson_stop_executions_success_total",
        "Successful stop-loss executions"
    ).unwrap();

    static ref STOP_EXECUTIONS_FAILED: Counter = register_counter!(
        "robson_stop_executions_failed_total",
        "Failed stop-loss executions"
    ).unwrap();

    static ref EXECUTION_LATENCY: Histogram = register_histogram!(
        "robson_stop_execution_latency_seconds",
        "Time from trigger detection to order placement",
        vec![0.01, 0.05, 0.1, 0.5, 1.0, 5.0]
    ).unwrap();

    static ref ACTIVE_OPERATIONS_MONITORED: IntGauge = register_int_gauge!(
        "robson_active_operations_monitored",
        "Number of operations currently being monitored"
    ).unwrap();
}

// Usage in code:
STOP_TRIGGERS_TOTAL.inc();
let timer = EXECUTION_LATENCY.start_timer();
// ... execute ...
timer.observe_duration();
STOP_EXECUTIONS_SUCCESS.inc();
```

#### Structured Logging

```rust
use tracing::{info, warn, error, instrument};

#[instrument(skip(binance, db), fields(op_id, symbol))]
async fn execute_stop_loss(
    op_id: i64,
    symbol: &str,
    binance: &BinanceClient,
    db: &PgPool,
) -> Result<(), ExecutionError> {
    info!("Executing stop-loss");

    // ... execution logic ...

    match result {
        Ok(order_id) => {
            info!(order_id = %order_id, "Stop-loss executed successfully");
        }
        Err(e) => {
            error!(error = %e, "Stop-loss execution failed");
        }
    }
}
```

#### Alerting Rules (Prometheus → AlertManager)

```yaml
groups:
  - name: robson_stop_monitor
    rules:
      - alert: HighStopExecutionFailureRate
        expr: |
          rate(robson_stop_executions_failed_total[5m])
          /
          rate(robson_stop_executions_success_total[5m]) > 0.1
        for: 5m
        labels:
          severity: critical
        annotations:
          summary: "Stop-loss execution failure rate >10%"
          description: "{{ $value | humanizePercentage }} of stop executions failing"

      - alert: StopExecutionLatencyHigh
        expr: |
          histogram_quantile(0.95,
            rate(robson_stop_execution_latency_seconds_bucket[5m])
          ) > 1.0
        for: 5m
        labels:
          severity: warning
        annotations:
          summary: "P95 stop execution latency >1s"
          description: "Slow stop executions detected: {{ $value }}s"
```

---

## Implementation Plan

### Phase 1: Fix Current System (Immediate)

**Goal**: Connect technical stops to monitor, remove DRY-RUN.

#### Step 1.1: Update Operation Creation Logic

**File**: `apps/backend/monolith/api/views/user_operations.py`

**Change** (line 305-312):
```python
# BEFORE (BROKEN):
operation = Operation.objects.create(
    client=client,
    symbol=symbol,
    strategy=strategy,
    side=side,
    status='PLANNED',
    # ❌ stop_loss_percent NOT SET!
)

# AFTER (FIXED):
operation = Operation.objects.create(
    client=client,
    symbol=symbol,
    strategy=strategy,
    side=side,
    status='PLANNED',
    # ✅ Calculate percentage from technical stop
    stop_loss_percent=calc['stop_distance_percent'],  # From position sizing calc
    stop_gain_percent=calc.get('target_distance_percent'),  # If target provided
)
```

#### Step 1.2: Remove `--dry-run` from CronJob

**File**: `infra/k8s/prod/rbs-stop-monitor-cronjob.yml`

**Change** (line 30-35):
```yaml
# BEFORE:
command:
- python
- manage.py
- monitor_stops
- --dry-run  # ❌ Remove this line

# AFTER:
command:
- python
- manage.py
- monitor_stops  # ✅ Real execution
```

#### Step 1.3: Test & Validate

1. Create test operation with technical stop
2. Manually trigger price movement (testnet)
3. Verify stop-loss executes correctly
4. Check audit trail in database

**Deliverable**: Working stop-loss monitoring with current 60s polling.

---

### Phase 2: Rust Service Foundation (Weeks 1-2)

#### Step 2.1: Project Structure

```
apps/backend/
├── monolith/         # Django (existing)
└── stop-executor/    # ⭐ NEW: Rust service
    ├── Cargo.toml
    ├── src/
    │   ├── main.rs
    │   ├── binance/
    │   │   ├── mod.rs
    │   │   ├── websocket.rs
    │   │   └── rest.rs
    │   ├── database/
    │   │   ├── mod.rs
    │   │   ├── models.rs
    │   │   └── queries.rs
    │   ├── monitor/
    │   │   ├── mod.rs
    │   │   ├── trigger_detector.rs
    │   │   └── executor.rs
    │   ├── redis/
    │   │   ├── mod.rs
    │   │   └── publisher.rs
    │   └── config/
    │       ├── mod.rs
    │       └── settings.rs
    └── Dockerfile
```

#### Step 2.2: Dependencies (`Cargo.toml`)

```toml
[package]
name = "rbs-stop-executor"
version = "0.1.0"
edition = "2021"

[dependencies]
# Async runtime
tokio = { version = "1", features = ["full"] }
tokio-tungstenite = "0.20"  # WebSocket client

# HTTP & JSON
reqwest = { version = "0.11", features = ["json"] }
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"

# Database
sqlx = { version = "0.7", features = ["runtime-tokio-native-tls", "postgres", "decimal"] }
rust_decimal = "1.33"

# Redis
redis = { version = "0.23", features = ["tokio-comp", "connection-manager"] }

# Configuration
config = "0.13"
dotenvy = "0.15"

# Logging & Tracing
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }

# Metrics
prometheus = "0.13"

# Error handling
anyhow = "1.0"
thiserror = "1.0"

# UUID
uuid = { version = "1.0", features = ["v4"] }
```

#### Step 2.3: Core Implementation Skeleton

**`src/main.rs`**:
```rust
use anyhow::Result;
use tracing::{info, error};

mod binance;
mod database;
mod monitor;
mod redis;
mod config;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter("rbs_stop_executor=info")
        .init();

    info!("🚀 Robson Stop Executor starting...");

    // Load configuration
    let settings = config::Settings::new()?;

    // Initialize database pool
    let db_pool = database::init_pool(&settings.database_url).await?;

    // Initialize Redis connection
    let redis_client = redis::init_client(&settings.redis_url)?;

    // Initialize Binance WebSocket client
    let ws_client = binance::websocket::BinanceWsClient::new(&settings.binance_ws_url);

    // Start monitoring loop
    let monitor = monitor::StopMonitor::new(db_pool, redis_client, ws_client);

    match monitor.run().await {
        Ok(_) => info!("Monitor stopped gracefully"),
        Err(e) => error!("Monitor error: {}", e),
    }

    Ok(())
}
```

**`src/monitor/trigger_detector.rs`**:
```rust
use rust_decimal::Decimal;
use sqlx::PgPool;
use tracing::info;

use crate::database::models::{Operation, OperationStatus};

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
    ) -> Result<Vec<i64>, sqlx::Error> {
        let operations = sqlx::query_as::<_, Operation>(
            r#"
            SELECT * FROM operation
            WHERE status = $1
            AND symbol_id IN (SELECT id FROM symbol WHERE name = $2)
            "#
        )
        .bind(OperationStatus::Active.to_string())
        .bind(symbol)
        .fetch_all(&self.db)
        .await?;

        let mut triggered = Vec::new();

        for op in operations {
            if self.is_triggered(&op, current_price) {
                info!("🚨 Stop triggered for operation {}", op.id);
                triggered.push(op.id);
            }
        }

        Ok(triggered)
    }

    fn is_triggered(&self, op: &Operation, current_price: Decimal) -> bool {
        // Calculate stop price from percentage
        let stop_loss_percent = match &op.stop_loss_percent {
            Some(pct) => pct,
            None => return false,
        };

        let entry_price = match &op.average_entry_price {
            Some(price) => price,
            None => return false,
        };

        let stop_price = if op.side == "BUY" {
            entry_price * (Decimal::from(100) - stop_loss_percent) / Decimal::from(100)
        } else {
            entry_price * (Decimal::from(100) + stop_loss_percent) / Decimal::from(100)
        };

        // Check if triggered
        if op.side == "BUY" {
            current_price <= stop_price
        } else {
            current_price >= stop_price
        }
    }
}
```

**`src/monitor/executor.rs`**:
```rust
use rust_decimal::Decimal;
use sqlx::{PgPool, Postgres, Transaction};
use tracing::{info, error, warn};
use uuid::Uuid;

use crate::binance::rest::BinanceClient;
use crate::database::models::{Operation, OperationStatus};

pub struct StopExecutor {
    db: PgPool,
    binance: BinanceClient,
}

impl StopExecutor {
    pub fn new(db: PgPool, binance: BinanceClient) -> Self {
        Self { db, binance }
    }

    pub async fn execute(
        &self,
        operation_id: i64,
    ) -> Result<String, ExecutionError> {
        // Step 1: Acquire lock (optimistic locking)
        let token = self.claim_operation(operation_id).await?;

        // Step 2: Get operation details
        let op = self.get_operation(operation_id).await?;

        // Step 3: Place market order on Binance
        info!("Placing market order for operation {}", operation_id);

        let order_response = self.binance.place_market_order(
            &op.symbol_name,
            if op.side == "BUY" { "SELL" } else { "BUY" },
            op.total_entry_quantity,
        ).await?;

        // Step 4: Update database
        self.save_exit_order(operation_id, &order_response).await?;

        info!(
            "✅ Stop-loss executed: Op {} -> Order {}",
            operation_id, order_response.order_id
        );

        Ok(order_response.order_id)
    }

    async fn claim_operation(
        &self,
        operation_id: i64,
    ) -> Result<String, ExecutionError> {
        let token = Uuid::new_v4().to_string();

        let updated = sqlx::query(
            r#"
            UPDATE operation
            SET
                status = $1,
                version = version + 1,
                stop_execution_token = $2,
                last_stop_check_at = NOW()
            WHERE
                id = $3
                AND status = $4
                AND stop_execution_token IS NULL
            "#
        )
        .bind(OperationStatus::StopTriggered.to_string())
        .bind(&token)
        .bind(operation_id)
        .bind(OperationStatus::Active.to_string())
        .execute(&self.db)
        .await?
        .rows_affected();

        if updated == 0 {
            return Err(ExecutionError::AlreadyProcessed);
        }

        Ok(token)
    }

    // ... rest of implementation
}

#[derive(Debug, thiserror::Error)]
pub enum ExecutionError {
    #[error("Operation already processed by another worker")]
    AlreadyProcessed,

    #[error("Database error: {0}")]
    Database(#[from] sqlx::Error),

    #[error("Binance API error: {0}")]
    BinanceApi(String),

    #[error("Orphaned order: {0}")]
    OrphanedOrder(String),
}
```

---

### Phase 3: Kubernetes Deployment (Week 3)

#### Step 3.1: Dockerfile

**`apps/backend/stop-executor/Dockerfile`**:
```dockerfile
FROM rust:1.75 AS builder

WORKDIR /usr/src/app

# Copy manifests
COPY Cargo.toml Cargo.lock ./

# Build dependencies (cache layer)
RUN mkdir src && echo "fn main() {}" > src/main.rs
RUN cargo build --release
RUN rm -rf src

# Copy source code
COPY src ./src

# Build application
RUN cargo build --release

# Runtime image
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /usr/src/app/target/release/rbs-stop-executor /usr/local/bin/

CMD ["rbs-stop-executor"]
```

#### Step 3.2: Kubernetes Manifests

**`infra/k8s/prod/rbs-stop-executor-deployment.yml`**:
```yaml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: rbs-stop-executor
  namespace: robson
  labels:
    app: rbs-stop-executor
    component: critical
spec:
  replicas: 2  # HA: Run 2 instances (locks prevent duplicates)
  strategy:
    type: RollingUpdate
    rollingUpdate:
      maxSurge: 1
      maxUnavailable: 0  # Zero downtime
  selector:
    matchLabels:
      app: rbs-stop-executor
  template:
    metadata:
      labels:
        app: rbs-stop-executor
      annotations:
        prometheus.io/scrape: "true"
        prometheus.io/port: "9090"
        prometheus.io/path: "/metrics"
    spec:
      containers:
      - name: stop-executor
        image: ldamasio/rbs-stop-executor:latest
        imagePullPolicy: Always
        env:
        - name: RUST_LOG
          value: "rbs_stop_executor=info"
        - name: DATABASE_URL
          valueFrom:
            secretKeyRef:
              name: rbs-django-secret
              key: DATABASE_URL  # postgres://user:pass@host/db
        - name: REDIS_URL
          value: "redis://rbs-redis:6379"
        - name: BINANCE_WS_URL
          value: "wss://stream.binance.com:9443/ws"
        - name: BINANCE_API_KEY
          valueFrom:
            secretKeyRef:
              name: rbs-django-secret
              key: RBS_BINANCE_API_KEY_PROD
        - name: BINANCE_SECRET_KEY
          valueFrom:
            secretKeyRef:
              name: rbs-django-secret
              key: RBS_BINANCE_SECRET_KEY_PROD
        ports:
        - containerPort: 9090  # Metrics
          name: metrics
        resources:
          requests:
            memory: "128Mi"
            cpu: "100m"
          limits:
            memory: "512Mi"
            cpu: "1000m"
        livenessProbe:
          httpGet:
            path: /health
            port: 9090
          initialDelaySeconds: 10
          periodSeconds: 30
        readinessProbe:
          httpGet:
            path: /ready
            port: 9090
          initialDelaySeconds: 5
          periodSeconds: 10
---
apiVersion: v1
kind: Service
metadata:
  name: rbs-stop-executor
  namespace: robson
spec:
  selector:
    app: rbs-stop-executor
  ports:
  - name: metrics
    port: 9090
    targetPort: 9090
```

#### Step 3.3: Update CronJob to Backstop Role

**`infra/k8s/prod/rbs-stop-monitor-cronjob.yml`**:
```yaml
apiVersion: batch/v1
kind: CronJob
metadata:
  name: rbs-stop-monitor-backstop
  namespace: robson
  labels:
    app: rbs-stop-monitor-backstop
spec:
  # ⭐ CHANGED: Every 5 minutes (backstop, not primary)
  schedule: "*/5 * * * *"
  successfulJobsHistoryLimit: 3
  failedJobsHistoryLimit: 1
  concurrencyPolicy: Forbid
  jobTemplate:
    spec:
      ttlSecondsAfterFinished: 300
      template:
        metadata:
          labels:
            app: rbs-stop-monitor-backstop
        spec:
          restartPolicy: Never
          containers:
          - name: stop-monitor
            image: ldamasio/rbs-backend-monolith-prod:latest
            command:
            - python
            - manage.py
            - monitor_stops
            # ⭐ REMOVED: --dry-run
            # This is now a BACKSTOP: catches what Rust service missed
            env:
            # ... (same as before, omitted for brevity)
```

---

### Phase 4: Go Notification Service (Week 4)

**Purpose**: Separate dashboard notifications from critical execution.

#### Step 4.1: Update Go Server

**`cli/cmd/server.go`** (modify):
```go
// Remove mock price generation (line 110-137)
// Instead, subscribe to Redis channel published by Rust service

func runServer() {
    ctx := context.Background()
    hub := newHub()
    go hub.run()

    rdb := redis.NewClient(&redis.Options{
        Addr: redisAddr,
    })

    // Subscribe to stop-loss events from Rust service
    go func() {
        pubsub := rdb.Subscribe(ctx, "stop_events", "price_updates")
        defer pubsub.Close()

        ch := pubsub.Channel()
        for msg := range ch {
            // Forward to WebSocket clients
            hub.broadcast <- []byte(msg.Payload)
        }
    }()

    // HTTP/WebSocket Server
    http.HandleFunc("/ws", func(w http.ResponseWriter, r *http.Request) {
        serveWs(hub, w, r)
    })

    log.Printf("Starting notification WebSocket server on :%s", wsPort)
    if err := http.ListenAndServe(":"+wsPort, nil); err != nil {
        log.Fatal("ListenAndServe: ", err)
    }
}
```

#### Step 4.2: Kubernetes Deployment

**`infra/k8s/prod/rbs-ws-notify-deployment.yml`**:
```yaml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: rbs-ws-notify
  namespace: robson
spec:
  replicas: 2
  selector:
    matchLabels:
      app: rbs-ws-notify
  template:
    metadata:
      labels:
        app: rbs-ws-notify
    spec:
      containers:
      - name: ws-notify
        image: ldamasio/rbs-cli:latest
        command: ["robson", "server", "--redis", "rbs-redis:6379", "--port", "8080"]
        ports:
        - containerPort: 8080
          name: websocket
        resources:
          requests:
            memory: "64Mi"
            cpu: "50m"
          limits:
            memory: "256Mi"
            cpu: "500m"
---
apiVersion: v1
kind: Service
metadata:
  name: rbs-ws-notify
  namespace: robson
spec:
  selector:
    app: rbs-ws-notify
  ports:
  - name: websocket
    port: 8080
    targetPort: 8080
```

---

## Documentation Deliverables

### 1. Runbook: Operating the Stop-Loss Monitor

**`docs/runbooks/stop-loss-monitor.md`**:
```markdown
# Stop-Loss Monitor Runbook

## Architecture Overview

- **Primary**: Rust service (rbs-stop-executor) - Real-time WebSocket monitoring
- **Backstop**: Python CronJob (every 5 minutes) - Catches missed triggers
- **Notifications**: Go service (rbs-ws-notify) - Dashboard WebSocket

## Normal Operations

### Check Service Health

```bash
# Rust service
kubectl get pods -n robson -l app=rbs-stop-executor
kubectl logs -n robson -l app=rbs-stop-executor --tail=100

# CronJob (backstop)
kubectl get cronjob -n robson rbs-stop-monitor-backstop
kubectl get jobs -n robson -l app=rbs-stop-monitor-backstop

# Go notification service
kubectl get pods -n robson -l app=rbs-ws-notify
```

### Monitor Metrics (Grafana)

1. Navigate to: `https://grafana.robsonbot.com/d/stop-monitor`
2. Check:
   - `robson_stop_triggers_total` (increasing steadily)
   - `robson_stop_executions_success_total` (should be 100%)
   - `robson_stop_execution_latency_seconds` (P95 < 1s)

## Troubleshooting

### Scenario 1: Stop-Loss Not Executing

**Symptoms**: Price hit stop level, but position still open.

**Diagnosis**:
```bash
# Check Rust service logs
kubectl logs -n robson -l app=rbs-stop-executor --tail=200 | grep ERROR

# Check if operation is in correct state
psql -c "SELECT id, status, stop_loss_percent, stop_execution_token FROM operation WHERE id = <OP_ID>;"

# Check CronJob backstop logs
kubectl logs -n robson -l app=rbs-stop-monitor-backstop --tail=50
```

**Possible Causes**:
1. Rust service crashed → Backstop will execute within 5 minutes
2. Database lock contention → Check for long-running transactions
3. Binance API error → Check Binance status page

**Resolution**:
```bash
# If Rust service down, restart:
kubectl rollout restart deployment/rbs-stop-executor -n robson

# If Binance API issue, wait for recovery (backstop will retry)

# Manual execution (last resort):
python manage.py monitor_stops  # Run once manually
```

### Scenario 2: Orphaned Orders

**Symptoms**: Order placed on Binance, but DB not updated.

**Diagnosis**:
```bash
# Check logs for "ORPHANED ORDER" errors
kubectl logs -n robson -l app=rbs-stop-executor | grep ORPHANED

# Query operations with STOP_TRIGGERED but no exit order
psql -c "
SELECT o.id, o.status, COUNT(exit.id) AS exit_orders
FROM operation o
LEFT JOIN operation_exit_orders exit ON exit.operation_id = o.id
WHERE o.status = 'STOP_TRIGGERED'
GROUP BY o.id
HAVING COUNT(exit.id) = 0;
"
```

**Resolution**:
```bash
# Manually reconcile:
python manage.py reconcile_orphaned_orders
# (You'll need to implement this command)
```

### Scenario 3: High Execution Latency

**Symptoms**: Alert: "P95 stop execution latency >1s"

**Diagnosis**:
```bash
# Check Prometheus metrics
curl http://rbs-stop-executor:9090/metrics | grep execution_latency

# Check database performance
psql -c "SELECT * FROM pg_stat_statements ORDER BY mean_exec_time DESC LIMIT 10;"
```

**Possible Causes**:
1. Database connection pool exhausted
2. Binance API slow response
3. Network latency

**Resolution**:
- Scale Rust service: `kubectl scale deployment/rbs-stop-executor --replicas=3 -n robson`
- Check database indexes on `operation` table
- Monitor Binance API status

## Maintenance Windows

### Rolling Restart (Zero Downtime)

```bash
# Rust service (2 replicas, safe to restart)
kubectl rollout restart deployment/rbs-stop-executor -n robson
kubectl rollout status deployment/rbs-stop-executor -n robson

# Go notification service
kubectl rollout restart deployment/rbs-ws-notify -n robson
```

### Database Migration

1. Stop Rust service: `kubectl scale deployment/rbs-stop-executor --replicas=0 -n robson`
2. Run migration: `python manage.py migrate`
3. Start Rust service: `kubectl scale deployment/rbs-stop-executor --replicas=2 -n robson`

(CronJob backstop will continue running during migration)
```

### 2. ADR: Why Not Use Exchange Stop-Orders

**`docs/adr/ADR-0013-market-orders-only.md`**:
```markdown
# ADR-0013: Always Use Market Orders for Stop-Loss (Never Exchange Stop-Orders)

## Status

Accepted

## Context

Binance offers native STOP_LOSS_LIMIT and STOP_LOSS (market) order types that automatically
execute when a price threshold is crossed. This might seem more efficient than monitoring
prices ourselves and placing market orders.

## Decision

**We will NEVER use Binance's native stop-loss order types. We will ALWAYS:**
1. Monitor prices ourselves (WebSocket + polling backstop)
2. Detect triggers internally
3. Place MARKET orders manually when triggered

## Rationale

### 1. **Control & Observability**

**Native Stop-Orders**:
- ❌ Binance's black box (we don't know when/how it triggered)
- ❌ No audit trail on our side
- ❌ Can't add custom logic (multi-tenant filters, risk checks)

**Our Approach**:
- ✅ Full visibility into trigger detection
- ✅ Complete audit trail in our database
- ✅ Can apply business rules before execution

### 2. **Risk Management Integration**

**Native Stop-Orders**:
- ❌ No pre-execution checks
- ❌ Can't validate drawdown limits
- ❌ Can't block execution if monthly risk exceeded

**Our Approach**:
- ✅ Validate risk limits before placing order
- ✅ Check portfolio exposure
- ✅ Enforce multi-tenant isolation

### 3. **Resilience**

**Native Stop-Orders**:
- ❌ If Binance glitches, order might not execute
- ❌ No retry logic
- ❌ We don't know it failed until too late

**Our Approach**:
- ✅ Retry with exponential backoff
- ✅ Backstop CronJob catches missed triggers
- ✅ Alerts on failures

### 4. **Testing**

**Native Stop-Orders**:
- ❌ Hard to test (requires real price movement)
- ❌ Can't simulate in testnet reliably

**Our Approach**:
- ✅ Can mock price streams
- ✅ Full integration tests without real trading

### 5. **Market Orders Guarantee Execution**

**STOP_LOSS_LIMIT**:
- ❌ Might not fill if price gaps past limit
- ❌ Leaves you exposed if limit not hit

**MARKET Orders**:
- ✅ Guaranteed execution (accepts slippage)
- ✅ Protects capital (better some loss than total loss)

## Consequences

### Positive

- Full control over execution logic
- Complete audit trail
- Testable and resilient
- Integration with risk management

### Negative

- Must maintain WebSocket infrastructure
- More complex (but mitigated by Rust service + backstop)
- Slight latency vs. native stop-orders (acceptable trade-off)

## Compliance

This decision aligns with:
- **GOLDEN RULE**: Position sizing from technical stops (we need control over execution)
- **Robson's Mission**: Risk management assistant (requires observability)
- **Multi-Tenant**: Isolation requires our logic, not exchange's
```

### 3. Migration Guide

**`docs/guides/migrate-to-rust-stop-monitor.md`**:
```markdown
# Migration Guide: Python CronJob → Rust Stop Monitor

## Prerequisites

- [ ] Kubernetes cluster access
- [ ] Database access (psql)
- [ ] Docker registry credentials
- [ ] Redis deployed (`rbs-redis`)

## Step 1: Database Schema Migration

```bash
# Add new fields to Operation model
python manage.py makemigrations api --name add_stop_execution_fields

# Apply migration
python manage.py migrate
```

**SQL Preview**:
```sql
ALTER TABLE operation
ADD COLUMN version INTEGER DEFAULT 1 NOT NULL,
ADD COLUMN stop_execution_attempts INTEGER DEFAULT 0 NOT NULL,
ADD COLUMN last_stop_check_at TIMESTAMP WITH TIME ZONE,
ADD COLUMN stop_execution_token VARCHAR(64) UNIQUE;

CREATE INDEX idx_operation_status_version ON operation(status, version);
CREATE INDEX idx_operation_stop_token ON operation(stop_execution_token);
```

## Step 2: Build & Push Rust Image

```bash
cd apps/backend/stop-executor

# Build
docker build -t ldamasio/rbs-stop-executor:latest .

# Test locally
docker run --rm \
  -e DATABASE_URL="postgres://..." \
  -e REDIS_URL="redis://localhost:6379" \
  ldamasio/rbs-stop-executor:latest

# Push
docker push ldamasio/rbs-stop-executor:latest
```

## Step 3: Deploy Rust Service

```bash
# Apply Kubernetes manifests
kubectl apply -f infra/k8s/prod/rbs-stop-executor-deployment.yml

# Check status
kubectl get pods -n robson -l app=rbs-stop-executor
kubectl logs -n robson -l app=rbs-stop-executor --tail=100
```

## Step 4: Update CronJob (Gradual Migration)

### Phase A: Dual Mode (Both Running)

```bash
# Keep existing CronJob running
# Rust service runs in parallel
# Both detect triggers, but optimistic lock prevents duplicates
```

**Monitor for 1 week**:
```bash
# Check metrics
kubectl logs -n robson -l app=rbs-stop-executor | grep "RACE CONDITION" | wc -l
# (Should be non-zero, confirming both are running)
```

### Phase B: Reduce CronJob Frequency

```bash
# Update CronJob to 5-minute backstop
kubectl apply -f infra/k8s/prod/rbs-stop-monitor-cronjob.yml

# CronJob now runs every 5 minutes (backstop mode)
```

**Monitor for 2 weeks**:
```bash
# Check if CronJob ever catches anything
kubectl logs -n robson -l app=rbs-stop-monitor-backstop | grep "STOP_LOSS triggered"
# (Should be very rare, confirming Rust service is primary)
```

### Phase C: Production Mode

```bash
# Update CronJob schedule to hourly (safety net only)
# Edit cronjob:
kubectl edit cronjob rbs-stop-monitor-backstop -n robson
# Change schedule: "0 * * * *"  # Every hour
```

## Step 5: Rollback Plan

If issues detected:

```bash
# Scale down Rust service
kubectl scale deployment/rbs-stop-executor --replicas=0 -n robson

# Restore CronJob to 1-minute schedule
kubectl edit cronjob rbs-stop-monitor-backstop -n robson
# Change schedule back to: "* * * * *"
```

## Step 6: Monitoring Setup

```bash
# Add Grafana dashboard
# Import: docs/dashboards/stop-monitor.json

# Configure AlertManager rules
kubectl apply -f infra/k8s/monitoring/stop-monitor-alerts.yml
```

## Validation Checklist

- [ ] Database migration applied
- [ ] Rust service deployed (2 replicas)
- [ ] Metrics endpoint accessible (`:9090/metrics`)
- [ ] Redis pub/sub working (check logs)
- [ ] CronJob backstop running (5-minute schedule)
- [ ] Grafana dashboard showing metrics
- [ ] AlertManager rules configured
- [ ] Test stop-loss execution on testnet
- [ ] Monitor for 1 week before full production
```

---

## Testing Strategy

### Unit Tests (Rust)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn test_trigger_detection_long_position() {
        let op = Operation {
            id: 1,
            side: "BUY".to_string(),
            stop_loss_percent: Some(dec!(2.0)),
            average_entry_price: Some(dec!(95000)),
            // ...
        };

        let detector = TriggerDetector::new(/* mock db */);

        // Price above stop: not triggered
        assert!(!detector.is_triggered(&op, dec!(94000)));

        // Price at stop: triggered
        assert!(detector.is_triggered(&op, dec!(93100)));

        // Price below stop: triggered
        assert!(detector.is_triggered(&op, dec!(93000)));
    }

    #[tokio::test]
    async fn test_idempotency() {
        let executor = StopExecutor::new(/* mock db */, /* mock binance */);

        // Execute twice
        let result1 = executor.execute(123).await;
        let result2 = executor.execute(123).await;

        // First succeeds
        assert!(result1.is_ok());

        // Second returns AlreadyProcessed
        assert_matches!(result2, Err(ExecutionError::AlreadyProcessed));
    }
}
```

### Integration Tests (Python)

```python
import pytest
from decimal import Decimal
from django.utils import timezone

from api.models import Operation, Order, Symbol, Strategy
from api.application.stop_monitor import PriceMonitor, StopExecutor

@pytest.mark.django_db
def test_stop_loss_execution_end_to_end(client, binance_mock):
    """Test full stop-loss flow from trigger to execution."""

    # Setup: Create active operation
    symbol = Symbol.objects.create(name="BTCUSDC", base_asset="BTC", quote_asset="USDC")
    strategy = Strategy.objects.create(name="Test Strategy")

    operation = Operation.objects.create(
        symbol=symbol,
        strategy=strategy,
        side="BUY",
        status="ACTIVE",
        stop_loss_percent=Decimal("2.0"),
    )

    # Create entry order
    entry_order = Order.objects.create(
        symbol=symbol,
        side="BUY",
        quantity=Decimal("0.1"),
        avg_fill_price=Decimal("95000"),
        status="FILLED",
    )
    operation.entry_orders.add(entry_order)

    # Mock Binance API
    binance_mock.create_order.return_value = {
        "orderId": 123456,
        "executedQty": "0.1",
        "fills": [{"price": "93100", "qty": "0.1", "commission": "0.0001"}],
    }

    # Simulate price drop to stop level
    current_price = Decimal("93100")

    # Act: Monitor detects trigger
    monitor = PriceMonitor(binance_mock)
    triggers = monitor.check_all_operations()

    assert len(triggers) == 1
    assert triggers[0].operation_id == operation.id
    assert triggers[0].trigger_type == "STOP_LOSS"

    # Act: Executor places order
    executor = StopExecutor(binance_mock)
    result = executor.execute(triggers[0])

    # Assert: Order created
    assert result.success is True
    assert result.order_id is not None

    # Assert: Operation closed
    operation.refresh_from_db()
    assert operation.status == "CLOSED"

    # Assert: Exit order exists
    assert operation.exit_orders.count() == 1
    exit_order = operation.exit_orders.first()
    assert exit_order.side == "SELL"
    assert exit_order.status == "FILLED"

@pytest.mark.django_db
def test_concurrent_execution_idempotency():
    """Test that concurrent executions don't create duplicate orders."""
    import threading

    # Setup operation
    op = Operation.objects.create(
        # ... (same as above)
        status="ACTIVE",
    )

    results = []

    def execute_stop():
        executor = StopExecutor()
        result = executor.execute(op.id)
        results.append(result)

    # Run two threads simultaneously
    threads = [threading.Thread(target=execute_stop) for _ in range(2)]
    for t in threads:
        t.start()
    for t in threads:
        t.join()

    # Exactly one succeeds, one gets AlreadyProcessed
    successes = [r for r in results if r.success]
    failures = [r for r in results if not r.success]

    assert len(successes) == 1
    assert len(failures) == 1
    assert "AlreadyProcessed" in failures[0].error
```

---

## Risks & Mitigation

| Risk | Impact | Probability | Mitigation |
|------|--------|-------------|------------|
| **Rust service crash** | Stops not executed | Medium | CronJob backstop (5-min max delay) |
| **Database connection pool exhausted** | Execution failures | Low | Connection pooling + limits |
| **Binance API rate limit** | Execution throttled | Low | Exponential backoff + retry |
| **Orphaned orders** | DB inconsistency | Low | Reconciliation script + alerts |
| **Price gap (slippage)** | Worse fill than expected | High | Accept slippage (market orders) |
| **WebSocket disconnection** | Missed price updates | Medium | Auto-reconnect + heartbeat |

---

## Success Metrics

### Phase 1 (Immediate - Fix Current System)
- [ ] `stop_loss_percent` populated on operation creation
- [ ] `--dry-run` removed from production CronJob
- [ ] At least 1 successful stop-loss execution in testnet
- [ ] Audit trail complete in database

### Phase 2 (Rust Service)
- [ ] Rust service deployed (2 replicas)
- [ ] Metrics endpoint serving data
- [ ] WebSocket connected to Binance
- [ ] At least 10 successful stop executions
- [ ] Zero duplicate executions (idempotency working)

### Phase 3 (Production)
- [ ] P95 execution latency < 500ms
- [ ] 99.9% success rate (failures only due to Binance API issues)
- [ ] Zero data inconsistencies (orphaned orders)
- [ ] CronJob backstop triggers <1% of executions

---

## Timeline

| Phase | Duration | Deliverables |
|-------|----------|--------------|
| **Phase 1**: Fix Current System | 2 days | Updated views, CronJob YAML, tests |
| **Phase 2**: Rust Service Foundation | 2 weeks | Rust crate, Docker image, unit tests |
| **Phase 3**: Kubernetes Deployment | 1 week | Manifests, integration tests, monitoring |
| **Phase 4**: Go Notification Service | 1 week | Updated Go server, WebSocket deployment |
| **Phase 5**: Documentation | 3 days | Runbook, ADR, migration guide |
| **Phase 6**: Production Migration | 2 weeks | Gradual rollout, monitoring, validation |

**Total**: ~6 weeks

---

## Alternatives Considered

### Alternative 1: Keep Python, Add WebSocket

**Pros**: Familiar stack, reuse Django ORM
**Cons**: GIL limits concurrency, slower execution
**Decision**: Rejected. Critical path requires performance.

### Alternative 2: Use Binance Native Stop-Orders

**Pros**: Simpler, no infrastructure
**Cons**: No control, no audit trail, no risk checks
**Decision**: Rejected. See ADR-0013.

### Alternative 3: Single WebSocket (Rust Only)

**Pros**: Simpler architecture
**Cons**: Rust WebSocket broadcast more complex than Go
**Decision**: Rejected. Dual service (Rust critical + Go notify) is cleaner.

---

## References

- [CLAUDE.md](../../CLAUDE.md): Project context
- [ADR-0007](./ADR-0007-strategy-semantic-clarity.md): Strategy semantics
- [ADR-0011](./ADR-0011-gitops-automatic-manifest-updates.md): GitOps auto-deploy
- [POSITION-SIZING-GOLDEN-RULE.md](../requirements/POSITION-SIZING-GOLDEN-RULE.md): Technical stops

---

**Author**: Claude Code AI Assistant
**Reviewed**: Pending
**Approved**: Pending
