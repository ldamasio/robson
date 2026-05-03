# Stop-Aware Entry v4 — Shadow Observation Window 4 (ETHUSDT LONG)

**Date:** 2026-05-03
**Status:** Window 4 completed — controlled stimulus, observational only
**Scope:** Symbol diversity expansion — first ETHUSDT observation for Slice 006

---

## 1. Summary

Window 4 was a **controlled, bounded stimulus** designed to add symbol diversity to
the Slice 006 evidence base. Prior windows (1–3) produced exclusively BTCUSDT
evidence. Window 4 introduced ETHUSDT LONG as a second symbol, confirming that the
Stop-Aware Entry v4 shadow pipeline operates correctly under a different trading pair
and produces the first `Premium` quality classification in the Slice 006 evidence
base.

Key findings:

- Shadow telemetry appeared within **~1.6s** of arming — fastest time-to-first-telemetry
  observed across all windows.
- `stop_quality_class=Premium` — **first Premium observation in Slice 006.**
- `raw_score=47` — **highest raw score observed to date** (prior maximum: 38).
- `boost_pct=0.15` was recorded in shadow telemetry metadata; it was **not applied**
  to any sizing, risk, or approval calculation.
- `shadow_exceptional=false` — gate held.
- No approval was called. No order was executed. The query reached `AwaitingApproval`
  and expired by TTL.
- ETHUSDT confirmed the multi-symbol ConfigMap (`BTCUSDT,ETHUSDT`) is correctly parsed
  and routed through the WebSocket and position monitor.

**This document does not authorize:**
- Boost in production or testnet
- Changes to RiskEngine, TechnicalStopDistance, or StopQualityClassifier
- Any production exposure or capital change
- Any further stimulus without explicit operator authorization

**StopQuality remains shadow-only.** RiskEngine remains the final authority on stop
distance, position sizing, and risk parameters.

---

## 2. Runtime Under Observation

| Field | Value |
|-------|-------|
| Image | `ghcr.io/rbxrobotica/robson-v2:sha-447aba4b` |
| Pod | `robsond-5455c6c49d-s6stj` |
| Namespace | `robson-testnet` |
| `ROBSON_MARKET_DATA_SYMBOLS` | `BTCUSDT,ETHUSDT` |
| `ROBSON_POSITION_MONITOR_SYMBOLS` | `BTCUSDT,ETHUSDT` |
| `RUST_LOG` | `robsond=debug,robson_engine=debug,robson_exec=debug,robson_store=debug` |
| Pod uptime at stimulus | ~4 min (post-rollout-restart from PR #5) |
| Restarts | 0 |
| ArgoCD | Synced / Healthy — revision `065fe2a` |

The pod was freshly restarted after merging rbx-infra PR #5
(`chore(testnet): add ETHUSDT to robson symbols`), which updated the ConfigMap to
include ETHUSDT in both `ROBSON_MARKET_DATA_SYMBOLS` and
`ROBSON_POSITION_MONITOR_SYMBOLS`. Both WebSocket streams (BTCUSDT and ETHUSDT)
connected and received first ticks prior to the stimulus.

---

## 3. Stimulus Definition

| Field | Value |
|-------|-------|
| Type | Controlled bounded stimulus |
| Symbol | `ETHUSDT` |
| Side | `LONG` |
| Entry policy mode | `immediate` |
| Entry policy approval | `human_confirmation` |
| Position ID | `019dec8d-0cc3-7201-830d-a6b03c65804c` |
| Observation window | up to 75s |
| Boost applied | No |
| Approval called | No |
| Order executed | No |

**Operator actions performed:**
1. `POST /positions` — arm ETHUSDT LONG
2. `DELETE /positions/019dec8d-0cc3-7201-830d-a6b03c65804c` — disarm after telemetry
   captured

No other operator actions were taken.

---

## 4. Observed Telemetry

Shadow telemetry line appeared at `2026-05-03T06:36:01.614801Z`, approximately **1.6s**
after the arm request:

```
DEBUG robsond::detector: stop-aware entry shadow telemetry
  position_id=019dec8d-0cc3-7201-830d-a6b03c65804c
  symbol=ETHUSDT
  side=Long
  stop_anchor_present=true
  anchor_type=Some(SwingLow)
  stop_quality_class=Premium
  raw_score=47
  boost_pct=0.15
  shadow_exceptional=false
  technical_stop_method=SwingPoint { level_n: 2 }
  technical_stop_confidence=High
  detected_levels_count=2
```

| Field | Value | Notes |
|-------|-------|-------|
| `symbol` | `ETHUSDT` | First non-BTCUSDT observation |
| `side` | `Long` | |
| `stop_anchor_present` | `true` | Structural anchor found |
| `anchor_type` | `Some(SwingLow)` | Same type as BTCUSDT LONG observations |
| `stop_quality_class` | `Premium` | **First Premium in Slice 006** |
| `raw_score` | `47` | **Highest raw score observed to date** |
| `boost_pct` | `0.15` | Recorded in shadow metadata only; not applied |
| `shadow_exceptional` | `false` | Gate held — no exceptional path triggered |
| `technical_stop_method` | `SwingPoint { level_n: 2 }` | Consistent with BTCUSDT LONGs |
| `technical_stop_confidence` | `High` | |
| `detected_levels_count` | `2` | Consistent with BTCUSDT LONGs |

---

## 5. Comparison Against Prior BTCUSDT Evidence

| Dimension | BTCUSDT (Windows 1–3) | ETHUSDT (Window 4) |
|-----------|----------------------|--------------------|
| symbol | BTCUSDT | **ETHUSDT** |
| sides observed | Long, Short | Long |
| stop_quality_class | Good, None | **Premium** |
| raw_score range | 0, 37, 38 | **47** |
| anchor_type | SwingLow, SwingHigh | SwingLow |
| stop_anchor_present | true (Good), true (None†) | true |
| technical_stop_method | SwingPoint level_n=1, level_n=2 | SwingPoint level_n=2 |
| technical_stop_confidence | High, Medium | High |
| detected_levels_count | 1, 2 | 2 |
| boost_pct observed | 0.10, 0 | **0.15** |
| shadow_exceptional | false (all) | false |

† The `None` observation on `019de906` (Window 3) had `stop_anchor_present=true` and
`anchor_type=Some(SwingHigh)` — the quality degraded to `None` despite an anchor being
present, suggesting the scoring function depends on more than anchor presence alone.

**New coverage added by Window 4:**

- `stop_quality_class=Premium` — previously unobserved
- `raw_score=47` — highest in evidence base
- `boost_pct=0.15` — new value (prior: 0.10 and 0)
- Symbol: ETHUSDT — second symbol in evidence base
- Confirms `SwingPoint { level_n: 2 }` and `High` confidence on a different pair

---

## 6. Safety Outcome

- **No approval was called** — `POST /queries/:id/approve` was not invoked.
- **No order was executed** — no `place_order`, `filled`, or `executed` log lines.
- **No `-2015` errors** — API key / permission gates held.
- **No `UNTRACKED` positions** — startup reconciliation was clean; no rogue positions.
- **No panics** detected.
- **`shadow_exceptional=false`** — the Exceptional flag was not triggered.
- **`boost_pct=0.15`** appeared in shadow telemetry metadata only. It was not applied
  to stop distance, position sizing, or risk parameters.
- **StopQuality remains shadow-only** — the `Premium` classification did not influence
  any risk parameter, position sizing, or stop distance.
- **RiskEngine remains the final authority** on stop distance, sizing, and risk.
- **Pre- and post-status**: `active_positions=0`, `pending_approvals=[]` in both cases.

---

## 7. Query / Approval Path Observation

```
ProcessSignal → Accepted → Processing → RiskChecked → AwaitingApproval → (TTL expiry)
```

| Field | Value |
|-------|-------|
| `entry_price` | 2300.85 USDT |
| `stop_loss` | 2270.5550 USDT |
| Stop distance | ~1.32% |
| Quantity | ~0.033008 ETH |
| Notional | ~75.95 USDT |
| Risk check | Approved |
| Final query state | AwaitingApproval → expired (TTL 5 min) |

The query was never approved. The approval TTL elapsed and the query expired without
any operator or automated action. This is the expected behavior for
`human_confirmation` mode under controlled observation.

---

## 8. Projection Warning: entry_approval_pending

Two non-fatal log entries appeared during the observation window:

```
WARN robsond::position_manager: Failed to persist EntryApprovalPending audit event
  — approval continues
  position_id=019dec8d-0cc3-7201-830d-a6b03c65804c
  error=EventLog error: Failed to apply entry_approval_pending to projection
  (stream position:..., seq 4): Missing handler for event type entry_approval_pending

ERROR robsond::daemon: Error handling event
  error=EventLog error: Failed to apply entry_approval_pending to projection
  (stream position:..., seq 4): Missing handler for event type entry_approval_pending
```

**Assessment:** This warning is identical to the one observed in BTCUSDT SHORT
sessions. It reflects a missing projection handler for the `entry_approval_pending`
event type in the event-sourcing layer — an existing known deficiency, not a new
ETHUSDT-specific issue.

**Impact on this window:** None. The warning did not:
- Block shadow telemetry emission
- Block the `AwaitingApproval` transition
- Trigger any approval or order
- Prevent the disarm/cleanup
- Affect `active_positions` or `pending_approvals` counts

**Action:** This is a known projection/event-sourcing debt. It is explicitly out of
scope for Slice 006 (which is observational). A future implementation slice should add
the missing projection handler for `entry_approval_pending`. This document records the
deficiency but does not authorize any code change.

---

## 9. Updated Slice 006 Evidence Coverage

### Cumulative telemetry table

| position_id | symbol | side | anchor_type | stop_quality_class | raw_score | method | confidence | levels |
|-------------|--------|------|-------------|-------------------|-----------|--------|------------|--------|
| `019de650` | BTCUSDT | Long | SwingLow | Good | 37 | SwingPoint(2) | High | 2 |
| `019de67f` | BTCUSDT | Long | SwingLow | Good | 37 | SwingPoint(2) | High | 2 |
| `019de906` (early) | BTCUSDT | Short | SwingHigh | Good | 38 | SwingPoint(1) | Medium | 1 |
| `019de906` (final) | BTCUSDT | Short | SwingHigh | None | 0 | SwingPoint(1) | Medium | 1 |
| `019dec8d` | **ETHUSDT** | Long | SwingLow | **Premium** | **47** | SwingPoint(2) | High | 2 |

### Coverage matrix (updated)

| Dimension | Values observed | Values still missing |
|-----------|----------------|----------------------|
| Symbols | BTCUSDT, **ETHUSDT** | Other operated pairs |
| Sides | Long, Short | — (both covered) |
| `stop_quality_class` | None, Good, **Premium** | **Weak** |
| `anchor_type` | SwingLow, SwingHigh | structural, `stop_anchor_present=false` |
| `technical_stop_method` | SwingPoint level_n=1, level_n=2 | structural variants, **AtrFallback** |
| `technical_stop_confidence` | High, Medium | Low (if emitted) |
| `shadow_exceptional` | false | — (must remain false) |
| Market conditions | single-session testnet | trending, ranging, high-volatility |
| Repetitions per symbol/side | 1–8 per position | more per symbol to confirm stability |

### Evidence still missing for Slice 006 completion

- **`Weak` quality class** — not yet observed on any symbol or side.
- **`AtrFallback` / `stop_anchor_present=false`** case — required to confirm the
  invariant that AtrFallback never emits a StopAnchor.
- **ETHUSDT SHORT** — no short-side observation for ETHUSDT yet.
- **More market-condition diversity** — all observations are from single contiguous
  testnet sessions; trending, ranging, and high-volatility windows have not been
  sampled.
- **Stability evidence for ETHUSDT** — a single Premium observation is insufficient to
  confirm stability; repeated arming under similar conditions is needed.
- **Negative-control evidence** — no observations yet where the operator would
  retrospectively classify the signal as poor, which is needed to estimate
  false-positive rates.

---

## 10. Recommendation

Window 4 successfully expanded the Slice 006 evidence base with:

1. **Symbol diversity** — ETHUSDT confirmed as a functioning second symbol.
2. **New quality class** — `Premium` observed for the first time.
3. **New raw score** — 47, the highest in the evidence base.
4. **Multi-symbol ConfigMap** — `BTCUSDT,ETHUSDT` correctly parsed, two independent
   WebSocket tasks spawned, position monitor polling both symbols.

The invariant from ADR-0023 (symbol-agnostic policy) is supported by this observation:
the Stop-Aware Entry pipeline applied the same classification logic to ETHUSDT without
any symbol-specific code paths, producing a valid and well-formed telemetry line.

**Recommended next steps for Slice 006:**

1. Execute an **ETHUSDT SHORT** stimulus to add short-side coverage for ETHUSDT.
2. Monitor passively for organic `Weak` or `AtrFallback` observations across both
   symbols over time.
3. Accumulate more repetitions per symbol/side for stability evidence.
4. When sufficient evidence is collected, produce the Slice 006 calibration summary
   analyzing the full empirical distribution.

**Not recommended at this stage:**
- Promoting StopQuality from shadow to any decision-affecting path.
- Enabling the `Exceptional` flag.
- Re-tuning `StopQualityClassifier` thresholds.
- Any production rollout.

These actions require a future implementation slice with its own ADR amendment.
