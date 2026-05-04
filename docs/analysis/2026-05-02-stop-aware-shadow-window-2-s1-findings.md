# Stop-Aware Entry v4 — Shadow Observation Window 2, S1 (BTCUSDT SHORT)

**Date:** 2026-05-02
**Status:** S1 completed — evidence recorded, no production authorization
**Scope:** Observational — one controlled testnet stimulus, shadow telemetry only

---

## 1. Summary

A single controlled stimulus was executed on the Binance testnet to observe
Stop-Aware Entry v4 shadow telemetry for **BTCUSDT SHORT** — the first SHORT
observation in the Slice 006 evidence base. The stimulus produced shadow
telemetry with `anchor_type=SwingHigh`, `raw_score=38`, `confidence=Medium`,
and `SwingPoint { level_n: 1 }`, adding side diversity, anchor diversity,
method diversity, and confidence diversity to the evidence accumulated so far.

The position was armed with `entry_policy.mode=immediate` and
`approval=human_confirmation`. The query reached `AwaitingApproval` and
expired by TTL without any approval being called, any order being executed,
or any boost being applied. The position was then deleted via cleanup.

**This document does not authorize:**
- Boost in production or testnet
- Changes to RiskEngine
- Changes to TechnicalStopDistance
- Changes to StopQualityClassifier thresholds
- Any production exposure or capital change

**StopQuality remains shadow-only.** `boost_pct` was observed in telemetry
metadata, not applied to any sizing or risk calculation. `shadow_exceptional`
remains `false`. RiskEngine remains the final authority on stop distance,
position sizing, and risk parameters.

---

## 2. Runtime Under Observation

| Field | Value |
|-------|-------|
| Image | `ghcr.io/rbxrobotica/robson-v2:sha-447aba4b` |
| Pod | `robsond-6b65c96c54-smbdn` |
| Namespace | `robson-testnet` |
| ArgoCD | Synced / Healthy |
| RUST_LOG | `robsond=debug,robson_engine=debug,robson_exec=debug,robson_store=debug` |
| Symbol | BTCUSDT |
| Side | SHORT |
| Entry policy | `immediate` + `human_confirmation` |
| Shadow mode | Enabled (no boost applied, no approval called, no order executed) |

---

## 3. Stimulus Definition

**Stimulus S1:**

```
POST /positions
{
  "symbol": "BTCUSDT",
  "side": "SHORT",
  "entry_policy": {
    "mode": "immediate",
    "approval": "human_confirmation"
  }
}
```

**Response:**

```json
{
  "position_id": "019de906-f5d8-73c3-937c-6bdfcf9d3cca",
  "symbol": "BTCUSDT",
  "side": "Short",
  "state": "Armed"
}
```

**Pre-conditions confirmed:**
- Image `sha-447aba4b` active on testnet pod
- `active_positions=0`, `pending_approvals=[]`
- No `-2015` errors, no `UNTRACKED` positions
- Clean recovery on startup

---

## 4. Observed Telemetry

First shadow telemetry line (after ~2s):

```
DEBUG robsond::detector: stop-aware entry shadow telemetry
  position_id=019de906-f5d8-73c3-937c-6bdfcf9d3cca
  symbol=BTCUSDT
  side=Short
  stop_anchor_present=true
  anchor_type=Some(SwingHigh)
  stop_quality_class=Good
  raw_score=38
  boost_pct=0.10
  shadow_exceptional=false
  technical_stop_method=SwingPoint { level_n: 1 }
  technical_stop_confidence=Medium
  detected_levels_count=1
```

Key observations:

- **Stop anchor** resolved to `SwingHigh` — the SHORT-side anchor, distinct from
  LONG's `SwingLow`. Confirms the `Side → AnchorType` mapping in
  `detector.rs:635` works correctly.
- **Stop quality** classified as `Good` with `raw_score=38` — same class as LONG
  but score differs by 1, indicating the classifier has sensitivity to market
  structure changes.
- **Technical stop method** is `SwingPoint { level_n: 1 }` — first swing level,
  compared to LONG's `level_n: 2`. Fewer detected levels (1 vs 2).
- **Confidence** is `Medium` — lower than LONG's `High`, correlating with the
  lower `detected_levels_count`.
- **Shadow exceptional** is `false` — no boost escalation triggered.
- **Boost pct** `0.10` is the default parameter, observed in shadow metadata
  only. It was **not applied** to any sizing or risk calculation.

---

## 5. Comparison Against Prior LONG Validation

| Dimension | LONG (Run 1 & 2) | SHORT (S1) | Delta |
|-----------|-------------------|------------|-------|
| side | Long | **Short** | new |
| anchor_type | SwingLow | **SwingHigh** | new |
| raw_score | 37 | **38** | +1 |
| stop_quality_class | Good | Good | same |
| technical_stop_method | SwingPoint { level_n: 2 } | **SwingPoint { level_n: 1 }** | new |
| technical_stop_confidence | High | **Medium** | new |
| detected_levels_count | 2 | **1** | new |
| boost_pct | 0.10 | 0.10 | same |
| shadow_exceptional | false | false | same |
| stop_anchor_present | true | true | same |

**Interpretation:**

- The SHORT side produces a structurally different anchor (`SwingHigh` vs
  `SwingLow`), confirming side-awareness in the stop anchor builder.
- `raw_score` varies (38 vs 37) even within the same class (`Good`), showing
  the classifier is not trivially constant.
- Fewer detected swing levels (1 vs 2) correlates with lower confidence
  (`Medium` vs `High`), consistent with the heuristic design.
- `stop_quality_class=Good` is stable across both sides, but the sample is too
  small to draw distributional conclusions.

---

## 6. Safety Outcome

| Check | S1 Result |
|-------|-----------|
| No approval called | Confirmed |
| No order executed | Confirmed |
| No boost applied | Confirmed (`shadow_exceptional=false`) |
| Position deleted | Confirmed via DELETE |
| Final `active_positions` | 0 |
| Final `pending_approvals` | `[]` |
| No `-2015` errors | Confirmed |
| No `UNTRACKED` positions | Confirmed |
| RiskEngine unchanged | Confirmed — no code changes to risk logic |
| TechnicalStopDistance unchanged | Confirmed — no code changes |
| StopQuality shadow-only | Confirmed — classifier output used for telemetry only |

**Cleanup:** `DELETE /positions/019de906-f5d8-73c3-937c-6bdfcf9d3cca` executed
successfully. Post-cleanup status confirmed `active_positions=0`,
`pending_approvals=[]`.

---

## 7. Query / Approval Path Observation

Unlike the LONG validation runs (where the quantity was rejected by Binance's
step size before reaching the approval gate), the SHORT stimulus produced a
quantity of `0.00476 BTC` — above the `0.001` minimum step size — and the query
progressed through the **full lifecycle**:

```
Accepted → Processing → RiskChecked → AwaitingApproval
```

The query entered `AwaitingApproval` at 14:25:43 UTC with a 5-minute TTL
(expiring at 14:30:43 UTC). No approval was called. The TTL expired, the query
transitioned to `Expired`, and the detector re-armed automatically.

This observation validates that:

1. `human_confirmation` acts as a hard gate — no order is placed without
   explicit operator approval.
2. The approval TTL mechanism works correctly — expired queries do not linger.
3. Detector re-arm after approval expiry is functional.

**No order was executed at any point.** The query never reached the execution
phase.

---

## 8. New Evidence Added to Slice 006

S1 added the following dimensions not previously observed:

| Dimension | Previous | After S1 |
|-----------|----------|----------|
| Side diversity | Long only | **Long + Short** |
| Anchor diversity | SwingLow only | **SwingLow + SwingHigh** |
| Method diversity | SwingPoint n:2 only | **SwingPoint n:1 + n:2** |
| Confidence diversity | High only | **High + Medium** |
| raw_score range | 37 only | **37–38** |
| Full query lifecycle | Partial (rejected at quantity) | **Complete to AwaitingApproval** |

---

## 9. Non-Blocking Findings

### 9.1 `entry_approval_pending` Projection Handler Missing

**Observation:** When the query reached `AwaitingApproval`, the event
`entry_approval_pending` was emitted to the eventlog but the projection worker
warned:

```
WARN robsond::position_manager: Failed to persist EntryApprovalPending audit
event — approval continues
  error=Missing handler for event type entry_approval_pending
```

**Impact:** The event is persisted to the eventlog (append-only, no data loss).
The projection cannot apply it, so the projection state does not reflect the
approval-pending status. Approval flow continues unaffected.

**Classification:** Non-blocking for Slice 006. Same pattern as the
`entry_policy_resolved` handler gap fixed in commit `a5e962ea`. Treat as a
projector/event-sourcing debt item for a future slice.

**Condition:** Only triggers when `approval=human_confirmation` and the query
reaches `AwaitingApproval`. Does not affect `automatic` approval or shadow
telemetry.

### 9.2 Detector Re-Arm After Approval Expiry

**Observation:** After the first query expired (5min TTL), the detector re-armed
automatically and produced a second cycle of shadow telemetry with identical
values. This is expected behavior but worth noting: without manual DELETE, the
detector will continue producing telemetry cycles at approval-TTL intervals.

**Impact:** None for evidence quality (telemetry is identical). Could produce
log noise in extended observation windows.

---

## 10. Updated Evidence Coverage

Cumulative Slice 006 evidence (LONG validation runs + S1 SHORT):

| Dimension | Coverage |
|-----------|----------|
| Symbols | 1 (BTCUSDT) |
| Sides | 2 (Long + Short) |
| `stop_quality_class` values | 1 (`Good`) |
| `anchor_type` values | 2 (`SwingLow`, `SwingHigh`) |
| `raw_score` values | 2 (37, 38) |
| `technical_stop_method` values | 2 (`SwingPoint { level_n: 1 }`, `SwingPoint { level_n: 2 }`) |
| `technical_stop_confidence` values | 2 (`High`, `Medium`) |
| `shadow_exceptional` | 1 (always `false`) |
| `boost_pct` | 1 (`0.10`) |
| `AtrFallback` observations | 0 |
| `stop_anchor_present=false` | 0 |
| `StopQuality` None / Weak / Premium | 0 |
| Market-condition diversity | Low (single session, single market regime) |
| Symbol diversity | Low (BTCUSDT only — ETHUSDT not in testnet config) |
| Independent stimuli | 3 (2 LONG + 1 SHORT) |

---

## 11. Recommendation

**Evidence gaps remaining:**

1. **Symbol diversity** — ETHUSDT requires ConfigMap change (`ROBSON_MARKET_DATA_SYMBOLS`,
   `ROBSON_POSITION_MONITOR_SYMBOLS`). Out of scope for Slice 006 unless operator
   authorizes GitOps change.
2. **StopQuality class diversity** — Only `Good` observed. `None`, `Weak`, and
   `Premium` require different market conditions or symbol structures.
3. **AtrFallback / no-anchor case** — Requires market conditions where no swing
   levels are detected within the lookback window. May occur naturally over
   longer observation windows.
4. **Market-condition diversity** — All observations are from a single BTCUSDT
   session. Trending, ranging, and high-volatility regimes may produce
   different classifications.
5. **`entry_approval_pending` projection handler** — Should be addressed in a
   future slice, but does not block Slice 006 evidence collection.

**Next steps:**

- Consider a **Window 3** passive observation after a gap (hours/days) to capture
  different market conditions for BTCUSDT.
- Consider whether adding ETHUSDT to the testnet ConfigMap is justified for
  symbol diversity.
- Do **not** re-stimulate BTCUSDT LONG — it would not add coverage.
- Do **not** apply boost, change RiskEngine, or alter StopQualityClassifier
  thresholds based on this evidence.
