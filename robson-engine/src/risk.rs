//! Risk Gate: Pre-trade approval checks (ADR-0024).
//!
//! The RiskGate evaluates proposed trades against portfolio-level risk limits.
//! It answers: "given the current portfolio state, should this trade be
//! allowed?"
//!
//! # Checks Performed (ADR-0024)
//!
//! 1. Duplicate position (same symbol+side) — operational constraint
//! 2. Dynamic slot exhaustion (replaces static max_open_positions)
//! 3. 1x margin availability (stop-derived size must fit available capital)
//! 4. Monthly drawdown hard limit (from TradingPolicy)
//!
//! # Eliminated by ADR-0024
//!
//! - max_open_positions → dynamic slot calculation
//! - max_total_exposure_pct → physical capital bound (1x margin availability)
//! - max_single_position_pct → physical capital bound (1x margin availability)
//! - daily_loss_limit_pct → removed: the policy is 1% max loss per operation
//!   plus the 4% monthly drawdown; there is no per-day loss budget
//!
//! # Design
//!
//! - Pure computation (no I/O)
//! - Called by Engine before producing entry actions
//! - Rejection emits RiskCheckFailed event for audit

use robson_domain::TradingPolicy;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use tracing::debug;

// =============================================================================
// Risk Limits (legacy compatibility — fields no longer enforced by ADR-0024)
// =============================================================================

/// Portfolio-level risk limits.
///
/// Legacy compatibility fields (max_open_positions, max_total_exposure_pct,
/// max_single_position_pct) are preserved for struct compatibility but are no
/// longer enforced by the risk gate per ADR-0024. Active risk enforcement uses
/// `TradingPolicy` for dynamic slot calculation and drawdown limits.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RiskLimits {
    /// Legacy: no longer enforced (ADR-0024 uses dynamic slot calculation).
    pub max_open_positions: usize,

    /// Legacy: no longer enforced (ADR-0024 relies on exchange physical
    /// bounds).
    pub max_total_exposure_pct: Decimal,

    /// Legacy: no longer enforced (ADR-0024 relies on exchange physical
    /// bounds).
    pub max_single_position_pct: Decimal,

    /// Monthly drawdown limit as percentage of capital (sourced from
    /// TradingPolicy).
    pub max_monthly_drawdown_pct: Decimal,
}

impl Default for RiskLimits {
    fn default() -> Self {
        Self {
            max_open_positions: 3,
            max_total_exposure_pct: Decimal::from(30),
            max_single_position_pct: Decimal::from(15),
            max_monthly_drawdown_pct: Decimal::from(4),
        }
    }
}

impl RiskLimits {
    /// Create risk limits with custom values (legacy compatibility).
    pub fn new(
        max_open_positions: usize,
        max_total_exposure_pct: Decimal,
        max_single_position_pct: Decimal,
    ) -> Self {
        Self {
            max_open_positions,
            max_total_exposure_pct,
            max_single_position_pct,
            max_monthly_drawdown_pct: Decimal::from(4),
        }
    }
}

// =============================================================================
// Risk Context (derived from events/positions)
// =============================================================================

/// Minimal position info needed for risk checks
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PositionSummary {
    /// Position identifier
    pub position_id: uuid::Uuid,
    /// Trading pair symbol
    pub symbol: String,
    /// Position direction (lowercase: "long" or "short")
    pub side: String,
    /// Notional value (quantity × price)
    pub notional_value: Decimal,
    /// Initial margin (notional / leverage)
    pub initial_margin: Decimal,
    /// Unrealized PnL
    pub unrealized_pnl: Decimal,
    /// Entry price (for latent risk calculation)
    pub entry_price: Decimal,
    /// Quantity (for latent risk calculation)
    pub quantity: Decimal,
    /// Current stop price (for latent risk calculation)
    pub current_stop: Decimal,
}

/// Snapshot of current portfolio risk state
///
/// Derived from events/positions, not stored directly.
/// Built by caller (PositionManager) before calling engine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RiskContext {
    /// Current available capital
    pub capital: Decimal,
    /// Currently open positions
    pub open_positions: Vec<PositionSummary>,
    /// Total notional exposure across all positions
    pub total_notional_exposure: Decimal,
    /// Monthly realized PnL (v3 policy: halt at -4%)
    pub monthly_realized_pnl: Decimal,
    /// Monthly unrealized PnL
    pub monthly_unrealized_pnl: Decimal,
    /// Sum of absolute losses from closed positions this month (ADR-0024 slot
    /// calc). Wins do NOT offset this value. Used exclusively by
    /// `realized_loss_abs()`.
    pub monthly_realized_loss: Decimal,
}

impl RiskContext {
    /// Create an empty context with just capital
    pub fn new(capital: Decimal) -> Self {
        Self {
            capital,
            open_positions: vec![],
            total_notional_exposure: Decimal::ZERO,
            monthly_realized_pnl: Decimal::ZERO,
            monthly_unrealized_pnl: Decimal::ZERO,
            monthly_realized_loss: Decimal::ZERO,
        }
    }

    /// Create context with positions
    pub fn with_positions(capital: Decimal, open_positions: Vec<PositionSummary>) -> Self {
        let total_notional_exposure = open_positions.iter().map(|p| p.notional_value).sum();

        Self {
            capital,
            open_positions,
            total_notional_exposure,
            monthly_realized_pnl: Decimal::ZERO,
            monthly_unrealized_pnl: Decimal::ZERO,
            monthly_realized_loss: Decimal::ZERO,
        }
    }

    /// Create context with positions and monthly PnL
    pub fn with_monthly_pnl(
        capital: Decimal,
        open_positions: Vec<PositionSummary>,
        monthly_realized_pnl: Decimal,
        monthly_unrealized_pnl: Decimal,
    ) -> Self {
        let monthly_realized_loss = if monthly_realized_pnl.is_sign_negative() {
            monthly_realized_pnl.abs()
        } else {
            Decimal::ZERO
        };
        let total_notional_exposure = open_positions.iter().map(|p| p.notional_value).sum();

        Self {
            capital,
            open_positions,
            total_notional_exposure,
            monthly_realized_pnl,
            monthly_unrealized_pnl,
            monthly_realized_loss,
        }
    }

    /// Create context with positions, monthly PnL, and the authoritative
    /// monthly realized loss (ADR-0024: wins do not offset losses).
    pub fn with_monthly_state(
        capital: Decimal,
        open_positions: Vec<PositionSummary>,
        monthly_realized_pnl: Decimal,
        monthly_unrealized_pnl: Decimal,
        monthly_realized_loss: Decimal,
    ) -> Self {
        let total_notional_exposure = open_positions.iter().map(|p| p.notional_value).sum();

        Self {
            capital,
            open_positions,
            total_notional_exposure,
            monthly_realized_pnl,
            monthly_unrealized_pnl,
            monthly_realized_loss,
        }
    }

    /// Count open positions
    pub fn open_position_count(&self) -> usize {
        self.open_positions.len()
    }

    /// Calculate total monthly PnL (realized + unrealized)
    pub fn total_monthly_pnl(&self) -> Decimal {
        self.monthly_realized_pnl + self.monthly_unrealized_pnl
    }


    /// Check if there's an existing position with same symbol and side
    pub fn has_duplicate_position(&self, symbol: &str, side: &str) -> bool {
        self.open_positions.iter().any(|p| p.symbol == symbol && p.side == side)
    }

    /// Sum latent risk across all open positions (ADR-0024 Decision 5).
    ///
    /// For LONG:  max(0, (entry - stop) × qty)
    /// For SHORT: max(0, (stop - entry) × qty)
    /// Unknown side contributes zero.
    pub fn latent_risk_sum(&self) -> Decimal {
        self.open_positions
            .iter()
            .map(|p| {
                let risk = match p.side.to_lowercase().as_str() {
                    "long" => (p.entry_price - p.current_stop) * p.quantity,
                    "short" => (p.current_stop - p.entry_price) * p.quantity,
                    _ => Decimal::ZERO,
                };
                risk.max(Decimal::ZERO)
            })
            .sum()
    }

    /// Absolute realized loss for the current month (ADR-0024).
    ///
    /// Returns the sum of absolute losses from closed positions this month.
    /// Wins do NOT offset losses — this is the budget consumed by losing
    /// trades.
    pub fn realized_loss_abs(&self) -> Decimal {
        self.monthly_realized_loss
    }

    /// Dynamic slot count via TradingPolicy (ADR-0024 Decision 5).
    pub fn slots_available(&self, policy: &TradingPolicy, capital_base: Decimal) -> u32 {
        policy.slots_available(capital_base, self.realized_loss_abs(), self.latent_risk_sum())
    }
}

// =============================================================================
// Proposed Trade
// =============================================================================

/// The proposed trade to be evaluated
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProposedTrade {
    /// Trading pair symbol
    pub symbol: String,
    /// Position direction
    pub side: String,
    /// Position quantity
    pub quantity: Decimal,
    /// Entry price
    pub entry_price: Decimal,
    /// Notional value (quantity × entry_price)
    pub notional_value: Decimal,
    /// Initial margin (notional / leverage)
    pub initial_margin: Decimal,
    /// Planned worst-case loss if the stop is hit, cost-priced per ADR-0039
    /// (stop distance + executable-stop buffer + gap allowance + round-trip
    /// fees, × quantity). The gate charges this against the monthly budget
    /// (ADR-0043). Zero means unpriced — the full 1% per-trade cap is
    /// reserved instead.
    #[serde(default)]
    pub planned_risk: Decimal,
}

// =============================================================================
// Risk Verdict
// =============================================================================

/// Which risk check failed
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RiskCheck {
    /// Too many open positions (legacy — no longer emitted by ADR-0024 gate)
    MaxOpenPositions,
    /// Total exposure exceeds limit (legacy — no longer emitted by ADR-0024
    /// gate)
    TotalExposure,
    /// Single position too large (legacy — no longer emitted by ADR-0024 gate)
    SinglePositionConcentration,
    /// Not enough margin available
    InsufficientMargin,
    /// Monthly drawdown limit exceeded (v3: 4% → MonthlyHalt)
    MonthlyDrawdown,
    /// Daily loss limit exceeded (legacy — no longer emitted; the policy is
    /// 1% max loss per operation, with no per-day loss budget)
    DailyLossLimit,
    /// Already have position on same symbol+side
    DuplicatePosition,
    /// Risk engine did not return before the runtime safety timeout.
    RiskEngineTimeout,
    /// Remaining monthly budget cannot absorb this trade's planned risk
    /// (ADR-0043). Unlike `MonthlyDrawdown`, budget remains — a smaller
    /// trade may still fit, so this denial must NOT trigger MonthlyHalt.
    RiskBudgetInsufficient,
}

impl RiskCheck {
    /// Get human-readable name
    pub fn name(&self) -> &'static str {
        match self {
            RiskCheck::MaxOpenPositions => "max_open_positions",
            RiskCheck::TotalExposure => "total_exposure",
            RiskCheck::SinglePositionConcentration => "single_position_concentration",
            RiskCheck::InsufficientMargin => "insufficient_margin",
            RiskCheck::MonthlyDrawdown => "monthly_drawdown",
            RiskCheck::DailyLossLimit => "daily_loss_limit",
            RiskCheck::DuplicatePosition => "duplicate_position",
            RiskCheck::RiskEngineTimeout => "risk_engine_timeout",
            RiskCheck::RiskBudgetInsufficient => "risk_budget_insufficient",
        }
    }
}

/// Result of risk evaluation
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum RiskVerdict {
    /// Trade approved, proceed with execution
    Approved,
    /// Trade rejected, do not execute
    Rejected {
        /// Which check failed
        check: RiskCheck,
        /// Human-readable reason
        reason: String,
    },
}

// =============================================================================
// Risk Gate
// =============================================================================

/// The pre-trade approval gate (ADR-0024).
///
/// Evaluates proposed trades against TradingPolicy. Pure computation, no I/O.
#[derive(Debug, Clone)]
pub struct RiskGate {
    limits: RiskLimits,
    policy: TradingPolicy,
}

impl RiskGate {
    /// Create a new RiskGate with default limits and policy
    pub fn new() -> Self {
        Self {
            limits: RiskLimits::default(),
            policy: TradingPolicy::default(),
        }
    }

    /// Create a RiskGate with custom limits (legacy compatibility)
    pub fn with_limits(limits: RiskLimits) -> Self {
        Self { limits, policy: TradingPolicy::default() }
    }

    /// Create a RiskGate with a specific TradingPolicy (ADR-0024)
    pub fn with_policy(policy: TradingPolicy) -> Self {
        Self { limits: RiskLimits::default(), policy }
    }

    /// Get the current limits (legacy compatibility)
    pub fn limits(&self) -> &RiskLimits {
        &self.limits
    }

    /// Get the current policy
    pub fn policy(&self) -> &TradingPolicy {
        &self.policy
    }

    /// Evaluate a proposed trade against current risk context (ADR-0024).
    pub fn evaluate(&self, proposed: &ProposedTrade, context: &RiskContext) -> RiskVerdict {
        // 1. Check duplicate position (same symbol + side)
        if context.has_duplicate_position(&proposed.symbol, &proposed.side) {
            debug!(
                symbol = %proposed.symbol,
                side = %proposed.side,
                "Risk check failed: duplicate position"
            );
            return RiskVerdict::Rejected {
                check: RiskCheck::DuplicatePosition,
                reason: format!("Already have {} position on {}", proposed.side, proposed.symbol),
            };
        }

        // 2. Check physical capital bound. At 1x, stop-derived notional must fit
        // available margin; tight stops may create a valid 1% loss amount but an
        // invalid position size for the account.
        if proposed.initial_margin > context.capital {
            debug!(
                symbol = %proposed.symbol,
                side = %proposed.side,
                initial_margin = %proposed.initial_margin,
                capital = %context.capital,
                "Risk check failed: insufficient margin for 1x position"
            );
            return RiskVerdict::Rejected {
                check: RiskCheck::InsufficientMargin,
                reason: format!(
                    "Initial margin {} exceeds available capital {} at 1x leverage",
                    proposed.initial_margin, context.capital
                ),
            };
        }

        // 3. Check monthly drawdown hard limit (ADR-0024: sourced from policy)
        let monthly_pnl = context.total_monthly_pnl();
        let monthly_loss_limit =
            context.capital * self.policy.max_monthly_drawdown_pct / Decimal::from(100);
        if monthly_pnl <= -monthly_loss_limit {
            debug!(
                monthly_pnl = %monthly_pnl,
                limit = %monthly_loss_limit,
                "Risk check failed: monthly drawdown limit (MonthlyHalt)"
            );
            return RiskVerdict::Rejected {
                check: RiskCheck::MonthlyDrawdown,
                reason: format!(
                    "Monthly P&L {} has exceeded drawdown limit of -{}% (MonthlyHalt triggered)",
                    monthly_pnl, self.policy.max_monthly_drawdown_pct
                ),
            };
        }

        // 4. Budget-metered admission (ADR-0043: replaces the ADR-0024 slot
        //    reservation at the full 1% cap). The trade is charged its planned
        //    worst-case loss; the 1% per-trade cap still bounds any single
        //    trade, and the 4% monthly budget stays the hard invariant.
        let capital_base = context.capital; // MIG-v3#11 approximation; MIG-v3#12 persists real capital base.
        let realized_loss = context.realized_loss_abs();
        let latent_risk = context.latent_risk_sum();
        let remaining = self.policy.remaining_budget(capital_base, realized_loss, latent_risk);

        if remaining <= Decimal::ZERO {
            debug!(
                capital_base = %capital_base,
                realized_loss = %realized_loss,
                latent_risk = %latent_risk,
                "Risk check failed: monthly risk budget exhausted"
            );
            return RiskVerdict::Rejected {
                check: RiskCheck::MonthlyDrawdown,
                reason: format!(
                    "Monthly risk budget exhausted (capital={}, realized_loss={}, latent_risk={})",
                    capital_base, realized_loss, latent_risk
                ),
            };
        }

        if !self.policy.can_admit(capital_base, realized_loss, latent_risk, proposed.planned_risk)
        {
            let cap = self.policy.risk_per_trade_amount(capital_base);
            let charge = if proposed.planned_risk <= Decimal::ZERO {
                cap
            } else {
                proposed.planned_risk
            };
            debug!(
                capital_base = %capital_base,
                remaining_budget = %remaining,
                planned_risk = %proposed.planned_risk,
                charged_risk = %charge,
                per_trade_cap = %cap,
                "Risk check failed: planned risk does not fit remaining monthly budget"
            );
            return RiskVerdict::Rejected {
                check: RiskCheck::RiskBudgetInsufficient,
                reason: format!(
                    "Planned risk {} does not fit remaining monthly budget {} (per-trade cap {})",
                    charge, remaining, cap
                ),
            };
        }

        debug!(
            symbol = %proposed.symbol,
            side = %proposed.side,
            notional = %proposed.notional_value,
            planned_risk = %proposed.planned_risk,
            remaining_budget = %remaining,
            guaranteed_full_cap_slots = context.slots_available(&self.policy, capital_base),
            "Risk check passed"
        );

        RiskVerdict::Approved
    }
}

impl Default for RiskGate {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use rust_decimal_macros::dec;

    use super::*;

    fn sample_context() -> RiskContext {
        RiskContext::new(dec!(10000))
    }

    fn sample_proposed() -> ProposedTrade {
        ProposedTrade {
            symbol: "BTCUSDT".to_string(),
            side: "long".to_string(),
            quantity: dec!(0.02),
            entry_price: dec!(50000),
            notional_value: dec!(1000),
            initial_margin: dec!(100),
            // Full 1% cap of the $10k sample context — worst-case pricing.
            planned_risk: dec!(100),
        }
    }

    fn summary_with_stop(
        symbol: &str,
        side: &str,
        entry: Decimal,
        stop: Decimal,
        qty: Decimal,
    ) -> PositionSummary {
        PositionSummary {
            position_id: uuid::Uuid::nil(),
            symbol: symbol.to_string(),
            side: side.to_string(),
            notional_value: qty * entry,
            initial_margin: qty * entry,
            unrealized_pnl: Decimal::ZERO,
            entry_price: entry,
            quantity: qty,
            current_stop: stop,
        }
    }

    #[test]
    fn test_risk_gate_approves_normal_trade() {
        let gate = RiskGate::new();
        let context = sample_context();
        let proposed = sample_proposed();

        let verdict = gate.evaluate(&proposed, &context);
        assert_eq!(verdict, RiskVerdict::Approved);
    }

    #[test]
    fn test_risk_gate_rejects_duplicate_position() {
        let gate = RiskGate::new();
        let context = RiskContext::with_positions(dec!(10000), vec![PositionSummary {
            position_id: uuid::Uuid::nil(),
            symbol: "BTCUSDT".to_string(),
            side: "long".to_string(),
            notional_value: dec!(1000),
            initial_margin: dec!(100),
            unrealized_pnl: dec!(0),
            entry_price: dec!(50000),
            quantity: dec!(0.02),
            current_stop: dec!(48000),
        }]);
        let proposed = sample_proposed();

        let verdict = gate.evaluate(&proposed, &context);
        assert!(matches!(verdict, RiskVerdict::Rejected {
            check: RiskCheck::DuplicatePosition,
            ..
        }));
    }

    #[test]
    fn test_risk_gate_rejects_stop_derived_size_that_exceeds_1x_margin() {
        let gate = RiskGate::new();
        let context = RiskContext::new(dec!(352));
        let proposed = ProposedTrade {
            symbol: "BTCUSDT".to_string(),
            side: "long".to_string(),
            quantity: dec!(0.0586666666666666666666666667),
            entry_price: dec!(60000),
            notional_value: dec!(3520.000000000000000000000002),
            initial_margin: dec!(3520.000000000000000000000002),
            planned_risk: dec!(3.52),
        };

        let verdict = gate.evaluate(&proposed, &context);
        assert!(matches!(verdict, RiskVerdict::Rejected {
            check: RiskCheck::InsufficientMargin,
            ..
        }));
    }

    #[test]
    fn test_risk_gate_rejects_monthly_drawdown() {
        let gate = RiskGate::new();
        let context = RiskContext {
            capital: dec!(10000),
            open_positions: vec![],
            total_notional_exposure: Decimal::ZERO,
            monthly_realized_pnl: dec!(-350),
            monthly_unrealized_pnl: dec!(-100),
            monthly_realized_loss: dec!(350),
        };
        let proposed = sample_proposed();

        let verdict = gate.evaluate(&proposed, &context);
        assert!(matches!(verdict, RiskVerdict::Rejected {
            check: RiskCheck::MonthlyDrawdown,
            ..
        }));
    }

    #[test]
    fn test_risk_gate_allows_within_monthly_drawdown() {
        let gate = RiskGate::new();
        let context = RiskContext {
            capital: dec!(10000),
            open_positions: vec![],
            total_notional_exposure: Decimal::ZERO,
            monthly_realized_pnl: dec!(-300),
            monthly_unrealized_pnl: dec!(0),
            monthly_realized_loss: dec!(300),
        };
        let proposed = sample_proposed();

        let verdict = gate.evaluate(&proposed, &context);
        assert_eq!(verdict, RiskVerdict::Approved);
    }

    #[test]
    fn test_risk_gate_allows_at_3_pct_monthly_drawdown() {
        let gate = RiskGate::new();
        let context = RiskContext {
            capital: dec!(10000),
            open_positions: vec![],
            total_notional_exposure: Decimal::ZERO,
            monthly_realized_pnl: dec!(-300),
            monthly_unrealized_pnl: dec!(0),
            monthly_realized_loss: dec!(300),
        };
        let proposed = sample_proposed();

        let verdict = gate.evaluate(&proposed, &context);
        assert_eq!(
            verdict,
            RiskVerdict::Approved,
            "3.00% monthly loss with 1 slot remaining must be allowed"
        );
    }

    #[test]
    fn test_risk_gate_denies_full_cap_trade_in_budget_tail_without_halt() {
        // 3.99% realized loss (399 out of 400 budget) leaves $1. A worst-case
        // $100 trade does not fit, but budget remains — ADR-0043: deny with
        // RiskBudgetInsufficient, NOT MonthlyDrawdown (no MonthlyHalt).
        let gate = RiskGate::new();
        let context = RiskContext {
            capital: dec!(10000),
            open_positions: vec![],
            total_notional_exposure: Decimal::ZERO,
            monthly_realized_pnl: dec!(-399),
            monthly_unrealized_pnl: dec!(0),
            monthly_realized_loss: dec!(399),
        };
        let proposed = sample_proposed();

        let verdict = gate.evaluate(&proposed, &context);
        assert!(
            matches!(
                verdict,
                RiskVerdict::Rejected { check: RiskCheck::RiskBudgetInsufficient, .. }
            ),
            "full-cap trade in the budget tail must deny without triggering MonthlyHalt"
        );
    }

    #[test]
    fn test_risk_gate_admits_small_planned_risk_in_budget_tail() {
        // ADR-0043: saved risk becomes an extra operation. With $1 of budget
        // left, a trade whose planned worst-case loss is $0.90 is admitted.
        let gate = RiskGate::new();
        let context = RiskContext {
            capital: dec!(10000),
            open_positions: vec![],
            total_notional_exposure: Decimal::ZERO,
            monthly_realized_pnl: dec!(-399),
            monthly_unrealized_pnl: dec!(0),
            monthly_realized_loss: dec!(399),
        };
        let proposed = ProposedTrade { planned_risk: dec!(0.90), ..sample_proposed() };

        let verdict = gate.evaluate(&proposed, &context);
        assert_eq!(
            verdict,
            RiskVerdict::Approved,
            "planned risk below remaining budget must be admitted (extra operation)"
        );
    }

    #[test]
    fn test_risk_gate_rejects_planned_risk_above_per_trade_cap() {
        // The 1% per-trade cap survives ADR-0043: planned risk above the cap
        // is never admitted, even with a fresh monthly budget.
        let gate = RiskGate::new();
        let context = sample_context();
        let proposed = ProposedTrade { planned_risk: dec!(101), ..sample_proposed() };

        let verdict = gate.evaluate(&proposed, &context);
        assert!(
            matches!(
                verdict,
                RiskVerdict::Rejected { check: RiskCheck::RiskBudgetInsufficient, .. }
            ),
            "planned risk above the 1% per-trade cap must be rejected"
        );
    }

    #[test]
    fn test_risk_gate_unpriced_trade_reserves_full_cap() {
        // planned_risk == 0 (unpriced, e.g. legacy caller) falls back to
        // reserving the full 1% cap — the pre-ADR-0043 behavior.
        let gate = RiskGate::new();
        let tail_context = RiskContext {
            capital: dec!(10000),
            open_positions: vec![],
            total_notional_exposure: Decimal::ZERO,
            monthly_realized_pnl: dec!(-350),
            monthly_unrealized_pnl: dec!(0),
            monthly_realized_loss: dec!(350),
        };
        let unpriced = ProposedTrade { planned_risk: Decimal::ZERO, ..sample_proposed() };

        // Remaining $50 < $100 full cap → denied.
        let verdict = gate.evaluate(&unpriced, &tail_context);
        assert!(
            matches!(
                verdict,
                RiskVerdict::Rejected { check: RiskCheck::RiskBudgetInsufficient, .. }
            ),
            "unpriced trade must reserve the full cap and be denied in the tail"
        );

        // Fresh budget → approved.
        let verdict = gate.evaluate(&unpriced, &sample_context());
        assert_eq!(verdict, RiskVerdict::Approved);
    }

    #[test]
    fn test_risk_gate_blocks_at_exactly_4_pct_monthly_drawdown() {
        let gate = RiskGate::new();
        let context = RiskContext {
            capital: dec!(10000),
            open_positions: vec![],
            total_notional_exposure: Decimal::ZERO,
            monthly_realized_pnl: dec!(-400),
            monthly_unrealized_pnl: dec!(0),
            monthly_realized_loss: dec!(400),
        };
        let proposed = sample_proposed();

        let verdict = gate.evaluate(&proposed, &context);
        assert!(
            matches!(verdict, RiskVerdict::Rejected { check: RiskCheck::MonthlyDrawdown, .. }),
            "exactly 4.00% monthly loss must be blocked"
        );
    }

    #[test]
    fn test_risk_gate_allows_same_symbol_opposite_side() {
        let gate = RiskGate::new();
        let context = RiskContext::with_positions(dec!(10000), vec![PositionSummary {
            position_id: uuid::Uuid::nil(),
            symbol: "BTCUSDT".to_string(),
            side: "short".to_string(),
            notional_value: dec!(1000),
            initial_margin: dec!(100),
            unrealized_pnl: dec!(0),
            entry_price: dec!(50000),
            quantity: dec!(0.02),
            current_stop: dec!(52000),
        }]);
        let proposed = sample_proposed();

        let verdict = gate.evaluate(&proposed, &context);
        assert_eq!(verdict, RiskVerdict::Approved);
    }

    // =========================================================================
    // No per-day loss budget (policy: 1% max loss per operation)
    // =========================================================================

    #[test]
    fn test_risk_gate_has_no_daily_loss_limit() {
        // A loss larger than 1% of capital realized within a single day must
        // NOT block the next entry while the monthly budget still has room.
        // (Regression: a phantom 1%/day limit denied entries after one stop-out.)
        let gate = RiskGate::new();
        let context = RiskContext {
            capital: dec!(10000),
            open_positions: vec![],
            total_notional_exposure: Decimal::ZERO,
            monthly_realized_pnl: dec!(-120),
            monthly_unrealized_pnl: Decimal::ZERO,
            monthly_realized_loss: dec!(120),
        };
        let proposed = sample_proposed();

        let verdict = gate.evaluate(&proposed, &context);
        assert_eq!(
            verdict,
            RiskVerdict::Approved,
            "1.2% loss in one day must be allowed while the monthly budget has room"
        );
    }

    // =========================================================================
    // Latent risk and slot calculation tests (ADR-0024)
    // =========================================================================

    #[test]
    fn test_latent_risk_sum_long() {
        let ctx = RiskContext::with_positions(dec!(10000), vec![summary_with_stop(
            "BTCUSDT",
            "long",
            dec!(80000),
            dec!(78400),
            dec!(0.001),
        )]);
        // LONG: (80000 - 78400) * 0.001 = 1.6
        assert_eq!(ctx.latent_risk_sum(), dec!(1.6));
    }

    #[test]
    fn test_latent_risk_sum_short() {
        let ctx = RiskContext::with_positions(dec!(10000), vec![summary_with_stop(
            "BTCUSDT",
            "short",
            dec!(80000),
            dec!(81600),
            dec!(0.001),
        )]);
        // SHORT: (81600 - 80000) * 0.001 = 1.6
        assert_eq!(ctx.latent_risk_sum(), dec!(1.6));
    }

    #[test]
    fn test_latent_risk_breakeven_stop() {
        // Stop at entry → risk = 0 (breakeven)
        let ctx = RiskContext::with_positions(dec!(10000), vec![summary_with_stop(
            "BTCUSDT",
            "long",
            dec!(80000),
            dec!(80000),
            dec!(0.001),
        )]);
        assert_eq!(ctx.latent_risk_sum(), dec!(0));
    }

    #[test]
    fn test_latent_risk_stop_beyond_entry() {
        // LONG with stop above entry → max(0, negative) = 0
        let ctx = RiskContext::with_positions(dec!(10000), vec![summary_with_stop(
            "BTCUSDT",
            "long",
            dec!(80000),
            dec!(81000),
            dec!(0.001),
        )]);
        assert_eq!(ctx.latent_risk_sum(), dec!(0));
    }

    #[test]
    fn test_realized_loss_abs_negative() {
        let ctx = RiskContext {
            capital: dec!(10000),
            open_positions: vec![],
            total_notional_exposure: Decimal::ZERO,
            monthly_realized_pnl: dec!(-150),
            monthly_unrealized_pnl: Decimal::ZERO,
            monthly_realized_loss: dec!(150),
        };
        assert_eq!(ctx.realized_loss_abs(), dec!(150));
    }

    #[test]
    fn test_realized_loss_abs_positive() {
        let ctx = RiskContext {
            capital: dec!(10000),
            open_positions: vec![],
            total_notional_exposure: Decimal::ZERO,
            monthly_realized_pnl: dec!(200),
            monthly_unrealized_pnl: Decimal::ZERO,
            monthly_realized_loss: Decimal::ZERO,
        };
        assert_eq!(ctx.realized_loss_abs(), dec!(0));
    }

    #[test]
    fn test_realized_loss_is_not_offset_by_wins() {
        let policy = TradingPolicy::default();
        let ctx = RiskContext {
            capital: dec!(10000),
            open_positions: vec![],
            total_notional_exposure: Decimal::ZERO,
            // One -100 loser and one +100 winner net to zero PnL.
            monthly_realized_pnl: Decimal::ZERO,
            monthly_unrealized_pnl: Decimal::ZERO,
            // ADR-0024 slots consume the losing trade only; wins do not offset it.
            monthly_realized_loss: dec!(100),
        };

        assert_eq!(ctx.realized_loss_abs(), dec!(100));
        assert_eq!(ctx.slots_available(&policy, dec!(10000)), 3);
    }

    #[test]
    fn test_slots_available_via_context() {
        let policy = TradingPolicy::default();
        // capital=100, budget=4, risk=1, no loss, no latent → 4 slots
        let ctx = RiskContext::new(dec!(100));
        assert_eq!(ctx.slots_available(&policy, dec!(100)), 4);
    }

    // =========================================================================
    // ADR-0024 manual concept: policy-compliant large notional
    // =========================================================================

    #[test]
    fn test_approves_large_notional_with_available_slots() {
        // capital = 100, existing BTC long entry = 80000, stop = 78400, qty = 0.000625
        // latent risk = (80000 - 78400) * 0.000625 = 1
        // slots_available = floor((4 - 0 - 1) / 1) = 3
        // proposed notional = 50 → 50% of capital, but policy allows it
        let gate = RiskGate::new();
        let existing = summary_with_stop(
            "ETHUSDT", // different symbol to avoid duplicate check
            "long",
            dec!(80000),
            dec!(78400),
            dec!(0.000625),
        );
        let context = RiskContext::with_positions(dec!(100), vec![existing]);

        let proposed = ProposedTrade {
            symbol: "BTCUSDT".to_string(),
            side: "long".to_string(),
            quantity: dec!(0.000625),
            entry_price: dec!(80000),
            notional_value: dec!(50), /* 50% of capital — would have been rejected by old
                                       * SinglePositionConcentration */
            initial_margin: dec!(50),
            planned_risk: dec!(1),
        };

        let verdict = gate.evaluate(&proposed, &context);
        assert_eq!(
            verdict,
            RiskVerdict::Approved,
            "50% notional must be approved with available slots"
        );
    }

    #[test]
    fn test_slots_exhausted_uses_monthly_drawdown_check() {
        let gate = RiskGate::new();
        // capital=100, budget=4, risk=1 per slot
        // 4 positions each with latent risk 1 → remaining = 4 - 0 - 4 = 0 → no slots
        let positions: Vec<PositionSummary> = (0..4)
            .map(|i| {
                summary_with_stop(
                    &format!("SYM{}USDT", i),
                    "long",
                    dec!(80000),
                    dec!(78400), // 2% stop → (80000-78400)*0.000625 = 1
                    dec!(0.000625),
                )
            })
            .collect();

        let context = RiskContext::with_positions(dec!(100), positions);
        let proposed = ProposedTrade {
            symbol: "NEWUSDT".to_string(),
            side: "long".to_string(),
            quantity: dec!(0.001),
            entry_price: dec!(50000),
            notional_value: dec!(50),
            initial_margin: dec!(50),
            planned_risk: dec!(1),
        };

        let verdict = gate.evaluate(&proposed, &context);
        assert!(
            matches!(verdict, RiskVerdict::Rejected { check: RiskCheck::MonthlyDrawdown, .. }),
            "exhausted budget (remaining = 0) must reject with MonthlyDrawdown check"
        );
    }
}
