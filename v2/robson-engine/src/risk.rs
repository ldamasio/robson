//! Risk Gate: Pre-trade approval checks
//!
//! The RiskGate evaluates proposed trades against portfolio-level risk limits.
//! It answers: "given the current portfolio state, should this trade be allowed?"
//!
//! # Checks Performed
//!
//! 1. Max open positions not exceeded
//! 2. Total exposure (aggregate notional) within limit
//! 3. Single position concentration within limit
//! 4. No duplicate position on same symbol+side
//! 5. Daily loss circuit breaker not triggered
//!
//! # Design
//!
//! - Pure computation (no I/O)
//! - Called by Engine before producing entry actions
//! - Rejection emits RiskCheckFailed event for audit

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use tracing::debug;

// =============================================================================
// Risk Limits (static configuration)
// =============================================================================

/// Portfolio-level risk limits (configured at startup, static)
///
/// These are the guardrails that prevent excessive risk exposure.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RiskLimits {
    /// Maximum number of simultaneous open positions
    /// Default: 3 (with 1% risk per trade and 10x leverage = ~30% margin)
    pub max_open_positions: usize,

    /// Maximum total notional exposure as percentage of capital
    /// Default: 30% (e.g., $3000 exposure on $10000 capital)
    pub max_total_exposure_pct: Decimal,

    /// Maximum single position size as percentage of capital
    /// Default: 15% (prevents concentration risk)
    pub max_single_position_pct: Decimal,

    /// Daily loss limit as percentage of capital (circuit breaker)
    /// Default: 3% (at 1% risk per trade, 3 consecutive losses hit this)
    pub daily_loss_limit_pct: Decimal,
}

impl Default for RiskLimits {
    fn default() -> Self {
        Self {
            max_open_positions: 3,
            max_total_exposure_pct: Decimal::from(30),
            max_single_position_pct: Decimal::from(15),
            daily_loss_limit_pct: Decimal::from(3),
        }
    }
}

impl RiskLimits {
    /// Create risk limits with custom values
    pub fn new(
        max_open_positions: usize,
        max_total_exposure_pct: Decimal,
        max_single_position_pct: Decimal,
        daily_loss_limit_pct: Decimal,
    ) -> Self {
        Self {
            max_open_positions,
            max_total_exposure_pct,
            max_single_position_pct,
            daily_loss_limit_pct,
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
    /// Position direction
    pub side: String,
    /// Notional value (quantity × price)
    pub notional_value: Decimal,
    /// Margin used
    pub margin_used: Decimal,
    /// Unrealized PnL
    pub unrealized_pnl: Decimal,
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
    /// Daily realized PnL
    pub daily_realized_pnl: Decimal,
    /// Daily unrealized PnL
    pub daily_unrealized_pnl: Decimal,
}

impl RiskContext {
    /// Create an empty context with just capital
    pub fn new(capital: Decimal) -> Self {
        Self {
            capital,
            open_positions: vec![],
            total_notional_exposure: Decimal::ZERO,
            daily_realized_pnl: Decimal::ZERO,
            daily_unrealized_pnl: Decimal::ZERO,
        }
    }

    /// Create context with positions
    pub fn with_positions(
        capital: Decimal,
        open_positions: Vec<PositionSummary>,
        daily_realized_pnl: Decimal,
        daily_unrealized_pnl: Decimal,
    ) -> Self {
        let total_notional_exposure = open_positions.iter().map(|p| p.notional_value).sum();

        Self {
            capital,
            open_positions,
            total_notional_exposure,
            daily_realized_pnl,
            daily_unrealized_pnl,
        }
    }

    /// Count open positions
    pub fn open_position_count(&self) -> usize {
        self.open_positions.len()
    }

    /// Calculate total daily PnL (realized + unrealized)
    pub fn total_daily_pnl(&self) -> Decimal {
        self.daily_realized_pnl + self.daily_unrealized_pnl
    }

    /// Check if there's an existing position with same symbol and side
    pub fn has_duplicate_position(&self, symbol: &str, side: &str) -> bool {
        self.open_positions.iter().any(|p| p.symbol == symbol && p.side == side)
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
    /// Margin required (notional / leverage)
    pub margin_required: Decimal,
}

// =============================================================================
// Risk Verdict
// =============================================================================

/// Which risk check failed
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RiskCheck {
    /// Too many open positions
    MaxOpenPositions,
    /// Total exposure exceeds limit
    TotalExposure,
    /// Single position too large
    SinglePositionConcentration,
    /// Not enough margin available
    InsufficientMargin,
    /// Daily loss limit exceeded
    DailyLossLimit,
    /// Already have position on same symbol+side
    DuplicatePosition,
}

impl RiskCheck {
    /// Get human-readable name
    pub fn name(&self) -> &'static str {
        match self {
            RiskCheck::MaxOpenPositions => "max_open_positions",
            RiskCheck::TotalExposure => "total_exposure",
            RiskCheck::SinglePositionConcentration => "single_position_concentration",
            RiskCheck::InsufficientMargin => "insufficient_margin",
            RiskCheck::DailyLossLimit => "daily_loss_limit",
            RiskCheck::DuplicatePosition => "duplicate_position",
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

/// The pre-trade approval gate
///
/// Evaluates proposed trades against risk limits.
/// Pure computation, no I/O.
#[derive(Debug, Clone)]
pub struct RiskGate {
    limits: RiskLimits,
}

impl RiskGate {
    /// Create a new RiskGate with default limits
    pub fn new() -> Self {
        Self { limits: RiskLimits::default() }
    }

    /// Create a RiskGate with custom limits
    pub fn with_limits(limits: RiskLimits) -> Self {
        Self { limits }
    }

    /// Get the current limits
    pub fn limits(&self) -> &RiskLimits {
        &self.limits
    }

    /// Evaluate a proposed trade against current risk context
    ///
    /// Returns:
    /// - `RiskVerdict::Approved` if all checks pass
    /// - `RiskVerdict::Rejected` if any check fails
    pub fn evaluate(&self, proposed: &ProposedTrade, context: &RiskContext) -> RiskVerdict {
        // 1. Check max open positions
        if context.open_position_count() >= self.limits.max_open_positions {
            debug!(
                current = context.open_position_count(),
                max = self.limits.max_open_positions,
                "Risk check failed: max open positions"
            );
            return RiskVerdict::Rejected {
                check: RiskCheck::MaxOpenPositions,
                reason: format!(
                    "Already have {} open positions (max: {})",
                    context.open_position_count(),
                    self.limits.max_open_positions
                ),
            };
        }

        // 2. Check total exposure
        let new_total_exposure = context.total_notional_exposure + proposed.notional_value;
        let max_exposure =
            context.capital * self.limits.max_total_exposure_pct / Decimal::from(100);
        if new_total_exposure > max_exposure {
            debug!(
                current = %context.total_notional_exposure,
                proposed = %proposed.notional_value,
                max = %max_exposure,
                "Risk check failed: total exposure"
            );
            return RiskVerdict::Rejected {
                check: RiskCheck::TotalExposure,
                reason: format!(
                    "Total exposure {} + {} would exceed {}% of capital ({})",
                    context.total_notional_exposure,
                    proposed.notional_value,
                    self.limits.max_total_exposure_pct,
                    max_exposure
                ),
            };
        }

        // 3. Check single position concentration
        let max_single = context.capital * self.limits.max_single_position_pct / Decimal::from(100);
        if proposed.notional_value > max_single {
            debug!(
                proposed = %proposed.notional_value,
                max = %max_single,
                "Risk check failed: single position concentration"
            );
            return RiskVerdict::Rejected {
                check: RiskCheck::SinglePositionConcentration,
                reason: format!(
                    "Position size {} exceeds {}% of capital ({})",
                    proposed.notional_value, self.limits.max_single_position_pct, max_single
                ),
            };
        }

        // 4. Check duplicate position (same symbol + side)
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

        // 5. Check daily loss circuit breaker
        let daily_pnl = context.total_daily_pnl();
        let daily_loss_limit =
            context.capital * self.limits.daily_loss_limit_pct / Decimal::from(100);
        if daily_pnl < -daily_loss_limit {
            debug!(
                daily_pnl = %daily_pnl,
                limit = %daily_loss_limit,
                "Risk check failed: daily loss limit"
            );
            return RiskVerdict::Rejected {
                check: RiskCheck::DailyLossLimit,
                reason: format!(
                    "Daily P&L {} has exceeded loss limit of -{}",
                    daily_pnl, daily_loss_limit
                ),
            };
        }

        // 6. Check margin availability (optional - exchange will also validate)
        // Note: This is a pre-check; exchange will do final validation
        // For now, we rely on exchange validation for margin

        debug!(
            symbol = %proposed.symbol,
            side = %proposed.side,
            notional = %proposed.notional_value,
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
    use super::*;
    use rust_decimal_macros::dec;

    fn sample_context() -> RiskContext {
        RiskContext::new(dec!(10000))
    }

    fn sample_proposed() -> ProposedTrade {
        ProposedTrade {
            symbol: "BTCUSDT".to_string(),
            side: "long".to_string(),
            quantity: dec!(0.02), // 0.02 BTC
            entry_price: dec!(50000),
            notional_value: dec!(1000), // $1,000 (within 15% single position limit)
            margin_required: dec!(100),
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
    fn test_risk_gate_rejects_max_positions() {
        let gate = RiskGate::new();
        let context = RiskContext::with_positions(
            dec!(10000),
            vec![
                PositionSummary {
                    position_id: uuid::Uuid::nil(),
                    symbol: "ETHUSDT".to_string(),
                    side: "long".to_string(),
                    notional_value: dec!(3000),
                    margin_used: dec!(300),
                    unrealized_pnl: dec!(0),
                };
                3
            ],
            dec!(0),
            dec!(0),
        );
        let proposed = sample_proposed();

        let verdict = gate.evaluate(&proposed, &context);
        assert!(matches!(
            verdict,
            RiskVerdict::Rejected { check: RiskCheck::MaxOpenPositions, .. }
        ));
    }

    #[test]
    fn test_risk_gate_rejects_total_exposure() {
        let gate = RiskGate::new();
        let context = RiskContext::with_positions(
            dec!(10000),
            vec![PositionSummary {
                position_id: uuid::Uuid::nil(),
                symbol: "ETHUSDT".to_string(),
                side: "long".to_string(),
                notional_value: dec!(2900),
                margin_used: dec!(290),
                unrealized_pnl: dec!(0),
            }],
            dec!(0),
            dec!(0),
        );
        // 2900 + 5000 = 7900 > 3000 (30% of 10000)
        let proposed = ProposedTrade {
            symbol: "BTCUSDT".to_string(),
            side: "long".to_string(),
            quantity: dec!(0.1),
            entry_price: dec!(50000),
            notional_value: dec!(5000),
            margin_required: dec!(500),
        };

        let verdict = gate.evaluate(&proposed, &context);
        assert!(matches!(verdict, RiskVerdict::Rejected { check: RiskCheck::TotalExposure, .. }));
    }

    #[test]
    fn test_risk_gate_rejects_single_concentration() {
        let gate = RiskGate::new();
        let context = sample_context();
        // 2000 > 1500 (15% of 10000)
        let proposed = ProposedTrade {
            symbol: "BTCUSDT".to_string(),
            side: "long".to_string(),
            quantity: dec!(0.04),
            entry_price: dec!(50000),
            notional_value: dec!(2000),
            margin_required: dec!(200),
        };

        let verdict = gate.evaluate(&proposed, &context);
        assert!(matches!(
            verdict,
            RiskVerdict::Rejected {
                check: RiskCheck::SinglePositionConcentration,
                ..
            }
        ));
    }

    #[test]
    fn test_risk_gate_rejects_duplicate_position() {
        let gate = RiskGate::new();
        let context = RiskContext::with_positions(
            dec!(10000),
            vec![PositionSummary {
                position_id: uuid::Uuid::nil(),
                symbol: "BTCUSDT".to_string(),
                side: "long".to_string(),
                notional_value: dec!(1000),
                margin_used: dec!(100),
                unrealized_pnl: dec!(0),
            }],
            dec!(0),
            dec!(0),
        );
        let proposed = sample_proposed(); // BTCUSDT long

        let verdict = gate.evaluate(&proposed, &context);
        assert!(matches!(
            verdict,
            RiskVerdict::Rejected { check: RiskCheck::DuplicatePosition, .. }
        ));
    }

    #[test]
    fn test_risk_gate_rejects_daily_loss_limit() {
        let gate = RiskGate::new();
        let context = RiskContext {
            capital: dec!(10000),
            open_positions: vec![],
            total_notional_exposure: Decimal::ZERO,
            daily_realized_pnl: dec!(-250),
            daily_unrealized_pnl: dec!(-100),
        };
        // -350 < -300 (3% of 10000)
        let proposed = sample_proposed();

        let verdict = gate.evaluate(&proposed, &context);
        assert!(matches!(
            verdict,
            RiskVerdict::Rejected { check: RiskCheck::DailyLossLimit, .. }
        ));
    }

    #[test]
    fn test_risk_gate_allows_same_symbol_opposite_side() {
        let gate = RiskGate::new();
        let context = RiskContext::with_positions(
            dec!(10000),
            vec![PositionSummary {
                position_id: uuid::Uuid::nil(),
                symbol: "BTCUSDT".to_string(),
                side: "short".to_string(), // Different side
                notional_value: dec!(1000),
                margin_used: dec!(100),
                unrealized_pnl: dec!(0),
            }],
            dec!(0),
            dec!(0),
        );
        let proposed = sample_proposed(); // BTCUSDT long

        let verdict = gate.evaluate(&proposed, &context);
        assert_eq!(verdict, RiskVerdict::Approved);
    }
}
