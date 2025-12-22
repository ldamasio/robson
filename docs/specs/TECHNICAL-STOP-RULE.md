# Technical Stop Rule - Robson Bot

## Business Rule: Technical Stop Calculation

**Critical Rule**: The technical stop MUST be calculated BEFORE position entry, and the position size MUST be calculated based on the technical stop distance.

## Stop Calculation Method

### For LONG Positions (BUY)
- **Technical Stop**: Second technical support to the left on 15-minute chart
- **Rationale**: Protection against downtrend breakout (if price breaks first support and second support, trend is broken)

### For SHORT Positions (SELL)
- **Technical Stop**: Second technical resistance to the left on 15-minute chart
- **Rationale**: Protection against uptrend breakout

## Workflow

```
1. User selects strategy and symbol
   ↓
2. System analyzes 15-min chart (last 100-200 candles)
   ↓
3. System identifies technical events (supports/resistances)
   ↓
4. System selects SECOND event to the left (from current price)
   ↓
5. System calculates stop distance (entry_price - stop_price)
   ↓
6. System calculates position size:
   - Max loss = 1% of total capital
   - Position size = (capital × 0.01) / stop_distance
   ↓
7. System presents to user:
   - Entry price: $X
   - Technical stop: $Y (distance: Z%)
   - Position size: N units ($M value)
   - Max loss: $L (1% of capital)
   ↓
8. User confirms or adjusts
   ↓
9. System executes entry with stop already defined
```

## Example (BTC Long)

```
Capital: $10,000
Current BTC price: $90,000

Technical Analysis (15min chart):
- First support: $88,500
- Second support: $87,000 ← TECHNICAL STOP

Stop calculation:
- Entry: $90,000
- Stop: $87,000
- Stop distance: $3,000 (3.33%)

Position size calculation:
- Max loss: $10,000 × 1% = $100
- Position size: $100 / $3,000 = 0.0333 BTC
- Position value: $90,000 × 0.0333 = $3,000

If stop is hit:
- Loss: 0.0333 BTC × $3,000 = $99.90 ≈ 1% capital ✓
```

## Technical Event Detection

### Support Definition
A support is identified when:
1. Price touches a level and bounces UP
2. Multiple touches at same level increase support strength
3. Higher volume at support increases reliability

### Resistance Definition
A resistance is identified when:
1. Price touches a level and bounces DOWN
2. Multiple touches at same level increase resistance strength
3. Higher volume at resistance increases reliability

### Algorithm (Simplified)

```python
def find_second_support(candles_15min, current_price, side="LONG"):
    """
    Find second technical support/resistance for stop placement.

    Args:
        candles_15min: List of OHLCV candles (100-200 candles)
        current_price: Current market price
        side: "LONG" or "SHORT"

    Returns:
        stop_price: Price level for technical stop
        stop_distance_percent: Distance in percentage
    """
    if side == "LONG":
        # Find supports (local minima where price bounced up)
        supports = identify_supports(candles_15min)

        # Filter supports below current price
        valid_supports = [s for s in supports if s < current_price]

        # Sort by distance from current price (closest first)
        valid_supports.sort(reverse=True)

        # Select SECOND support (index 1)
        if len(valid_supports) >= 2:
            return valid_supports[1]
        else:
            # Fallback: use default 2% stop if insufficient data
            return current_price * 0.98

    else:  # SHORT
        # Find resistances (local maxima where price bounced down)
        resistances = identify_resistances(candles_15min)

        # Filter resistances above current price
        valid_resistances = [r for r in resistances if r > current_price]

        # Sort by distance from current price (closest first)
        valid_resistances.sort()

        # Select SECOND resistance (index 1)
        if len(valid_resistances) >= 2:
            return valid_resistances[1]
        else:
            # Fallback: use default 2% stop if insufficient data
            return current_price * 1.02
```

## Implementation Strategy

### Phase 1: Technical Analysis Module
- [ ] Create `apps/backend/monolith/api/application/technical_analysis.py`
- [ ] Implement support/resistance detection
- [ ] Add tests with real market data

### Phase 2: Stop Calculator
- [ ] Create `calculate_technical_stop` use case
- [ ] Integrate with Binance klines API (15min candles)
- [ ] Return stop price + visualization data for frontend

### Phase 3: Updated Position Size Calculator
- [ ] Modify `PositionSizeCalculator` to accept technical stop as input
- [ ] Remove default 2% stop (must use technical stop)

### Phase 4: API Endpoints
- [ ] `POST /api/trade/calculate-entry/` - Calculate stop + position size
- [ ] `GET /api/trade/chart-analysis/{symbol}/` - Get support/resistance levels

### Phase 5: CLI Integration
- [ ] Update `robson plan buy BTCUSDT 0.001` to:
  - Fetch 15min candles
  - Calculate technical stop
  - Calculate position size
  - Show analysis to user

### Phase 6: Frontend
- [ ] Chart view showing supports/resistances
- [ ] Entry form with calculated stop and position size
- [ ] Visual representation of risk (1% rule)

## Risk Management Rules (Summary)

1. ✅ **1% Risk Rule**: Never risk more than 1% of capital per trade
2. ✅ **Technical Stop Rule**: Stop based on second technical event (15min chart)
3. ✅ **Position Size Rule**: Size calculated from stop distance, not arbitrary
4. ⏳ **Pre-Entry Validation**: Stop MUST be defined before entry
5. ⏳ **No Manual Stops**: Stops are calculated, not guessed

## Applies To

- ✅ Spot trading (current)
- ✅ Margin Isolated (future implementation)
- ✅ All strategies (manual, algorithmic, signals)

## Notes

- First operation (BTC 0.00033 @ $88,837) was exceptional (bootstrap)
- ALL future operations MUST follow this rule
- This is a CORE business rule, not optional
- Violations should be blocked by the system

---

**Status**: Pending implementation
**Priority**: HIGH - Blocks new trading operations
**Owner**: Development team
**Related**: ADR-0007 (to be created)
