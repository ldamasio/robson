# Stop-Aware Entry v4 — Slice 007 Worked Examples (Shadow Decision-Mirror)

**Date:** 2026-05-03
**Status:** Design artifact — manual computation, no runtime implementation
**Slice:** 007 — Shadow Decision-Mirror (design-first)
**Branch:** `stop-aware-shadow-testnet`
**Mirror version:** `v0.0-manual`
**Related:**
- [Slice 007 Plan — Shadow Decision-Mirror Design](2026-05-03-stop-aware-slice-007-shadow-decision-mirror-plan.md)
- [Slice 006 Calibration Summary](2026-05-03-stop-aware-slice-006-calibration-summary.md)
- [ADR-0024 — Stop-Aware Entry Policy (v4)](../adr/ADR-0024-stop-aware-entry-policy.md)

---

## 0. Scope and Hard Boundaries

This document is a **manual computation artifact** for Slice 007. It applies the Shadow
Decision-Mirror schema (Section 7 of the Slice 007 plan) to each of the five distinct
signal types represented in the Slice 006 telemetry, computing what the mirror record
would contain for each case.

**No runtime code is changed. No boost is applied. No testnet stimulus is issued.**

This document satisfies design gate G2 from the Slice 007 plan: *"Worked example: mirror
output computed manually from all six Slice 006 events"*, and contributes to gate G3
(*"decision_delta taxonomy validated against worked examples"*).

**Hard boundaries for this document:**
- No `.rs` edits
- No database changes
- No Kubernetes / GitOps changes
- No boost application (shadow or real)
- No RiskEngine change
- No new testnet stimulus
- No commit, push, or deploy

---

## 1. Evidence Base

All five examples derive from the six distinct telemetry events collected in Slice 006
(Windows 1–5). The BTCUSDT LONG duplicate (`019de67f`, Run 2 of Window 1) produced
telemetry identical to Run 1 and is treated as corroborating evidence for E-01, not an
additional worked example.

| ID | position_id (prefix) | symbol | side | anchor | stop_quality | raw_score | boost_pct | method | confidence | levels | window |
|----|---------------------|--------|------|--------|-------------|-----------|-----------|--------|------------|--------|--------|
| E-01 | `019de650` | BTCUSDT | Long | SwingLow | Good | 37 | 0.10 | SwingPoint(2) | High | 2 | W1 |
| E-02 | `019de906` (early) | BTCUSDT | Short | SwingHigh | Good | 38 | 0.10 | SwingPoint(1) | Medium | 1 | W2 |
| E-03 | `019de906` (final) | BTCUSDT | Short | SwingHigh | None | 0 | 0 | SwingPoint(1) | Medium | 1 | W3 |
| E-04 | `019dec8d` | ETHUSDT | Long | SwingLow | Premium | 47 | 0.15 | SwingPoint(2) | High | 2 | W4 |
| E-05 | `019dee3c` | ETHUSDT | Short | SwingHigh | Good | 38 | 0.10 | SwingPoint(1) | Medium | 1 | W5 |

---

## 2. Assumptions and Analytical Framework

### 2.1 Scoring threshold boundaries

The exact `StopQualityClassifier` threshold values are not published in the Slice 006
evidence documents. The following are inferred from observed data:

| Boundary | Inference | Basis |
|----------|-----------|-------|
| None / Good | Exists below raw_score=37 | `019de906` degraded Good(38)→None(0) within same detection setup; 0 is the `None` sentinel |
| Good / Premium | Lies in the open interval (38, 47) | Highest Good = 38; lowest Premium = 47; boundary is uncharacterized |
| Premium / Exceptional | Exceptional is a flag, not a score tier | `shadow_exceptional=false` in all six observations |

**Key consequence:** A 10% boost on Good scores 37–38 yields 40.70–41.80. This range
falls within the uncharacterized interval (38, 47). Whether 40.70 or 41.80 crosses the
Good/Premium boundary cannot be determined from existing evidence. All worked examples
use the conservative assumption that the boundary is above 41.80 and note this
explicitly where it affects `threshold_crossed_shadow`.

### 2.2 real_decision in all observed cases

In all five Slice 006 windows, positions used `entry_policy.mode=immediate` with
`approval=human_confirmation`. Shadow telemetry fires upstream of order construction
(confirmed in Window 1 report, §5). For BTCUSDT LONG (E-01), a quantity-below-step-size
rejection would have occurred at order construction time, which is reachable only
post-approval — and no approval was ever granted. The real decision at the mirror
observation point (immediately after the policy layer emits its outcome) is
`AwaitingApproval` for all five examples.

For E-03 (None class, final cycle of `019de906`): the cycling pattern documented in
Window 3 §7 confirms that every cycle of `019de906` — including the final `None` cycle —
progressed through `RiskChecked → AwaitingApproval` before expiring.

### 2.3 mirrored_decision invariant

`mirrored_decision` equals `real_decision` in all cases by definition (Slice 007 plan,
Section 7). The mirror is a read-only analytical record. A hypothetical boost that
changes the score or even the classification tier does not alter `mirrored_decision` —
it only affects `decision_delta` and `delta_reason`.

### 2.4 Gate invariants

All gate-invariant flags (`risk_engine_unchanged`, `entry_policy_unchanged`,
`approval_required_unchanged`, `sizing_unchanged`, `monthly_slots_unchanged`,
`execution_unchanged`) are `true` in every example. This is the expected baseline:
StopQuality is not wired to any decision gate in the current implementation.
Any `false` value would indicate a bug in the mirror implementation, not an expected
outcome.

### 2.5 exceptional_ignored — architectural rule

In the Slice 007 mirror design, the Exceptional boost tier is **disabled by
architecture**. The `exceptional_ignored=true` flag expresses this design commitment:
the mirror never evaluates the Exceptional path, regardless of what `shadow_exceptional`
reports in the underlying telemetry.

This is distinct from the per-event observation of `shadow_exceptional`. In all Slice 006
events, `shadow_exceptional=false` was observed — meaning the detector itself also did
not trigger Exceptional. Both facts coexist independently:

| Fact | Source | Value |
|------|--------|-------|
| `shadow_exceptional` | Observed telemetry (detector output) | `false` (all 6 events) |
| `exceptional_ignored` | Mirror architectural rule (design decision) | `true` (all examples) |

Setting `exceptional_ignored=false` would imply the mirror is capable of evaluating and
applying the Exceptional path — which contradicts the Slice 007 design constraint.
The correct value is `true` throughout, because the Exceptional mode is unconditionally
disabled for the v0.0-manual mirror design. The observed `shadow_exceptional=false` is
noted in each example as a separate fact and does not affect this value.

### 2.6 decision_delta=ScoreOnly — definition

`decision_delta=ScoreOnly` is the mirror classification for events where a hypothetical
boost changes the numeric score (`boosted_score_shadow` ≠ `real_score`) but does not
change any real outcome. Specifically, `ScoreOnly` asserts that all of the following
hold simultaneously:

- **`real_decision` is not altered** — the outcome (AwaitingApproval, Rejected, etc.)
  is identical with or without the hypothetical boost.
- **Approval gate is not altered** — the `human_confirmation` requirement is unchanged.
- **Sizing is not altered** — position size is computed identically.
- **Monthly slots are not altered** — no slot is consumed or modified by the mirror.
- **Execution is not altered** — no real order is placed, modified, or cancelled.
- **The hypothetical score difference** (`boosted_score_shadow` − `real_score`) is
  recorded **only as a shadow mirror observation** and is never passed to any real
  evaluation path.

`ScoreOnly` does not imply the score change is negligible. It asserts that, given the
current architecture where StopQuality is not wired to any decision gate, the score
change has zero effect on real outcomes. It is a purely analytical record.

---

## 3. Worked Examples

### E-01 — BTCUSDT LONG, Good, raw_score=37

**Source:** Windows 1–2, Run 1. Two independent runs produced identical telemetry.
Run 2 (`019de67f`) is corroborating; mirror record is identical.

**Raw telemetry (reference log line):**
```
DEBUG robsond::detector: stop-aware entry shadow telemetry
  position_id=019de650-1fce-7e72-969a-0fd7fabebd5c
  symbol=BTCUSDT  side=Long
  stop_anchor_present=true  anchor_type=Some(SwingLow)
  stop_quality_class=Good  raw_score=37  boost_pct=0.10
  shadow_exceptional=false
  technical_stop_method=SwingPoint { level_n: 2 }
  technical_stop_confidence=High  detected_levels_count=2
```

**Mirror record:**

| Field | Value | Derivation / justification |
|-------|-------|---------------------------|
| `mirror_version` | `v0.0-manual` | Manual design-phase computation |
| `source_signal_id` | `BTCUSDT_LONG_2026-05-02_SL2_RS37` | Symbol\_Side\_Date\_AnchorMethod\_RawScore |
| `position_id` | `019de650-1fce-7e72-969a-0fd7fabebd5c` | From telemetry |
| `symbol` | `BTCUSDT` | From telemetry |
| `side` | `Long` | From telemetry |
| `stop_quality_class` | `Good` | From telemetry |
| `raw_score` | `37` | From telemetry |
| `hypothetical_boost_pct` | `0.10` | From shadow metadata (`boost_pct` field) |
| `hypothetical_boost_cap` | `0.15` | Production cap; Exceptional tier (0.20) disabled by mirror design (exceptional_ignored=true, see §2.5) |
| `real_decision` | `AwaitingApproval` | Query passed RiskEngine; `human_confirmation` gate active; no approval granted in window |
| `real_score` | `37` | `raw_score`; boost not applied to real path |
| `boosted_score_shadow` | `40.70` | `37 × (1 + 0.10) = 40.70` |
| `mirrored_decision` | `AwaitingApproval` | Mirror never overrides real_decision |
| `decision_delta` | `ScoreOnly` | Score changes 37→40.70; classification tier stays Good; no real gate crossed (see §2.6) |
| `delta_reason` | "10% boost raises raw_score from 37 to 40.70, remaining within the Good tier (inferred Good/Premium boundary is in the open interval (38, 47); 40.70 falls below the upper bound of 47). No decision gate is sensitive to a score change within the same tier. Real outcomes are all unchanged: real_decision=AwaitingApproval, approval gate unchanged, sizing unchanged, monthly slot consumption unchanged, execution unchanged. boosted_score_shadow=40.70 is recorded only as a shadow mirror observation." | Manual analysis |
| `threshold_crossed_shadow` | `false` | 40.70 < 47 (lower bound of Premium); conservative assumption: boundary > 41.80 (see §2.1) |
| `risk_engine_unchanged` | `true` | RiskEngine does not receive `stop_quality_class` or `raw_score` as inputs |
| `entry_policy_unchanged` | `true` | EntryPolicy outcome is determined by signal validity, not StopQuality score |
| `approval_required_unchanged` | `true` | `human_confirmation` gate is set by `entry_policy_mode` config, not raw_score |
| `sizing_unchanged` | `true` | Position sizing is not a function of StopQuality in current design |
| `monthly_slots_unchanged` | `true` | Mirror never consumes slots; invariant by definition |
| `execution_unchanged` | `true` | Mirror never executes orders; no real order was placed in any observed window |
| `exceptional_ignored` | `true` | Exceptional disabled by Slice 007 mirror design (architectural rule, see §2.5); the Exceptional boost tier is not evaluated regardless of telemetry value. Observed: `shadow_exceptional=false` |

**Notes:**
- Baseline case: lowest Good raw_score (37) in the evidence base. Boost of 10% yields 40.70.
- The 40.70 score sits approximately 15% below the only evidence-based lower bound for the
  Good/Premium boundary (47). Even under the most aggressive threshold assumption (boundary=39),
  40.70 would cross into Premium — but this assumption is not supported by evidence.
- `decision_delta=ScoreOnly`: score moves, all real outcomes unchanged (real_decision,
  approval, sizing, slots, execution). Purely analytical observation (see §2.6).

---

### E-02 — BTCUSDT SHORT, Good, raw_score=38

**Source:** Window 2, early cycles of `019de906`. The query lifecycle was fully confirmed:
`Accepted → Processing → RiskChecked → AwaitingApproval` (Window 2, §7).

**Raw telemetry (reference log line):**
```
DEBUG robsond::detector: stop-aware entry shadow telemetry
  position_id=019de906-f5d8-73c3-937c-6bdfcf9d3cca
  symbol=BTCUSDT  side=Short
  stop_anchor_present=true  anchor_type=Some(SwingHigh)
  stop_quality_class=Good  raw_score=38  boost_pct=0.10
  shadow_exceptional=false
  technical_stop_method=SwingPoint { level_n: 1 }
  technical_stop_confidence=Medium  detected_levels_count=1
```

**Mirror record:**

| Field | Value | Derivation / justification |
|-------|-------|---------------------------|
| `mirror_version` | `v0.0-manual` | — |
| `source_signal_id` | `BTCUSDT_SHORT_2026-05-02_SH1_RS38` | — |
| `position_id` | `019de906-f5d8-73c3-937c-6bdfcf9d3cca` | From telemetry (early-cycle reference) |
| `symbol` | `BTCUSDT` | From telemetry |
| `side` | `Short` | From telemetry |
| `stop_quality_class` | `Good` | From telemetry |
| `raw_score` | `38` | From telemetry |
| `hypothetical_boost_pct` | `0.10` | From shadow metadata |
| `hypothetical_boost_cap` | `0.15` | Exceptional tier (0.20) disabled by mirror design (exceptional_ignored=true, see §2.5) |
| `real_decision` | `AwaitingApproval` | Confirmed in Window 2 §7 |
| `real_score` | `38` | No boost applied to real path |
| `boosted_score_shadow` | `41.80` | `38 × (1 + 0.10) = 41.80` |
| `mirrored_decision` | `AwaitingApproval` | Equals real_decision |
| `decision_delta` | `ScoreOnly` | Score changes 38→41.80; Good tier retained under conservative threshold assumption; no real gate crossed (see §2.6) |
| `delta_reason` | "10% boost raises raw_score from 38 to 41.80. The highest-observed Good score is 38 and the only observed Premium score is 47; the Good/Premium boundary is in the uncharacterized interval (38, 47). A score of 41.80 falls within this interval. Under the conservative assumption that the boundary exceeds 41.80, no classification change occurs and decision_delta=ScoreOnly. If the boundary is ≤ 41.80, decision_delta would be ClassificationChanged — this cannot be resolved without exact threshold documentation. Real outcomes are all unchanged regardless of which classification applies: real_decision=AwaitingApproval, approval gate unchanged, sizing unchanged, monthly slot consumption unchanged, execution unchanged. boosted_score_shadow=41.80 is recorded only as a shadow mirror observation." | Manual analysis; threshold ambiguity noted |
| `threshold_crossed_shadow` | `false` | Conservative: 41.80 assumed below Good/Premium boundary; exact boundary unknown |
| `risk_engine_unchanged` | `true` | — |
| `entry_policy_unchanged` | `true` | — |
| `approval_required_unchanged` | `true` | — |
| `sizing_unchanged` | `true` | — |
| `monthly_slots_unchanged` | `true` | — |
| `execution_unchanged` | `true` | — |
| `exceptional_ignored` | `true` | Exceptional disabled by mirror design (architectural rule, see §2.5). Observed: `shadow_exceptional=false` |

**Notes:**
- E-02 carries the highest observed Good raw_score (38) and its boosted score (41.80) is
  the closest value to the uncharacterized Good/Premium boundary among all examples.
- This case is the most sensitive to the threshold ambiguity identified in §2.1. Before any
  boost-enabling slice, the exact Good/Premium boundary must be extracted from the
  `StopQualityClassifier` source and documented, as a 10% boost on the typical SHORT Good
  score may produce a classification change.
- Cross-symbol note: E-05 (ETHUSDT SHORT, Good, RS=38) produces a mirror record identical to
  E-02 in every field, confirming the ADR-0023 symbol-agnostic invariant at the mirror level.

---

### E-03 — BTCUSDT SHORT, None (intra-position degradation), raw_score=0

**Source:** Window 3, final cycle of `019de906` at 14:30:44 UTC. Prior cycles of the same
position produced Good (E-02). The degradation occurred within a single detector cycle;
anchor type, method, confidence, and detected_levels_count were unchanged. This is the first
and only `StopQuality=None` observation in the Slice 006 evidence base.

**Raw telemetry (reference log line):**
```
DEBUG robsond::detector: stop-aware entry shadow telemetry
  position_id=019de906-f5d8-73c3-937c-6bdfcf9d3cca
  symbol=BTCUSDT  side=Short
  stop_anchor_present=true  anchor_type=Some(SwingHigh)
  stop_quality_class=None  raw_score=0  boost_pct=0
  shadow_exceptional=false
  technical_stop_method=SwingPoint { level_n: 1 }
  technical_stop_confidence=Medium  detected_levels_count=1
```

**Mirror record:**

| Field | Value | Derivation / justification |
|-------|-------|---------------------------|
| `mirror_version` | `v0.0-manual` | — |
| `source_signal_id` | `BTCUSDT_SHORT_2026-05-02_SH1_RS0_NONE` | None-degradation variant; same position as E-02, final cycle |
| `position_id` | `019de906-f5d8-73c3-937c-6bdfcf9d3cca` | Same position_id as E-02; final detector cycle |
| `symbol` | `BTCUSDT` | From telemetry |
| `side` | `Short` | From telemetry |
| `stop_quality_class` | `None` | From telemetry |
| `raw_score` | `0` | From telemetry |
| `hypothetical_boost_pct` | `0` | `boost_pct=0` for None class; from shadow metadata |
| `hypothetical_boost_cap` | `0.15` | Cap unchanged; moot since effective boost is 0 |
| `real_decision` | `AwaitingApproval` | Confirmed: cycling pattern in Window 3 §7 shows every cycle of `019de906`, including the final None cycle, produced a query that reached AwaitingApproval before TTL expiry |
| `real_score` | `0` | No boost applicable |
| `boosted_score_shadow` | `0.00` | `0 × (1 + 0) = 0.00` |
| `mirrored_decision` | `AwaitingApproval` | Equals real_decision |
| `decision_delta` | `None` | Score: 0→0 (unchanged). Classification: None→None (unchanged). No gate crossed. |
| `delta_reason` | "StopQuality=None carries boost_pct=0 by design. Applying a 0% boost to raw_score=0 yields boosted_score_shadow=0. No change in score, classification, or any decision gate. The mirror record is computationally inert for this event: the None class provides no boost signal to hypothetically evaluate. decision_delta=None indicates the hypothetical boost dimension contributes nothing for this observation." | — |
| `threshold_crossed_shadow` | `false` | boosted_score=0; no threshold is crossed by a zero score |
| `risk_engine_unchanged` | `true` | Boost=0; RiskEngine output is trivially unchanged |
| `entry_policy_unchanged` | `true` | — |
| `approval_required_unchanged` | `true` | — |
| `sizing_unchanged` | `true` | — |
| `monthly_slots_unchanged` | `true` | — |
| `execution_unchanged` | `true` | — |
| `exceptional_ignored` | `true` | Exceptional disabled by mirror design (architectural rule, see §2.5). Observed: `shadow_exceptional=false` |

**Notes:**
- E-03 is the degenerate case. `StopQuality=None` collapses the boost computation to a
  no-op: `boost_pct=0` means `boosted_score_shadow = raw_score = 0`. The mirror record is
  correct but carries zero analytical information about what boost would have changed.
- `decision_delta=None` does not mean the real decision was negligible — the query reached
  `AwaitingApproval`. It means the hypothetical StopQuality boost dimension adds nothing.
  `decision_delta` reports only on the boost-delta axis, not on overall decision significance.
- Intra-position context: E-02 (Good, RS=38, same `position_id`) and E-03 (None, RS=0)
  are from the same position across consecutive detector cycles. The mirror produces
  `decision_delta=ScoreOnly` for E-02 and `decision_delta=None` for E-03 — two different
  outcomes from the same position lifecycle, driven solely by the degradation of
  `stop_quality_class`.

---

### E-04 — ETHUSDT LONG, Premium, raw_score=47

**Source:** Window 4. First `Premium` observation in the evidence base; highest raw_score
observed (47). The Slice 007 plan (§12, design acceptance criteria) designates this as the
primary reference case.

**Raw telemetry (reference log line):**
```
DEBUG robsond::detector: stop-aware entry shadow telemetry
  position_id=019dec8d-0cc3-7201-830d-a6b03c65804c
  symbol=ETHUSDT  side=Long
  stop_anchor_present=true  anchor_type=Some(SwingLow)
  stop_quality_class=Premium  raw_score=47  boost_pct=0.15
  shadow_exceptional=false
  technical_stop_method=SwingPoint { level_n: 2 }
  technical_stop_confidence=High  detected_levels_count=2
```

**Mirror record:**

| Field | Value | Derivation / justification |
|-------|-------|---------------------------|
| `mirror_version` | `v0.0-manual` | — |
| `source_signal_id` | `ETHUSDT_LONG_2026-05-03_SL2_RS47` | — |
| `position_id` | `019dec8d-0cc3-7201-830d-a6b03c65804c` | From telemetry |
| `symbol` | `ETHUSDT` | From telemetry |
| `side` | `Long` | From telemetry |
| `stop_quality_class` | `Premium` | From telemetry |
| `raw_score` | `47` | From telemetry |
| `hypothetical_boost_pct` | `0.15` | From shadow metadata; Premium class boost |
| `hypothetical_boost_cap` | `0.15` | Exceptional tier (0.20) disabled by mirror design (exceptional_ignored=true, see §2.5) |
| `real_decision` | `AwaitingApproval` | Confirmed in Window 4 §7: `entry_price=2300.85 USDT`, `stop_loss=2270.5550 USDT`, query reached AwaitingApproval and expired by TTL |
| `real_score` | `47` | No boost applied to real path |
| `boosted_score_shadow` | `54.05` | `47 × (1 + 0.15) = 54.05` |
| `mirrored_decision` | `AwaitingApproval` | Equals real_decision |
| `decision_delta` | `ScoreOnly` | Score changes 47→54.05; Premium is the highest accessible tier; no classification escalation possible; no real gate crossed (see §2.6) |
| `delta_reason` | "15% boost raises raw_score from 47 to 54.05. Premium is the highest classification tier; the Exceptional boost tier is disabled by mirror design (exceptional_ignored=true). No higher tier exists to trigger ClassificationChanged. The score increase of 7.05 points is absorbed within the Premium tier. Real outcomes are all unchanged: real_decision=AwaitingApproval, approval gate unchanged, sizing unchanged, monthly slot consumption unchanged, execution unchanged. boosted_score_shadow=54.05 is recorded only as a shadow mirror observation." | Manual analysis |
| `threshold_crossed_shadow` | `false` | Premium is the highest accessible tier; no higher scoring threshold exists in current architecture |
| `risk_engine_unchanged` | `true` | — |
| `entry_policy_unchanged` | `true` | — |
| `approval_required_unchanged` | `true` | — |
| `sizing_unchanged` | `true` | — |
| `monthly_slots_unchanged` | `true` | — |
| `execution_unchanged` | `true` | — |
| `exceptional_ignored` | `true` | Exceptional disabled by Slice 007 mirror design (architectural rule, see §2.5); the Exceptional boost tier is not evaluated regardless of telemetry value. Observed: `shadow_exceptional=false` |

**Notes:**
- E-04 produces the highest `boosted_score_shadow` in the evidence base (54.05) yet still
  yields `decision_delta=ScoreOnly`. This illustrates a key property of the mirror design:
  score magnitude does not determine gate sensitivity. What matters is whether any real
  decision gate is wired to respond to the score — and none currently are.
- `exceptional_ignored=true` reflects the Slice 007 architectural rule (§2.5): the
  mirror does not evaluate the Exceptional boost tier, regardless of what
  `shadow_exceptional` reports. In E-04, the observed value is `shadow_exceptional=false`
  — the detector itself also did not trigger Exceptional. Both facts are independent:
  the architectural rule applies universally, and the telemetry observation confirms no
  Exceptional event occurred in this window.
- Single-observation caveat: This is the only Premium observation in the evidence base.
  The mirror computation is structurally valid but does not speak to the stability or
  frequency of Premium events. Future ETHUSDT LONG observations under comparable
  conditions are needed to determine whether `raw_score=47` is a stable Premium baseline
  or an outlier.

---

### E-05 — ETHUSDT SHORT, Good, raw_score=38

**Source:** Window 5. First ETHUSDT SHORT observation. All seven observable shadow
telemetry fields matched BTCUSDT SHORT (E-02) exactly, producing the first cross-symbol
corroboration in the Slice 006 evidence base.

**Raw telemetry (reference log line):**
```
DEBUG robsond::detector: stop-aware entry shadow telemetry
  position_id=019dee3c-59c2-7993-a7e7-34c3a4259df8
  symbol=ETHUSDT  side=Short
  stop_anchor_present=true  anchor_type=Some(SwingHigh)
  stop_quality_class=Good  raw_score=38  boost_pct=0.10
  shadow_exceptional=false
  technical_stop_method=SwingPoint { level_n: 1 }
  technical_stop_confidence=Medium  detected_levels_count=1
```

**Mirror record:**

| Field | Value | Derivation / justification |
|-------|-------|---------------------------|
| `mirror_version` | `v0.0-manual` | — |
| `source_signal_id` | `ETHUSDT_SHORT_2026-05-03_SH1_RS38` | — |
| `position_id` | `019dee3c-59c2-7993-a7e7-34c3a4259df8` | From telemetry |
| `symbol` | `ETHUSDT` | From telemetry |
| `side` | `Short` | From telemetry |
| `stop_quality_class` | `Good` | From telemetry |
| `raw_score` | `38` | From telemetry |
| `hypothetical_boost_pct` | `0.10` | From shadow metadata |
| `hypothetical_boost_cap` | `0.15` | Exceptional tier (0.20) disabled by mirror design (exceptional_ignored=true, see §2.5) |
| `real_decision` | `AwaitingApproval` | Confirmed in Window 5 §8: `entry_price=2322.69 USDT`, `stop_loss=2329.11 USDT`, query reached AwaitingApproval |
| `real_score` | `38` | No boost applied to real path |
| `boosted_score_shadow` | `41.80` | `38 × (1 + 0.10) = 41.80` |
| `mirrored_decision` | `AwaitingApproval` | Equals real_decision |
| `decision_delta` | `ScoreOnly` | Identical computation to E-02: score changes 38→41.80; Good tier retained under conservative assumption; no real gate crossed (see §2.6) |
| `delta_reason` | "10% boost raises raw_score from 38 to 41.80. Mirror record is identical to BTCUSDT SHORT (E-02), confirming the cross-symbol symmetry from Slice 006 Window 5. The Good/Premium boundary ambiguity from E-02 applies equally: if the boundary is ≤ 41.80, decision_delta would be ClassificationChanged — but this cannot be confirmed without exact threshold data. Conservative value: ScoreOnly. Real outcomes are all unchanged regardless of classification: real_decision=AwaitingApproval, approval gate unchanged, sizing unchanged, monthly slot consumption unchanged, execution unchanged. boosted_score_shadow=41.80 is recorded only as a shadow mirror observation." | Manual analysis; threshold ambiguity inherited from E-02 |
| `threshold_crossed_shadow` | `false` | Same conservative assumption as E-02 |
| `risk_engine_unchanged` | `true` | — |
| `entry_policy_unchanged` | `true` | — |
| `approval_required_unchanged` | `true` | — |
| `sizing_unchanged` | `true` | — |
| `monthly_slots_unchanged` | `true` | — |
| `execution_unchanged` | `true` | — |
| `exceptional_ignored` | `true` | Exceptional disabled by mirror design (architectural rule, see §2.5). Observed: `shadow_exceptional=false` |

**Notes:**
- E-05 produces a mirror record that is field-for-field identical to E-02 (different
  `position_id` and `source_signal_id`; all computed values identical). This is a direct
  consequence of the cross-symbol SHORT corroboration documented in Window 5.
- The ADR-0023 symbol-agnostic policy invariant is reflected at the mirror level: identical
  signal characteristics produce identical mirror output regardless of symbol. The mirror
  design correctly inherits this property without any symbol-specific logic.
- All threshold-ambiguity notes from E-02 apply without modification.

---

## 4. Summary Table

| ID | source_signal_id | quality | raw_score | boost_pct | boosted_score_shadow | real_decision | mirrored_decision | decision_delta | threshold_crossed | risk_engine_unchanged |
|----|-----------------|---------|-----------|-----------|---------------------|--------------|------------------|----------------|------------------|-----------------------|
| E-01 | `BTCUSDT_LONG_2026-05-02_SL2_RS37` | Good | 37 | 0.10 | **40.70** | AwaitingApproval | AwaitingApproval | ScoreOnly | false | true |
| E-02 | `BTCUSDT_SHORT_2026-05-02_SH1_RS38` | Good | 38 | 0.10 | **41.80** | AwaitingApproval | AwaitingApproval | ScoreOnly | false† | true |
| E-03 | `BTCUSDT_SHORT_2026-05-02_SH1_RS0_NONE` | None | 0 | 0 | **0.00** | AwaitingApproval | AwaitingApproval | **None** | false | true |
| E-04 | `ETHUSDT_LONG_2026-05-03_SL2_RS47` | Premium | 47 | 0.15 | **54.05** | AwaitingApproval | AwaitingApproval | ScoreOnly | false | true |
| E-05 | `ETHUSDT_SHORT_2026-05-03_SH1_RS38` | Good | 38 | 0.10 | **41.80** | AwaitingApproval | AwaitingApproval | ScoreOnly | false† | true |

†Threshold ambiguity: if the Good/Premium boundary is ≤ 41.80, `threshold_crossed_shadow`
would be `true` and `decision_delta` would be `ClassificationChanged` for E-02 and E-05.
Exact threshold documentation is required to resolve this.

### Gate-invariant verification

| Flag | E-01 | E-02 | E-03 | E-04 | E-05 | Expected |
|------|------|------|------|------|------|---------|
| `risk_engine_unchanged` | true | true | true | true | true | always true |
| `entry_policy_unchanged` | true | true | true | true | true | always true |
| `approval_required_unchanged` | true | true | true | true | true | always true |
| `sizing_unchanged` | true | true | true | true | true | always true |
| `monthly_slots_unchanged` | true | true | true | true | true | always true |
| `execution_unchanged` | true | true | true | true | true | always true |
| `exceptional_ignored` | true | true | true | true | true | always true — Exceptional disabled by architecture in Slice 007 (see §2.5) |

All invariants hold across all five examples.

---

## 5. decision_delta Taxonomy Validation

The Slice 007 plan (§6, Step 3) defines four `decision_delta` values. This section
validates the taxonomy against all five worked examples.

| delta value | Definition | Observed in | Notes |
|-------------|------------|------------|-------|
| `None` | Score unchanged; classification unchanged; all gates unchanged | E-03 | Occurs when boost_pct=0 (None class) |
| `ScoreOnly` | Score changed; classification tier unchanged; real_decision, approval, sizing, monthly slots, and execution all unchanged; hypothetical score difference recorded as shadow-only observation (see §2.6) | E-01, E-02, E-04, E-05 | Dominant outcome in current evidence |
| `ClassificationChanged` | Boost changes the classification tier; real outcomes remain unchanged | Not observed | Possible for E-02/E-05 if Good/Premium boundary ≤ 41.80 |
| `ThresholdCrossed` | A real decision gate would have been crossed by the boosted score | Not observed | Architecturally unreachable: no gate is wired to raw_score in current design |

**Finding:** Four of five examples produce `ScoreOnly`; one produces `None`. The more
consequential values (`ClassificationChanged`, `ThresholdCrossed`) are not evidenced. This
outcome is consistent with the design intent:

1. `ClassificationChanged` is unobservable without knowing exact thresholds. The E-02/E-05
   boosted score of 41.80 sits in the uncharacterized boundary region, leaving this value
   theoretically reachable but empirically unconfirmed.
2. `ThresholdCrossed` is architecturally unreachable in the current implementation because
   no decision gate receives `raw_score` or `boosted_score_shadow` as input. Before this
   value becomes meaningful, the architecture must define what gate would respond to a
   boosted score — otherwise the taxonomy entry cannot be exercised.

---

## 6. Open Questions for Future Slices

The following questions were raised by the worked examples and should be resolved before
any boost-enabling slice:

| # | Question | Impact |
|---|----------|--------|
| OQ-1 | **Exact Good/Premium threshold.** Boundary is in (38, 47). A 10% boost on RS=38 yields 41.80 — within this range. | If boundary ≤ 41.80, E-02 and E-05 should report `ClassificationChanged`, not `ScoreOnly`. Exact threshold must be extracted from `StopQualityClassifier` source before worked examples can be finalized. |
| OQ-2 | **`real_decision` for BTCUSDT LONG quantity rejection.** E-01 assumes `AwaitingApproval`; the quantity step-size rejection is post-approval. If the quantity check is pre-approval (at the RiskEngine stage), `real_decision` would be `Rejected`. | Affects only E-01 field values; mirror gate invariants are unchanged in either interpretation. Requires tracing the quantity check location in `robsond`. |
| OQ-3 | **Premium stability.** E-04 is a single observation. Whether ETHUSDT LONG consistently produces `raw_score≥47` under similar conditions is unknown. | Affects how representative the E-04 mirror record is. Repeated observations needed before any boost authorization. |
| OQ-4 | **`ThresholdCrossed` gating.** No real gate currently responds to `raw_score` or `boosted_score_shadow`. | Before any boost-enabling slice, the architecture must define which gate (if any) would be triggered by a boosted score. Without this, `ThresholdCrossed` remains vacuously false. |
| OQ-5 | **`Weak` class behavior.** No `Weak` observations in evidence base. Boost behavior for Weak is uncharacterized. | The mirror taxonomy is complete for observed classes; the Weak case remains a design gap. |

---

## 7. Compliance with Slice 007 Design Gates

| Gate | Status |
|------|--------|
| G1 — Plan reviewed and accepted by operator | Awaiting operator acknowledgment |
| G2 — Worked examples from all Slice 006 events | ✅ This document (5 signal types, 6 telemetry events) |
| G3 — `decision_delta` taxonomy validated against worked examples | ✅ Section 5 of this document |
| G4 — Forbidden field list reviewed | ✅ Section 8 of this document |
| G5 — Storage strategy selected | Awaiting operator selection |
| G6–G9 | Gate future implementation slices; not addressed here |

---

## 8. Forbidden Field Verification

Per Section 8 of the Slice 007 plan, the following fields must never appear in mirror
output. Verified across all five examples:

| Forbidden field category | Present in any example? |
|-------------------------|------------------------|
| API keys or secrets | No |
| Exchange credentials (Binance key/secret) | No |
| Database URLs or connection strings | No |
| Tenant secrets beyond `position_id` | No |
| Full account balance or equity | No |
| Raw order payload (price, qty, client_order_id) | No |
| Human approval tokens | No |
| Fields that can replay or construct an order | No |
| `shadow_exceptional=true` used as active boost input | No (all `false`) |

All examples are clean. No forbidden fields appear in any mirror record.

---

*This document was produced as the Slice 007 worked examples artifact on branch
`stop-aware-shadow-testnet`. No runtime change was made. No code was modified. Boost was
not applied in any form. All invariants from Section 11 of the Slice 007 plan held across
all five examples.*
