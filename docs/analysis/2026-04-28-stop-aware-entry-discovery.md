# Stop-Aware Entry Pre-Implementation Discovery

**Date**: 2026-04-28
**Status**: COMPLETE (Phase 3 — Discovery Only)
**Related**: [ADR-0035 — Stop-Aware Entry Policy (v4)](../adr/ADR-0035-stop-aware-entry-policy.md)
**Related**: [StopQuality Heuristics Spec](2026-04-28-stop-quality-heuristics.md)

---

## Purpose

Pre-implementation discovery required by ADR-0035 Implementation Notes before
any code changes. This report maps the current v3 architecture and identifies
integration points for StopAnchor, StopQuality, and telemetry.

**Scope**: Read-only analysis. No runtime changes.

---

## Commands Used

```bash
# Find TechnicalStopDistance calculation
grep -r -i "technicalstop\|technical_stop" v3/ --include="*.rs"

# Find EntryScore existence
grep -r -i "entry.*score\|entryscore" v3/ --include="*.rs"

# Find EntryPolicy and signal strategy
grep -r -i "entry.*policy\|signal.*strategy" v3/ --include="*.rs"

# Find Risk Engine
grep -r -i "risk.*gate\|RiskGate" v3/ --include="*.rs"

# Find slots and position management
grep -r -i "slot\|position.*manager" v3/ --include="*.rs"

# Find DetectorSignal creation
grep -r "DetectorSignal::new\|with_technical_stop_analysis" v3/ --include="*.rs"

# List all Rust files
find v3/ -name "*.rs" -type f

# Count total Rust files
find v3/ -name "*.rs" -type f | wc -l  # Output: 84
```

---

## 1. TechnicalStopDistance Location

**File**: `v3/robson-engine/src/technical_stop_analyzer.rs` (680 lines)

**Function**: `TechnicalStopAnalyzer::analyze()`

**Signature**:
```rust
pub fn analyze(
    candles: &[Candle],
    entry_price: Price,
    side: Side,
    config: &TechnicalStopConfig,
) -> Result<TechnicalStopAnalysis, TechnicalStopError>
```

**Output**: `TechnicalStopAnalysis`
```rust
pub struct TechnicalStopAnalysis {
    pub stop_price: Price,              // Chart-derived stop price (absolute)
    pub method: TechnicalStopMethod,     // SwingPoint { level_n } or AtrFallback
    pub confidence: StopConfidence,      // High, Medium, Low
    pub detected_levels: Vec<Price>,     // All swing levels detected (audit trail)
}
```

**Algorithm** (priority order):
1. **Swing points** (primary): Nth support/resistance level (default: 2nd)
2. **ATR fallback**: `entry ± atr_multiplier × ATR(14)` when no swing levels found

**Key Insight**: Already outputs method/confidence/levels suitable for StopAnchor metadata.

**Configuration**: `TechnicalStopConfig` with defaults:
- `min_candles: 100`
- `swing_lookback: 2`
- `support_level_n: 2`
- `atr_period: 14`
- `atr_multiplier: 1.5`
- `min_stop_distance_pct: 0.001` (0.1%)
- `max_stop_distance_pct: 0.10` (10%)

---

## 2. EntryScore Existence

**Result**: NOT FOUND in v3

**Analysis**:
- No `EntryScore` or equivalent base_score exists
- `SignalDecision` returns `{ side, reason, observed_at, reference_price }`
- No scoring mechanism currently exists
- Signal is binary: NoSignal or SignalConfirmed

**Conclusion**: v4 `EntryCandidateEvaluation` will be a NEW layer, not an evolution
of existing v3 code.

---

## 3. EntryPolicy Signal Generation

**File**: `v3/robson-engine/src/signal_strategy.rs` (300+ lines)

**EntryPolicy Enum**:
```rust
pub enum EntryPolicy {
    Immediate,           // No strategy
    ConfirmedTrend,      // SmaCrossoverStrategy
    ConfirmedReversal,   // ReversalPatternStrategy
    ConfirmedKeyLevel,   // KeyLevelStrategy
}
```

**Strategy Registry**:
```rust
pub trait SignalStrategy: Send + Sync {
    fn evaluate(&self, ctx: SignalContext) -> SignalDecision;
}

pub struct StrategyRegistry {
    pub strategies: HashMap<StrategyId, Box<dyn SignalStrategy>>,
}
```

**Signal Output**: `SignalDecision`
```rust
pub enum SignalDecision {
    NoSignal,
    SignalConfirmed {
        side: Side,
        reason: SignalReason,      // Immediate, SmaCrossover, ReversalPattern, KeyLevelReaction
        observed_at: DateTime<Utc>,
        reference_price: Decimal,  // Used for entry_price
    },
}
```

**Key Insight**: Strategies return binary decisions, not scores. StopQuality boost
will be a NEW layer applied AFTER signal confirmation.

---

## 4. Risk Engine Authorization

**File**: `v3/robson-engine/src/risk.rs` (400+ lines)

**RiskGate**:
```rust
pub struct RiskGate {
    policy: TradingPolicy,
}

impl RiskGate {
    pub fn evaluate(
        &self,
        request: RiskCheckRequest,
        context: &RiskContext,
    ) -> RiskEvaluation { ... }
}
```

**RiskEvaluation Output**:
```rust
pub struct RiskEvaluation {
    pub approved: bool,
    pub rejection_reason: Option<RiskRejectionReason>,
    pub slots_available: u32,
}
```

**Checks Performed** (ADR-0024):
1. Duplicate position (same symbol+side)
2. Dynamic slot exhaustion (replaces static max_open_positions)
3. Monthly drawdown hard limit
4. Daily loss limit

**Key Insight**: Pure computation (no I/O). StopQuality boost must NOT contaminate
this purity. `boosted_score` is INPUT, not authorization.

---

## 5. Slots Enforcement

**File**: `v3/robson-domain/src/policy.rs`

**TradingPolicy**:
```rust
pub struct TradingPolicy {
    pub risk_per_trade_pct: Decimal,        // Fixed at 1%
    pub max_monthly_drawdown_pct: Decimal,  // Fixed at 4%
}

impl TradingPolicy {
    pub fn slots_available(
        &self,
        capital_base: Decimal,
        realized_loss: Decimal,
        latent_risk: Decimal,
    ) -> u32 {
        let monthly_budget = self.monthly_budget(capital_base);
        let risk_per_trade = self.risk_per_trade_amount(capital_base);
        floor((monthly_budget - realized_loss - latent_risk) / risk_per_trade)
    }
}
```

**Key Insight**: Dynamic slot calculation per ADR-0024. NOT static `max_open_positions`.
Slots consume losing trades only; wins do NOT offset (confirmed in tests).

---

## 6. v3 Live Positions State

**Storage**:
- `v3/robson-domain/src/entities.rs`: `Position` entity
- `v3/robson-store/src/repository.rs`: Repository pattern
- `v3/robson-projector/`: Read projections

**Position Entity** (excerpt):
```rust
pub struct Position {
    pub id: PositionId,
    pub symbol: Symbol,
    pub side: Side,
    pub state: PositionState,  // Armed, Entered, Exited, etc.
    pub entry_order_id: Option<OrderId>,
    pub exit_order_id: Option<OrderId>,
    // ... other fields
}
```

**Key Insight**: v3 live positions are read-only authoritative state per ADR-0022.
v4 must NOT revalidate, cancel, or reclassify existing positions.

---

## 7. StopAnchor Implicit → Explicit Opportunity

**Current State**: `TechnicalStopAnalysis` already captures:
- `stop_price`: Absolute price level
- `method`: SwingPoint { level_n } or AtrFallback
- `confidence`: High, Medium, Low
- `detected_levels`: All swing levels (audit trail)

**Missing for StopAnchor** (per ADR-0035):
- `anchor_type`: Explicit enum (support, resistance, swing_low, swing_high, breakout_retest, liquidity_level)
- `anchor_price`: Explicit field (currently implicit in `stop_price`)
- `timeframe`: Explicit field (hardcoded 15m in TechnicalStopAnalyzer)
- `source_event_id`: Reference to technical event
- `invalidation_reason`: For anchor invalidation tracking

**Integration Point**: `build_technical_stop_audit()` in `v3/robsond/src/detector.rs` (line 542)

**Current Audit**: `TechnicalStopAnalysisAudit`
```rust
pub struct TechnicalStopAnalysisAudit {
    pub stop_price: Price,
    pub method: TechnicalStopMethodSnapshot,
    pub confidence: TechnicalStopConfidenceSnapshot,
    pub detected_levels: Vec<Price>,
    pub config: TechnicalStopConfigSnapshot,
}
```

**Proposed Extension**: Add StopAnchor fields to this struct.

---

## 8. StopQuality Shadow Mode Integration

**Best Integration Point**: `DetectorTask::create_signal()` AFTER `compute_technical_stop()`

**Current Flow**:
```text
SignalDecision::SignalConfirmed
    ↓
compute_technical_stop() → TechnicalStopAnalysis
    ↓
DetectorSignal::new() with with_technical_stop_audit()
    ↓
EventBus::publish(DaemonEvent::DetectorSignal)
```

**Proposed Insertion Point** (shadow mode only):
```text
SignalDecision::SignalConfirmed
    ↓
compute_technical_stop() → TechnicalStopAnalysis
    ↓
[NEW] classify_stop_quality() → StopQualityClassification
    ↓
DetectorSignal::new() with stop_quality_metadata
    ↓
EventBus::publish(DaemonEvent::DetectorSignal)
```

**Key Constraint**: Shadow mode MUST NOT affect execution decisions. Classification
runs but does NOT filter or boost.

---

## 9. Telemetry Emission Point

**Current**: `DetectorSignal` emitted with `technical_stop_analysis` audit payload.

**Proposed Addition**: Add fields to `DetectorSignal`:
```rust
pub struct StopQualityTelemetry {
    pub stop_quality_class: StopQuality,
    pub raw_score: i32,
    pub boost_pct: f64,
    pub boosted_score_hypothetical: f64,  // If applied
    pub production_cap_applied: Option<f64>,  // e.g., 0.15 when shadow_exceptional=true
    pub exceptional_shadow_score: Option<f64>,
    pub decision_delta: Option<f64>,  // If shadow mode reveals different decision
}
```

**Integration**: `EventBus.publish(DaemonEvent::DetectorSignal { telemetry, ... })`

**Logging**: Use `tracing::info!` to log all telemetry fields for each candidate.

---

## 10. Regression Risks

| Risk Area | Current Behavior | Protection Strategy |
|-----------|------------------|---------------------|
| DetectorTask single-shot | Emits exactly ONE signal then exits | StopQuality classifier does NOT alter signal emission |
| RiskGate purity | Pure computation, no I/O | StopQuality boost INPUT only, does NOT contaminate RiskGate |
| v3 live positions | Read-only authoritative (ADR-0022) | v4 code path checks `position.state == Armed` only |
| TechnicalStopDistance | Pure chart analysis | Zero changes to `TechnicalStopAnalyzer::analyze()` |
| Dynamic slot calculation | Per ADR-0024 formula | Zero changes to `TradingPolicy::slots_available()` |
| EventBus schema | DetectorSignal with audit payload | Additive fields only, backward compatible |

---

## 11. Files Analyzed

| File | Lines | Purpose |
|------|-------|---------|
| `v3/robson-engine/src/technical_stop_analyzer.rs` | 680 | TechnicalStopDistance calculation |
| `v3/robson-engine/src/signal_strategy.rs` | 300+ | EntryPolicy → Strategy mapping |
| `v3/robson-engine/src/risk.rs` | 400+ | RiskGate evaluation |
| `v3/robson-domain/src/policy.rs` | 150+ | TradingPolicy, slots_available |
| `v3/robson-domain/src/entities.rs` | 500+ | Position, DetectorSignal entities |
| `v3/robsond/src/detector.rs` | 600+ | DetectorTask, signal creation |
| `v3/robson-store/src/repository.rs` | 200+ | Position persistence |
| `v3/robson-projector/src/types.rs` | 100+ | Event types |

**Total**: 84 Rust files in v3/ (8 core files analyzed in depth)

---

## 12. Phase 3 Implementation Plan (Shadow Metadata)

**Objective**: Emit StopAnchor and StopQuality metadata without decision change.

### Step 1: Extend Domain Types

**File**: `v3/robson-domain/src/entities.rs`

Add new types:
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StopAnchor {
    pub anchor_type: AnchorType,
    pub anchor_price: Price,
    pub timeframe: Timeframe,
    pub source_event_id: Option<Uuid>,
    pub invalidation_reason: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum StopQuality {
    None,
    Weak,
    Good,
    Premium,
    Exceptional,
}
```

Extend `TechnicalStopAnalysisAudit`:
```rust
pub struct TechnicalStopAnalysisAudit {
    pub stop_price: Price,
    pub method: TechnicalStopMethodSnapshot,
    pub confidence: TechnicalStopConfidenceSnapshot,
    pub detected_levels: Vec<Price>,
    pub config: TechnicalStopConfigSnapshot,
    // NEW:
    pub stop_anchor: Option<StopAnchor>,
    pub stop_quality: Option<StopQualityClassification>,
}
```

### Step 2: Implement StopQuality Classifier

**File**: `v3/robson-engine/src/stop_quality_classifier.rs` (NEW)

Pure function (no I/O):
```rust
pub struct StopQualityClassifier;

impl StopQualityClassifier {
    pub fn classify(
        input: &StopQualityInput,
        config: &StopQualityThresholds,
        exceptional_enabled: bool,
    ) -> StopQualityClassification { ... }
}
```

Use heuristics from [StopQuality Heuristics Spec](2026-04-28-stop-quality-heuristics.md).

### Step 3: Integrate in DetectorTask

**File**: `v3/robsond/src/detector.rs`

Modify `create_signal()`:
```rust
// After compute_technical_stop()
let analysis = self.compute_technical_stop(entry_price, side, &self.config.symbol).await?;

// NEW: Build StopAnchor from analysis
let stop_anchor = Some(StopAnchor {
    anchor_type: Self::map_method_to_anchor_type(analysis.method),
    anchor_price: analysis.stop_price,
    timeframe: CandleInterval::FifteenMinutes,
    source_event_id: None,  // Will be populated when event system supports
    invalidation_reason: None,
});

// NEW: Classify StopQuality (shadow mode)
let stop_quality = Some(StopQualityClassifier::classify(
    &StopQualityInput::from_analysis(&analysis, entry_price),
    &StopQualityThresholds::default(),
    false,  // exceptional_enabled = false initially
));

Ok(DetectorSignal::new(...)
    .with_technical_stop_analysis(
        Self::build_technical_stop_audit(&analysis, &self.config.technical_stop_config)
            .with_stop_anchor(stop_anchor)
            .with_stop_quality(stop_quality)
    ))
```

### Step 4: Emit Telemetry

**File**: `v3/robsond/src/detector.rs`

Add logging after signal creation:
```rust
tracing::info!(
    position_id = %position_id,
    signal_id = %signal.signal_id,
    stop_quality_class = ?signal.technical_stop_analysis.as_ref().and_then(|a| a.stop_quality.as_ref()).map(|q| q.class),
    boost_pct = signal.technical_stop_analysis.as_ref().and_then(|a| a.stop_quality.as_ref()).map(|q| q.boost_pct).unwrap_or(0.0),
    "DetectorSignal with StopQuality telemetry"
);
```

### Step 5: Tests

**File**: `v3/robson-engine/src/stop_quality_classifier.rs` (tests module)

Test scenarios:
- None: valid anchor, no confluence
- Weak: distant anchor, low confluence
- Good: recent anchor, moderate distance
- Premium: recent + clean, efficient distance
- Exceptional: rare case (feature-flagged)

---

## 13. Compatibility Verification

| Invariant (ADR-0035) | Status |
|---------------------|--------|
| TechnicalStopDistance unchanged | ✅ Pure function, zero changes |
| StopQuality absence ≠ rejection | ✅ Shadow mode only, no filtering |
| StopQualityBoost additive only | ✅ Metadata only, not applied |
| v3 live positions read-only | ✅ New code path for Armed positions only |
| Risk Engine authority final | ✅ StopQuality INPUT only |
| boosted_score NOT authorization | ✅ Telemetry only |
| Shadow mode no execution impact | ✅ Metadata emission only |

---

## 14. Next Steps

After this discovery is approved:

1. **Create Implementation Guide**: Detailed step-by-step for Phase 3
2. **Add StopQualityThresholds config**: TOML/YAML configuration
3. **Implement StopQualityClassifier**: Rule-based heuristics
4. **Extend domain types**: StopAnchor, StopQualityClassification
5. **Integrate in DetectorTask**: Shadow mode metadata emission
6. **Add telemetry logging**: tracing::info! for all fields
7. **Write tests**: Unit tests for classifier, integration tests for detector
8. **VAL-001 scenario**: Test on testnet, confirm zero decision impact

**No production boost application until Phase 5+ (separate ADR/update required).**

---

## 15. References

- [ADR-0035 — Stop-Aware Entry Policy (v4)](../adr/ADR-0035-stop-aware-entry-policy.md)
- [ADR-0021 — Opportunity Detection vs Technical Stop Analysis](../adr/ADR-0021-opportunity-detection-vs-technical-stop-analysis.md)
- [ADR-0022 — Robson-Authored Position Invariant](../adr/ADR-0022-robson-authored-position-invariant.md)
- [StopQuality Heuristics Spec](2026-04-28-stop-quality-heuristics.md)
- [v3/CLAUDE.md](../../v3/CLAUDE.md) — Robson v3 context
