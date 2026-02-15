# GitOps Phase 3: Validation Checklist

**Date**: 2026-02-15
**Scope**: AppProject activation, root App-of-Apps restructure, retry policies

---

## Pre-Merge Validation (Review Only)

### 1. Verify AppProject Permissions

For each Application, confirm the target AppProject allows:

| Application | Project | sourceRepo | dest namespace | cluster resources |
|---|---|---|---|---|
| `istio-ambient` | rbx-platform | github.com/ldamasio/robson | argocd | CRDs, Webhooks |
| `gateway-api-crds` | rbx-platform | github.com/ldamasio/robson | argocd | CRDs |
| `platform-cert-manager` | rbx-platform | github.com/ldamasio/robson | argocd | ClusterIssuer |
| `robson-backend` | rbx-applications | github.com/ldamasio/robson | robson | ClusterIssuer |
| `robson-frontend` | rbx-applications | github.com/ldamasio/robson | robson | none |
| `robson-prod` | rbx-applications | github.com/ldamasio/robson | robson | ClusterIssuer |
| `dns-*` | rbx-applications | github.com/ldamasio/robson | dns | none |
| preview apps | rbx-previews | github.com/ldamasio/robson | h-* | Namespace |

### 2. Verify No Circular Dependencies

- `rbx-projects` Application uses `project: default` (always exists)
- `robson-root` Application uses `project: default` (always exists)
- `platform-argocd` Application uses `project: default` (no ArgoCD self-mgmt risk)
- All other Applications depend on AppProjects created by `rbx-projects`
- Retry policies (limit: 5, backoff 10s→3m) provide ordering tolerance

### 3. Verify Split Consistency

All child Applications from the old monolithic `root.yml` must appear as
individual files in `infra/k8s/gitops/app-of-apps/`. No Application name
has changed - ArgoCD treats this as an update, not a create+delete.

---

## Post-Merge Validation

### Immediate (0-5 minutes after merge)

```bash
# 1. Verify AppProjects were created
kubectl get appprojects -n argocd
# Expected: default, rbx-platform, rbx-applications, rbx-previews

# 2. List all Applications and their sync status
argocd app list
# Expected: All Applications in Synced/Healthy state

# 3. Verify project assignments
argocd app list -o wide
# Check the PROJECT column matches the mapping table above

# 4. Check for permission errors in any Application
argocd app get robson-prod --show-operation
argocd app get istio-ambient --show-operation
argocd app get robson-backend --show-operation
argocd app get dns-infrastructure-metallb --show-operation

# 5. Verify production pods are running
kubectl get pods -n robson
kubectl get pods -n argocd
```

### Extended (5-15 minutes)

```bash
# 6. Verify retry policies are working (check sync history)
argocd app get robson-root --show-operation

# 7. Check for any degraded Applications
argocd app list --status Degraded
argocd app list --status Unknown

# 8. Verify ApplicationSet is generating preview apps correctly
kubectl get applicationsets -n argocd
argocd app list -l robson/preview=true

# 9. Check ArgoCD logs for errors
kubectl logs -n argocd -l app.kubernetes.io/name=argocd-application-controller --tail=100
```

---

## Detecting Common Issues

### Forbidden sourceRepo

**Symptom**: Application shows `ComparisonError` with message like:
```
application repo https://... is not permitted in project rbx-applications
```

**Diagnosis**:
```bash
argocd proj get rbx-applications -o yaml | grep -A 20 sourceRepos
```

**Fix**: Add the missing repo to the AppProject's `sourceRepos` list.

### Forbidden Destination Namespace

**Symptom**: Application shows `ComparisonError` with message like:
```
application destination {namespace} is not permitted in project rbx-applications
```

**Diagnosis**:
```bash
argocd proj get rbx-applications -o yaml | grep -A 20 destinations
```

**Fix**: Add the missing namespace to the AppProject's `destinations` list.

### Forbidden Cluster Resource

**Symptom**: Sync fails with:
```
resource ClusterIssuer is not permitted in project rbx-applications
```

**Diagnosis**:
```bash
argocd proj get rbx-applications -o yaml | grep -A 20 clusterResourceWhitelist
```

**Fix**: Add the resource kind to the AppProject's `clusterResourceWhitelist`.

### Project Not Found

**Symptom**: Application shows error:
```
application references project rbx-platform which does not exist
```

**Cause**: The `rbx-projects` Application hasn't synced yet.

**Fix**: Wait for retry (up to 3 minutes with backoff). If persistent:
```bash
argocd app sync rbx-projects
```

---

## Rollback Procedure

### Quick Rollback (Git Revert)

```bash
# Revert the merge commit
git revert HEAD
git push origin main

# ArgoCD will auto-sync within 3 minutes, restoring:
# - All Applications back to project: default
# - Monolithic root.yml
# - Original standalone Application files
```

### Manual Rollback (Emergency)

If ArgoCD is unresponsive or in a bad state:

```bash
# 1. Force-set all Applications back to default project
for app in $(argocd app list -o name); do
  argocd app set "$app" --project default
done

# 2. Delete AppProjects (optional, safe - they're permissive not restrictive)
kubectl delete appproject rbx-platform rbx-applications rbx-previews -n argocd

# 3. Verify all Applications are syncing
argocd app list
```

### Nuclear Option (ArgoCD Lockout Recovery)

If the ArgoCD UI is inaccessible and CLI fails:

```bash
# 1. Direct kubectl edit to reset project
kubectl edit application robson-root -n argocd
# Change spec.project to "default"

# 2. Restart ArgoCD controllers
kubectl rollout restart deployment argocd-application-controller -n argocd
kubectl rollout restart deployment argocd-server -n argocd

# 3. Wait for reconciliation
kubectl get applications -n argocd -w
```

---

## Project Assignment Reference

| Application | Project | Rationale |
|---|---|---|
| `robson-root` | default | Root must stay in default to avoid lockout |
| `rbx-projects` | default | Manages AppProjects, avoids circular dependency |
| `platform-argocd` | default | ArgoCD self-management, deferred to future migration |
| `istio-ambient` | rbx-platform | Platform infrastructure |
| `gateway-api-crds` | rbx-platform | Platform infrastructure |
| `platform-cert-manager` | rbx-platform | Platform infrastructure |
| `robson-branch-previews` | rbx-platform | Manages ApplicationSet (platform concern) |
| `robson-backend` | rbx-applications | Product application |
| `robson-frontend` | rbx-applications | Product application |
| `robson-prod` | rbx-applications | Product application (production manifests) |
| `dns-infrastructure-metallb` | rbx-applications | Product application (DNS) |
| `dns-infrastructure-nodeport` | rbx-applications | Product application (DNS) |
| Preview apps (generated) | rbx-previews | Ephemeral branch environments |
