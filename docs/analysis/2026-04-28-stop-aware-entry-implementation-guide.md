# Stop-Aware Entry Implementation Guide

**Date**: 2026-04-28
**Status**: PROPOSED (Phase 3 Runtime — Shadow Metadata Only)
**Related**:
- [ADR-0035 — Stop-Aware Entry Policy (v4)](../adr/ADR-0035-stop-aware-entry-policy.md)
- [StopQuality Heuristics Spec](2026-04-28-stop-quality-heuristics.md)
- [Pre-Implementation Discovery](2026-04-28-stop-aware-entry-discovery.md)

---

## Purpose

Transform ADR-0035 + heuristics spec + discovery report into a safe incremental plan
for implementing shadow metadata (StopAnchor + StopQuality telemetry) without
behavioral change.

**Scope**: Implementation guide only. No runtime changes in this document.

---

## 1. Base Documents Summary

### ADR-0035 — Stop-Aware Entry Policy (v4)

**Status**: PROPOSED, committed as `02d41278`

**Key Decisions**:
- StopAnchor: explicit metadata (not new calculation)
- StopQuality: capped additive boost (+0% to +20%, production cap +15%)
- TechnicalStopDistance: preserved unchanged
- No boost is not rejection
- v3 live positions: read-only, authoritative
- boosted_score: INPUT to Risk Engine, NOT authorization
- Risk Engine: final authority

**11 Non-Negotiable Invariants**:
1. TechnicalStopDistance calculation unchanged
2. StopQuality absence ≠ rejection
3. StopQualityBoost additive only
4. v3 positions never revalidated
5. StopQualityBoost cannot rescue invalid signal
6. RiskEngine rejection always wins
7. boosted_score is NOT authorization
8. Shadow mode does not affect execution
9. Exceptional +20% disabled by default
10. All boosted decisions must log telemetry
11. StopAnchor invalidation on open positions does NOT rewrite entry thesis

**Rollout Phases**: 0 (ADR) → 1 (Shadow metadata) → 2 (Telemetry) → 3 (Boost +15%) → 4 (Exceptional shadow) → 5 (Feature-flagged +20%)

### StopQuality Heuristics Spec

**Status**: PROPOSED, committed as `c2e6a0ef`

**Classification Scale**:
- None (0%): Valid anchor, no structural advantage
- Weak (+5%): Distant/old anchor, low confluence
- Good (+10%): Recent anchor, moderate distance, 1+ confirmation
- Premium (+15%): Recent + clean, efficient distance, multiple confluences
- Exceptional (+20%): Rare, feature-flagged, shadow-mode only

**Inputs Required** (11 total):
- Core: stop_anchor_valid, technical_stop_distance, anchor_type, anchor_price, timeframe, entry_price
- Derived: distance_pct, distance_atr, anchor_freshness, confluence_count, volume_confirmation, candle_structure_confirmation, liquidity_sweep_or_retest, expected_rr_after_stop_distance

**Pseudo-code**: Rule-based scoring system with configurable thresholds.

**Required Telemetry**: base_score, stop_quality_class, boost_pct, boosted_score_hypothetical, production_cap_applied, exceptional_shadow_score, decision_delta.

### Pre-Implementation Discovery

**Status**: COMPLETE, committed as `ac3dd86e`

**Key Findings**:
- TechnicalStopDistance: `v3/robson-engine/src/technical_stop_analyzer.rs` (680 lines)
- EntryScore: NOT FOUND in v3 → v4 will create NEW layer
- EntryPolicy: `v3/robson-engine/src/signal_strategy.rs` with StrategyRegistry
- Risk Engine: `v3/robson-engine/src/risk.rs` with RiskGate (pure computation)
- Slots: `v3/robson-domain/src/policy.rs` with dynamic `slots_available()`
- v3 live positions: entities.rs + repository.rs + projector (read-only)

**Integration Points**:
- StopAnchor: extend `build_technical_stop_audit()` in `detector.rs` (line 542)
- StopQuality: classify after `compute_technical_stop()` in `DetectorTask::create_signal()`
- Telemetry: `EventBus.publish(DetectorSignal)` with additive fields

---

## 2. Objective of First Real Implementation

**Phase 3 Runtime**: Shadow Metadata Only

**Goal**: Emit StopAnchor and StopQuality metadata WITHOUT behavioral change.

**Constraints**:
- Do NOT alter entry decision
- Do NOT alter position sizing
- Do NOT alter Risk Engine behavior
- Do NOT apply boost (yet)
- Do NOT reject candidates due to lack of boost
- Do NOT revalidate v3 live positions
- Do NOT touch occupied production slots

**Success Criteria**:
- DetectorSignal includes StopAnchor metadata (derived from existing TechnicalStopAnalysis)
- DetectorSignal includes StopQuality classification (calculated but NOT applied)
- Telemetry logged for all candidates
- Zero change in entry decisions
- Zero change in position sizing
- Zero change in Risk Engine rejections/approvals

---

## 3. Suggested Incremental Order

### Step 1: Add Domain Types (Metadata Only)

**File**: `v3/robson-domain/src/entities.rs`

**Action**: Add new types for StopAnchor and StopQuality.

**Types to Add**:
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StopAnchor {
    pub anchor_type: AnchorType,
    pub anchor_price: Price,
    pub timeframe: Timeframe,
    pub source_event_id: Option<Uuid>,
    pub invalidation_reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AnchorType {
    Support,
    Resistance,
    SwingLow,
    SwingHigh,
    BreakoutRetest,
    LiquidityLevel,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StopQualityClassification {
    pub class: StopQuality,
    pub raw_score: i32,
    pub boost_pct: f64,
    pub shadow_exceptional: bool,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StopQuality {
    None,
    Weak,
    Good,
    Premium,
    Exceptional,
}
```

**Risk**: Low. Pure data types, no behavior change.

**Verification**: `cargo build` succeeds.

---

### Step 2: Extend TechnicalStopAnalysisAudit

**File**: `v3/robson-domain/src/entities.rs`

**Action**: Add optional StopAnchor and StopQuality fields to existing audit struct.

**Change**:
```rust
pub struct TechnicalStopAnalysisAudit {
    pub stop_price: Price,
    pub method: TechnicalStopMethodSnapshot,
    pub confidence: TechnicalStopConfidenceSnapshot,
    pub detected_levels: Vec<Price>,
    pub config: TechnicalStopConfigSnapshot,
    // NEW (optional for backward compatibility):
    pub stop_anchor: Option<StopAnchor>,
    pub stop_quality: Option<StopQualityClassification>,
}
```

**Risk**: Low. Additive optional fields, backward compatible.

**Verification**: `cargo build` succeeds, existing tests pass.

---

### Step 3: Implement StopQuality Classifier (Pure Function)

**File**: `v3/robson-engine/src/stop_quality_classifier.rs` (NEW)

**Action**: Implement rule-based classifier per heuristics spec.

**Signature**:
```rust
pub struct StopQualityClassifier;

impl StopQualityClassifier {
    pub fn classify(
        input: &StopQualityInput,
        config: &StopQualityThresholds,
        exceptional_enabled: bool,  // MUST be false in Phase 3
    ) -> StopQualityClassification { ... }
}
```

**Key Points**:
- Pure function (no I/O)
- Uses heuristics from spec
- All thresholds configurable
- `exceptional_enabled = false` for Phase 3

**Risk**: Low. Isolated pure function, not called yet.

**Verification**: Unit tests for each classification (None, Weak, Good, Premium, Exceptional).

---

### Step 4: Build StopAnchor from TechnicalStopAnalysis

**File**: `v3/robsond/src/detector.rs`

**Action**: Extend `build_technical_stop_audit()` to populate StopAnchor.

**Change**: Add helper function and call in `build_technical_stop_audit()`.

```rust
impl DetectorTask {
    fn build_stop_anchor(
        analysis: &TechnicalStopAnalysis,
        side: Side,
    ) -> StopAnchor {
        let anchor_type = match (analysis.method, side) {
            (TechnicalStopMethod::SwingPoint { .. }, Side::Long) => AnchorType::SwingLow,
            (TechnicalStopMethod::SwingPoint { .. }, Side::Short) => AnchorType::SwingHigh,
            (TechnicalStopMethod::AtrFallback, _) => {
                // Infer from context or mark as LiquidityLevel
                AnchorType::LiquidityLevel
            },
        };

        StopAnchor {
            anchor_type,
            anchor_price: analysis.stop_price,
            timeframe: CandleInterval::FifteenMinutes,
            source_event_id: None,  // TODO: populate when event system supports
            invalidation_reason: None,
        }
    }

    fn build_technical_stop_audit(
        analysis: &TechnicalStopAnalysis,
        config: &TechnicalStopConfig,
    ) -> TechnicalStopAnalysisAudit {
        TechnicalStopAnalysisAudit {
            stop_price: analysis.stop_price,
            method: Self::map_technical_stop_method(analysis.method),
            confidence: Self::map_technical_stop_confidence(analysis.confidence),
            detected_levels: analysis.detected_levels.clone(),
            config: TechnicalStopConfigSnapshot { ... },
            // NEW:
            stop_anchor: Some(Self::build_stop_anchor(analysis, side)),  // side needs to be passed in
            stop_quality: None,  // Will be populated in Step 5
        }
    }
}
```

**Risk**: Low. Metadata only, does not affect signal creation logic.

**Verification**: `cargo build` succeeds, existing detector tests pass.

---

### Step 5: Classify StopQuality in Shadow Mode

**File**: `v3/robsond/src/detector.rs`

**Action**: Calculate StopQuality after `compute_technical_stop()` but DO NOT apply it.

**Change**: Modify `create_signal()` method.

```rust
impl DetectorTask {
    async fn create_signal(
        &self,
        decision: SignalDecision,
    ) -> DaemonResult<DetectorSignal> {
        // ... existing validation ...

        let entry_price = Price::new(reference_price)?;

        // EXISTING: compute technical stop
        let analysis = self.compute_technical_stop(entry_price, side, &self.config.symbol).await?;

        // NEW: classify StopQuality (shadow mode only)
        let stop_quality_input = StopQualityInput::from_analysis(&analysis, entry_price);
        let stop_quality = Some(StopQualityClassifier::classify(
            &stop_quality_input,
            &StopQualityThresholds::default(),
            false,  // exceptional_enabled = FALSE in Phase 3
        ));

        // EXISTING: build signal
        Ok(DetectorSignal::new(...)
            .with_technical_stop_analysis(
                Self::build_technical_stop_audit(&analysis, &self.config.technical_stop_config, side)
                    .with_stop_quality(stop_quality)  // NEW: attach classification
            ))
    }
}
```

**Risk**: Medium. Classification runs but is NOT used for decision making. Must ensure no code path uses `stop_quality` to filter or reject.

**Verification**:
- Unit test: classification produces valid output
- Integration test: signal creation still works
- Shadow test: logged telemetry visible, decision unchanged

---

### Step 6: Emit Telemetry Without Decision Change

**File**: `v3/robsond/src/detector.rs`

**Action**: Add tracing::info! log after signal creation.

**Change**:
```rust
let signal = DetectorSignal::new(...)
    .with_technical_stop_analysis(...)?;

// NEW: telemetry logging
if let Some(audit) = &signal.technical_stop_analysis {
    if let Some(sq) = &audit.stop_quality {
        tracing::info!(
            position_id = %self.config.position_id,
            signal_id = %signal.signal_id,
            stop_quality_class = ?sq.class,
            raw_score = sq.raw_score,
            boost_pct = sq.boost_pct,
            shadow_exceptional = sq.shadow_exceptional,
            reasons = ?sq.reasons,
            "DetectorSignal with StopQuality telemetry (shadow mode, no decision change)"
        );
    }
}

Ok(signal)
```

**Risk**: Low. Logging only, does not affect logic.

**Verification**: Run detector, observe logs, confirm decision unchanged.

---

### Step 7: Add Tests for Zero Behavioral Change

**File**: `v3/robsond/tests/detector_stop_quality_test.rs` (NEW) or inline in `detector.rs`

**Test Cases**:

```rust
#[cfg(test)]
mod stop_quality_shadow_tests {
    use super::*;

    #[tokio::test]
    async fn stop_quality_classification_does_not_alter_signal_creation() {
        // Arrange: create detector with known market data
        // Act: generate signal
        // Assert: signal created successfully with stop_quality metadata
        // Assert: entry_price, stop_loss unchanged from baseline
    }

    #[tokio::test]
    async fn stop_quality_none_does_not_reject_valid_signal() {
        // Arrange: market data that produces StopQuality::None
        // Act: generate signal
        // Assert: signal is created (not rejected)
    }

    #[tokio::test]
    async fn exceptional_quality_is_capped_at_premium_when_flag_disabled() {
        // Arrange: market data that would score Exceptional
        // Act: classify with exceptional_enabled = false
        // Assert: class is Premium, shadow_exceptional = true
    }

    #[tokio::test]
    async fn stop_quality_metadata_is_serializable() {
        // Arrange: signal with stop_quality
        // Act: serialize to JSON
        // Assert: valid JSON with all fields
    }
}
```

**Risk**: Low. Tests only, no production code.

**Verification**: `cargo test` passes, including new tests.

---

### Step 8: Verification Commands

**Before Commit**:
```bash
# Format
cargo fmt --all

# Lint
cargo clippy --all-targets -- -D warnings

# Unit tests
cargo test --all

# Check for accidental exceptional flag usage
grep -r "exceptional_enabled.*=.*true" v3/ --include="*.rs" || echo "OK: no exceptional=true found"

# Verify no boost application
grep -r "boosted_score.*apply\|boost.*authorization" v3/ --include="*.rs" || echo "OK: no boost application found"
```

**After Merge** (before production):
```bash
# Integration test on testnet
# Follow VAL-001 scenario from ADR-0022
```

---

## 4. Probable Files (To Be Revalidated)

**Based on discovery, agent MUST revalidate before editing**:

| File | Purpose | Change Type |
|------|---------|-------------|
| `v3/robson-domain/src/entities.rs` | Add StopAnchor, StopQuality types | Additive |
| `v3/robson-engine/src/stop_quality_classifier.rs` | NEW: classifier implementation | New file |
| `v3/robsond/src/detector.rs` | Build StopAnchor, classify StopQuality, emit telemetry | Additive |
| `v3/robson-engine/src/lib.rs` | Export StopQualityClassifier (if new file) | Additive |
| `v3/robsond/src/lib.rs` | Export StopQualityClassifier (if used) | Additive |

**Files NOT to be changed in Phase 3**:
- `v3/robson-engine/src/technical_stop_analyzer.rs` — leave untouched
- `v3/robson-engine/src/signal_strategy.rs` — leave untouched
- `v3/robson-engine/src/risk.rs` — leave untouched
- `v3/robson-domain/src/policy.rs` — leave untouched
- `v3/robson-store/src/*` — leave untouched
- `v3/robson-projector/src/*` — leave untouched

---

## 5. Mandatory Tests

### Unit Tests

**StopQualityClassifier** (`v3/robson-engine/src/stop_quality_classifier.rs`):
- `classify_returns_none_for_valid_anchor_with_no_confluence`
- `classify_returns_weak_for_distant_anchor`
- `classify_returns_good_for_recent_anchor_with_moderate_distance`
- `classify_returns_premium_for_recent_clean_anchor_with_efficient_distance`
- `classify_returns_exceptional_only_when_flag_enabled`
- `classify_caps_at_premium_when_exceptional_flag_disabled`
- `classify_respects_configurable_thresholds`

**DetectorTask Integration** (`v3/robsond/src/detector.rs`):
- `create_signal_with_stop_quality_metadata_succeeds`
- `create_signal_stop_quality_none_does_not_reject`
- `build_stop_anchor_maps_swing_low_correctly`
- `build_stop_anchor_maps_swing_high_correctly`
- `build_stop_anchor_handles_atr_fallback`

### Regression Tests

**TechnicalStopDistance**:
- `analyze_returns_same_stop_price_with_metadata_extension`
- `analyze_confidence_unchanged_when_stop_anchor_added`

**Risk Engine**:
- `risk_gate_evaluate_unchanged_when_stop_quality_present`
- `slots_available_calculation_unchanged`

**v3 Live Positions**:
- `existing_positions_not_revalidated_when_stop_quality_emitted`
- `occupied_slots_remain_unchanged`

### Shadow Mode Tests

**Zero Behavioral Change**:
- `signal_with_premium_quality_has_same_entry_price_as_baseline`
- `signal_with_none_quality_has_same_entry_price_as_baseline`
- `signal_creation_success_rate_unchanged_with_stop_quality`

---

## 6. Non-Regression Verification

### Pre-Commit Commands

```bash
# Format check
cargo fmt --all --check

# Lint
cargo clippy --all-targets -- -D warnings

# All tests
cargo test --all

# Feature flag check (must be disabled)
! grep -r "STOP_QUALITY_EXCEPTIONAL_ENABLED.*true" v3/ --include="*.rs" || echo "ERROR: exceptional flag enabled!"

# Boost application check (must not exist)
! grep -r "boost.*apply\|boosted_score.*authorization" v3/ --include="*.rs" || echo "ERROR: boost application found!"

# v3 revalidation check (must not exist)
! grep -r "revalidate.*v3\|rewrite.*entry.*thesis" v3/ --include="*.rs" || echo "ERROR: v3 revalidation found!"
```

### Post-Merge Verification (Testnet)

```bash
# VAL-001 scenario: arm position, observe detector, confirm signal creation
# Check logs for StopQuality telemetry
# Confirm entry decisions unchanged from baseline
# Confirm no positions rejected due to lack of boost
```

---

## 7. Rollback Plan

**Commit Strategy**: One small commit per step.

**If Step Fails**:
1. Identify which step caused failure
2. Revert that specific commit: `git revert <commit-hash>`
3. Fix issue, re-apply with new commit

**Full Rollback** (if needed):
1. Revert entire Phase 3: `git revert <phase-3-commit-range>`
2. Verify: `cargo test --all`
3. No migrations to undo (Phase 3 is metadata-only)

**No Destructive Changes**:
- No database migrations in Phase 3
- No breaking schema changes
- Additive fields only (optional)
- Backward compatible

---

## 8. Acceptance Criteria

Phase 3 runtime implementation is accepted ONLY if:

1. **Build**: `cargo build --release` succeeds with zero warnings
2. **Format**: `cargo fmt --all --check` passes
3. **Lint**: `cargo clippy --all-targets -- -D warnings` passes
4. **Tests**: `cargo test --all` passes (100% pass rate)
5. **No Regression**:
   - TechnicalStopDistance calculation unchanged (verified by tests)
   - Entry decisions unchanged (verified by shadow mode comparison)
   - Position sizing unchanged (verified by tests)
   - Risk Engine behavior unchanged (verified by tests)
   - v3 live positions not touched (verified by code review)
   - Occupied slots unchanged (verified by tests)
6. **Shadow Mode Verified**:
   - StopQuality telemetry visible in logs
   - StopAnchor metadata populated in DetectorSignal
   - Zero decision change attributable to StopQuality
   - `exceptional_enabled = false` enforced (no +20% in production)
7. **Documentation**:
   - All new types have doc comments
   - Implementation notes updated
   - VAL-001 scenario documented

---

## 9. Post-Phase 3 (Future Work)

**NOT in scope for this implementation guide**:

- Phase 4: Telemetry pipeline (aggregate scores/deltas)
- Phase 5: Apply boost up to Premium (+15%)
- Phase 6: Exceptional evaluation (+20% shadow mode)
- Phase 7: Feature-flagged +20% production

**Each phase requires**:
- Separate ADR update
- Separate Implementation Guide
- Separate commits
- Separate testing
- Separate rollback plan

---

## 10. References

- [ADR-0035 — Stop-Aware Entry Policy (v4)](../adr/ADR-0035-stop-aware-entry-policy.md)
- [StopQuality Heuristics Spec](2026-04-28-stop-quality-heuristics.md)
- [Pre-Implementation Discovery](2026-04-28-stop-aware-entry-discovery.md)
- [ADR-0022 — Robson-Authored Position Invariant](../adr/ADR-0022-robson-authored-position-invariant.md)
- [v3/CLAUDE.md](../../v3/CLAUDE.md) — Robson v3 context

---

**Remember**: Small, incremental, safe changes. Always validate. Always test.
Zero behavioral change in Phase 3. Metadata and telemetry only.
