# Strategy Semantic Clarity - Robson Bot Requirements

**Status**: APPROVED
**Date**: 2025-12-23
**Priority**: CRITICAL
**Type**: Semantic Definition & Architectural Constraint

---

## Executive Summary

The term **"Strategy"** has different meanings across Robson Bot's architecture layers. This document establishes the **canonical definition** and prevents semantic confusion that could lead to incorrect implementation.

**Core Principle**: Robson Bot is a **Risk Management Assistant**, not an autonomous trading system.

---

## Problem Statement

The word "strategy" appears in multiple contexts:
1. User-selected trading approach (Mean Reversion, Breakout, etc.)
2. System-generated algorithmic signals
3. Risk management rules (position sizing, stop-loss calculation)

**Risk**: Conflating these meanings leads to:
- Architectural confusion (who decides what?)
- Implementation errors (automating when user should control)
- User trust issues (unexpected automated actions)

---

## Primary Definition: User-Selected Strategy

### Canonical Meaning

**Strategy** in Robson Bot **primarily means**:

> **The trading approach selected by the USER** to guide their manual trading decisions.

The user chooses:
- Which strategy to follow (e.g., "Mean Reversion MA99")
- When to enter (based on their analysis)
- Which symbol to trade
- Their conviction level

### What Strategy Is NOT

❌ **NOT**: Autonomous algorithmic trading
❌ **NOT**: System-generated signals that auto-execute
❌ **NOT**: Black-box decision-making

### What Strategy IS

✅ **IS**: User's documented trading plan
✅ **IS**: Context for risk calculation
✅ **IS**: Configuration for position sizing rules
✅ **IS**: Reference for performance tracking

---

## Robson's Intelligence: Position Sizing Engine

### Core Value Proposition

**Robson's primary intelligence is NOT generating trading signals.**

**Robson's primary intelligence IS calculating the correct position size** based on deterministic risk rules.

### The 1% Rule Implementation

When user says: *"I want to buy BTC with 2% stop-loss using Mean Reversion strategy"*

**User provides**:
- Symbol: BTCUSDC
- Direction: BUY
- Stop-loss level: 2% below entry
- Strategy context: Mean Reversion

**Robson calculates** (the intelligence):
- Maximum position size that risks exactly 1% of capital
- Order quantity with proper precision
- Validates against total exposure limits
- Confirms monthly drawdown is within 4% limit

**Example**:
```
User Input:
  Entry: $90,000
  Stop:  $88,200 (2% stop)
  Strategy: Mean Reversion MA99

Robson Calculates:
  Capital: $1,000
  Max Risk (1%): $10
  Stop Distance: $1,800
  → Quantity: 0.00555556 BTC
  → Position Value: $500 (50% of capital)

  Validation:
  ✓ Risk = $10 (1% of capital)
  ✓ Exposure = $500 (50% of capital, within limit)
  ✓ Monthly DD = 1.5% (within 4% limit)
  → APPROVED
```

### What Robson Does

1. **Calculates** optimal position size (1% risk rule)
2. **Validates** against risk limits (drawdown, exposure)
3. **Monitors** stops automatically (24/7 monitoring)
4. **Executes** stops when triggered (safety automation)
5. **Tracks** performance per strategy (analytics)

### What Robson Does NOT Do

1. ❌ Generate trading signals automatically
2. ❌ Decide when to enter trades
3. ❌ Override user's strategy selection
4. ❌ Auto-trade without user confirmation

---

## Semantic Map: Strategy Across Layers

### 1. Domain Layer (Pure Business Logic)

**Class**: `Strategy`
**Location**: `apps/backend/monolith/api/models/trading.py`

**Definition**: User-configured trading plan with risk parameters.

**Attributes**:
```python
class Strategy:
    name: str                # e.g., "Mean Reversion MA99"
    description: str         # User's documented approach
    config: dict            # Indicator settings (MA periods, etc.)
    risk_config: dict       # Risk rules (stop %, take-profit %)
    is_active: bool         # User can enable/disable
```

**Semantics**: This is the **user's plan**, not system automation.

---

### 2. Application Layer (Use Cases)

**Field**: `strategy_name` in `TradingIntent`
**Location**: `apps/backend/core/domain/trading.py`

**Definition**: Reference to which strategy the user was following when creating this intent.

**Usage**:
```python
@dataclass
class TradingIntent:
    strategy_name: str  # "Mean Reversion MA99"
    # ... other fields
```

**Semantics**: **Context tracking** - which strategy did the user apply? (for analytics)

**NOT**: "Which algorithm generated this signal"

---

### 3. Decision Engine Layer (Future - Optional)

**IF** we implement automated signal generation (future feature):

**Class**: `DecisionStrategy` or `SignalGenerator`
**Location**: `apps/backend/core/application/decision_engine.py`

**Definition**: Algorithmic pattern detector (mean reversion detection, breakout detection).

**Semantics**: **Signal generation logic**, NOT user's trading strategy.

**Naming Convention**:
- ✅ `MeanReversionDetector` (clear intent)
- ❌ `MeanReversionStrategy` (confusing with user strategy)

---

## Architectural Constraints

### Constraint 1: User Intent is Primary

**Rule**: Every trade MUST originate from explicit user intent.

**Implementation**:
```python
# CORRECT: User provides intent
user_intent = {
    'symbol': 'BTCUSDC',
    'side': 'BUY',
    'entry_price': 90000,
    'stop_price': 88200,
    'strategy_name': 'Mean Reversion MA99',  # User's chosen strategy
}

# Robson calculates position size
calculated_quantity = calculate_position_size(
    capital=1000,
    entry_price=90000,
    stop_price=88200,
    max_risk_percent=1.0
)

# User confirms before execution
if user_confirms(calculated_quantity):
    execute_order(...)
```

**INCORRECT**:
```python
# ❌ System decides to trade autonomously
signal = decision_engine.generate_signal('BTCUSDC')
execute_order(signal)  # No user confirmation!
```

---

### Constraint 2: Strategy Selection is Manual

**Rule**: User explicitly selects which strategy to use.

**UI Flow**:
```
1. User opens "New Operation" dialog
2. User selects from dropdown:
   - [ ] Mean Reversion MA99
   - [ ] Breakout Consolidation
   - [ ] Manual Analysis
3. User enters trade parameters
4. System calculates position size
5. User confirms
```

**NOT**:
```
1. System detects mean reversion pattern
2. System auto-selects "Mean Reversion" strategy
3. System executes trade
```

---

### Constraint 3: Configuration vs Automation

**Strategy.config**: Configuration data, NOT automation logic.

**Example**:
```python
strategy.config = {
    'indicator_periods': {
        'MA_short': 7,
        'MA_medium': 25,
        'MA_long': 99,
    },
    'entry_rules': {
        'price_below_ma99_percent': 1.5,  # Documentation
        'bounce_above_ma99_percent': 0.1,  # Documentation
    }
}
```

**Semantics**: These are **reference values** for user's manual analysis, NOT triggers for automated execution.

---

## Use Case Examples

### Use Case 1: User Creates Operation with Strategy

**Actor**: Trader
**Goal**: Open a BTC long position with proper risk management

**Flow**:
1. User analyzes chart, identifies mean reversion setup
2. User selects "Mean Reversion MA99" strategy from dropdown
3. User enters:
   - Symbol: BTCUSDC
   - Entry: $90,000 (current price)
   - Stop: $88,200 (technical support level - user's analysis)
4. **Robson calculates**:
   - Quantity: 0.00555556 BTC (1% risk)
   - Position value: $500
   - Risk amount: $10
5. User reviews calculation, confirms
6. System places order, monitors stop

**Key Point**: User decided to enter, user chose strategy, **Robson only calculated size**.

---

### Use Case 2: Strategy Performance Tracking

**Actor**: Trader
**Goal**: Analyze which strategy performs better

**Flow**:
1. User views "Strategy Analytics" dashboard
2. System shows P&L by strategy:
   - Mean Reversion MA99: +$50 (10 trades, 60% win rate)
   - Breakout Consolidation: -$20 (5 trades, 40% win rate)
3. User decides to focus more on Mean Reversion

**Key Point**: Strategy is a **label for grouping trades**, not an autonomous agent.

---

### Use Case 3: Systematic Trading (Future - Opt-In)

**IF** we add systematic trading in the future:

**Actor**: Advanced Trader
**Goal**: Auto-execute pre-approved setups

**Flow**:
1. User **explicitly enables** "Auto-Execute" mode
2. User configures: "Auto-execute Mean Reversion when confidence > 80%"
3. System monitors market
4. System detects setup matching user criteria
5. System **requests user approval** (notification)
6. User approves → System executes
7. OR: User has pre-approved via "Auto-Approve" setting (advanced)

**Key Point**: Even in systematic mode, user has **explicit control and approval**.

---

## Glossary: Preventing Confusion

| Term | Correct Meaning | Incorrect Meaning (Avoid) |
|------|-----------------|---------------------------|
| **Strategy** | User's selected trading approach | Algorithmic trading bot |
| **TradingIntent** | User's intent to trade | System-generated signal |
| **DecisionEngine** | Pattern detector (future) | Autonomous trader |
| **Signal** | Detected market pattern | Execution command |
| **Execution** | Placing orders | Deciding what to trade |

---

## Database Schema Clarification

### Strategy Model

```python
class Strategy(BaseModel):
    """User-configured trading plan."""

    name = models.CharField(max_length=255)  # User-chosen name
    description = models.TextField()         # User's documented plan
    config = models.JSONField()              # Reference settings
    risk_config = models.JSONField()         # Risk parameters
    is_active = models.BooleanField()        # User can toggle

    # Performance tracking (read-only analytics)
    total_trades = models.IntegerField(default=0)
    winning_trades = models.IntegerField(default=0)
    total_pnl = models.DecimalField(...)
```

**Semantics**: This is a **configuration record**, not an executable agent.

---

### Operation Model

```python
class Operation(BaseModel):
    """User's trading operation."""

    strategy = models.ForeignKey(Strategy)  # Which strategy user was following
    symbol = models.ForeignKey(Symbol)
    side = models.CharField(...)            # BUY/SELL (user chose)

    # Risk parameters (calculated by Robson)
    stop_gain_percent = models.DecimalField()
    stop_loss_percent = models.DecimalField()
```

**Semantics**: `strategy` is a **reference**, not a controller.

---

## Implementation Guidelines

### For Backend Developers

✅ **DO**:
- Ask user for strategy selection
- Calculate position size based on strategy's risk_config
- Track performance by strategy
- Validate user inputs against strategy limits

❌ **DON'T**:
- Auto-select strategy for user
- Execute trades based solely on strategy rules
- Assume strategy means autonomous trading

---

### For Frontend Developers

✅ **DO**:
- Show strategy dropdown in "New Operation" form
- Display calculated position size clearly
- Require user confirmation before execution
- Show strategy performance analytics

❌ **DON'T**:
- Hide strategy selection from user
- Auto-submit orders based on strategy
- Imply Robson will trade automatically

---

### For DevOps/SRE

✅ **DO**:
- Monitor stop-loss execution (automated safety)
- Alert on risk limit breaches
- Track strategy performance metrics

❌ **DON'T**:
- Deploy "auto-trading" workers without user opt-in
- Enable systematic execution by default

---

## Migration Path for Systematic Trading (Future)

**IF** we want to add systematic trading later:

### Phase 1: Manual (Current - Required)
- User selects strategy
- User enters trade parameters
- Robson calculates size
- User confirms

### Phase 2: Assisted (Future - Optional)
- User enables "Suggestions" mode
- System detects patterns
- System **suggests** trades (notifications)
- User reviews and approves each one

### Phase 3: Semi-Automatic (Future - Advanced)
- User enables "Auto-Execute" for specific strategy
- User sets confidence threshold (e.g., > 90%)
- System generates intents
- User can pause/override anytime
- System requests periodic re-confirmation

### Phase 4: Fully Automatic (Future - Expert)
- User explicitly opts-in to "Autonomous Mode"
- User sets strict limits (max trades/day, max drawdown)
- System trades within limits
- User receives real-time notifications
- User can pause instantly

**Key**: Each phase requires **explicit user opt-in**.

---

## Success Criteria

This requirement is satisfied when:

✅ Every developer understands: Strategy = User's choice, not system automation
✅ Code review catches misuse of "strategy" as autonomous trading
✅ UI clearly shows strategy as user selection
✅ Documentation consistently uses correct semantics
✅ User trust is maintained (no unexpected automation)

---

## References

- Original discussion: 2025-12-23 user clarification
- Related ADRs: None (this defines new requirement)
- Related docs: `EXECUTION-PLAN-STRATEGIC-OPERATIONS.md`

---

**Approved By**: Product Owner (User)
**Implementation Status**: In Progress (semantic clarification complete, code review pending)
**Review Date**: 2025-12-23
