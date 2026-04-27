//! Deterministic signal strategy engine.
//!
//! Entry policies resolve to strategy identifiers. Strategies evaluate persisted
//! market data and return pure signal decisions. They do not size positions,
//! compute technical stops, check risk, request approval, or execute orders.

use std::{collections::HashMap, fmt};

use chrono::{DateTime, Utc};
use robson_domain::{Candle, EntryPolicy, PositionId, Side, StrategyId, Symbol};
use rust_decimal::Decimal;

/// Pure context supplied to a signal strategy.
#[derive(Debug, Clone)]
pub struct SignalContext {
    /// Position being evaluated.
    pub position_id: PositionId,
    /// Trading pair being evaluated.
    pub symbol: Symbol,
    /// Direction requested by the entry intent.
    pub side: Side,
    /// Closed candles ordered oldest-first.
    pub candles: Vec<Candle>,
    /// Deterministic evaluation time supplied by the caller.
    pub evaluated_at: DateTime<Utc>,
}

/// Deterministic strategy evaluation interface.
pub trait SignalStrategy: Send + Sync {
    /// Evaluate the strategy using only the supplied context.
    fn evaluate(&self, ctx: SignalContext) -> SignalDecision;
}

/// Strategy evaluation result.
#[derive(Debug, Clone, PartialEq)]
pub enum SignalDecision {
    /// No signal is confirmed.
    NoSignal,
    /// Strategy confirmed a signal.
    SignalConfirmed {
        /// Confirmed entry side.
        side: Side,
        /// Deterministic reason for audit.
        reason: SignalReason,
        /// Candle close time or other deterministic observation time.
        observed_at: DateTime<Utc>,
        /// Reference price used for entry evaluation.
        reference_price: Decimal,
    },
}

/// Deterministic signal reason.
#[derive(Debug, Clone, PartialEq)]
pub enum SignalReason {
    /// Immediate policy confirmation.
    Immediate,
    /// SMA crossover confirmation.
    SmaCrossover {
        /// Fast SMA period.
        fast_period: usize,
        /// Slow SMA period.
        slow_period: usize,
    },
    /// Candlestick reversal pattern confirmation.
    ReversalPattern {
        /// Pattern that confirmed the signal.
        pattern: ReversalPattern,
    },
    /// Key-level rejection confirmation.
    KeyLevelReaction {
        /// Level kind.
        level_kind: KeyLevelKind,
        /// Deterministic support/resistance level.
        level_price: Decimal,
    },
}

impl fmt::Display for SignalReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SignalReason::Immediate => write!(f, "immediate"),
            SignalReason::SmaCrossover { fast_period, slow_period } => {
                write!(f, "sma_crossover:{fast_period}/{slow_period}")
            },
            SignalReason::ReversalPattern { pattern } => write!(f, "reversal_pattern:{pattern}"),
            SignalReason::KeyLevelReaction { level_kind, level_price } => {
                write!(f, "key_level:{level_kind}:{level_price}")
            },
        }
    }
}

/// Supported reversal patterns for v1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReversalPattern {
    /// Hammer pattern.
    Hammer,
    /// Shooting star pattern.
    ShootingStar,
    /// Bullish engulfing pattern.
    BullishEngulfing,
    /// Bearish engulfing pattern.
    BearishEngulfing,
}

impl fmt::Display for ReversalPattern {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ReversalPattern::Hammer => write!(f, "hammer"),
            ReversalPattern::ShootingStar => write!(f, "shooting_star"),
            ReversalPattern::BullishEngulfing => write!(f, "bullish_engulfing"),
            ReversalPattern::BearishEngulfing => write!(f, "bearish_engulfing"),
        }
    }
}

/// Key level kind used by key-level strategy reasons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyLevelKind {
    /// Support level for long entries.
    Support,
    /// Resistance level for short entries.
    Resistance,
}

impl fmt::Display for KeyLevelKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            KeyLevelKind::Support => write!(f, "support"),
            KeyLevelKind::Resistance => write!(f, "resistance"),
        }
    }
}

/// Internal key-level preconditions.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SignalPrecondition {
    /// Price interacts with a deterministic support/resistance level.
    KeyLevelInteraction,
    /// Confirmation candle rejects the level and aligns with entry direction.
    ReactionConfirmation,
}

/// Strategy registry keyed by stable strategy identifiers.
pub struct StrategyRegistry {
    /// Registered deterministic strategies.
    pub strategies: HashMap<StrategyId, Box<dyn SignalStrategy>>,
}

impl StrategyRegistry {
    /// Create a registry populated with v1 production strategies.
    pub fn new() -> Self {
        let mut registry = Self::empty();
        registry.register(
            StrategyId::new("sma_crossover", 1),
            Box::new(SmaCrossoverStrategy::default()),
        );
        registry.register(
            StrategyId::new("reversal_patterns", 1),
            Box::new(ReversalPatternStrategy::default()),
        );
        registry.register(StrategyId::new("key_level", 1), Box::new(KeyLevelStrategy::default()));
        registry
    }

    /// Create an empty strategy registry.
    pub fn empty() -> Self {
        Self { strategies: HashMap::new() }
    }

    /// Register a strategy implementation.
    pub fn register(
        &mut self,
        strategy_id: StrategyId,
        strategy: Box<dyn SignalStrategy>,
    ) -> Option<Box<dyn SignalStrategy>> {
        self.strategies.insert(strategy_id, strategy)
    }

    /// Resolve a strategy by identifier.
    pub fn get(&self, strategy_id: &StrategyId) -> Option<&dyn SignalStrategy> {
        self.strategies.get(strategy_id).map(|strategy| strategy.as_ref())
    }

    /// Resolve an entry policy to its strategy identifier.
    pub fn strategy_id_for_policy(policy: EntryPolicy) -> Option<StrategyId> {
        match policy {
            EntryPolicy::Immediate => None,
            EntryPolicy::ConfirmedTrend => Some(StrategyId::new("sma_crossover", 1)),
            EntryPolicy::ConfirmedReversal => Some(StrategyId::new("reversal_patterns", 1)),
            EntryPolicy::ConfirmedKeyLevel => Some(StrategyId::new("key_level", 1)),
        }
    }
}

impl Default for StrategyRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// SMA crossover strategy v1.
#[derive(Debug, Clone, Copy)]
pub struct SmaCrossoverStrategy {
    /// Fast SMA period.
    pub fast_period: usize,
    /// Slow SMA period.
    pub slow_period: usize,
}

impl SmaCrossoverStrategy {
    /// Construct an SMA crossover strategy.
    pub fn new(fast_period: usize, slow_period: usize) -> Self {
        Self { fast_period, slow_period }
    }
}

impl Default for SmaCrossoverStrategy {
    fn default() -> Self {
        Self::new(9, 21)
    }
}

impl SignalStrategy for SmaCrossoverStrategy {
    fn evaluate(&self, ctx: SignalContext) -> SignalDecision {
        if !candles_match_symbol(&ctx.candles, &ctx.symbol) {
            return SignalDecision::NoSignal;
        }

        let closes: Vec<Decimal> = ctx.candles.iter().map(|candle| candle.close).collect();
        if !detect_sma_crossover_from_prices(&closes, ctx.side, self.fast_period, self.slow_period)
        {
            return SignalDecision::NoSignal;
        }

        let Some(last) = ctx.candles.last() else {
            return SignalDecision::NoSignal;
        };

        SignalDecision::SignalConfirmed {
            side: ctx.side,
            reason: SignalReason::SmaCrossover {
                fast_period: self.fast_period,
                slow_period: self.slow_period,
            },
            observed_at: last.close_time,
            reference_price: last.close,
        }
    }
}

/// Detect a single-shot SMA crossover from ordered prices.
///
/// The last price is the current observation. The previous observation is the
/// same series without the last price. This mirrors the legacy detector's
/// "previous MA then current MA" crossover semantics.
pub fn detect_sma_crossover_from_prices(
    prices: &[Decimal],
    side: Side,
    fast_period: usize,
    slow_period: usize,
) -> bool {
    if fast_period < 2 || fast_period >= slow_period || prices.len() < slow_period + 1 {
        return false;
    }

    let previous_prices = &prices[..prices.len() - 1];
    let Some(previous_fast) = simple_moving_average(previous_prices, fast_period) else {
        return false;
    };
    let Some(previous_slow) = simple_moving_average(previous_prices, slow_period) else {
        return false;
    };
    let Some(current_fast) = simple_moving_average(prices, fast_period) else {
        return false;
    };
    let Some(current_slow) = simple_moving_average(prices, slow_period) else {
        return false;
    };

    let was_above = previous_fast > previous_slow;
    let is_above = current_fast > current_slow;

    match side {
        Side::Long => !was_above && is_above,
        Side::Short => was_above && !is_above,
    }
}

fn simple_moving_average(prices: &[Decimal], period: usize) -> Option<Decimal> {
    if period == 0 || prices.len() < period {
        return None;
    }
    let start_idx = prices.len() - period;
    let sum: Decimal = prices[start_idx..].iter().copied().sum();
    Some(sum / Decimal::from(period))
}

/// Reversal pattern strategy v1.
#[derive(Debug, Clone, Copy)]
pub struct ReversalPatternStrategy {
    /// Number of prior candles used for directional-move confirmation.
    pub directional_move_candles: usize,
    /// Lookback window used for local extreme confirmation.
    pub local_extreme_lookback: usize,
}

impl ReversalPatternStrategy {
    /// Construct a reversal pattern strategy.
    pub fn new(directional_move_candles: usize, local_extreme_lookback: usize) -> Self {
        Self {
            directional_move_candles,
            local_extreme_lookback,
        }
    }
}

impl Default for ReversalPatternStrategy {
    fn default() -> Self {
        Self::new(3, 8)
    }
}

impl SignalStrategy for ReversalPatternStrategy {
    fn evaluate(&self, ctx: SignalContext) -> SignalDecision {
        if !candles_match_symbol(&ctx.candles, &ctx.symbol) || ctx.candles.len() < 3 {
            return SignalDecision::NoSignal;
        }

        let pattern = match ctx.side {
            Side::Long => detect_bullish_reversal_pattern(&ctx.candles),
            Side::Short => detect_bearish_reversal_pattern(&ctx.candles),
        };
        let Some(pattern) = pattern else {
            return SignalDecision::NoSignal;
        };

        if !self.has_directional_move(&ctx.candles, ctx.side, pattern) {
            return SignalDecision::NoSignal;
        }

        if !self.occurs_near_local_extreme(&ctx.candles, ctx.side, pattern) {
            return SignalDecision::NoSignal;
        }

        let last = ctx.candles.last().expect("length checked above");
        SignalDecision::SignalConfirmed {
            side: ctx.side,
            reason: SignalReason::ReversalPattern { pattern },
            observed_at: last.close_time,
            reference_price: last.close,
        }
    }
}

impl ReversalPatternStrategy {
    fn has_directional_move(
        &self,
        candles: &[robson_domain::Candle],
        side: Side,
        pattern: ReversalPattern,
    ) -> bool {
        let pattern_candles = match pattern {
            ReversalPattern::BullishEngulfing | ReversalPattern::BearishEngulfing => 2,
            ReversalPattern::Hammer | ReversalPattern::ShootingStar => 1,
        };
        if candles.len() < pattern_candles + self.directional_move_candles {
            return false;
        }

        let end = candles.len() - pattern_candles;
        let start = end - self.directional_move_candles;
        let closes: Vec<Decimal> = candles[start..end].iter().map(|candle| candle.close).collect();

        match side {
            Side::Long => closes.windows(2).all(|window| window[0] > window[1]),
            Side::Short => closes.windows(2).all(|window| window[0] < window[1]),
        }
    }

    fn occurs_near_local_extreme(
        &self,
        candles: &[robson_domain::Candle],
        side: Side,
        pattern: ReversalPattern,
    ) -> bool {
        let pattern_candles = match pattern {
            ReversalPattern::BullishEngulfing | ReversalPattern::BearishEngulfing => 2,
            ReversalPattern::Hammer | ReversalPattern::ShootingStar => 1,
        };
        if candles.len() < pattern_candles {
            return false;
        }

        let lookback = self.local_extreme_lookback.min(candles.len());
        let window = &candles[candles.len() - lookback..];
        let pattern_window = &candles[candles.len() - pattern_candles..];

        match side {
            Side::Long => {
                let local_low = window.iter().map(|candle| candle.low).min();
                let pattern_low = pattern_window.iter().map(|candle| candle.low).min();
                local_low == pattern_low
            },
            Side::Short => {
                let local_high = window.iter().map(|candle| candle.high).max();
                let pattern_high = pattern_window.iter().map(|candle| candle.high).max();
                local_high == pattern_high
            },
        }
    }
}

/// Key-level strategy v1.
#[derive(Debug, Clone, Copy)]
pub struct KeyLevelStrategy {
    /// Number of candles used to find deterministic local highs/lows.
    pub lookback_window: usize,
    /// Number of candles on each side required for a local high/low.
    pub swing_window: usize,
    /// Allowed relative distance from level, for example `0.005` means 0.5%.
    pub tolerance: Decimal,
}

impl KeyLevelStrategy {
    /// Construct a key-level strategy.
    pub fn new(lookback_window: usize, swing_window: usize, tolerance: Decimal) -> Self {
        Self { lookback_window, swing_window, tolerance }
    }
}

impl Default for KeyLevelStrategy {
    fn default() -> Self {
        Self::new(30, 2, Decimal::new(5, 3))
    }
}

impl SignalStrategy for KeyLevelStrategy {
    fn evaluate(&self, ctx: SignalContext) -> SignalDecision {
        if !candles_match_symbol(&ctx.candles, &ctx.symbol)
            || ctx.candles.len() < self.swing_window * 2 + 4
        {
            return SignalDecision::NoSignal;
        }

        match ctx.side {
            Side::Long => self.evaluate_support_reaction(ctx),
            Side::Short => self.evaluate_resistance_reaction(ctx),
        }
    }
}

impl KeyLevelStrategy {
    fn evaluate_support_reaction(&self, ctx: SignalContext) -> SignalDecision {
        let interaction = &ctx.candles[ctx.candles.len() - 2];
        let confirmation = ctx.candles.last().expect("length checked by caller");
        let history_end = ctx.candles.len() - 2;
        let history_start = history_end.saturating_sub(self.lookback_window);
        let levels =
            local_support_levels(&ctx.candles[history_start..history_end], self.swing_window);

        let Some(level) = closest_level(interaction.low, &levels, self.tolerance) else {
            return SignalDecision::NoSignal;
        };

        if !self.precondition_met(
            SignalPrecondition::KeyLevelInteraction,
            Side::Long,
            interaction,
            level,
        ) || !self.precondition_met(
            SignalPrecondition::ReactionConfirmation,
            Side::Long,
            confirmation,
            level,
        ) {
            return SignalDecision::NoSignal;
        }

        SignalDecision::SignalConfirmed {
            side: Side::Long,
            reason: SignalReason::KeyLevelReaction {
                level_kind: KeyLevelKind::Support,
                level_price: level,
            },
            observed_at: confirmation.close_time,
            reference_price: confirmation.close,
        }
    }

    fn evaluate_resistance_reaction(&self, ctx: SignalContext) -> SignalDecision {
        let interaction = &ctx.candles[ctx.candles.len() - 2];
        let confirmation = ctx.candles.last().expect("length checked by caller");
        let history_end = ctx.candles.len() - 2;
        let history_start = history_end.saturating_sub(self.lookback_window);
        let levels =
            local_resistance_levels(&ctx.candles[history_start..history_end], self.swing_window);

        let Some(level) = closest_level(interaction.high, &levels, self.tolerance) else {
            return SignalDecision::NoSignal;
        };

        if !self.precondition_met(
            SignalPrecondition::KeyLevelInteraction,
            Side::Short,
            interaction,
            level,
        ) || !self.precondition_met(
            SignalPrecondition::ReactionConfirmation,
            Side::Short,
            confirmation,
            level,
        ) {
            return SignalDecision::NoSignal;
        }

        SignalDecision::SignalConfirmed {
            side: Side::Short,
            reason: SignalReason::KeyLevelReaction {
                level_kind: KeyLevelKind::Resistance,
                level_price: level,
            },
            observed_at: confirmation.close_time,
            reference_price: confirmation.close,
        }
    }

    fn precondition_met(
        &self,
        precondition: SignalPrecondition,
        side: Side,
        candle: &robson_domain::Candle,
        level: Decimal,
    ) -> bool {
        match (precondition, side) {
            (SignalPrecondition::KeyLevelInteraction, Side::Long) => {
                within_tolerance(candle.low, level, self.tolerance)
            },
            (SignalPrecondition::KeyLevelInteraction, Side::Short) => {
                within_tolerance(candle.high, level, self.tolerance)
            },
            (SignalPrecondition::ReactionConfirmation, Side::Long) => {
                candle.close > candle.open
                    && candle.close > level
                    && candle.low <= level * (Decimal::ONE + self.tolerance)
            },
            (SignalPrecondition::ReactionConfirmation, Side::Short) => {
                candle.close < candle.open
                    && candle.close < level
                    && candle.high >= level * (Decimal::ONE - self.tolerance)
            },
        }
    }
}

fn candles_match_symbol(candles: &[robson_domain::Candle], symbol: &Symbol) -> bool {
    !candles.is_empty() && candles.iter().all(|candle| candle.symbol == *symbol)
}

fn detect_bullish_reversal_pattern(candles: &[robson_domain::Candle]) -> Option<ReversalPattern> {
    let last = candles.last()?;
    if is_hammer(last) {
        return Some(ReversalPattern::Hammer);
    }
    let previous = candles.get(candles.len().checked_sub(2)?)?;
    if is_bullish_engulfing(previous, last) {
        return Some(ReversalPattern::BullishEngulfing);
    }
    None
}

fn detect_bearish_reversal_pattern(candles: &[robson_domain::Candle]) -> Option<ReversalPattern> {
    let last = candles.last()?;
    if is_shooting_star(last) {
        return Some(ReversalPattern::ShootingStar);
    }
    let previous = candles.get(candles.len().checked_sub(2)?)?;
    if is_bearish_engulfing(previous, last) {
        return Some(ReversalPattern::BearishEngulfing);
    }
    None
}

fn is_hammer(candle: &robson_domain::Candle) -> bool {
    let Some((body, upper_shadow, lower_shadow, range)) = candle_shape(candle) else {
        return false;
    };
    body > Decimal::ZERO
        && body <= range * Decimal::new(35, 2)
        && lower_shadow >= body * Decimal::from(2)
        && upper_shadow <= body
}

fn is_shooting_star(candle: &robson_domain::Candle) -> bool {
    let Some((body, upper_shadow, lower_shadow, range)) = candle_shape(candle) else {
        return false;
    };
    body > Decimal::ZERO
        && body <= range * Decimal::new(35, 2)
        && upper_shadow >= body * Decimal::from(2)
        && lower_shadow <= body
}

fn is_bullish_engulfing(previous: &robson_domain::Candle, current: &robson_domain::Candle) -> bool {
    previous.close < previous.open
        && current.close > current.open
        && current.open <= previous.close
        && current.close >= previous.open
}

fn is_bearish_engulfing(previous: &robson_domain::Candle, current: &robson_domain::Candle) -> bool {
    previous.close > previous.open
        && current.close < current.open
        && current.open >= previous.close
        && current.close <= previous.open
}

fn candle_shape(candle: &robson_domain::Candle) -> Option<(Decimal, Decimal, Decimal, Decimal)> {
    let range = candle.high - candle.low;
    if range <= Decimal::ZERO {
        return None;
    }
    let body_top = candle.open.max(candle.close);
    let body_bottom = candle.open.min(candle.close);
    let body = body_top - body_bottom;
    let upper_shadow = candle.high - body_top;
    let lower_shadow = body_bottom - candle.low;
    Some((body, upper_shadow, lower_shadow, range))
}

fn local_support_levels(candles: &[robson_domain::Candle], swing_window: usize) -> Vec<Decimal> {
    if candles.len() < swing_window * 2 + 1 {
        return Vec::new();
    }

    let mut levels = Vec::new();
    for index in swing_window..candles.len() - swing_window {
        let low = candles[index].low;
        let start = index - swing_window;
        let end = index + swing_window;
        if candles[start..=end].iter().all(|candle| low <= candle.low) {
            levels.push(low);
        }
    }
    levels
}

fn local_resistance_levels(candles: &[robson_domain::Candle], swing_window: usize) -> Vec<Decimal> {
    if candles.len() < swing_window * 2 + 1 {
        return Vec::new();
    }

    let mut levels = Vec::new();
    for index in swing_window..candles.len() - swing_window {
        let high = candles[index].high;
        let start = index - swing_window;
        let end = index + swing_window;
        if candles[start..=end].iter().all(|candle| high >= candle.high) {
            levels.push(high);
        }
    }
    levels
}

fn closest_level(price: Decimal, levels: &[Decimal], tolerance: Decimal) -> Option<Decimal> {
    levels
        .iter()
        .copied()
        .filter(|level| within_tolerance(price, *level, tolerance))
        .min_by_key(|level| (price - *level).abs())
}

fn within_tolerance(price: Decimal, level: Decimal, tolerance: Decimal) -> bool {
    if level <= Decimal::ZERO {
        return false;
    }
    ((price - level).abs() / level) <= tolerance
}

#[cfg(test)]
mod tests {
    use chrono::{Duration, Utc};
    use rust_decimal_macros::dec;
    use uuid::Uuid;

    use super::*;

    fn candle(
        index: i64,
        open: Decimal,
        high: Decimal,
        low: Decimal,
        close: Decimal,
    ) -> robson_domain::Candle {
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let open_time = Utc::now() + Duration::minutes(index * 15);
        robson_domain::Candle::new(
            symbol,
            open,
            high,
            low,
            close,
            dec!(100),
            10,
            open_time,
            open_time + Duration::minutes(15),
        )
    }

    fn ctx(side: Side, candles: Vec<robson_domain::Candle>) -> SignalContext {
        SignalContext {
            position_id: Uuid::now_v7(),
            symbol: Symbol::from_pair("BTCUSDT").unwrap(),
            side,
            candles,
            evaluated_at: Utc::now(),
        }
    }

    #[test]
    fn sma_crossover_confirms_long_once() {
        let prices = vec![dec!(100), dec!(99), dec!(98), dec!(97), dec!(96), dec!(105)];
        assert!(detect_sma_crossover_from_prices(&prices, Side::Long, 2, 5));
        assert!(!detect_sma_crossover_from_prices(&prices, Side::Short, 2, 5));
    }

    #[test]
    fn sma_strategy_uses_last_closed_candle_price() {
        let candles = vec![
            candle(0, dec!(100), dec!(100), dec!(100), dec!(100)),
            candle(1, dec!(99), dec!(99), dec!(99), dec!(99)),
            candle(2, dec!(98), dec!(98), dec!(98), dec!(98)),
            candle(3, dec!(97), dec!(97), dec!(97), dec!(97)),
            candle(4, dec!(96), dec!(96), dec!(96), dec!(96)),
            candle(5, dec!(105), dec!(105), dec!(105), dec!(105)),
        ];
        let strategy = SmaCrossoverStrategy::new(2, 5);
        let decision = strategy.evaluate(ctx(Side::Long, candles));

        assert!(matches!(
            decision,
            SignalDecision::SignalConfirmed {
                side: Side::Long,
                reference_price,
                ..
            } if reference_price == dec!(105)
        ));
    }

    #[test]
    fn reversal_strategy_confirms_hammer_after_down_move_at_local_low() {
        let candles = vec![
            candle(0, dec!(110), dec!(111), dec!(109), dec!(109)),
            candle(1, dec!(109), dec!(110), dec!(107), dec!(107)),
            candle(2, dec!(107), dec!(108), dec!(105), dec!(105)),
            candle(3, dec!(103), dec!(104), dec!(95), dec!(102)),
        ];
        let strategy = ReversalPatternStrategy::new(3, 4);
        let decision = strategy.evaluate(ctx(Side::Long, candles));

        assert!(matches!(
            decision,
            SignalDecision::SignalConfirmed {
                reason: SignalReason::ReversalPattern { pattern: ReversalPattern::Hammer },
                ..
            }
        ));
    }

    #[test]
    fn reversal_strategy_rejects_hammer_without_prior_down_move() {
        let candles = vec![
            candle(0, dec!(100), dec!(101), dec!(99), dec!(101)),
            candle(1, dec!(101), dec!(102), dec!(100), dec!(102)),
            candle(2, dec!(102), dec!(103), dec!(101), dec!(103)),
            candle(3, dec!(103), dec!(104), dec!(95), dec!(102)),
        ];
        let strategy = ReversalPatternStrategy::new(3, 4);
        assert_eq!(strategy.evaluate(ctx(Side::Long, candles)), SignalDecision::NoSignal);
    }

    #[test]
    fn reversal_strategy_confirms_bearish_engulfing_after_up_move_at_local_high() {
        let candles = vec![
            candle(0, dec!(100), dec!(101), dec!(99), dec!(101)),
            candle(1, dec!(101), dec!(103), dec!(100), dec!(103)),
            candle(2, dec!(103), dec!(105), dec!(102), dec!(105)),
            candle(3, dec!(105), dec!(108), dec!(104), dec!(107)),
            candle(4, dec!(108), dec!(109), dec!(100), dec!(104)),
        ];
        let strategy = ReversalPatternStrategy::new(3, 5);
        let decision = strategy.evaluate(ctx(Side::Short, candles));

        assert!(matches!(
            decision,
            SignalDecision::SignalConfirmed {
                reason: SignalReason::ReversalPattern {
                    pattern: ReversalPattern::BearishEngulfing
                },
                ..
            }
        ));
    }

    #[test]
    fn key_level_strategy_confirms_support_reaction() {
        let candles = vec![
            candle(0, dec!(110), dec!(112), dec!(108), dec!(111)),
            candle(1, dec!(111), dec!(113), dec!(109), dec!(112)),
            candle(2, dec!(112), dec!(114), dec!(100), dec!(110)),
            candle(3, dec!(110), dec!(113), dec!(108), dec!(112)),
            candle(4, dec!(112), dec!(115), dec!(109), dec!(114)),
            candle(5, dec!(114), dec!(116), dec!(101), dec!(113)),
            candle(6, dec!(113), dec!(115), dec!(100.3), dec!(112)),
            candle(7, dec!(101), dec!(108), dec!(99.8), dec!(107)),
        ];
        let strategy = KeyLevelStrategy::new(6, 2, dec!(0.005));
        let decision = strategy.evaluate(ctx(Side::Long, candles));

        assert!(matches!(
            decision,
            SignalDecision::SignalConfirmed {
                reason: SignalReason::KeyLevelReaction {
                    level_kind: KeyLevelKind::Support,
                    level_price
                },
                ..
            } if level_price == dec!(100)
        ));
    }

    #[test]
    fn key_level_strategy_rejects_without_reaction_confirmation() {
        let candles = vec![
            candle(0, dec!(110), dec!(112), dec!(108), dec!(111)),
            candle(1, dec!(111), dec!(113), dec!(109), dec!(112)),
            candle(2, dec!(112), dec!(114), dec!(100), dec!(110)),
            candle(3, dec!(110), dec!(113), dec!(108), dec!(112)),
            candle(4, dec!(112), dec!(115), dec!(109), dec!(114)),
            candle(5, dec!(114), dec!(116), dec!(101), dec!(113)),
            candle(6, dec!(113), dec!(115), dec!(100.3), dec!(112)),
            candle(7, dec!(107), dec!(108), dec!(99.8), dec!(100.5)),
        ];
        let strategy = KeyLevelStrategy::new(6, 2, dec!(0.005));
        assert_eq!(strategy.evaluate(ctx(Side::Long, candles)), SignalDecision::NoSignal);
    }

    #[test]
    fn registry_maps_policies_to_v1_strategies() {
        let registry = StrategyRegistry::new();
        let trend_id = StrategyRegistry::strategy_id_for_policy(EntryPolicy::ConfirmedTrend)
            .expect("confirmed trend must resolve");
        let reversal_id = StrategyRegistry::strategy_id_for_policy(EntryPolicy::ConfirmedReversal)
            .expect("confirmed reversal must resolve");
        let key_level_id = StrategyRegistry::strategy_id_for_policy(EntryPolicy::ConfirmedKeyLevel)
            .expect("confirmed key level must resolve");

        assert_eq!(trend_id, StrategyId::new("sma_crossover", 1));
        assert_eq!(reversal_id, StrategyId::new("reversal_patterns", 1));
        assert_eq!(key_level_id, StrategyId::new("key_level", 1));
        assert!(StrategyRegistry::strategy_id_for_policy(EntryPolicy::Immediate).is_none());
        assert!(registry.get(&trend_id).is_some());
        assert!(registry.get(&reversal_id).is_some());
        assert!(registry.get(&key_level_id).is_some());
    }

    #[test]
    fn strategy_determinism_same_candles_same_decision() {
        // Each strategy must produce identical SignalDecisions when given the
        // same candles, evaluated 100 times.
        let registry = StrategyRegistry::new();

        // SMA crossover candles (long confirmation).
        let sma_candles = vec![
            candle(0, dec!(100), dec!(100), dec!(100), dec!(100)),
            candle(1, dec!(99), dec!(99), dec!(99), dec!(99)),
            candle(2, dec!(98), dec!(98), dec!(98), dec!(98)),
            candle(3, dec!(97), dec!(97), dec!(97), dec!(97)),
            candle(4, dec!(96), dec!(96), dec!(96), dec!(96)),
            candle(5, dec!(105), dec!(105), dec!(105), dec!(105)),
        ];
        let sma_ctx = ctx(Side::Long, sma_candles.clone());
        let sma_id = StrategyId::new("sma_crossover", 1);
        let sma_strategy = registry.get(&sma_id).expect("sma strategy registered");
        let first = sma_strategy.evaluate(sma_ctx.clone());
        for _ in 0..100 {
            assert_eq!(sma_strategy.evaluate(sma_ctx.clone()), first, "SMA strategy not deterministic");
        }

        // Reversal pattern candles (long hammer).
        let reversal_candles = vec![
            candle(0, dec!(110), dec!(111), dec!(109), dec!(109)),
            candle(1, dec!(109), dec!(110), dec!(107), dec!(107)),
            candle(2, dec!(107), dec!(108), dec!(105), dec!(105)),
            candle(3, dec!(103), dec!(104), dec!(95), dec!(102)),
        ];
        let rev_ctx = ctx(Side::Long, reversal_candles.clone());
        let rev_id = StrategyId::new("reversal_patterns", 1);
        let rev_strategy = registry.get(&rev_id).expect("reversal strategy registered");
        let first = rev_strategy.evaluate(rev_ctx.clone());
        for _ in 0..100 {
            assert_eq!(rev_strategy.evaluate(rev_ctx.clone()), first, "Reversal strategy not deterministic");
        }

        // Key level candles (support reaction).
        let kl_candles = vec![
            candle(0, dec!(110), dec!(112), dec!(108), dec!(111)),
            candle(1, dec!(111), dec!(113), dec!(109), dec!(112)),
            candle(2, dec!(112), dec!(114), dec!(100), dec!(110)),
            candle(3, dec!(110), dec!(113), dec!(108), dec!(112)),
            candle(4, dec!(112), dec!(115), dec!(109), dec!(114)),
            candle(5, dec!(114), dec!(116), dec!(101), dec!(113)),
            candle(6, dec!(113), dec!(115), dec!(100.3), dec!(112)),
            candle(7, dec!(101), dec!(108), dec!(99.8), dec!(107)),
        ];
        let kl_ctx = ctx(Side::Long, kl_candles.clone());
        let kl_id = StrategyId::new("key_level", 1);
        let kl_strategy = registry.get(&kl_id).expect("key level strategy registered");
        let first = kl_strategy.evaluate(kl_ctx.clone());
        for _ in 0..100 {
            assert_eq!(kl_strategy.evaluate(kl_ctx.clone()), first, "Key level strategy not deterministic");
        }
    }
}
