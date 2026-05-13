//! Trading policy layer (ADR-0024).
//!
//! Immutable trading policies and configurable technical stop parameters.
//! This module is the single source of truth for risk policy decisions.
//! `robson-engine` consumes policies; `robsond` constructs them at startup.

use std::fmt;

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// Entry policy selected by the operator.
///
/// This is a selector only. It must not contain signal-detection logic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryPolicy {
    /// Enter immediately after technical stop analysis, risk, and approval
    /// gates. No signal strategy is required.
    Immediate,
    /// Require deterministic trend confirmation before entry.
    ConfirmedTrend,
    /// Require deterministic reversal confirmation before entry.
    ConfirmedReversal,
    /// Require deterministic key-level interaction and reaction confirmation.
    ConfirmedKeyLevel,
}

impl Default for EntryPolicy {
    fn default() -> Self {
        Self::ConfirmedTrend
    }
}

/// Operator approval policy for an entry request.
///
/// This is independent from [`EntryPolicy`]. A confirmed strategy may still be
/// automatic, and an immediate entry may still require human confirmation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalPolicy {
    /// Proceed automatically after strategy, technical stop, and risk gates.
    Automatic,
    /// Hold the risk-approved entry until explicit operator confirmation.
    HumanConfirmation,
}

impl Default for ApprovalPolicy {
    fn default() -> Self {
        Self::Automatic
    }
}

/// Policy bundle supplied by the operator for an entry lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EntryPolicyConfig {
    /// Signal policy mode.
    pub mode: EntryPolicy,
    /// Operator approval mode.
    pub approval: ApprovalPolicy,
}

impl EntryPolicyConfig {
    /// Construct a new entry policy configuration.
    pub fn new(mode: EntryPolicy, approval: ApprovalPolicy) -> Self {
        Self { mode, approval }
    }
}

impl Default for EntryPolicyConfig {
    fn default() -> Self {
        Self {
            mode: EntryPolicy::default(),
            approval: ApprovalPolicy::default(),
        }
    }
}

/// Stable strategy identifier used in event payloads and replay.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct StrategyId {
    /// Stable strategy name, for example `sma_crossover`.
    pub name: String,
    /// Monotonic strategy version.
    pub version: u32,
}

impl StrategyId {
    /// Construct a new strategy identifier.
    pub fn new(name: impl Into<String>, version: u32) -> Self {
        Self { name: name.into(), version }
    }
}

impl fmt::Display for StrategyId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:v{}", self.name, self.version)
    }
}

/// Persisted outcome type for signal-strategy audit events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignalEvaluationOutcome {
    /// Strategy did not confirm an entry signal.
    NoSignal,
    /// Strategy confirmed an entry signal.
    SignalConfirmed,
}

/// Primary trading policy with immutable risk parameters (ADR-0024 Decision 2).
///
/// Risk per trade (1%) and max monthly drawdown (4%) are fixed by product
/// definition. These values are not configurable via environment variables,
/// operator API, or any runtime mechanism.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TradingPolicy {
    /// Risk per trade as percentage of capital. Fixed at 1%.
    pub risk_per_trade_pct: Decimal,
    /// Maximum monthly drawdown as percentage of capital. Fixed at 4%.
    pub max_monthly_drawdown_pct: Decimal,
}

impl TradingPolicy {
    /// Returns the default policy (1% risk, 4% max drawdown).
    pub fn new() -> Self {
        Self {
            risk_per_trade_pct: Decimal::ONE,
            max_monthly_drawdown_pct: Decimal::from(4),
        }
    }

    /// Dollar risk per trade: `capital * risk_per_trade_pct / 100`.
    pub fn risk_per_trade_amount(&self, capital: Decimal) -> Decimal {
        capital * self.risk_per_trade_pct / Decimal::from(100)
    }

    /// Monthly risk budget: `capital_base * max_monthly_drawdown_pct / 100`.
    pub fn monthly_budget(&self, capital_base: Decimal) -> Decimal {
        capital_base * self.max_monthly_drawdown_pct / Decimal::from(100)
    }

    /// Dynamic slot count (ADR-0024 Decision 5).
    ///
    /// Returns the number of new positions that can be opened given current
    /// capital, realized losses, and latent risk of open positions.
    ///
    /// Returns 0 if capital_base <= 0, risk amount <= 0, or remaining budget
    /// is less than one risk unit.
    pub fn slots_available(
        &self,
        capital_base: Decimal,
        realized_loss: Decimal,
        latent_risk: Decimal,
    ) -> u32 {
        if capital_base <= Decimal::ZERO {
            return 0;
        }
        let risk_amount = self.risk_per_trade_amount(capital_base);
        if risk_amount <= Decimal::ZERO {
            return 0;
        }
        let remaining = self.monthly_budget(capital_base) - realized_loss - latent_risk;
        if remaining < risk_amount {
            return 0;
        }
        let slots = remaining / risk_amount;
        decimal_floor_to_u32(slots).unwrap_or(u32::MAX)
    }
}

impl Default for TradingPolicy {
    fn default() -> Self {
        Self::new()
    }
}

/// Configurable technical stop parameters (ADR-0024 Decision 3).
///
/// These govern how chart analysis produces a technical stop. They are
/// configurable per environment via environment variables.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TechStopConfig {
    /// Minimum technical stop distance as percentage of entry.
    pub min_stop_pct: Decimal,
    /// Maximum technical stop distance as percentage of entry.
    pub max_stop_pct: Decimal,
    /// Support/resistance level to use (e.g., 2 = second support).
    pub support_level_n: usize,
    /// Number of candles to look back for stop analysis.
    pub lookback_candles: usize,
}

impl TechStopConfig {
    /// Returns production-safe defaults (ADR-0024).
    pub fn new() -> Self {
        Self {
            min_stop_pct: Decimal::ONE,
            max_stop_pct: Decimal::from(10),
            support_level_n: 2,
            lookback_candles: 100,
        }
    }
}

impl Default for TechStopConfig {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Decimal → u32 conversion helper (no unwrap in production)
// ---------------------------------------------------------------------------

fn decimal_floor_to_u32(v: Decimal) -> Option<u32> {
    use rust_decimal::prelude::ToPrimitive;
    if v < Decimal::ZERO {
        return None;
    }
    let floored = v.floor();
    let i = ToPrimitive::to_i64(&floored)?;
    u32::try_from(i).ok()
}

#[cfg(test)]
mod tests {
    use rust_decimal_macros::dec;

    use super::*;

    #[test]
    fn default_policy_values() {
        let p = TradingPolicy::default();
        assert_eq!(p.risk_per_trade_pct, dec!(1));
        assert_eq!(p.max_monthly_drawdown_pct, dec!(4));
    }

    #[test]
    fn risk_per_trade_amount() {
        let p = TradingPolicy::default();
        assert_eq!(p.risk_per_trade_amount(dec!(10000)), dec!(100));
        assert_eq!(p.risk_per_trade_amount(dec!(5000)), dec!(50));
    }

    #[test]
    fn monthly_budget_calculation() {
        let p = TradingPolicy::default();
        assert_eq!(p.monthly_budget(dec!(10000)), dec!(400));
        assert_eq!(p.monthly_budget(dec!(50000)), dec!(2000));
    }

    #[test]
    fn slots_available_basic() {
        let p = TradingPolicy::default();
        // capital=100, budget=4, risk=1, 0 loss, 0 latent → 4 slots
        assert_eq!(p.slots_available(dec!(100), dec!(0), dec!(0)), 4);
    }

    #[test]
    fn slots_available_with_realized_loss() {
        let p = TradingPolicy::default();
        // budget=4, loss=1, risk=1 → (4-1)/1 = 3 slots
        assert_eq!(p.slots_available(dec!(100), dec!(1), dec!(0)), 3);
    }

    #[test]
    fn slots_available_with_latent_risk() {
        let p = TradingPolicy::default();
        // budget=4, loss=0, latent=1, risk=1 → (4-0-1)/1 = 3 slots
        assert_eq!(p.slots_available(dec!(100), dec!(0), dec!(1)), 3);
    }

    #[test]
    fn slots_available_combined() {
        let p = TradingPolicy::default();
        // budget=4, loss=1, latent=1, risk=1 → (4-1-1)/1 = 2 slots
        assert_eq!(p.slots_available(dec!(100), dec!(1), dec!(1)), 2);
    }

    #[test]
    fn slots_available_exhausted() {
        let p = TradingPolicy::default();
        // budget=4, loss=1, latent=3 → remaining=0 < 1 → 0
        assert_eq!(p.slots_available(dec!(100), dec!(1), dec!(3)), 0);
    }

    #[test]
    fn slots_available_zero_capital() {
        let p = TradingPolicy::default();
        assert_eq!(p.slots_available(dec!(0), dec!(0), dec!(0)), 0);
        assert_eq!(p.slots_available(dec!(-100), dec!(0), dec!(0)), 0);
    }

    #[test]
    fn slots_available_manual_concept() {
        // ADR-0024 manual concept: capital=100, BTC long entry=80000,
        // stop=78400 (2%), qty=0.000625, latent_risk=1
        let p = TradingPolicy::default();
        let slots = p.slots_available(dec!(100), dec!(0), dec!(1));
        assert_eq!(slots, 3, "expected floor((4-0-1)/1) = 3");
    }

    #[test]
    fn default_tech_stop_config() {
        let c = TechStopConfig::default();
        assert_eq!(c.min_stop_pct, dec!(1));
        assert_eq!(c.max_stop_pct, dec!(10));
        assert_eq!(c.support_level_n, 2);
        assert_eq!(c.lookback_candles, 100);
    }
}
