# CI/CD Image Tagging Strategy

## Overview

This document describes how Docker images are tagged in the Robson CI/CD pipeline and how to promote releases to production using GitOps (ArgoCD).

---

## Tag Types

| Tag Pattern | When Created | Use Case | Example |
|-------------|--------------|----------|---------|
| `sha-<7chars>` | Every push to `main` | **Production (recommended)** | `sha-a1b2c3d` |
| `v<semver>` | Git tag push (`v*`) | Releases, changelogs | `v0.3.0` |
| `latest` | Every push to `main` | **Dev/local only** | `latest` |

### SHA Tags (Golden Standard)

- **Format**: `sha-<first-7-chars-of-commit>`
- **Example**: `sha-a1b2c3d`
- **Purpose**: Immutable, traceable, rollback-friendly
- **When**: Every commit to `main`

**Why SHA is the golden standard:**
- ✅ Immutable (same SHA = same content)
- ✅ Traceable (`git log`, `git show`, `git blame`)
- ✅ Rollback-friendly (revert to any previous SHA)
- ✅ No ambiguity (unlike `:latest`)

### SemVer Tags (Releases)

- **Format**: `v<major>.<minor>.<patch>`
- **Example**: `v0.3.0`, `v1.0.0`
- **Purpose**: Human-readable milestones, changelogs
- **When**: Creating a GitHub Release with a `v*` tag

### Latest Tag (Dev Only)

- **⚠️ WARNING**: Never use `:latest` in production
- **Purpose**: Developer convenience for local testing
- **Problem**: Mutable, unpredictable, breaks rollbacks

---

## Workflow

### 1. Regular Development (Push to main)

```
Developer pushes to main
         │
         ▼
GitHub Actions builds images
         │
         ▼
Tags published:
  - sha-a1b2c3d  ← Use this in prod
  - latest      ← Dev only
```

### 2. Creating a Release

```bash
# 1. Create annotated tag
git tag -a v0.3.0 -m "Release v0.3.0: Add feature X"

# 2. Push tag
git push origin v0.3.0

# 3. (Optional) Create GitHub Release via UI or CLI
gh release create v0.3.0 --title "v0.3.0" --notes "Release notes here"
```

This triggers the workflow with additional tags:

```
Tags published:
  - sha-a1b2c3d  ← Immutable reference
  - v0.3.0      ← Human-readable release
```

---

## Production Deployment (GitOps)

### Golden Rule

**Production manifests in `infra/k8s/prod/` MUST use SHA tags.**

### Promoting an Image to Production

1. **Get the SHA tag** from GitHub Actions summary or commit hash:
   ```bash
   # From git log
   git log --oneline -1
   # Output: a1b2c3d feat: add new feature
   # Tag: sha-a1b2c3d
   ```

2. **Update the manifest** in `infra/k8s/prod/`:
   ```yaml
   # Before
   image: ldamasio/rbs-backend-monolith-prod:sha-OLD_SHA
   
   # After
   image: ldamasio/rbs-backend-monolith-prod:sha-a1b2c3d
   ```

3. **Commit and push**:
   ```bash
   git add infra/k8s/prod/rbs-backend-monolith-prod-deploy.yml
   git commit -m "deploy: promote backend to sha-a1b2c3d"
   git push
   ```

4. **ArgoCD syncs automatically** (or manually via UI/CLI):
   ```bash
   argocd app sync robson-prod
   ```

### Example Diff

```diff
# infra/k8s/prod/rbs-backend-monolith-prod-deploy.yml
 spec:
   containers:
   - name: rbs-backend-monolith-prod-deploy
-    image: ldamasio/rbs-backend-monolith-prod:sha-b2c3d4e
+    image: ldamasio/rbs-backend-monolith-prod:sha-a1b2c3d
```

### Rollback

To rollback, update the tag to a previous SHA:

```bash
# Find previous working SHA
git log --oneline infra/k8s/prod/rbs-backend-monolith-prod-deploy.yml

# Update manifest to previous SHA, commit, push
# ArgoCD syncs automatically
```

---

## Images Reference

| Service | Image Repository | Dockerfile |
|---------|------------------|------------|
| Frontend | `ldamasio/rbs-frontend-prod` | `apps/frontend/docker/Dockerfile` |
| Backend Monolith | `ldamasio/rbs-backend-monolith-prod` | `apps/backend/monolith/docker/Dockerfile_django` |
| Backend Nginx | `ldamasio/rbs-backend-nginx-prod` | `apps/backend/monolith/docker/Dockerfile_nginx` |

---

## Cache Strategy

The workflow uses GitHub Actions cache (`type=gha`) for Docker layer caching:

```yaml
cache-from: type=gha
cache-to: type=gha,mode=max
```

**Requirements**: `permissions: actions: write` in the workflow.

---

## Troubleshooting

### Image not found in Docker Hub

1. Check GitHub Actions run completed successfully
2. Verify the SHA: `git rev-parse --short HEAD`
3. Check Docker Hub: `docker pull ldamasio/rbs-backend-monolith-prod:sha-<sha>`

### Cache not working

1. Verify `permissions: actions: write` is set
2. Check Actions tab for cache hits/misses

### ArgoCD not syncing

1. Verify manifest is valid YAML
2. Check ArgoCD UI for sync errors
3. Force sync: `argocd app sync robson-prod --force`

---

## References

- [GitHub Actions Workflow](.github/workflows/main.yml)
- [Production Manifests](infra/k8s/prod/)
- [ArgoCD Documentation](https://argo-cd.readthedocs.io/)
