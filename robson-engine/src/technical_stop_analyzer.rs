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
//!    (SHORT) in the candle history, merge nearby levels into clusters, and
//!    take the `support_level_n`-th cluster (default: 2nd) ordered by distance
//!    from entry. A cluster is represented by its **adverse extreme** — the
//!    deepest member for LONG, the highest for SHORT — so the stop anchors
//!    beyond the technical event, never at the zone's statistical center
//!    (ADR-0053).
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

use robson_domain::{Candle, Price, Side};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::{Deserialize, Serialize};
use thiserror::Error;

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

    /// Which support/resistance level to use as the stop (1-indexed, default:
    /// 2)
    ///
    /// `support_level_n = 2` means "second support below entry for LONG".
    pub support_level_n: usize,

    /// Price tolerance for clustering nearby levels (as a fraction, default:
    /// 0.005 = 0.5%)
    ///
    /// Swing lows within this fraction of each other are merged into a single
    /// support cluster before counting N levels.
    pub level_tolerance: Decimal,

    /// ATR period for the fallback calculation (default: 14)
    pub atr_period: usize,

    /// ATR multiplier for the fallback stop (default: 1.5)
    pub atr_multiplier: Decimal,

    /// Minimum allowed stop distance as fraction of entry (default: 0.001 =
    /// 0.1%)
    pub min_stop_distance_pct: Decimal,

    /// Maximum allowed stop distance as fraction of entry (default: 0.10 = 10%)
    pub max_stop_distance_pct: Decimal,
}

impl Default for TechnicalStopConfig {
    fn default() -> Self {
        Self::with_bounds(&robson_domain::StopDistanceBounds::default())
    }
}

impl TechnicalStopConfig {
    /// Build the analyzer config from the single distance-bounds source
    /// (ADR-0050 §5). The distance fields are derived from `bounds` so the
    /// analyzer, the domain validation, and sizing observe the same limits.
    pub fn with_bounds(bounds: &robson_domain::StopDistanceBounds) -> Self {
        Self {
            min_candles: 100,
            swing_lookback: 2,
            support_level_n: 2,
            level_tolerance: dec!(0.005),
            atr_period: 14,
            atr_multiplier: dec!(1.5),
            min_stop_distance_pct: bounds.min_fraction(),
            max_stop_distance_pct: bounds.max_fraction(),
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
    /// Value is the 1-indexed level number that was used (e.g., 2 = second
    /// support).
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

/// Why a chart level was skipped by valid-level selection (ADR-0050 §1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SkipReason {
    /// Level distance is below the minimum stop distance bound.
    BelowMin,
    /// Level distance is above the maximum stop distance bound.
    AboveMax,
}

impl SkipReason {
    /// Stable snake_case identifier for audit payloads.
    pub fn as_str(&self) -> &'static str {
        match self {
            SkipReason::BelowMin => "below_min",
            SkipReason::AboveMax => "above_max",
        }
    }
}

/// A chart level that selection considered and skipped (ADR-0050 §1 audit).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkippedLevel {
    /// The skipped chart level.
    pub level: Price,
    /// Its distance from entry as a fraction (0.001 = 0.1%).
    pub distance_fraction: Decimal,
    /// Why it was skipped.
    pub reason: SkipReason,
}

/// Result of technical stop analysis.
///
/// Pass `stop_price` to `TechnicalStopDistance::new_validated(entry,
/// stop_price, side)` to obtain the validated distance used for position
/// sizing.
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
    /// The `support_level_n` the selection anchored at (ADR-0050 §1).
    #[serde(default = "default_configured_level_n")]
    pub configured_level_n: usize,
    /// The 1-indexed level actually selected; `None` for the ATR fallback.
    #[serde(default)]
    pub selected_level_n: Option<usize>,
    /// Levels considered and skipped during the anchor-N walk (ADR-0050 §1).
    #[serde(default)]
    pub skipped_levels: Vec<SkippedLevel>,
    /// Clustering representative rule in effect for this analysis
    /// (ADR-0053). Historical payloads deserialize as
    /// [`ClusterRepresentative::Mean`].
    #[serde(default)]
    pub cluster_representative: ClusterRepresentative,
}

/// Which statistic of a support/resistance cluster anchors the stop
/// (ADR-0053 audit field).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ClusterRepresentative {
    /// Pre-ADR-0053 behavior: mean of the cluster members. Only produced by
    /// historical payloads; current analysis never emits this.
    #[default]
    Mean,
    /// Adverse extreme of the cluster members: minimum for Long supports,
    /// maximum for Short resistances (REQ-CORE-TECHSTOP-001 conformance).
    AdverseExtreme,
}

impl ClusterRepresentative {
    /// Stable snake_case identifier for audit payloads.
    pub fn as_str(&self) -> &'static str {
        match self {
            ClusterRepresentative::Mean => "mean",
            ClusterRepresentative::AdverseExtreme => "adverse_extreme",
        }
    }
}

fn default_configured_level_n() -> usize {
    2
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
/// No I/O is performed here — all OHLCV fetching is the caller's
/// responsibility.
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
    /// `TechnicalStopDistance::new_validated(entry_price, result.stop_price,
    /// side)`.
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
        let clustered = cluster_levels(&mut filtered, entry_val, config.level_tolerance, side);

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
        let mut skipped_levels: Vec<SkippedLevel> = Vec::new();
        let out_of_range = |stop_val: Decimal| TechnicalStopError::AtrStopOutOfRange {
            stop_price: stop_val,
            entry_price: entry_val,
            min_pct: config.min_stop_distance_pct * dec!(100),
            max_pct: config.max_stop_distance_pct * dec!(100),
        };
        let distance_fraction = |level: Decimal| {
            if entry_val > Decimal::ZERO {
                (level - entry_val).abs() / entry_val
            } else {
                Decimal::ZERO
            }
        };

        if ordered.len() >= n {
            // Primary path (ADR-0050 §1): anchor at the configured Nth level
            // and walk DEEPER ONLY until the first level whose distance is
            // within bounds. Never select a level shallower than N; a level
            // above the maximum ends the walk (deeper levels are wider
            // still). Every skip is recorded for the audit trail.
            for (index, &level_val) in ordered.iter().enumerate().skip(n - 1) {
                let fraction = distance_fraction(level_val);
                if fraction < config.min_stop_distance_pct {
                    if let Ok(level) = Price::new(level_val) {
                        skipped_levels.push(SkippedLevel {
                            level,
                            distance_fraction: fraction,
                            reason: SkipReason::BelowMin,
                        });
                    }
                    continue;
                }
                if fraction > config.max_stop_distance_pct {
                    if let Ok(level) = Price::new(level_val) {
                        skipped_levels.push(SkippedLevel {
                            level,
                            distance_fraction: fraction,
                            reason: SkipReason::AboveMax,
                        });
                    }
                    break;
                }

                let selected_level_n = index + 1;
                let stop_price = Price::new(level_val).map_err(|_| out_of_range(level_val))?;
                let confidence = if selected_level_n == n {
                    StopConfidence::High
                } else {
                    // Anchor was skipped: a deeper (more conservative) level
                    // carries the stop (ADR-0050 §1 downgrade).
                    StopConfidence::Medium
                };
                return Ok(TechnicalStopAnalysis {
                    stop_price,
                    method: TechnicalStopMethod::SwingPoint { level_n: selected_level_n },
                    confidence,
                    detected_levels,
                    configured_level_n: n,
                    selected_level_n: Some(selected_level_n),
                    skipped_levels,
                    cluster_representative: ClusterRepresentative::AdverseExtreme,
                });
            }
            // No in-bounds level at or beyond the anchor: fall through to the
            // ATR fallback with the recorded skips.
        } else if !ordered.is_empty() {
            // Degraded path: fewer levels than requested; the deepest
            // available level carries the stop only when it is in bounds
            // (ADR-0050 §1 closes the previously unchecked path).
            let index = ordered.len() - 1;
            let level_val = ordered[index];
            let fraction = distance_fraction(level_val);
            if fraction >= config.min_stop_distance_pct && fraction <= config.max_stop_distance_pct
            {
                let stop_price = Price::new(level_val).map_err(|_| out_of_range(level_val))?;
                return Ok(TechnicalStopAnalysis {
                    stop_price,
                    method: TechnicalStopMethod::SwingPoint { level_n: ordered.len() },
                    confidence: StopConfidence::Medium,
                    detected_levels,
                    configured_level_n: n,
                    selected_level_n: Some(ordered.len()),
                    skipped_levels,
                    cluster_representative: ClusterRepresentative::AdverseExtreme,
                });
            }
            if let Ok(level) = Price::new(level_val) {
                skipped_levels.push(SkippedLevel {
                    level,
                    distance_fraction: fraction,
                    reason: if fraction < config.min_stop_distance_pct {
                        SkipReason::BelowMin
                    } else {
                        SkipReason::AboveMax
                    },
                });
            }
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
            configured_level_n: n,
            selected_level_n: None,
            skipped_levels,
            cluster_representative: ClusterRepresentative::AdverseExtreme,
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
/// center are merged into that cluster. Membership is decided against the
/// running mean of the cluster (its geometric center), but the cluster
/// representative is the **adverse extreme** of its members: the minimum for
/// `Long` (deepest support) and the maximum for `Short` (highest resistance).
///
/// The representative is what the stop anchors at, and a stop must sit
/// beyond the technical event, not at the statistical center of the zone
/// (REQ-CORE-TECHSTOP-001 "below/above the level"; ADR-0053). A mean
/// representative places the stop inside the zone's test range, where an
/// ordinary probe of the level fills it without the level ever breaking
/// (2026-08-13 production stop-out, issue #173).
fn cluster_levels(
    levels: &mut Vec<Decimal>,
    entry: Decimal,
    tolerance: Decimal,
    side: Side,
) -> Vec<Decimal> {
    if levels.is_empty() {
        return vec![];
    }

    // Sort so we process closest levels first (for LONG: descending; we just sort
    // any way and let the caller re-sort after clustering)
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
        .filter_map(|c| match side {
            Side::Long => c.iter().min().copied(),
            Side::Short => c.iter().max().copied(),
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
    use robson_domain::Symbol;
    use rust_decimal_macros::dec;

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
        let result = cluster_levels(&mut levels, dec!(95000), dec!(0.005), Side::Long);
        // All within 0.5% of 95000 (= 475); spread is 200 — should merge
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn keeps_distant_levels_separate() {
        let mut levels = vec![dec!(90000), dec!(93000)];
        let result = cluster_levels(&mut levels, dec!(95000), dec!(0.005), Side::Long);
        // Gap of 3000 >> 0.5% of 95000 (475)
        assert_eq!(result.len(), 2);
    }

    /// ADR-0053: the cluster representative is the adverse extreme of the
    /// members, never the mean — deepest member for Long supports.
    #[test]
    fn long_cluster_representative_is_the_minimum_member() {
        let mut levels = vec![dec!(93000), dec!(93200), dec!(93100)];
        let result = cluster_levels(&mut levels, dec!(95000), dec!(0.005), Side::Long);
        assert_eq!(result, vec![dec!(93000)], "Long support anchors at the deepest member");
    }

    /// ADR-0053: highest member for Short resistances.
    #[test]
    fn short_cluster_representative_is_the_maximum_member() {
        let mut levels = vec![dec!(97000), dec!(96800), dec!(96900)];
        let result = cluster_levels(&mut levels, dec!(95000), dec!(0.005), Side::Short);
        assert_eq!(result, vec![dec!(97000)], "Short resistance anchors at the highest member");
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

    /// Contract test (ADR-0050 §5, issue #148): the analyzer config derives
    /// its distance limits from the single `StopDistanceBounds` source, so
    /// analyzer, domain validation, and sizing observe the same values.
    #[test]
    fn test_config_bounds_derive_from_single_source() {
        let bounds = robson_domain::StopDistanceBounds::new(dec!(25), dec!(800)).unwrap();
        let config = TechnicalStopConfig::with_bounds(&bounds);
        assert_eq!(config.min_stop_distance_pct, bounds.min_fraction());
        assert_eq!(config.max_stop_distance_pct, bounds.max_fraction());

        // Default config equals default bounds (historical 0.1%-10%).
        let default_config = TechnicalStopConfig::default();
        let default_bounds = robson_domain::StopDistanceBounds::default();
        assert_eq!(default_config.min_stop_distance_pct, default_bounds.min_fraction());
        assert_eq!(default_config.max_stop_distance_pct, default_bounds.max_fraction());
    }

    // ── ADR-0050 §1: anchor-N walk-deeper selection ────────────────────────

    /// Config with a tiny cluster tolerance so closely spaced test levels
    /// stay distinct, mirroring dense structure near price.
    fn walk_test_config() -> TechnicalStopConfig {
        TechnicalStopConfig {
            level_tolerance: dec!(0.0001),
            ..TechnicalStopConfig::default()
        }
    }

    /// 100 base candles with swing lows injected at well-separated indices.
    fn candles_with_swing_lows(base: Decimal, lows: &[Decimal]) -> Vec<Candle> {
        let mut cs: Vec<Candle> = (0..100).map(|_| flat_candle(base)).collect();
        for (i, &low) in lows.iter().enumerate() {
            let idx = 30 + i * 10;
            cs[idx] = candle(low, base, low, low);
        }
        cs
    }

    /// Anchor level in bounds: selection is unchanged (level N, High, no
    /// skips) and the new audit fields are populated.
    #[test]
    fn anchor_in_bounds_selects_level_n_with_audit_fields() {
        let cs = candles_with_swing_lows(dec!(100), &[dec!(99.5), dec!(99.0)]);
        let entry = Price::new(dec!(100)).unwrap();
        let result =
            TechnicalStopAnalyzer::analyze(&cs, entry, Side::Long, &walk_test_config()).unwrap();

        assert_eq!(result.method, TechnicalStopMethod::SwingPoint { level_n: 2 });
        assert_eq!(result.confidence, StopConfidence::High);
        assert_eq!(result.configured_level_n, 2);
        assert_eq!(result.selected_level_n, Some(2));
        assert!(result.skipped_levels.is_empty());
    }

    /// Regression for the 2026-08-02 production incident (#147): the anchor
    /// level sits inside the minimum distance; selection must walk to the
    /// next deeper in-bounds level instead of failing downstream, with the
    /// skip audited and confidence downgraded.
    #[test]
    fn too_tight_anchor_walks_to_deeper_valid_level() {
        // Long mirror of the short incident: level 1 = 0.05%, level 2
        // (anchor) = 0.07% (too tight), level 3 = 0.5% (valid).
        let cs = candles_with_swing_lows(dec!(100), &[dec!(99.95), dec!(99.93), dec!(99.5)]);
        let entry = Price::new(dec!(100)).unwrap();
        let result =
            TechnicalStopAnalyzer::analyze(&cs, entry, Side::Long, &walk_test_config()).unwrap();

        assert_eq!(result.method, TechnicalStopMethod::SwingPoint { level_n: 3 });
        assert_eq!(result.stop_price.as_decimal(), dec!(99.5));
        assert_eq!(result.confidence, StopConfidence::Medium, "skip downgrades confidence");
        assert_eq!(result.configured_level_n, 2);
        assert_eq!(result.selected_level_n, Some(3));
        assert_eq!(result.skipped_levels.len(), 1);
        assert_eq!(result.skipped_levels[0].level.as_decimal(), dec!(99.93));
        assert_eq!(result.skipped_levels[0].reason, SkipReason::BelowMin);
    }

    /// Every level at or beyond the anchor too tight and no usable ATR:
    /// analysis fails (slice 3 maps this to the per-policy no-valid-stop
    /// outcome) instead of emitting an invalid stop.
    #[test]
    fn all_levels_too_tight_without_atr_fails() {
        let cs = candles_with_swing_lows(dec!(100), &[dec!(99.97), dec!(99.95)]);
        let entry = Price::new(dec!(100)).unwrap();
        let result = TechnicalStopAnalyzer::analyze(&cs, entry, Side::Long, &walk_test_config());
        assert!(
            matches!(result, Err(TechnicalStopError::AtrStopOutOfRange { .. })),
            "expected out-of-range failure, got {result:?}"
        );
    }

    /// Anchor above the maximum: the walk must NOT retreat to the shallower
    /// level 1; with flat candles (no ATR) the analysis fails.
    #[test]
    fn anchor_above_max_never_retreats_to_shallower_level() {
        // Level 1 = 5% (in bounds but shallower than the anchor), level 2
        // (anchor) = 15% (above max).
        let cs = candles_with_swing_lows(dec!(100), &[dec!(95), dec!(85)]);
        let entry = Price::new(dec!(100)).unwrap();
        let result = TechnicalStopAnalyzer::analyze(&cs, entry, Side::Long, &walk_test_config());
        assert!(
            matches!(result, Err(TechnicalStopError::AtrStopOutOfRange { .. })),
            "selection must not fall back to a level shallower than the anchor, got {result:?}"
        );
    }

    /// Degraded path (fewer than N levels): the deepest available level is
    /// bounds-checked now; a too-tight single level no longer produces an
    /// invalid stop.
    #[test]
    fn degraded_path_bounds_checks_the_deepest_level() {
        let cs = candles_with_swing_lows(dec!(100), &[dec!(99.95)]);
        let entry = Price::new(dec!(100)).unwrap();
        let result = TechnicalStopAnalyzer::analyze(&cs, entry, Side::Long, &walk_test_config());
        assert!(
            matches!(result, Err(TechnicalStopError::AtrStopOutOfRange { .. })),
            "too-tight degraded level must not be emitted, got {result:?}"
        );

        // A single in-bounds level still works (existing degraded behavior).
        let cs = candles_with_swing_lows(dec!(100), &[dec!(99.0)]);
        let result =
            TechnicalStopAnalyzer::analyze(&cs, entry, Side::Long, &walk_test_config()).unwrap();
        assert_eq!(result.method, TechnicalStopMethod::SwingPoint { level_n: 1 });
        assert_eq!(result.confidence, StopConfidence::Medium);
        assert_eq!(result.selected_level_n, Some(1));
    }

    /// Short side walk: the incident's actual direction. Anchor resistance
    /// too close above entry walks to the deeper (higher) valid level.
    #[test]
    fn short_side_too_tight_anchor_walks_up() {
        // Swing highs above entry 100: 100.05 (level 1), 100.07 (anchor,
        // too tight), 100.5 (valid).
        let mut cs: Vec<Candle> = (0..100).map(|_| flat_candle(dec!(100))).collect();
        for (i, high) in [dec!(100.05), dec!(100.07), dec!(100.5)].iter().enumerate() {
            let idx = 30 + i * 10;
            cs[idx] = candle(*high, *high, dec!(100), *high);
        }
        let entry = Price::new(dec!(100)).unwrap();
        let result =
            TechnicalStopAnalyzer::analyze(&cs, entry, Side::Short, &walk_test_config()).unwrap();

        assert_eq!(result.method, TechnicalStopMethod::SwingPoint { level_n: 3 });
        assert_eq!(result.stop_price.as_decimal(), dec!(100.5));
        assert_eq!(result.confidence, StopConfidence::Medium);
        assert_eq!(result.skipped_levels.len(), 1);
        assert_eq!(result.skipped_levels[0].reason, SkipReason::BelowMin);
    }

    // ── ADR-0053 production replay fixtures (issue #173) ──────────────────────

    /// 100 base candles with swing highs injected at well-separated indices.
    fn candles_with_swing_highs(base: Decimal, highs: &[Decimal]) -> Vec<Candle> {
        let mut cs: Vec<Candle> = (0..100).map(|_| flat_candle(base)).collect();
        for (i, &high) in highs.iter().enumerate() {
            let idx = 30 + i * 5;
            cs[idx] = candle(high, high, base, high);
        }
        cs
    }

    /// Replay of the 2026-08-13 Short arm (position 019ffb62), the incident
    /// that motivated ADR-0053. The 15m chart carried seven swing highs that
    /// the 0.5% tolerance merges into a single resistance cluster. Under the
    /// mean representative the stop landed at ~63927.78 and a probe that
    /// topped 63980.6 (without breaking the 63990.7 swing high) stopped the
    /// position out. The adverse-extreme representative must anchor at the
    /// cluster's highest member, beyond every level of the zone.
    #[test]
    fn replay_2026_08_13_short_stop_anchors_beyond_the_whole_resistance_zone() {
        let entry = Price::new(dec!(63768.50)).unwrap();
        let zone = [
            dec!(63800.0),
            dec!(63828.0),
            dec!(63881.3),
            dec!(63916.6),
            dec!(63990.7),
            dec!(64050.3),
            dec!(64131.4),
        ];
        let cs = candles_with_swing_highs(dec!(63700), &zone);

        let result = TechnicalStopAnalyzer::analyze(
            &cs,
            entry,
            Side::Short,
            &TechnicalStopConfig::default(),
        )
        .unwrap();

        assert_eq!(result.stop_price.as_decimal(), dec!(64131.4));
        assert!(
            result.stop_price.as_decimal() > dec!(63990.7),
            "stop must clear the swing high the 2026-08-13 probe never broke"
        );
        assert_eq!(result.cluster_representative, ClusterRepresentative::AdverseExtreme);
        assert!(matches!(result.method, TechnicalStopMethod::SwingPoint { .. }));
    }

    /// Replay of the 2026-08-12 Long arm (position 019ff820). Under the mean
    /// representative the single support cluster averaged to 0.069% from
    /// entry, fell below the ADR-0050 minimum bound, and the stop silently
    /// degraded to the ATR fallback. The adverse extreme (63283.0, 0.116%
    /// from entry) is in bounds, so the chart-derived level now carries the
    /// stop.
    #[test]
    fn replay_2026_08_12_long_adverse_extreme_rescues_the_chart_level_from_atr() {
        let entry = Price::new(dec!(63356.40)).unwrap();
        let zone = [dec!(63291.1), dec!(63338.9), dec!(63283.0), dec!(63337.5)];
        let cs = candles_with_swing_lows(dec!(63400), &zone);

        let result =
            TechnicalStopAnalyzer::analyze(&cs, entry, Side::Long, &TechnicalStopConfig::default())
                .unwrap();

        assert_eq!(result.stop_price.as_decimal(), dec!(63283.0));
        assert!(matches!(result.method, TechnicalStopMethod::SwingPoint { .. }));
        assert_ne!(
            result.method,
            TechnicalStopMethod::AtrFallback,
            "the chart level must carry the stop instead of the ATR fallback"
        );
        assert_eq!(result.cluster_representative, ClusterRepresentative::AdverseExtreme);
    }
}
