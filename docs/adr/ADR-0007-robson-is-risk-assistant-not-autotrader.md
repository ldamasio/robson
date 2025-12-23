# ADR-0007: Robson is a Risk Management Assistant, Not an Auto-Trader

**Status**: ACCEPTED
**Date**: 2025-12-23
**Deciders**: Product Owner, Development Team
**Related**: STRATEGY-SEMANTIC-CLARITY.md

---

## Context

During implementation of systematic trading features, a critical semantic ambiguity emerged around the term "strategy" and the core value proposition of Robson Bot.

**Key Question**: Is Robson an **autonomous trading system** or a **risk management assistant**?

This decision fundamentally shapes:
- System architecture (who initiates trades?)
- User experience (manual vs automated)
- Trust model (user control vs delegation)
- Regulatory compliance (automated trading rules)
- Product positioning (tool vs bot)

---

## Decision

**Robson Bot is a Risk Management Assistant, NOT an autonomous auto-trader.**

### Core Value Proposition

**Robson's intelligence is calculating optimal position sizes, not generating trading signals.**

**User responsibilities**:
- Analyze markets
- Decide when to trade
- Choose strategy to follow
- Set entry and stop levels

**Robson's responsibilities**:
- Calculate position size (1% risk rule)
- Validate risk limits (drawdown, exposure)
- Monitor stops 24/7
- Execute stops automatically (safety)
- Track performance per strategy

---

## Rationale

### 1. User Trust and Control

**Reasoning**: Traders want **assistance**, not replacement.

- Users have domain expertise (chart analysis, market context)
- Users want to maintain control over trade timing
- Unexpected automation erodes trust
- Manual confirmation builds confidence

**Evidence**: User explicitly stated: *"The user selects the strategy to operate"*

---

### 2. Risk and Liability

**Reasoning**: Autonomous trading carries significant risks.

- Regulatory complexity (auto-trading compliance)
- Legal liability for losses
- Black-box decision-making (explainability issues)
- Flash crash scenarios (uncontrolled automation)

**Mitigation**: User-initiated trades have clear accountability.

---

### 3. Product Differentiation

**Reasoning**: Market is saturated with "trading bots."

**Robson's differentiation**:
- ✅ Professional risk management tool
- ✅ Surgical position sizing (1% rule)
- ✅ 24/7 stop monitoring
- ✅ Transparent calculations
- ✅ User remains in control

vs. Commodity trading bots:
- ❌ Opaque signal generation
- ❌ Black-box algorithms
- ❌ No user control
- ❌ High failure rate

---

### 4. Technical Simplicity

**Reasoning**: Risk assistant is simpler than autonomous trader.

**Risk Assistant** (current scope):
- Calculate position size ✓
- Validate limits ✓
- Monitor stops ✓
- Execute stops ✓

**Autonomous Trader** (out of scope):
- Market regime detection (complex ML)
- Signal generation (pattern recognition)
- Entry timing (tick-level optimization)
- Portfolio optimization (multi-asset correlation)

**Conclusion**: Ship value faster by focusing on core differentiation.

---

## Consequences

### Positive

✅ **Clear Product Identity**: "Risk Management Assistant for Manual Traders"
✅ **User Control**: User always initiates trades (builds trust)
✅ **Simpler Architecture**: No need for complex decision engine initially
✅ **Faster Time-to-Market**: Core value (position sizing) ships first
✅ **Regulatory Clarity**: Manual trading, not automated
✅ **Explainability**: All calculations transparent to user

---

### Negative

❌ **Not Fully Automated**: Users seeking 100% automation won't choose Robson
❌ **Manual Overhead**: User must analyze and decide (but this is the point)
❌ **Limited Scale**: Can't monitor 50+ symbols simultaneously (future feature)

---

### Neutral

⚪ **Future Expansion**: Can add systematic trading as opt-in feature later
⚪ **Hybrid Model**: Can assist with suggestions without auto-executing

---

## Implementation

### Architecture Changes

**Before** (Incorrect - too autonomous):
```python
# ❌ System decides to trade
signal = decision_engine.generate_signal('BTCUSDC')
intent = generate_intent_use_case.execute(signal)
execute_intent_use_case.execute(intent.id)  # Auto-execute!
```

**After** (Correct - user-initiated):
```python
# ✅ User provides intent
user_input = {
    'symbol': 'BTCUSDC',
    'side': 'BUY',
    'entry_price': 90000,
    'stop_price': 88200,
    'strategy_name': 'Mean Reversion MA99',  # User's strategy
}

# Robson calculates size
calculated = calculate_position_size(
    capital=user.capital,
    entry=user_input['entry_price'],
    stop=user_input['stop_price'],
    max_risk_percent=1.0,
)

# User confirms
if user_confirms(calculated):
    execute_order(...)
```

---

### Code Review Checklist

When reviewing code, ensure:

- [ ] All trade entries require explicit user input
- [ ] "Strategy" refers to user's selection, not system automation
- [ ] Position sizing is calculated, not guessed
- [ ] User confirms before any order execution
- [ ] Stop monitoring is automated (safety feature)
- [ ] No hidden autonomous trading logic

---

### UI/UX Guidelines

**DO**:
- Show calculated position size prominently
- Require explicit confirmation button
- Display risk amount in USDC (e.g., "$10 at risk")
- Explain calculations (transparency)

**DON'T**:
- Hide position size calculation
- Auto-submit orders
- Use confusing terms like "auto-trade"
- Imply Robson makes decisions for user

---

## Future Evolution: Systematic Trading (Optional)

**IF** we add systematic trading later:

### Phase 1: Suggestions (Assistive)
- System detects patterns
- System **suggests** trades via notifications
- User reviews each suggestion
- User confirms or rejects

### Phase 2: Opt-In Automation (Advanced)
- User **explicitly enables** auto-execute mode
- User sets strict limits (confidence threshold, max trades/day)
- System generates intents
- System **requests approval** for each batch
- User can pause/override anytime

### Phase 3: Full Automation (Expert - Future)
- User explicitly opts into "Autonomous Mode"
- User signs risk acknowledgment
- System trades within pre-approved limits
- User receives real-time notifications
- User can emergency-stop instantly

**Critical**: Each phase requires **explicit user opt-in**.

---

## Examples

### Example 1: User Creates Manual Trade

**User Journey**:
1. User analyzes BTC chart, spots mean reversion setup
2. User opens "New Operation" form
3. User selects "Mean Reversion MA99" from strategy dropdown
4. User enters:
   - Entry: $90,000 (current market price)
   - Stop: $88,200 (technical support level)
5. **Robson displays**:
   - Calculated quantity: 0.00555556 BTC
   - Position value: $500
   - Risk amount: $10 (1% of $1,000 capital)
   - "Confirm to proceed" button
6. User reviews, clicks "Confirm"
7. Order placed, stop monitor activated

**Key**: User initiated, Robson calculated, user confirmed.

---

### Example 2: Stop Monitor (Automation - Safety)

**Scenario**: User's BTC long hits stop-loss at 2am

**System Behavior**:
1. Stop monitor detects price hit $88,200
2. System **automatically** places sell order (safety automation)
3. Order executes, position closed
4. User receives notification: "Stop-loss executed on BTCUSDC"
5. Audit trail records: "Automatic stop execution (user stop: $88,200)"

**Key**: Automation for **safety**, not for **entry decisions**.

---

### Example 3: Strategy Performance Review

**User Journey**:
1. User views "Analytics" dashboard
2. System shows P&L grouped by strategy:
   - **Mean Reversion MA99**: +$50 (10 trades, 60% win rate)
   - **Breakout Consolidation**: -$20 (5 trades, 40% win rate)
3. User decides: "I'll focus more on Mean Reversion"
4. User adjusts their manual trading accordingly

**Key**: Strategy is a **label** for analytics, not an autonomous agent.

---

## Compliance with This ADR

### Checklist for New Features

Before shipping any feature that involves trading:

- [ ] Does this require user initiation? (YES required)
- [ ] Is position size calculated by Robson? (YES required)
- [ ] Does user confirm before execution? (YES required)
- [ ] Is automation limited to safety (stops)? (YES required)
- [ ] Are calculations transparent to user? (YES required)
- [ ] Can user override/pause anytime? (YES required)

If any answer is NO, feature violates this ADR.

---

## Related Decisions

- **ADR-0002**: Hexagonal Architecture (enables user-facing adapters)
- **ADR-0006**: English-Only Codebase (international positioning)
- **STRATEGY-SEMANTIC-CLARITY.md**: Detailed semantic definitions

---

## References

- User clarification: 2025-12-23 conversation
- Product positioning: Risk Management Assistant
- Regulatory: Manual trading (no auto-trading license needed)

---

**Approved By**: Product Owner
**Implementation Date**: 2025-12-23
**Review Cycle**: Quarterly (or when adding systematic features)
