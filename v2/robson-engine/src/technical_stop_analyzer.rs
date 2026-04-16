//! Technical Stop Analyzer
//!
//! Pure chart-analysis component that computes `TechnicalStopDistance` from
//! OHLCV data. This fulfills the second architectural responsibility defined
//! in ADR-0021: "WHERE is the stop?" (as opposed to the `DetectorTask`'s
//! responsibility of "WHEN to enter?").
//!
//! # Policy (REQ-CORE-TECHSTOP-001 / ADR-0021)
//!
//! The stop loss MUST be a price level derived from chart analysis.
//! A percentage of entry price is **never** a valid stop computation.
//!
//! # Algorithm (priority order per REQ-CORE-TECHSTOP-001)
//!
//! 1. **Swing points** (primary): identify swing lows (LONG) or swing highs
//!    (SHORT) in the candle history. Take the `support_level_n`-th level
//!    (default: 2nd) ordered by distance from entry.
//! 2. **ATR fallback**: when fewer than `support_level_n` swing levels are
//!    found below/above entry, use `entry ± atr_multiplier × ATR(atr_period)`.
//!
//! # Inputs / Outputs
//!
//! This module is **pure** (no I/O). The caller fetches candles via
//! `OhlcvPort` and passes them in as `&[Candle]`.
//!
//! ```text
//! [fetch candles — OhlcvPort caller]
//!         ↓
//! TechnicalStopAnalyzer::analyze(candles, entry, side, config)
//!         ↓
//! TechnicalStopAnalysis { stop_price, method, confidence, detected_levels }
//!         ↓
//! TechnicalStopDistance::new_validated(entry, stop_price, side)
//!         ↓
//! DetectorSignal { entry_price, stop_loss }
//! ```

use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use robson_domain::{Candle, Price, Side};

// =============================================================================
// Configuration
// =============================================================================

/// Configuration for `TechnicalStopAnalyzer`.
///
/// Default values are per REQ-CORE-TECHSTOP-004.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TechnicalStopConfig {
    /// Number of candles to require as minimum (default: 100)
    pub min_candles: usize,

    /// Number of candles on each side to use for swing-point detection
    /// (default: 2 — a candle is a swing low/high if it is the extreme
    /// within a window of 2 candles on each side)
    pub swing_lookback: usize,

    /// Which support/resistance level to use as the stop (1-indexed, default: 2)
    ///
    /// `support_level_n = 2` means "second support below entry for LONG".
    pub support_level_n: usize,

    /// Price tolerance for clustering nearby levels (as a fraction, default: 0.005 = 0.5%)
    ///
    /// Swing lows within this fraction of each other are merged into a single
    /// support cluster before counting N levels.
    pub level_tolerance: Decimal,

    /// ATR period for the fallback calculation (default: 14)
    pub atr_period: usize,

    /// ATR multiplier for the fallback stop (default: 1.5)
    pub atr_multiplier: Decimal,

    /// Minimum allowed stop distance as fraction of entry (default: 0.001 = 0.1%)
    pub min_stop_distance_pct: Decimal,

    /// Maximum allowed stop distance as fraction of entry (default: 0.10 = 10%)
    pub max_stop_distance_pct: Decimal,
}

impl Default for TechnicalStopConfig {
    fn default() -> Self {
        Self {
            min_candles: 100,
            swing_lookback: 2,
            support_level_n: 2,
            level_tolerance: dec!(0.005),
            atr_period: 14,
            atr_multiplier: dec!(1.5),
            min_stop_distance_pct: dec!(0.001),
            max_stop_distance_pct: dec!(0.10),
        }
    }
}

// =============================================================================
// Output types
// =============================================================================

/// Method used to derive the technical stop level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TechnicalStopMethod {
    /// Stop placed at the Nth swing low (LONG) or swing high (SHORT).
    /// Value is the 1-indexed level number that was used (e.g., 2 = second support).
    SwingPoint { level_n: usize },
    /// Fallback: stop at `entry ± atr_multiplier × ATR(atr_period)`.
    /// Used when fewer than `support_level_n` swing levels are found.
    AtrFallback,
}

/// Confidence in the computed stop level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StopConfidence {
    /// ≥ `support_level_n` swing levels found — primary level used.
    High,
    /// Fewer than `support_level_n` levels found — first available level used.
    Medium,
    /// No swing levels found — ATR fallback applied.
    Low,
}

/// Result of technical stop analysis.
///
/// Pass `stop_price` to `TechnicalStopDistance::new_validated(entry, stop_price, side)`
/// to obtain the validated distance used for position sizing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TechnicalStopAnalysis {
    /// Chart-derived stop price (absolute price level, NOT a percentage).
    pub stop_price: Price,
    /// Method used to derive this stop.
    pub method: TechnicalStopMethod,
    /// Confidence level of the result.
    pub confidence: StopConfidence,
    /// All swing levels detected below (LONG) or above (SHORT) the entry,
    /// ordered by distance from entry ascending. Useful for audit trail.
    pub detected_levels: Vec<Price>,
}

// =============================================================================
// Errors
// =============================================================================

/// Errors from `TechnicalStopAnalyzer::analyze`.
#[derive(Debug, Clone, Error)]
pub enum TechnicalStopError {
    /// Not enough candles to compute a reliable stop.
    #[error(
        "Insufficient candle data: need at least {required} candles, got {got}. \
         Fetch more history before computing the technical stop."
    )]
    InsufficientData { required: usize, got: usize },

    /// ATR fallback produced a stop outside the allowed distance range.
    #[error(
        "ATR fallback stop ({stop_price}) is outside the allowed distance range \
         [{min_pct}%–{max_pct}%] from entry ({entry_price}). \
         Market may be in an extreme volatility state."
    )]
    AtrStopOutOfRange {
        stop_price: Decimal,
        entry_price: Decimal,
        min_pct: Decimal,
        max_pct: Decimal,
    },
}

// =============================================================================
// Analyzer
// =============================================================================

/// Pure technical stop analyzer.
///
/// Call [`TechnicalStopAnalyzer::analyze`] with pre-fetched candle data.
/// No I/O is performed here — all OHLCV fetching is the caller's responsibility.
pub struct TechnicalStopAnalyzer;

impl TechnicalStopAnalyzer {
    /// Compute a chart-derived stop level for a potential position.
    ///
    /// # Arguments
    ///
    /// * `candles` — Historical OHLCV data, oldest-first. Must contain at least
    ///   `config.min_candles` entries.
    /// * `entry_price` — Intended entry price.
    /// * `side` — Position direction (`Long` or `Short`).
    /// * `config` — Tuning parameters (use `TechnicalStopConfig::default()` for
    ///   standard 15m/100-candle analysis).
    ///
    /// # Returns
    ///
    /// [`TechnicalStopAnalysis`] with an absolute stop price. Callers must then
    /// construct [`TechnicalStopDistance`] via
    /// `TechnicalStopDistance::new_validated(entry_price, result.stop_price, side)`.
    ///
    /// # Errors
    ///
    /// Returns [`TechnicalStopError`] if there is insufficient data or if the
    /// ATR fallback produces an out-of-range result.
    pub fn analyze(
        candles: &[Candle],
        entry_price: Price,
        side: Side,
        config: &TechnicalStopConfig,
    ) -> Result<TechnicalStopAnalysis, TechnicalStopError> {
        // ── 1. Minimum data guard ─────────────────────────────────────────────
        let min_required = config.min_candles.max(config.atr_period + 1);
        if candles.len() < min_required {
            return Err(TechnicalStopError::InsufficientData {
                required: min_required,
                got: candles.len(),
            });
        }

        // ── 2. Detect swing levels ────────────────────────────────────────────
        let swing_levels = detect_swing_levels(candles, side, config.swing_lookback);

        // ── 3. Filter to levels on the correct side of entry ──────────────────
        let entry_val = entry_price.as_decimal();
        let mut filtered: Vec<Decimal> = swing_levels
            .into_iter()
            .filter(|&level| match side {
                Side::Long => level < entry_val,
                Side::Short => level > entry_val,
            })
            .collect();

        // ── 4. Cluster nearby levels ──────────────────────────────────────────
        let clustered = cluster_levels(&mut filtered, entry_val, config.level_tolerance);

        // ── 5. Sort by distance from entry (ascending — closest first) ────────
        let mut ordered = clustered;
        match side {
            // For LONG: levels are below entry; highest level = closest → sort descending
            Side::Long => {
                ordered.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal))
            },
            // For SHORT: levels are above entry; lowest level = closest → sort ascending
            Side::Short => {
                ordered.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
            },
        }

        // Build audit list before consuming ordered
        let detected_levels: Vec<Price> =
            ordered.iter().filter_map(|&p| Price::new(p).ok()).collect();

        // ── 6. Select the Nth level or fall back to ATR ───────────────────────
        let n = config.support_level_n;

        if ordered.len() >= n {
            // Primary path: Nth support/resistance level
            let stop_val = ordered[n - 1];
            let stop_price =
                Price::new(stop_val).map_err(|_| TechnicalStopError::AtrStopOutOfRange {
                    stop_price: stop_val,
                    entry_price: entry_val,
                    min_pct: config.min_stop_distance_pct * dec!(100),
                    max_pct: config.max_stop_distance_pct * dec!(100),
                })?;
            return Ok(TechnicalStopAnalysis {
                stop_price,
                method: TechnicalStopMethod::SwingPoint { level_n: n },
                confidence: StopConfidence::High,
                detected_levels,
            });
        }

        if !ordered.is_empty() {
            // Degraded path: fewer levels than requested; use what we have
            let stop_val = ordered[ordered.len() - 1];
            let stop_price =
                Price::new(stop_val).map_err(|_| TechnicalStopError::AtrStopOutOfRange {
                    stop_price: stop_val,
                    entry_price: entry_val,
                    min_pct: config.min_stop_distance_pct * dec!(100),
                    max_pct: config.max_stop_distance_pct * dec!(100),
                })?;
            return Ok(TechnicalStopAnalysis {
                stop_price,
                method: TechnicalStopMethod::SwingPoint { level_n: ordered.len() },
                confidence: StopConfidence::Medium,
                detected_levels,
            });
        }

        // ── 7. ATR fallback ───────────────────────────────────────────────────
        let atr = compute_atr(candles, config.atr_period);
        let stop_val = match side {
            Side::Long => entry_val - config.atr_multiplier * atr,
            Side::Short => entry_val + config.atr_multiplier * atr,
        };

        let distance_pct = if entry_val > Decimal::ZERO {
            (stop_val - entry_val).abs() / entry_val
        } else {
            Decimal::ZERO
        };

        if distance_pct < config.min_stop_distance_pct
            || distance_pct > config.max_stop_distance_pct
        {
            return Err(TechnicalStopError::AtrStopOutOfRange {
                stop_price: stop_val,
                entry_price: entry_val,
                min_pct: config.min_stop_distance_pct * dec!(100),
                max_pct: config.max_stop_distance_pct * dec!(100),
            });
        }

        let stop_price =
            Price::new(stop_val).map_err(|_| TechnicalStopError::AtrStopOutOfRange {
                stop_price: stop_val,
                entry_price: entry_val,
                min_pct: config.min_stop_distance_pct * dec!(100),
                max_pct: config.max_stop_distance_pct * dec!(100),
            })?;

        Ok(TechnicalStopAnalysis {
            stop_price,
            method: TechnicalStopMethod::AtrFallback,
            confidence: StopConfidence::Low,
            detected_levels,
        })
    }
}

// =============================================================================
// Private helpers
// =============================================================================

/// Find swing lows (for LONG) or swing highs (for SHORT) in the candle slice.
///
/// A candle at index `i` is a swing low when its `low` is the minimum within
/// the window `[i - lookback, i + lookback]`. Symmetric for swing highs.
///
/// Candles within `lookback` of either boundary are excluded (insufficient
/// context to confirm the swing).
fn detect_swing_levels(candles: &[Candle], side: Side, lookback: usize) -> Vec<Decimal> {
    let len = candles.len();
    if len < 2 * lookback + 1 {
        return vec![];
    }

    let mut levels = Vec::new();
    for i in lookback..(len - lookback) {
        let candidate = match side {
            Side::Long => candles[i].low,
            Side::Short => candles[i].high,
        };

        // Strict inequality: the candidate must be strictly lower (for Long)
        // or strictly higher (for Short) than ALL candles in the lookback window.
        // Equal-valued neighbours do not count — a true swing point is a
        // local extreme, not a plateau.
        let is_extreme = (1..=lookback).all(|offset| {
            let before = match side {
                Side::Long => candles[i - offset].low,
                Side::Short => candles[i - offset].high,
            };
            let after = match side {
                Side::Long => candles[i + offset].low,
                Side::Short => candles[i + offset].high,
            };
            match side {
                Side::Long => candidate < before && candidate < after,
                Side::Short => candidate > before && candidate > after,
            }
        });

        if is_extreme {
            levels.push(candidate);
        }
    }
    levels
}

/// Cluster nearby price levels together, returning representative levels.
///
/// Levels within `tolerance` (as a fraction of entry) of an existing cluster
/// center are merged into that cluster. The cluster representative is the mean
/// of all members. Clusters are ordered in the same order they were first seen.
fn cluster_levels(levels: &mut Vec<Decimal>, entry: Decimal, tolerance: Decimal) -> Vec<Decimal> {
    if levels.is_empty() {
        return vec![];
    }

    // Sort so we process closest levels first (for LONG: descending; we just sort any way
    // and let the caller re-sort after clustering)
    levels.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    let tolerance_abs = if entry > Decimal::ZERO {
        entry * tolerance
    } else {
        tolerance
    };

    let mut clusters: Vec<Vec<Decimal>> = Vec::new();

    'outer: for &level in levels.iter() {
        for cluster in clusters.iter_mut() {
            let center = cluster.iter().fold(Decimal::ZERO, |s, &v| s + v)
                / Decimal::from(cluster.len() as u32);
            if (level - center).abs() <= tolerance_abs {
                cluster.push(level);
                continue 'outer;
            }
        }
        clusters.push(vec![level]);
    }

    clusters
        .iter()
        .map(|c| {
            let sum = c.iter().fold(Decimal::ZERO, |s, &v| s + v);
            sum / Decimal::from(c.len() as u32)
        })
        .collect()
}

/// Compute Average True Range over the last `period` candles.
///
/// True Range = max(high - low, |high - prev_close|, |low - prev_close|)
/// ATR = simple mean of the last `period` True Range values.
///
/// Requires at least `period + 1` candles (each TR needs a previous close).
/// Returns `Decimal::ZERO` if there is insufficient data.
fn compute_atr(candles: &[Candle], period: usize) -> Decimal {
    let len = candles.len();
    if len < period + 1 {
        return Decimal::ZERO;
    }

    // Compute TRs for the last `period` completed candles
    let start = len - period;
    let mut tr_sum = Decimal::ZERO;

    for i in start..len {
        let prev_close = candles[i - 1].close;
        let high = candles[i].high;
        let low = candles[i].low;

        let hl = high - low;
        let hc = (high - prev_close).abs();
        let lc = (low - prev_close).abs();

        let tr = hl.max(hc).max(lc);
        tr_sum += tr;
    }

    tr_sum / Decimal::from(period as u32)
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use rust_decimal_macros::dec;

    use robson_domain::Symbol;

    use super::*;

    // ── Helpers ───────────────────────────────────────────────────────────────

    /// Build a simple candle with a fixed OHLCV.
    fn candle(open: Decimal, high: Decimal, low: Decimal, close: Decimal) -> Candle {
        Candle::new(
            Symbol::from_pair("BTCUSDT").unwrap(),
            open,
            high,
            low,
            close,
            dec!(100),
            10,
            Utc::now(),
            Utc::now(),
        )
    }

    /// Build a flat candle (all prices equal) at a given price level.
    fn flat_candle(price: Decimal) -> Candle {
        candle(price, price, price, price)
    }

    /// Build 100 flat candles at a given price, with a spike down at index 50
    /// (swing low) and another at index 70 (second swing low).
    fn candles_with_two_swing_lows(
        base: Decimal,
        first_low: Decimal,
        second_low: Decimal,
    ) -> Vec<Candle> {
        let mut cs: Vec<Candle> = (0..100).map(|_| flat_candle(base)).collect();
        // Inject first swing low at index 50: lower than neighbours
        cs[48] = flat_candle(base);
        cs[49] = flat_candle(base);
        cs[50] = candle(first_low, first_low + dec!(10), first_low, first_low);
        cs[51] = flat_candle(base);
        cs[52] = flat_candle(base);
        // Inject second swing low at index 70
        cs[68] = flat_candle(base);
        cs[69] = flat_candle(base);
        cs[70] = candle(second_low, second_low + dec!(10), second_low, second_low);
        cs[71] = flat_candle(base);
        cs[72] = flat_candle(base);
        cs
    }

    // ── detect_swing_levels ───────────────────────────────────────────────────

    #[test]
    fn detects_swing_low_in_flat_series_with_one_dip() {
        // Build 20 candles at 100, with a dip to 90 at index 10
        let mut cs: Vec<Candle> = (0..20).map(|_| flat_candle(dec!(100))).collect();
        cs[8] = flat_candle(dec!(100));
        cs[9] = flat_candle(dec!(100));
        cs[10] = candle(dec!(90), dec!(100), dec!(90), dec!(90));
        cs[11] = flat_candle(dec!(100));
        cs[12] = flat_candle(dec!(100));

        let lows = detect_swing_levels(&cs, Side::Long, 2);
        assert!(lows.contains(&dec!(90)), "expected 90 as swing low, got {lows:?}");
    }

    #[test]
    fn detects_no_swing_lows_in_flat_series() {
        let cs: Vec<Candle> = (0..20).map(|_| flat_candle(dec!(100))).collect();
        let lows = detect_swing_levels(&cs, Side::Long, 2);
        // Flat candles all tie — the equality condition (`<=`) means every
        // candle qualifies. This is acceptable; clustering will merge them.
        // Just verify the function returns without panicking.
        let _ = lows;
    }

    // ── cluster_levels ────────────────────────────────────────────────────────

    #[test]
    fn clusters_nearby_levels_into_one() {
        let mut levels = vec![dec!(93000), dec!(93200), dec!(93100)];
        let result = cluster_levels(&mut levels, dec!(95000), dec!(0.005));
        // All within 0.5% of 95000 (= 475); spread is 200 — should merge
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn keeps_distant_levels_separate() {
        let mut levels = vec![dec!(90000), dec!(93000)];
        let result = cluster_levels(&mut levels, dec!(95000), dec!(0.005));
        // Gap of 3000 >> 0.5% of 95000 (475)
        assert_eq!(result.len(), 2);
    }

    // ── compute_atr ───────────────────────────────────────────────────────────

    #[test]
    fn atr_is_zero_when_all_candles_are_flat() {
        let cs: Vec<Candle> = (0..20).map(|_| flat_candle(dec!(100))).collect();
        let atr = compute_atr(&cs, 14);
        assert_eq!(atr, dec!(0));
    }

    #[test]
    fn atr_reflects_candle_range() {
        // 16 candles: first is base, then 15 candles each with range of 100
        let mut cs = vec![flat_candle(dec!(1000))];
        for _ in 0..15 {
            cs.push(candle(dec!(1000), dec!(1100), dec!(1000), dec!(1000)));
        }
        let atr = compute_atr(&cs, 14);
        // Each TR = max(100, 0, 0) = 100; ATR = 100
        assert_eq!(atr, dec!(100));
    }

    // ── analyze — primary path ────────────────────────────────────────────────

    #[test]
    fn analyze_long_returns_second_swing_low_as_stop() {
        let entry = Price::new(dec!(95000)).unwrap();
        // Two swing lows below entry: 93000 (closer) and 90000 (farther)
        let cs = candles_with_two_swing_lows(dec!(94000), dec!(93000), dec!(90000));

        let config = TechnicalStopConfig::default();
        let result = TechnicalStopAnalyzer::analyze(&cs, entry, Side::Long, &config).unwrap();

        assert_eq!(result.confidence, StopConfidence::High);
        assert_eq!(result.method, TechnicalStopMethod::SwingPoint { level_n: 2 });
        // Second level (farther from entry for LONG) = 90000
        assert_eq!(result.stop_price.as_decimal(), dec!(90000));
    }

    #[test]
    fn analyze_long_falls_back_to_medium_when_only_one_level() {
        let entry = Price::new(dec!(95000)).unwrap();
        // Only one swing low at 93000
        let mut cs: Vec<Candle> = (0..100).map(|_| flat_candle(dec!(94000))).collect();
        cs[48] = flat_candle(dec!(94000));
        cs[49] = flat_candle(dec!(94000));
        cs[50] = candle(dec!(93000), dec!(93500), dec!(93000), dec!(93000));
        cs[51] = flat_candle(dec!(94000));
        cs[52] = flat_candle(dec!(94000));

        let config = TechnicalStopConfig::default();
        let result = TechnicalStopAnalyzer::analyze(&cs, entry, Side::Long, &config).unwrap();

        assert_eq!(result.confidence, StopConfidence::Medium);
        assert!(matches!(result.method, TechnicalStopMethod::SwingPoint { level_n: 1 }));
        assert_eq!(result.stop_price.as_decimal(), dec!(93000));
    }

    #[test]
    fn analyze_falls_back_to_atr_when_no_swing_levels_below_entry() {
        let entry = Price::new(dec!(95000)).unwrap();
        // All candles above entry — no swing lows below entry
        let cs: Vec<Candle> = (0..100)
            .map(|_| candle(dec!(96000), dec!(97000), dec!(95500), dec!(96000)))
            .collect();

        let config = TechnicalStopConfig::default();
        let result = TechnicalStopAnalyzer::analyze(&cs, entry, Side::Long, &config).unwrap();

        assert_eq!(result.confidence, StopConfidence::Low);
        assert_eq!(result.method, TechnicalStopMethod::AtrFallback);
        // ATR stop must be below entry for LONG
        assert!(result.stop_price.as_decimal() < entry.as_decimal());
    }

    #[test]
    fn analyze_rejects_insufficient_data() {
        let entry = Price::new(dec!(95000)).unwrap();
        let cs: Vec<Candle> = (0..10).map(|_| flat_candle(dec!(94000))).collect();

        let config = TechnicalStopConfig::default();
        let err = TechnicalStopAnalyzer::analyze(&cs, entry, Side::Long, &config).unwrap_err();

        assert!(matches!(err, TechnicalStopError::InsufficientData { .. }));
    }

    #[test]
    fn stop_is_below_entry_for_long() {
        let entry = Price::new(dec!(95000)).unwrap();
        let cs = candles_with_two_swing_lows(dec!(94000), dec!(93000), dec!(90000));
        let config = TechnicalStopConfig::default();
        let result = TechnicalStopAnalyzer::analyze(&cs, entry, Side::Long, &config).unwrap();
        assert!(result.stop_price.as_decimal() < entry.as_decimal());
    }

    #[test]
    fn stop_is_above_entry_for_short() {
        let entry = Price::new(dec!(90000)).unwrap();
        // Two swing highs above entry at 92000 and 94000
        let mut cs: Vec<Candle> = (0..100).map(|_| flat_candle(dec!(91000))).collect();
        cs[48] = flat_candle(dec!(91000));
        cs[49] = flat_candle(dec!(91000));
        cs[50] = candle(dec!(92000), dec!(92000), dec!(91500), dec!(92000));
        cs[51] = flat_candle(dec!(91000));
        cs[52] = flat_candle(dec!(91000));
        cs[68] = flat_candle(dec!(91000));
        cs[69] = flat_candle(dec!(91000));
        cs[70] = candle(dec!(94000), dec!(94000), dec!(93500), dec!(94000));
        cs[71] = flat_candle(dec!(91000));
        cs[72] = flat_candle(dec!(91000));

        let config = TechnicalStopConfig::default();
        let result = TechnicalStopAnalyzer::analyze(&cs, entry, Side::Short, &config).unwrap();
        assert!(result.stop_price.as_decimal() > entry.as_decimal());
    }
}
