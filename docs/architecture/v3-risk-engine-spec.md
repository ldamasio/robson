# ROBSON v3 — RISK ENGINE SPECIFICATION

**Date**: 2026-04-03  
**Status**: APPROVED  
**Classification**: CRITICAL PATH — Financial Safety Component

---

## Role

The Risk Engine is the single component where a bug means financial loss. It is CENTRAL and BLOCKING. All execution flows through the Risk Engine. It is not consulted — it is a mandatory gate. No action proceeds without Risk Engine clearance.

---

## Architecture

```
                EngineAction (proposed)
                       │
                       ▼
              ┌────────────────┐
              │  RISK ENGINE   │
              │                │
              │  Pre-Check     │──── Circuit Breaker State
              │  Hard Limits   │──── Static Thresholds
              │  Dynamic Limits│──── Market Conditions (v3)
              │  Verdict       │
              └───────┬────────┘
                      │
           ┌──────────┴──────────┐
           │                     │
    RiskClearance::        RiskClearance::
    Approved                Denied(reason)
           │                     │
           ▼                     ▼
    GovernedAction          RiskDenied Event
    (proceed to Executor)   (logged, cycle continues)
```

---

## Interface

```rust
pub struct RiskEngine {
    limits: RiskLimits,
    dynamic_config: Option<DynamicRiskConfig>,
    circuit_breaker: CircuitBreakerState,
}

impl RiskEngine {
    /// Pre-check: is the system in a state where any action is allowed?
    pub fn pre_check(&self) -> Result<(), RiskDenial> {
        if self.circuit_breaker.is_open() {
            return Err(RiskDenial::CircuitBreakerOpen {
                level: self.circuit_breaker.level(),
                reason: self.circuit_breaker.reason(),
            });
        }
        Ok(())
    }

    /// Evaluate a specific action against all limits
    pub fn evaluate(
        &self,
        action: &EngineAction,
        portfolio: &PortfolioSnapshot,
        market: &MarketSnapshot,
    ) -> RiskClearance {
        // Check hard limits first (non-overridable)
        if let Some(denial) = self.check_hard_limits(action, portfolio) {
            return RiskClearance::Denied(denial);
        }
        
        // Check soft limits (overridable by operator)
        if let Some(denial) = self.check_soft_limits(action, portfolio) {
            return RiskClearance::Denied(denial);
        }
        
        // Check dynamic limits (v3, fallback to hard limits if unavailable)
        if let Some(ref dynamic) = self.dynamic_config {
            if let Some(denial) = self.check_dynamic_limits(action, portfolio, market, dynamic) {
                return RiskClearance::Denied(denial);
            }
        }
        
        RiskClearance::Approved {
            limits_applied: self.limits.clone(),
            dynamic_adjustments: self.dynamic_config.clone(),
            evaluated_at: Utc::now(),
        }
    }

    /// Post-check: did the completed action approach any limit?
    pub fn post_check(
        &self,
        portfolio: &PortfolioSnapshot,
    ) -> Vec<RiskWarning> {
        let mut warnings = vec![];
        
        if portfolio.daily_pnl_pct < Decimal::new(-15, 1) {  // -1.5%
            warnings.push(RiskWarning::ApproachingDailyLimit {
                current: portfolio.daily_pnl_pct,
                limit: self.limits.max_daily_loss_pct,
            });
        }
        // ... other early warnings
        
        warnings
    }
}
```

---

## Hard Limits (v2.5 — Non-Negotiable)

These limits CANNOT be overridden by the operator. They exist to protect the operator from themselves during emotional or impaired trading.

| Limit | Default | Range | Override | Circuit Breaker? |
|-------|---------|-------|----------|-----------------|
| **Max daily loss** | 3% of capital | 1-5% | NO | YES — triggers L2 at 3%, L3 at 4% |
| **Max monthly drawdown** | 4% of capital | 2-10% | NO | YES — triggers L3 immediately |
| **Max slippage per order** | 5% | 1-10% | NO | Order rejected |
| **Max execution frequency** | 10 orders/minute | 1-60 | NO | Orders queued, excess rejected |
| **Risk per trade (Golden Rule)** | 1% of capital | 0.5-2% | NO | Trade rejected if exceeded |

### Enforcement

```rust
fn check_hard_limits(
    &self,
    action: &EngineAction,
    portfolio: &PortfolioSnapshot,
) -> Option<RiskDenial> {
    match action {
        EngineAction::PlaceEntryOrder { quantity, .. } => {
            // Daily loss check
            if portfolio.daily_pnl_pct <= -self.limits.max_daily_loss_pct {
                return Some(RiskDenial::DailyLossExceeded {
                    current: portfolio.daily_pnl_pct,
                    limit: self.limits.max_daily_loss_pct,
                });
            }
            
            // Monthly drawdown check
            if portfolio.monthly_drawdown_pct >= self.limits.max_monthly_drawdown_pct {
                return Some(RiskDenial::MonthlyDrawdownExceeded {
                    current: portfolio.monthly_drawdown_pct,
                    limit: self.limits.max_monthly_drawdown_pct,
                });
            }
            
            // Per-trade risk check (Golden Rule)
            // Risk = |entry - stop| * quantity
            // Must be <= capital * max_risk_per_trade_pct
            // This is already enforced by position sizing, but double-check here
            
            // Rate limit check
            if portfolio.orders_last_minute >= self.limits.max_orders_per_minute {
                return Some(RiskDenial::RateLimitExceeded);
            }
            
            None
        }
        EngineAction::TriggerExit { .. } => None,  // Exits always allowed
        EngineAction::PanicClose { .. } => None,    // Panic always allowed
        _ => None,
    }
}
```

---

## Soft Limits (v2.5 — Operator-Overridable)

These limits can be overridden by the operator with a `RiskOverride` event. Overrides are logged and time-limited.

| Limit | Default | Override? | Override Expiry |
|-------|---------|-----------|----------------|
| **Max open positions** | 3 | YES | 24h |
| **Max total exposure** | 30% of capital | YES | 24h |
| **Max single position** | 15% of capital | YES | Per-action (one-time) |

### Override Mechanism

```rust
pub struct RiskOverride {
    pub override_id: Ulid,
    pub limit_name: String,
    pub original_value: Decimal,
    pub override_value: Decimal,
    pub reason: String,           // Operator must provide reason
    pub issued_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub operator_confirmed: bool, // Must be explicitly confirmed
}
```

**Rules**:
1. Override cannot exceed 2x the default limit (e.g., max positions can go from 3 to 6, not 3 to 100)
2. Override expires after 24h (or per-action for single-position limit)
3. Override is logged as an immutable event in EventLog
4. Hard limits CANNOT be overridden even with RiskOverride

---

## Circuit Breaker

### States

```rust
pub enum CircuitBreakerState {
    /// Normal operation. All actions allowed.
    Closed,
    
    /// Risk threshold breached. Actions restricted by level.
    Open {
        level: CircuitBreakerLevel,
        triggered_at: DateTime<Utc>,
        trigger_reason: String,
        auto_escalation_at: Option<DateTime<Utc>>,
    },
    
    /// Testing recovery. Limited actions allowed.
    HalfOpen {
        since: DateTime<Utc>,
        previous_level: CircuitBreakerLevel,
    },
}
```

### Escalation Ladder

```
   CLOSED (normal)
      │
      │ daily_loss > 2% OR approaching single-trade limit
      ▼
   L1: BLOCK NEW ORDERS
      │
      │ 30 min without operator ack
      │ OR daily_loss > 3%
      ▼
   L2: REDUCE EXPOSURE (close 50% of largest positions)
      │
      │ 15 min without operator ack
      │ OR daily_loss > 4%
      │ OR monthly_drawdown > 4%
      ▼
   L3: CLOSE ALL POSITIONS (emergency market orders)
      │
      │ Immediate after L3 execution
      ▼
   L4: SYSTEM HALT (no cycles, full audit dump)
      │
      │ Only operator reset with typed confirmation
      ▼
   CLOSED (normal) — after manual reset
```

### Level Details

#### Level 1: Block New Orders

**Trigger**: `daily_pnl_pct < -2%` OR single position approaching 1% loss

**Actions allowed**:
- UpdateTrailingStop (tightening stops is safe)
- TriggerExit (closing positions is safe)
- Operator commands (always allowed)

**Actions blocked**:
- PlaceEntryOrder (no new exposure)

**Auto-escalation**: If no operator acknowledgment within 30 minutes, escalate to L2.

#### Level 2: Reduce Exposure

**Trigger**: Auto-escalation from L1 (30 min timeout) OR `daily_pnl_pct < -3%`

**Automatic action**: Close 50% of total exposure. Selection: largest positions first (highest absolute value).

**Actions allowed**:
- TriggerExit
- Operator commands

**Actions blocked**:
- PlaceEntryOrder
- UpdateTrailingStop (positions being closed)

**Auto-escalation**: If no operator acknowledgment within 15 minutes, escalate to L3.

#### Level 3: Close All

**Trigger**: Auto-escalation from L2 (15 min timeout) OR `daily_pnl_pct < -4%` OR `monthly_drawdown_pct >= 4%` OR operator unreachable for 45 min during L1/L2

**Automatic action**: Market sell ALL open positions immediately.

**Actions allowed**:
- None (system transitions to L4 immediately after close execution)

#### Level 4: Halt

**Trigger**: After L3 execution completes OR operator `panic` command

**Actions**: NONE. No control loop cycles. System is inert.

**Recovery**: Operator must:
1. Connect to system (CLI or UI)
2. Review audit trail of the event that triggered halt
3. Type exact confirmation phrase: `RESET CIRCUIT BREAKER LEVEL 4 ACKNOWLEDGE RISK`
4. System transitions to CLOSED state
5. Full audit event recorded

---

## Dynamic Limits (v3)

### Inputs

| Input | Source | Effect | Staleness Threshold |
|-------|--------|--------|-------------------|
| 20-day realized volatility | Calculated from OHLCV data | High vol -> reduce max position size by up to 50% | 4 hours |
| Funding rate | Binance API | Extreme funding (>0.1%) -> reduce max exposure for same-direction positions | 1 hour |
| Open interest change | Binance API | Rapid OI decrease -> tighten stops (potential liquidation cascade) | 30 minutes |
| Portfolio correlation | Calculated from position returns | Correlated positions -> reduce combined exposure limit | 24 hours |

### Computation

```rust
pub struct DynamicRiskConfig {
    pub volatility_adjustment: Decimal,      // 0.5 to 1.0 multiplier
    pub funding_rate_adjustment: Decimal,    // 0.5 to 1.0 multiplier
    pub correlation_adjustment: Decimal,     // 0.5 to 1.0 multiplier
    pub computed_at: DateTime<Utc>,
    pub data_freshness: HashMap<String, DateTime<Utc>>,
}

impl DynamicRiskConfig {
    pub fn adjusted_max_position_pct(&self, base: Decimal) -> Decimal {
        base * self.volatility_adjustment * self.funding_rate_adjustment
    }
    
    pub fn adjusted_max_exposure_pct(&self, base: Decimal) -> Decimal {
        base * self.correlation_adjustment
    }
    
    pub fn is_stale(&self) -> bool {
        // Any input exceeds its staleness threshold
        self.data_freshness.values().any(|ts| {
            Utc::now() - *ts > Duration::hours(4) // most conservative threshold
        })
    }
}
```

### Fallback

If dynamic computation fails OR data is stale:
1. Log `DynamicLimitFallback` event with reason
2. Revert to hard limits (which are always more conservative)
3. Alert operator: "Dynamic risk limits unavailable — using hard limits"
4. Retry dynamic computation on next cycle

---

## Safety Net (Independent)

The Safety Net is a SEPARATE monitoring process that runs independently of the Risk Engine. It is defense-in-depth.

### Purpose

Detect and close "rogue positions" — positions on Binance that were NOT opened by Robson.

### Operation

```
Every 60 seconds:
1. Query Binance for all open margin positions
2. Compare to Robson's known positions (from RuntimeState)
3. If unknown position found:
   a. Log RoguePositionDetected event
   b. If safety_net_auto_close = true: close the rogue position
   c. Alert operator
4. If known position has unexpected state (wrong quantity, extra orders):
   a. Log PositionDiscrepancy event
   b. Alert operator (do NOT auto-close)
```

### Independence Guarantee

- Safety Net runs as a separate tokio task, not part of the control loop
- Safety Net has its OWN exchange client (separate rate limit budget)
- Safety Net writes to EventLog but does NOT read from it (uses exchange as source of truth)
- If Risk Engine is down, Safety Net still operates
- If Safety Net is down, Risk Engine still operates

---

## Audit Trail

Every risk decision produces an immutable event:

```rust
pub struct RiskDecisionEvent {
    pub cycle_id: Ulid,
    pub action_type: String,           // "PlaceEntryOrder", "TriggerExit", etc.
    pub action_details: Value,          // Full action payload
    
    // Limits at time of evaluation
    pub hard_limits: RiskLimits,
    pub dynamic_adjustments: Option<DynamicRiskConfig>,
    pub effective_limits: RiskLimits,   // hard * dynamic
    
    // Portfolio state at time of evaluation
    pub portfolio_snapshot: PortfolioSnapshot,
    
    // Verdict
    pub verdict: RiskVerdict,           // Approved | Denied(reason)
    pub denied_by: Option<String>,      // Which check denied
    
    // Override (if any)
    pub operator_override: Option<RiskOverride>,
    
    pub timestamp: DateTime<Utc>,
}
```

### Queryable Audit

For compliance and post-mortem analysis:

```sql
-- All denied actions in the last 24h
SELECT * FROM event_log 
WHERE event_type = 'risk_decision' 
AND payload->>'verdict' = 'denied'
AND timestamp > NOW() - INTERVAL '24 hours'
ORDER BY timestamp DESC;

-- All operator overrides
SELECT * FROM event_log
WHERE event_type = 'risk_override'
ORDER BY timestamp DESC;

-- Circuit breaker history
SELECT * FROM event_log
WHERE event_type LIKE 'circuit_breaker_%'
ORDER BY timestamp DESC;
```

---

## Testing Requirements

### Mandatory Coverage

Risk Engine has **100% branch coverage** on all threshold and circuit breaker logic. No exceptions.

### Property-Based Tests

Using `proptest` crate:

```rust
proptest! {
    #[test]
    fn risk_limits_never_violated(
        portfolio in arbitrary_portfolio(),
        action in arbitrary_action(),
        limits in arbitrary_limits(),
    ) {
        let engine = RiskEngine::new(limits.clone());
        let clearance = engine.evaluate(&action, &portfolio, &MarketSnapshot::default());
        
        match clearance {
            RiskClearance::Approved { .. } => {
                // Verify that executing this action would not violate limits
                let new_portfolio = simulate_action(&portfolio, &action);
                prop_assert!(new_portfolio.daily_pnl_pct > -limits.max_daily_loss_pct);
                prop_assert!(new_portfolio.monthly_drawdown_pct < limits.max_monthly_drawdown_pct);
                prop_assert!(new_portfolio.total_exposure_pct <= limits.max_total_exposure_pct);
            }
            RiskClearance::Denied(_) => {
                // Denial is always safe — no assertion needed
            }
        }
    }
}
```

### Specific Test Cases

1. **Boundary test**: Portfolio at exactly 2.99% daily loss. One more trade that would push to 3.01%. Must be DENIED.
2. **Circuit breaker escalation**: Simulate L1 trigger, wait 30 min (simulated), verify auto-escalation to L2.
3. **Override test**: Soft limit denied, operator override, verify action proceeds but audit event contains override.
4. **Hard limit override attempt**: Daily loss at limit, operator attempts override, verify STILL DENIED.
5. **Dynamic fallback**: Stale market data, verify hard limits used instead of dynamic.
6. **Concurrent state**: Two rapid cycles where the first approved action hasn't settled yet. Second cycle must use projected (pessimistic) portfolio state, not stale.

---

## Failure Modes — Definitive Answers

| Scenario | Decision | Rationale |
|----------|----------|-----------|
| Risk Engine process crashes | **HALT ALL ACTIVITY** | Continuing without risk checks in a financial system is equivalent to driving without brakes. The cost of missing trades (opportunity cost) is always less than the cost of uncontrolled losses. |
| Dynamic limits computed from stale data | **Revert to hard limits** | Hard limits are conservative by design. Stale dynamic data could reflect market conditions that no longer exist. |
| Operator overrides a risk constraint | **Allowed for soft limits, BLOCKED for hard limits** | Hard limits protect the operator from emotional/impaired decisions. Soft limits allow flexibility for informed decisions. |
| Risk Engine and Safety Net disagree about position state | **Both operate independently** | Defense in depth. If one is wrong, the other provides coverage. Discrepancy is logged for investigation. |
| Risk Engine evaluation takes >200ms | **DENY the action** | Slow Risk Engine suggests system overload or data access issues. Safe default is to deny. |
