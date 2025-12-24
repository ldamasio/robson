# Technical Stop-Loss Requirements

## REQ-CORE-TECHSTOP-001: Technical Stop-Loss Calculator

### Summary
Position sizing MUST be calculated based on **technical stop levels** derived from chart analysis, NOT arbitrary percentage-based stops.

### Business Rationale
- A 1% percentage stop is arbitrary and may not respect market structure
- Professional traders place stops at **technical invalidation levels**
- The stop should be where "the trade thesis is proven wrong"
- Position size is then calculated backwards from the technical stop

### Technical Stop Definition
A **technical stop** is placed at a price level where:
1. The trade setup is invalidated
2. Key support/resistance is broken
3. The market structure changes

### Detection Methods (Priority Order)

#### 1. Support/Resistance Levels (Primary)
- **N-th Support Level**: Find the Nth significant support below current price
- For LONG: Stop below the 2nd support level on the timeframe
- For SHORT: Stop above the 2nd resistance level on the timeframe

#### 2. Swing Points
- **Recent Swing Low** (for LONG): Stop below the most recent swing low
- **Recent Swing High** (for SHORT): Stop above the most recent swing high

#### 3. Moving Average Zones
- Stop below key moving averages (EMA 21, EMA 50, EMA 200)

#### 4. ATR-Based (Fallback)
- Stop at N × ATR below entry (typically 1.5-2x ATR)
- Only used when no clear technical level exists

---

## REQ-CORE-TECHSTOP-002: Position Sizing from Technical Stop

### Formula
```
Technical Stop Distance = |Entry Price - Technical Stop Price|
Max Risk Amount = Capital × 1%
Position Size = Max Risk Amount / Technical Stop Distance
```

### Example
```
Entry Price: $95,000
Technical Stop (2nd Support): $93,500
Stop Distance: $1,500

Capital: $10,000
Max Risk (1%): $100

Position Size = $100 / $1,500 = 0.0667 BTC
Position Value = 0.0667 × $95,000 = $6,333.33
```

### Key Insight
- Wider technical stop → Smaller position size
- Tighter technical stop → Larger position size
- Risk amount stays constant at 1%

---

## REQ-CORE-TECHSTOP-003: Integration Flow

### Trading Flow with Technical Stop

```
1. USER identifies trade opportunity
         │
         ▼
2. ROBSON analyzes chart for technical levels
   - Fetches OHLCV data (15m timeframe default)
   - Identifies support/resistance levels
   - Calculates swing points
   - Determines technical stop price
         │
         ▼
3. ROBSON calculates position size
   - Uses technical stop distance
   - Applies 1% risk rule
   - Returns safe quantity
         │
         ▼
4. RISK GUARDS validate
   - RiskManagementGuard (with technical stop)
   - MonthlyDrawdownGuard
         │
         ▼
5. USER confirms and executes
```

---

## REQ-CORE-TECHSTOP-004: Technical Analysis Parameters

### Default Configuration
| Parameter | Default | Description |
|-----------|---------|-------------|
| `timeframe` | 15m | Chart timeframe for analysis |
| `lookback_periods` | 100 | Number of candles to analyze |
| `support_level_n` | 2 | Which support level to use (2nd) |
| `min_touches` | 2 | Minimum touches to confirm level |
| `level_tolerance` | 0.5% | Price tolerance for level detection |
| `atr_period` | 14 | ATR calculation period |
| `atr_multiplier` | 1.5 | ATR multiplier for fallback stop |

### Customizable per Strategy
Different strategies may use different technical stop parameters.

---

## REQ-CORE-TECHSTOP-005: API Contract

### Input
```python
TechnicalStopRequest:
    symbol: str          # e.g., "BTCUSDC"
    side: str            # "BUY" or "SELL"
    entry_price: Decimal # Intended entry price
    timeframe: str       # "15m", "1h", "4h"
    method: str          # "support", "swing", "atr"
    level_n: int         # Which level (1st, 2nd, 3rd support)
```

### Output
```python
TechnicalStopResult:
    stop_price: Decimal       # Calculated technical stop
    stop_distance: Decimal    # Distance from entry
    stop_distance_pct: Decimal # As percentage
    method_used: str          # Which method found the level
    confidence: str           # "high", "medium", "low"
    levels_found: List[Level] # All detected levels
    chart_context: dict       # Additional chart info
```

---

## REQ-CORE-TECHSTOP-006: Safety Rules

### Mandatory Checks
1. Technical stop MUST be on the correct side of entry
   - For LONG: stop < entry
   - For SHORT: stop > entry

2. Stop distance MUST be reasonable
   - Minimum: 0.1% from entry
   - Maximum: 10% from entry (configurable)

3. If no technical level found:
   - Fall back to ATR-based stop
   - Log warning for manual review

4. Position size MUST NOT exceed max_position_percent (50% default)

---

## REQ-CORE-TECHSTOP-007: Audit Trail

All technical stop calculations MUST be logged:
- Timestamp
- Symbol and timeframe
- Entry price
- Detected levels (all)
- Selected stop level
- Method used
- Confidence score
- Resulting position size

---

## Implementation Priority

1. **Phase 1**: Support/Resistance detection (swing points)
2. **Phase 2**: ATR-based fallback
3. **Phase 3**: Advanced pattern recognition
4. **Phase 4**: ML-enhanced level detection

## Dependencies
- Historical OHLCV data from Binance
- pandas/numpy for calculations
- Optional: TA-Lib for advanced indicators

