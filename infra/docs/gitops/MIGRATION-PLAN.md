# GitOps Phase 2: Migration Plan

**Date**: 2026-02-15
**Status**: Proposed
**Prerequisite**: Gap analysis completed (GAP-ANALYSIS-PHASE2.md)

---

## Migration Sequence

Changes are organized in three incremental PRs to minimize risk and allow validation between steps.

### PR 1: Labels + AppProjects (This PR)

**Scope**: Metadata-only changes to existing files. New AppProject files (not applied to cluster).

**Changes**:
1. Add RBX labels to all Applications in `root.yml`
2. Add RBX labels to all platform Applications (`platform/istio-ambient/*.yml`, `platform/cert-manager/app.yml`, `platform/argocd/app.yml`, `platform/gateway-api-crds/app.yml`)
3. Add RBX labels to standalone Applications (`robson-prod.yml`, `dns-metallb.yml`, `dns-nodeport.yml`)
4. Add RBX labels to ApplicationSet (`branches.yml`)
5. Create AppProject manifests in `gitops/projects/` (not referenced by any Application yet)
6. Gap analysis report and migration plan documentation

**Downtime risk**: None. Labels are metadata-only. ArgoCD will sync the label changes on next reconciliation. No resources are created or deleted.

**Rollback**: Revert the commit. Labels are removed on next sync.

**Validation**:
- `argocd app list` shows all Applications
- `argocd app get <name>` shows updated labels
- No sync errors in ArgoCD UI

### PR 2: Structural Alignment + AppProject Activation

**Scope**: Split root.yml, activate AppProjects, normalize syncPolicy.

**Changes**:
1. Split `root.yml` into separate files (one per child Application)
2. Update `spec.project` from `default` to the appropriate AppProject (`rbx-platform`, `rbx-applications`, `rbx-previews`)
3. Add the AppProject directory (`gitops/projects/`) to the root App-of-Apps source or create a dedicated Application that manages projects
4. Add retry backoff to root children that currently lack it
5. Move DNS Applications into the app-of-apps directory or document them as standalone with clear ownership

**Downtime risk**: Low. Splitting root.yml into separate files in the same directory does not change what ArgoCD sees (it reads all YAMLs in the path). Changing `spec.project` requires the AppProject to exist first.

**Order of operations**:
1. Apply AppProjects to cluster first (either manually or via a temporary Application)
2. Then update `spec.project` in all Applications
3. ArgoCD will reconcile and validate project permissions

**Rollback**: Revert `spec.project` changes back to `default`. AppProjects can be left in place (they are permissive, not restrictive by default).

**Validation**:
- `argocd proj list` shows three projects
- `argocd app get <name>` shows the correct project
- All Applications are still Synced and Healthy
- No permission errors in sync operations

### PR 3: Policies + Sync Windows + Documentation Updates

**Scope**: Operational guardrails and documentation alignment.

**Changes**:
1. Create `gitops/policies/` directory with sync window definitions (if production needs maintenance windows)
2. Update ARGOCD-STRATEGY.md to reflect the completed Phase 2 changes
3. Update ARGOCD-DECISION-RECORD.md to mark open questions as resolved
4. Add notification templates for sync failures (if ArgoCD Notifications controller is available)

**Downtime risk**: None. Sync windows are additive. Documentation changes have no cluster impact.

**Rollback**: Remove sync window definitions if they block legitimate deployments.

---

## Safe Migration Rules

### What Can Be Changed Without Downtime

- **Labels**: Adding or modifying labels does not affect resource reconciliation
- **Annotations**: Adding or modifying annotations does not affect resource reconciliation
- **AppProject creation**: Creating an AppProject has no impact until Applications reference it
- **New files in existing directories**: ArgoCD picks them up on next sync
- **syncOptions additions**: Adding options like PruneLast is additive

### What Requires Careful Ordering

- **Changing spec.project**: The target AppProject must exist before the Application references it. Otherwise ArgoCD rejects the sync with a "project not found" error.
- **Splitting multi-document YAML**: If the root.yml is split into separate files in the same directory, ArgoCD should handle it transparently. However, verify that no resource names change during the split.
- **Removing Applications from root.yml**: If a child is removed from root.yml while it still has managed resources, those resources will be deleted (if prune is enabled). Remove the Application only after its resources are managed by another Application or deliberately decommissioned.

### What Requires Manual Sync

- **AppProject activation**: After AppProjects are created, changing `spec.project` on existing Applications may trigger a sync error if the project permissions do not match the current source/destination. Test with one Application first.
- **Moving files between directories**: If a file moves from `gitops/applications/` to `gitops/app-of-apps/`, ArgoCD may see it as a new resource in one location and a deleted resource in another. Coordinate the move carefully.

---

## Verification Checklist

After each PR merge:

- [ ] `argocd app list` shows all expected Applications
- [ ] No Applications in "Unknown" or "Error" state
- [ ] `argocd app get <name> --show-operation` shows no failed syncs
- [ ] Labels are visible: `argocd app list -l rbx.env=production`
- [ ] Production workloads are running: `kubectl get pods -n robson`
- [ ] DNS is resolving: `kubectl get pods -n dns`
- [ ] Preview environments (if any active PRs) are functional
