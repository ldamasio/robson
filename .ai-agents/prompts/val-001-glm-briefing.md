# GLM Briefing — VAL-001 Testnet E2E Execution

**Your role**: Executor
**Parallel track**: Codex is running a pre-flight code audit and will deliver a risk report before you start Phase 1. Wait for it if available; proceed after 15 min regardless.

---

## Context

You are executing the first operational validation gate for Robson v3, a Rust-based execution and risk management daemon for leveraged crypto trading operated by RBX Systems.

**What Robson is**: execution and risk enforcement system. It is NOT an auto-trader. The operator decides when to trade; Robson enforces position sizing (Golden Rule: `Position Size = (Capital × 1%) / |Entry − Stop|`) and governs every order through a blocking Risk Engine.

**Your task**: execute VAL-001 end-to-end on the testnet environment. This is the blocking gate before real capital can be enabled.

**Cycle to validate**: `arm → signal inject → fill → trailing stop monitor → exit`

---

## Environment

| Key | Value |
|-----|-------|
| Namespace | `robson-testnet` |
| Exchange | `testnet.binance.vision` (synthetic capital, safe) |
| Position monitor | enabled (`ROBSON_BINANCE_USE_TESTNET: "true"`) |
| Daemon access | ClusterIP only — `kubectl port-forward` required |
| Mutating API routes | Bearer token required |
| Production namespace | `robson` — **do not touch** |

---

## Critical Constraint

**Do NOT accept or trigger a new Docker image build for `robsond` during this execution.**
The deployed image is `sha-88242685`. A new image mid-run would replace the testnet pod and invalidate the validation. If CI triggers unexpectedly, pause and report.

---

## Your Runbook

Full procedure is at:
```
/home/psyctl/apps/robson/docs/runbooks/val-001-testnet-e2e-validation.md
```

Read and follow it exactly. The runbook is authoritative. This briefing is context; the runbook is instruction.

---

## Setup (run first)

```bash
kubectl port-forward svc/robsond 8080:8080 -n robson-testnet &

export ROBSON_TOKEN=$(kubectl get secret -n robson-testnet robsond-secret \
  -o jsonpath='{.data.ROBSON_API_TOKEN}' | base64 -d)

export POSITION_ID=""  # set after Phase 1 ARM response
```

---

## Execution Summary

### Prerequisites P1–P6
Run all 6 checks from the runbook. If any fails, stop and report — do not proceed.

### Phase 1 — ARM
POST to `/positions`. Export `POSITION_ID` from the response. State must be `Armed`.

### Phase 2 — Signal Inject
Fetch live BTCUSDT price from `testnet.binance.vision`. Set stop_loss at **8% below entry** (not 2%).

> **Why 8%**: Risk Engine hard limit is 15% of capital per position.
> `position_value = (capital × 1%) / stop_pct = (100 × 0.01) / 0.08 = 12.5 USDT (12.5% ✅)`.
> A 2% stop yields 50 USDT (50% ❌) — risk-denied silently with HTTP 200, no order placed.

```bash
PRICE=$(curl -s "https://testnet.binance.vision/api/v3/ticker/price?symbol=BTCUSDT" | jq -r '.price')
STOP=$(echo "$PRICE * 0.92" | bc -l | xargs printf "%.2f")
```

POST to `/positions/$POSITION_ID/signal`. Then **verify the signal was not silently denied**:
```bash
# HTTP 200 does NOT guarantee the action was executed — check for Blocked events
curl -s http://localhost:8080/positions/$POSITION_ID | jq '.state'
# Must be "Entering" or "Active" within 10s, NOT still "Armed"
# If still Armed after 10s: the Risk Engine denied the action silently — abort Phase 2
```
Check for pending approvals at `/status` — approve if present.

### Phase 3 — Fill Verification
Poll `/positions/$POSITION_ID` every 5s for up to 2 min. State must reach `Active`. If fill does not arrive: check Binance testnet account balance and logs.

### Phase 4 — Trailing Stop Monitor
Watch logs for tick events processed by the position monitor. **Primary evidence is log output, not EventLog events** — trailing stop events only emit after a full favorable price span, which may not occur during a short testnet run.

Acceptance: position monitor logs show BTCUSDT ticks being processed while position is `Active`.

### Phase 5 — Exit
Use 5A (manual DELETE) unless Codex audit from Phase 4 indicates 5B (stop-triggered) is preferable.

---

## At Each Phase Boundary

After completing each phase, output:
```
PHASE <N> COMPLETE
  State: <state>
  Last log line: <relevant log excerpt>
  EventLog last event: <event_type> at <timestamp>
```

This gives Codex the signal to run the EventLog audit for that phase.

---

## Abort Criteria

Stop immediately if:
- Position state stays `Armed` more than 10s after signal POST (silent risk-deny — wrong stop distance)
- Daemon pod restarts during execution
- Exchange returns an order for wrong symbol, wrong side, or size exceeding 15% of capital
- Exit order fails and position remains open after 3 retry attempts

On abort:
```bash
curl -s -X POST http://localhost:8080/panic \
  -H "Authorization: Bearer $ROBSON_TOKEN" | jq .
```
Then report the phase, the last EventLog entry, and the exact error.

---

## On Completion

Update the Run Log in the runbook:
```
| 2026-04-15 | GLM | ✅ PASS / ❌ FAIL | <one-line summary> |
```

Report to the PO (Claude) with:
1. Final state: PASS or FAIL
2. POSITION_ID used
3. Any deviation from the expected flow
4. Phase where failure occurred (if FAIL)
