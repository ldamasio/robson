# Stop-Aware Entry v4 — Shadow Observation Window 3 (Passive/Read-Only)

**Date:** 2026-05-02
**Status:** Window 3 completed — passive observation, no new stimulus, no new position
**Scope:** Observational — read-only passive telemetry collection across time gap

---

## 1. Summary

Window 3 was a **passive, read-only observation** conducted after a time gap following
Window 2. No stimulus was applied, no new position was created, and no operator action
was taken beyond the pre-existing disarm of position `019de906`.

The observation confirmed that the testnet environment remained healthy, that no new
DetectorTask was generated organically by market activity, and that the three known
positions remained the sole evidence base. However, position `019de906` (SHORT) produced
additional telemetry through a cycling pattern (repeated signal → approval → expiry)
that yielded a new calibration finding: **intra-position StopQuality degradation from
`Good` to `None`** within the same position lifecycle.

This is the first `StopQuality=None` observation in the Slice 006 evidence base.

**This document does not authorize:**
- Boost in production or testnet
- Changes to RiskEngine
- Changes to TechnicalStopDistance
- Changes to StopQualityClassifier thresholds
- Any production exposure or capital change
- Any new stimulus or position creation

**StopQuality remains shadow-only.** `boost_pct` was observed in telemetry metadata,
not applied to any sizing or risk calculation. `shadow_exceptional` remains `false`.
RiskEngine remains the final authority on stop distance, position sizing, and risk
parameters.

---

## 2. Runtime Under Observation

| Field | Value |
|-------|-------|
| Image | `ghcr.io/rbxrobotica/robson-v2:sha-447aba4b` |
| Pod | `robsond-6b65c96c54-smbdn` |
| Namespace | `robson-testnet` |
| ArgoCD | Synced / Healthy |
| ArgoCD revision | `e1e0f72a` (manifest digest) |
| RUST_LOG | `robsond=debug,robson_engine=debug,robson_exec=debug,robson_store=debug` |
| Pod uptime | ~15h |
| Restarts | 0 |

---

## 3. Observation Method

Window 3 was entirely **passive**:
- No `POST /positions` was called
- No `DELETE /positions` was called
- No `POST /queries/:id/approve` was called
- No boost was applied
- No approval was triggered
- No order was executed
- No frontend was started
- No code, Kubernetes, GitOps, or DB changes were made

The observation consisted of reading pod logs across multiple time windows
(3h, 6h, 12h, 24h) and filtering for `stop-aware entry shadow telemetry` lines.

---

## 4. Telemetry Windows

| Window | Lines | Position IDs | Detail |
|--------|-------|-------------|--------|
| 3h | 5 | `019de906` only | SHORT cycling 14:10–14:30 UTC |
| 6h | 5 | `019de906` only | Same 5 lines |
| 12h | 5 | `019de906` only | Same 5 lines |
| 24h | 21 | `019de650`, `019de67f`, `019de906` | Full evidence base |

**24h breakdown:**

| Position | Side | Anchor | Count | Timestamps |
|----------|------|--------|-------|------------|
| `019de650` | Long | SwingLow | 8 | 01:31:45–01:31:50 UTC |
| `019de67f` | Long | SwingLow | 8 | 02:23:29–02:23:35 UTC |
| `019de906` | Short | SwingHigh | 5 | 14:10:41–14:30:44 UTC |

**No new position IDs** were detected in any window. Filtering out the three known
positions produced `NO_NEW_TELEMETRY_EXCLUDING_KNOWN_POSITIONS`.

---

## 5. New Calibration Finding: Intra-Position StopQuality Degradation

Position `019de906` (BTCUSDT SHORT) cycled through the detector signal → approval
path multiple times at ~5-minute intervals. This cycling produced telemetry at each
iteration with consistent parameters — except the final cycle.

### Early cycles (14:10–14:25 UTC)

```
stop_quality_class=Good
raw_score=38
boost_pct=0.10
shadow_exceptional=false
technical_stop_method=SwingPoint { level_n: 1 }
technical_stop_confidence=Medium
detected_levels_count=1
anchor_type=Some(SwingHigh)
stop_anchor_present=true
```

### Final cycle (14:30:44 UTC)

```
stop_quality_class=None
raw_score=0
boost_pct=0
shadow_exceptional=false
technical_stop_method=SwingPoint { level_n: 1 }
technical_stop_confidence=Medium
detected_levels_count=1
anchor_type=Some(SwingHigh)
stop_anchor_present=true
```

### Significance

- **First `StopQuality=None` observation** in the Slice 006 evidence base.
- Degradation occurred within the same position_id, same symbol, same side, same anchor
  type, same stop method — differing only in `stop_quality_class`, `raw_score`, and
  `boost_pct`.
- `raw_score` dropped from `38` to `0`, and `boost_pct` from `0.10` to `0`.
- The stop method, confidence, anchor type, and detected levels remained unchanged,
  indicating the degradation is in the quality scoring logic specifically, not in swing
  detection or anchor identification.
- This suggests the scoring function can produce `None` (or map a zero-score to `None`)
  when market conditions or internal state shift between detector cycles — even without
  any external stimulus.

### Calibration implication

The StopQuality classifier must handle the `None` class gracefully when promoted from
shadow to active. Current evidence:

| StopQuality | Observations | Position IDs |
|-------------|-------------|-------------|
| Good | 20 telemetry lines | `019de650`, `019de67f`, `019de906` |
| None | 1 telemetry line | `019de906` (final cycle) |
| Weak | 0 | — |
| Premium | 0 | — |

---

## 6. Safety Outcome

- **No approval was called** for any cycle of `019de906`.
- **No order was executed** at any point.
- **No boost was applied** — `boost_pct` appeared only in shadow telemetry metadata.
- **No `-2015` errors** (API key / permission) were detected.
- **No `UNTRACKED` position** events were detected.
- **No panics** were detected.
- **No credential errors** were detected.
- **StopQuality remains shadow-only** — the `None` classification did not influence any
  risk parameter, position sizing, or stop distance.
- **RiskEngine remains the final authority** on stop distance, sizing, and risk.

---

## 7. Query / Approval Path Observation

The cycling behavior of `019de906` revealed a recurring pattern in the QueryEngine:

```
ProcessSignal → Accepted → Processing → RiskChecked → AwaitingApproval → Expired
```

Each cycle:
1. Detector received market data for `019de906` (price varied: 78295.20, 78332.90, 78370.80)
2. Signal emitted with `stop_loss` recalculated per price
3. Query transitioned through RiskChecked → AwaitingApproval
4. Approval TTL expired (~5 min)
5. Query transitioned to Expired
6. Next detector cycle generated a new signal and query
7. Process repeated until operator disarm

**Final cycle disarm sequence (14:33:47 UTC):**

| Time | Event |
|------|-------|
| 14:30:44 | Last telemetry emitted (`None` quality) |
| 14:30:44 | Signal accepted, query `019de919` → AwaitingApproval |
| 14:33:47 | Operator issued DisarmPosition (query `019de91c`) |
| 14:33:47 | Pending approval invalidated: "position disarmed" |
| 14:33:47 | Query `019de919` → Failed |
| 14:33:48 | Position state: Armed → **Cancelled** |

Since disarm: **no further telemetry, no query transitions, no detector activity** for
`019de906`.

---

## 8. Projection Warning: entry_approval_pending

A recurring WARN/ERROR was observed throughout the cycling period:

```
WARN robsond::position_manager: Failed to persist EntryApprovalPending audit event
  — approval continues position_id=019de906 error=EventLog error: Failed to apply
  entry_approval_pending to projection (...): Missing handler for event type
  entry_approval_pending (seq N, stream=position:019de906-...)

ERROR robsond::daemon: Error handling event error=EventLog error: Failed to apply
  entry_approval_pending to projection (...): Missing handler for event type
  entry_approval_pending (seq N, stream=position:019de906-...)
```

**Characteristics:**
- Seq numbers incremented: 4 → 6 → 8 → 10 → 12 (one per cycle)
- **Non-blocking**: the WARN explicitly states "approval continues"
- **Did not block telemetry** — shadow telemetry was emitted normally
- **Did not block AwaitingApproval** — the query path completed each cycle
- **Did not execute any order** — no order was placed
- Root cause: the projection handler for `entry_approval_pending` event type is
  not registered, so the eventlog cannot apply the event to the projection

**Assessment:** This is a code defect in the eventlog/projection layer that should be
tracked as a technical issue or future slice. It does not block Slice 006 calibration
provided that:
- Recovery/projection consistency remains intact (no data corruption observed)
- The missing handler only affects audit persistence, not operational path
- The event is still written to the eventlog (it fails on projection application, not
  on event emission)

**Recommendation:** Create a tracking issue for the missing `entry_approval_pending`
projection handler. Do not fix in Slice 006 — address in a dedicated slice to avoid
scope creep.

---

## 9. Updated Evidence Coverage

### By position

| Position | Symbol | Side | Anchor | Quality | Method | Confidence | Levels |
|----------|--------|------|--------|---------|--------|------------|--------|
| `019de650` | BTCUSDT | Long | SwingLow | Good | SwingPoint(2) | High | 2 |
| `019de67f` | BTCUSDT | Long | SwingLow | Good | SwingPoint(2) | High | 2 |
| `019de906` | BTCUSDT | Short | SwingHigh | Good→None | SwingPoint(1) | Medium | 1 |

### Coverage matrix

| Dimension | Covered | Missing |
|-----------|---------|---------|
| Symbol | BTCUSDT | ETHUSDT, other pairs |
| Side | Long + Short | — |
| Anchor | SwingLow + SwingHigh | — |
| StopQuality | Good + None | Weak, Premium |
| Confidence | High + Medium | Low |
| Stop method | SwingPoint(n=1, n=2) | AtrFallback |
| Anchor absent | — | `stop_anchor_present=false` |
| Market condition | Single-regime BTCUSDT | Volatility/diversity |

### Evidence gaps (unchanged from Window 2)

1. **Symbol diversity** — only BTCUSDT observed; need at least one more pair (e.g.
   ETHUSDT via ConfigMap/GitOps) to validate symbol-agnostic invariant
2. **Weak/Premium quality** — no observations; may require different market conditions
   or synthetic test cases
3. **AtrFallback** — no observations; stop method has been exclusively SwingPoint
4. **No-anchor case** — `stop_anchor_present=false` has not been observed; may occur
   when no swing point is detected within the analysis window
5. **Market-condition diversity** — all observations within a narrow BTCUSDT range
   (~78295–78370); broader volatility regimes would strengthen calibration

---

## 10. Recommendation

### Immediate (within Slice 006 scope)

1. **Record the `None` degradation** as a calibration data point — the classifier
   can produce `None` organically, and the active path must handle this case
2. **Close Window 3** — no further passive observation needed; cycling has stopped
   and no new organic signals are expected without stimulus

### Next steps (separate sessions)

3. **Symbol diversity stimulus** — plan an ETHUSDT (or other pair) controlled stimulus
   via ConfigMap/GitOps to expand evidence beyond BTCUSDT
4. **Track `entry_approval_pending` projection bug** — create a tracking issue for
   the missing projection handler; do not fix in Slice 006
5. **Consider a longer observation window** (24–48h) if organic signal diversity is
   desired without manual stimulus, but this is lower priority than explicit symbol
   diversity testing

### Not recommended

- Do not promote StopQuality from shadow to active based on current evidence
- Do not change RiskEngine parameters based on shadow observations
- Do not modify TechnicalStopDistance or StopQualityClassifier thresholds
