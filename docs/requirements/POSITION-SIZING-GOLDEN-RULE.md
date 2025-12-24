# POSITION SIZING GOLDEN RULE

## ⚠️ CRITICAL: This is the CORE principle of Robson's Risk Management

---

## THE GOLDEN RULE

> **Position Size is DERIVED from the Technical Stop.**
> **Never the other way around.**

---

## THE FORMULA

```
┌─────────────────────────────────────────────────────────────────┐
│                                                                 │
│    Position Size = Max Risk Amount / Technical Stop Distance    │
│                                                                 │
│    Where:                                                       │
│    • Max Risk Amount = Capital × 1%                             │
│    • Technical Stop Distance = |Entry Price - Technical Stop|   │
│    • Technical Stop = 2nd Support Level (from chart analysis)   │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘
```

---

## THE CORRECT ORDER OF OPERATIONS

```
Step 1: ANALYZE CHART
        │
        └── Find swing lows (support levels)
        └── Identify the Nth support (default: 2nd)
        └── This becomes the TECHNICAL STOP
        │
        ▼
Step 2: CALCULATE STOP DISTANCE
        │
        └── Stop Distance = |Entry - Technical Stop|
        └── This is in PRICE UNITS (e.g., $950)
        │
        ▼
Step 3: DETERMINE MAX RISK AMOUNT
        │
        └── Max Risk = Capital × 1%
        └── This is the MAX you can lose (e.g., $100)
        │
        ▼
Step 4: DERIVE POSITION SIZE
        │
        └── Position Size = Max Risk / Stop Distance
        └── This ensures you lose EXACTLY 1% if stopped
        │
        ▼
Step 5: EXECUTE ORDER
        └── Buy the calculated quantity
        └── Place stop-loss at Technical Stop
```

---

## EXAMPLE

```
GIVEN:
  Capital         = $10,000
  Entry Price     = $95,000
  Technical Stop  = $93,500 (2nd support on 15m chart)

CALCULATE:
  Stop Distance   = |$95,000 - $93,500| = $1,500
  Max Risk (1%)   = $10,000 × 1% = $100
  Position Size   = $100 / $1,500 = 0.0667 BTC
  Position Value  = 0.0667 × $95,000 = $6,333.33

RESULT:
  • You buy 0.0667 BTC for $6,333.33
  • If stopped at $93,500, you lose $100 (exactly 1%)
  • Position is sized FOR the stop, not despite it
```

---

## WHY THIS ORDER MATTERS

### ❌ WRONG: "I want to buy $5,000 of BTC, where should I put my stop?"

This is backwards. You're choosing the position first and forcing a stop.

### ✅ RIGHT: "My technical stop is at $93,500. How much can I buy?"

This is correct. The stop drives the position size.

---

## KEY INSIGHTS

### 1. Wide Stop = Smaller Position
If the 2nd support is far away:
- Stop Distance is large
- Position Size is small
- Risk is still 1%

### 2. Tight Stop = Larger Position
If the 2nd support is close:
- Stop Distance is small
- Position Size is larger
- Risk is still 1%

### 3. Risk is CONSTANT
No matter the stop distance:
- You always risk exactly 1% of capital
- The position size adjusts to match

---

## FOR AI AGENTS

When implementing or explaining trading in Robson:

1. **NEVER** ask "how much do you want to invest?"
2. **ALWAYS** ask "where is your technical invalidation level?"
3. The investment amount is CALCULATED, not chosen
4. The stop-loss is TECHNICAL, not arbitrary

### Code Pattern

```python
# CORRECT
technical_stop = analyze_chart_for_support(symbol, timeframe)
stop_distance = abs(entry_price - technical_stop)
position_size = (capital * 0.01) / stop_distance

# WRONG
position_size = capital * 0.5  # Arbitrary!
stop_loss = entry_price * 0.98  # Arbitrary!
```

---

## RELATED DOCUMENTS

- `docs/requirements/technical-stop-requirements.md` - Technical stop detection
- `apps/backend/core/domain/technical_stop.py` - Implementation
- `CLAUDE.md` - AI agent instructions

---

## ENFORCEMENT

The following guards BLOCK trades that violate this rule:

1. **RiskManagementGuard** - Blocks if risk > 1%
2. **TechnicalStopCalculator** - Derives position from stop
3. **RiskManagedTradeUseCase** - Orchestrates the flow

There is NO way to place a trade in Robson without:
- A technical stop defined
- Position size derived from that stop
- Risk validated to be ≤ 1%

---

## SUMMARY

| Concept | Source |
|---------|--------|
| Technical Stop | Chart analysis (2nd support) |
| Stop Distance | Calculated from stop |
| Max Risk | 1% of capital (constant) |
| Position Size | **DERIVED** from stop distance |

**The stop comes FIRST. Everything else follows.**

