# VAL-001 — Testnet E2E Validation

**Severity**: Critical
**Time to Execute**: 30–60 min
**Required Access**: `kubectl` with `robson-testnet` namespace, Binance testnet account, `robsond` API token

---

## Run Log

| Date | Executor | Result | Notes |
|------|----------|--------|-------|
| 2026-04-15 | Codex | **READY** | Detector now computes chart-derived stops via `TechnicalStopAnalyzer`; verified with `cargo test --all` and `cargo check --all-targets` |
| 2026-04-16 | GLM+Codex | **Phase 1 PASS / Phase 2 inconclusive** | ARM fix deployed (sha-5db3daad, 377 tests). Phase 1: `position_armed` confirmed, `tech_stop_distance: null`. Phase 2: detector fired (MA crossover, chart stop $73,825.27), but Risk Engine correctly blocked entry — exposure $87 > 30% of capital $100 ($30 limit). Position disarmed cleanly. |
| 2026-04-18 | GLM+Codex | **Phase 2 blocked by RiskGate** | Testnet Binance secret was sanitized and invalid testnet key was rotated. Detector emitted chart-derived BTCUSDT signals, but RiskGate correctly denied entries: current stop distance produced proposed notional around 50-55% of capital, above the 30% total exposure limit (and above the 15% single-position limit). Capital-only retries are not valid because sizing and exposure limits both scale from the same `RiskConfig.capital`. All armed positions were disarmed; final `/status` was clean. |

*VAL-001 Phase 1 PASS. Phase 2 remains blocked until a detector-provided technical stop produces policy-compliant sizing, or an explicit testnet-only policy change is approved and documented. VAL-002 remains blocked.*

---

## Purpose

Validate the complete position lifecycle on `robson-testnet` before enabling real capital in production.

**Blocking gate for**: VAL-002 (real capital activation — Binance real keys + `ROBSON_POSITION_MONITOR_ENABLED: "true"` in prod).

**Cycle under validation**:
```
arm → detector signal → fill → trailing stop monitor → exit
```

### Symbol selection (ADR-0023 — Symbol-Agnostic Policy)

This runbook is **symbol-agnostic**. The default validation target is `BTCUSDT`
because tick flow is most reliable there on testnet, but the procedure below applies
verbatim to any symbol the operator configures in `robsond`. Wherever a command
contains `BTCUSDT`, treat the value as a placeholder: export `SYMBOL=BTCUSDT` (or
your target) and substitute as needed. Before promoting a new pair to production,
VAL-001 MUST be re-executed with that pair (follow-up required per ADR-0023).

### Position-authorship invariant (ADR-0022)

Throughout this runbook, the **Robson-authored position invariant** applies: every
open exchange position on the testnet account must trace to a `robsond`-authored
entry. Any UNTRACKED position detected at any phase is a P0 abort condition. See
prerequisite **P7** and [UNTRACKED-POSITION-RECONCILIATION.md](../policies/UNTRACKED-POSITION-RECONCILIATION.md).

**Environment facts** (repository-verified, 2026-04-15):

| Key | Value |
|-----|-------|
| Namespace | `robson-testnet` |
| Exchange | `testnet.binance.vision` (`ROBSON_BINANCE_USE_TESTNET: "true"`) |
| Position monitor | enabled (`ROBSON_POSITION_MONITOR_ENABLED: "true"`, symbol: `BTCUSDT`) |
| API access | ClusterIP — `kubectl port-forward` only |
| Mutating routes | Bearer token required |

---

## Risk And Sizing Notes

VAL-001 must not bypass the Technical Stop Distance policy. The detector must
derive `stop_loss` from chart analysis; do not inject a percentage stop or
manually override `tech_stop_distance` to force the test through RiskGate.

Current sizing uses the same operator-supplied capital for both the 1% per-trade
risk amount and the portfolio exposure limits:

```text
risk_amount = capital * 1%
technical_stop_distance = abs(entry_price - detector_stop_loss)
position_size = risk_amount / technical_stop_distance
notional_exposure = position_size * entry_price
max_total_exposure = capital * 30%
max_single_position = capital * 15%
```

Because both `notional_exposure` and exposure limits scale with capital,
increasing `capital` alone does not change the exposure ratio:

```text
notional_exposure / capital = 1% / technical_stop_distance_pct
```

With the default limits, a candidate entry requires approximately:

```text
technical_stop_distance_pct >= 3.33%  # total exposure <= 30%
technical_stop_distance_pct >= 6.67%  # single position <= 15%
```

If the detector emits a valid chart-derived stop closer than this, RiskGate must
deny entry and VAL-001 Phase 2 remains inconclusive/blocked. Do not treat that as
a daemon failure.

---

## Prerequisites

> **Executor: GLM** — run all checks before starting Phase 1.

| ID | Check | Command | Expected |
|----|-------|---------|----------|
| P1 | Pod running | `kubectl get pods -n robson-testnet` | `1/1 Running`, 0 restarts |
| P2 | ArgoCD Synced/Healthy | `kubectl get app robson-testnet -n argocd -o jsonpath='{.status.sync.status} {.status.health.status}'` | `Synced Healthy` |
| P3 | DB migrations applied | `kubectl logs -n robson-testnet deploy/robsond --since=10m \| grep -i migrat` | No migration errors |
| P4 | Symbol-under-test ticks flowing | `kubectl logs -n robson-testnet deploy/robsond --since=2m \| grep -iE "tick\|market_data\|$SYMBOL"` | Tick events visible for the symbol under validation (default example: `BTCUSDT`; any configured pair is acceptable per ADR-0023) |
| P5 | Clean state (no open positions) | `curl http://localhost:8080/status` (after port-forward) | `"active_positions": 0` |
| P6 | API token available | `kubectl get secret -n robson-testnet robsond-testnet-secret -o jsonpath='{.data.api-token}' \| base64 -d` | Non-empty string |
| P7 | No UNTRACKED exchange positions (ADR-0022) | Query Binance testnet account for ALL open positions/balances across ALL account types and symbols; cross-check against `event_log` `entry_order_placed` by exchange order id | Zero UNTRACKED positions. Any position without a matching event is a P0 block — close it before proceeding (see [UNTRACKED-POSITION-RECONCILIATION.md](../policies/UNTRACKED-POSITION-RECONCILIATION.md)) |

**Setup**:
```bash
kubectl port-forward svc/robsond 8080:8080 -n robson-testnet &
export ROBSON_TOKEN=$(kubectl get secret -n robson-testnet robsond-testnet-secret \
  -o jsonpath='{.data.api-token}' | base64 -d)
```

**If any prerequisite fails**: do not proceed. Fix the blocking condition first. See related runbooks.

---

## Procedure

### Phase 1 — Arm

> **Executor: GLM**

```bash
ARM_RESPONSE=$(curl -s -X POST http://localhost:8080/positions \
  -H "Authorization: Bearer $ROBSON_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"symbol": "BTCUSDT", "side": "long", "capital": "100"}')

echo $ARM_RESPONSE | jq .
export POSITION_ID=$(echo $ARM_RESPONSE | jq -r '.position_id')
```

**Expected output**:
```json
{
  "position_id": "<uuid>",
  "symbol": "BTCUSDT",
  "side": "long",
  "state": "Armed"
}
```

**If this fails**: check `ROBSON_TOKEN` is set; verify `/status` returns 200; check logs for `arm` errors.

**EventLog audit** — Codex verifies:
```sql
SELECT event_type, payload, timestamp
FROM event_log
WHERE stream_key = 'position:<uuid>'
ORDER BY sequence;
-- Required: position_armed
```

**Phase 1 acceptance**: `state = Armed` AND `position_armed` event in EventLog.

---

### Phase 2 — Detector Signal

> **Executor: GLM**

```bash
kubectl logs -n robson-testnet deploy/robsond -f \
  | grep -E "MA crossover|technical stop|Detector emitted signal|entry|Entering|Armed|order"
```

Do not inject a synthetic percentage stop. The detector must fetch 100 15-minute
candles and emit `DetectorSignal.stop_loss` from chart analysis. If no detector
signal occurs during the validation window, record the run as inconclusive and do
not proceed to VAL-002.

**QueryEngine approval gate check** (GLM):
```bash
# If capital > 5% of risk limit, an approval may be queued
curl -s http://localhost:8080/status | jq '.pending_approvals'
# If non-empty: POST /queries/<query_id>/approve
curl -s -X POST http://localhost:8080/queries/<query_id>/approve \
  -H "Authorization: Bearer $ROBSON_TOKEN" | jq .
```

**EventLog audit** — Codex verifies:
```sql
SELECT event_type, payload, timestamp
FROM event_log
WHERE stream_key = 'position:<uuid>'
ORDER BY sequence;
-- Required: entry_signal_received, entry_order_placed
-- Verify: entry_order_placed has cycle_id (GovernedAction proof)
```

**Phase 2 acceptance**: `entry_signal_received` AND `entry_order_placed` (with `cycle_id`) in EventLog.

---

### Phase 3 — Fill Verification

> **Executor: GLM**

```bash
# Poll for Active state (max 2 min — testnet fills can be slow)
for i in $(seq 1 24); do
  STATE=$(curl -s http://localhost:8080/positions/$POSITION_ID | jq -r '.state')
  echo "$(date +%T) state=$STATE"
  [[ "$STATE" == "Active" ]] && echo "FILL CONFIRMED" && break
  sleep 5
done

kubectl logs -n robson-testnet deploy/robsond --since=3m \
  | grep -E "fill|Active|entry_filled"
```

**If fill does not arrive within 2 min**: check Binance testnet account balance; check order status in Binance testnet UI; review logs for `OrderFailed` or `Blocked` events.

**EventLog audit** — Codex verifies:
```sql
-- Required: entry_filled, position_active
-- Verify: fill_price in entry_filled payload is within 1% of entry_price
SELECT event_type,
       payload->>'fill_price' AS fill_price,
       payload->>'entry_price' AS entry_price,
       timestamp
FROM event_log
WHERE stream_key = 'position:<uuid>'
  AND event_type IN ('entry_filled', 'position_active')
ORDER BY sequence;
```

**Phase 3 acceptance**: `state = Active`, `entry_filled` event with `fill_price` in EventLog.

---

### Phase 4 — Trailing Stop Monitor

> **Executor: GLM**

```bash
# Monitor position monitor ticks for at least 3 ticks
kubectl logs -n robson-testnet deploy/robsond -f \
  | grep -E "trailing|stop|monitor|tick|BTCUSDT" \
  | head -20
```

**Expected log pattern**:
```
DEBUG robsond::position_monitor: tick BTCUSDT price=X trailing_stop=Y
```

**EventLog audit** — Codex verifies:
```sql
-- Primary evidence: position_monitor_tick fires on every tick
SELECT event_type,
       payload->>'price'          AS price,
       payload->>'current_stop'   AS current_stop,
       payload->>'high_watermark' AS high_watermark,
       payload->>'span_remaining' AS span_remaining,
       timestamp
FROM event_log
WHERE stream_key = 'position:<uuid>'
  AND event_type = 'position_monitor_tick'
ORDER BY sequence;
-- Required: at least 3 rows
-- Verify: for a long, high_watermark is non-decreasing across rows

-- Secondary evidence: trailing_stop_updated (only fires when stop moves)
SELECT event_type, payload, timestamp
FROM event_log
WHERE stream_key = 'position:<uuid>'
  AND event_type = 'trailing_stop_updated'
ORDER BY sequence;
-- Optional on short runs — stop may not move if price stays flat
-- If present: verify current_stop increases for long positions
```

**Phase 4 acceptance**: at least 3 `position_monitor_tick` events in EventLog, `high_watermark` non-decreasing for long positions.

---

### Phase 5 — Exit

Two strategies — choose based on available time:

**5A — Manual exit** (faster, validates the manual path):

> **Executor: GLM**

```bash
curl -s -X DELETE http://localhost:8080/positions/$POSITION_ID \
  -H "Authorization: Bearer $ROBSON_TOKEN" | jq .

kubectl logs -n robson-testnet deploy/robsond -f \
  | grep -E "exit|Exiting|Closed|pnl"
```

**5B — Stop-triggered exit** (more complete, validates the automatic path):

> **Executor: GLM** — arm a new position and wait for the detector-provided technical stop plus position monitor to trigger the exit automatically. Do not manufacture a stop from a percentage of entry.

**EventLog audit** — Codex verifies full sequence:
```sql
SELECT event_type, payload, timestamp
FROM event_log
WHERE stream_key = 'position:<uuid>'
ORDER BY sequence;
-- Required full sequence:
-- position_armed
-- entry_signal_received
-- entry_order_placed        (with cycle_id)
-- entry_filled
-- position_active
-- trailing_stop_updated     (at least 1)
-- exit_order_placed         (with cycle_id)
-- exit_filled
-- position_closed           (with pnl calculated)
```

**Phase 5 acceptance**: `state = Closed`, `position_closed` event with `pnl` field in EventLog.

---

## Validation Checklist

Complete after Phase 5. All 6 items required for PASS.

- [ ] **Full event sequence**: all 7+ events (`position_armed` → `position_closed`) present in correct order
- [ ] **Governance proof**: every `entry_order_placed` and `exit_order_placed` has a `cycle_id` in payload (GovernedAction token — Risk Engine not bypassed)
- [ ] **PnL calculated**: `position_closed` event has `pnl` field with a numeric value
- [ ] **Zero critical errors**: no `ERROR` or `PANIC` in daemon logs during the cycle
- [ ] **Clean state**: `GET /status` returns `"active_positions": 0` after exit
- [ ] **Zero UNTRACKED positions** (ADR-0022): a post-exit scan of all testnet account types and symbols shows no open exchange position without a matching `entry_order_placed` event

**Result**: record PASS or FAIL with notes in the Run Log at the top of this document.

---

## Abort Criteria

Stop immediately and do not proceed to VAL-002 if:

- Any order reaches the exchange with incorrect symbol, side, or size
- Risk Engine is bypassed (order placed without `cycle_id` in EventLog)
- Daemon crashes (pod restarts) during the cycle
- Exit order fails and position remains open on exchange after cleanup
- An UNTRACKED position is detected at any point (ADR-0022): an open exchange
  position on any symbol or account type with no matching `entry_order_placed`
  event. Close it immediately and investigate the root cause before retrying.

**Abort procedure**:
```bash
# Emergency: close all open positions on testnet
curl -s -X POST http://localhost:8080/panic \
  -H "Authorization: Bearer $ROBSON_TOKEN" | jq .
```

---

## Executor Division

| Responsibility | GLM | Codex |
|---------------|-----|-------|
| Run kubectl/curl commands | ✅ | |
| Monitor logs in real time | ✅ | |
| Poll state between phases | ✅ | |
| Capture outputs as evidence | ✅ | |
| Audit EventLog event sequence | | ✅ |
| Verify cycle_id present on all orders | | ✅ |
| Identify missing or malformed events | | ✅ |
| Root-cause analysis on phase failures | | ✅ |
| Propose code fix if a phase fails | | ✅ |
| Write PASS/FAIL verdict with evidence | | ✅ |

---

## Rollback

VAL-001 is read-only with respect to production. Testnet is isolated by design.

If the testnet environment is left in a dirty state after a failed run:
```bash
# Disarm any armed positions
curl -s -X DELETE http://localhost:8080/positions/$POSITION_ID \
  -H "Authorization: Bearer $ROBSON_TOKEN"

# Or panic-close everything
curl -s -X POST http://localhost:8080/panic \
  -H "Authorization: Bearer $ROBSON_TOKEN" | jq .
```

To reset the testnet DB to a clean state: bounce the pod (the daemon recovers state from EventLog on restart).

---

## Known Gaps (pre-flight audit, 2026-04-15)

These were identified by Codex B3 before first execution. They do not block VAL-001 PASS but
must be tracked as follow-up work:

| # | Gap | Impact | Follow-up |
|---|-----|--------|-----------|
| 1 | `trailing_stop_updated` events only emit after full favorable span | Phase 4 cannot use EventLog as primary evidence on short runs | Document span threshold; consider emitting a `position_monitor_tick` audit event |

---

## Related Documentation

- [VAL-002 — Real Capital Activation](val-002-real-capital-activation.md) — next gate after this one passes
- [v3-migration-plan.md](../architecture/v3-migration-plan.md) — MIG-v3 status table references this runbook
- [v3-runtime-spec.md](../architecture/v3-runtime-spec.md) — Control loop and GovernedAction spec
- [v3-control-loop.md](../architecture/v3-control-loop.md) — Cycle stages validated here
- [ADR-0003](../adr/ADR-0003-robson-testnet-isolation.md) — Testnet isolation architecture decision
