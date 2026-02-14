# ADR-0014: Safety Net and Core Trading Coordination

## Status
**Accepted** (2026-02-14)

## Context

Robson v2 implements **two distinct stop loss modalities** that operate on the same Binance account:

1. **Core Trading (Internal Flow)**
   - User arms position via CLI (`robson arm BTCUSDT`)
   - Detector monitors market, fires entry signal on MA crossover
   - Engine calculates trailing stop dynamically
   - Monitors WebSocket for real-time price updates
   - Exits when trailing stop is hit

2. **Safety Net (External Flow)**
   - User opens position manually on Binance app (bypassing Robson v2)
   - Position Monitor polls Binance API every 20 seconds
   - Detects "rogue" positions not created by Robson
   - Calculates fixed 2% safety stop
   - Exits when safety stop is hit

### The Problem: Double Execution Risk

Without coordination, both systems may attempt to close the same position:

```
Timeline:
1. User arms BTCUSDT via CLI (Core Trading)
2. Detector fires signal, entry order placed
3. Position enters Active state (trailing stop at $93.5k)
4. Safety Net polling detects the Binance position
5. Safety Net calculates safety stop ($93.1k)
6. Price drops to $93.5k
7. Core Trading: TRIGGERED (trailing stop hit)
8. Core executes EXIT order via robson-exec
9. Safety Net: Still monitoring (unaware of Core execution)
10. Safety Net may attempt second EXIT order → ERROR or partial fill
```

**Impact:**
- Order rejection from Binance (insufficient balance)
- Partial fill of second order (overselling)
- Inconsistent state between systems
- Loss tracking errors

### Forces

- Core Trading positions should use trailing stops (better exit strategy)
- Safety Net positions should use fixed stops (minimal protection)
- Same Binance account, same PostgreSQL database
- Both systems run concurrently in the same daemon process
- No single source of truth to distinguish "Core" vs "Manual" positions at the exchange level
- Binance API does not distinguish between "Robson-created" and "manually-created" positions

---

## Decision

We will implement a **three-layer exclusion mechanism** to prevent Safety Net from monitoring Core Trading positions:

### Layer 1: Database Query (Primary)

Safety Net queries the Core Trading `positions` table before monitoring:

```rust
async fn is_core_managed(&self, symbol: &Symbol, side: Side) -> bool {
    // Query: SELECT id FROM positions 
    // WHERE symbol = ? AND side = ? 
    // AND state IN ('Entering', 'Active', 'Exiting')
    
    match self.core_position_repo
        .find_active_by_symbol_and_side(symbol, side)
        .await 
    {
        Ok(Some(_)) => true,   // Core is managing this
        _ => false,            // Safety Net should monitor
    }
}
```

**When to check:** Before adding position to Safety Net tracking map.

### Layer 2: Event Bus (Real-time)

Core Trading emits events when positions open/close. Safety Net subscribes and maintains an in-memory exclusion set:

```rust
pub enum DaemonEvent {
    // ... existing events
    CorePositionOpened {
        position_id: PositionId,
        symbol: Symbol,
        side: Side,
        binance_position_id: String,
    },
    CorePositionClosed {
        position_id: PositionId,
        symbol: Symbol,
        side: Side,
    },
}
```

Safety Net maintains:

```rust
struct PositionMonitor {
    // ...
    core_exclusion_set: RwLock<HashSet<(Symbol, Side)>>,
}
```

**When to check:** Before executing safety stop exit order (double-check).

### Layer 3: Position ID Linking (Forensic)

Add `binance_position_id` to Core Trading positions table for reconciliation:

```sql
ALTER TABLE positions 
ADD COLUMN binance_position_id VARCHAR(255);
```

When Core Trading places entry order and receives fill, store Binance's internal position ID. This enables:
- Post-mortem analysis: "Did Safety Net attempt to close a Core position?"
- Debugging: Link Core position to Binance position
- Future: Two-way lookup for advanced coordination

---

## Consequences

### Positive

1. **Zero Double Execution Risk**
   - Safety Net skips Core-managed positions
   - No race conditions between modalities
   - Safe to run both simultaneously

2. **Decoupled Architecture**
   - Core Trading unaware of Safety Net
   - Safety Net queries Core via repository interface
   - Single direction dependency (Safety → Core)

3. **Operational Simplicity**
   - Both modalities run in same daemon process
   - No external coordination service needed
   - No distributed locks or consensus

4. **Auditability**
   - Event log shows which system managed which position
   - `binance_position_id` enables forensic analysis
   - Clear separation in logs and metrics

5. **User Freedom**
   - Can use Core Trading (trailing stops)
   - Can open manual positions (safety stops)
   - Can mix both on different symbols
   - Safety Net always provides backup protection

### Negative / Trade-offs

1. **Safety Net Delay**
   - Database query adds latency (polling every 20s anyway)
   - Mitigated by: Query is fast (indexed, single table)

2. **Memory Overhead**
   - In-memory exclusion set per (symbol, side)
   - Mitigated by: HashSet is small (typical: 1-5 active positions)

3. **Code Coupling**
   - Safety Net depends on Core position schema
   - Mitigated by: Dependency injection via trait (`PositionRepository`)
   - Future: Can extract to shared service if needed

4. **Event Bus Dependency**
   - Safety Net must subscribe to Core events
   - Mitigated by: Event bus already exists for detector coordination
   - Same pattern, reused infrastructure

### Risks & Mitigations

| Risk | Impact | Mitigation |
|------|--------|------------|
| Query fails during check | Safety Net might monitor Core position | Fail-safe: Skip monitoring on error (log alert) |
| Event bus message lost | Exclusion set out of sync | Layer 1 (DB query) is primary, event bus is optimization |
| Core crashes before storing binance_position_id | Link broken | Not critical: Layer 1 and 2 still work |
| User opens manual position on same (symbol, side) as Core | Safety Net might skip | Core should check Binance before entry |

---

## Alternatives Considered

### Alternative 1: Global Lock (Rejected)

Place distributed lock on (symbol, side) to prevent both systems from operating:

**Why not chosen:**
- Requires external lock service (Redis, etcd) or database locks
- Deadlock risk if Core crashes holding lock
- Reduces Safety Net availability (can't operate if lock held)
- Over-engineered: problem is simpler than distributed coordination

### Alternative 2: Safety Net Disabled for Core Symbols (Rejected)

Configure Safety Net to only monitor specific symbols:

```rust
safety_net_symbols: ["ETHUSDT", "ADAUSDT"],  // NOT BTCUSDT
core_trading_symbols: ["BTCUSDT"],
```

**Why not chosen:**
- Manual configuration required
- Error-prone (user might forget to configure)
- No protection if user opens manual position on "core symbol"
- Doesn't scale: what if user wants both modalities on same symbol?

### Alternative 3: Tag Positions on Binance (Rejected)

Use Binance's `newClientOrderId` with prefix to identify Robson-created positions:

```rust
client_order_id: "robson_core_{ulid}"     // Core Trading
client_order_id: "robson_manual_{ulid}"   // Manual (but can't control)
```

**Why not chosen:**
- Cannot tag manual positions (user opens via Binance app)
- Requires querying order history to determine position source
- Expensive: N API calls for N positions
- Unreliable: User might cancel/replace orders

### Alternative 4: Safety Net Only (Rejected)

Remove Core Trading's trailing stop, rely solely on Safety Net:

**Why not chosen:**
- Loses trailing stop benefits (captures more profit)
- All positions get same treatment (fixed 2% stop)
- No differentiation between "Robson-managed" and "emergency backup"
- Core Trading value proposition lost

---

## Implementation Notes

### Phase 9 Changes

**Files Modified:**
- `v2/robsond/src/position_monitor.rs` - Add `is_core_managed()` filter
- `v2/robsond/src/event_bus.rs` - Add `CorePositionOpened` event
- `v2/robson-domain/src/entities.rs` - Add `binance_position_id` field
- `v2/robson-store/src/repository.rs` - Add `find_active_by_symbol_and_side()`
- `v2/migrations/003_add_binance_position_id.sql` - Schema update

**Files Created:**
- `v2/robsond/tests/core_safety_coordination_test.rs` - Integration tests

### Testing Strategy

**Unit Tests:**
```rust
#[test]
fn test_is_core_managed_returns_true_for_active_position()
#[test]
fn test_is_core_managed_returns_false_for_manual_position()
#[test]
fn test_exclusion_set_updated_on_core_position_opened_event()
```

**Integration Tests:**
```rust
#[tokio::test]
async fn test_safety_net_skips_core_position()
#[tokio::test]
async fn test_safety_net_monitors_manual_position()
#[tokio::test]
async fn test_both_modalities_different_symbols_no_conflict()
```

### Observability

**Logs:**
```
INFO  Safety Net: Skipping BTCUSDT Long (Core-managed position_id=...)
INFO  Safety Net: Monitoring ETHUSDT Short (manual position, safety stop=...)
```

**Metrics:**
```
safety_net_positions_skipped_total{reason="core_managed"}
safety_net_positions_monitored_total{symbol="ETHUSDT"}
```

### Related ADRs

- **ADR-0012 (Event Sourcing)**: Event Bus used for coordination
- **ADR-0013 (CLI-Daemon IPC)**: Unrelated, different concern

---

## References

- [v2/docs/DOMAIN.md](../DOMAIN.md) - Position entity, state machine
- [v2/docs/RELIABILITY.md](../RELIABILITY.md) - Failure modes, idempotency
- [v2/docs/EXECUTION-PLAN.md](../EXECUTION-PLAN.md) - Phase 9-10 roadmap
- [Binance API: Isolated Margin](https://binance-docs.github.io/apidocs/spot/en/#query-isolated-margin-account-info-user_data)

---

**Decision Date**: 2026-02-14
**Approved By**: Architecture Review
**Review Date**: 2026-08-14 (6 months after deployment)
