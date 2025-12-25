# ADR-0012: Production Deployment Integrity

**Status**: Accepted
**Date**: 2024-12-25
**Deciders**: Robson Bot Core Team

## Context

As the Robson Bot platform matures and handles real financial operations, we need strict controls over production deployments to ensure:

1. **Auditability**: Every production change must be traceable to a Git commit
2. **Reproducibility**: Production state must be fully recoverable from version control
3. **No Configuration Drift**: Production cannot silently diverge from documented state
4. **Compliance**: Regulatory requirements demand complete change tracking
5. **Team Safety**: No individual can bypass review and deploy unvetted code

### The Problem

Without enforced policies, teams might be tempted to:
- Run `kubectl apply` directly for "quick fixes"
- SSH into nodes to manually patch configurations
- Deploy custom images that bypass CI/CD
- Make "temporary" changes that become permanent

These practices create:
- **Undocumented state**: Changes lost during next deployment
- **Audit gaps**: No record of who changed what and why
- **Recovery failures**: Cannot rebuild production from Git
- **Security risks**: Unreviewed code reaching production

### Real-World Incident Example

**Scenario**: Production API is returning 500 errors. Under pressure, developer SSHs to pod, edits config file, restarts service. Issue resolved in 2 minutes.

**Problems**:
1. Next deployment overwrites the fix → issue returns
2. No Git record of what was changed
3. Other environments don't get the fix
4. Compliance audit fails (undocumented production change)

**Better approach**: Hotfix through Git takes 5-10 minutes but creates proper audit trail and prevents regression.

## Decision

**Establish strict production deployment integrity policy**:

### Golden Rule

**ALL production code and configuration MUST originate from the `main` branch and deploy exclusively through the GitOps pipeline.**

**No exceptions. No workarounds. No shortcuts.**

### Enforcement Mechanisms

1. **ArgoCD Auto-Sync + Self-Heal**
   - Automatically reverts manual `kubectl` changes within 3 minutes
   - Configuration: `automated.selfHeal: true`

2. **GitHub Branch Protection**
   - `main` branch requires PR approval
   - No direct pushes allowed
   - No force pushes allowed

3. **Kubernetes RBAC**
   - Developers: Read-only production access
   - CI Service Account: Image registry writes only
   - ArgoCD: Full sync permissions
   - On-Call: Emergency access (logged and audited)

4. **Audit Logging**
   - Git history: Every commit + author
   - GitHub Actions: CI/CD logs (90 days)
   - ArgoCD: Sync history + diffs
   - Kubernetes: API audit log (30 days)

### Approved Workflow

```
Feature Branch → PR → Code Review → Merge to main → GitHub Actions → ArgoCD → Production
```

All changes tracked in Git with clear attribution and justification.

### Emergency Hotfixes

Even for production incidents, follow the process:

1. Create hotfix branch
2. Make minimal fix
3. Open PR marked `[HOTFIX]`
4. Fast-track review (senior engineer)
5. Merge → Auto-deploy via GitOps

**Timeline**: 5-10 minutes end-to-end (faster than manual intervention + post-incident documentation)

## Alternatives Considered

### 1. Allow Manual Changes with Post-Incident Documentation

- **Pros**: Faster immediate response to incidents
- **Cons**: Relies on discipline, easy to forget documentation, creates drift
- **Why not chosen**: Human error risk too high; automation is more reliable

### 2. Separate Emergency Access with Approval Workflow

- **Pros**: Maintains strict normal process, provides escape hatch
- **Cons**: Complex approval system, still creates drift, audit gaps
- **Why not chosen**: Emergency Git workflow is fast enough; no need for exception

### 3. Read-Only Production (No Direct Access)

- **Pros**: Maximum security, forces GitOps
- **Cons**: Cannot troubleshoot or emergency rollback if GitOps fails
- **Why not chosen**: Too restrictive; need emergency access when infrastructure itself fails

### 4. Post-Deployment Drift Detection Only

- **Pros**: Allows changes, detects them later
- **Cons**: Detection without prevention allows temporary drift
- **Why not chosen**: Prevention (self-heal) is better than detection

## Consequences

### Positive

- ✅ **Complete Audit Trail**: Every change tracked in Git with author and timestamp
- ✅ **Disaster Recovery**: Entire production state in version control
- ✅ **Configuration Consistency**: No drift between Git and running state
- ✅ **Compliance Ready**: Meets regulatory change tracking requirements
- ✅ **Team Confidence**: Cannot accidentally break production with manual changes
- ✅ **Knowledge Sharing**: All changes documented and reviewable

### Negative

- ⚠️ **Slower Emergency Response**: Hotfixes require 5-10 min instead of 2 min
- ⚠️ **Requires Discipline**: Team must follow process even under pressure
- ⚠️ **GitOps Dependency**: If ArgoCD down, deployment blocked (mitigation: manual with post-incident PR)

### Mitigations

- **Emergency Response**: Hotfix workflow optimized to <10 minutes
- **Team Training**: Quarterly incident drills, clear runbooks
- **GitOps Failure**: Documented manual intervention procedure with 24h Git reconciliation requirement

## Implementation

### ArgoCD Configuration

All production applications configured with:

```yaml
apiVersion: argoproj.io/v1alpha1
kind: Application
metadata:
  name: robson-backend-prod
spec:
  syncPolicy:
    automated:
      prune: true       # Delete resources not in Git
      selfHeal: true    # Revert manual changes within 3 minutes
    syncOptions:
      - CreateNamespace=false
      - PruneLast=true
```

### GitHub Branch Protection

```bash
# Applied to 'main' branch
gh api repos/ldamasio/robson/branches/main/protection \
  --method PUT \
  --field required_pull_request_reviews[required_approving_review_count]=1 \
  --field enforce_admins=true
```

### RBAC Roles

**Developer Role** (read-only production):
```yaml
apiVersion: rbac.authorization.k8s.io/v1
kind: RoleBinding
metadata:
  name: developer-read-only
  namespace: production
subjects:
- kind: Group
  name: developers
roleRef:
  kind: ClusterRole
  name: view  # Kubernetes built-in read-only role
```

**On-Call Role** (emergency access):
```yaml
apiVersion: rbac.authorization.k8s.io/v1
kind: RoleBinding
metadata:
  name: oncall-emergency
  namespace: production
subjects:
- kind: Group
  name: oncall-engineers
roleRef:
  kind: ClusterRole
  name: edit  # Can modify, but changes reverted by ArgoCD self-heal
```

## Monitoring

### Sync Status Dashboard

Monitor ArgoCD sync health:

```bash
argocd app get robson-backend-prod
# Expected: Health=Healthy, Sync=Synced
```

**Alert if**:
- `OutOfSync` for > 10 minutes
- `Degraded` health status
- Sync fails 3 times consecutively

### Configuration Drift Alerts

ArgoCD Slack notifications:
- Manual changes detected → `#infra-alerts`
- Self-heal activated → `#infra-alerts`
- Sync failures → `#oncall-urgent`

## Training Requirements

### For All Developers

**Week 1 Onboarding**:
1. Read production deployment policy
2. Deploy test feature through full GitOps flow
3. Practice hotfix workflow in staging

### For On-Call Engineers

**Quarterly Drills**:
- Simulate production incident
- Practice hotfix workflow
- Verify <10 minute end-to-end time

## Success Metrics

Track quarterly:

1. **Compliance**: 100% of production changes through GitOps
2. **Speed**: Hotfix time <10 minutes (95th percentile)
3. **Drift**: Zero untracked configuration drift incidents
4. **Audit**: Complete Git history for all production state

## Related Documents

- **[Production Deployment Policy](../policies/PRODUCTION-DEPLOYMENT.md)** - Detailed operational policy
- **[ADR-0011: GitOps Automatic Manifest Updates](ADR-0011-gitops-automatic-manifest-updates.md)** - Technical automation
- **[ADR-0004: GitOps Preview Environments](ADR-0004-gitops-preview-envs.md)** - Preview environment workflow
- **[ArgoCD Setup Runbook](../runbooks/argocd-initial-setup.md)** - Infrastructure setup
- **[CI/CD Image Tagging](../runbooks/ci-cd-image-tagging.md)** - Image versioning workflow

## References

- [GitOps Principles](https://www.gitops.tech/) - Core GitOps concepts
- [ArgoCD Documentation](https://argo-cd.readthedocs.io/) - Official ArgoCD docs
- [Kubernetes RBAC](https://kubernetes.io/docs/reference/access-authn-authz/rbac/) - Access control patterns
- [NIST Cybersecurity Framework](https://www.nist.gov/cyberframework) - Change management standards

## Review Schedule

**Review Frequency**: Quarterly
**Next Review**: 2025-03-25
**Owner**: Engineering Lead

## Changelog

| Date | Change | Author |
|------|--------|--------|
| 2024-12-25 | Initial ADR created | Robson Bot Core Team |
