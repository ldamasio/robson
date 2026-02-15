# Runbook: GitOps Production Changes

Step-by-step procedures for making changes to RBX Systems production infrastructure through the GitOps workflow.

---

## 1. Standard Change Flow

### Step 1: Create a Branch

```bash
git checkout main
git pull origin main
git checkout -b <type>/<short-description>
```

Use conventional branch prefixes: `feat/`, `fix/`, `chore/`, `infra/`.

### Step 2: Make Changes

Edit the relevant manifests under `infra/`. Common change locations:

| Change Type | Path |
|-------------|------|
| Application image tag | `infra/k8s/prod/*.yml` (automated by CI for app code) |
| Helm values | `infra/charts/<chart>/values.yaml` |
| New Application | `infra/k8s/gitops/applications/<name>.yml` |
| New ApplicationSet | `infra/k8s/gitops/applicationsets/<name>.yml` |
| Platform component | `infra/k8s/platform/<component>/` |
| New product directory | `infra/k8s/products/<product>/` |

### Step 3: Validate Locally

Before pushing, check YAML syntax:

```bash
# Validate YAML syntax
yamllint infra/k8s/gitops/**/*.yml

# If using Helm, template locally
helm template infra/charts/robson-backend/ -f infra/charts/robson-backend/values.yaml

# If using Kustomize
kubectl kustomize infra/k8s/prod/
```

### Step 4: Open a Pull Request

```bash
git add infra/
git commit -m "infra(<scope>): <description>"
git push origin <branch-name>
gh pr create --title "infra: <description>" --body "## Summary\n<what and why>"
```

### Step 5: Review

The reviewer checks:

- YAML syntax and structure
- Correct namespace and cluster destination
- Labels include `rbx.change_id`, `rbx.agent_id`, `rbx.env`
- No secrets or credentials in plain text
- Sync policy is appropriate (auto vs manual)
- Resource limits and requests are set

### Step 6: Merge

After approval, merge the PR to `main`. Squash merge is preferred for clean history.

### Step 7: Argo CD Synchronization

After merge to `main`:

1. **Webhook delivery** (under 30 seconds): GitHub sends a webhook to Argo CD at `https://argocd.robson.rbx.ia.br/api/webhook`.
2. **Argo CD detects the change**: Compares the new Git state with the cluster state.
3. **Auto-sync executes**: If `syncPolicy.automated` is enabled, Argo CD applies the changes.
4. **Health check**: Argo CD monitors resource health (Deployments, StatefulSets, Jobs).

### Step 8: Verify

```bash
# Check Application sync status
argocd app get <app-name>

# Check all Applications
argocd app list

# Watch sync progress
argocd app wait <app-name> --sync --health --timeout 120
```

In the Argo CD web UI at `https://argocd.robson.rbx.ia.br`:
- Application should show "Synced" and "Healthy"
- Resource tree should show all resources green

---

## 2. Troubleshooting

### Application Stuck in "OutOfSync"

**Symptoms**: Application shows OutOfSync but auto-sync does not trigger.

**Checks**:
1. Verify `syncPolicy.automated` is set in the Application spec
2. Check for sync errors: `argocd app get <app-name> --show-operation`
3. Check Argo CD controller logs: `kubectl logs -n argocd -l app.kubernetes.io/name=argocd-application-controller --tail=50`
4. Force a manual sync: `argocd app sync <app-name>`

### Application Shows "Degraded"

**Symptoms**: Resources deployed but health check fails.

**Checks**:
1. Check pod status: `kubectl get pods -n <namespace>`
2. Check pod events: `kubectl describe pod <pod-name> -n <namespace>`
3. Check container logs: `kubectl logs <pod-name> -n <namespace> --tail=50`
4. Common causes: image pull errors, resource quota exceeded, readiness probe failing

### Webhook Not Triggering Sync

**Symptoms**: Changes merged but Argo CD does not detect them for up to 3 minutes (polling interval).

**Checks**:
1. Verify webhook delivery in GitHub: Repository Settings > Webhooks > Recent Deliveries
2. Check for HTTP errors (403, 404, 500)
3. Verify Argo CD server is reachable from GitHub
4. If webhook is broken, Argo CD falls back to polling (default 3-minute interval)

### Sync Conflict Between Applications

**Symptoms**: Two Applications trying to manage the same resource. One shows "OutOfSync" or resources flicker.

**Resolution**:
1. Identify which Applications manage the conflicting resource
2. Remove the resource from one Application's source path
3. Ensure the ownership rule: one resource, one Application

### Prune Deleted a Resource Unexpectedly

**Symptoms**: A resource disappears after sync because it was removed from Git.

**Resolution**:
1. If intentional: no action needed
2. If unintentional: restore the manifest from Git history and re-commit
3. To prevent: use `syncOptions: [Prune=false]` for sensitive resources, or annotate resources with `argocd.argoproj.io/sync-options: Prune=false`

### Image Tag Not Updated

**Symptoms**: CI built a new image but the deployment still runs the old tag.

**Checks**:
1. Verify CI updated the manifest: `git log --oneline -5 infra/k8s/prod/`
2. Check for `[skip ci]` commit from the bot
3. Run the GitOps sync checker manually: `gh workflow run gitops-sync-checker.yml`
4. Verify the image exists on DockerHub: `docker manifest inspect ldamasio/rbs-backend-prod:sha-<hash>`

---

## 3. Rollback Procedure

### Option A: Git Revert (Preferred)

```bash
# Find the commit that introduced the bad change
git log --oneline -10

# Revert the commit
git revert <commit-hash>

# Push the revert
git push origin main
```

Argo CD syncs the reverted state automatically.

### Option B: Manual Sync to Previous Revision

```bash
# Sync to a specific Git commit
argocd app sync <app-name> --revision <good-commit-hash>
```

Note: This is temporary. The next auto-sync will bring the Application back to HEAD. Use Git revert for a permanent fix.

### Option C: Disable Auto-sync Temporarily

```bash
# Disable auto-sync
argocd app set <app-name> --sync-policy none

# Fix the issue in Git
# ...

# Re-enable auto-sync
argocd app set <app-name> --sync-policy automated --self-heal --auto-prune
```

---

## 4. Break-glass Policy

In exceptional circumstances where the standard PR workflow cannot be followed (cluster outage, security incident, time-critical fix):

### Authorization

- Requires verbal or written approval from at least one platform engineer
- The engineer performing the action must document it immediately after

### Permitted Actions

- Direct `kubectl apply` of a specific manifest to restore service
- Manual Argo CD sync or rollback via CLI or UI
- Temporary disabling of auto-sync on a specific Application

### Post-incident Requirements

1. Create a follow-up PR within 24 hours that captures the change in Git
2. Ensure Git and cluster state are reconciled (no drift)
3. Document the incident: what happened, what was done, and why the standard flow was bypassed
4. Review whether the break-glass scenario reveals a gap in the standard workflow

### Boundaries

- Break-glass does not authorize bulk changes across multiple Applications
- Break-glass does not authorize changes to Argo CD RBAC or credentials
- All break-glass actions must be logged and traceable

---

## 5. Quick Reference

| Task | Command |
|------|---------|
| List all Applications | `argocd app list` |
| Get Application details | `argocd app get <name>` |
| Manual sync | `argocd app sync <name>` |
| Sync with prune | `argocd app sync <name> --prune` |
| Watch sync progress | `argocd app wait <name> --sync --health` |
| View sync history | `argocd app history <name>` |
| Diff (what would change) | `argocd app diff <name>` |
| Disable auto-sync | `argocd app set <name> --sync-policy none` |
| Re-enable auto-sync | `argocd app set <name> --sync-policy automated --self-heal --auto-prune` |
| View ApplicationSets | `kubectl get applicationsets -n argocd` |
| Generate ApplicationSet preview | `argocd appset generate <file.yml>` |

---

## References

- [ARGOCD-STRATEGY.md](./ARGOCD-STRATEGY.md)
- [ARGOCD-DECISION-RECORD.md](./ARGOCD-DECISION-RECORD.md)
- [EXAMPLES.md](./EXAMPLES.md)
