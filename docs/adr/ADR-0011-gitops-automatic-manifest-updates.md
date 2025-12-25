# ADR-0011: GitOps Automatic Manifest Updates

**Status**: Accepted
**Date**: 2024-12-24
**Deciders**: Robson Bot Core Team

## Context

Previously, deploying to production required manual intervention:

1. GitHub Actions built images with SHA tags (e.g., `sha-a1b2c3d`)
2. **MANUAL**: Developer had to update `infra/k8s/prod/*.yml` with new SHA
3. **MANUAL**: Developer had to commit and push the manifest changes
4. ArgoCD would then sync automatically

This manual step was:
- **Error-prone**: Developers could forget to update manifests
- **Time-consuming**: Added 5-10 minutes to deployment process
- **Inconsistent**: Sometimes only some manifests were updated

## Decision

Implement **automatic manifest updates** in the GitHub Actions CI/CD pipeline:

1. After building images, the pipeline uses `sed` to update image tags in manifests
2. Pipeline commits the changes with `[skip ci]` to prevent infinite loops
3. ArgoCD detects the manifest change and syncs automatically
4. Optional: Pipeline waits for ArgoCD sync and runs smoke tests

### Implementation Details

```yaml
# .github/workflows/main.yml (excerpt)

- name: Update K8s manifests with new image tags
  run: |
    SHA_TAG="${{ steps.meta.outputs.sha_tag }}"
    
    # Update all deployment manifests
    sed -i "s|image: ldamasio/rbs-frontend-prod:sha-[a-f0-9]*|...:${SHA_TAG}|g" \
      infra/k8s/prod/rbs-frontend-prod-deploy.yml
    # ... repeat for backend-monolith, backend-nginx, stop-monitor

- name: Commit and push manifest changes
  run: |
    git config --global user.name "github-actions[bot]"
    git config --global user.email "github-actions[bot]@users.noreply.github.com"
    git add infra/k8s/prod/*.yml
    git commit -m "chore(gitops): update image tags to ${SHA_TAG} [skip ci]"
    git push
```

## Alternatives Considered

### 1. ArgoCD Image Updater
- **Pros**: Dedicated tool for this purpose, supports multiple registries
- **Cons**: Additional infrastructure, different auth mechanism, learning curve
- **Why not chosen**: Adds complexity; `sed` in CI is simpler for our scale

### 2. Helm with Values Files
- **Pros**: Standard Kubernetes tooling, well-documented
- **Cons**: Requires Helm charts, adds abstraction layer
- **Why not chosen**: We use raw manifests; migration to Helm is separate decision

### 3. Kustomize Image Transformers
- **Pros**: Native Kubernetes tool, declarative
- **Cons**: Requires Kustomize adoption, changes directory structure
- **Why not chosen**: Similar to Helm - good option but migration is separate

### 4. Keep Manual Updates
- **Pros**: Full control, explicit audit trail
- **Cons**: Slow, error-prone, blocks automation
- **Why not chosen**: Does not scale, creates bottleneck

## Consequences

### Positive

- ✅ **Zero-touch deployments**: Push to main = deploy to production
- ✅ **Faster deployments**: Reduced from ~15 min to ~6-10 min
- ✅ **Reduced errors**: No manual copy-paste of SHA tags
- ✅ **Full GitOps**: All changes tracked in Git (by bot, with clear attribution)
- ✅ **Smoke tests**: Pipeline verifies production health after deploy

### Negative

- ⚠️ **Bot commits**: Commit history includes bot-generated commits
- ⚠️ **Rollback complexity**: Must identify correct SHA to rollback to
- ⚠️ **Requires `contents: write`**: Workflow needs write permission to repo

### Mitigations

- **Bot commits**: Clear commit message format, `[skip ci]` prevents noise
- **Rollback**: `git log infra/k8s/prod/` shows history of SHA changes
- **Permissions**: Scoped to only pushing to `infra/k8s/prod/`

## Deployment Flow (After This ADR)

```
Push to main
    │
    ▼
GitHub Actions: Build images (sha-XXXXXX)
    │
    ▼
GitHub Actions: Update infra/k8s/prod/*.yml
    │
    ▼
GitHub Actions: Commit + Push [skip ci]
    │
    ▼
ArgoCD: Detect manifest change (< 3 min)
    │
    ▼
ArgoCD: Apply changes (rolling update)
    │
    ▼
GitHub Actions: Smoke test production
    │
    ▼
✅ Deploy complete
```

## Related Documents

- [CI/CD Image Tagging Runbook](../runbooks/ci-cd-image-tagging.md)
- [ArgoCD Setup Runbook](../runbooks/argocd-initial-setup.md)
- [ADR-0004: GitOps Preview Environments](ADR-0004-gitops-preview-envs.md)

## References

- [GitHub Actions: Pushing to protected branches](https://docs.github.com/en/actions/using-workflows/workflow-syntax-for-github-actions#permissions)
- [ArgoCD Auto-Sync](https://argo-cd.readthedocs.io/en/stable/user-guide/auto_sync/)
- [GitOps Principles](https://www.gitops.tech/)

