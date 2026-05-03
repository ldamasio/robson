# Stop-Aware Entry — Shadow Validation Runbook

**Severity**: Low (observational, no runtime change)
**Time to Execute**: 10–15 min (local), ongoing (testnet/prod shadow)
**Required Access**: repo checkout, optionally `kubectl` for testnet/prod log access

---

## Run Log

| Date | Executor | Result | Notes |
|------|----------|--------|-------|
| | | **PENDING** | Runbook created after Slice 004; no operational run yet |
| 2026-05-01 | Operator session | **PARTIAL PASS** | Binance Futures Testnet secret validated/reloaded for `robson-testnet`; `robsond` restarted successfully; `-2015 Invalid API-key` absent from latest checked logs. Shadow telemetry not observed yet because the testnet image is still `ghcr.io/rbxrobotica/robson-v2:sha-7c3af2b9` and `RUST_LOG` was not changed. |

*Update this table after every validation run.*

---

## Current Operational Status

- Binance Futures Testnet secret was validated/reloaded for `robson-testnet`.
- `robsond` rollout restart completed successfully.
- New pod came up healthy.
- `-2015 Invalid API-key` did not appear in the latest checked logs.
- Binance WebSocket connected and first tick was received.
- No image update was performed.
- `RUST_LOG` was not changed.
- Production namespace was not touched.
- No code was changed.

The secret rotation/reload unblocked exchange connectivity for testnet, but it did
not validate stop-aware shadow telemetry. The testnet deployment still runs image
`ghcr.io/rbxrobotica/robson-v2:sha-7c3af2b9`, which predates the stop-aware shadow
commits listed below.

---

## Remaining Gates Before Shadow Observation

1. Build/push/deploy image containing commits through at least `020a0dcd`.
2. Confirm testnet rollout with the new image.
3. Temporarily set `RUST_LOG=robsond::detector=debug`.
4. Observe `stop-aware entry shadow telemetry`.
5. Confirm:
   - no boost applied
   - no RiskEngine change
   - no decision change
   - `Exceptional` does not appear
   - SwingPoint can emit StopAnchor
   - AtrFallback does not emit StopAnchor
6. Observe for a few days before any Slice 006 planning.

---

## Explicit Non-Goals

- Do not apply boost.
- Do not change RiskEngine.
- Do not change TechnicalStopDistance.
- Do not change DetectorSignal/EventBus.
- Do not modify slots or live v3 positions.
- Do not plan Slice 006 before shadow observation.

---

## Purpose

Validate that the Stop-Aware Entry shadow metadata (Slices 001–004) is functional,
observable, and does not interfere with v3 execution decisions.

**This runbook does NOT change runtime behavior.** It documents how to observe and
audit shadow telemetry that is already emitted by the detector.

**Blocking prerequisite for**: Slice 006 and any future boost application.

---

## 1. Current Implementation State

Four runtime commits implement shadow-only stop-aware metadata:

| Commit | Description | Scope |
|--------|-------------|-------|
| `500449fe` | feat(domain): add shadow stop-aware entry metadata | `robson-domain/src/entities.rs`, construction sites |
| `31384069` | feat(engine): add pure stop quality classifier | `robson-engine/src/stop_quality_classifier.rs` (new) |
| `41e21a71` | feat(detector): populate stop-aware metadata in shadow mode | `robsond/src/detector.rs` |
| `020a0dcd` | feat(detector): log stop-aware shadow telemetry | `robsond/src/detector.rs` |

Six documentation commits precede the runtime work:

`02d41278`, `c2e6a0ef`, `ac3dd86e`, `8b0fa1c2`, `9414da65`, `cd77d318`

**Total tests**: ~17 stop-aware tests across detector and classifier.

**What runs**: When a `DetectorTask` emits a `DetectorSignal`, it now also classifies
stop quality and logs shadow telemetry — but does NOT use the result for any decision.

---

## 2. Safety Invariants

These invariants are **enforced by the current code** and must remain true:

| # | Invariant | Status |
|---|-----------|--------|
| 1 | `TechnicalStopDistance` calculation unchanged | Enforced (zero changes to `technical_stop_analyzer.rs`) |
| 2 | `StopAnchor` is metadata, not a substitute for `TechnicalStopDistance` | Enforced (used only in audit) |
| 3 | `StopQuality` is additive, capped, shadow-only | Enforced (`exceptional_enabled=false`) |
| 4 | Absence of `StopQuality` boost never causes rejection | Enforced (no code path checks boost for filtering) |
| 5 | `boosted_score` is not authorization | Enforced (not consumed anywhere) |
| 6 | `RiskEngine` is final authority | Enforced (zero changes to `risk.rs`) |
| 7 | v3 live positions are read-only | Enforced (zero changes to position manager logic) |
| 8 | Occupied slots remain occupied | Enforced (zero changes to `policy.rs`) |
| 9 | `DetectorSignal`/`EventBus` schema unchanged | Enforced (additive optional fields only) |
| 10 | Shadow mode does not affect execution decisions | Enforced (classification runs after signal construction) |
| 11 | Telemetry is observability, not control | Enforced (`debug!` logging only, no metrics, no feedback) |

---

## 3. Local Validation

```bash
cd /home/psyctl/apps/robson/v3

# Build
cargo build
# Expected: success

# All tests
cargo test --all
# Expected: 488 passed, 0 failed

# Grep for prohibited patterns
rg -n "apply.*boost|boosted_score.*authori|exceptional_enabled.*=.*true|revalidate.*v3|rewrite.*entry.*thesis" . --type rust
# Expected: no matches (exit code 1)
```

**Note on clippy**: The repo has pre-existing clippy debt (missing docs, config warnings).
This is NOT a blocker for shadow validation. Only flag new warnings in stop-aware files:

```bash
cargo clippy --all-targets -- -D warnings 2>&1 | grep -E "stop_quality|stop_anchor|detector.rs"
# Expected: no matches for stop-aware files
```

---

## 4. Enabling Shadow Telemetry

**Critical**: Shadow telemetry uses `debug!` level, but `robsond` defaults to `info`.

In `v3/robsond/src/main.rs`, the tracing subscriber is configured as:

```rust
tracing_subscriber::registry()
    .with(fmt::layer())
    .with(EnvFilter::from_default_env().add_directive("robsond=info".parse()?))
    .init();
```

This means the shadow telemetry line `"stop-aware entry shadow telemetry"` **will NOT
appear** under default `RUST_LOG` settings.

To see shadow telemetry, set:

```bash
# Option A: all robsond debug output
RUST_LOG=robsond=debug

# Option B: target only the detector module (less noise)
RUST_LOG=robsond::detector=debug

# Option C: for kubectl/k8s, set in deployment env or use --env
kubectl set env deploy/robsond RUST_LOG=robsond::detector=debug -n <namespace>
```

**Warning**: `robsond=debug` produces significant output. Prefer `robsond::detector=debug`
for targeted shadow observation.

---

## 5. Locating Shadow Logs

### Local / Stdout

If running `robsond` locally with `RUST_LOG=robsond=debug` (or `robsond::detector=debug` for less noise):

```bash
# After a detector signal fires:
grep "stop-aware entry shadow telemetry" <logfile-or-stdout>
```

### Kubernetes (Testnet/Prod)

```bash
# Stream logs with debug level
kubectl logs -n <namespace> deploy/robsond -f | grep "stop-aware entry shadow telemetry"
```

**Important**: The log only appears when a `DetectorTask` fires and creates a signal
(`create_signal` path). It does NOT appear on ticks without signals.

---

## 6. Expected Telemetry Fields

When visible, each shadow telemetry line contains these fields:

| Field | Type | Description |
|-------|------|-------------|
| `position_id` | UUID | Position being evaluated |
| `symbol` | string | Trading pair (e.g. `BTCUSDT`) |
| `side` | enum | `Long` or `Short` |
| `stop_anchor_present` | bool | Whether a structural anchor was found |
| `anchor_type` | enum or null | `SwingLow`, `SwingHigh`, or absent for ATR fallback |
| `stop_quality_class` | enum | `None`, `Weak`, `Good`, `Premium`, or `Exceptional` |
| `raw_score` | i32 | Numeric score before threshold mapping |
| `boost_pct` | Decimal | Associated boost percentage (0.00–0.20) |
| `shadow_exceptional` | bool | Whether score would be Exceptional if flag enabled |
| `technical_stop_method` | enum | `SwingPoint { level_n }` or `AtrFallback` |
| `technical_stop_confidence` | enum | `High`, `Medium`, or `Low` |
| `detected_levels_count` | usize | Number of swing levels detected |

**Example log line** (reconstructed for illustration):

```
DEBUG robsond::detector: stop-aware entry shadow telemetry
    position_id=019db2e1-...
    symbol=BTCUSDT side=Long
    stop_anchor_present=true anchor_type=SwingLow
    stop_quality_class=Good raw_score=30 boost_pct=0.10
    shadow_exceptional=false
    technical_stop_method=SwingPoint { level_n: 2 }
    technical_stop_confidence=High
    detected_levels_count=3
```

---

## 7. Prohibited / Sensitive Fields

The following fields must **never** appear in shadow telemetry logs:

| Prohibited Field | Why |
|------------------|-----|
| API keys / secrets | Security |
| Account balance / capital | Financial sensitivity |
| `entry_price` | Not needed for quality audit |
| `stop_price` / `anchor_price` | Recoverable from audit if needed; avoids logging prices |
| `reasons` Vec | Contains detail unnecessary for shadow observation |
| Full DetectorSignal payload | Excessive logging, potential financial data |
| Any PII or account identifier | Security |

**Verification**: The telemetry code in `v3/robsond/src/detector.rs:526-539` explicitly
avoids logging these fields. If a future grep reveals any of them in the telemetry line,
it must be treated as a security regression.

---

## 8. Interpreting `stop_quality_class`

| Class | Boost | Meaning | Expected Frequency |
|-------|-------|---------|-------------------|
| `None` | 0% | Valid anchor but no structural advantage | Common for ATR fallback, distant anchors |
| `Weak` | +5% | Valid but distant or low confidence | Occasional |
| `Good` | +10% | Recent anchor, moderate distance, some confirmation | Typical for SwingPoint stops |
| `Premium` | +15% | Recent + clean anchor, efficient distance, multiple levels | Less common |
| `Exceptional` | +20% | Rare: strong confluence, very clean structure | **Must NOT appear** |

**Critical rule**: With `exceptional_enabled=false` (the current setting), `Exceptional`
must never appear as `stop_quality_class`. If the raw score qualifies as Exceptional,
the output should be `Premium` with `shadow_exceptional=true` instead.

---

## 9. Verifying Boost Was Not Applied

```bash
# From repo root
rg -n "apply.*boost|boosted_score.*authori|exceptional_enabled.*=.*true|revalidate.*v3|rewrite.*entry.*thesis" . --type rust
# Expected: no matches
```

If this returns matches, stop and investigate before proceeding.

Additionally, verify that the classifier is called with `false` for `exceptional_enabled`:

```bash
rg -n "classify_stop_quality" v3/robsond/src/detector.rs
# Should show: classify_stop_quality(..., false)
```

The third argument (`false`) is `exceptional_enabled`. It must be `false`.

---

## 10. Verifying RiskEngine Unchanged

```bash
# Confirm risk.rs has no stop_quality references
rg -n "stop_quality|StopQuality|stop_anchor|StopAnchor" v3/robson-engine/src/risk.rs
# Expected: no matches

# Confirm the RiskGate signature is unchanged
rg -n "pub fn evaluate" v3/robson-engine/src/risk.rs
# Should show the same pure evaluation function
```

---

## 11. Verifying v3 Live Positions Untouched

```bash
# Position manager only has construction sites with None
rg -n "stop_anchor|stop_quality" v3/robsond/src/position_manager.rs
# Expected: only "stop_anchor: None" and "stop_quality: None" in construction sites

# No revalidation logic
rg -n "revalidate|recheck|reassess" v3/robsond/src/position_manager.rs
# Expected: no matches
```

The stop-aware feature acts exclusively in the `DetectorTask::create_signal()` path.
It does not touch `PositionManager`, projections, risk evaluation, or existing
position state.

---

## 12. Criteria Before Slice 006

Before any future slice that uses `StopQuality` for decision-making, the following
must be confirmed:

- [ ] Shadow telemetry logs visible in a controlled environment (local or testnet)
- [ ] No operational errors attributable to stop-aware code
- [ ] `stop_quality_class` distribution is plausible (not all `None`, not all `Premium`)
- [ ] `AtrFallback` stops do **not** produce `StopAnchor` (anchor_type absent)
- [ ] `SwingPoint` stops **do** produce `StopAnchor` (SwingLow for Long, SwingHigh for Short)
- [ ] `Exceptional` never appears as `stop_quality_class`
- [ ] `shadow_exceptional=true` appears occasionally (confirms the shadow comparison works)
- [ ] No boost applied to any score used in decision-making
- [ ] No increase in log noise under default `info` level (shadow logs stay at `debug`)
- [ ] Entry price, stop loss, and final decision identical with/without shadow metadata
- [ ] At least one full position lifecycle observed with shadow telemetry active
- [ ] No divergence between shadow-enabled and shadow-disabled runs

**Minimum observation period**: A few days of testnet operation with `RUST_LOG=robsond=debug`
(or `robsond::detector=debug` for less noise) before considering any boost application.

---

## 13. Rollback

### For This Runbook

This runbook is doc-only. Rollback is:

```bash
git revert <runbook-commit-hash>
```

### For Existing Runtime (Slices 001–004)

If a rollback of the shadow metadata is needed (not expected):

```bash
# Revert all four runtime commits (newest first)
git revert 020a0dcd  # telemetry logging
git revert 41e21a71  # shadow population
git revert 31384069  # classifier
git revert 500449fe  # domain types
```

**This is NOT recommended** unless a critical issue is found, because the shadow
code has zero behavioral impact by design. Prefer investigation over rollback.

---

## 14. Testnet / Prod Shadow Observation Checklist

Use this checklist when observing shadow telemetry in a live environment.

### Pre-Observation

- [ ] Confirm deployment includes commits through `020a0dcd`
- [ ] Set `RUST_LOG=robsond=debug` (or `robsond::detector=debug` for less noise) on the deployment
- [ ] Verify log output is accessible (`kubectl logs` or equivalent)
- [ ] Confirm no other config changes were made

### During Observation

- [ ] At least one detector signal fired (grepped by `"stop-aware entry shadow telemetry"`)
- [ ] `stop_anchor_present=true` observed for at least one SwingPoint signal
- [ ] `stop_anchor_present=false` observed for at least one AtrFallback signal (if applicable)
- [ ] `stop_quality_class` varies (not stuck on a single value)
- [ ] `Exceptional` never appears as `stop_quality_class`
- [ ] `shadow_exceptional=true` observed at least once (confirms shadow comparison)
- [ ] Entry decisions unchanged compared to pre-shadow behavior
- [ ] No ERROR or PANIC in daemon logs attributable to stop-aware code

### Post-Observation

- [ ] Restore `RUST_LOG` to default (`robsond=info`) to reduce log volume
- [ ] Record results in Run Log table above
- [ ] Note any anomalies or unexpected `stop_quality_class` distributions
- [ ] Update this runbook if findings differ from expectations

---

## Related Documentation

- [ADR-0035 — Stop-Aware Entry Policy (v4)](../adr/ADR-0035-stop-aware-entry-policy.md)
- [StopQuality Heuristics Spec](../analysis/2026-04-28-stop-quality-heuristics.md)
- [Implementation Guide](../analysis/2026-04-28-stop-aware-entry-implementation-guide.md)
- [VAL-001 — Testnet E2E Validation](val-001-testnet-e2e-validation.md)
- [ADR-0022 — Robson-Authored Position Invariant](../adr/ADR-0022-robson-authored-position-invariant.md)
