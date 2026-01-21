# AI Skills Framework: Robson Bot

**Reusable AI capabilities and prompt patterns for the Robson Bot AI-First Trading System.**

This document defines the skills, behaviors, and decision-making protocols for AI agents operating within the Robson Bot ecosystem. It serves as a prompt engineering guide for building reliable, safe, and effective AI-powered trading assistants.

**Last Updated**: 2026-01-20
**Target**: AI Engineers, LLM Integrations, Autonomous Agents
**System**: RBX Systems AI-First Trading Platform

---

## Table of Contents

1. [Overview](#overview)
2. [Core Skills Taxonomy](#core-skills-taxonomy)
3. [Skill Definitions](#skill-definitions)
4. [Prompt Templates](#prompt-templates)
5. [Decision Trees](#decision-trees)
6. [Safety Protocols](#safety-protocols)
7. [Context Management](#context-management)
8. [Tool Integration](#tool-integration)
9. [Multi-Agent Orchestration](#multi-agent-orchestration)
10. [Evaluation Metrics](#evaluation-metrics)

---

## Overview

### What is an AI Skill?

In the Robson Bot context, a **skill** is a **composable AI capability** that:
- ✅ Accepts structured input (parameters, context)
- ✅ Executes a specific trading-related task
- ✅ Returns validated output with confidence scores
- ✅ Follows safety protocols (fail-safe, auditable)
- ✅ Can be chained with other skills

### AI-First Design Principles

**Robson Bot is NOT a chatbot. It's an AI-native trading platform where:**

```
User Intent → AI Planning → Risk Validation → User Confirmation → AI Execution → Audit Trail
```

**Core Tenets**:
1. **User-Initiated, AI-Calculated**: User provides intent, AI calculates optimal execution
2. **Validation-First**: Every action goes through PLAN → VALIDATE → EXECUTE
3. **Explainable**: All AI decisions must be traceable and understandable
4. **Fail-Safe**: Default to DRY-RUN mode; LIVE requires explicit acknowledgment
5. **Position Sizing is Sacred**: Always calculated from technical stop (never arbitrary)

---

## Core Skills Taxonomy

### Level 1: Foundation Skills (Atomic Operations)

| Skill ID | Name | Description | Input | Output |
|----------|------|-------------|-------|--------|
| `SKILL-001` | **Parse Trading Intent** | Extract symbol, side, strategy from natural language | User message | Structured intent |
| `SKILL-002` | **Fetch Market Data** | Get current price, volume, indicators | Symbol, timeframe | OHLCV + indicators |
| `SKILL-003` | **Identify Technical Stop** | Find invalidation level from chart | Symbol, timeframe, side | Stop price + confidence |
| `SKILL-004` | **Calculate Position Size** | Compute quantity from risk (1% rule) | Capital, entry, stop | Quantity, risk amount |
| `SKILL-005` | **Validate Risk Limits** | Check exposure, drawdown constraints | Position params | PASS/FAIL + reasons |
| `SKILL-006` | **Generate Execution Plan** | Create PLAN from validated intent | Intent, market data | Execution plan object |
| `SKILL-007` | **Execute Order** | Place order on exchange (DRY-RUN/LIVE) | Plan, mode | Order confirmation |
| `SKILL-008` | **Monitor Stop Levels** | Watch for stop/profit trigger events | Open positions | Trigger events |
| `SKILL-009` | **Audit Transaction** | Record financial movement | Transaction details | Audit record |
| `SKILL-010` | **Explain Decision** | Provide human-readable reasoning | Decision context | Explanation text |
| `SKILL-011` | **Query Knowledge Base** | Retrieve relevant knowledge from PR history | Query, context | Knowledge entries |

### Level 2: Composite Skills (Workflows)

| Skill ID | Name | Composed Of | Description |
|----------|------|-------------|-------------|
| `SKILL-101` | **Smart Order Entry** | 001→002→003→004→005→006 | Full entry workflow with risk calculation |
| `SKILL-102` | **Strategy Execution** | 002→006→007→008→009 | Execute pre-validated strategy |
| `SKILL-103` | **Risk Assessment Report** | 002→005→010 | Comprehensive risk analysis |
| `SKILL-104` | **Position Management** | 002→008→007 | Monitor and adjust open positions |
| `SKILL-105` | **Pattern Recognition** | 002→ML Model→010 | Identify chart patterns with explanations |

### Level 3: Agentic Skills (Autonomous Loops)

| Skill ID | Name | Description | Autonomy Level |
|----------|------|-------------|----------------|
| `SKILL-201` | **24/7 Stop Monitor** | Continuous monitoring + auto-execution on triggers | Semi-autonomous (user pre-approved) |
| `SKILL-202` | **Market Scanner** | Scan 100+ symbols for strategy setups | Autonomous (alerting only) |
| `SKILL-203` | **Risk Guardian** | Real-time portfolio risk monitoring + auto-hedge | Semi-autonomous (defensive only) |
| `SKILL-204` | **Performance Analyst** | Daily P&L analysis + strategy optimization suggestions | Autonomous (reporting only) |

---

## Skill Definitions

### SKILL-001: Parse Trading Intent

**Purpose**: Convert natural language user input into structured trading intent.

**Prompt Template**:
```
You are a trading intent parser. Extract structured information from user messages.

User Message: "{user_input}"

Extract:
1. Symbol (e.g., BTCUSDT, ETHUSDT) - REQUIRED
2. Side (BUY or SELL) - REQUIRED
3. Entry Price (if specified, else "market")
4. Stop Price or Stop Percentage (if specified)
5. Strategy Name (if mentioned, else "Manual")
6. Timeframe (if mentioned, else "15m")
7. Confidence (0.0-1.0): How confident are you in this parsing?

Output Format (JSON):
{
  "symbol": "BTCUSDT",
  "side": "BUY",
  "entry_price": 95000.0,
  "stop_price": 93500.0,
  "strategy": "All In",
  "timeframe": "15m",
  "confidence": 0.95,
  "ambiguities": ["User didn't specify entry price, assuming market"]
}

If you cannot extract REQUIRED fields with confidence > 0.7, return:
{
  "error": "INSUFFICIENT_INFORMATION",
  "missing_fields": ["symbol", "side"],
  "clarification_needed": "Please specify which asset you want to trade and whether you want to BUY or SELL."
}
```

**Example Inputs**:
- ✅ "Buy BTC, stop at 93500, All In strategy" → Full extraction
- ✅ "Short ETH on 4h chart" → Infers SELL, asks for stop
- ❌ "Go long" → Missing symbol, requests clarification

**Safety Checks**:
- Never assume symbol (too risky)
- Never assume side (catastrophic if wrong)
- Always validate strategy name against database
- Flag ambiguities for user confirmation

---

### SKILL-003: Identify Technical Stop

**Purpose**: Determine technical invalidation level from chart analysis (2nd support/resistance).

**Prompt Template**:
```
You are a technical analyst specializing in stop-loss placement.

Task: Identify the technical invalidation level for a {side} position on {symbol} using {timeframe} chart.

Context:
- Current Price: {current_price}
- Side: {side}
- Timeframe: {timeframe}
- Recent Candles (last 20): {ohlcv_data}
- Support/Resistance Levels: {sr_levels}

Methodology:
1. For LONG (BUY): Find 2nd support level below entry
2. For SHORT (SELL): Find 2nd resistance level above entry
3. Validate with volume profile (must be significant level)
4. Check if stop is at least 1.5% away from entry (avoid noise)

Output Format (JSON):
{
  "technical_stop": 93500.0,
  "stop_type": "2nd_support",
  "distance_percent": 1.58,
  "confidence": 0.88,
  "reasoning": "2nd support at $93,500 (tested 3 times in last 24h with high volume). 1st support too close at $94,800 (only 0.2% away).",
  "chart_evidence": {
    "support_touches": [
      {"timestamp": "2026-01-20T08:15:00Z", "price": 93510, "volume": 1250},
      {"timestamp": "2026-01-19T14:30:00Z", "price": 93490, "volume": 980}
    ]
  },
  "warning": null
}

If stop is too tight (<1% away):
{
  "technical_stop": 94950.0,
  "warning": "STOP_TOO_TIGHT",
  "recommendation": "1st support is only 0.05% away. Consider 2nd support at $93,500 (1.58% away) to avoid stop hunting."
}
```

**Decision Tree**:
```
1. Fetch recent price data (last 50 candles on specified timeframe)
2. Calculate support/resistance levels using:
   - Swing highs/lows
   - Volume profile POCs (Point of Control)
   - Fibonacci retracements (optional)
3. For LONG: Identify supports below current price
4. For SHORT: Identify resistances above current price
5. Select 2nd level (1st is often too tight)
6. Validate:
   ✓ At least 1.5% away from entry (configurable)
   ✓ Has volume confirmation (>1.2x average volume on touches)
   ✓ Not in the middle of nowhere (must be a real level)
7. Return stop price + confidence score
```

**Safety Checks**:
- ⚠️ If no clear technical level exists → Return confidence < 0.5 + explanation
- ⚠️ If stop is >5% away → Flag as "wide stop, small position expected"
- ⚠️ If market is in consolidation → Warn user about choppy conditions

---

### SKILL-004: Calculate Position Size

**Purpose**: Compute position size using the 1% risk rule (FROM TECHNICAL STOP, NEVER ARBITRARY).

**Critical Context**:
```
⚠️ GOLDEN RULE: Position Size is DERIVED from Technical Stop

THE ORDER OF OPERATIONS:
1. FIRST: Identify technical stop (SKILL-003)
2. THEN: Calculate stop distance = |Entry - Technical Stop|
3. THEN: Max Risk = Capital × 1%
4. FINALLY: Position Size = Max Risk / Stop Distance

Position Size = (Capital × 1%) / |Entry Price - Technical Stop|

This is NOT negotiable. The formula is sacred.
```

**Prompt Template**:
```
You are a position sizing calculator following the 1% risk rule.

Given:
- Capital: {capital} USD
- Entry Price: {entry_price} USD
- Technical Stop: {technical_stop} USD (from SKILL-003)
- Risk Percentage: 1% (fixed, non-negotiable)

Calculate:
1. Stop Distance = |{entry_price} - {technical_stop}|
2. Max Risk = {capital} × 0.01
3. Position Size (base asset) = Max Risk / Stop Distance
4. Position Value (quote asset) = Position Size × Entry Price
5. Position Percentage of Capital = (Position Value / Capital) × 100

Output Format (JSON):
{
  "position_size": 0.0667,  // BTC
  "position_value": 6333.33,  // USD
  "capital_allocation_percent": 63.33,
  "max_risk": 100.0,  // USD (1% of $10,000)
  "stop_distance": 1500.0,  // USD
  "risk_reward_ratio": 2.0,  // If take-profit provided
  "validation": {
    "is_valid": true,
    "warnings": []
  }
}

Validation Rules:
1. Position value must be >= exchange minimum (e.g., $10 for Binance)
2. Position size must be within exchange LOT_SIZE limits
3. If position value > 90% of capital → WARNING (over-leveraged)
4. If stop distance < 1% → WARNING (stop too tight)
5. If stop distance > 10% → WARNING (stop too wide, tiny position)
```

**Example**:
```json
// Input
{
  "capital": 10000,
  "entry_price": 95000,
  "technical_stop": 93500
}

// Output
{
  "position_size": 0.0667,  // BTC
  "position_value": 6333.33,  // $6,333
  "capital_allocation_percent": 63.33,
  "max_risk": 100.0,  // Always $100 (1% of $10k)
  "stop_distance": 1500.0,
  "explanation": "Wide stop (1.58%) results in smaller position to maintain 1% risk. If stopped at $93,500, loss = 0.0667 × $1,500 = $100 = 1% ✓"
}
```

**Anti-Patterns (NEVER DO THIS)**:
```python
# ❌ WRONG: Arbitrary position size
position_size = capital * 0.5  # "I want to invest 50% of my capital"

# ❌ WRONG: Percentage-based stop
stop_price = entry_price * 0.98  # "2% stop loss"

# ✅ CORRECT: Technical stop first, then calculate size
technical_stop = identify_technical_level(chart_data)  # From chart!
position_size = (capital * 0.01) / abs(entry_price - technical_stop)
```

---

### SKILL-005: Validate Risk Limits

**Purpose**: Enforce risk management rules before execution.

**Prompt Template**:
```
You are a risk management guardian. Validate if the proposed position complies with all risk limits.

Proposed Position:
{position_params}

Current Portfolio State:
{portfolio_state}

Risk Limits:
- Max Single Position Risk: 1% of capital
- Max Daily Drawdown: 3% of capital
- Max Portfolio Exposure: 50% of capital
- Max Correlation Exposure: 30% (positions in correlated assets)
- Max Leverage: 3x (for margin trades)

Validation Checks:
1. ✓/✗ Single Position Risk <= 1%
2. ✓/✗ Daily Drawdown + This Risk <= 3%
3. ✓/✗ Total Exposure + This Position <= 50%
4. ✓/✗ Correlation Check (if BTC position exists, limit altcoins)
5. ✓/✗ Leverage within bounds
6. ✓/✗ Sufficient Free Capital

Output Format (JSON):
{
  "validation_result": "PASS" | "FAIL" | "WARNING",
  "checks": [
    {"name": "single_position_risk", "status": "PASS", "value": "1.0%", "limit": "1.0%"},
    {"name": "daily_drawdown", "status": "PASS", "value": "0.5%", "limit": "3.0%"},
    {"name": "portfolio_exposure", "status": "WARNING", "value": "45%", "limit": "50%", "message": "High exposure, consider reducing other positions"},
    {"name": "correlation", "status": "PASS", "value": "15%", "limit": "30%"}
  ],
  "overall_status": "WARNING",
  "blockers": [],
  "warnings": ["Portfolio exposure at 45%, close to limit"],
  "recommendations": ["Consider closing ETHUSDT position before opening this one"]
}

If validation_result == "FAIL":
  Block execution and return specific blockers.
If validation_result == "WARNING":
  Allow execution but log warnings.
If validation_result == "PASS":
  Proceed to execution.
```

**Decision Logic**:
```python
def validate_risk_limits(position, portfolio):
    blockers = []
    warnings = []

    # BLOCKER: Single position risk > 1%
    if position.risk_amount > portfolio.capital * 0.01:
        blockers.append("Position risk exceeds 1% limit")

    # BLOCKER: Daily drawdown would exceed 3%
    if portfolio.daily_drawdown + position.risk_amount > portfolio.capital * 0.03:
        blockers.append("Would exceed 3% daily drawdown limit")

    # WARNING: High exposure
    if portfolio.total_exposure + position.value > portfolio.capital * 0.45:
        warnings.append("Portfolio exposure approaching 50% limit")

    # BLOCKER: Insufficient capital
    if portfolio.free_capital < position.value:
        blockers.append("Insufficient free capital")

    if blockers:
        return "FAIL", blockers, warnings
    elif warnings:
        return "WARNING", blockers, warnings
    else:
        return "PASS", blockers, warnings
```

---

### SKILL-101: Smart Order Entry (Composite)

**Purpose**: Full workflow from user intent to execution plan with all validations.

**Orchestration Flow**:
```
User Input
    ↓
SKILL-001: Parse Intent
    ↓
[Validation: Intent Clear?] → NO → Ask Clarification → Loop back
    ↓ YES
SKILL-002: Fetch Market Data
    ↓
SKILL-003: Identify Technical Stop
    ↓
[Validation: Stop Confidence > 0.7?] → NO → Warn User + Suggest Manual Stop
    ↓ YES
SKILL-004: Calculate Position Size
    ↓
SKILL-005: Validate Risk Limits
    ↓
[Validation: Risk Checks Pass?] → NO → Block + Explain → END
    ↓ YES
SKILL-006: Generate Execution Plan
    ↓
Present to User for Confirmation
    ↓
[User Confirms?] → NO → Cancel → END
    ↓ YES
SKILL-007: Execute Order (DRY-RUN or LIVE)
    ↓
SKILL-009: Audit Transaction
    ↓
SKILL-008: Activate Stop Monitor
    ↓
Return Confirmation + Audit Trail
```

**Prompt Template**:
```
You are executing the Smart Order Entry workflow.

Step-by-step execution:

1. Parse Intent:
   Input: "{user_message}"
   Execute: SKILL-001
   Result: {intent_object}

2. Fetch Market Data:
   Execute: SKILL-002(symbol={intent.symbol}, timeframe={intent.timeframe})
   Result: {market_data}

3. Identify Technical Stop:
   Execute: SKILL-003(symbol={intent.symbol}, side={intent.side}, timeframe={intent.timeframe}, market_data={market_data})
   Result: {technical_stop_object}

   Checkpoint: If confidence < 0.7 → Ask user to manually specify stop

4. Calculate Position Size:
   Execute: SKILL-004(capital={user.capital}, entry={market_data.current_price}, stop={technical_stop_object.stop_price})
   Result: {position_size_object}

5. Validate Risk Limits:
   Execute: SKILL-005(position={position_size_object}, portfolio={user.portfolio})
   Result: {validation_result}

   Checkpoint: If status == "FAIL" → Block execution + explain blockers

6. Generate Execution Plan:
   Execute: SKILL-006(intent={intent_object}, market={market_data}, position={position_size_object})
   Result: {execution_plan}

7. Present to User:
   """
   📊 Execution Plan Summary:

   Symbol: {plan.symbol}
   Side: {plan.side}
   Entry Price: ${plan.entry_price}
   Stop Loss: ${plan.stop_price} ({plan.stop_distance_percent}%)

   Position Size: {plan.quantity} {plan.base_asset}
   Position Value: ${plan.position_value}
   Max Risk: ${plan.max_risk} ({plan.risk_percent}%)

   Strategy: {plan.strategy}

   ⚠️ Mode: DRY-RUN (simulation)

   Confirm execution? (yes/no)
   """

8. If user confirms → SKILL-007(plan={execution_plan}, mode="DRY-RUN")
9. Audit → SKILL-009(transaction=order_result)
10. Monitor → SKILL-008(position=order_result.position)

Return final confirmation with order ID and audit trail.
```

---

## Prompt Templates

### Template: User Clarification Request

**Use When**: Insufficient information to proceed.

```
I need more information to execute this trade safely.

Missing Information:
{list_of_missing_fields}

Ambiguities:
{list_of_ambiguities}

Please provide:
1. {specific_question_1}
2. {specific_question_2}

Example: "Buy BTC with stop at $93,500"
```

**Anti-Pattern**: Never proceed with assumptions on critical fields (symbol, side, capital).

---

### Template: Risk Rejection Explanation

**Use When**: Risk validation fails.

```
⚠️ Cannot Execute: Risk Limits Exceeded

Blockers:
{list_blockers_with_explanations}

Current Portfolio State:
- Total Exposure: {exposure}% (Limit: 50%)
- Daily Drawdown: {drawdown}% (Limit: 3%)
- Free Capital: ${free_capital}

Recommendations:
{actionable_recommendations}

Would you like to:
1. Adjust position size
2. Close an existing position first
3. Cancel this trade
```

**Tone**: Firm but helpful. Never override safety limits.

---

### Template: Execution Confirmation

**Use When**: Order successfully executed.

```
✅ Order Executed Successfully

Order ID: {order_id}
Symbol: {symbol}
Side: {side}
Quantity: {quantity} {base_asset}
Entry Price: ${entry_price}
Position Value: ${position_value}

Risk Management:
- Stop Loss: ${stop_price} ({stop_percent}%)
- Max Risk: ${max_risk} (1% of capital)
- Stop Monitor: ACTIVE ✓

Strategy: {strategy_name}
Mode: {mode}  // DRY-RUN or LIVE

Audit Trail: {audit_record_id}

Next Steps:
- Monitor position in dashboard
- Stop-loss order will auto-execute if triggered
- Consider setting take-profit target
```

---

## Decision Trees

### Tree 1: Should AI Auto-Execute or Request Confirmation?

```
START: User provides trading intent
    ↓
Is intent from a PRE-APPROVED strategy?
    ├─ YES → Is mode == LIVE?
    │         ├─ YES → Did user provide --acknowledge-risk flag?
    │         │         ├─ YES → AUTO-EXECUTE + AUDIT
    │         │         └─ NO → REQUEST CONFIRMATION (safety fallback)
    │         └─ NO (DRY-RUN) → AUTO-EXECUTE (safe simulation)
    └─ NO (manual/new intent) → ALWAYS REQUEST CONFIRMATION

RULE: When in doubt, ask for confirmation.
EXCEPTION: 24/7 Stop Monitor can auto-execute stop-loss (user pre-approved at position open).
```

---

### Tree 2: How to Handle Technical Stop Identification Failure?

```
SKILL-003 returns confidence < 0.7
    ↓
Is there a 1st support level with confidence > 0.6?
    ├─ YES → Suggest 1st support + warn it's tight
    │         → Ask user: "Accept tight stop or manually specify?"
    └─ NO → No clear technical level
            ↓
        Present options:
        1. Use ATR-based stop (Average True Range × 2)
        2. Use percentage-based stop (e.g., 2%)
        3. User manually specifies stop
        4. Cancel trade

        → User selects option
        → Proceed with selected stop + flag as "non-technical stop"
```

---

## Safety Protocols

### Protocol 1: Financial Operations (CRITICAL)

**Rules**:
1. ✅ **Default to DRY-RUN**: All executions are simulations unless `--live --acknowledge-risk` provided
2. ✅ **Explicit Confirmation**: User must confirm every LIVE trade
3. ✅ **Audit Everything**: Every financial movement logged to `AuditTransaction`
4. ✅ **Idempotency**: Execution plans must be idempotent (re-execution safe)
5. ✅ **Rollback Support**: Failed executions must be cleanly rolled back

**Code Guard**:
```python
def execute_order(plan: ExecutionPlan, mode: str = "DRY-RUN", acknowledged_risk: bool = False):
    # GUARD 1: Default to safe mode
    if mode == "LIVE" and not acknowledged_risk:
        raise SafetyException("LIVE mode requires explicit risk acknowledgment")

    # GUARD 2: Final confirmation
    if mode == "LIVE":
        log_critical_action(f"LIVE EXECUTION: {plan}")
        send_notification_to_user(plan)

    # GUARD 3: Pre-execution validation
    validation = validate_risk_limits(plan)
    if validation.status == "FAIL":
        raise RiskLimitException(validation.blockers)

    # Execute with audit trail
    with atomic_transaction():
        order = exchange.create_order(plan) if mode == "LIVE" else simulate_order(plan)
        audit_record = create_audit_transaction(order)
        return order, audit_record
```

---

### Protocol 2: Position Sizing Validation

**Rules**:
1. ✅ Position size MUST be calculated from technical stop (never arbitrary)
2. ✅ Maximum risk per trade = 1% of capital (non-negotiable)
3. ✅ Minimum position value >= exchange minimum (e.g., $10 USD)
4. ✅ Position size must respect exchange LOT_SIZE filters

**Validation Chain**:
```
Technical Stop Identified? → YES → Calculate Position Size → Validate Exchange Limits → Proceed
                          ↓ NO
                    Ask User for Manual Stop → Recalculate → Proceed
```

---

### Protocol 3: Stop-Loss Monitor (Autonomous Agent)

**Context**: This is the ONLY autonomous agent with execution permissions (user pre-approved).

**Rules**:
1. ✅ Monitors 24/7 for stop-loss/take-profit triggers
2. ✅ Auto-executes ONLY on positions with active stop orders
3. ✅ Logs every execution to audit trail
4. ✅ Sends notifications to user on execution
5. ✅ Fail-safe: If exchange API fails, retries with exponential backoff

**Agent Loop**:
```python
def stop_monitor_loop():
    while True:
        positions = get_open_positions_with_stops()

        for position in positions:
            current_price = fetch_current_price(position.symbol)

            # Check stop-loss trigger
            if should_trigger_stop(position, current_price):
                log_info(f"Stop triggered: {position}")
                execute_stop_order(position)  # Auto-execute (pre-approved)
                audit_transaction(position, "STOP_LOSS_TRIGGERED")
                notify_user(position, current_price)
                continue

            # Check take-profit trigger
            if should_trigger_profit(position, current_price):
                log_info(f"Take-profit triggered: {position}")
                execute_stop_order(position)  # Auto-execute (pre-approved)
                audit_transaction(position, "TAKE_PROFIT_TRIGGERED")
                notify_user(position, current_price)

        sleep(30)  # Check every 30 seconds
```

**Safety**: Pre-approved by user when position was opened. User can disable monitor anytime.

---

## Context Management

### Context Window Strategy

**Problem**: LLMs have limited context windows. Trading decisions require:
- Market data (OHLCV, indicators)
- Portfolio state (positions, capital)
- Historical performance (past trades)
- User preferences (risk tolerance, strategies)

**Solution**: Hierarchical context loading.

```
┌─────────────────────────────────────────────┐
│  TIER 1: Critical Context (Always Loaded)  │
│  - Current portfolio state                  │
│  - Active positions                         │
│  - Risk limits                              │
│  - User intent (current request)            │
│  Size: ~2K tokens                           │
└─────────────────────────────────────────────┘
                    ↓
┌─────────────────────────────────────────────┐
│  TIER 2: Task-Specific Context (On-Demand) │
│  - Market data for requested symbol         │
│  - Technical indicators (if pattern scan)   │
│  - Strategy parameters (if executing)       │
│  Size: ~5K tokens                           │
└─────────────────────────────────────────────┘
                    ↓
┌─────────────────────────────────────────────┐
│  TIER 3: Historical Context (Retrieval)     │
│  - Similar past trades (RAG)                │
│  - Strategy performance history             │
│  - User preference embeddings               │
│  Size: ~3K tokens (top-K retrieval)         │
└─────────────────────────────────────────────┘

Total: ~10K tokens (well within GPT-4/Claude limits)
```

**Prompt Injection**:
```
<context>
  <tier1_critical>
    <portfolio>
      <capital>10000.00</capital>
      <free_capital>7500.00</free_capital>
      <total_exposure>2500.00</total_exposure>
      <daily_drawdown>0.00</daily_drawdown>
    </portfolio>
    <active_positions>
      <position symbol="ETHUSDT" side="LONG" quantity="1.5" entry="2500" stop="2450" />
    </active_positions>
    <risk_limits>
      <max_single_risk>100.00</max_single_risk>
      <max_daily_drawdown>300.00</max_daily_drawdown>
    </risk_limits>
  </tier1_critical>

  <tier2_task_specific>
    <market_data symbol="BTCUSDT" timeframe="15m">
      <current_price>95000.00</current_price>
      <supports>[93500, 92000, 90500]</supports>
      <resistances>[96500, 98000, 100000]</resistances>
    </market_data>
  </tier2_task_specific>

  <tier3_historical>
    <similar_trades>
      <trade symbol="BTCUSDT" side="LONG" result="WIN" pnl="150" strategy="All In" />
      <trade symbol="BTCUSDT" side="LONG" result="LOSS" pnl="-100" strategy="All In" />
    </similar_trades>
  </tier3_historical>
</context>

<user_intent>
  Buy BTC with stop at 93500, All In strategy
</user_intent>

Execute SKILL-101 (Smart Order Entry).
```

---

## AI Memory Database (Runtime Knowledge Store)

### Overview

**Problem**: AI agents need access to project-specific knowledge that isn't in their training data:
- How did we solve similar bugs in the past?
- What architectural decisions were made in previous PRs?
- What code patterns does this project prefer?
- What are common pitfalls and how to avoid them?

**Solution**: **AI Memory Database** - A thread-safe, in-memory knowledge store that syncs from GitHub Pull Requests.

### Architecture

```
┌─────────────────────────────────────────────────────────┐
│                    GitHub API                            │
│              (Pull Requests Source)                      │
└────────────────────┬────────────────────────────────────┘
                     │
                     │ sync_pr_knowledge command
                     ↓
┌─────────────────────────────────────────────────────────┐
│              PR Knowledge Extractor                      │
│  - Extract decisions from PR descriptions                │
│  - Extract code patterns from reviews                    │
│  - Extract bug fixes from comments                       │
│  - Classify knowledge types                              │
└────────────────────┬────────────────────────────────────┘
                     │
                     ↓
┌─────────────────────────────────────────────────────────┐
│                AI Memory Database                        │
│  - Thread-safe in-memory storage                        │
│  - Indexed by keywords, type, PR number                 │
│  - Fast semantic search                                  │
│  - O(1) lookup by ID, O(log n) by keyword               │
└────────────────────┬────────────────────────────────────┘
                     │
                     │ query(), get_by_type()
                     ↓
┌─────────────────────────────────────────────────────────┐
│                  AI Agents                               │
│  - Query relevant knowledge during execution             │
│  - Learn from past decisions                             │
│  - Apply project-specific patterns                       │
└─────────────────────────────────────────────────────────┘
```

### Knowledge Types

The system extracts and classifies knowledge into these types:

| Type | Description | Example |
|------|-------------|---------|
| `DECISION` | Architecture/design decisions | "We decided to use hexagonal architecture to..." |
| `CODE_PATTERN` | Code patterns and best practices | "Always use type hints for function parameters" |
| `BUG_FIX` | Bug fixes and solutions | "Fixed race condition by adding lock..." |
| `REFACTORING` | Refactoring patterns | "Extracted risk validation into separate module" |
| `CONFIGURATION` | Config changes and rationale | "Changed cache TTL to 1s for real-time pricing" |
| `DISCUSSION` | Important discussions/consensus | "Agreed to use 1% risk rule consistently" |
| `TEST_PATTERN` | Testing patterns | "Mock exchange API in tests using pytest fixtures" |

### SKILL-011: Query Knowledge Base

**Purpose**: Retrieve relevant knowledge from historical PRs to inform current task.

**Prompt Template**:
```
You are querying the AI Memory Database for relevant knowledge.

Current Task: {task_description}

Query the memory database:
1. Identify search keywords from task
2. Query memory with keywords
3. Filter by relevance score >= 0.3
4. Return top 5 most relevant entries

Example Task: "Implement stop-loss monitoring for margin trades"

Search Strategy:
- Keywords: ["stop-loss", "monitoring", "margin", "trades"]
- Knowledge types: CODE_PATTERN, BUG_FIX (how was it done before?)
- Min confidence: 0.3

Query Execution:
```python
from api.application.ai_memory import get_ai_memory

memory = get_ai_memory()

# Semantic search
results = memory.query(
    query="stop-loss monitoring margin trades",
    knowledge_type=KnowledgeType.CODE_PATTERN,
    min_confidence=0.3,
    limit=5
)

# Format results for LLM context
knowledge_context = []
for entry, score in results:
    knowledge_context.append({
        "content": entry.content,
        "source": f"PR #{entry.source_pr}",
        "url": entry.source_url,
        "relevance": score,
        "type": entry.type.value
    })
```

Output Format (for LLM injection):
```xml
<knowledge_from_prs>
  <entry relevance="0.85" type="CODE_PATTERN" source="PR #234">
    <content>
    Stop-loss monitoring is implemented in api/application/stop_monitor.py.
    Key pattern: Use continuous loop with 30s sleep. Query positions with
    active stops. Compare current price with stop levels. Execute market
    orders immediately when triggered.
    </content>
    <source_url>https://github.com/ldamasio/robson/pull/234</source_url>
  </entry>

  <entry relevance="0.72" type="BUG_FIX" source="PR #189">
    <content>
    Fixed race condition in stop monitor by adding position-level locks.
    Without locks, multiple threads could trigger the same stop-loss.
    Solution: Use threading.Lock per position ID.
    </content>
    <source_url>https://github.com/ldamasio/robson/pull/189</source_url>
  </entry>
</knowledge_from_prs>
```
```

**Usage in Agent Workflow**:
```
1. Agent receives task: "Add trailing stop feature"

2. Before implementing, query knowledge base:
   memory.query("trailing stop")
   memory.get_code_patterns("stop monitoring")

3. Inject retrieved knowledge into context:
   <context>
     <tier1_critical>...</tier1_critical>
     <tier2_task_specific>...</tier2_task_specific>
     <tier3_historical>...</tier3_historical>
     <tier4_pr_knowledge>  <!-- NEW TIER -->
       {retrieved knowledge from memory DB}
     </tier4_pr_knowledge>
   </context>

4. Agent generates code informed by past patterns

5. Result: New code follows established project patterns
```

### Syncing Knowledge from GitHub

**Django Command**: `sync_pr_knowledge`

**Usage**:
```bash
# Sync all merged PRs from last 30 days
python manage.py sync_pr_knowledge

# Sync specific PR
python manage.py sync_pr_knowledge --pr 234

# Sync PRs with specific labels
python manage.py sync_pr_knowledge --labels "architecture,refactoring"

# Dry run (see what would be synced)
python manage.py sync_pr_knowledge --dry-run

# Clear memory and re-sync
python manage.py sync_pr_knowledge --clear

# Show current memory stats
python manage.py sync_pr_knowledge --stats
```

**Environment Variables**:
```bash
export GITHUB_TOKEN="ghp_xxxxxxxxxxxxx"  # Required
export GITHUB_REPO="ldamasio/robson"     # Optional (default)
```

**Example Output**:
```
Syncing knowledge from GitHub repo: ldamasio/robson
Fetching merged PRs since 2025-12-21...
PR #234: feat(trading): add stop-loss monitor (5 entries)
PR #189: fix(monitor): race condition in stop execution (3 entries)
PR #167: refactor(risk): extract risk guards (4 entries)
Stored 12 knowledge entries from 3 PRs

============================================================
AI Memory Database Statistics
============================================================
Total Knowledge Entries: 87
Total PRs Indexed: 23
Total Keywords: 342

Entries by Type:
  - CODE_PATTERN: 28
  - DECISION: 15
  - BUG_FIX: 19
  - REFACTORING: 12
  - DISCUSSION: 13

Last Sync: 2026-01-20T14:30:00Z
============================================================
```

### Scheduled Sync (Cron Job)

**For continuous learning, schedule periodic syncs:**

```yaml
# k8s CronJob (infra/k8s/cronjobs/sync-pr-knowledge.yaml)
apiVersion: batch/v1
kind: CronJob
metadata:
  name: sync-pr-knowledge
  namespace: robson
spec:
  schedule: "0 */6 * * *"  # Every 6 hours
  jobTemplate:
    spec:
      template:
        spec:
          containers:
          - name: sync
            image: robson-backend:latest
            command:
              - python
              - manage.py
              - sync_pr_knowledge
              - --days
              - "7"
            env:
            - name: GITHUB_TOKEN
              valueFrom:
                secretKeyRef:
                  name: github-credentials
                  key: token
          restartPolicy: OnFailure
```

### Memory Query Patterns

**Pattern 1: Context-Aware Code Generation**
```python
# Agent is implementing a new feature
task = "Add take-profit monitoring"

# Query similar implementations
memory = get_ai_memory()
patterns = memory.get_code_patterns("monitoring")
similar_features = memory.query("take-profit OR stop-loss")

# Inject into LLM prompt
context = f"""
You are implementing: {task}

Relevant patterns from past PRs:
{format_knowledge_entries(patterns)}

Generate code following these established patterns.
"""
```

**Pattern 2: Bug Resolution**
```python
# Agent encounters an error
error = "ThreadPoolExecutor deadlock in stop monitor"

# Find similar bug fixes
memory = get_ai_memory()
similar_bugs = memory.get_similar_bug_fixes(error)

# Show to agent
if similar_bugs:
    print(f"Found {len(similar_bugs)} similar bug fixes from PRs:")
    for entry in similar_bugs:
        print(f"  - PR #{entry.source_pr}: {entry.content[:100]}...")
```

**Pattern 3: Decision Retrieval**
```python
# Agent needs to make architectural choice
decision_needed = "How to structure margin trading module?"

# Query past decisions
memory = get_ai_memory()
decisions = memory.query(
    decision_needed,
    knowledge_type=KnowledgeType.DECISION,
    limit=3
)

# Present options based on past decisions
for entry, score in decisions:
    print(f"Past decision (relevance: {score:.2f}):")
    print(f"  {entry.content}")
    print(f"  Source: PR #{entry.source_pr}")
```

### Performance Characteristics

| Operation | Complexity | Latency |
|-----------|-----------|---------|
| Store entry | O(k) where k = keywords | <1ms |
| Query by keyword | O(n) where n = entries | <10ms for 1000 entries |
| Query by type | O(n) | <5ms |
| Get by PR number | O(1) | <1ms |
| Semantic search | O(n) | <20ms for 1000 entries |

**Memory Usage**:
- Average entry size: ~500 bytes (content + metadata)
- 100 PRs with avg 5 entries each = 500 entries × 500 bytes = ~250KB
- Keyword index: ~50KB
- Total: ~300KB for 100 PRs (negligible)

**Thread Safety**:
- All operations use `RLock` (reentrant lock)
- Safe for concurrent reads and writes
- No deadlocks (reentrant)

### Integration with Skills

**SKILL-011 is used by other skills:**

```
SKILL-101: Smart Order Entry
    ↓
1. Parse Intent (SKILL-001)
    ↓
2. Query Knowledge (SKILL-011)  ← "position sizing patterns"
    ↓
3. Fetch Market Data (SKILL-002)
    ↓ (knowledge informs technical stop identification)
4. Identify Technical Stop (SKILL-003)
    ↓
... continue workflow
```

**Example: Agent uses SKILL-011 before implementing**:
```python
# Agent workflow
task = "Implement margin liquidation monitor"

# Step 1: Query knowledge base
memory = get_ai_memory()
knowledge = memory.query("liquidation monitoring margin")

# Step 2: Inject knowledge into context
context = build_context_with_knowledge(task, knowledge)

# Step 3: Generate code with LLM (context includes PR knowledge)
code = llm.generate(context)

# Result: Code follows patterns from PR #156, PR #189
```

### Best Practices

**DO**:
✅ Sync knowledge regularly (every 6 hours)
✅ Query knowledge before implementing new features
✅ Use specific queries (not too broad)
✅ Filter by knowledge type when appropriate
✅ Check relevance scores (>0.3 is usually good)

**DON'T**:
❌ Don't rely solely on memory (use documentation too)
❌ Don't store PII or secrets in knowledge
❌ Don't query on every single line of code (too expensive)
❌ Don't ignore low relevance scores
❌ Don't forget to set GITHUB_TOKEN

---

## Tool Integration

### Django Management Commands (CLI)

**AI agents interact with Robson Bot via Django commands.**

**Key Commands**:
```bash
# Market Data
python manage.py fetch_price --symbol BTCUSDT
python manage.py scan_patterns --strategy "Rescue Forces" --timeframe 15m

# Risk Management
python manage.py status --detailed  # Portfolio state
python manage.py positions --live   # Current positions

# Execution
python manage.py technical_stop_buy --capital 100 --strategy "All In"  # DRY-RUN
python manage.py technical_stop_buy --capital 100 --strategy "All In" --live --confirm  # LIVE

# Monitoring
python manage.py monitor_stops --dry-run     # Check without execution
python manage.py monitor_stops --continuous  # 24/7 loop

# Audit
python manage.py operations --open  # Open positions with movements
python manage.py sync_transactions  # Sync from Binance
```

**Tool Use Pattern**:
```xml
<tool_use>
  <tool>bash</tool>
  <command>python manage.py technical_stop_buy --capital 100 --strategy "All In" --dry-run</command>
  <expected_output>
    Execution plan created.
    Plan ID: plan_abc123
    Position size: 0.0667 BTC
    Max risk: $100 (1%)
  </expected_output>
</tool_use>

<reasoning>
  Command executed successfully. Plan created in DRY-RUN mode.
  Position size calculated from technical stop.
  Risk is exactly 1% as expected.
  Ready to present to user for confirmation.
</reasoning>

<user_response>
  "I've created an execution plan for your All In strategy:

  - Position: 0.0667 BTC (~$6,333)
  - Risk: $100 (1% of your capital)
  - Stop: $93,500 (1.58% below entry)

  Would you like to execute this in LIVE mode? (requires --confirm flag)"
</user_response>
```

---

### REST API Integration

**For web/mobile frontends.**

**Key Endpoints**:
```http
POST /api/operations/calculate-size/
Content-Type: application/json

{
  "symbol": "BTCUSDT",
  "entry_price": 95000.0,
  "stop_price": 93500.0,
  "capital": 10000.0
}

Response:
{
  "position_size": 0.0667,
  "position_value": 6333.33,
  "max_risk": 100.0,
  "risk_percent": 1.0
}
```

```http
POST /api/operations/create/
Content-Type: application/json

{
  "symbol": "BTCUSDT",
  "side": "BUY",
  "strategy_id": 1,
  "entry_price": 95000.0,
  "stop_price": 93500.0,
  "mode": "DRY-RUN"
}

Response:
{
  "operation_id": "op_xyz789",
  "order_id": "order_123",
  "status": "FILLED",
  "audit_record_id": "audit_456"
}
```

---

## Multi-Agent Orchestration

### Agent Roles

**Robson Bot uses a multi-agent architecture:**

| Agent | Role | Autonomy | Tools |
|-------|------|----------|-------|
| **Intent Parser** | Understand user requests | Stateless | NLP, intent extraction |
| **Risk Guardian** | Enforce risk limits | Stateless validator | Portfolio state, limits |
| **Execution Planner** | Create execution plans | Stateless | Market data, position sizing |
| **Order Executor** | Execute trades | Semi-autonomous | Exchange API, audit log |
| **Stop Monitor** | 24/7 stop-loss monitoring | Autonomous (pre-approved) | WebSocket, exchange API |
| **Performance Analyst** | Daily P&L reports | Autonomous (reporting) | Database, analytics |

### Agent Communication Protocol

**Message Format** (Internal):
```json
{
  "from_agent": "intent_parser",
  "to_agent": "execution_planner",
  "task": "CREATE_PLAN",
  "payload": {
    "intent": {
      "symbol": "BTCUSDT",
      "side": "BUY",
      "strategy": "All In"
    },
    "context": {
      "user_id": 1,
      "capital": 10000.0
    }
  },
  "correlation_id": "req_abc123",
  "timestamp": "2026-01-20T12:34:56Z"
}
```

### Orchestration Example: Multi-Step Trade

```
User: "Buy BTC and ETH, All In strategy, split capital 60/40"

Orchestrator:
  ↓
1. Intent Parser → Extract 2 intents (BTCUSDT, ETHUSDT)
  ↓
2. Risk Guardian → Check if both positions fit within limits
  ↓
3. Execution Planner (BTC) → Create plan for BTCUSDT (60% capital)
  ↓
4. Execution Planner (ETH) → Create plan for ETHUSDT (40% capital)
  ↓
5. Risk Guardian → Validate combined exposure
  ↓
6. Present both plans to user → User confirms
  ↓
7. Order Executor (BTC) → Execute BTCUSDT order
  ↓
8. Order Executor (ETH) → Execute ETHUSDT order
  ↓
9. Stop Monitor → Activate monitoring for both positions
  ↓
10. Return confirmation with audit trail
```

**Failure Handling**:
- If step 3 fails (BTC plan fails) → Cancel entire workflow
- If step 7 succeeds but step 8 fails → BTC position is open, ETH cancelled (user notified)
- All-or-nothing behavior OPTIONAL (user preference)

---

## Evaluation Metrics

### Skill Performance Metrics

**For each skill, track:**

| Metric | Description | Target |
|--------|-------------|--------|
| **Accuracy** | Correct outputs / Total executions | >95% |
| **Confidence Calibration** | Predicted confidence matches actual accuracy | Correlation >0.8 |
| **Latency** | Time to complete skill | <2s for atomic, <10s for composite |
| **Safety Rate** | Executions without risk violations | 100% |
| **User Confirmation Rate** | User confirms AI suggestions | >80% |

### Agent Performance Metrics

| Metric | Description | Target |
|--------|-------------|--------|
| **Intent Understanding** | Successfully parsed intents / Total requests | >90% |
| **Risk Rejection Rate** | Blocked trades / Total trades | <10% (good filtering) |
| **Stop Accuracy** | Technical stops that match trader expectations | >85% |
| **Execution Success** | Successfully executed orders / Attempted | >99% |
| **Audit Completeness** | Transactions with complete audit trail | 100% |

### Business Metrics

| Metric | Description | Measurement |
|--------|-------------|-------------|
| **Time to Trade** | User intent → Order execution | Target: <60s |
| **AI Assistance Rate** | Trades using AI vs. manual | Growth over time |
| **User Trust** | AI suggestions accepted without modification | >70% |
| **Risk Compliance** | Zero risk limit violations | 100% |
| **Profitability** | Strategies using AI vs. manual strategies | Measure P&L difference |

---

## Conclusion

This **AI Skills Framework** provides a structured approach to building reliable, safe, and effective AI agents for the Robson Bot trading platform.

**Key Takeaways**:
1. ✅ **Skills are composable**: Atomic → Composite → Agentic
2. ✅ **Safety is paramount**: Default to DRY-RUN, validate everything, audit all
3. ✅ **Position sizing is sacred**: Always from technical stop, never arbitrary
4. ✅ **User is in control**: AI calculates, user confirms
5. ✅ **Explainability matters**: Every decision must be traceable

**For AI Engineers**:
- Use this document as a **prompt engineering guide**
- Build skills incrementally (start with atomic, compose later)
- Always validate against safety protocols
- Monitor performance metrics continuously

**For Product/Business**:
- Skills map directly to user features
- Metrics provide clear success criteria
- Multi-agent architecture enables scalability

---

**Related Documentation**:
- [AGENTS.md](AGENTS.md) - Comprehensive AI agent guide
- [AI_WORKFLOW.md](AI_WORKFLOW.md) - AI collaboration guidelines
- [CLAUDE.md](../CLAUDE.md) - Claude Code specific context
- [POSITION-SIZING-GOLDEN-RULE.md](requirements/POSITION-SIZING-GOLDEN-RULE.md) - Position sizing deep-dive
- [AGENTIC-TRADING.md](AGENTIC-TRADING.md) - PLAN → VALIDATE → EXECUTE workflow

---

**Last Updated**: 2026-01-20
**Version**: 1.0
**Maintained by**: RBX Systems AI Team
**License**: Same as project
