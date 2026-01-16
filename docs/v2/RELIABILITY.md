# Robson v2 Reliability Architecture

**Version**: 2.0.0-alpha
**Last Updated**: 2026-01-12
**Status**: Planning Phase

---

## Table of Contents

1. [Reliability Goals](#reliability-goals)
2. [Failure Modes](#failure-modes)
3. [Leader Election](#leader-election)
4. [Reconciliation](#reconciliation)
5. [Idempotency](#idempotency)
6. [Degraded Mode](#degraded-mode)
7. [Insurance Stop (Optional)](#insurance-stop-optional)
8. [Observability](#observability)
9. [Architecture Decision Records](#architecture-decision-records)

---

## Reliability Goals

### RTO (Recovery Time Objective)
- **Target**: < 30 seconds for failover
- **Rationale**: Market moves fast; we need to resume monitoring SL/SG quickly

### RPO (Recovery Point Objective)
- **Target**: Zero data loss (event sourcing + WAL)
- **Rationale**: Every decision must be auditable

### Availability
- **Target**: 99.9% uptime (< 9 hours downtime/year)
- **Rationale**: Positions are monitored 24/7

### Consistency
- **Model**: Eventually consistent with exchange as source of truth
- **Guarantee**: No duplicate orders, no lost stop-loss triggers

---

## Failure Modes

### 1. Pod Crash (OOMKill, panic, bug)
**Impact**: Active position unmonitored until recovery

**Mitigation**:
- Kubernetes restarts pod automatically
- On startup: Reconcile state, resume monitoring
- If price passed SL during downtime: Close immediately

**Detection**: Liveness probe failure

### 2. Node Failure (hardware, network)
**Impact**: All pods on node down

**Mitigation**:
- Kubernetes reschedules pods on healthy nodes
- Leader election ensures only one active trader
- Lease TTL expires, new leader takes over

**Detection**: Node NotReady status

### 3. Network Partition
**Impact**: Cannot reach exchange or database

**Mitigation**:
- Lease heartbeat fails → Self-terminate
- Exchange WebSocket reconnect with exponential backoff
- Database connection pool retry

**Detection**: Lease renewal failure, WebSocket disconnect

### 4. Exchange Downtime (Binance API outage)
**Impact**: Cannot place/cancel orders

**Mitigation**:
- Retry with exponential backoff
- Alert on repeated failures
- (Optional) Insurance stop on exchange as last resort

**Detection**: HTTP 5xx, timeout, WebSocket disconnect

### 5. Database Unavailability
**Impact**: Cannot persist events, cannot acquire lease

**Mitigation**:
- In-memory buffer for events (bounded)
- Flush on DB recovery
- If lease cannot be renewed: Graceful shutdown

**Detection**: Connection pool exhaustion, query timeout

### 6. Clock Skew / Time Desync
**Impact**: Lease TTL calculation incorrect, order timestamps wrong

**Mitigation**:
- NTP sync on all nodes
- Lease uses monotonic time + server-side TTL
- Exchange timestamps are authoritative

**Detection**: Monitor clock drift via health checks

### 7. Split-Brain (Multiple Active Traders)
**Impact**: Duplicate orders, conflicting decisions

**Mitigation**:
- Leader election with exclusive lease
- Fencing: Lease holder ID in all operations
- Detect: Query exchange before placing order

**Detection**: Duplicate order errors, inconsistent state

---

## Leader Election

### Strategy: Postgres Advisory Locks with TTL

**Why Postgres over Kubernetes Lease API?**
- ✅ Same database used for events (single source of truth)
- ✅ Transactional guarantees
- ✅ Simpler than etcd/Consul
- ❌ Requires database availability (acceptable trade-off)

### Implementation

```rust
pub struct LeaseManager {
    pool: PgPool,
    lease_key: String,  // e.g., "account:BTC_symbol:BTCUSDT"
    instance_id: Uuid,
    ttl: Duration,      // Default: 30 seconds
}

impl LeaseManager {
    pub async fn acquire_lease(&self) -> Result<Lease> {
        // Try to acquire advisory lock
        let acquired = sqlx::query!(
            r#"
            SELECT pg_try_advisory_lock($1) AS locked,
                   NOW() AS acquired_at
            "#,
            self.lease_key_hash()
        )
        .fetch_one(&self.pool)
        .await?;

        if acquired.locked {
            // Write lease record with TTL
            sqlx::query!(
                r#"
                INSERT INTO leases (key, instance_id, acquired_at, expires_at)
                VALUES ($1, $2, NOW(), NOW() + $3)
                ON CONFLICT (key) DO UPDATE
                SET instance_id = EXCLUDED.instance_id,
                    acquired_at = EXCLUDED.acquired_at,
                    expires_at = EXCLUDED.expires_at
                "#,
                self.lease_key,
                self.instance_id,
                self.ttl
            )
            .execute(&self.pool)
            .await?;

            Ok(Lease {
                key: self.lease_key.clone(),
                instance_id: self.instance_id,
                expires_at: acquired.acquired_at + self.ttl,
            })
        } else {
            Err(LeaseError::AlreadyHeld)
        }
    }

    pub async fn renew_lease(&self, lease: &Lease) -> Result<()> {
        // Heartbeat: Extend TTL
        let rows_affected = sqlx::query!(
            r#"
            UPDATE leases
            SET expires_at = NOW() + $1
            WHERE key = $2 AND instance_id = $3
            "#,
            self.ttl,
            lease.key,
            lease.instance_id
        )
        .execute(&self.pool)
        .await?
        .rows_affected();

        if rows_affected == 0 {
            return Err(LeaseError::LostLease);
        }

        Ok(())
    }

    pub async fn release_lease(&self, lease: &Lease) -> Result<()> {
        sqlx::query!(
            r#"
            DELETE FROM leases WHERE key = $1 AND instance_id = $2;
            SELECT pg_advisory_unlock($3);
            "#,
            lease.key,
            lease.instance_id,
            self.lease_key_hash()
        )
        .execute(&self.pool)
        .await?;

        Ok(())
    }
}
```

### Lease Table Schema

```sql
CREATE TABLE leases (
    key TEXT PRIMARY KEY,              -- "account:symbol"
    instance_id UUID NOT NULL,         -- Daemon instance ID
    acquired_at TIMESTAMPTZ NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL,
    CONSTRAINT expires_in_future CHECK (expires_at > acquired_at)
);

CREATE INDEX idx_leases_expires_at ON leases(expires_at);
```

### Lease Lifecycle

```
1. Daemon starts → Generate instance_id (UUID)
2. Acquire lease for (account, symbol)
   - If acquired: Proceed to reconciliation
   - If not acquired: Wait + retry OR exit
3. Main loop: Renew lease every 10 seconds (TTL = 30s)
4. On SIGTERM: Release lease, cancel orders, shutdown
5. If renew fails: Lost lease → Graceful shutdown
```

### Fencing Token

Every operation includes `instance_id` for fencing:

```rust
pub struct OrderIntent {
    pub intent_id: Uuid,
    pub instance_id: Uuid,  // Lease holder ID
    pub symbol: Symbol,
    pub side: Side,
    pub quantity: Quantity,
    // ...
}
```

If `instance_id` doesn't match current lease holder → Reject

---

## Reconciliation

### When to Reconcile

1. **On Startup**: Before starting main loop
2. **After Lease Acquisition**: Verify state consistency
3. **After Network Reconnect**: Re-sync with exchange
4. **On Demand**: CLI command `robson reconcile`

### Reconciliation Process

```rust
pub struct Reconciler {
    store: Store,
    connector: Box<dyn ExchangeConnector>,
}

impl Reconciler {
    pub async fn reconcile(&self, position_id: PositionId) -> Result<ReconcileReport> {
        // 1. Load position from database (snapshot + events)
        let local_position = self.store.load_position(position_id).await?;

        // 2. Query exchange for ground truth
        let exchange_snapshot = self.connector
            .get_position(local_position.symbol)
            .await?;

        let exchange_orders = self.connector
            .get_open_orders(local_position.symbol)
            .await?;

        // 3. Detect discrepancies
        let report = self.detect_discrepancies(
            &local_position,
            &exchange_snapshot,
            &exchange_orders,
        );

        // 4. Decide on corrective actions
        let actions = self.compute_corrective_actions(&report);

        // 5. Execute actions (e.g., cancel ghost orders, close position)
        for action in actions {
            self.execute_action(action).await?;
        }

        Ok(report)
    }

    fn detect_discrepancies(
        &self,
        local: &Position,
        exchange_snapshot: &PositionSnapshot,
        exchange_orders: &[Order],
    ) -> ReconcileReport {
        let mut issues = Vec::new();

        // Check position size mismatch
        if local.quantity != exchange_snapshot.quantity {
            issues.push(Discrepancy::QuantityMismatch {
                local: local.quantity,
                exchange: exchange_snapshot.quantity,
            });
        }

        // Check for ghost orders (local thinks order exists, but not on exchange)
        for local_order in &local.open_orders {
            if !exchange_orders.iter().any(|o| o.id == local_order.id) {
                issues.push(Discrepancy::GhostOrder {
                    order_id: local_order.id,
                });
            }
        }

        // Check for untracked orders (exchange has order, but not in local state)
        for exchange_order in exchange_orders {
            if !local.open_orders.iter().any(|o| o.id == exchange_order.id) {
                issues.push(Discrepancy::UntrackedOrder {
                    order_id: exchange_order.id,
                });
            }
        }

        ReconcileReport { issues }
    }
}
```

### Reconciliation Scenarios

#### Scenario 1: Price Passed SL During Downtime

```
Local State: Active position, SL = $95,000
Exchange State: Position still open
Current Price: $94,500 (below SL)

Action: Enter degraded mode → Close position immediately (market order)
```

#### Scenario 2: Order Filled But Not Recorded

```
Local State: Entering (entry order pending)
Exchange State: Order filled, position open

Action: Record fill event, update state to Active, start monitoring
```

#### Scenario 3: Ghost Order (Order Cancelled on Exchange)

```
Local State: Exiting (exit order pending)
Exchange State: No open orders, position closed

Action: Mark position as closed, calculate PnL from exchange trades
```

---

## Idempotency

### Intent-Based Execution

Every action gets a unique `intent_id` (UUID v7 for temporal ordering):

```rust
pub struct Intent {
    pub id: IntentId,          // UUID v7 (time-ordered)
    pub position_id: PositionId,
    pub action: ActionType,
    pub status: IntentStatus,
    pub created_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

pub enum IntentStatus {
    Pending,     // Created but not executed
    Executing,   // Sent to exchange
    Completed,   // Confirmed by exchange
    Failed,      // Permanent failure (e.g., insufficient balance)
    Cancelled,   // Explicitly cancelled
}
```

### Idempotency Guarantees

1. **Same Intent ID → Same Outcome**
   - Check journal before executing
   - If intent exists and completed: Return cached result
   - If intent exists and failed: Return error
   - If intent missing: Safe to execute

2. **Write-Ahead Log**
   - Write intent to journal BEFORE sending to exchange
   - If crash before exchange response: Reconcile on startup

3. **Correlation ID**
   - Include intent_id in exchange order `clientOrderId`
   - Query exchange by `clientOrderId` to find order

### Example: Idempotent Order Placement

```rust
impl ExecutionEngine {
    pub async fn place_order(&mut self, intent: OrderIntent) -> Result<OrderResult> {
        // 1. Check if intent already executed
        if let Some(existing) = self.journal.get_intent(&intent.id).await? {
            return match existing.status {
                IntentStatus::Completed => Ok(existing.result.unwrap()),
                IntentStatus::Failed => Err(existing.error.unwrap()),
                IntentStatus::Executing => {
                    // Query exchange for order status
                    self.query_order_status(&intent.id).await
                }
                _ => unreachable!(),
            };
        }

        // 2. Write intent to journal (WAL)
        self.journal.append_intent(&intent).await?;

        // 3. Place order on exchange
        let response = self.connector
            .place_order(OrderRequest {
                client_order_id: intent.id.to_string(),
                symbol: intent.symbol,
                side: intent.side,
                quantity: intent.quantity,
                order_type: OrderType::Market,
            })
            .await;

        // 4. Update intent status
        match response {
            Ok(order) => {
                self.journal.mark_completed(&intent.id, &order).await?;
                Ok(OrderResult::Placed(order))
            }
            Err(e) if e.is_retryable() => {
                // Network error: keep Executing status, retry later
                Err(e)
            }
            Err(e) => {
                // Permanent error
                self.journal.mark_failed(&intent.id, &e).await?;
                Err(e)
            }
        }
    }
}
```

---

## Degraded Mode

When reconciliation detects critical issues, enter **degraded mode**:

### Triggers

1. Price passed SL/SG during downtime
2. Position size mismatch (exchange != local)
3. Unrecognized orders on exchange
4. Critical errors (e.g., margin call risk)

### Actions in Degraded Mode

1. **Close Position Immediately** (market order)
2. **Cancel All Open Orders**
3. **Emit Alert** (log + notification)
4. **Freeze New Operations** (reject arm/disarm commands)
5. **Require Manual Intervention** (admin approval to resume)

### Exit Degraded Mode

```bash
# After reviewing logs and fixing root cause
robson admin clear-degraded --position-id <id> --confirm
```

---

## Insurance Stop (Optional)

**ADR-001: Insurance Stop on Exchange**

### Problem

If daemon crashes AND exchange WebSocket fails, we have no way to close position when SL triggers.

### Options

#### Option A: No Insurance Stop (Current)
- ✅ Simple: All logic in daemon
- ✅ Consistent: One source of truth
- ❌ Risk: Downtime during SL trigger = uncontrolled loss

#### Option B: Dual-Stop (Insurance + Local)
- ✅ Safety: Exchange stop as last resort
- ❌ Complexity: Reconcile two stop mechanisms
- ❌ Race Condition: Both stops trigger simultaneously

#### Option C: Watchdog-Triggered Insurance Stop
- ✅ Safety: Watchdog places exchange stop if daemon unhealthy
- ✅ Consistency: Daemon is still primary
- ❌ Complexity: External watchdog service

### Decision: Option B (Dual-Stop) with Safeguards

**Implementation**:

1. When position becomes Active:
   - Place local SL monitor (primary)
   - Place exchange STOP_LOSS_LIMIT order (insurance, ~1% wider than technical SL)

2. Local monitor triggers first (99% of cases):
   - Place market order
   - Cancel insurance stop
   - Mark position closed

3. Insurance stop triggers (daemon down):
   - Exchange executes stop
   - On daemon recovery: Detect filled insurance stop → Mark closed

4. Both trigger (race):
   - Exchange rejects duplicate close (insufficient balance)
   - Reconcile: Position closed, ignore error

**Trade-offs**:
- 🟢 Increased safety (worst-case loss capped)
- 🔴 Increased complexity (two stop mechanisms)
- 🟡 Risk of insurance stop executing when daemon is fine (if latency spike)

**Mitigation for False Triggers**:
- Insurance stop is STOP_LOSS_LIMIT (not STOP_LOSS_MARKET)
- Limit price set 0.5% worse than stop price
- If market gaps through: Insurance stop doesn't fill (daemon can still close)

**Configuration**:
```rust
pub struct RiskConfig {
    pub insurance_stop_enabled: bool,  // Default: true
    pub insurance_stop_buffer: Decimal, // Default: 0.01 (1%)
    pub insurance_stop_use_limit: bool, // Default: true
}
```

**SQL Schema**:
```sql
CREATE TABLE insurance_stops (
    position_id UUID PRIMARY KEY,
    exchange_order_id TEXT NOT NULL,
    stop_price DECIMAL NOT NULL,
    limit_price DECIMAL NOT NULL,
    placed_at TIMESTAMPTZ NOT NULL,
    cancelled_at TIMESTAMPTZ,
    triggered_at TIMESTAMPTZ
);
```

---

## Observability

### Structured Logging

```rust
use tracing::{info, warn, error};

#[instrument(skip(self))]
async fn place_order(&self, intent: OrderIntent) -> Result<OrderResult> {
    info!(
        intent_id = %intent.id,
        symbol = %intent.symbol,
        side = ?intent.side,
        quantity = %intent.quantity,
        "Placing order"
    );

    let result = self.connector.place_order(intent).await;

    match &result {
        Ok(order) => info!(
            order_id = %order.id,
            "Order placed successfully"
        ),
        Err(e) => error!(
            error = %e,
            "Failed to place order"
        ),
    }

    result
}
```

### Metrics (Future)

- `robson_orders_placed_total` (counter)
- `robson_orders_failed_total` (counter)
- `robson_position_pnl` (gauge)
- `robson_lease_renewals_total` (counter)
- `robson_reconciliation_issues_total` (counter)

### Health Checks

```rust
// Liveness: "Is daemon responsive?"
GET /health/live
→ 200 if API server responding

// Readiness: "Is daemon ready to trade?"
GET /health/ready
→ 200 if lease held + exchange connected + db connected
→ 503 otherwise
```

### Alerts

- Lease lost (failover event)
- Reconciliation detected discrepancy
- Degraded mode entered
- Exchange API errors > threshold
- Database connection lost

---

## Architecture Decision Records

### ADR-001: Dual-Stop Strategy (Insurance + Local)

**Status**: Proposed
**Date**: 2026-01-12

**Context**: Need to protect positions during daemon downtime

**Decision**: Implement dual-stop with insurance stop on exchange as backup

**Consequences**:
- Increased safety (worst-case loss capped)
- Increased complexity (reconcile two mechanisms)
- Requires careful coordination to avoid race conditions

---

### ADR-002: Postgres Advisory Locks for Leader Election

**Status**: Accepted
**Date**: 2026-01-12

**Context**: Need leader election for HA

**Decision**: Use Postgres advisory locks with TTL table

**Alternatives Considered**:
- Kubernetes Lease API (separate system, more moving parts)
- etcd/Consul (overkill for single-cluster deployment)
- Redis locks (less ACID guarantees)

**Consequences**:
- Single source of truth (DB for both events and leases)
- Simpler deployment (no additional services)
- DB is single point of failure (acceptable with Postgres HA)

---

### ADR-003: Event Sourcing with Snapshots

**Status**: Accepted
**Date**: 2026-01-12

**Context**: Need to reconstruct position state after failures

**Decision**: Store all events in append-only log + periodic snapshots

**Consequences**:
- Complete audit trail
- Can replay events for debugging
- Snapshot required for performance (don't replay 10k events)

---

## Validation Checklist

Before marking reliability architecture complete:

- [ ] Leader election tested with multiple instances
- [ ] Reconciliation handles all identified discrepancies
- [ ] Idempotency tested with duplicate requests
- [ ] Degraded mode triggers and exits correctly
- [ ] Insurance stop tested in simulation
- [ ] Health checks integrated with Kubernetes
- [ ] Structured logging tested with real scenarios
- [ ] Lease renewal tested with database failures
- [ ] Split-brain scenario tested (multiple leaders)
- [ ] Failover time measured (< 30s target)

---

**Next Steps**: Implement components in order:
1. Store + Lease Manager
2. Intent Journal + Idempotency
3. Reconciliation Engine
4. Degraded Mode Logic
5. Insurance Stop (optional)

See [EXECUTION-PLAN.md](./EXECUTION-PLAN.md) for detailed roadmap.
