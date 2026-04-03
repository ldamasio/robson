# GitOps Guide — Robson

## Architecture Overview

Robson uses a **GitOps deployment model** where all infrastructure is version-controlled and changes are applied automatically.

```
┌──────────────────────────────────────────────────────────────┐
│                         Deployment Flow                       │
└──────────────────────────────────────────────────────────────┘

1. Code Push
   └─▶ git push (robson repo)

2. CI/CD Workflow (.github/workflows/ci.yml)
   ├─▶ Build Docker image
   │   └─▶ Push to ghcr.io/rbxrobotica/robson:sha-XXXXXXX
   │
   └─▶ Update GitOps Manifest
       ├─▶ Clone rbx-infra repo
       ├─▶ Update apps/prod/robson/kustomization.yml
       └─▶ Commit + Push to rbx-infra

3. ArgoCD (running on cluster)
   ├─▶ Detects change in rbx-infra
   ├─▶ Syncs to cluster
   └─▶ Deploys new image

4. Cluster State
   └─▶ Pods running with new image tag
```

## Key Components

| Component | Purpose | Location |
|-----------|---------|----------|
| **Source Code** | Application code and Dockerfiles | `ldamasio/robson` |
| **Container Registry** | Docker images | `ghcr.io/rbxrobotica/robson` |
| **GitOps Repo** | Kubernetes manifests | `rbxrobotica/rbx-infra` |
| **ArgoCD** | Continuous deployment | Runs in k3s cluster |

## Deployment Process

### Automatic Deployment (Normal Flow)

```bash
# 1. Make changes
git add .
git commit -m "feat: new feature"
git push

# 2. CI/CD runs automatically
# - Builds image: ghcr.io/rbxrobotica/robson:sha-abc1234
# - Updates rbx-infra/apps/prod/robson/kustomization.yml
# - Commits with message: "chore(robson): update image tags to sha-abc1234"

# 3. ArgoCD syncs within 3-5 minutes
# - Detects change in rbx-infra
# - Applies to cluster
# - New pods are created

# 4. Verify deployment
kubectl get pods -n robson
# Should show: ghcr.io/rbxrobotica/robson:sha-abc1234
```

### Manual Deployment (Emergency)

If CI/CD fails or you need immediate deployment:

```bash
# 1. Clone rbx-infra
git clone git@github.com:rbxrobotica/rbx-infra.git
cd rbx-infra

# 2. Update image tag manually
# Edit apps/prod/robson/kustomization.yml
# Change newTag: sha-old123 to newTag: sha-new456

# 3. Commit and push
git add apps/prod/robson/kustomization.yml
git commit -m "chore(robson): emergency deploy to sha-new456"
git push

# 4. Wait for ArgoCD to sync (or force sync)
kubectl patch application robson -n argocd --type merge -p '{"operation":{"sync":{"revision":"HEAD"}}}'
```

## Monitoring

### Check CI/CD Status

```bash
# List recent workflow runs
curl -s https://api.github.com/repos/ldamasio/robson/actions/runs?per_page=5 | \
  grep -E '"status"|"conclusion"|"created_at"'

# Or use gh CLI (if installed)
gh run list --repo ldamasio/robson --limit 5
```

### Check ArgoCD Status

```bash
# Check application status
kubectl get application robson -n argocd -o jsonpath='{.status.sync.status} | {.status.health.status}'
# Expected: Synced | Healthy

# Get detailed status
kubectl describe application robson -n argocd

# Check what image is deployed
kubectl get pods -n robson -o jsonpath='{.items[0].spec.containers[0].image}'
```

### Check Running Pods

```bash
# List all robson pods
kubectl get pods -n robson -o wide

# Check specific deployment
kubectl get deployment -n robson -o wide

# Get pod logs
kubectl logs -n robson -l app.kubernetes.io/name=robson --tail=50
```

## Troubleshooting

### CI/CD Build Fails

**Problem**: Workflow fails before image is built.

**Solution**: Fix code issues and push again. No manual intervention needed.

```bash
# Check workflow logs
gh run view <run-id> --log

# Fix issues locally
git commit -m "fix: resolve build error"
git push
```

### Manifest Update Fails

**Problem**: Image built but rbx-infra update fails.

**Symptoms**:
- Workflow completes with errors
- Image exists in GHCR
- But `kustomization.yml` not updated

**Solution**: Manual update (see Manual Deployment above)

```bash
# 1. Get the SHA from the failed run
gh run view <run-id> --log | grep "sha-"

# 2. Manually update rbx-infra (see manual deployment section)
```

### ArgoCD Not Syncing

**Problem**: Manifest updated but cluster not deploying.

**Check**:
```bash
# Check ArgoCD application status
kubectl get application robson -n argocd

# Check for sync errors
kubectl get application robson -n argocd -o yaml | grep -A 10 status
```

**Solutions**:

**A. Force manual sync**
```bash
kubectl patch application robson -n argocd --type merge -p '{"operation":{"sync":{"revision":"HEAD"}}}'
```

**B. Check ArgoCD logs**
```bash
kubectl logs -n argocd -l app.kubernetes.io/name=argocd-application-controller
```

**C. Restart ArgoCD**
```bash
kubectl rollout restart deployment argocd-application-controller -n argocd
```

### Rollback to Previous Version

```bash
# 1. Find previous SHA
cd rbx-infra
git log apps/prod/robson/kustomization.yml --oneline | head -5

# 2. Revert to previous commit
git revert <commit-sha>
git push

# 3. ArgoCD will automatically deploy the old version
```

## Best Practices

### ✅ Do

- Let CI/CD handle all deployments
- Wait for ArgoCD to sync (3-5 minutes)
- Check ArgoCD status before manual intervention
- Use semantic commit messages
- Keep manifests in rbx-infra repo

### ❌ Don't

- Don't use `kubectl apply` directly (bypasses GitOps)
- Don't edit manifests in robson repo (they don't exist there anymore)
- Don't force-push to rbx-infra (breaks ArgoCD)
- Don't skip CI/CD for production deploys
- Don't use `docker push` manually (use CI/CD)

## Security

### Container Registry Access

Images are pushed to **GitHub Container Registry (GHCR)**:
- Authentication: `GITHUB_TOKEN` (automatic in CI/CD)
- Visibility: Public (read), private (write)
- URL: `ghcr.io/rbxrobotica/robson`

### GitOps Repository Access

Updates to `rbx-infra` require:
- SSH deploy key: `INFRA_DEPLOY_KEY` (stored in GitHub Secrets)
- Scoped to: `rbxrobotica/rbx-infra` (write access)
- Used by: CI/CD workflow only

### Cluster Access

ArgoCD uses:
- In-cluster service account
- RBAC permissions for `robson` namespace
- TLS certificates managed by cert-manager

## Related Documentation

- [Container Registry Standard](/home/psyctl/apps/rbx-infra/docs/CONTAINER-REGISTRY.md)
- [ArgoCD Best Practices](/home/psyctl/apps/rbx-infra/docs/ARGOCD-BEST-PRACTICES.md)
- [Infrastructure Repository](https://github.com/rbxrobotica/rbx-infra)

---

**Last Updated**: 2026-04-03
**Supersedes**: the removed legacy GitOps recovery guide
