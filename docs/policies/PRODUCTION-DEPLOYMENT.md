# Production Deployment Policy

**Status**: Active
**Effective Date**: 2024-12-25
**Owner**: Engineering Team
**Version**: 1.0

---

## Golden Rule

**ALL production code MUST originate from the `main` branch and deploy exclusively through the GitOps pipeline.**

**No exceptions. No workarounds. No shortcuts.**

---

## Rationale

### Why This Matters

1. **Auditability**: Every production change is tracked in Git history
2. **Reproducibility**: Entire production state can be recreated from `main`
3. **Disaster Recovery**: Git serves as single source of truth for restoration
4. **Configuration Drift Prevention**: Production cannot diverge from documented state
5. **Compliance**: Regulatory requirements demand change tracking
6. **Team Safety**: No single person can bypass review process

### What We Prevent

- Undocumented hot-patches that break during next deployment
- Configuration drift between environments
- "Works on my machine" production debugging
- Tribal knowledge about manual tweaks
- Loss of changes during cluster rebuilds

---

## Prohibited Practices

The following are **STRICTLY FORBIDDEN** in production:

### ❌ Manual Resource Application

```bash
# NEVER DO THIS
kubectl apply -f my-fix.yaml
kubectl create configmap hotfix --from-file=config.json
kubectl patch deployment backend -p '{"spec": {"replicas": 5}}'
```

### ❌ Direct SSH Interventions

```bash
# NEVER DO THIS
ssh root@production-node
docker run -d my-custom-image
systemctl restart kubelet
```

### ❌ Custom Images

- Images not built by GitHub Actions CI pipeline
- Images with `:latest` or `:dev` tags
- Images from personal Docker Hub accounts
- Images without Git SHA tags

### ❌ kubectl edit in Production

```bash
# NEVER DO THIS
kubectl edit deployment backend -n production
kubectl edit configmap app-config -n production
```

### ❌ Bypassing Code Review

- Pushing directly to `main` without PR
- Merging your own PRs without approval
- Emergency commits without post-incident review

---

## Approved Workflow

### Standard Deployment Flow

```
┌─────────────────┐
│ 1. Create Branch│
│   feature/xyz   │
└────────┬────────┘
         │
         ▼
┌─────────────────┐
│ 2. Develop      │
│   + Test        │
│   + Commit      │
└────────┬────────┘
         │
         ▼
┌─────────────────┐
│ 3. Push Branch  │
│   (triggers CI) │
└────────┬────────┘
         │
         ▼
┌─────────────────┐
│ 4. Open PR      │
│   + Review      │
│   + Approval    │
└────────┬────────┘
         │
         ▼
┌─────────────────┐
│ 5. Merge to main│
└────────┬────────┘
         │
         ▼
┌─────────────────────────────────────────┐
│ 6. AUTOMATIC PRODUCTION DEPLOYMENT      │
│                                          │
│  GitHub Actions                          │
│   └─> Build image (sha-XXXXXX)          │
│   └─> Push to registry                  │
│   └─> Update infra/k8s/overlays/prod/   │
│   └─> Commit manifest changes           │
│                                          │
│  ArgoCD                                  │
│   └─> Detects Git change                │
│   └─> Syncs cluster state               │
│   └─> Deployment complete ✓             │
└─────────────────────────────────────────┘
```

### Configuration Changes

For ConfigMaps, Secrets, or resource limits:

1. **Edit**: `infra/k8s/base/` or `infra/k8s/overlays/prod/`
2. **Commit**: Follow conventional commits
3. **PR**: Get review + approval
4. **Merge**: ArgoCD auto-syncs within 3 minutes

Example:

```bash
# Edit manifest
vim infra/k8s/overlays/prod/backend-deployment.yaml

# Commit
git add infra/k8s/overlays/prod/backend-deployment.yaml
git commit -m "feat(infra): increase backend replicas to 5 for Black Friday"

# Push + PR + Merge
git push origin feature/scale-backend
gh pr create --title "Scale backend for high traffic"
# (After approval)
gh pr merge
```

---

## Emergency Scenarios

### "Production is Down! We Need to Fix It NOW!"

**Still follow the process**, but expedite:

1. **Hotfix Branch**: `git checkout -b hotfix/critical-fix`
2. **Minimal Fix**: Smallest possible change
3. **Quick PR**: Mark as `[HOTFIX]` in title
4. **Fast Review**: Tag senior engineer for immediate review
5. **Merge**: Use "Squash and merge"
6. **Monitor**: Watch ArgoCD sync (2-3 minutes)

**Timeline**: 5-10 minutes end-to-end (faster than manual intervention + documentation later)

### "But ArgoCD is Down!"

If GitOps infrastructure itself fails:

1. **Manual intervention IS allowed** (exceptional case)
2. **Document everything**: Save all `kubectl` commands
3. **Create post-incident PR**: Replicate manual changes in Git within 24 hours
4. **Review**: Explain why manual intervention was necessary

### "We Need to Rollback Immediately!"

**Option A: Git Revert** (Recommended)

```bash
git revert <bad-commit-sha>
git push origin main
# ArgoCD auto-deploys previous state
```

**Option B: ArgoCD Rollback**

```bash
argocd app rollback robson-backend-prod
# Then create PR to revert Git state
```

---

## Enforcement Mechanisms

### 1. ArgoCD Configuration

**Auto-Sync + Self-Heal** enabled:

```yaml
# infra/argocd/applications/robson-backend-prod.yaml
apiVersion: argoproj.io/v1alpha1
kind: Application
metadata:
  name: robson-backend-prod
spec:
  syncPolicy:
    automated:
      prune: true       # Delete resources not in Git
      selfHeal: true    # Revert manual changes
    syncOptions:
      - CreateNamespace=false
      - PruneLast=true
```

**Effect**: Manual `kubectl apply` changes are **reverted within 3 minutes**.

### 2. RBAC Restrictions

Production cluster RBAC limits manual changes:

- **Developers**: Read-only access
- **CI Service Account**: Write access (image updates only)
- **ArgoCD**: Full sync permissions
- **On-Call Engineer**: Emergency access (logged + audited)

### 3. Branch Protection Rules

GitHub `main` branch requires:

- ✅ At least 1 approval
- ✅ CI checks pass
- ✅ Branch up-to-date with main
- ❌ Direct pushes disabled
- ❌ Force pushes disabled

### 4. Audit Logging

All production changes logged:

- **Git History**: Every commit + author
- **GitHub Actions**: CI/CD logs (90 days retention)
- **ArgoCD**: Sync history + diffs
- **Kubernetes Audit Log**: API calls (30 days retention)

---

## Monitoring & Alerts

### ArgoCD Sync Status

Monitor sync health:

```bash
# Check sync status
argocd app get robson-backend-prod

# Expected output
Health Status:      Healthy
Sync Status:        Synced
Last Sync:          2024-12-25 14:32:15 UTC (2 minutes ago)
```

**Alert if**:

- Sync status `OutOfSync` for > 10 minutes
- Health status `Degraded`
- Sync fails 3 times consecutively

### Configuration Drift Detection

ArgoCD detects drift automatically:

- Manual changes trigger `OutOfSync` status
- Self-heal reverts changes within 3 minutes
- Slack notification sent to `#infra-alerts`

---

## Training & Onboarding

### For New Developers

**Day 1**: Read this policy + ADR-0011
**Week 1**: Deploy test feature through full GitOps flow
**Week 2**: Shadow on-call engineer during deployment

### For Operations Team

**Required Knowledge**:

- How to review ArgoCD sync status
- Emergency rollback procedures
- When manual intervention is justified

**Quarterly Drill**:

- Simulate production incident
- Practice hotfix workflow
- Verify <10 minute response time

---

## Related Documentation

- **[ADR-0011](../adr/ADR-0011-gitops-automatic-manifest-updates.md)**: GitOps Automatic Manifest Updates
- **[ADR-0004](../adr/ADR-0004-gitops-preview-environments.md)**: GitOps Preview Environments
- **[DEVELOPER.md](../DEVELOPER.md)**: Development workflow
- **[CLAUDE.md](../CLAUDE.md)**: Quick reference for AI assistants

---

## Exceptions Log

Document any approved exceptions here:

| Date | Reason | Approver | Post-Incident PR |
|------|--------|----------|------------------|
| _None yet_ | - | - | - |

---

## Policy Review

**Review Frequency**: Quarterly
**Next Review**: 2025-03-25
**Responsible**: Engineering Lead

---

## Changelog

| Version | Date | Changes | Author |
|---------|------|---------|--------|
| 1.0 | 2024-12-25 | Initial policy creation | Engineering Team |

---

**Questions?** Open an issue or contact the engineering lead.

**Violations?** Report immediately to `#security` channel.
