# Stop-Aware Entry v4 — Shadow Observation Window 5 (ETHUSDT SHORT)

**Date:** 2026-05-03
**Status:** Window 5 completed — controlled stimulus, observational only
**Scope:** Side diversity expansion — first ETHUSDT SHORT observation for Slice 006

---

## 1. Summary

Window 5 was a **controlled, bounded stimulus** designed to add SHORT-side coverage
for ETHUSDT to the Slice 006 evidence base. Window 4 had established ETHUSDT LONG as
the first non-BTCUSDT observation. Window 5 completes the LONG/SHORT pair for ETHUSDT
and reveals a cross-symbol SHORT behavior pattern.

Key findings:

- Shadow telemetry appeared within **~1.9s** of arming.
- `stop_quality_class=Good`, `raw_score=38` — identical to the BTCUSDT SHORT
  observation (Windows 2–3). This is the first cross-symbol corroboration of SHORT
  classification behavior in the evidence base.
- `anchor_type=Some(SwingHigh)`, `technical_stop_method=SwingPoint { level_n: 1 }`,
  `technical_stop_confidence=Medium`, `detected_levels_count=1` — all four values
  match BTCUSDT SHORT exactly, suggesting the classifier produces consistent output
  for SHORT positions across different symbols under similar market conditions.
- `boost_pct=0.10` was recorded in shadow telemetry metadata; it was **not applied**
  to any sizing, risk, or approval calculation.
- `shadow_exceptional=false` — gate held across all five windows to date.
- No approval was called. No order was executed. The query reached `AwaitingApproval`
  and was gated by `human_confirmation` TTL.
- An `insufficient candle data` warning appeared on the first detector tick and
  resolved on the second tick; it was transient and non-blocking.

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
| Pod uptime at stimulus | ~7h49m |
| Restarts | 0 |
| `ROBSON_MARKET_DATA_SYMBOLS` | `BTCUSDT,ETHUSDT` |
| `ROBSON_POSITION_MONITOR_SYMBOLS` | `BTCUSDT,ETHUSDT` |
| `RUST_LOG` | `robsond=debug,robson_engine=debug,robson_exec=debug,robson_store=debug` |
| ArgoCD | Synced / Healthy — revision `065fe2a` (rbx-infra PR #5) |

This is the same pod as Window 4, now at 7h49m uptime. No configuration change was
made between Window 4 and Window 5.

---

## 3. Stimulus Definition

| Field | Value |
|-------|-------|
| Type | Controlled bounded stimulus |
| Symbol | `ETHUSDT` |
| Side | `SHORT` |
| Entry policy mode | `immediate` |
| Entry policy approval | `human_confirmation` |
| Position ID | `019dee3c-59c2-7993-a7e7-34c3a4259df8` |
| Observation window | up to 75s |
| Boost applied | No |
| Approval called | No |
| Order executed | No |

**Operator actions performed:**
1. `POST /positions` — arm ETHUSDT SHORT
2. `DELETE /positions/019dee3c-59c2-7993-a7e7-34c3a4259df8` — disarm after telemetry
   captured (HTTP 204)

No other operator actions were taken.

---

## 4. Observed Telemetry

Shadow telemetry line appeared at `2026-05-03T14:27:06.862926Z`, approximately **1.9s**
after the arm request:

```
DEBUG robsond::detector: stop-aware entry shadow telemetry
  position_id=019dee3c-59c2-7993-a7e7-34c3a4259df8
  symbol=ETHUSDT
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

| Field | Value | Notes |
|-------|-------|-------|
| `symbol` | `ETHUSDT` | |
| `side` | `Short` | First ETHUSDT SHORT observation |
| `stop_anchor_present` | `true` | Structural anchor found |
| `anchor_type` | `Some(SwingHigh)` | Expected for SHORT positions |
| `stop_quality_class` | `Good` | Matches BTCUSDT SHORT |
| `raw_score` | `38` | Matches BTCUSDT SHORT |
| `boost_pct` | `0.10` | Recorded in shadow metadata only; not applied |
| `shadow_exceptional` | `false` | Gate held — 5/5 windows |
| `technical_stop_method` | `SwingPoint { level_n: 1 }` | Matches BTCUSDT SHORT |
| `technical_stop_confidence` | `Medium` | Matches BTCUSDT SHORT |
| `detected_levels_count` | `1` | Matches BTCUSDT SHORT |

---

## 5. Comparison Against ETHUSDT LONG

| Field | ETHUSDT LONG (Window 4) | ETHUSDT SHORT (Window 5) |
|-------|------------------------|--------------------------|
| `side` | Long | **Short** |
| `anchor_type` | SwingLow | **SwingHigh** |
| `stop_quality_class` | Premium | **Good** |
| `raw_score` | 47 | **38** |
| `boost_pct` | 0.15 | **0.10** |
| `technical_stop_method` | SwingPoint level_n=2 | **SwingPoint level_n=1** |
| `technical_stop_confidence` | High | **Medium** |
| `detected_levels_count` | 2 | **1** |
| `stop_anchor_present` | true | true |
| `shadow_exceptional` | false | false |

Within ETHUSDT, the SHORT side produces lower `raw_score`, lower confidence, fewer
detected levels, and a shallower stop method (`level_n=1`) compared to the LONG side.
This mirrors the same directional difference observed between BTCUSDT LONG and BTCUSDT
SHORT, suggesting the pattern is side-driven rather than symbol-driven.

---

## 6. Cross-Symbol SHORT Comparison

| Field | BTCUSDT SHORT (Windows 2–3) | ETHUSDT SHORT (Window 5) |
|-------|----------------------------|--------------------------|
| `anchor_type` | SwingHigh | SwingHigh |
| `stop_quality_class` | Good | Good |
| `raw_score` | 38 | 38 |
| `boost_pct` | 0.10 | 0.10 |
| `technical_stop_method` | SwingPoint level_n=1 | SwingPoint level_n=1 |
| `technical_stop_confidence` | Medium | Medium |
| `detected_levels_count` | 1 | 1 |
| `stop_anchor_present` | true | true |
| `shadow_exceptional` | false | false |

**All seven observable fields matched exactly** between BTCUSDT SHORT and ETHUSDT
SHORT. This is the first cross-symbol corroboration in the evidence base and provides
initial support for the symbol-agnostic policy invariant (ADR-0023): the classifier
produced identical shadow output for the SHORT side across two different symbols under
the observed testnet conditions.

This is a single-window observation for ETHUSDT SHORT and should not be treated as a
statistical confirmation. More repetitions under varying market conditions are required
before drawing strong conclusions about cross-symbol stability.

---

## 7. Safety Outcome

- **No approval was called** — `POST /queries/:id/approve` was not invoked.
- **No order was executed** — no `place_order`, `filled`, or `executed` log lines.
- **No `-2015` errors** — API key / permission gates held.
- **No `UNTRACKED` positions** — startup and runtime clean.
- **No panics** detected.
- **`shadow_exceptional=false`** — gate held in all five windows to date.
- **`boost_pct=0.10`** appeared in shadow telemetry metadata only. It was not applied
  to stop distance, position sizing, or risk parameters.
- **StopQuality remains shadow-only** — the `Good` classification did not influence
  any risk parameter, position sizing, or stop distance.
- **RiskEngine remains the final authority** on stop distance, sizing, and risk.
- **Pre- and post-status**: `active_positions=0`, `pending_approvals=[]` in both cases.
- **Disarm**: HTTP 204 — clean, no error.

---

## 8. Query / Approval Path Observation

```
ArmPosition → Accepted → Processing → Acting → Completed
  └─ Detector spawned → MarketData received → TechnicalStop computed
       └─ ProcessSignal → Accepted → Processing → RiskChecked → AwaitingApproval
```

| Field | Value |
|-------|-------|
| `entry_price` | 2322.69 USDT |
| `stop_loss` | 2329.11 USDT |
| Stop distance | ~0.28% — compact for SHORT |
| Quantity | ~0.1557 ETH |
| Notional | ~361.60 USDT |
| Risk check | Approved |
| Final query state | AwaitingApproval (TTL expiry ~14:32:06 UTC) |

The query was never approved. The approval TTL will elapse and the query will expire
without any operator or automated action. This is the expected behavior for
`human_confirmation` mode under controlled observation.

The stop distance of ~0.28% is notably compact relative to the configured minimum
(`ROBSON_MIN_TECH_STOP_PCT=1.0%`). The RiskEngine accepted the signal; the stop
distance validation is applied at the TechnicalStopDistance layer before signal
emission, so the fact that a signal was emitted implies the engine considers this
distance valid within its policy envelope. This warrants further investigation in
future calibration windows but is not a blocker for Slice 006.

---

## 9. Non-Blocking Warnings

### 9.1 Insufficient candle data (transient)

```
WARN robsond::detector: Detector could not compute technical stop
  position_id=019dee3c-59c2-7993-a7e7-34c3a4259df8
  symbol=ETHUSDT
  error=Detector error: Insufficient candle data: need at least 100 candles, got 92.
  Fetch more history before computing the technical stop.
```

This warning appeared on the **first detector tick** (~14:27:06.334Z), approximately
0.5s after the detector was spawned. The detector received only 92 candles on its
initial fetch, below the required 100 (`ROBSON_TECH_STOP_LOOKBACK=100`).

On the **second tick** (~14:27:06.609Z), the detector had sufficient data and
proceeded to compute the technical stop and emit shadow telemetry normally.

**Impact:** One detector cycle skipped. No telemetry was suppressed beyond the initial
cycle. Not a blocker. This pattern may appear more frequently for ETHUSDT than BTCUSDT
if the testnet has less historical data available for ETH; it is worth monitoring but
does not require action in Slice 006.

### 9.2 entry_approval_pending projection handler missing

```
WARN robsond::position_manager: Failed to persist EntryApprovalPending audit event
  — approval continues
  error=Missing handler for event type entry_approval_pending (seq=4)

ERROR robsond::daemon: Error handling event
  error=Missing handler for event type entry_approval_pending (seq=4)
```

This is the same warning observed in BTCUSDT SHORT (Windows 2–3) and ETHUSDT LONG
(Window 4). It reflects an absent projection handler for the `entry_approval_pending`
event type. It did not block telemetry, did not block `AwaitingApproval`, did not
cause any approval or order, and did not prevent the disarm. This is a known
event-sourcing projection debt to be addressed in a future implementation slice.
It is out of scope for Slice 006.

---

## 10. Updated Slice 006 Evidence Coverage

### Cumulative telemetry table

| position_id (short) | symbol | side | anchor | stop_quality | raw_score | method | confidence | levels |
|---------------------|--------|------|--------|-------------|-----------|--------|------------|--------|
| `019de650` | BTCUSDT | Long | SwingLow | Good | 37 | SwingPoint(2) | High | 2 |
| `019de67f` | BTCUSDT | Long | SwingLow | Good | 37 | SwingPoint(2) | High | 2 |
| `019de906` (early) | BTCUSDT | Short | SwingHigh | Good | 38 | SwingPoint(1) | Medium | 1 |
| `019de906` (final) | BTCUSDT | Short | SwingHigh | None | 0 | SwingPoint(1) | Medium | 1 |
| `019dec8d` | ETHUSDT | Long | SwingLow | Premium | 47 | SwingPoint(2) | High | 2 |
| `019dee3c` | **ETHUSDT** | **Short** | SwingHigh | Good | 38 | SwingPoint(1) | Medium | 1 |

### Coverage matrix (updated after Window 5)

| Dimension | Values observed | Values still missing |
|-----------|----------------|----------------------|
| Symbols | BTCUSDT, ETHUSDT | Other operated pairs |
| Sides | Long ✅, Short ✅ | — (both covered for both symbols) |
| `stop_quality_class` | None, Good, Premium | **Weak** |
| `anchor_type` | SwingLow, SwingHigh | structural, `stop_anchor_present=false` |
| `technical_stop_method` | SwingPoint level_n=1, level_n=2 | structural variants, **AtrFallback** |
| `technical_stop_confidence` | High, Medium | Low (if emitted) |
| `shadow_exceptional` | false (5/5) | — (must remain false) |
| Market conditions | testnet single-session windows | trending, ranging, high-volatility |
| Repetitions per symbol/side | 1–8 BTCUSDT Long; 1 each ETHUSDT | more per symbol/side for stability |

### Emerging patterns

**Side pattern (consistent across symbols):**

| Field | LONG (both symbols) | SHORT (both symbols) |
|-------|--------------------|--------------------|
| anchor_type | SwingLow | SwingHigh |
| level_n | 2 | 1 |
| confidence | High | Medium |
| detected_levels | 2 | 1 |
| raw_score | 37–47 | 0–38 |

This pattern held across two symbols. It suggests the classifier's output is more
strongly governed by side than by symbol — i.e., the structural conditions for
SwingHigh SHORT (fewer levels, lower confidence) differ systematically from SwingLow
LONG under the observed testnet market conditions.

**`shadow_exceptional=false` invariant:** Held across all five windows (21+ telemetry
lines). No Premium or Good classification has triggered the `Exceptional` path.

---

## 11. Recommendation

Window 5 closes the LONG/SHORT pair for ETHUSDT and produces the first cross-symbol
SHORT corroboration in the evidence base. The Slice 006 evidence base now covers:

- 2 symbols (BTCUSDT, ETHUSDT)
- 2 sides (Long, Short)
- 3 quality classes (None, Good, Premium)
- 2 anchor types (SwingLow, SwingHigh)
- 2 stop methods (SwingPoint level_n=1, level_n=2)
- 2 confidence levels (High, Medium)

**Remaining evidence gaps for Slice 006 completion:**

1. **`Weak` quality class** — not yet observed. May require different market conditions
   or signals with intermediate structural quality.
2. **`AtrFallback` / `stop_anchor_present=false`** — required to confirm the invariant
   that AtrFallback never emits a StopAnchor. No organic observation yet.
3. **Broader market-condition diversity** — all observations are from discrete testnet
   sessions; trending, ranging, and high-volatility conditions have not been sampled.
4. **More repetitions per symbol/side** — ETHUSDT has only one observation per side;
   stability cannot be confirmed from a single window per (symbol, side) pair.
5. **Negative-control evidence** — no observations where the operator would
   retrospectively classify the signal as poor.

**Recommended next steps:**

1. Allow passive observation over multiple sessions to accumulate repetitions and
   catch `Weak` or `AtrFallback` organically.
2. When evidence coverage is sufficient, produce the Slice 006 calibration summary
   with the full empirical distribution across all collected windows.

**Not recommended at this stage:**
- Promoting StopQuality from shadow to any decision-affecting path.
- Enabling the `Exceptional` flag.
- Re-tuning `StopQualityClassifier` thresholds.
- Any production rollout.
- Triggering further controlled stimuli solely to hunt for missing quality classes —
  the `Weak` and `AtrFallback` cases should emerge from organic market observation
  rather than forced stimuli that may not represent real signal conditions.
