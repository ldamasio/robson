# Stop-Aware Entry v4 — Slice 006 Calibration Summary

**Date:** 2026-05-03
**Status:** Slice 006 complete — evidence reviewed, calibration documented
**Scope:** Shadow Evidence Review / Calibration — observational only
**Branch:** `stop-aware-shadow-testnet`
**HEAD at writing:** `154158ea`

---

## 1. Executive Summary

Slice 006 set out to accumulate shadow telemetry across a broader-than-initial set of
symbols, sides, market conditions, and quality classes in order to characterize the
empirical behavior of the Stop-Aware Entry v4 shadow pipeline before any
decision-affecting use is considered.

Five observation windows were conducted across two symbols (BTCUSDT, ETHUSDT), two
sides (Long, Short), and multiple market sessions. A total of six distinct telemetry
events were collected, covering three `stop_quality_class` values (None, Good,
Premium), two anchor types (SwingLow, SwingHigh), two stop methods (SwingPoint
level_n=1, level_n=2), and two confidence levels (High, Medium).

**Slice 006 concludes:**

1. The shadow pipeline is structurally coherent and produces stable, well-formed
   telemetry across symbols and sides.
2. The evidence base is **not sufficient** to act on StopQuality for any
   decision-affecting purpose.
3. **No boost is authorized.** No production rollout is authorized.
4. The `Exceptional` flag must remain disabled. `shadow_exceptional=false` held
   across all observations.
5. `Premium` was observed once — it is a promising signal but a single observation
   is not a basis for decisioning.
6. `Weak` was never observed. `AtrFallback` was never observed.
7. The next recommended slice, if pursued, is **Shadow Decision-Mirror** design
   (a read-only simulation of what decisions StopQuality would have produced) — not
   boost application.

**This document does not authorize:**
- Boost in production or testnet
- Any change to RiskEngine, TechnicalStopDistance, StopQualityClassifier thresholds,
  DetectorSignal, or EventBus
- Any production exposure or capital change
- Frontend visualization of StopQuality
- Enabling the `Exceptional` flag anywhere

---

## 2. Evidence Sources

| Document | Windows | Symbol | Side |
|----------|---------|--------|------|
| `2026-05-02-stop-aware-entry-shadow-validation-report.md` | 1 (runs 1–2) | BTCUSDT | Long |
| `2026-05-02-stop-aware-shadow-window-2-s1-findings.md` | 2 | BTCUSDT | Short |
| `2026-05-02-stop-aware-shadow-window-3-findings.md` | 3 | BTCUSDT | Short (passive) |
| `2026-05-03-stop-aware-shadow-window-4-ethusdt-long-findings.md` | 4 | ETHUSDT | Long |
| `2026-05-03-stop-aware-shadow-window-5-ethusdt-short-findings.md` | 5 | ETHUSDT | Short |

All windows were conducted on `ghcr.io/rbxrobotica/robson-v2:sha-447aba4b` in namespace
`robson-testnet`. No code changes were made during or between windows.

---

## 3. Coverage Matrix

### Raw telemetry table

| position_id (prefix) | symbol | side | anchor_type | stop_quality_class | raw_score | method | confidence | levels |
|----------------------|--------|------|-------------|-------------------|-----------|--------|------------|--------|
| `019de650` | BTCUSDT | Long | SwingLow | Good | 37 | SwingPoint(2) | High | 2 |
| `019de67f` | BTCUSDT | Long | SwingLow | Good | 37 | SwingPoint(2) | High | 2 |
| `019de906` (early cycles) | BTCUSDT | Short | SwingHigh | Good | 38 | SwingPoint(1) | Medium | 1 |
| `019de906` (final cycle) | BTCUSDT | Short | SwingHigh | None | 0 | SwingPoint(1) | Medium | 1 |
| `019dec8d` | ETHUSDT | Long | SwingLow | Premium | 47 | SwingPoint(2) | High | 2 |
| `019dee3c` | ETHUSDT | Short | SwingHigh | Good | 38 | SwingPoint(1) | Medium | 1 |

### Dimensional coverage

| Dimension | Values observed | Values absent |
|-----------|----------------|---------------|
| Symbols | BTCUSDT, ETHUSDT | Other operated pairs |
| Sides | Long, Short | — |
| `stop_quality_class` | None, Good, Premium | **Weak** |
| `anchor_type` | SwingLow, SwingHigh | structural, `stop_anchor_present=false` |
| `technical_stop_method` | SwingPoint level_n=1, SwingPoint level_n=2 | structural variants, **AtrFallback** |
| `technical_stop_confidence` | High, Medium | Low |
| `shadow_exceptional` | false (all) | — |

---

## 4. StopQuality Distribution

| stop_quality_class | Count (events) | Positions | Notes |
|-------------------|----------------|-----------|-------|
| Good | 4 | `019de650`, `019de67f`, `019de906` (early), `019dee3c` | Dominant class in current evidence |
| None | 1 | `019de906` (final cycle) | Intra-position degradation from Good |
| Premium | 1 | `019dec8d` | Single observation — ETHUSDT LONG |
| Weak | 0 | — | Never observed |

**Findings:**

- `Good` is the most frequently observed class, appearing across both symbols, both
  sides, and multiple sessions.
- `None` was observed once as an intra-position degradation within the same
  `position_id` (`019de906`). The stop method, anchor type, and confidence were
  unchanged between Good and None cycles, indicating the scoring function can produce
  zero scores independent of detection quality.
- `Premium` was observed once, in ETHUSDT LONG, with `raw_score=47`. This is the
  highest score in the evidence base. A single observation is insufficient to
  characterize when Premium is expected or how stable it is.
- `Weak` was never observed. Its threshold boundaries and real-world trigger conditions
  remain unknown empirically.

### raw_score distribution

| raw_score | stop_quality_class | symbol | side |
|-----------|-------------------|--------|------|
| 0 | None | BTCUSDT | Short |
| 37 | Good | BTCUSDT | Long (×2) |
| 38 | Good | BTCUSDT | Short |
| 38 | Good | ETHUSDT | Short |
| 47 | Premium | ETHUSDT | Long |

The score 38 appears in both BTCUSDT SHORT and ETHUSDT SHORT — the first cross-symbol
score corroboration. Score 37 appears in both BTCUSDT LONG runs. Score 47 is an
outlier (Premium) only in ETHUSDT LONG.

---

## 5. Anchor / Side Behavior

A consistent pattern emerged across all non-None observations:

| Side | anchor_type | method | confidence | levels | raw_score range |
|------|-------------|--------|------------|--------|----------------|
| Long | SwingLow | SwingPoint level_n=2 | High | 2 | 37–47 |
| Short | SwingHigh | SwingPoint level_n=1 | Medium | 1 | 0–38 |

This pattern held across both symbols. The classifier consistently produces:
- Higher scores, more detected levels, and higher confidence for Long positions.
- Lower scores, fewer detected levels, and medium confidence for Short positions.

**Interpretation:** The difference is likely structural. Short positions anchor to
SwingHigh levels, which may be less abundant or less well-formed in the observed
testnet windows. The `level_n=1` (shallowest swing) available for Short positions
yields a single detected level vs. `level_n=2` for Long. This produces lower
confidence and raw score without indicating a classifier defect.

**Caution:** This pattern is based on few observations under a limited range of market
conditions. It should not be generalized until more data is collected across trending,
ranging, and high-volatility sessions.

---

## 6. Symbol Diversity Findings

### BTCUSDT vs ETHUSDT — same side, same quality class

| Field | BTCUSDT SHORT | ETHUSDT SHORT |
|-------|--------------|--------------|
| anchor_type | SwingHigh | SwingHigh |
| stop_quality_class | Good | Good |
| raw_score | 38 | 38 |
| boost_pct | 0.10 | 0.10 |
| method | SwingPoint(1) | SwingPoint(1) |
| confidence | Medium | Medium |
| levels | 1 | 1 |

All seven observable fields matched exactly between BTCUSDT SHORT and ETHUSDT SHORT.
This is the first cross-symbol corroboration in the evidence base.

### BTCUSDT LONG vs ETHUSDT LONG

| Field | BTCUSDT LONG | ETHUSDT LONG |
|-------|-------------|-------------|
| anchor_type | SwingLow | SwingLow |
| stop_quality_class | Good | **Premium** |
| raw_score | 37 | **47** |
| boost_pct | 0.10 | **0.15** |
| method | SwingPoint(2) | SwingPoint(2) |
| confidence | High | High |
| levels | 2 | 2 |

Long positions share the same structural characteristics (anchor type, method,
confidence, level count) but ETHUSDT LONG produced a higher score and different quality
class. This divergence may reflect different market conditions at the time of the
observation rather than a symbol-specific classifier behavior — one window per
(symbol, side) pair is insufficient to distinguish the two.

**Conclusion:** The symbol-agnostic policy invariant (ADR-0023) is supported by this
evidence. The pipeline applied identical logic across symbols without symbol-specific
code paths. The score divergence between BTCUSDT LONG and ETHUSDT LONG warrants
further observation but is not anomalous.

---

## 7. Safety Findings

Across all five windows and all telemetry observations:

| Safety gate | Result |
|-------------|--------|
| `shadow_exceptional=false` | ✅ Held in 6/6 telemetry events |
| No approval called | ✅ `POST /queries/:id/approve` never invoked |
| No order executed | ✅ No `place_order`, `filled`, or `executed` log lines |
| `active_positions=0` at cleanup | ✅ All windows |
| `pending_approvals=[]` at cleanup | ✅ All windows |
| `-2015` (API key error) absent | ✅ All windows |
| `UNTRACKED` position absent | ✅ Startup reconciliation clean in all windows |
| Panic absent | ✅ All windows |
| `boost_pct` applied | ✅ Never — observed in shadow metadata only |
| StopQuality influenced any decision | ✅ Never — shadow-only throughout |
| RiskEngine authority | ✅ Remained sole decision authority |

`boost_pct` values observed in telemetry (0, 0.10, 0.15) are shadow metadata
fields only. They appeared in log lines and are recorded here for calibration purposes.
They were not passed to RiskEngine, TechnicalStopDistance, position sizing, or any
approval path.

---

## 8. Non-Blocking Technical Debt

### 8.1 Missing projection handler: entry_approval_pending

Observed in Windows 2–5 (all windows with a SHORT or ETHUSDT LONG observation that
reached `AwaitingApproval`):

```
WARN: Failed to persist EntryApprovalPending audit event — approval continues
ERROR: Missing handler for event type entry_approval_pending (seq=4)
```

**Impact:** None on Slice 006 objectives. Shadow telemetry was emitted before this
path was reached. The `AwaitingApproval` state functioned correctly. No order was
executed. Disarm succeeded in all cases.

**Debt:** The `entry_approval_pending` event type lacks a projection handler in the
event-sourcing layer. This means the audit trail for approval-pending events is
incomplete in the projection. It is out of scope for Slice 006 and must be addressed
in a future implementation slice.

### 8.2 Insufficient candle data on first detector tick (Window 5)

Observed once in ETHUSDT SHORT (Window 5):

```
WARN: Insufficient candle data: need at least 100 candles, got 92
```

Appeared on the first tick, resolved on the second tick. Telemetry was emitted
normally. Transient and non-blocking. May be more frequent for ETHUSDT if testnet
historical data for ETH is less available than for BTC; worth monitoring passively.

---

## 9. Calibration Interpretation

### What the evidence supports

1. **Pipeline correctness:** The shadow telemetry pipeline is correctly wired. It
   emits structured, well-formed output within 1–2 seconds of arming across all
   observed (symbol, side) pairs.

2. **Reproducibility:** BTCUSDT LONG was observed across two independent runs
   (Windows 1–2) with identical telemetry, confirming stability under repeated
   conditions.

3. **Quality class variety:** Three of four quality classes (None, Good, Premium) were
   observed. The classifier is sensitive to both market conditions and side.

4. **Cross-symbol consistency for SHORT:** BTCUSDT SHORT and ETHUSDT SHORT produced
   identical classification output — the strongest structural finding in the evidence
   base, consistent with ADR-0023.

5. **Side-driven structural pattern:** Long and Short positions exhibit systematic
   differences in anchor type, method depth, confidence, and score that are consistent
   across both symbols.

### What the evidence does not support

1. **Boost decisions:** A single Premium observation, no Weak observations, and no
   AtrFallback observations are insufficient grounds for using StopQuality to influence
   sizing, stop distance, or approval.

2. **Stability of Premium:** `019dec8d` produced Premium in one window. Whether ETHUSDT
   LONG consistently produces Premium under similar conditions is unknown.

3. **False-positive rate:** No negative-control observations (operator retrospectively
   classifying a signal as poor) have been collected. The false-positive rate of the
   classifier is empirically unconstrained.

4. **AtrFallback invariant:** The invariant that AtrFallback must never emit a
   StopAnchor has not been empirically verified. No AtrFallback observation has
   occurred. This remains a theoretical guarantee from the implementation; Slice 006
   evidence does not confirm it.

5. **Market-condition range:** All observations were collected from discrete testnet
   sessions under a limited range of conditions. The classifier has not been observed
   under trending, ranging, or high-volatility regimes.

---

## 10. Remaining Evidence Gaps

In priority order:

| Gap | Why it matters | How to close |
|-----|---------------|-------------|
| `Weak` class unobserved | Cannot characterize the Good/Weak boundary | Passive observation; may emerge organically |
| `AtrFallback` / `stop_anchor_present=false` | Must confirm invariant that fallback never emits anchor | Passive observation in low-swing-data conditions |
| Negative-control evidence | False-positive rate unconstrained | Collect windows where operator assesses signal quality post-hoc |
| Market-condition diversity | Pattern may not hold in trending/ranging/volatile regimes | Extended passive observation across multiple sessions |
| ETHUSDT stability | Single window per (ETH, side) pair — cannot confirm stability | Repeat ETHUSDT observations across sessions |
| Long-form window | All windows are short (minutes); no multi-hour passive window | Leave pod armed for BTCUSDT/ETHUSDT over hours |

---

## 11. Gates Before Any Future Boost

The following gates must all be met before any boost (i.e., use of `boost_pct` to
influence stop distance or sizing) can be considered in any slice:

1. **`Weak` observed** — at least one observation of each quality class is required
   to characterize the full score distribution.
2. **`AtrFallback` empirically confirmed** — at least one `stop_anchor_present=false`
   observation confirming fallback behavior.
3. **Negative-control baseline established** — at least one session with post-hoc
   operator signal quality assessment.
4. **Premium stability confirmed** — at least three independent Premium observations
   across different sessions and market conditions.
5. **Shadow Decision-Mirror completed** — a read-only off-line simulation showing
   what decisions StopQuality would have produced on historical data, with operator
   review of false-positive and false-negative rates.
6. **ADR amendment** — any use of StopQuality for decisions requires an explicit ADR
   amendment with operator sign-off, independent of evidence sufficiency.
7. **`shadow_exceptional=false` maintained** — the Exceptional path must remain
   disabled until the Decision-Mirror analysis is complete and reviewed.

None of these gates are currently met.

---

## 12. Recommendation

Slice 006 is **evidence-complete for its stated scope**: it has collected multi-symbol,
multi-side shadow telemetry and produced a structured calibration summary. The
pipeline is sound. The evidence is coherent. The safety gates held in every window.

**What Slice 006 does not justify:**
- Boost application in any form
- Production rollout of StopQuality
- RiskEngine or TechnicalStopDistance changes
- Enabling `Exceptional`
- Frontend StopQuality display
- Any decisioning use of `stop_quality_class`, `raw_score`, or `boost_pct`

**Recommended path forward:**

The natural successor to Slice 006 is a **Shadow Decision-Mirror** slice — a
read-only, off-line simulation that replays the collected telemetry and answers:
*"If StopQuality had been used to gate or boost, what would have happened?"* This
simulation is computed off-line from existing logs and does not touch any runtime
path, approval flow, or order execution.

A Shadow Decision-Mirror slice would:
1. Define explicit hypothetical decision rules (e.g., "approve only if Good or
   Premium", "boost by `boost_pct` if Premium").
2. Apply those rules to the existing telemetry log.
3. Compute the number of decisions that would have changed, and in which direction.
4. Estimate false-positive rates using any available negative-control observations.
5. Report to the operator for review — without applying anything.

Only after the Decision-Mirror analysis and operator review should a boost-enabling
slice be planned. That slice would also require an ADR amendment.

**In the interim:** continue passive observation to accumulate `Weak` and `AtrFallback`
evidence organically. No further controlled stimuli are required to advance Slice 006;
it is complete.
