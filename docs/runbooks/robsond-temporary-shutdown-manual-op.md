# Robsond — Temporary Shutdown for Manual Binance Operation

**Severity**: High
**Time to Execute**: 5–15 min (excluding time spent on the manual operation itself)
**Required Access**: `rbx-infra` GitOps write access, `kubectl` with production `robson` namespace, Binance account

---

## Run Log

| Date | Executor | Reason | Result | Notes |
|------|----------|--------|--------|-------|
| 2026-05-14 | psyctl | Manual trade on Binance account | Completed — position auto-closed by reconciliation worker on scale-up with open position | First execution; runbook authored post-incident |

*Update this table after every execution.*

---

## Purpose

This runbook describes how to temporarily stop the `robsond` daemon so the operator can perform manual operations on the Binance account (e.g., placing an order via the Binance website or app, emergency manual close, account inspection).

**Critical constraint**: the Binance account operated by `robsond` is under daemon authority at all times when the daemon is running. The reconciliation worker enforces ADR-0022 unconditionally — any position not traceable to a `robsond`-authored entry will be closed at market within one reconciliation interval (~60 s). This is not a bug; it is the intended behavior.

**The safe window for manual operations is: after the pod terminates, before the pod restarts.**

---

## When to Use This Runbook

Use this procedure when you need to:

- Place a manual trade on the Binance account while Robson is temporarily out of the way.
- Perform a manual emergency close of a position directly on the exchange.
- Conduct account-level operations (transfers, margin adjustments) that require exclusive access.
- Test or inspect account state without Robson interfering.

**If the operation can be done on a separate Binance account (not connected to `robsond` credentials), do that instead.** This runbook is for cases where the operated account specifically is required.

---

## Incident Reference

**2026-05-14 incident**: operator scaled down Robson, placed a manual Long (BTCUSDT 0.086 BTC @ ~81,328), then scaled Robson back up before closing the position. The reconciliation worker detected the UNTRACKED position and issued a Market Sell (order `1011992057990`, 0.086 BTC @ 81,315.10) within ~46 minutes of the daemon restarting. The close was correct per ADR-0022 but was unintended by the operator. Root cause: no documented procedure warning that all manual positions must be closed before scale-up.

---

## Prerequisites

- `rbx-infra` repository cloned locally and up to date (`git pull origin main`).
- `kubectl` access to the production `robson` namespace.
- Knowledge of what open positions Robson currently holds (check `/status` or the frontend before scaling down).

---

## Procedure

### Step 1 — Record current Robson state

Before stopping the daemon, note what Robson has open so you can distinguish Robson-authored positions from your manual ones when you return.

```bash
# List active positions via API (or check the frontend)
curl -s -H "Authorization: Bearer $ROBSON_TOKEN" \
  https://robson.rbx.ia.br/api/v1/status | jq '.active_positions'
```

Alternatively, open the frontend at `https://robson.rbx.ia.br` and take note of all armed/active positions.

### Step 2 — Scale down via GitOps

Edit `apps/prod/robson/robsond-deploy.yml` in the `rbx-infra` repository:

```yaml
spec:
  replicas: 0   # was: 1
```

Commit and push:

```bash
git add apps/prod/robson/robsond-deploy.yml
git commit -m "ops(robson): scale down to 0 for manual Binance operation"
git push origin main
```

ArgoCD will sync automatically. Wait for the pod to terminate before proceeding.

### Step 3 — Confirm pod is gone

```bash
kubectl get pods -n robson -l app.kubernetes.io/name=robsond
# Expected: No resources found in robson namespace.
```

Do not proceed until the pod is fully terminated. While a pod is in `Terminating` state it may still be processing WebSocket messages or placing trailing-stop orders.

### Step 4 — Perform your manual operations on Binance

With the daemon stopped, you have exclusive access to the account. The reconciliation worker is not running.

Take note of every position you open so you can close them before restarting the daemon.

### Step 5 — Close all manually opened positions before scale-up ⚠️

**This is the critical step.**

Before scaling `robsond` back up, close every position you opened manually. Verify on the Binance website or app that the account has zero open positions (or only positions that Robson authored in Step 1 and that you did not touch).

If you restart the daemon with any manually-opened position still open, the reconciliation worker will close it automatically within one reconciliation interval (~60 s). The close is a mandatory Market Sell and is not overridable by configuration.

### Step 6 — Restore replicas via GitOps

Edit `apps/prod/robson/robsond-deploy.yml` back to `replicas: 1`:

```yaml
spec:
  replicas: 1   # restored
```

Commit and push:

```bash
git add apps/prod/robson/robsond-deploy.yml
git commit -m "ops(robson): restore replicas to 1 after manual Binance operation"
git push origin main
```

### Step 7 — Confirm daemon is healthy

```bash
kubectl get pods -n robson -l app.kubernetes.io/name=robsond
# Expected: robsond-<hash>  1/1  Running

kubectl logs -n robson -l app.kubernetes.io/name=robsond --since=2m | grep -E "INFO|WARN|ERROR" | head -30
```

Check that startup recovery completes without UNTRACKED detections:

```bash
kubectl logs -n robson -l app.kubernetes.io/name=robsond --since=2m | grep -i "untracked\|rogue"
# Expected: no output (zero untracked detections)
```

---

## What Happens If You Scale Up With Open Manual Positions

The reconciliation worker runs every ~60 s and is not gated by `ROBSON_POSITION_MONITOR_ENABLED`. On its first scan after startup, it queries Binance for all open positions, checks each one against `event_log`, and classifies any position without a matching `entry_order_placed` event as UNTRACKED. It then:

1. Emits `position_untracked_detected` to `event_log`.
2. Sends a CRITICAL alert to the operator channel.
3. Issues a Market Sell (or Market Buy for Short positions) for the full UNTRACKED quantity.
4. Emits `untracked_position_closed` with the resulting fill.

The close cannot be aborted once triggered. The `POST /reconciliation/suspend` endpoint (planned, not yet implemented) will allow a short TTL suspension in future; until then, the close is non-overridable.

This behavior is correct and intentional — see ADR-0022 and UNTRACKED-POSITION-RECONCILIATION.md.

---

## Future: Reconciliation Suspend Endpoint

Once `POST /reconciliation/suspend` is implemented, this runbook will be updated. The endpoint will allow the operator to suspend the reconciliation worker for up to 300 seconds without scaling the daemon down, which is a safer and faster alternative for short manual operations.

Until then, scale-down + scale-up via GitOps is the only supported approach.

---

## Related Documentation

- [ADR-0022 — Robson-Authored Position Invariant](../adr/ADR-0022-robson-authored-position-invariant.md)
- [UNTRACKED-POSITION-RECONCILIATION.md](../policies/UNTRACKED-POSITION-RECONCILIATION.md) — full policy including I1/I2/I3
- [VAL-001 — Testnet E2E Validation](val-001-testnet-e2e-validation.md)
- [VAL-002 — Real Capital Activation](val-002-real-capital-activation.md)
