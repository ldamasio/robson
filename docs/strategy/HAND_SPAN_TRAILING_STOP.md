# Hand-Span Trailing Stop

**Version:** 1.0
**Date:** 2024-12-28
**Status:** Implemented

## Overview

The Hand-Span Trailing Stop is a discrete trailing-stop system that automatically adjusts stop-loss levels as a position moves into profit. The "hand-span" (palmo da mão) is the distance between the entry price and the initial technical stop-loss.

**Key Properties:**

- **Discrete**: Adjusts in fixed steps (not continuous)
- **Deterministic**: Same inputs always produce same outputs
- **Idempotent**: Multiple applications don't create duplicates
- **Monotonic**: Stop never loosens (only tightens or stays same)
- **Auditable**: Every adjustment is logged with full context

## Concept

### The "Span"

The **span** is the initial risk distance:

```
span = |entry_price - initial_technical_stop|
```

Example:

- Entry: $50,000
- Technical Stop: $49,000
- Span: $1,000 (one "hand-span")

### Adjustment Rules

The stop adjusts at discrete thresholds:

| Profit Distance        | Spans Crossed | Action                     | New Stop Location                  |
| ---------------------- | ------------- | -------------------------- | ---------------------------------- |
| < 1 span               | 0             | No adjustment              | Keep initial stop                  |
| 1 span (± fees)        | 1             | Move to break-even         | Entry + fees/slippage              |
| 2 spans                | 2             | Trail by 1 span            | Entry + 1 span                     |
| 3 spans                | 3             | Trail by 2 spans           | Entry + 2 spans                    |
| N spans (N ≥ 2)        | N             | Trail by (N-1) spans       | Entry + (N-1) × span               |

## Algorithm

### For LONG Positions

```python
profit_distance = current_price - entry_price
spans_crossed = floor(profit_distance / span)

if spans_crossed == 0:
    new_stop = current_stop  # No change

elif spans_crossed == 1:
    # Move to break-even (accounting for fees + slippage)
    new_stop = entry_price * (1 + fee_percent + slippage_percent)

else:  # spans_crossed >= 2
    # Trail by (N-1) spans
    new_stop = entry_price + ((spans_crossed - 1) * span)

# CRITICAL: Enforce monotonic property
new_stop = max(current_stop, new_stop)  # Never decrease
```

### For SHORT Positions

```python
profit_distance = entry_price - current_price
spans_crossed = floor(profit_distance / span)

if spans_crossed == 0:
    new_stop = current_stop  # No change

elif spans_crossed == 1:
    # Move to break-even (accounting for fees + slippage)
    new_stop = entry_price / (1 + fee_percent + slippage_percent)

else:  # spans_crossed >= 2
    # Trail by (N-1) spans
    new_stop = entry_price - ((spans_crossed - 1) * span)

# CRITICAL: Enforce monotonic property
new_stop = min(current_stop, new_stop)  # Never increase
```

## Examples

### Example 1: LONG Position - Progressive Trailing

**Setup:**

- Entry: $50,000
- Initial Stop: $49,000
- Span: $1,000
- Fee Config: 0.1% trading + 0.05% slippage = 0.15% total

**Scenario:**

| Current Price | Profit | Spans | New Stop    | Reason        | Notes                            |
| ------------- | ------ | ----- | ----------- | ------------- | -------------------------------- |
| $49,500       | -$500  | 0     | $49,000     | No adjustment | Still at loss                    |
| $50,000       | $0     | 0     | $49,000     | No adjustment | At entry                         |
| $50,500       | +$500  | 0     | $49,000     | No adjustment | Profit < 1 span                  |
| $51,000       | +$1000 | 1     | **$50,075** | Break-even    | Entry + 0.15% = $50,075          |
| $51,500       | +$1500 | 1     | $50,075     | No adjustment | Still in first span              |
| $52,000       | +$2000 | 2     | **$51,000** | Trailing      | Entry + 1 span = $51,000         |
| $53,000       | +$3000 | 3     | **$52,000** | Trailing      | Entry + 2 spans = $52,000        |
| $54,000       | +$4000 | 4     | **$53,000** | Trailing      | Entry + 3 spans = $53,000        |

**Analysis:**

- First span: Moves to break-even (protects against fees)
- Each subsequent span: Trails by one additional span
- Stop is now a "stop-gain" ($53,000 stop on $50,000 entry = $3,000 locked profit)

### Example 2: SHORT Position - Progressive Trailing

**Setup:**

- Entry: $3,000
- Initial Stop: $3,100
- Span: $100
- Fee Config: 0.15% total

**Scenario:**

| Current Price | Profit | Spans | New Stop    | Reason        | Notes                            |
| ------------- | ------ | ----- | ----------- | ------------- | -------------------------------- |
| $3,050        | -$50   | 0     | $3,100      | No adjustment | Still at loss                    |
| $3,000        | $0     | 0     | $3,100      | No adjustment | At entry                         |
| $2,950        | +$50   | 0     | $3,100      | No adjustment | Profit < 1 span                  |
| $2,900        | +$100  | 1     | **$2,995**  | Break-even    | Entry / 1.0015 ≈ $2,995          |
| $2,850        | +$150  | 1     | $2,995      | No adjustment | Still in first span              |
| $2,800        | +$200  | 2     | **$2,900**  | Trailing      | Entry - 1 span = $2,900          |
| $2,700        | +$300  | 3     | **$2,800**  | Trailing      | Entry - 2 spans = $2,800         |

**Analysis:**

- Short positions move stop DOWN as profit increases
- Same logic as LONG, just inverted direction
- Protects profit while allowing room for position to breathe

### Example 3: Monotonic Property (Stop Never Loosens)

**Setup:**

- Entry: $50,000
- Initial Stop: $49,000
- Current Stop: $51,000 (already at 2 spans)
- Span: $1,000

**Scenario: Price Retraces**

| Current Price | Spans | Calculated Stop | Actual New Stop | Reason                         |
| ------------- | ----- | --------------- | --------------- | ------------------------------ |
| $52,000       | 2     | $51,000         | $51,000         | No change                      |
| $51,500       | 1     | $50,075         | **$51,000**     | Monotonic: max(51000, 50075)   |
| $51,000       | 1     | $50,075         | **$51,000**     | Monotonic: stop never loosens  |
| $50,500       | 0     | $49,000         | **$51,000**     | Monotonic: stop stays at best  |

**Critical Insight:**

Once the stop has moved to $51,000, it NEVER moves back down, even if price retraces. This is the **monotonic guarantee** - stop only tightens or stays the same, never loosens.

## Edge Cases

### Edge Case 1: Exact Span Boundary

**Question:** What happens if price is exactly at 1 span?

**Answer:** Uses integer floor division:

```python
spans_crossed = int(profit_distance / span)
```

- Price at $51,000 with span $1,000 → `spans_crossed = 1` ✅
- Price at $50,999 with span $1,000 → `spans_crossed = 0` ❌

The position must FULLY cross the span threshold.

### Edge Case 2: Very Small Spans

**Question:** What if the span is very small (e.g., $0.50)?

**Answer:** Algorithm works the same:

- Entry: $50,000.00
- Initial Stop: $49,999.50
- Span: $0.50
- At $50,001.00 → 2 spans crossed → stop moves to $50,000.50

This works correctly with Decimal precision (8 decimal places).

### Edge Case 3: Position Opened with Trailing Stop Already Above Entry

**Question:** What if user manually sets current_stop above entry?

**Answer:**

- For LONG: Allowed (this is a stop-gain)
- For SHORT: Allowed (this is a stop-gain)
- Monotonic property still applies - stop won't loosen

### Edge Case 4: Zero Profit (Price at Entry)

**Answer:**

```python
spans_crossed = 0
reason = NO_ADJUSTMENT
new_stop = current_stop  # No change
```

### Edge Case 5: Negative Profit (Loss)

**Answer:**

```python
if profit_distance <= 0:
    spans_crossed = 0
```

Stop never adjusts when position is at a loss.

## Fee Configuration

### Default Configuration

```python
trading_fee_percent = 0.1%      # 0.1% maker/taker fee
slippage_buffer_percent = 0.05% # 0.05% slippage buffer
total_cost = 0.15%              # Combined
```

### Break-Even Calculation

**LONG:**

```python
break_even = entry_price * (1 + total_cost_percent / 100)
          = entry_price * 1.0015
```

**SHORT:**

```python
break_even = entry_price / (1 + total_cost_percent / 100)
          = entry_price / 1.0015
```

### Customization

Fees can be configured per tenant/strategy:

```python
custom_config = FeeConfig(
    trading_fee_percent=Decimal("0.2"),  # Higher fee
    slippage_buffer_percent=Decimal("0.1"),  # More buffer
)

calculator = HandSpanCalculator(fee_config=custom_config)
```

## Integration

### Architecture

The module follows hexagonal architecture:

```
trailing_stop/
├── domain.py          # Pure entities (TrailingStopState, StopAdjustment)
├── calculator.py      # Pure functions (HandSpanCalculator)
├── ports.py           # Interfaces (PriceProvider, Repository, etc.)
├── use_cases.py       # Business logic (AdjustTrailingStopUseCase)
└── adapters.py        # Django implementations
```

### Usage in Code

```python
from api.application.trailing_stop import (
    HandSpanCalculator,
    AdjustTrailingStopUseCase,
)
from api.application.trailing_stop.adapters import (
    BinancePriceProvider,
    DjangoTrailingStopRepository,
)

# Setup
calculator = HandSpanCalculator()
price_provider = BinancePriceProvider()
repository = DjangoTrailingStopRepository()

use_case = AdjustTrailingStopUseCase(
    calculator=calculator,
    price_provider=price_provider,
    repository=repository,
)

# Execute
result = use_case.execute(position_id="123")

if result.adjusted:
    print(f"Stop adjusted: {result.adjustment.old_stop} → {result.adjustment.new_stop}")
else:
    print(f"No adjustment: {result.error or 'no profit yet'}")
```

### Management Command

```bash
# Adjust trailing stops for all eligible positions
python manage.py adjust_trailing_stops

# Adjust for specific client
python manage.py adjust_trailing_stops --client-id 1

# Dry-run mode (no changes)
python manage.py adjust_trailing_stops --dry-run

# Verbose output
python manage.py adjust_trailing_stops -v 2
```

### Automation

**Periodic Job (Recommended):**

```yaml
# Kubernetes CronJob (every minute)
apiVersion: batch/v1
kind: CronJob
metadata:
  name: trailing-stop-adjuster
spec:
  schedule: "* * * * *"
  jobTemplate:
    spec:
      template:
        spec:
          containers:
            - name: adjuster
              image: robson-backend:latest
              command:
                - python
                - manage.py
                - adjust_trailing_stops
```

**WebSocket Handler (Real-time):**

```python
# In WebSocket price update handler
async def on_price_update(symbol: str, price: Decimal):
    # Get positions for this symbol
    positions = get_positions_for_symbol(symbol)

    for position in positions:
        result = adjust_use_case.execute(position.id)
        if result.adjusted:
            logger.info(f"Real-time adjustment: {result.adjustment}")
```

## Audit Trail

Every adjustment is logged to `AuditTransaction`:

```json
{
  "transaction_id": "uuid",
  "transaction_type": "STOP_LOSS_PLACED",
  "status": "FILLED",
  "symbol": "BTCUSDT",
  "price": 51000.0,
  "description": "Trailing stop adjusted (TRAILING): 49000 → 51000 [LONG, entry=50000, span=1000, step=2]",
  "source": "trailing_stop",
  "raw_response": {
    "adjustment_token": "123:adjust:1234567890",
    "old_stop": "49000",
    "new_stop": "51000",
    "reason": "TRAILING",
    "spans_crossed": 2,
    "step_index": 2,
    "metadata": {
      "symbol": "BTCUSDT",
      "side": "LONG",
      "entry_price": "50000",
      "span": "1000",
      "fee_config": {
        "trading_fee_percent": "0.1",
        "slippage_buffer_percent": "0.05"
      }
    }
  }
}
```

## Idempotency

Adjustments are idempotent via **adjustment tokens**:

```python
adjustment_token = f"{position_id}:adjust:{timestamp_ms}"
```

**Guarantees:**

1. Same token cannot be saved twice (checked before persistence)
2. Multiple calls with same state produce same adjustment
3. Network retries don't create duplicate adjustments

## Testing

### Unit Tests

```bash
# Run trailing stop tests
pytest apps/backend/monolith/api/tests/test_trailing_stop.py -v

# Run specific test class
pytest apps/backend/monolith/api/tests/test_trailing_stop.py::TestHandSpanCalculator -v

# Run property tests (monotonic guarantee)
pytest apps/backend/monolith/api/tests/test_trailing_stop.py::TestMonotonicProperty -v
```

### Test Coverage

- ✅ Domain entity validation (LONG/SHORT, prices, quantities)
- ✅ Calculator logic (break-even, trailing, no adjustment)
- ✅ Monotonic property (stop never loosens)
- ✅ Edge cases (exact boundaries, small spans, zero profit)
- ✅ Fee configuration (default, custom, break-even calculation)
- ✅ Serialization (to_dict for audit trail)

## Limitations

### Current Limitations

1. **No Partial Position Support**: Assumes full position size
2. **Single Stop per Position**: Cannot have multiple trailing stops for same position
3. **No Time-Based Decay**: Stop doesn't tighten based on time
4. **Fixed Span**: Span is constant (doesn't compress over time)

### Future Enhancements

1. **Adaptive Span**: Compress span based on volatility or time held
2. **Partial Trailing**: Trail only a portion of the position
3. **Time-Based Adjustments**: Tighten stop after X hours
4. **Volatility-Aware**: Adjust span based on ATR or Bollinger Bands
5. **Multiple Thresholds**: Different rules for different profit levels

## FAQ

### Q: What happens if I manually change the stop while trailing is active?

**A:** The system will respect the manual change if it's tighter than the trailing stop. However, on the next adjustment check:

- If your manual stop is tighter → trailing stop respects it (monotonic property)
- If your manual stop is looser → trailing stop overrides it with calculated level

### Q: Can I disable trailing stop for specific positions?

**A:** Yes, simply remove the position from the eligible filter, or set a flag in the position metadata.

### Q: Does this work with margin positions?

**A:** Yes, works with both `Operation` (spot) and `MarginPosition` (margin). The adapter detects which model to use.

### Q: What if price gaps through multiple spans instantly?

**A:** The algorithm handles this correctly:

- Price jumps from $50k to $54k (4 spans)
- System calculates: `spans_crossed = 4`
- Stop moves directly to: `entry + 3 spans = $53,000`
- No intermediate steps required

### Q: How often should I run the adjustment check?

**A:** Recommended frequencies:

- **Real-time (WebSocket)**: Best for active trading
- **Every minute (CronJob)**: Good for most cases
- **Every 5 minutes**: Acceptable for less active trading

### Q: Can this be used with systematic/algorithmic trading?

**A:** Yes! The module is designed to work with both:

- User-initiated operations (manual trades)
- Systematic intents (algorithmic signals)

Just ensure the position has `initial_stop` and `current_stop` fields populated.

## References

- **Code**: `apps/backend/monolith/api/application/trailing_stop/`
- **Tests**: `apps/backend/monolith/api/tests/test_trailing_stop.py`
- **ADR-0012**: Event-Sourced Stop-Loss Monitor
- **POSITION-SIZING-GOLDEN-RULE**: Position sizing principles

## Version History

- **v1.0** (2024-12-28): Initial implementation
  - Core algorithm (break-even + trailing)
  - Monotonic guarantee
  - Django integration
  - Comprehensive tests
  - Audit trail
