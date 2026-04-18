# VAL-002 — Real Capital Activation

**Severity**: Critical
**Time to Execute**: 30–60 min
**Required Access**: `pass`, `kubectl` with production `robson` namespace, Binance real account, `rbx-infra` Ansible and GitOps access

---

## Run Log

| Date | Executor | Result | Notes |
|------|----------|--------|-------|
| — | — | blocked | Requires VAL-001 PASS before first execution |

*Update this table after every execution. VAL-002 must not start until VAL-001 shows `PASS`.*

---

## Purpose

Activate the production `robsond` daemon for real Binance credentials and enable the production position monitor only after the testnet lifecycle validation has passed.

**Blocking prerequisite**: [VAL-001 — Testnet E2E Validation](val-001-testnet-e2e-validation.md) must show `PASS` in its Run Log.

**Activation sequence**:
```text
real Binance keys in pass → Ansible secret refresh → production endpoint verification → monitor enabled by GitOps
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

If any prerequisite fails, stop here. Do not enable the production monitor.

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

### Step 2: Ansible Secret Source And Kubernetes Secret ✅ DONE (2026-04-17)

`rbx-infra` Ansible is already configured to read from `rbx/robson/` (`pass_robson_binance_api_key`, `pass_robson_binance_api_secret` in `bootstrap/ansible/roles/k8s-secrets/defaults/main.yml`). The production `robsond-secret` in namespace `robson` already contains the keys from `rbx/robson/`.

To reconcile after any key rotation:
```bash
cd ~/apps/rbx-infra
bash bootstrap/scripts/init-vault-from-pass.sh
ansible-playbook bootstrap/ansible/site.yml \
  -i bootstrap/ansible/inventory/hosts.yml \
  --tags k8s-secrets
kubectl rollout restart deployment/robsond -n robson
```

### Step 3: Verify Production Connects To Real Binance

Restart or roll the production daemon only if the secret refresh does not automatically restart it. Then verify the daemon is connected to real Binance, not testnet.

**Command**:
```bash
kubectl get pods -n robson -l app.kubernetes.io/name=robsond
kubectl logs -n robson deploy/robsond --since=10m | grep -E "api.binance.com|testnet.binance.vision|Binance|testnet"
kubectl get configmap -n robson robsond-config -o yaml | grep ROBSON_BINANCE_USE_TESTNET || true
```

**Expected Output**:
```text
robsond pod is Running.
Logs indicate production Binance endpoint usage: api.binance.com.
No production configuration routes robsond to testnet.binance.vision.
```

**If this fails**: rollback immediately. Production connecting to `testnet.binance.vision` after the Ansible run is an abort condition.

### Safety Checks Before Flip

Before enabling the production position monitor, verify the production namespace has no open Robson lifecycle positions.

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

**Expected Output**:
```text
/status reports active_positions = 0.
The positions_current query returns 0 rows.
```

**If this fails**: do not enable the monitor. Close or reconcile open positions first and repeat the safety checks.

### Step 4: Enable Production Position Monitor Via GitOps

Change `ROBSON_POSITION_MONITOR_ENABLED` to `"true"` in `rbx-infra/apps/prod/robson/robsond-config.yml`, commit, and push to `rbx-infra/main`.

**Command**:
```bash
cd ~/apps/rbx-infra
rg -n "ROBSON_POSITION_MONITOR_ENABLED" apps/prod/robson/robsond-config.yml
git diff -- apps/prod/robson/robsond-config.yml
git status --short
git add apps/prod/robson/robsond-config.yml
git commit -m "Enable robson production position monitor"
git push
```

**Expected Output**:
```text
Only apps/prod/robson/robsond-config.yml changes.
ROBSON_POSITION_MONITOR_ENABLED is "true".
Push succeeds and ArgoCD auto-sync begins.
```

**If this fails**: do not force-push. Restore `ROBSON_POSITION_MONITOR_ENABLED: "false"` if the commit was partially applied.

---

## Validation

Verify the activation succeeded:

- [ ] VAL-001 Run Log has a `PASS` entry.
- [ ] `pass show rbx/robson/binance-api-key` and `pass show rbx/robson/binance-api-secret` both exit 0.
- [ ] Ansible secret refresh completed successfully from `rbx-infra/bootstrap/ansible/`.
- [ ] Production daemon logs indicate `api.binance.com`, not `testnet.binance.vision`.
- [ ] Safety checks before flip showed `active_positions = 0` and no open rows in `positions_current`.
- [ ] ArgoCD `robson-prod` is `Synced Healthy`.
- [ ] Production ConfigMap has `ROBSON_POSITION_MONITOR_ENABLED: "true"`.
- [ ] Production daemon pod is Running after the GitOps sync.
- [ ] No unexpected Safety Net exit or panic events appear after monitor activation.

**Command**:
```bash
kubectl get app robson-prod -n argocd -o jsonpath='{.status.sync.status} {.status.health.status}{"\n"}'
kubectl get configmap -n robson robsond-config -o jsonpath='{.data.ROBSON_POSITION_MONITOR_ENABLED}{"\n"}'
kubectl logs -n robson deploy/robsond --since=10m | grep -E "Position monitor|api.binance.com|testnet.binance.vision|Safety"
```

---

## Abort Criteria

Stop immediately and rollback if any of these occur:

- VAL-001 does not have a `PASS` Run Log entry.
- Production daemon connects to `testnet.binance.vision` after the Ansible run.
- Real Binance credentials cannot be verified in `pass`.
- Ansible secret refresh fails or writes the wrong credential source.
- Safety checks show any `armed`, `entering`, `active`, or `exiting` production positions before the monitor flip.
- ArgoCD sync is degraded after the monitor change.
- The monitor emits unexpected Safety Net exit or panic events immediately after activation.

---

## Rollback

If production connects to the testnet endpoint after the Ansible run:

1. Restore the previous Ansible defaults in `rbx-infra/bootstrap/ansible/`.
2. Re-run the Ansible secret workflow.
3. Restart or roll `deploy/robsond` in namespace `robson` if required.
4. Verify logs no longer show `testnet.binance.vision`.

If monitor activation causes an issue:

1. Set `ROBSON_POSITION_MONITOR_ENABLED: "false"` in `rbx-infra/apps/prod/robson/robsond-config.yml`.
2. Commit and push the rollback to `rbx-infra/main`.
3. Wait for ArgoCD auto-sync.
4. Verify the production ConfigMap and daemon logs.

**Command**:
```bash
cd ~/apps/rbx-infra
git diff -- apps/prod/robson/robsond-config.yml
git add apps/prod/robson/robsond-config.yml
git commit -m "Disable robson production position monitor"
git push
kubectl get app robson-prod -n argocd -o jsonpath='{.status.sync.status} {.status.health.status}{"\n"}'
```

---

## Related Documentation

- [VAL-001 — Testnet E2E Validation](val-001-testnet-e2e-validation.md)
- [ROBSON v3 — Complete Migration Plan](../architecture/v3-migration-plan.md)
- `rbx-infra/bootstrap/ansible/`
- `rbx-infra/apps/prod/robson/robsond-config.yml`
