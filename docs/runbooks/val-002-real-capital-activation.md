# VAL-002 — Real Capital Activation

**Severity**: Critical
**Time to Execute**: 30–60 min
**Required Access**: `pass`, `kubectl` with production `robson` namespace, Binance real account, `rbx-infra` Ansible and GitOps access

---

## Run Log

| Date | Executor | Result | Notes |
|------|----------|--------|-------|
| 2026-04-22 | GLM-5.1 + psyctl | PASS | Foundation production sha-9448ce20, legacy monitor active, 0 UNTRACKED in 10 minutes |

*Update this table after every execution. VAL-002 must not start until VAL-001 shows `PASS`.*

The 2026-04-22 entry is historical evidence. “legacy monitor active” referred to the
now-superseded fixed-percentage `PositionMonitor`; it is not authorization to
re-enable that runtime path and is not evidence for the retirement rollout.

---

## Purpose

Activate a reviewed production `robsond` image with real Binance credentials,
the legacy fixed-percentage `PositionMonitor` disabled, and the unconditional
ADR-0022 reconciliation worker demonstrably live.

**Blocking prerequisite**: [VAL-001 — Testnet E2E Validation](val-001-testnet-e2e-validation.md) must show `PASS` in its Run Log.

**Activation sequence**:
```text
real Binance keys present → flat-account verification → Ansible secret refresh → production adapter verification → reviewed image via GitOps → reconciliation liveness verification
```

---

## Prerequisites

- VAL-001 Run Log has a `PASS` entry.
- Real Binance API key and API secret are available for the production account.
- `pass` is initialized and writable on the operator workstation.
- `kubectl` can access the production `robson` namespace.
- `rbx-infra` repository is available locally and can run `bootstrap/ansible/`.
- Ansible access is available for the production cluster secret refresh.
- ArgoCD `robson-prod` is configured for auto-sync from `rbx-infra/main`.

If any prerequisite fails, stop here. Do not roll out the production image.

---

## Procedure

### Step 1: Store Real Binance Keys In `pass` ✅ DONE (2026-04-17)

Production Binance credentials are stored at:

```bash
rbx/robson/binance-api-key
rbx/robson/binance-api-secret
```

Verify presence with:
```bash
pass show rbx/robson/binance-api-key >/dev/null
pass show rbx/robson/binance-api-secret >/dev/null
```

**Expected Output**: Both commands exit 0. No secret value printed.

Do not create, revoke, or apply replacement keys until the flat-account check in
Step 2 passes.

### Step 2: Safety Checks Before Any Credential Or Runtime Change

Before rotating credentials, changing the Kubernetes Secret, restarting a pod,
or changing the production image, verify the production namespace has no open
Robson lifecycle positions AND that the real Binance production account holds
no UNTRACKED positions (ADR-0022 — Robson-authored position invariant).

**Command**:
```bash
kubectl port-forward svc/robsond 18080:8080 -n robson
```

In a second terminal:

```bash
curl -s http://localhost:18080/status | jq '.active_positions, .positions'
```

Then query the projection:

```bash
PARADEDB_POD=$(kubectl get pod -n robson -l app.kubernetes.io/name=robson-paradedb -o jsonpath='{.items[0].metadata.name}')
kubectl exec -n robson "$PARADEDB_POD" -- psql -U robson -d robson -c \
  "SELECT position_id, symbol, side, state FROM positions_current WHERE state IN ('armed', 'entering', 'active', 'exiting') ORDER BY updated_at DESC;"
```

Then enumerate every open position on the real Binance production account across
every account type (spot, isolated margin, cross margin, futures) and every symbol.
Cross-check each exchange order id against `event_log` `entry_order_placed`:

```bash
# All non-zero balances (spot + margin) on the operator's Binance account
# All non-zero positions on futures
# ... (use the binance-cli / private tooling available to the operator)
# For each open position, verify an entry_order_placed event exists with a
# matching exchange order id in event_log.
```

**Expected Output**:
```text
/status reports active_positions = 0.
/status reports stale_active_count = 0 and reconciliation_blockers = [].
The positions_current query returns 0 rows.
Zero UNTRACKED positions on the Binance production account.
```

**If this fails**: do not roll out the image. Any UNTRACKED position on the
production Binance account is a P0 block — close it and investigate how it was
opened (was a credential leaked? was a manual order placed on a Robson-operated
account? is a legacy service still active?). See
[UNTRACKED-POSITION-RECONCILIATION.md](../policies/UNTRACKED-POSITION-RECONCILIATION.md).
Repeat the safety checks after the account is clean.

### Step 3: Ansible Secret Source And Kubernetes Secret ✅ DONE (2026-04-17)

`rbx-infra` Ansible is already configured to read from `rbx/robson/`
(`pass_robson_binance_api_key`, `pass_robson_binance_api_secret` in
`bootstrap/ansible/roles/k8s-secrets/defaults/main.yml`). The production
`robsond-secret` in namespace `robson` already contains the keys from
`rbx/robson/`.

To reconcile after a key rotation, only after Step 2 passes:

```bash
cd ~/apps/rbx-infra
bash bootstrap/scripts/init-vault-from-pass.sh
ansible-playbook bootstrap/ansible/site.yml \
  -i bootstrap/ansible/inventory/hosts.yml \
  --tags k8s-secrets
```

Do not restart the Deployment here. The reviewed GitOps image rollout is the
attended restart boundary.

### Step 4: Verify Production Exchange Selection

Verify the current production configuration is not marked as testnet and the
running daemon selected the production Binance adapter. The connector uses
`fapi.binance.com` for production USD-M Futures and
`testnet.binancefuture.com` for testnet, but those URLs are not emitted as a
startup log; validate the explicit adapter-selection log instead.

**Command**:

```bash
kubectl get pods -n robson -l app.kubernetes.io/name=robsond
kubectl logs -n robson deploy/robsond | rg 'Exchange: (Binance \(production\)|Binance \(testnet\)|Stub)'
kubectl get configmap -n robson robsond-config \
  -o jsonpath='{.data.ROBSON_BINANCE_USE_TESTNET}{"\n"}'
```

**Expected Output**:

```text
robsond pod is Running.
Logs contain Exchange: Binance (production).
ROBSON_BINANCE_USE_TESTNET is absent or is not true.
```

**If this fails**: stop before any restart or image rollout. A production
ConfigMap marked as testnet, a testnet adapter-selection log, or a Stub exchange
selection is an abort condition.

### Step 5: Roll Out A Reviewed Image Via GitOps

Keep `ROBSON_POSITION_MONITOR_ENABLED: "false"`. Submit and merge a reviewed
`rbx-infra` change that pins both the `robsond` Deployment and database-migration
Job to the same immutable application image SHA. Never re-enable the legacy flag
as a rollout or rollback mechanism.

Before merging, record the current image SHA and confirm the account is still
flat. After ArgoCD sync, confirm that exactly the reviewed SHA is running.

**Verification commands**:
```bash
cd ~/apps/rbx-infra
rg -n "ROBSON_POSITION_MONITOR_ENABLED|image:" apps/prod/robson
git diff -- apps/prod/robson
git status --short

kubectl get app robson-prod -n argocd \
  -o jsonpath='{.status.sync.status} {.status.health.status}{"\n"}'
kubectl get configmap -n robson robsond-config \
  -o jsonpath='{.data.ROBSON_POSITION_MONITOR_ENABLED}{"\n"}'
kubectl get pods -n robson -l app.kubernetes.io/name=robsond
kubectl get deployment -n robson robsond \
  -o jsonpath='{.spec.template.spec.containers[0].image}{"\n"}'
```

**Expected output**:

```text
ArgoCD is Synced Healthy.
ROBSON_POSITION_MONITOR_ENABLED is false.
The pod is Running and Ready with the reviewed immutable image SHA.
```

**If this fails**: do not force-push or toggle the legacy flag. Follow the
rollback section and preserve `ROBSON_POSITION_MONITOR_ENABLED=false`.

---

## Validation

Verify the rollout succeeded:

- [ ] VAL-001 Run Log has a `PASS` entry.
- [ ] `pass show rbx/robson/binance-api-key` and `pass show rbx/robson/binance-api-secret` both exit 0.
- [ ] Ansible secret refresh completed successfully from `rbx-infra/bootstrap/ansible/`.
- [ ] New production daemon logs contain `Exchange: Binance (production)`, never the testnet or Stub selection, and `ROBSON_BINANCE_USE_TESTNET` is not `true`.
- [ ] Safety checks before rollout showed `active_positions = 0`, `stale_active_count = 0`, empty `reconciliation_blockers`, and no open rows in `positions_current`.
- [ ] **Zero UNTRACKED positions on the production Binance account** (ADR-0022): every open exchange position across all account types and all symbols has a matching `entry_order_placed` event, OR the account is empty.
- [ ] ArgoCD `robson-prod` is `Synced Healthy`.
- [ ] Production ConfigMap has `ROBSON_POSITION_MONITOR_ENABLED: "false"`.
- [ ] Production daemon pod is Running and Ready on the reviewed image SHA with zero unexpected restarts.
- [ ] `/safety/status` reports the disabled legacy contract and `/safety/test` reports `success=false`.
- [ ] Logs contain the runtime-retirement message and do not contain legacy monitor initialization/start messages.
- [ ] `robsond_reconciliation_scans_total{result="completed"}` and both reconciliation timestamps advance across at least one configured interval; the error counter does not advance and the in-progress gauge returns to zero.
- [ ] No unexpected `UNTRACKED position detected`, `UNTRACKED position closed`, or failed-close structured logs appear in the first 10 minutes. These logs are the current signal; durable I2 EventLog records remain follow-up work.

**Command**:
```bash
kubectl get app robson-prod -n argocd -o jsonpath='{.status.sync.status} {.status.health.status}{"\n"}'
kubectl get configmap -n robson robsond-config -o jsonpath='{.data.ROBSON_POSITION_MONITOR_ENABLED}{"\n"}'
kubectl get pods -n robson -l app.kubernetes.io/name=robsond
kubectl logs -n robson deploy/robsond --since=10m \
  | rg "Exchange:|Legacy PositionMonitor|Position monitor initialized|Position monitor started|UNTRACKED|panic"

kubectl port-forward svc/robsond 18080:8080 -n robson
```

In a second terminal, sample the metrics twice with at least one configured
reconciliation interval between samples:

```bash
curl -fsS http://localhost:18080/readyz | jq .
curl -fsS http://localhost:18080/safety/status | jq .
curl -fsS http://localhost:18080/safety/test | jq .
curl -fsS http://localhost:18080/metrics \
  | rg '^robsond_reconciliation_(scans_total|scan_in_progress|last_attempt_timestamp_seconds|last_completed_timestamp_seconds)'
```

---

## Abort Criteria

Stop immediately and rollback if any of these occur:

- VAL-001 does not have a `PASS` Run Log entry.
- Production configuration has `ROBSON_BINANCE_USE_TESTNET=true`, or startup logs select `Exchange: Binance (testnet)` or `Exchange: Stub`.
- Real Binance credentials cannot be verified in `pass`.
- Ansible secret refresh fails or writes the wrong credential source.
- Safety checks show any `armed`, `entering`, `active`, or `exiting` production positions before the rollout.
- **Any UNTRACKED position is found on the production Binance account** (ADR-0022): an open exchange position on any symbol or account type with no matching `entry_order_placed` event. Do not proceed until the account is clean and the root cause is identified.
- ArgoCD sync is degraded after the image change.
- The legacy `/safety/status` contract reports `enabled=true`, or logs show the legacy `PositionMonitor` initialized or started.
- Reconciliation completed scans and timestamps do not advance, the error counter advances, or the in-progress gauge remains stuck.
- Unexpected reconciliation exits or panic events appear after rollout.

---

## Rollback

If production selects the testnet or Stub adapter after the Ansible run:

1. Restore the previous Ansible defaults in `rbx-infra/bootstrap/ansible/`.
2. Re-run the Ansible secret workflow.
3. Keep the account flat and roll only through a reviewed GitOps image/config change.
4. Verify the new pod logs `Exchange: Binance (production)` and the testnet marker is not `true`.

If the reviewed application image causes an issue:

1. Keep `ROBSON_POSITION_MONITOR_ENABLED: "false"` and preserve the pod-template
   config-revision annotation introduced by `rbx-infra` PR #325. Never revert
   that PR wholesale.
2. Submit a reviewed GitOps rollback that pins both the `robsond` Deployment and
   database-migration Job to the last accepted immutable image SHA.
3. Confirm the account is flat, merge the rollback, and wait for ArgoCD auto-sync.
4. Re-run the full validation above, including reconciliation-worker liveness.

A prior image may still contain the legacy code path; keeping the flag false is
therefore a required rollback invariant, not an optional feature toggle.

---

## Related Documentation

- [VAL-001 — Testnet E2E Validation](val-001-testnet-e2e-validation.md)
- [ROBSON v3 — Complete Migration Plan](../architecture/v3-migration-plan.md)
- `rbx-infra/bootstrap/ansible/`
- `rbx-infra/apps/prod/robson/robsond-config.yml`
