# Gate 4 Implementation: Operation Creation from TradingIntent

**Status**: ✅ Complete
**Date**: 2026-01-02
**Scope**: Minimal, domain-safe Operation creation from LIVE TradingIntent execution

---

## Summary

Gate 4 implements the **Operation (Level 2)** entity as the atomic market commitment record created from LIVE TradingIntent execution.

### What Changed

1. **Operation Model** - Added `trading_intent` OneToOneField (nullable, unique)
2. **ExecutionFramework** - LIVE mode now creates Operation + AuditTransaction after exchange confirms
3. **AuditService Integration** - Movements (L3) linked to Operations (L2)
4. **API Endpoints** - Read-only `/api/operations/` endpoints
5. **Tests** - Comprehensive test coverage for all invariants

### What Did NOT Change

- ❌ No WebSocket/polling for order status updates (Gate 5)
- ❌ No stop-loss automation (separate concern)
- ❌ No PnL tracking (computed on-demand)
- ❌ No frontend changes (backend-only gate)

---

## Architecture: Three-Level Hierarchy

```
LEVEL 1: STRATEGY → Trading methodology
   ↓
LEVEL 2: OPERATION → Trade lifecycle (entry → exit)
   ↓
LEVEL 3: MOVEMENT (AuditTransaction) → Atomic exchange action
```

### Entities

| Entity | Level | Purpose | Example |
|--------|-------|---------|---------|
| **Strategy** | L1 | Trading algorithm | "Mean Reversion MA99" |
| **Operation** | L2 | Trade lifecycle anchor | BUY 0.001 BTC, stop at $93k, active |
| **AuditTransaction** | L3 | Atomic exchange fact | SPOT_BUY, order_id=12345, filled |

---

## Causality Chain (LIVE Execution)

```
User clicks "Execute LIVE"
   ↓
POST /api/trading-intents/{id}/execute/?mode=live
   ↓
ExecutionFramework.execute(intent, mode="live")
   ↓
BinanceExecution.place_market(symbol, side, quantity)
   ↓
Exchange returns: {"orderId": "12345", "status": "FILLED", ...}
   ↓
🔒 ATOMIC TRANSACTION:
   │
   ├─> Create Operation (Level 2)
   │   - trading_intent = intent (OneToOne)
   │   - symbol, side, quantity = from intent
   │   - status = "ACTIVE" (immediately)
   │   - stop_price, target_price = from intent
   │
   ├─> Create AuditTransaction (Level 3)
   │   - transaction_type = SPOT_BUY/SPOT_SELL
   │   - binance_order_id = "12345" (proof!)
   │   - related_operation = operation (FK)
   │   - symbol, quantity, price, fee = from fills
   │
   └─> Update TradingIntent.execution_result
       - {"operation_id": <uuid>, "movement_id": <uuid>}
   ↓
TradingIntent.status → EXECUTED
```

---

## Invariants (Enforced)

### 1. LIVE-Only Creation
```python
# DRY-RUN: Never creates Operation
if mode == "dry-run":
    # Simulates, no real entities
    return ExecutionResult(mode=DRY_RUN, ...)

# LIVE: Creates Operation only after exchange confirms
elif mode == "live":
    order_response = binance.place_market(...)  # Exchange call
    exchange_order_id = order_response["orderId"]  # Proof of commitment
    operation = Operation.objects.create(...)  # Now create
```

### 2. Exchange Proof Required
```python
# Operation exists ONLY if exchange returned order_id
assert AuditTransaction.binance_order_id is not None
assert AuditTransaction.binance_order_id != ""
```

### 3. Idempotency
```python
# Check before creating
if intent.execution_result.get('operation_id'):
    existing_op = Operation.objects.get(id=operation_id)
    return (existing_op.id, ...)  # Reuse existing

# Otherwise, create new
operation = Operation.objects.create(...)
```

### 4. Plan-Reality Consistency
```python
# Operation fields MUST match TradingIntent
assert operation.symbol == intent.symbol
assert operation.side == intent.side
assert operation.stop_price == intent.stop_price
assert operation.strategy == intent.strategy
```

### 5. Status Rule (Gate 4)
```python
# Operation created with status=ACTIVE (no pending/created state)
# Rationale: No WS/polling in Gate 4, so immediate ACTIVE
operation = Operation.objects.create(
    ...,
    status="ACTIVE"  # Gate 4 rule
)
```

### 6. One-to-One Relationship
```python
# Database constraint: trading_intent OneToOneField
# One LIVE TradingIntent → One Operation (max)
class Operation:
    trading_intent = models.OneToOneField(
        TradingIntent,
        unique=True,  # DB-level enforcement
        null=True,  # Operation can exist without intent (manual flow)
        ...
    )
```

### 7. Atomic Transaction
```python
# Operation + AuditTransaction created in single DB transaction
with transaction.atomic():
    operation = Operation.objects.create(...)
    movement = audit_service._create_transaction(...)
    movement.related_operation = operation
    movement.save()
```

---

## Implementation Details

### Schema Changes

**Migration**: `0028_link_operation_to_tradingintent.py`

```python
# Added to Operation model
trading_intent = models.OneToOneField(
    'TradingIntent',
    on_delete=models.SET_NULL,
    null=True,
    blank=True,
    related_name='operation',
    unique=True,
    help_text='TradingIntent that created this operation (agentic workflow only)'
)

# Added indexes
models.Index(fields=['client', 'status', 'created_at'], name='operation_portfolio_idx')
models.Index(fields=['trading_intent'], name='operation_intent_idx')
```

### Code Changes

1. **api/models/trading.py** - Added `trading_intent` field to Operation
2. **api/application/execution_framework.py** - Implemented `_execute_live()` method
3. **api/views/operation_views.py** - Created read-only API endpoints
4. **api/serializers/operation_serializers.py** - Created OperationSerializer
5. **api/main_urls.py** - Registered `/api/operations/` routes
6. **api/tests/test_operation_creation.py** - Comprehensive invariant tests

### API Endpoints

#### GET /api/operations/

List operations for authenticated user.

**Query Parameters**:
- `status` (str, optional): Filter by status (ACTIVE, CLOSED, etc.)
- `strategy` (int, optional): Filter by strategy ID
- `symbol` (int, optional): Filter by symbol ID
- `limit` (int, default=100): Maximum results
- `offset` (int, default=0): Pagination offset

**Response**:
```json
{
  "count": 2,
  "results": [
    {
      "id": 42,
      "trading_intent_id": "uuid-here",
      "strategy_name": "Mean Reversion MA99",
      "symbol_name": "BTCUSDC",
      "side": "BUY",
      "status": "ACTIVE",
      "stop_price": "93000.00",
      "target_price": "98000.00",
      "created_at": "2026-01-02T10:00:00Z",
      "movements_count": 1,
      "total_entry_quantity": "0.001",
      "average_entry_price": "95000.00"
    }
  ]
}
```

#### GET /api/operations/{id}/

Retrieve single operation with details.

---

## Testing

**Test File**: `api/tests/test_operation_creation.py`

### Test Coverage

| Invariant | Test | Status |
|-----------|------|--------|
| DRY-RUN never creates Operation | `test_dry_run_never_creates_operation` | ✅ Pass |
| LIVE creates Operation + Movement | `test_live_success_creates_operation_and_movement` | ✅ Pass |
| Idempotency (double execute) | `test_idempotency_double_execute_creates_single_operation` | ✅ Pass |
| Plan-reality consistency | `test_plan_reality_consistency` | ✅ Pass |
| Exchange proof required | `test_exchange_proof_required` | ✅ Pass |
| LIVE failure before exchange | `test_live_failure_before_exchange_no_operation` | ✅ Pass |
| Status immediately ACTIVE | `test_operation_status_immediately_active` | ✅ Pass |

### Running Tests

```bash
DJANGO_SETTINGS_MODULE=backend.settings \
PYTHONPATH=/home/psyctl/apps/robson/apps/backend/monolith \
.venv/bin/pytest api/tests/test_operation_creation.py -v
```

---

## Runtime Behavior

### DRY-RUN Execution

```
User executes DRY-RUN:
   ├─> Simulates order placement
   ├─> NO Operation created ✅
   ├─> NO AuditTransaction created ✅
   └─> TradingIntent.execution_result = {"simulated": true}
```

### LIVE Execution (Success)

```
User executes LIVE:
   ├─> Calls Binance API
   ├─> Exchange returns order_id="12345" ✅
   ├─> Creates Operation (status=ACTIVE) ✅
   ├─> Creates AuditTransaction (binance_order_id="12345") ✅
   ├─> Links Movement to Operation ✅
   └─> Updates TradingIntent.execution_result
```

### LIVE Execution (Failure Before Exchange)

```
User executes LIVE:
   ├─> Validation fails (insufficient balance)
   ├─> Exchange never called ✅
   ├─> NO Operation created ✅
   ├─> NO AuditTransaction created ✅
   └─> Returns error to user
```

### LIVE Execution (Idempotent Retry)

```
User double-clicks "Execute LIVE":
   ├─> 1st click: Creates Operation ✅
   ├─> 2nd click: Detects existing operation_id ✅
   ├─> Returns existing Operation (no duplicate) ✅
   └─> Binance only called once ✅
```

---

## Future Work (Out of Scope)

### Gate 5: Operation Lifecycle Updates
- WebSocket listeners for order status changes
- Poll Binance for order updates
- Transition ACTIVE → FILLED → CLOSED
- Update Operation.status based on exchange events

### Gate 6: Stop-Loss Automation
- Monitor Operation.stop_price
- Trigger exit order when price hits stop
- Link exit movement to Operation.exit_orders

### Gate 7: PnL Calculation
- Compute realized PnL from entry/exit movements
- Track unrealized PnL for active operations
- Portfolio-level P&L aggregation

---

## ADR References

- **ADR-0012**: Operation Domain Model and Creation Causality (Gate 3 design)
- **Transaction Hierarchy**: `docs/architecture/TRANSACTION-HIERARCHY.md`
- **Semantic Clarity**: `docs/requirements/STRATEGY-SEMANTIC-CLARITY.md`

---

## Verification Checklist

- [x] Operation model has `trading_intent` OneToOneField
- [x] Migration applied successfully
- [x] LIVE execution creates Operation after exchange confirm
- [x] DRY-RUN never creates Operation
- [x] Idempotency enforced (double-execute safe)
- [x] AuditTransaction linked to Operation via `related_operation`
- [x] Operation status = ACTIVE immediately
- [x] API endpoints return Operations filtered by user
- [x] All tests pass
- [x] No frontend changes (backend-only)
- [x] No WebSocket/polling (deferred to Gate 5)

---

**Gate 4 Implementation Complete** ✅

**Runtime behavior**: Operation creation is gated until LIVE returns exchange_order_id.
