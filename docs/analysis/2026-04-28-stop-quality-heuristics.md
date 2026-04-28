# StopQuality Heuristics Specification

**Date**: 2026-04-28
**Status**: PROPOSED (Phase 2 — Spec Only)
**Related**: [ADR-0024 — Stop-Aware Entry Policy (v4)](../adr/ADR-0024-stop-aware-entry-policy.md)

---

## Purpose

Define initial rule-based heuristics for `StopQuality` classification. This is a
specification document only — no runtime behavior changes.

---

## Compatibility Statement

This specification does NOT alter:
- v3 live positions (remain read-only and authoritative)
- `TechnicalStopDistance` calculation (remains unchanged)
- EntryPolicy behavior (StopQuality is additive only)
- Risk Engine authority (remains final authority)
- Production slots (remain occupied by v3 positions)

`StopQualityBoost` is:
- Additive only (never subtractive)
- Capped at +15% in production (+20% feature-flagged)
- Not a rejection when absent
- INPUT to Risk Engine, NOT authorization

---

## 1. Inputs Required

### Core Inputs

| Input | Type | Description |
|-------|------|-------------|
| `stop_anchor_valid` | bool | Whether the StopAnchor event is valid |
| `technical_stop_distance` | Price | Absolute distance from entry to stop |
| `anchor_type` | enum | support, resistance, swing_low, swing_high, breakout_retest, liquidity_level |
| `anchor_price` | Price | Price level of the anchor |
| `timeframe` | Timeframe | Timeframe of the anchor (15m in v3) |
| `entry_price` | Price | Candidate entry price |

### Derived Inputs

| Input | Type | Description |
|-------|------|-------------|
| `distance_pct` | f64 | `technical_stop_distance / entry_price` as percentage |
| `distance_atr` | f64 | `technical_stop_distance / ATR(14)` in ATR units |
| `anchor_freshness` | Duration | Time since anchor was last confirmed/touched |
| `confluence_count` | u32 | Number of confluences at anchor region |
| `volume_confirmation` | bool | Whether volume spike confirms the anchor |
| `candle_structure_confirmation` | bool | Whether candle pattern confirms the anchor |
| `liquidity_sweep_or_retest` | bool | Whether liquidity sweep/retest occurred |
| `expected_rr_after_stop_distance` | f64 | Expected risk/reward ratio given the stop distance |

---

## 2. StopQuality Classification

### Scale

| Class | Boost | Production Cap |
|-------|-------|----------------|
| None | 0% | ✅ |
| Weak | +5% | ✅ |
| Good | +10% | ✅ |
| Premium | +15% | ✅ (initial cap) |
| Exceptional | +20% | ❌ (feature-flagged, shadow-mode only) |

### Rules

1. **Production cap**: +15% initially (Premium class max)
2. **Exceptional +20%**: disabled by default, shadow-mode only
3. **No boost is not rejection**: absence of boost does NOT create rejection
4. **Boost does NOT authorize**: `boosted_score` is INPUT to Risk Engine only
5. **Risk Engine always wins**: may reject even with maximum boost

---

## 3. Initial Heuristics

### None (0%)
- StopAnchor valid, but no relevant confluence
- Distance technically acceptable, but no clear advantage
- No candle/volume confirmation
- Default fallback when no other criteria met

### Weak (+5%)
- Anchor valid but distant (>2.0 ATR)
- Anchor not recent (freshness > 48 candles on 15m)
- Low confluence (0-1 factors)
- No additional confirmations

### Good (+10%)
- Anchor recent (freshness ≤ 24 candles on 15m)
- Distance moderate (0.5–1.5 ATR)
- Clear 15m structure at anchor
- RR ≥ 1.5
- At least 1 additional confirmation (candle OR volume)

### Premium (+15%)
- Anchor recent AND clean (freshness ≤ 12 candles, well-defined)
- Distance efficient (0.3–1.0 ATR)
- Well-defined support/resistance OR swing level
- Candle OR volume confirms the region
- RR ≥ 2.0
- Multiple confluences (≥2 factors)

### Exceptional (+20% — feature-flagged)
- Rare case
- Anchor very clear (freshness ≤ 6 candles, sharp structure)
- Distance short but NOT inside noise (>0.2 ATR, <0.5 ATR)
- Sweep/retest/liquidity confirmation present
- RR ≥ 3.0
- Strong confluence (≥3 factors)
- **Shadow-mode only initially**

---

## 4. Pseudo-code

```rust
/// Configuration thresholds (all configurable)
struct StopQualityThresholds {
    // Distance thresholds (ATR)
    noise_max_atr: f64,           // default: 0.2
    short_max_atr: f64,            // default: 0.5
    moderate_max_atr: f64,         // default: 1.0
    distant_min_atr: f64,          // default: 2.0

    // Freshness thresholds (15m candles)
    very_recent_max: u32,          // default: 6
    recent_max: u32,               // default: 12
    acceptable_max: u32,           // default: 24

    // Confluence
    premium_min_confluence: u32,   // default: 2
    exceptional_min_confluence: u32, // default: 3

    // Risk/Reward
    good_min_rr: f64,              // default: 1.5
    premium_min_rr: f64,           // default: 2.0
    exceptional_min_rr: f64,       // default: 3.0

    // Score thresholds
    weak_min_score: i32,           // default: 10
    good_min_score: i32,           // default: 25
    premium_min_score: i32,        // default: 40
    exceptional_min_score: i32,    // default: 60
}

/// Classification result
struct StopQualityClassification {
    class: StopQuality,            // None, Weak, Good, Premium, Exceptional
    score: i32,                    // Raw score before thresholds
    boost_pct: f64,                // 0.0, 0.05, 0.10, 0.15, or 0.20
    shadow_exceptional: bool,      // Whether score would be Exceptional if enabled
    reasons: Vec<String>,          // Human-readable factors
}

fn classify_stop_quality(
    input: &StopQualityInput,
    config: &StopQualityThresholds,
    exceptional_enabled: bool,     // Feature flag
) -> Result<StopQualityClassification, ClassificationError> {
    // 1. Validate StopAnchor
    if !input.stop_anchor_valid {
        return Err(ClassificationError::InvalidAnchor);
    }

    // 2. Noise filter — if stop is too close, it's not a quality issue
    if input.distance_atr <= config.noise_max_atr {
        return Ok(StopQualityClassification {
            class: StopQuality::None,
            score: 0,
            boost_pct: 0.0,
            shadow_exceptional: false,
            reasons: vec!["Stop inside noise".to_string()],
        });
    }

    // 3. Calculate score
    let mut score = 0i32;
    let mut reasons = Vec::new();

    // Freshness points (0-15)
    score += freshness_points(input.anchor_freshness, config);
    // Distance ATR points (0-20)
    score += distance_atr_points(input.distance_atr, config);
    // Confluence points (0-15)
    score += confluence_points(input.confluence_count, config);
    // Candle confirmation (0-10)
    if input.candle_structure_confirmation {
        score += 10;
        reasons.push("Candle confirmation".to_string());
    }
    // Volume confirmation (0-10)
    if input.volume_confirmation {
        score += 10;
        reasons.push("Volume confirmation".to_string());
    }
    // Liquidity sweep/retest (0-15)
    if input.liquidity_sweep_or_retest {
        score += 15;
        reasons.push("Liquidity sweep/retest".to_string());
    }
    // Risk/Reward points (0-15)
    score += rr_points(input.expected_rr_after_stop_distance, config);

    // 4. Classify by thresholds
    let (class, boost_pct, shadow_exceptional) = if score >= config.exceptional_min_score {
        if exceptional_enabled {
            (StopQuality::Exceptional, 0.20, false)
        } else {
            (StopQuality::Premium, 0.15, true)
        }
    } else if score >= config.premium_min_score {
        (StopQuality::Premium, 0.15, false)
    } else if score >= config.good_min_score {
        (StopQuality::Good, 0.10, false)
    } else if score >= config.weak_min_score {
        (StopQuality::Weak, 0.05, false)
    } else {
        (StopQuality::None, 0.0, false)
    };

    Ok(StopQualityClassification {
        class,
        score,
        boost_pct,
        shadow_exceptional,
        reasons,
    })
}

// Helper functions (simplified)
fn freshness_points(freshness: Duration, config: &StopQualityThresholds) -> i32 {
    let candles = freshness.as_15m_candles();
    if candles <= config.very_recent_max { 15 }
    else if candles <= config.recent_max { 10 }
    else if candles <= config.acceptable_max { 5 }
    else { 0 }
}

fn distance_atr_points(distance_atr: f64, config: &StopQualityThresholds) -> i32 {
    if distance_atr <= config.short_max_atr { 20 }
    else if distance_atr <= config.moderate_max_atr { 15 }
    else if distance_atr < config.distant_min_atr { 5 }
    else { 0 }
}

fn confluence_points(count: u32, config: &StopQualityThresholds) -> i32 {
    if count >= config.exceptional_min_confluence { 15 }
    else if count >= config.premium_min_confluence { 10 }
    else if count >= 1 { 5 }
    else { 0 }
}

fn rr_points(rr: f64, config: &StopQualityThresholds) -> i32 {
    if rr >= config.exceptional_min_rr { 15 }
    else if rr >= config.premium_min_rr { 10 }
    else if rr >= config.good_min_rr { 5 }
    else { 0 }
}
```

---

## 5. Configurability

All thresholds MUST be configurable, not hardcoded. Default values are starting
points for calibration.

### Configuration Source

Suggested locations (to be determined in implementation):
- Runtime config file (TOML/YAML)
- Feature flags for production caps
- Database-backed config for ATR-based thresholds

### Default Thresholds

| Parameter | Default | Rationale |
|-----------|---------|-----------|
| `noise_max_atr` | 0.2 | Stops inside 0.2 ATR are too close (noise) |
| `short_max_atr` | 0.5 | Efficient stop: <0.5 ATR |
| `moderate_max_atr` | 1.0 | Acceptable stop: <1.0 ATR |
| `distant_min_atr` | 2.0 | Distant stop: ≥2.0 ATR |
| `very_recent_max` | 6 candles | 1.5 hours on 15m |
| `recent_max` | 12 candles | 3 hours on 15m |
| `acceptable_max` | 24 candles | 6 hours on 15m |
| `weak_min_score` | 10 | Minimal for Weak |
| `good_min_score` | 25 | Minimal for Good |
| `premium_min_score` | 40 | Minimal for Premium |
| `exceptional_min_score` | 60 | Minimal for Exceptional |
| `good_min_rr` | 1.5 | Minimal RR for Good |
| `premium_min_rr` | 2.0 | Minimal RR for Premium |
| `exceptional_min_rr` | 3.0 | Minimal RR for Exceptional |

---

## 6. Required Telemetry

For every candidate evaluated, log:

```rust
struct StopQualityTelemetry {
    // Input state
    base_score: f64,
    distance_pct: f64,
    distance_atr: f64,
    anchor_type: AnchorType,
    anchor_freshness_candles: u32,
    confluence_count: u32,

    // Classification result
    stop_quality_class: StopQuality,
    raw_score: i32,
    boost_pct: f64,
    boosted_score_hypothetical: f64,
    production_cap_applied: Option<f64>,  // e.g., 0.15 when shadow_exceptional=true
    exceptional_shadow_score: Option<f64>, // Score if exceptional were enabled

    // Decision outcome
    final_decision: Decision,  // Accepted, Rejected, Shadow
    rejection_reason: Option<String>,

    // Delta for analysis
    decision_delta: Option<f64>,  // boosted_score - base_score if decision changed
}
```

### Telemetry Output

All boosted decisions MUST log:
- `base_score`
- `stop_quality_class`
- `boost_pct`
- `boosted_score`
- `decision_delta` (if shadow mode reveals a different decision)

---

## 7. Risks and False Positives

### Known Risks

| Risk | Mitigation |
|------|------------|
| Subjective thresholds | All configurable; telemetry for calibration |
| Over-weighting freshness | Freshness caps at 15 points (25% of exceptional threshold) |
| ATR volatility skew | Use ATR(14) smoothed; log ATR value for analysis |
| Anchors in fast markets | Freshness reset on significant structure break |
| False "exceptional" in low volatility | Feature-flagged; shadow-mode only initially |

### False Positive Scenarios

1. **Fresh but weak anchor**: Recent anchor but poor structure
   - Mitigation: require confluence count ≥2 for Premium/Exceptional

2. **Short stop in low volatility**: ATR shrinks, distance appears efficient
   - Mitigation: absolute distance_pct floor (e.g., >0.3%)

3. **Liquidity sweep without follow-through**: Sweep occurs but price continues
   - Mitigation: require candle confirmation for Premium/Exceptional

---

## 8. Classification Examples

### Example 1: None
```
Anchor: support, 48 candles old
Distance: 2.2 ATR
Confluence: 0
Confirmation: none
RR: 1.2
→ Score: 0 (fresh) + 0 (distance) + 0 (confluence) + 0 + 0 + 0 + 0 (RR) = 0
→ Class: None (0%)
```

### Example 2: Good
```
Anchor: resistance, 18 candles old
Distance: 0.8 ATR
Confluence: 1 (VWAP)
Confirmation: volume yes, candle no
RR: 1.8
→ Score: 5 (fresh) + 15 (distance) + 5 (confluence) + 0 + 10 (vol) + 0 + 5 (RR) = 40
→ Class: Premium (+15%)
```

### Example 3: Premium (shadow-Exceptional)
```
Anchor: swing_low, 4 candles old
Distance: 0.4 ATR
Confluence: 2 (support + trendline)
Confirmation: candle yes, volume yes, sweep yes
RR: 2.8
→ Score: 15 (fresh) + 20 (distance) + 10 (confluence) + 10 + 10 + 15 + 10 (RR) = 80
→ Class: Premium (+15%) with shadow_exceptional=true
→ If exceptional_enabled: Exceptional (+20%)
```

---

## 9. Next Steps (Phase 3+)

This spec is Phase 2 (documentation only). Subsequent phases:

1. **Phase 3**: Shadow metadata implementation — emit `StopAnchor` and `StopQuality`,
   no decision change
2. **Phase 4**: Telemetry pipeline — aggregate scores and deltas
3. **Phase 5**: Boost cap +15% — apply boost up to Premium
4. **Phase 6**: Exceptional evaluation — +20% in shadow mode
5. **Phase 7**: Feature-flagged +20% — enable if evidence supports

Each phase requires separate ADR/update before implementation.

---

## 10. References

- [ADR-0024 — Stop-Aware Entry Policy (v4)](../adr/ADR-0024-stop-aware-entry-policy.md)
- [ADR-0021 — Opportunity Detection vs Technical Stop Analysis](../adr/ADR-0021-opportunity-detection-vs-technical-stop-analysis.md)
- [docs/architecture/v3-runtime-spec.md](../architecture/v3-runtime-spec.md)
