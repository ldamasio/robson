# Stop-Aware Entry — Slice 001 Execution Prompt

**Session Type**: Guarded Execution with Mandatory Checkpoints
**Slice**: Shadow Metadata Only
**Created**: 2026-04-28
**Status**: READY FOR REVIEW / EXECUTION REQUIRES EXPLICIT AUTHORIZATION

---

## Agent Instructions

You are executing **Slice 001** of the Stop-Aware Entry implementation. This is a
**guarded execution task** with explicit checkpoints. Follow this prompt exactly.

**CRITICAL**: You are executing runtime code changes. Every edit must be intentional,
verified, and tested BEFORE requesting authorization to commit.

**IMPORTANT**: Do NOT commit automatically. Stop after diff, request explicit authorization.

---

## Context Summary

### What Is This?

Robson v4 introduces **Stop-Aware Entry Policy** (ADR-0035):
- `StopAnchor`: explicit metadata about the technical stop event
- `StopQuality`: classification of stop region quality (None → Exceptional)
- `StopQualityBoost`: capped additive boost to entry signal (+0% to +20%)

**Slice 001 Objective**: Validate whether adding domain types is the correct minimal first change. If not, stop and report.

### Why This Matters

- Improves observability of technical stop decisions
- Enables future signal modulation based on stop quality
- Preserves existing v3 behavior (zero regression)
- Foundation for telemetry and shadow mode testing

### Architecture

```
Current v3 flow:
SignalDecision → TechnicalStopAnalysis → DetectorSignal → Risk Engine → Entry

Target v4 flow (after all slices):
SignalDecision → TechnicalStopAnalysis → StopAnchor + StopQuality → DetectorSignal → Risk Engine → Entry

Slice 001 changes (this session):
Add types to domain layer. No flow changes yet.
```

---

## Your Task: Slice 001 — Shadow Metadata Only

### Objective

**FIRST**: Validate current code structure and confirm that adding domain types to
`robson-domain/src/entities.rs` is the correct minimal first change.

**THEN**: If validated, add domain types for StopAnchor and StopQuality WITHOUT altering
any runtime behavior.

**IF NOT VALIDATED**: Stop, report findings, await revised instructions.

### Success Criteria

1. New types compile successfully
2. Existing tests pass (100%)
3. Zero behavioral change verified
4. Clean commit with proper message

### Constraints

- **DO NOT** alter entry decision logic
- **DO NOT** apply boost to any score
- **DO NOT** modify Risk Engine
- **DO NOT** touch detector.rs (that's Slice 002)
- **DO NOT** create migrations
- **DO NOT** modify v3 live positions

---

## Execution Steps (Follow Exactly)

### Step 1: Discovery and Re-Validation (CRITICAL — Before Any Edit)

**FIRST STEP**: Re-validate current code structure. Do NOT assume documents are correct.

**Before any code change:**

```bash
# 1.1: Confirm clean working tree
git status --short
# Expected: empty output

# 1.2: Confirm on correct branch
git branch --show-current
# Expected: main or feature branch

# 1.3: Confirm no stashed changes
git stash list
# Expected: empty or no relevant stashes

# 1.4: Verify current code state
cargo test --all 2>&1 | tail -20
# Expected: tests pass

# 1.5: Check for existing StopAnchor/StopQuality
grep -r "StopAnchor\|StopQuality" robson-domain/src/ --include="*.rs" || echo "NOT FOUND (expected)"
# Expected: NOT FOUND
```

**STOP**: If any check fails, report the issue and STOP.

---

### Step 2: Read and Understand Current Code

```bash
# 2.1: Read entities.rs structure
head -100 robson-domain/src/entities.rs

# 2.2: Find TechnicalStopAnalysisAudit
grep -n "pub struct TechnicalStopAnalysisAudit" robson-domain/src/entities.rs

# 2.3: Read the audit struct definition
sed -n '<found-line>,+20p' robson-domain/src/entities.rs

# 2.4: Check imports at top of file
head -50 robson-domain/src/entities.rs | grep "^use "

# 2.5: Verify no existing uses that would break
grep -r "TechnicalStopAnalysisAudit" . --include="*.rs" | grep -v "test" | head -10
```

**Report**: What you found. Current fields of TechnicalStopAnalysisAudit?

---

### Step 3: Add New Domain Types (After Re-Validation)

**IMPORTANT**: The code below is ILLUSTRATIVE SHAPE ONLY. After re-validating the current
structure of `entities.rs` in Step 2, adapt this code to match the existing patterns,
imports, and conventions. Do NOT copy-paste blindly.

**Edit**: `robson-domain/src/entities.rs`

**Location**: After existing type definitions, before impl blocks (confirm in Step 2).

**Illustrative shape** (adapt to match existing code style):

```rust
// =============================================================================
// Stop-Aware Entry Types (ADR-0035)
// =============================================================================

/// Explicit metadata about the technical stop anchor event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StopAnchor {
    /// Type of anchor (support, resistance, swing point, etc.)
    pub anchor_type: AnchorType,
    /// Price level of the anchor
    pub anchor_price: Price,
    /// Timeframe of the anchor (15m in v3)
    pub timeframe: Timeframe,
    /// Reference to the technical event (optional, future)
    pub source_event_id: Option<Uuid>,
    /// Reason for anchor invalidation (if applicable)
    pub invalidation_reason: Option<String>,
}

/// Classification of anchor types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AnchorType {
    /// Support level (for LONG entries)
    Support,
    /// Resistance level (for SHORT entries)
    Resistance,
    /// Swing low point
    SwingLow,
    /// Swing high point
    SwingHigh,
    /// Breakout retest level
    BreakoutRetest,
    /// Liquidity sweep level
    LiquidityLevel,
}

/// Stop quality classification result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StopQualityClassification {
    /// Quality class (None through Exceptional)
    pub class: StopQuality,
    /// Raw score before thresholds
    pub raw_score: i32,
    /// Boost percentage (0.0 to 0.20)
    pub boost_pct: f64,
    /// Whether this would be Exceptional if flag enabled
    pub shadow_exceptional: bool,
    /// Human-readable reasons for classification
    pub reasons: Vec<String>,
}

/// Stop quality class with associated boost percentage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StopQuality {
    /// No boost (0%) — valid anchor, no structural advantage
    None,
    /// Weak boost (+5%) — distant/old anchor, low confluence
    Weak,
    /// Good boost (+10%) — recent anchor, moderate distance, 1+ confirmation
    Good,
    /// Premium boost (+15%) — recent + clean, efficient distance, multiple confluences
    Premium,
    /// Exceptional boost (+20%) — rare, feature-flagged, shadow-mode only
    Exceptional,
}
```

**After editing, verify**:

```bash
# 3.1: Check formatting
cargo fmt --all

# 3.2: Try to build
cargo build 2>&1 | head -50
```

**Report**: Does it compile? Any errors?

---

### Step 4: Extend TechnicalStopAnalysisAudit

**Edit**: `robson-domain/src/entities.rs`

**Find**: The existing `TechnicalStopAnalysisAudit` struct

**Add fields** at the end of the struct (before closing brace):

```rust
pub struct TechnicalStopAnalysisAudit {
    pub stop_price: Price,
    pub method: TechnicalStopMethodSnapshot,
    pub confidence: TechnicalStopConfidenceSnapshot,
    pub detected_levels: Vec<Price>,
    pub config: TechnicalStopConfigSnapshot,

    // === NEW: Stop-Aware Entry metadata (ADR-0035) ===
    /// Explicit metadata about the stop anchor event
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_anchor: Option<StopAnchor>,

    /// Stop quality classification (shadow mode initially)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_quality: Option<StopQualityClassification>,
}
```

**CRITICAL**: Use `#[serde(default, skip_serializing_if = "Option::is_none")]` for backward compatibility.

**After editing, verify**:

```bash
# 4.1: Format
cargo fmt --all

# 4.2: Build
cargo build 2>&1 | tail -30

# 4.3: Full build
cargo build --release 2>&1 | tail -10
```

**Report**: Build status? Any warnings?

---

### Step 5: Verify Non-Regression

```bash
# 5.1: Format check
cargo fmt --all --check

# 5.2: All tests
cargo test --all 2>&1 | tail -30

# 5.3: Verify no boost application
grep -r "boost.*apply\|boosted_score.*authorization" . --include="*.rs" || echo "OK: no boost application"

# 5.4: Verify no exceptional flag enabled
grep -r "exceptional.*true\|STOP_QUALITY.*ENABLED.*true" . --include="*.rs" || echo "OK: no exceptional flag"

# 5.5: Verify no v3 revalidation
grep -r "revalidate.*v3\|rewrite.*entry.*thesis" . --include="*.rs" || echo "OK: no v3 revalidation"
```

**Expected Results**:
- `cargo fmt --all --check`: passes (no output)
- `cargo test --all`: 100% pass
- All grep checks: return "OK"

**Report**: All checks passed?

---

### Step 6: Review Changes

```bash
# 6.1: Show diff
git diff robson-domain/src/entities.rs

# 6.2: Diff stats
git diff --stat robson-domain/src/entities.rs

# 6.3: Check for unintended changes
git status --short
# Expected: only robson-domain/src/entities.rs modified
```

**Report**: Summary of changes.

---

### Step 7: Request Authorization (DO NOT Commit Automatically)

**CRITICAL**: Do NOT commit. Stop here, show the diff, and request EXPLICIT authorization.

**If and ONLY IF all previous steps passed:**

```bash
git add robson-domain/src/entities.rs

git commit -m "feat(domain): add StopAnchor and StopQuality types (Slice 001)

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

# Optional: add Co-Authored-By only if required by the actual execution agent/session.
# Do NOT include model-specific authorship unless applicable to the execution context.
"
```

**STOP**: Show the diff output from Step 6, report all test results, and REQUEST EXPLICIT
AUTHORIZATION to commit.

**DO NOT execute the above commands until authorization is granted.**

**After authorization is granted**:

```bash
# 7.1: Verify commit
git log --oneline -1

# 7.2: Verify clean tree
git status --short
# Expected: empty
```

---

## Rollback Plan

### If Any Step Fails

1. **DO NOT commit** partial work
2. Reset working tree:
   ```bash
   git restore robson-domain/src/entities.rs
   ```
3. Verify clean:
   ```bash
   git status --short
   ```
4. Report failure with error output
5. Analyze root cause

### If Commit Needs Revert (Post-Merge)

```bash
git revert <commit-hash>
cargo test --all
```

---

## Acceptance Criteria

Slice 001 is COMPLETE when:

- [ ] New types compile without errors
- [ ] `cargo build --release` succeeds
- [ ] `cargo fmt --all --check` passes
- [ ] `cargo test --all` passes (100%)
- [ ] No grep warnings (boost application, exceptional flag, v3 revalidation)
- [ ] Only `robson-domain/src/entities.rs` modified
- [ ] Clean commit with conventional commit message
- [ ] Working tree clean after commit

---

## Next Session (Slice 002)

After Slice 001 is complete, **next session will implement**:
- StopQualityClassifier (pure function)
- Integration in detector.rs
- Telemetry emission

**NOT in this session.**

---

## Emergency Contacts

If stuck or unsure:
1. STOP
2. Report current state
3. Show error/output
4. Wait for guidance

**When in doubt: STOP. Ask. Then proceed.**

---

## Session Start Command

To start this session in a fresh Claude instance:

```bash
# Navigate to repo
cd /home/psyctl/apps/robson

# Load this prompt
cat docs/analysis/2026-04-28-stop-aware-entry-slice-001-execution-prompt.md

# Or paste the entire prompt into Claude
```

**Remember**: This is a self-contained execution prompt. All context is included.
Follow the steps exactly. Verify at each checkpoint.

---

**End of Execution Prompt**
