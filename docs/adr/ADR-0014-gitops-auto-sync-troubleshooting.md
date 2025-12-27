# ADR-0012: GitOps Auto-Sync Troubleshooting

## Status

Accepted

## Context

**Date**: 2025-12-26
**Issue**: ArgoCD auto-sync failed to deploy portfolio feature despite successful GitOps workflow

## Problem

### Incident Timeline

1. **16:03** - Portfolio feature committed locally (57383193)
2. **17:06** - GitHub Actions build completed, created gitops commit (ca9c2aff)
3. **17:10** - GitOps manifests updated to `sha-6ab2f28`
4. **17:11-19:00** - ArgoCD **did not sync automatically**
5. **19:00** - Manual intervention required via `kubectl apply`

### Root Cause Analysis

**ArgoCD Application Status**: `OutOfSync`

**Cluster State**:
```
Frontend: ldamasio/rbs-frontend-prod:sha-cffd06f (OLD)
Git:      ldamasio/rbs-frontend-prod:sha-6ab2f28 (NEW)
```

**Verified Facts**:
- ✅ GitHub Actions workflow completed successfully
- ✅ Docker images built with SHA tags
- ✅ Manifests committed to git repository
- ✅ ArgoCD Application configured with `automated.prune: true` and `selfHeal: true`
- ❌ **ArgoCD did not trigger automatic sync**

### Possible Causes

1. **GitHub webhook expired or misconfigured**
2. **ArgoCD auth token expired** (observed: `invalid session: Token is expired`)
3. **Webhook payload not reaching ArgoCD**
4. **ArgoCD controller not processing webhook events**

## Decision

### Immediate Fix (Manual Sync)

When auto-sync fails, manually apply manifests:

```bash
# Apply all production manifests
ssh root@158.220.116.31 "kubectl apply -f -" < infra/k8s/prod/rbs-frontend-prod-deploy.yml
ssh root@158.220.116.31 "kubectl apply -f -" < infra/k8s/prod/rbs-backend-monolith-prod-deploy.yml
ssh root@158.220.116.31 "kubectl apply -f -" < infra/k8s/prod/rbs-backend-nginx-prod-deploy.yml
ssh root@158.220.116.31 "kubectl apply -f -" < infra/k8s/prod/rbs-stop-monitor-cronjob.yml

# Wait for rollouts
kubectl -n robson rollout status deployment rbs-frontend-prod-deploy
kubectl -n robson rollout status deployment rbs-backend-monolith-prod-deploy
```

### Long-term Fixes

#### 1. Verify GitHub Webhook

```bash
# Check webhook in GitHub repository
gh repo view ldamasio/robson --webhook
# or via GitHub UI: Settings → Webhooks → ArgoCD
```

Expected webhook URL:
```
https://argocd.robson.rbx.ia.br/api/webhook
```

#### 2. Renew ArgoCD Auth Token

```bash
# Login to ArgoCD
argocd login argocd.robson.rbx.ia.br --username admin --password <password>

# Generate new auth token
argocd account generate-token --account <service-account>
```

#### 3. Test Webhook Delivery

```bash
# Trigger webhook manually
curl -X POST https://argocd.robson.rbx.ia.br/api/webhook \
  -H "Content-Type: application/json" \
  -d '{"repository": {"url": "https://github.com/ldamasio/robson"}}'
```

#### 4. Monitor ArgoCD Controller Logs

```bash
# Check ArgoCD controller logs for webhook processing
kubectl -n argocd logs -l app.kubernetes.io/name=argocd-application-controller
```

### Detection and Monitoring

#### Add Pre-deploy Check

Update `.github/workflows/main.yml` to verify ArgoCD sync:

```yaml
- name: Verify ArgoCD sync
  run: |
    # Wait for ArgoCD to detect changes
    sleep 30

    # Check sync status
    SYNC_STATUS=$(argocd app get robson-prod --output json | jq -r '.status.sync.status')

    if [ "$SYNC_STATUS" != "Synced" ]; then
      echo "❌ ArgoCD not synced: $SYNC_STATUS"
      argocd app get robson-prod
      exit 1
    fi

    echo "✅ ArgoCD synced successfully"
```

#### Add Monitoring Alert

Create alert in ArgoCD or Prometheus to detect `OutOfSync` status:

```yaml
# Example Prometheus rule
- alert: ArgoCDAppOutOfSync
  expr: argocd_app_info{sync_status="OutOfSync"} == 1
  for: 5m
  labels:
    severity: warning
  annotations:
    summary: "ArgoCD app {{ $labels.name }} is out of sync"
```

## Consequences

### Positive

- **Manual sync works** as reliable fallback
- SHA-based image tagging ensures immutable deployments
- ArgoCD Application configuration is correct (auto-sync enabled)

### Negative

- **Auto-sync failure requires manual intervention**
- Deploy delayed by ~2 hours due to sync failure
- No automatic detection/alerting of sync failures

### Risks

- If manual sync becomes routine, team may ignore underlying issue
- Webhook failures may go undetected without monitoring

## Implementation

### Tasks

1. **[ ] Verify GitHub webhook configuration** (high priority)
2. **[ ] Renew ArgoCD auth tokens** (high priority)
3. **[ ] Add webhook delivery monitoring** (medium priority)
4. **[ ] Create alert for OutOfSync status** (medium priority)
5. **[ ] Update GitHub Actions workflow with sync verification** (low priority)
6. **[ ] Document manual sync procedure in runbooks** (low priority)

### Testing

After implementing fixes, test auto-sync:

```bash
# 1. Make a trivial change to manifests
sed -i 's/last-rollout: .*/last-rollout: "'$(date -Iseconds)'"/' infra/k8s/prod/rbs-frontend-prod-deploy.yml

# 2. Commit and push
git add infra/k8s/prod/rbs-frontend-prod-deploy.yml
git commit -m "test: trigger argocd sync"
git push origin main

# 3. Monitor ArgoCD sync
watch -n 5 'argocd app get robson-prod'
```

Expected: ArgoCD should sync within 30 seconds.

## References

- [ADR-0011: GitOps Automatic Manifest Updates](ADR-0011-gitops-automatic-manifest-updates.md)
- [ArgoCD Webhook Configuration](https://argo-cd.readthedocs.io/en/stable/operator-manual/webhook/)
- [GitHub Actions GitOps Workflow](../../.github/workflows/main.yml)
