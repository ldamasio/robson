# Stop-Aware Entry v4 — Shadow Telemetry Validation Report

**Date:** 2026-05-02
**Status:** Shadow telemetry validated on testnet (2 independent runs, reproducible)
**Scope:** Observational only — no production authorization, no boost, no Slice 006

---

## 1. Summary

The Stop-Aware Entry v4 shadow telemetry was successfully observed on the Binance testnet in **two independent executions**, both producing identical telemetry within ~2 seconds of arming. The `debug!("stop-aware entry shadow telemetry")` line in `robsond::detector` emitted complete metadata for BTCUSDT LONG positions, confirming that the entire shadow pipeline — from chart analysis through stop quality classification — is wired correctly and producing structured, reproducible output.

This report documents the validation. It does **not** authorize boost in production, does **not** start Slice 006, and does **not** alter RiskEngine or TechnicalStopDistance.

## 2. Runtime Under Test

| Field | Value |
|-------|-------|
| Image | `ghcr.io/rbxrobotica/robson-v2:sha-447aba4b` |
| Pod | `robsond-6b65c96c54-smbdn` |
| Namespace | `robson-testnet` |
| ArgoCD | Synced / Healthy at `c559600` |
| RUST_LOG | `robsond=debug,robson_engine=debug,robson_exec=debug,robson_store=debug` |
| Symbol | BTCUSDT |
| Side | LONG |
| Entry policy | `immediate` + `human_confirmation` |
| Shadow mode | Enabled (no boost applied, no approval called, no order executed) |

## 3. Validation Procedure

Two independent bounded stimuli were executed against the same pod (`robsond-6b65c96c54-smbdn`) on image `sha-447aba4b`.

**Pre-conditions (both runs):**
- Confirmed image `sha-447aba4b` active on testnet pod
- Confirmed `DEBUG robsond::` lines visible (log filter bugfix validated)
- Confirmed clean startup: no -2015 errors, 0 UNTRACKED positions, clean recovery
- Confirmed `active_positions=0`, `pending_approvals=[]` before each arm

**Per-run procedure:**
1. `GET /status` — confirm clean state
2. `POST /positions` — arm BTCUSDT LONG with `entry_policy.mode=immediate, approval=human_confirmation`
3. Poll logs up to 75s for `stop-aware entry shadow telemetry`
4. `DELETE /positions/{id}` — cleanup
5. `GET /status` — confirm `active_positions=0`, `pending_approvals=[]`

### Run 1 — 01:31 UTC

| Field | Value |
|-------|-------|
| Position ID | `019de650-1fce-7e72-969a-0fd7fabebd5c` |
| FOUND_TELEMETRY | 1 |
| Time to first telemetry | ~2s after arm |
| Stop anchor | SwingLow |
| Stop quality | Good, raw_score=37 |
| Confidence | High |
| Technical stop method | SwingPoint { level_n: 2 } |
| Final active_positions | 0 |
| Final pending_approvals | [] |

### Run 2 — 02:23 UTC

| Field | Value |
|-------|-------|
| Position ID | `019de67f-7dec-72e0-98ee-4999fc00f276` |
| FOUND_TELEMETRY | 1 |
| Time to first telemetry | ~2s after arm |
| Stop anchor | SwingLow |
| Stop quality | Good, raw_score=37 |
| Confidence | High |
| Technical stop method | SwingPoint { level_n: 2 } |
| Final active_positions | 0 |
| Final pending_approvals | [] |

Both runs produced identical telemetry data, confirming reproducibility.

## 4. Observed Telemetry

Representative shadow telemetry line (identical across both runs):

```
DEBUG robsond::detector: stop-aware entry shadow telemetry
  position_id=019de650-1fce-7e72-969a-0fd7fabebd5c
  symbol=BTCUSDT
  side=Long
  stop_anchor_present=true
  anchor_type=Some(SwingLow)
  stop_quality_class=Good
  raw_score=37
  boost_pct=0.10
  shadow_exceptional=false
  technical_stop_method=SwingPoint { level_n: 2 }
  technical_stop_confidence=High
  detected_levels_count=2
```

Key observations:

- **Stop anchor** resolved to `SwingLow` — chart-based structural anchor present
- **Stop quality** classified as `Good` with `raw_score=37` (above threshold)
- **Technical stop method** is `SwingPoint { level_n: 2 }` — second swing level used
- **Confidence** is `High` — analyst consensus strength
- **Shadow exceptional** is `false` — no boost escalation triggered
- **Boost pct** `0.10` is the default parameter, applied in shadow metadata only

Detector flow confirmed in logs:

1. `Detector task started` — position armed, detector spawned
2. `stop-aware entry shadow telemetry` — shadow metadata populated
3. `Detector emitted signal` — signal produced with entry_price and stop_loss
4. `technical_stop_analyzed` event persisted to eventlog and projection applied
5. `Entry signal received, requesting entry order` — engine processed the signal

## 5. Safety Outcome

| Check | Run 1 | Run 2 |
|-------|-------|-------|
| No approval called | Confirmed | Confirmed |
| No order executed | Confirmed | Confirmed |
| No boost applied | Confirmed (`shadow_exceptional=false`) | Confirmed |
| Position deleted | Confirmed | Confirmed |
| Final `active_positions` | 0 | 0 |
| Final `pending_approvals` | [] | [] |
| No -2015 errors | Confirmed | Confirmed |
| No UNTRACKED positions | Confirmed | Confirmed |
| RiskEngine unchanged | Confirmed — no code changes to risk logic |
| TechnicalStopDistance unchanged | Confirmed — no code changes to stop distance |
| StopQuality shadow-only | Confirmed — classifier output used for telemetry only |

The `quantity below Binance step size 0.001` rejection that appeared after telemetry is **expected and safe behavior**: Robson correctly refuses to round up quantities. The testnet capital (~5000 USDT) is insufficient to produce a valid BTCUSDT order quantity given the stop distance. This does not invalidate the shadow telemetry validation, which occurs upstream of order construction.

## 6. Issues Discovered and Resolved

Six blockers were identified and resolved during the validation phase:

| # | Issue | Resolution |
|---|-------|------------|
| 1 | Binance testnet secret/reload — `-2015` errors | Secret corrected, pod reloaded |
| 2 | Image shadow antiga — missing telemetry code | GitOps updated to deploy shadow-enabled image |
| 3 | `entry_policy_resolved` without handler in projector | Projector patched to handle the event |
| 4 | Stale row in `positions_current` | Controlled cleanup in testnet |
| 5 | `monthly_state` schema drift | Migrations 000009, 000010, 000011 applied |
| 6 | `robsond=info` hardcoded in main.rs | Removed `.add_directive("robsond=info")` to respect RUST_LOG (`sha-447aba4b`) |

Issue #6 was the direct blocker for telemetry observability. The fix was minimal: one line removed from `v3/robsond/src/main.rs`.

## 7. Remaining Non-Blocking Observations

- **Testnet capital**: ~5000 USDT is insufficient for BTCUSDT to reach the AwaitingApproval/order path. This prevents end-to-end simulation of the full query lifecycle but does not block telemetry validation.
- **Detector cycle frequency**: Shadow telemetry emits on every detector cycle (~1s), producing multiple identical lines per armed position. This is expected for shadow mode and will need throttling or deduplication before production use.
- **Stop quality calibration**: `Good` at `raw_score=37` is the first real data point. More observations across different market conditions are needed to validate the scoring thresholds.

## 8. Gates Before Slice 006

Slice 006 has **not** started. The following gates remain:

- [ ] Shadow evidence review — accumulate telemetry across multiple market conditions
- [ ] Stop quality calibration — validate `Good`/`Marginal`/`Poor` thresholds with real data
- [ ] Detector cycle throttling — reduce telemetry noise for production readiness
- [ ] Capital consideration — testnet or sim with sufficient capital for full query lifecycle
- [ ] RiskEngine sign-off — RiskEngine remains the authority on stop distance and position sizing

## 9. Recommendation

The Stop-Aware Entry v4 shadow telemetry is **validated and operational** on testnet. Two independent executions produced identical, reproducible results. The pipeline from chart analysis through stop quality classification produces correct, structured metadata.

**Next step:** Shadow Evidence Review / Calibration — accumulate shadow telemetry across varying market conditions before any consideration of boost or production application. This is an observational phase, not an implementation phase. Slice 006 remains the future implementation gate and has not been started.

**This report does not authorize:**
- Boost in production
- Changes to RiskEngine
- Changes to TechnicalStopDistance
- Changes to StopQualityClassifier thresholds
- Slice 006

**RiskEngine remains the final authority** on stop distance, position sizing, and risk parameters.
