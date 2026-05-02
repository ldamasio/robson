# Stop-Aware Entry v4 — Slice 006 Plan (Shadow Evidence Review / Calibration)

**Date**: 2026-05-02
**Status**: PLANNING — observational only, no runtime authorization
**Slice**: 006 — Shadow Evidence Review / Calibration
**Branch context**: `stop-aware-shadow-testnet` @ `57645ed91df1585285b4d381b122535ce6e0025c`
**Image under observation**: `ghcr.io/rbxrobotica/robson-v2:sha-447aba4b`
**Related**:
- [ADR-0021 — Separation of Opportunity Detection and Technical Stop Analysis](../adr/ADR-0021-opportunity-detection-vs-technical-stop-analysis.md)
- [ADR-0024 — Stop-Aware Entry Policy](../adr/ADR-0024-stop-aware-entry-policy.md)
- [Discovery Report](2026-04-28-stop-aware-entry-discovery.md)
- [Implementation Guide](2026-04-28-stop-aware-entry-implementation-guide.md)
- [StopQuality Heuristics Spec](2026-04-28-stop-quality-heuristics.md)
- [Runtime Slice 001 Checklist](2026-04-28-stop-aware-entry-runtime-slice-001-checklist.md)
- [Shadow Validation Report (2026-05-02)](2026-05-02-stop-aware-entry-shadow-validation-report.md)
- [Shadow Validation Runbook](../runbooks/stop-aware-entry-shadow-validation.md)

---

## 0. Document Scope and Hard Boundaries

This document is a **planning artifact only**. It does not authorize, implement,
deploy, or schedule any runtime change. Its purpose is to define what Slice 006
will be once it is started, what evidence must be collected before it can be
considered complete, and what gates must remain closed throughout.

**Hard boundaries for Slice 006 (this slice does NOT do these things):**

- No `.rs` edits.
- No database migrations.
- No Kubernetes / GitOps changes.
- No new container image build or release.
- No new testnet stimulus required by Slice 006 itself (passive observation
  reuses existing arming flows when operationally convenient; new arming is not
  a Slice 006 deliverable).
- No frontend work.
- No production rollout.
- No boost application (`shadow_exceptional` stays `false` everywhere).
- No change to `RiskEngine`.
- No change to `TechnicalStopDistance`.
- No change to `DetectorSignal` or `EventBus`.
- No change to `StopQualityClassifier` thresholds.
- No use of `StopQuality` for any decision (sizing, approval, rejection, boost).
- No enabling of `Exceptional` anywhere.
- No silent reinterpretation of existing telemetry as production-grade evidence.

These boundaries are restated explicitly so that future readers cannot mistake
Slice 006 for an implementation slice.

---

## 1. Goal

Accumulate and review enough shadow telemetry, across a broader-than-current set
of conditions, to characterize the empirical behavior of the Stop-Aware Entry v4
shadow pipeline before any future implementation slice considers acting on it.

Specifically, Slice 006 aims to answer the following questions with data:

- What is the empirical distribution of `stop_quality_class` (None / Weak /
  Good / Premium) across the symbols, sides, and market conditions Robson
  actually encounters?
- What is the empirical distribution of `anchor_type` (SwingLow, SwingHigh,
  structural, fallback) and how often is `stop_anchor_present=false`?
- What is the empirical distribution of `technical_stop_method` (SwingPoint
  level_n, structural, ATR fallback, etc.) and how does it correlate with
  `stop_quality_class`?
- Does `AtrFallback` (or any non-structural fallback method) ever emit a
  `StopAnchor`? It must not.
- Is the shadow telemetry stable, i.e. does the same input under the same
  market conditions produce the same classification?
- Would the shadow classification, if used, have changed any historical
  decision — and if so, in which direction (accept / reject / size up /
  size down)? This is computed off-line and never applied.
- Are there false-positive signatures (e.g. `Good`/`Premium` on signals that
  the operator would, in retrospect, classify as poor) visible in the data?

The output of Slice 006 is **evidence and analysis**, not code.

## 2. Non-Goals

Slice 006 explicitly does not:

- Use `StopQuality` to influence any production decision.
- Apply or simulate boost in any path that affects state, sizing, or risk.
- Re-tune `StopQualityClassifier` thresholds. (Re-tuning, if ever justified,
  is a future slice with its own ADR amendment.)
- Enable the `Exceptional` flag anywhere.
- Re-architect detector cycle frequency or telemetry throttling. (Throttling
  is a known item from Section 7 of the Shadow Validation Report and will be
  addressed separately if and when production exposure is ever pursued.)
- Add new fields to `TechnicalStopAnalysisAudit`, `DetectorSignal`, or any
  domain entity.
- Introduce frontend visualizations of StopQuality.
- Authorize any production exposure or capital change.

## 3. Preconditions

Before Slice 006 can be considered active (even as observation):

- [ ] Shadow Validation Report (2026-05-02) is merged to `main` and remains the
      authoritative baseline.
- [ ] Image `sha-447aba4b` (or successor that is strictly a superset for
      shadow-mode behavior) is the image under observation. No image change
      is made by Slice 006 itself.
- [ ] `RUST_LOG` retains a directive that emits `robsond::detector` at `debug`
      so the `stop-aware entry shadow telemetry` line is captured. This is an
      operational precondition; it is not a code change.
- [ ] `active_positions=0`, `pending_approvals=[]`, no UNTRACKED positions on
      the observed account at the start of any observation window.
- [ ] No concurrent migration or capital change on the testnet account during
      observation windows (so confounding factors do not contaminate evidence).
- [ ] Operator has read this document and the Shadow Validation Report and
      explicitly acknowledges that Slice 006 is observational.

## 4. Evidence Already Collected

From the 2026-05-02 Shadow Validation Report:

| Dimension | Coverage |
|-----------|----------|
| Symbols | 1 (BTCUSDT) |
| Sides | 1 (LONG) |
| Independent runs | 2 |
| Market conditions | 1 (single, contiguous testnet session) |
| `stop_quality_class` values observed | 1 (`Good`) |
| `raw_score` values observed | 1 (`37`) |
| `anchor_type` values observed | 1 (`SwingLow`) |
| `technical_stop_method` values observed | 1 (`SwingPoint { level_n: 2 }`) |
| `technical_stop_confidence` values observed | 1 (`High`) |
| `detected_levels_count` values observed | 1 (`2`) |
| Time-to-first-telemetry | ~2s after arm (consistent across runs) |
| Approval calls | 0 |
| Orders executed | 0 |
| Boost applications | 0 |
| Final `active_positions` after each run | 0 |
| Final `pending_approvals` after each run | `[]` |

This evidence is **sufficient to confirm the pipeline works and is reproducible**
under one narrow scenario. It is **not sufficient** to characterize empirical
behavior or to motivate any decision-affecting use.

## 5. Evidence Still Missing

Slice 006 needs to expand coverage along the following axes (none of these
require new code):

- **Symbol diversity**: more than one trading pair, in line with the
  symbol-agnostic policy invariant (ADR-0023). Symbols appear here only as
  illustrative examples; the policy applies to every operated pair.
- **Side diversity**: at least one LONG and at least one SHORT observation per
  symbol where the symbol naturally produces both sides over the window.
- **Market-condition diversity**: trending up, trending down, ranging, and
  high-volatility windows.
- **Anchor diversity**: at least one observation each for `SwingLow`,
  `SwingHigh`, structural anchors, and an explicit `stop_anchor_present=false`
  case (e.g. when only ATR fallback is available).
- **Method diversity**: at least one observation each for
  `SwingPoint { level_n: N }` for the values of `level_n` actually produced
  by the detector, plus any structural variants and the ATR fallback path.
- **Quality-class diversity**: at least one observation each for `None`,
  `Weak`, `Good`, and `Premium` (where the market actually generates them;
  absence of a class is itself a finding).
- **Confidence diversity**: at least one observation each for the confidence
  levels emitted by `TechnicalStopDistance`.
- **Negative-control evidence**: observations where the operator, after the
  fact, would classify the signal as poor — used to estimate false-positive
  rates of the classifier without ever applying its output.
- **Stability evidence**: repeated arming under nominally identical conditions
  to confirm classification stability already suggested by the two-run
  reproducibility in the validation report.

## 6. Metrics To Collect From Shadow Telemetry

For each captured `stop-aware entry shadow telemetry` event, persist the
following fields (from the existing log line — no schema change required):

- `position_id`
- `symbol`
- `side`
- `stop_anchor_present`
- `anchor_type`
- `stop_quality_class`
- `raw_score`
- `boost_pct` (recorded for completeness; remains an unused parameter in
  Slice 006)
- `shadow_exceptional` (must be `false`)
- `technical_stop_method`
- `technical_stop_confidence`
- `detected_levels_count`
- Wall-clock timestamp of the log line
- Pod / image SHA active at the time

Aggregations to compute from the collected events:

- Counts and percentages by `stop_quality_class`.
- Counts and percentages by `anchor_type`, including the `None` / absent case.
- Counts and percentages by `technical_stop_method`.
- Joint distribution of `stop_quality_class` × `anchor_type`.
- Joint distribution of `stop_quality_class` × `technical_stop_method`.
- Joint distribution of `stop_quality_class` × `technical_stop_confidence`.
- Histogram of `raw_score` overall and conditioned on `stop_quality_class`.
- Per-symbol and per-side breakdowns of all of the above.
- Any event where `technical_stop_method` is a fallback (e.g. ATR-based) but
  `stop_anchor_present=true` — this is a **failure mode**, see Section 9.

All aggregations live in analysis artifacts (notes, spreadsheets, or markdown
under `docs/analysis/`). None of them is wired into the runtime.

## 7. Minimum Sample Size / Observation Period Proposal

These thresholds are **proposed minima** for considering Slice 006 complete.
They are deliberately conservative; the operator may extend any of them.

- **Time window**: at least one continuous observation period of 7 consecutive
  calendar days during which `robsond` is healthy on testnet, plus a second,
  non-contiguous 7-day window to control for time-local market regime.
- **Distinct symbols**: at least 3, drawn from the symbols Robson is
  operationally configured for.
- **Distinct armings**: at least 30 across the observation windows, of which at
  least 5 must terminate without a `StopAnchor` (i.e. fallback path).
- **Distinct `stop_quality_class` values observed**: at least 3 of
  `{None, Weak, Good, Premium}` (the fourth being a finding if it never occurs).
- **Distinct `anchor_type` values observed**: at least `SwingLow`, `SwingHigh`,
  and the absent / fallback case.
- **Stability replicates**: at least 2 independent armings under
  nominally-identical conditions per symbol, per side, to spot-check stability.

These are floors, not ceilings. If a class, anchor, or method is structurally
rare or absent, that absence is itself reportable evidence.

## 8. StopQuality Distribution Expectations

Stated as falsifiable expectations against which the collected data will be
checked. Deviations are findings, not failures of Slice 006.

- `Premium` is expected to be **rare**. A run in which `Premium` dominates is
  a calibration warning, not a green light.
- `None` and `Weak` together are expected to make up a non-trivial share of
  events, especially in ranging or low-structure regimes.
- `Good` is expected to be the modal class in trending regimes with clean
  swing structure.
- `stop_anchor_present=false` is expected to coincide with non-`SwingPoint`
  / non-structural `technical_stop_method` values.
- `raw_score` distribution should be continuous across observed events, not
  clustered at a single value. A degenerate single-value distribution
  (analogous to the one-point evidence in the validation report) is a
  calibration warning.
- Identical inputs should produce identical classifications (stability).

## 9. Failure Modes To Watch

Slice 006 watches for these signatures during observation. Any occurrence is
recorded with `position_id`, timestamp, and full telemetry line, and the
operator is notified before continuing observation.

- **Fallback emits anchor**: any event with a non-structural
  `technical_stop_method` (e.g. ATR fallback) where `stop_anchor_present=true`.
  This violates the design contract.
- **Anchor without method**: `stop_anchor_present=true` with a
  `technical_stop_method` that does not support structural anchors.
- **Class/method mismatch**: `stop_quality_class=Premium` with a
  `technical_stop_method` that should not produce Premium-grade signals (e.g.
  fallback).
- **Confidence/class mismatch**: `stop_quality_class=Premium` or `Good` with
  `technical_stop_confidence=Low`.
- **`shadow_exceptional=true` anywhere**: must never occur in Slice 006.
- **Boost applied**: any sign in logs or downstream events that boost was
  applied to sizing, score, or risk parameters.
- **Decision drift**: any sign that StopQuality influenced an approval, a
  rejection, or an order. Must never occur.
- **Telemetry storm**: rate of `stop-aware entry shadow telemetry` lines
  materially higher than the ~1/cycle expected by detector cadence — would
  reveal a regression in the detector loop, not a Slice 006 deliverable to fix
  but a finding to report.
- **Class flip on identical input**: same `position_id`-equivalent conditions
  producing different `stop_quality_class` within a short window without an
  underlying market change — instability finding.

None of these failure modes triggers a Slice 006 code change. They trigger
documentation, a notification to the operator, and (if severe) a recommendation
to halt observation pending a separate, properly-scoped slice.

## 10. Criteria Before Considering Any Future Boost Application

Slice 006 does not apply boost. It does, however, define the bar that a
future, separately-authorized slice would have to clear before boost can even
be considered:

- All Section 5 evidence gaps closed at or above the Section 7 minima.
- Empirical distributions in Section 8 confirmed as non-degenerate.
- Zero occurrences of any Section 9 failure mode during the observation
  windows, or all occurrences fully explained and resolved by an upstream fix
  whose effect was then re-validated under shadow mode.
- An ADR amendment (or successor ADR) is written that explicitly authorizes
  boost, defines its mathematical effect on sizing or score, defines the
  RiskEngine-side guards that bound it, and defines a kill-switch.
- RiskEngine sign-off recorded in writing: RiskEngine remains the final
  authority and must explicitly accept the proposed boost semantics.
- A staged rollout plan exists (e.g. shadow → simulated apply → bounded live
  apply) with explicit go/no-go gates per stage.
- Operator explicitly authorizes that future slice. Slice 006 cannot
  pre-authorize it.

Until every one of these is satisfied, boost remains forbidden.

## 11. Criteria Before Production Exposure

Stricter than Section 10 because production exposure goes beyond shadow data:

- All Section 10 criteria satisfied.
- Detector-cycle telemetry rate addressed (throttling or deduplication) so
  production logs are not flooded.
- A documented kill-switch path exists to disable any StopQuality-driven
  effect at runtime without redeploy.
- A documented rollback path exists for the GitOps revision that introduces
  the production effect.
- Capital sufficient to exercise the order path (the `quantity below Binance
  step size 0.001` rejection in the validation report illustrates that the
  testnet wallet is too small for end-to-end paths on some symbols).
- An incident-response plan exists for the case where the production effect
  must be disabled mid-session.
- Robson-authored position invariant (ADR-0022) and symbol-agnostic policy
  invariant (ADR-0023) explicitly re-verified against the proposed change.
- Operator explicitly authorizes the production rollout. Slice 006 cannot
  pre-authorize it.

## 12. Proposed Operational Runbook For Passive Observation

Deliberate testnet stimuli may be used only under a separately authorized
operational runbook; they are not an implementation change and do not authorize
Slice 006 runtime behavior.

Steps for an operator running an observation window. Each step is read-only
or non-mutating with respect to Robson runtime; nothing here changes code,
config, image, or schema.

1. **Confirm baseline state** before the window:
   - Image SHA on the observed pod.
   - `RUST_LOG` directive includes `robsond=debug` (or equivalent).
   - `active_positions=0`, `pending_approvals=[]`, no UNTRACKED positions.
2. **Open a log capture** for the observation window targeting the lines of
   interest. The capture writes to a file on the operator workstation; it does
   not change the cluster.
3. **Let the system run normally** during the window. Do not introduce
   special stimuli specifically for Slice 006. If the operator independently
   arms positions for unrelated operational reasons, those armings count as
   passive evidence.
4. **At the end of the window**:
   - Confirm `active_positions=0`, `pending_approvals=[]`, no UNTRACKED
     positions, no -2015 errors.
   - Save the log capture under
     `docs/analysis/data/2026-05-XX-stop-aware-shadow-window-N.log` (path is
     suggestive; operator can choose).
   - Snapshot the image SHA at end of window for cross-check against start.
5. **Run the data extraction** described in Section 13 over the captured logs.
6. **Record findings** in a follow-on analysis note — never in this planning
   document, which stays a plan.

If at any point a Section 9 failure mode is observed, the operator stops the
window early, records what was seen, and does not start a new window until the
finding is triaged.

## 13. Proposed Data Extraction Commands From Logs

These commands operate on captured log files only. They do not touch the
cluster, the database, or the runtime. They are illustrative; the operator
may substitute equivalents.

Capture the lines of interest from a kubectl log stream into a local file:

```
kubectl -n robson-testnet logs <pod> \
  | grep "stop-aware entry shadow telemetry" \
  > docs/analysis/data/2026-05-XX-stop-aware-shadow-window-N.log
```

Count events by `stop_quality_class`:

```
grep -oE 'stop_quality_class=[A-Za-z]+' \
  docs/analysis/data/2026-05-XX-stop-aware-shadow-window-N.log \
  | sort | uniq -c | sort -rn
```

Count events by `anchor_type`:

```
grep -oE 'anchor_type=[A-Za-z(:) ]+' \
  docs/analysis/data/2026-05-XX-stop-aware-shadow-window-N.log \
  | sort | uniq -c | sort -rn
```

Count events by `technical_stop_method`:

```
grep -oE 'technical_stop_method=[A-Za-z]+( \{ [^}]+ \})?' \
  docs/analysis/data/2026-05-XX-stop-aware-shadow-window-N.log \
  | sort | uniq -c | sort -rn
```

Detect the **fallback-emits-anchor failure mode** (Section 9):

```
grep "technical_stop_method=AtrFallback" \
  docs/analysis/data/2026-05-XX-stop-aware-shadow-window-N.log \
  | grep "stop_anchor_present=true"
```

Detect any line where `shadow_exceptional=true` (must be empty):

```
grep "shadow_exceptional=true" \
  docs/analysis/data/2026-05-XX-stop-aware-shadow-window-N.log
```

Detect class instability for the same `position_id`:

```
awk '/stop-aware entry shadow telemetry/ {
  match($0, /position_id=([0-9a-f-]+)/, p);
  match($0, /stop_quality_class=([A-Za-z]+)/, q);
  print p[1] "\t" q[1]
}' docs/analysis/data/2026-05-XX-stop-aware-shadow-window-N.log \
  | sort | uniq | awk -F'\t' '{c[$1]++} END {for (k in c) if (c[k] > 1) print c[k], k}'
```

Histogram of `raw_score`:

```
grep -oE 'raw_score=[0-9]+' \
  docs/analysis/data/2026-05-XX-stop-aware-shadow-window-N.log \
  | awk -F= '{print $2}' | sort -n | uniq -c
```

These commands are reference-grade only and intentionally simple. They will
not fail in a way that mutates state. If a richer extraction is needed (e.g.
joining across windows, time-bucketing), the operator may write a one-off
script under `docs/analysis/data/` — it is still observational and outside
Slice 006's scope to upstream into the runtime.

## 14. Rollback / No-Op Guarantees

Slice 006 has no code, schema, infra, or release artifact to roll back. Its
no-op guarantees are stated by what it does not change:

- **Runtime no-op**: no `.rs`, no migration, no manifest, no image change.
  The runtime is byte-for-byte identical before and after Slice 006.
- **Decision no-op**: `RiskEngine` remains the final authority on stop
  distance, position sizing, and risk parameters. `StopQuality` does not
  influence approval, rejection, sizing, or boost in Slice 006.
- **TechnicalStopDistance no-op**: not modified, not retuned, not bypassed.
- **DetectorSignal / EventBus no-op**: not modified.
- **Boost no-op**: `shadow_exceptional` stays `false`. `boost_pct` is read
  in telemetry only; never applied.
- **Exceptional no-op**: not enabled.
- **GitOps no-op**: ArgoCD continues to reconcile the same revision that was
  active at the start of the window. Slice 006 does not bump it.
- **Production no-op**: not exposed to production. No production
  authorization is granted by Slice 006.
- **Frontend no-op**: not in scope.

If something during observation appears to violate any of these — for example,
logs suggesting boost was applied, or sizing changing in a way that correlates
with `stop_quality_class` — that is a Section 9 failure mode and the
appropriate response is to stop observation and triage, not to "roll back"
Slice 006 (which has nothing to roll back).

## 15. Recommended Next Implementation Slice After Slice 006

If, and only if, Slice 006 produces evidence that satisfies the bars in
Sections 7–11, the recommended next slice is:

- **Slice 007 (proposed name): Shadow Decision-Mirror.**
  Purpose: in shadow mode only, log what the decision *would have been* if
  StopQuality were used, side-by-side with the actual decision, without
  applying the alternate decision. This is still observational; it adds a
  parallel computation path, never a parallel effect.
  Authorization: this slice is not authorized by Slice 006. It would require
  its own ADR amendment, its own runtime slice checklist (analogous to the
  Slice 001 checklist), and explicit operator authorization.

Slice 007 would precede any slice that *applies* a StopQuality-driven effect
to sizing, approval, or boost. Such an applying slice (call it Slice 008 for
naming continuity) would in turn require all of Section 11's criteria.

If Slice 006 evidence is **inconclusive or negative**, the recommended next
slice is documentation-only: an ADR amendment that records the empirical
findings, narrows the policy, and either parks Stop-Aware Entry v4 or
specifies a new path forward. Under no circumstances does inconclusive
evidence promote the feature.

---

## Invariants Restated (must hold throughout Slice 006)

- StopQuality remains shadow-only.
- RiskEngine remains the final authority.
- TechnicalStopDistance remains unchanged.
- Absence of a StopQuality boost must never reject a candidate.
- Boost must not rescue an invalid signal.
- `Exceptional` remains disabled and not authorized.
- No production rollout is authorized by Slice 006.
- No frontend work is part of Slice 006.
- Robson-authored position invariant (ADR-0022) and symbol-agnostic policy
  invariant (ADR-0023) continue to apply.
- Technical Stop is from chart analysis, not `entry × (1 − pct)` (ADR-0021).

## Acceptance Criteria For Calling Slice 006 "Done"

Slice 006 is "done" when, and only when, all of the following are true:

- [ ] At least the Section 7 minima of shadow telemetry events have been
      collected, across more than one symbol, more than one side where the
      market produced both, and more than one market condition.
- [ ] Section 6 aggregations have been produced and recorded as analysis
      artifacts.
- [ ] Section 8 distribution expectations have been checked against the data,
      and deviations recorded as findings (not silently ignored).
- [ ] Zero Section 9 failure modes occurred, or every occurrence was recorded,
      explained, and resolved before continuing observation.
- [ ] No production decision was changed during any observation window.
- [ ] No boost was applied during any observation window.
- [ ] No `Exceptional` was enabled in any production path.
- [ ] No fallback `technical_stop_method` ever co-occurred with
      `stop_anchor_present=true`.
- [ ] No order was executed *because of* observation activity.
- [ ] No increase in risk surface was introduced by Slice 006.
- [ ] Final `active_positions=0` and `pending_approvals=[]` after each
      observation window.
- [ ] A follow-on analysis note has been written summarizing findings and
      explicitly stating whether Section 10 / Section 11 bars are met or
      not. That note is the input to any decision about Slice 007.

---

## Changelog

| Date       | Change                          | Author |
|------------|---------------------------------|--------|
| 2026-05-02 | Initial planning draft          | Claude |
