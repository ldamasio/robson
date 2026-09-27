# GLM Briefing — VAL-001 Testnet E2E Execution

**Your role**: Executor
**Parallel track**: Codex is running a pre-flight code audit and will deliver a risk report before you start Phase 1. Do not start until that audit is available.

---

## Context

You are executing the first operational validation gate for Robson v3, a Rust-based execution and risk management daemon for leveraged crypto trading operated by RBX Systems.

**What Robson is**: execution and risk enforcement system. It is NOT an auto-trader. The operator decides when to trade; Robson sizes from a chart-derived Technical Stop Distance, prices execution costs into the worst-case loss, and governs every order through a blocking Risk Engine. The 1% per-trade value is a loss cap, not a sizing target.

**Your task**: execute VAL-001 end-to-end on the testnet environment. This is the blocking gate before real capital can be enabled.

**Cycle to validate**: `arm → detector signal → fill → core trailing-stop engine → exit`

---

## Environment

| Key | Value |
|-----|-------|
| Namespace | `robson-testnet` |
| Exchange | Binance USD-M Futures Testnet (`testnet.binancefuture.com`) |
| Legacy PositionMonitor | removed in repository; verify the deployed image before relying on route removal |
| Daemon access | ClusterIP only — `kubectl port-forward` required |
| Mutating API routes | Google ID token required as Bearer credential |
| Production namespace | `robson` — **do not touch** |

---

## Critical Constraints

- Do not deploy or restart `robsond` during the validation. If the pod image or
  restart count changes, stop and report.
- Never read or print Kubernetes Secret values. Authenticate with
  `robson-cli auth login` and its short-lived Google ID token.
- Never manufacture a stop from a percentage of entry. The stop must come from
  the second support/resistance level on the 15-minute chart through the
  Technical Stop Analyzer.

---

## Your Runbook

Full procedure is at:
`docs/runbooks/val-001-testnet-e2e-validation.md`

Read and follow it exactly. The runbook is authoritative. This briefing is context; the runbook is instruction.

---

## Setup (run first)

```bash
kubectl port-forward svc/robsond 8080:8080 -n robson-testnet &
robson-cli auth status || robson-cli auth login
export ROBSON_TOKEN="$(robson-cli auth print-token)"

export POSITION_ID=""  # set after Phase 1 ARM response
```

---

## Execution Summary

### Prerequisites P1–P7
Run all 7 checks from the runbook. Record the deployed image, pod UID, and restart
count before Phase 1. If any check fails, stop and report — do not proceed.

### Phase 1 — ARM
POST to `/positions`. Export `POSITION_ID` from the response. State must be `Armed`.

### Phase 2 — Detector Signal
Wait for the configured detector to produce a signal and chart-derived stop from
100 15-minute candles. Do not call the test-only signal route with a synthetic
percentage stop. If no valid detector signal appears during the window, record
the run as inconclusive. Check `/status` for a pending approval and use the
authenticated approval route when required.

### Phase 3 — Fill Verification
Poll `/positions/$POSITION_ID` every 5s for up to 2 min. State must reach `Active`. If fill does not arrive: check Binance testnet account balance and logs.

### Phase 4 — Core Trailing-Stop Engine
Use durable `position_monitor_tick` EventLog rows as primary evidence and require
at least three ticks. The wire name is historical; the event is emitted by the
current core engine, not by the removed legacy PositionMonitor. Treat
`trailing_stop_updated` as optional on a short run because the stop may not move.

### Phase 5 — Exit
Follow Phase 5 in the runbook. Use authenticated `DELETE /positions/:id` for the
normal governed per-position exit. Reserve `POST /panic` for emergency cleanup.

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
- Any stop is not traceable to the chart-derived 15-minute Technical Stop Analysis
- Daemon pod restarts during execution
- Exchange returns an order for the wrong symbol or side
- Planned worst-case loss exceeds 1% of `capital_base`, including priced execution costs
- An order lacks its governed `cycle_id`, an UNTRACKED position appears, or an exit fails

On abort:
```bash
curl -s -X POST http://localhost:8080/panic \
  -H "Authorization: Bearer $ROBSON_TOKEN" | jq .
```
Then report the phase, the last EventLog entry, and the exact error.

---

## On Completion

Update the Run Log in the runbook with the actual execution date, executor,
PASS/FAIL (or inconclusive), and a one-line evidence summary.

Report to the PO (Claude) with:
1. Final state: PASS or FAIL
2. POSITION_ID used
3. Any deviation from the expected flow
4. Phase where failure occurred (if FAIL)
