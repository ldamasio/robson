# GitOps Recovery Guide

**Problem**: GitHub Actions builds Docker images but fails to update K8s manifests.
**Solution**: Automated recovery with robust retry logic and sync checker.

---

## Quick Recovery (Current Issue)

### Immediate Fix for sha-5b09d68

The commit `5b09d681` has images built but manifests not updated.

**Option A: Automated Recovery (Recommended)**
```bash
# Trigger the sync checker workflow manually
gh workflow run gitops-sync-checker.yml

# Check status
gh run list --workflow=gitops-sync-checker.yml

# Wait ~30 seconds, then verify
git pull
git log --oneline -3
# Should see: chore(gitops): recover missing manifest update to sha-5b09d68
```

**Option B: Manual Fix (If workflow not available)**
```bash
# Update manifests locally
sed -i 's/sha-7d6dc25/sha-5b09d68/g' infra/k8s/prod/*.yml

# Commit and push
git add infra/k8s/prod/*.yml
git commit -m "chore(gitops): recover missing manifest update to sha-5b09d68

Manual recovery for failed gitops workflow.

[skip ci]"

git push origin main
```

### Verify Fix on Cluster

```bash
# SSH to k3s node
ssh root@<vps-ip>

# Check ArgoCD sync
argocd app get robson-prod --grpc-web | grep -A 3 Sync

# Force sync if needed
argocd app sync robson-prod --grpc-web

# Verify deployments
kubectl get deployment -n robson -o wide | grep rbs-
# Should show: sha-5b09d68 for all pods

# Check frontend version
curl -s https://app.robson.rbx.ia.br | grep -o 'Build: sha-[a-f0-9]*'
# Should show: Build: sha-5b09d68
```

---

## Root Cause Analysis

### What Happened

1. ✅ Commit `5b09d681` pushed to main (2025-12-31 18:17:29)
2. ✅ GitHub Actions workflow triggered
3. ✅ Docker images built successfully:
   - `ldamasio/rbs-frontend-prod:sha-5b09d68`
   - `ldamasio/rbs-backend-monolith-prod:sha-5b09d68`
   - `ldamasio/rbs-backend-nginx-prod:sha-5b09d68`
4. ❌ **Git push of manifest updates FAILED**
5. ⚠️ Workflow continued without error (no validation)

### Why It Failed

**Race condition in gitops push step:**

```yaml
# OLD CODE (line 190 of main.yml)
git push  # ← Can fail silently if remote changed!
```

When multiple commits are pushed quickly:
- Workflow A starts for commit X
- Workflow B starts for commit Y (cancels A due to concurrency group)
- Workflow B pushes gitops commit successfully
- Workflow C starts for commit Z
- Workflow C's `git push` fails (remote changed, needs rebase)
- **No retry, no error handling** → GitOps broken

### Pattern of Failures

Found **5+ missing gitops commits** in recent history:
- `5b09d681` (sha-5b09d68) ← Current issue
- `621e056b` (sha-621e056)
- `27a97988` (sha-27a9799)
- `e6894ad9` (sha-e6894ad)
- `cef122a6` (sha-cef122a)

This is a **systemic issue**, not a one-off failure.

---

## Durable Solution Implemented

### 1. Robust GitOps Push (main.yml)

**Before:**
```yaml
git push  # Fails silently on conflicts
```

**After (with retry logic):**
```yaml
# Retry up to 3 times with exponential backoff
while [ $RETRY_COUNT -lt $MAX_RETRIES ]; do
  if git push origin main; then
    echo "✅ Success"
    break
  else
    echo "⚠️ Push failed, pulling and retrying..."
    git pull --rebase origin main
    sleep $((2 ** RETRY_COUNT))
  fi
done

# Final validation: Verify commit exists on remote
git ls-remote --heads origin main | grep -q "$(git rev-parse HEAD)"
```

**New safeguards:**
- ✅ Retry with pull/rebase before each attempt
- ✅ Exponential backoff (2s, 4s, 8s)
- ✅ Idempotent (checks if another workflow already updated)
- ✅ Final validation (confirms gitops commit on remote)
- ✅ Fails the build if push fails (no silent failures)

### 2. GitOps Sync Checker (gitops-sync-checker.yml)

**New scheduled workflow** that acts as a safety net:

**What it does:**
1. Checks DockerHub for latest image tags
2. Compares with K8s manifest tags
3. Detects gitops gaps (images exist, manifests outdated)
4. Automatically fixes by updating manifests and pushing

**Triggers:**
- **Automated**: Every 6 hours (cron)
- **Manual**: Via GitHub Actions UI (workflow_dispatch)

**Usage:**
```bash
# Run immediately (dry run)
gh workflow run gitops-sync-checker.yml -f dry_run=true

# Run immediately (apply fixes)
gh workflow run gitops-sync-checker.yml -f dry_run=false

# Check results
gh run list --workflow=gitops-sync-checker.yml
gh run view <run-id>
```

---

## Testing the Solution

### Simulate Race Condition

```bash
# Terminal 1: Push commit A
git commit -m "feat: test A" --allow-empty
git push

# Terminal 2: Immediately push commit B (within 5 seconds)
git commit -m "feat: test B" --allow-empty
git push

# Check GitHub Actions
gh run list --workflow=main.yml
# Both should show "completed" with green checkmarks

# Verify both gitops commits exist
git pull
git log --oneline --grep="chore(gitops)" | head -5
# Should see gitops commits for both test A and test B
```

### Verify Sync Checker

```bash
# Manually trigger sync checker
gh workflow run gitops-sync-checker.yml -f dry_run=false

# Wait ~30 seconds
gh run watch

# Check results
gh run view --log
# Should show: "✅ All In Sync" or "🚨 Gap Detected and Fixed"
```

---

## Monitoring and Alerts

### GitHub Actions Annotations

The workflows now use `::error::` annotations:

```yaml
echo "::error::GitOps manifest push failed - deployment NOT updated"
```

These show up as:
- ❌ Red X on failed runs
- 🔴 Error annotations in run logs
- 📧 Email notifications (if configured)

### Check for Failures

```bash
# List recent failures
gh run list --workflow=main.yml --status=failure

# View failure details
gh run view <failed-run-id> --log

# Check for error annotations
gh run view <run-id> --log | grep "::error::"
```

### Scheduled Checks

The sync checker runs every 6 hours and will automatically detect:
- Missing gitops commits
- Outdated manifests
- Image/manifest mismatches

Check sync checker runs:
```bash
gh run list --workflow=gitops-sync-checker.yml
```

---

## Rollback Plan

If the new workflow causes issues:

```bash
# Revert to old workflow
git revert <commit-sha-of-new-workflow>
git push

# Or disable sync checker
# Edit .github/workflows/gitops-sync-checker.yml
# Comment out the schedule trigger

# Manual gitops (emergency)
kubectl set image deployment/rbs-frontend-prod \
  rbs-frontend-prod=ldamasio/rbs-frontend-prod:sha-XXXXXXX \
  -n robson
```

---

## Maintenance

### Regular Checks

**Weekly:**
```bash
# Check for gitops gaps in last 7 days
gh run list --workflow=gitops-sync-checker.yml --created=">=2025-12-24"

# Check for failed main workflow runs
gh run list --workflow=main.yml --status=failure --created=">=2025-12-24"
```

**Monthly:**
```bash
# Audit gitops commits vs code commits
git log --oneline --since="30 days ago" | grep -v "chore(gitops)" | wc -l  # Code commits
git log --oneline --since="30 days ago" --grep="chore(gitops)" | wc -l     # GitOps commits

# They should be roughly equal (±2)
```

### Cleanup Old Gitops Commits

GitOps commits accumulate over time. They're safe to keep, but can be squashed if desired:

```bash
# DO NOT do this in production!
# Only for history cleanup if absolutely needed

# Create backup branch first
git checkout -b backup-before-squash

# Interactive rebase to squash gitops commits
git rebase -i HEAD~50  # Adjust number as needed

# Mark gitops commits as "squash" or "fixup"
# Save and push --force (DANGER!)
```

**⚠️ WARNING**: Force-pushing to main is dangerous. Only do this if:
- You have full team consensus
- You've verified ArgoCD won't break
- You have a rollback plan

---

## FAQ

### Q: What if ArgoCD doesn't auto-sync?

```bash
# Check ArgoCD app config
argocd app get robson-prod --grpc-web | grep -A 5 syncPolicy

# If auto-sync disabled, enable it
argocd app set robson-prod --sync-policy automated --grpc-web

# Or sync manually
argocd app sync robson-prod --grpc-web
```

### Q: What if manifests show sha-5b09d68 but pods show sha-7d6dc25?

```bash
# ArgoCD hasn't synced yet - wait 3-5 minutes
# Or force sync
argocd app sync robson-prod --grpc-web

# If still not updating, check for pending changes
kubectl get application robson-prod -n argocd -o yaml | grep -A 10 status

# Nuclear option: Delete and recreate pods
kubectl rollout restart deployment/rbs-frontend-prod -n robson
```

### Q: Can I disable the sync checker?

Yes, edit `.github/workflows/gitops-sync-checker.yml`:

```yaml
on:
  workflow_dispatch:  # Keep manual trigger
  # schedule:         # Comment out automated runs
  #   - cron: '0 */6 * * *'
```

### Q: What if sync checker creates too many commits?

It only creates commits when there's an actual gap. If it's triggering frequently:

1. Check main workflow logs for repeated failures
2. Increase retry count in main.yml
3. Adjust sync checker schedule to run less frequently (e.g., every 12h)

---

## Success Metrics

After deploying this solution, you should see:

✅ **Zero gitops gaps** (all commits have corresponding gitops commits)
✅ **Automated recovery** within 6 hours (sync checker)
✅ **No silent failures** (errors annotated in GitHub Actions)
✅ **Faster deployments** (retry logic prevents manual intervention)

---

**Last Updated**: 2025-12-31
**Author**: Claude Code (Anthropic)
**Related**: ADR-0011 GitOps Automatic Manifest Updates
