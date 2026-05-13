# Runtime Slice 001 — Shadow Metadata Only (Operational Checklist)

**Date**: 2026-04-28
**Status**: PROPOSED (Not executable yet — requires authorization)
**Slice**: Shadow Metadata Only
**Related**:
- [ADR-0035](../adr/ADR-0035-stop-aware-entry-policy.md)
- [Heuristics Spec](2026-04-28-stop-quality-heuristics.md)
- [Discovery Report](2026-04-28-stop-aware-entry-discovery.md)
- [Implementation Guide](2026-04-28-stop-aware-entry-implementation-guide.md)

---

## Purpose

Operational checklist for the FIRST runtime slice of Stop-Aware Entry implementation.
This slice prepares types/metadata for StopAnchor/StopQuality WITHOUT altering entry
decisions, sizing, Risk Engine, or TechnicalStopDistance.

**Scope**: Checklist document only. No runtime changes in this step.

---

## 1. Pre-Flight (Before Any Code Change)

### Environment Check
- [ ] Confirm current branch: `git branch --show-current`
- [ ] Confirm working tree clean: `git status --short` (should be empty)
- [ ] Confirm no uncommitted changes: `git diff --stat`
- [ ] Confirm no stashed changes: `git stash list`

### Document Re-Read (Mandatory)
- [ ] Re-read ADR-0035 (11 invariants, rollout phases)
- [ ] Re-read Heuristics Spec (classification scale, inputs, pseudo-code)
- [ ] Re-read Discovery Report (integration points, files analyzed)
- [ ] Re-read Implementation Guide (Step 1-8, verification commands)

### Code Re-Validation (Before Edit)
- [ ] Locate `robson-domain/src/entities.rs` — read current structure
- [ ] Locate `TechnicalStopAnalysis` struct — confirm current fields
- [ ] Locate `TechnicalStopAnalysisAudit` struct — confirm current fields
- [ ] Locate `DetectorSignal` struct — confirm current structure
- [ ] Search for existing `EntryScore` or similar — confirm still NOT FOUND
- [ ] Search for existing StopAnchor/StopQuality types — confirm NOT EXISTS
- [ ] Locate `build_technical_stop_audit()` in detector.rs — read current implementation
- [ ] Locate `create_signal()` in detector.rs — read current flow

### Hypothesis Validation
- [ ] **HYPOTHESIS**: `entities.rs` is the correct file for new types
  - Action: Re-read file, confirm imports, confirm module structure
  - If hypothesis wrong: STOP, update checklist, re-authorize

- [ ] **HYPOTHESIS**: `TechnicalStopAnalysisAudit` can accept optional fields
  - Action: Confirm struct is not `#[non_exhaustive]` breaking change
  - If hypothesis wrong: STOP, update checklist, re-authorize

- [ ] **HYPOTHESIS**: No existing code depends on exact `TechnicalStopAnalysisAudit` shape
  - Action: `grep -r "TechnicalStopAnalysisAudit {" . --include="*.rs"`
  - If matches found in consumers: STOP, assess backward compat

---

## 2. Slice 001 Objective

**Shadow Metadata Only**

This slice MUST:
- Add domain types for StopAnchor and StopQuality (entities.rs)
- Extend TechnicalStopAnalysisAudit with optional metadata fields
- Result: Types compile, existing tests pass, ZERO behavioral change

This slice MUST NOT:
- Alter entry decision logic
- Alter position sizing
- Alter Risk Engine behavior
- Apply boost to any score
- Reject candidates due to missing StopQuality
- Modify v3 live positions
- Modify slots enforcement
- Create database migrations
- Create feature flags
- Modify detector.rs flow (yet — that's Slice 002)

---

## 3. Candidate Files (As Hypothesis, Not Final)

**HYPOTHESIS — To be revalidated BEFORE first edit:**

| File | Purpose | Change Type | Confidence |
|------|---------|-------------|------------|
| `robson-domain/src/entities.rs` | Add StopAnchor, StopQuality, AnchorType enums | Additive types | Medium (revalidation required) |
| `robson-domain/src/entities.rs` | Extend TechnicalStopAnalysisAudit | Additive optional fields | Medium (revalidation required) |

**Files NOT to touch in Slice 001:**
- `robson-engine/src/technical_stop_analyzer.rs` — NO CHANGE
- `robson-engine/src/signal_strategy.rs` — NO CHANGE
- `robson-engine/src/risk.rs` — NO CHANGE
- `robson-domain/src/policy.rs` — NO CHANGE
- `robsond/src/detector.rs` — NO CHANGE (Slice 002)
- `robson-store/src/*` — NO CHANGE
- `robson-projector/src/*` — NO CHANGE

**Revalidation Action Required:**
Before editing ANY file, run:
```bash
# Confirm file exists and is readable
ls -la robson-domain/src/entities.rs

# Read current structure
head -100 robson-domain/src/entities.rs

# Search for existing uses
grep -r "TechnicalStopAnalysisAudit" . --include="*.rs"
```

If findings contradict hypothesis: **STOP**, update checklist, request re-authorization.

---

## 4. Non-Regression Criteria

Slice 001 is accepted ONLY if:

- [ ] **TechnicalStopDistance**: Calculation unchanged (verified by existing tests)
- [ ] **No boost applied**: Zero code applies boost to entry decision
- [ ] **No rejection by missing StopQuality**: Candidates not rejected due to lack of metadata
- [ ] **boosted_score not used as authorization**: No code path uses boosted_score for final decision
- [ ] **Risk Engine unchanged**: Zero modifications to risk.rs
- [ ] **v3 live positions read-only**: Zero code touches existing positions
- [ ] **Slots unchanged**: Zero modifications to slots_available() logic
- [ ] **No migration**: Zero database schema changes
- [ ] **No production behavior change**: Entry decisions identical to baseline

---

## 5. Expected Tests

### Pre-Commit (Required)
```bash
# Format
cargo fmt --all
cargo fmt --all --check  # CI-friendly verification

# Build
cargo build
cargo build --release  # Must succeed

# Lint (if repo uses clippy)
cargo clippy --all-targets -- -D warnings

# All tests
cargo test --all

# Verify no boost application
grep -r "boost.*apply\|boosted_score.*authorization" . --include="*.rs" || echo "OK"

# Verify no exceptional flag enabled
grep -r "STOP_QUALITY_EXCEPTIONAL.*true\|exceptional_enabled.*=.*true" . --include="*.rs" || echo "OK"

# Verify no v3 revalidation
grep -r "revalidate.*v3\|rewrite.*entry.*thesis" . --include="*.rs" || echo "OK"
```

### Expected Test Results
- [ ] `cargo fmt --all --check`: passes (no formatting changes needed)
- [ ] `cargo build`: succeeds with zero warnings
- [ ] `cargo clippy`: succeeds with zero warnings (if applicable)
- [ ] `cargo test --all`: 100% pass rate
- [ ] All grep checks: return "OK" (no prohibited patterns found)

### Specific Test Cases (After Implementation)
- [ ] New types compile: `StopAnchor`, `AnchorType`, `StopQuality`, `StopQualityClassification`
- [ ] `TechnicalStopAnalysisAudit` with new optional fields compiles
- [ ] Existing `DetectorSignal` creation still compiles
- [ ] Existing detector tests still pass (no behavioral change)

---

## 6. Edit Sequence (Proposed, Requires Authorization)

### Step 1: Read Before Edit
```bash
# Read entities.rs structure
head -200 robson-domain/src/entities.rs

# Find TechnicalStopAnalysisAudit location
grep -n "pub struct TechnicalStopAnalysisAudit" robson-domain/src/entities.rs

# Read surrounding context
sed -n '1,50p' robson-domain/src/entities.rs  # Check imports
```

**STOP HERE.** Show findings, request authorization to proceed.

### Step 2: Add New Types (Entities Only)
```rust
// Add to robson-domain/src/entities.rs
// Location: After existing type definitions, before impl blocks

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

**STOP HERE.** Run `cargo build`, show output, request authorization.

### Step 3: Extend TechnicalStopAnalysisAudit
```rust
// Find existing TechnicalStopAnalysisAudit struct
// Add optional fields at the end

pub struct TechnicalStopAnalysisAudit {
    pub stop_price: Price,
    pub method: TechnicalStopMethodSnapshot,
    pub confidence: TechnicalStopConfidenceSnapshot,
    pub detected_levels: Vec<Price>,
    pub config: TechnicalStopConfigSnapshot,
    // NEW (optional for backward compatibility):
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_anchor: Option<StopAnchor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_quality: Option<StopQualityClassification>,
}
```

**STOP HERE.** Run full test suite, show output, request authorization.

### Step 4: Verification (Post-Edit)
```bash
# Format check
cargo fmt --all --check

# Build
cargo build --release

# Tests
cargo test --all

# Grep checks (from Section 5)
```

**STOP HERE.** Show all outputs, request authorization for commit.

---

## 7. Mandatory Stop Points

### Stop Point 1: After Pre-Flight
**Before any code edit**, confirm:
- [ ] All pre-flight checks passed
- [ ] All documents re-read
- [ ] All candidate files re-validated
- [ ] All hypotheses confirmed or corrected

**Request authorization to proceed to Step 1 (Read Before Edit).**

### Stop Point 2: After Reading Files
**Before adding code**, confirm:
- [ ] File structure understood
- [ ] No unexpected dependencies found
- [ ] No existing conflicting types
- [ ] Insertion points identified

**Request authorization to proceed to Step 2 (Add New Types).**

### Stop Point 3: After Adding Types
**Before extending audit struct**, confirm:
- [ ] `cargo build` succeeds
- [ ] New types compile without errors
- [ ] No unexpected warnings

**Request authorization to proceed to Step 3 (Extend Audit Struct).**

### Stop Point 4: After All Edits
**Before commit**, confirm:
- [ ] All tests pass (100%)
- [ ] All grep checks pass
- [ ] `cargo fmt --all --check` passes
- [ ] `cargo clippy` passes (if applicable)

**Show diff, request authorization for commit.**

---

## 8. Rollback Plan

### If Step Fails
1. Identify which step failed
2. Do NOT commit partial work
3. Reset working tree: `git restore robson-domain/src/entities.rs`
4. Verify clean: `git status --short`
5. Analyze failure, update checklist, re-authorize

### If Commit Causes Issues (Post-Merge)
1. Revert commit: `git revert <commit-hash>`
2. Verify: `cargo test --all`
3. Report issue, analyze root cause

### No Migrations
- Slice 001 has ZERO database migrations
- Rollback is `git revert` only
- No schema cleanup needed

---

## 9. Commit Criteria (If Authorized)

Commit message (if authorized):
```
feat(domain): add StopAnchor and StopQuality types (Slice 001)

Shadow metadata only — no behavioral change.

Adds domain types for Stop-Aware Entry policy (ADR-0035):
- StopAnchor: explicit metadata for technical stop anchor event
- AnchorType: enum for anchor types (Support, Resistance, SwingLow, etc.)
- StopQuality: classification enum (None, Weak, Good, Premium, Exceptional)
- StopQualityClassification: metadata struct with score, boost, reasons

Extends TechnicalStopAnalysisAudit with optional fields:
- stop_anchor: Option<StopAnchor>
- stop_quality: Option<StopQualityClassification>

No behavioral changes:
- TechnicalStopDistance calculation unchanged
- No boost applied
- No candidate rejected by missing metadata
- Risk Engine unchanged
- v3 live positions untouched

All tests pass. Shadow metadata preparation only.

Related: ADR-0035, Implementation Guide Step 1-3

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
```

---

## 10. Post-Slice 001 (Future Work, Not This Slice)

Slice 002 (NOT THIS SLICE):
- Implement StopQualityClassifier
- Integrate in detector.rs
- Emit telemetry

Slice 003+ (NOT THIS SLICE):
- Apply boost
- Feature flags
- Production changes

---

## 11. Authorization Record

| Step | Date | Authorized By | Notes |
|------|------|---------------|-------|
| Pre-flight | | | |
| Step 1 (Read) | | | |
| Step 2 (Add Types) | | | |
| Step 3 (Extend Audit) | | | |
| Step 4 (Verification) | | | |
| Commit | | | |

---

## 12. Execution Log (To Be Filled During Execution)

| Timestamp | Action | Result | Next Step |
|-----------|--------|--------|-----------|
| | Pre-flight checks | | |
| | Re-read documents | | |
| | Re-validate code | | |
| | Step 1: Read files | | |
| | Step 2: Add types | | |
| | Step 3: Extend audit | | |
| | Step 4: Verify | | |
| | Request commit auth | | |

---

**Remember**: This is a CHECKLIST, not execution. Each step requires explicit authorization
before proceeding. Zero runtime changes until authorized.

**Rule**: When in doubt, STOP. Ask. Re-validate. Then proceed.
