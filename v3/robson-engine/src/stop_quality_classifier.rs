//! Stop Quality Classifier (ADR-0035)
//!
//! Rule-based classification of stop region quality. Pure function with
//! no I/O, no side effects, and no dependency on detector or runtime.
//!
//! This module is shelf-ready: compiled and tested, awaiting integration
//! into the detector flow in a future slice.
//!
//! # Available inputs (current slice)
//!
//! Only inputs derivable from `TechnicalStopAnalysis` are used:
//! - `method` (SwingPoint vs AtrFallback)
//! - `confidence` (High / Medium / Low)
//! - `detected_levels_count`
//! - `distance_pct`
//!
//! Factors not yet available (volume, candle structure, liquidity sweep,
//! expected RR, anchor freshness, distance in ATR units) contribute
//! zero to the score. The classification will "open up" as those inputs
//! become available in future slices.
//!
//! # Scoring
//!
//! | Factor             | Max points |
//! |--------------------|------------|
//! | Distance efficiency| 20         |
//! | Method quality     | 15         |
//! | Confidence         | 10         |
//! | Detected levels    | 10         |
//! | **Current total**  | **55**     |
//!
//! Default thresholds: Weak ≥ 10, Good ≥ 25, Premium ≥ 40, Exceptional ≥ 60.
//! Exceptional is unreachable with current inputs (max 55 < 60), which is
//! intentional — it requires future factors.

use robson_domain::{StopQuality, StopQualityClassification};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::{Deserialize, Serialize};

use crate::technical_stop_analyzer::{StopConfidence, TechnicalStopMethod};

// =============================================================================
// Input
// =============================================================================

/// Input for stop quality classification.
///
/// Derived from `TechnicalStopAnalysis` and entry context.
/// Fields that are not yet available in the current runtime
/// contribute zero to the score.
#[derive(Debug, Clone)]
pub struct StopQualityInput {
    /// Whether the stop anchor is considered valid.
    pub stop_anchor_valid: bool,
    /// Method used to derive the stop (SwingPoint or AtrFallback).
    pub method: TechnicalStopMethod,
    /// Confidence level from the analyzer.
    pub confidence: StopConfidence,
    /// Number of swing levels detected near the entry.
    pub detected_levels_count: usize,
    /// Stop distance as a fraction of entry price (e.g. 0.015 = 1.5%).
    pub distance_pct: Decimal,
}

// =============================================================================
// Configuration
// =============================================================================

/// Configurable thresholds for stop quality classification.
///
/// Default values are conservative starting points for calibration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StopQualityThresholds {
    // --- Distance (fraction of entry price) ---
    /// Stops closer than this are considered noise.
    pub noise_max_pct: Decimal,
    /// Stops within this range are considered very efficient.
    pub efficient_max_pct: Decimal,
    /// Stops within this range are considered moderate.
    pub moderate_max_pct: Decimal,
    /// Stops at or beyond this are considered distant.
    pub distant_min_pct: Decimal,

    // --- Detected levels ---
    /// Minimum levels for "many" confluence.
    pub many_levels_min: usize,
    /// Minimum levels for "some" confluence.
    pub some_levels_min: usize,

    // --- Score thresholds ---
    /// Minimum score for Weak classification.
    pub weak_min_score: i32,
    /// Minimum score for Good classification.
    pub good_min_score: i32,
    /// Minimum score for Premium classification.
    pub premium_min_score: i32,
    /// Minimum score for Exceptional classification.
    pub exceptional_min_score: i32,
}

impl Default for StopQualityThresholds {
    fn default() -> Self {
        Self {
            noise_max_pct: dec!(0.002),
            efficient_max_pct: dec!(0.008),
            moderate_max_pct: dec!(0.015),
            distant_min_pct: dec!(0.025),
            many_levels_min: 3,
            some_levels_min: 2,
            weak_min_score: 10,
            good_min_score: 25,
            premium_min_score: 40,
            exceptional_min_score: 60,
        }
    }
}

// =============================================================================
// Scoring helpers
// =============================================================================

/// Points for distance efficiency (0–20).
fn distance_points(distance_pct: Decimal, config: &StopQualityThresholds) -> i32 {
    if distance_pct <= config.noise_max_pct {
        0
    } else if distance_pct <= config.efficient_max_pct {
        20
    } else if distance_pct <= config.moderate_max_pct {
        15
    } else if distance_pct < config.distant_min_pct {
        5
    } else {
        0
    }
}

/// Points for stop derivation method (0–15).
fn method_points(method: &TechnicalStopMethod) -> i32 {
    match method {
        TechnicalStopMethod::SwingPoint { level_n } => {
            if *level_n >= 2 { 15 } else { 10 }
        },
        TechnicalStopMethod::AtrFallback => 0,
    }
}

/// Points for analyzer confidence (0–10).
fn confidence_points(confidence: StopConfidence) -> i32 {
    match confidence {
        StopConfidence::High => 10,
        StopConfidence::Medium => 5,
        StopConfidence::Low => 0,
    }
}

/// Points for detected levels count (0–10).
fn levels_points(count: usize, config: &StopQualityThresholds) -> i32 {
    if count >= config.many_levels_min {
        10
    } else if count >= config.some_levels_min {
        7
    } else if count >= 1 {
        3
    } else {
        0
    }
}

// =============================================================================
// Classifier
// =============================================================================

/// Classify stop quality from available inputs.
///
/// Pure function: no I/O, no side effects, no events.
/// `exceptional_enabled` must be explicitly `true` to allow Exceptional
/// classification. In production this parameter MUST be `false` until
/// operational evidence supports enabling it (ADR-0035 Phase 5).
pub fn classify_stop_quality(
    input: &StopQualityInput,
    config: &StopQualityThresholds,
    exceptional_enabled: bool,
) -> StopQualityClassification {
    // 1. Anchor must be valid
    if !input.stop_anchor_valid {
        return StopQualityClassification {
            class: StopQuality::None,
            raw_score: 0,
            boost_pct: Decimal::ZERO,
            shadow_exceptional: false,
            reasons: vec!["Anchor invalid".to_string()],
        };
    }

    // 2. Noise filter — stop too close
    if input.distance_pct <= config.noise_max_pct {
        return StopQualityClassification {
            class: StopQuality::None,
            raw_score: 0,
            boost_pct: Decimal::ZERO,
            shadow_exceptional: false,
            reasons: vec!["Stop inside noise floor".to_string()],
        };
    }

    // 3. Calculate score
    let mut reasons = Vec::new();
    let mut score = 0i32;

    let d_pts = distance_points(input.distance_pct, config);
    score += d_pts;
    if d_pts > 0 {
        reasons.push(format!("Distance efficiency: +{}", d_pts));
    }

    let m_pts = method_points(&input.method);
    score += m_pts;
    if m_pts > 0 {
        let label = match &input.method {
            TechnicalStopMethod::SwingPoint { level_n } => {
                format!("SwingPoint(N={})", level_n)
            },
            TechnicalStopMethod::AtrFallback => "AtrFallback".to_string(),
        };
        reasons.push(format!("Method {}: +{}", label, m_pts));
    }

    let c_pts = confidence_points(input.confidence);
    score += c_pts;
    if c_pts > 0 {
        reasons.push(format!("Confidence {:?}: +{}", input.confidence, c_pts));
    }

    let l_pts = levels_points(input.detected_levels_count, config);
    score += l_pts;
    if l_pts > 0 {
        reasons.push(format!(
            "Detected levels ({}): +{}",
            input.detected_levels_count, l_pts
        ));
    }

    // 4. Classify by thresholds
    let (class, boost_pct, shadow_exceptional) = if score >= config.exceptional_min_score {
        if exceptional_enabled {
            (StopQuality::Exceptional, dec!(0.20), false)
        } else {
            (StopQuality::Premium, dec!(0.15), true)
        }
    } else if score >= config.premium_min_score {
        (StopQuality::Premium, dec!(0.15), false)
    } else if score >= config.good_min_score {
        (StopQuality::Good, dec!(0.10), false)
    } else if score >= config.weak_min_score {
        (StopQuality::Weak, dec!(0.05), false)
    } else {
        (StopQuality::None, Decimal::ZERO, false)
    };

    StopQualityClassification {
        class,
        raw_score: score,
        boost_pct,
        shadow_exceptional,
        reasons,
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use robson_domain::StopQuality;
    use rust_decimal_macros::dec;

    fn default_config() -> StopQualityThresholds {
        StopQualityThresholds::default()
    }

    fn swing_input(
        distance_pct: Decimal,
        confidence: StopConfidence,
        levels: usize,
    ) -> StopQualityInput {
        StopQualityInput {
            stop_anchor_valid: true,
            method: TechnicalStopMethod::SwingPoint { level_n: 2 },
            confidence,
            detected_levels_count: levels,
            distance_pct,
        }
    }

    fn atr_input(
        distance_pct: Decimal,
        confidence: StopConfidence,
        levels: usize,
    ) -> StopQualityInput {
        StopQualityInput {
            stop_anchor_valid: true,
            method: TechnicalStopMethod::AtrFallback,
            confidence,
            detected_levels_count: levels,
            distance_pct,
        }
    }

    #[test]
    fn returns_none_when_anchor_invalid() {
        let input = StopQualityInput {
            stop_anchor_valid: false,
            method: TechnicalStopMethod::SwingPoint { level_n: 2 },
            confidence: StopConfidence::High,
            detected_levels_count: 5,
            distance_pct: dec!(0.01),
        };
        let result = classify_stop_quality(&input, &default_config(), false);
        assert_eq!(result.class, StopQuality::None);
        assert_eq!(result.raw_score, 0);
        assert_eq!(result.boost_pct, Decimal::ZERO);
    }

    #[test]
    fn returns_none_for_noise_floor() {
        let input = swing_input(dec!(0.001), StopConfidence::High, 5);
        let result = classify_stop_quality(&input, &default_config(), false);
        assert_eq!(result.class, StopQuality::None);
        assert_eq!(result.raw_score, 0);
        assert_eq!(result.boost_pct, Decimal::ZERO);
    }

    #[test]
    fn swing_point_scores_above_atr_fallback() {
        let distance = dec!(0.01);
        let swing = classify_stop_quality(
            &swing_input(distance, StopConfidence::High, 3),
            &default_config(),
            false,
        );
        let atr = classify_stop_quality(
            &atr_input(distance, StopConfidence::Low, 0),
            &default_config(),
            false,
        );
        assert!(
            swing.raw_score > atr.raw_score,
            "SwingPoint ({}) should score above AtrFallback ({})",
            swing.raw_score,
            atr.raw_score
        );
        assert!(swing.boost_pct > atr.boost_pct);
    }

    #[test]
    fn high_confidence_scores_above_low_confidence() {
        let high = classify_stop_quality(
            &swing_input(dec!(0.005), StopConfidence::High, 2),
            &default_config(),
            false,
        );
        let low = classify_stop_quality(
            &swing_input(dec!(0.005), StopConfidence::Low, 2),
            &default_config(),
            false,
        );
        assert!(
            high.raw_score > low.raw_score,
            "High ({}) should score above Low ({})",
            high.raw_score,
            low.raw_score
        );
    }

    #[test]
    fn multiple_detected_levels_add_points() {
        let many = classify_stop_quality(
            &swing_input(dec!(0.005), StopConfidence::High, 4),
            &default_config(),
            false,
        );
        let few = classify_stop_quality(
            &swing_input(dec!(0.005), StopConfidence::High, 1),
            &default_config(),
            false,
        );
        assert!(
            many.raw_score > few.raw_score,
            "Many levels ({}) should score above few ({})",
            many.raw_score,
            few.raw_score
        );
    }

    #[test]
    fn caps_exceptional_at_premium_when_disabled() {
        let mut config = default_config();
        config.exceptional_min_score = 40;

        let input = swing_input(dec!(0.005), StopConfidence::High, 4);
        let result = classify_stop_quality(&input, &config, false);
        assert_eq!(result.class, StopQuality::Premium);
        assert!(result.shadow_exceptional, "should be shadow-exceptional");
        assert_eq!(result.boost_pct, dec!(0.15));
    }

    #[test]
    fn allows_exceptional_when_enabled() {
        let mut config = default_config();
        config.exceptional_min_score = 40;

        let input = swing_input(dec!(0.005), StopConfidence::High, 4);
        let result = classify_stop_quality(&input, &config, true);
        assert_eq!(result.class, StopQuality::Exceptional);
        assert_eq!(result.boost_pct, dec!(0.20));
        assert!(!result.shadow_exceptional);
    }

    #[test]
    fn thresholds_default_are_conservative() {
        let config = default_config();
        assert_eq!(config.noise_max_pct, dec!(0.002));
        assert_eq!(config.efficient_max_pct, dec!(0.008));
        assert_eq!(config.moderate_max_pct, dec!(0.015));
        assert_eq!(config.distant_min_pct, dec!(0.025));
        assert_eq!(config.weak_min_score, 10);
        assert_eq!(config.good_min_score, 25);
        assert_eq!(config.premium_min_score, 40);
        assert_eq!(config.exceptional_min_score, 60);
        assert!(!config.noise_max_pct.is_negative());
        assert!(config.weak_min_score < config.good_min_score);
        assert!(config.good_min_score < config.premium_min_score);
        assert!(config.premium_min_score < config.exceptional_min_score);
    }

    #[test]
    fn classification_uses_decimal_boost_pct() {
        let input = swing_input(dec!(0.005), StopConfidence::High, 3);
        let result = classify_stop_quality(&input, &default_config(), false);
        assert!(result.boost_pct >= Decimal::ZERO);
        assert!(result.boost_pct <= dec!(0.20));
    }

    #[test]
    fn classification_roundtrip_serializable() {
        let input = swing_input(dec!(0.005), StopConfidence::High, 3);
        let result = classify_stop_quality(&input, &default_config(), false);

        let json = serde_json::to_string(&result).expect("serialization failed");
        let deserialized: StopQualityClassification =
            serde_json::from_str(&json).expect("deserialization failed");

        assert_eq!(result.class, deserialized.class);
        assert_eq!(result.raw_score, deserialized.raw_score);
        assert_eq!(result.boost_pct, deserialized.boost_pct);
        assert_eq!(result.shadow_exceptional, deserialized.shadow_exceptional);
        assert_eq!(result.reasons, deserialized.reasons);
    }
}
