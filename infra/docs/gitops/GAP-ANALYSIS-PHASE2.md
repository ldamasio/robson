# GitOps Phase 2: Gap Analysis Report

**Date**: 2026-02-15
**Scope**: Alignment between documented strategy (PR #32) and current repo state

---

## 1. Inventory Summary

| Category | Count | Details |
|----------|-------|---------|
| Application manifests | 16 | 8 in root.yml, 4 in platform/istio-ambient, 1 cert-manager, 1 argocd, 1 gateway-api, 1 robson-prod |
| ApplicationSet manifests | 1 | Branch previews |
| AppProject definitions | 0 | All use "default" project |
| Total ArgoCD files | 12 | Across gitops/, platform/ directories |

---

## 2. Gap Analysis

### GAP-01: No Labels on Root App-of-Apps Children

**Documented standard** (ARGOCD-STRATEGY.md, Section 9):
All Applications must include `rbx.env`, `rbx.agent_id`, `rbx.change_id`, `rbx.product`.

**Current state**:
- 8 Applications in `root.yml` have zero labels
- 4 Istio Applications in `platform/istio-ambient/` have zero labels
- `platform/cert-manager/app.yml` has zero labels
- `platform/argocd/app.yml` has zero labels
- `platform/gateway-api-crds/app.yml` has zero labels

**Compliant**:
- `robson-prod.yml` has `app.kubernetes.io/name` and `app.kubernetes.io/component` (partial)
- `dns-metallb.yml` and `dns-nodeport.yml` have `app.kubernetes.io/name`, `app.kubernetes.io/component`, `deployment-scenario` (partial)

**Verdict**: 0 out of 16 Applications fully comply with the label standard.

### GAP-02: No AppProjects Defined

**Documented recommendation** (ARGOCD-DECISION-RECORD.md, Open Question 1):
Evaluate AppProjects per product for RBAC and source restrictions.

**Current state**:
All 16 Applications use `project: default`. No AppProject manifests exist.

**Risk**: Any Application can deploy to any namespace and reference any source repo. No tenant isolation.

### GAP-03: ApplicationSet Missing RBX Labels

**Documented standard**: All ArgoCD resources must include RBX labels.

**Current state**:
`branches.yml` ApplicationSet has no metadata labels. The generated Applications have only `robson/preview: 'true'`.

### GAP-04: Inconsistent syncPolicy Across Applications

**Root children** (root.yml):
- Minimal syncPolicy: `automated.prune: true`, `selfHeal: true`
- No retry, no syncOptions, no allowEmpty

**Standalone Applications** (robson-prod.yml, dns-*.yml):
- Full syncPolicy: retry with backoff, syncOptions (CreateNamespace, PrunePropagationPolicy, PruneLast), allowEmpty: false
- ignoreDifferences for Deployment replicas

**Risk**: Root children have no retry backoff. A transient failure (network, API throttle) will not be retried.

### GAP-05: Root App-of-Apps Embeds All Children in One File

**Current**: `root.yml` is a single multi-document YAML with 8 resources (root + 7 children).

**Documented layout** (ARGOCD-STRATEGY.md, Section 7): Suggests separate files per child in `app-of-apps/` directory.

**Assessment**: Not blocking. ArgoCD handles multi-document YAML. However, separate files improve diff readability and per-resource PR review. Recommend splitting in a future PR.

### GAP-06: No Sync Windows or Policies Directory

**Documented layout**: `infra/k8s/gitops/policies/` for sync windows and resource overrides.

**Current state**: Directory does not exist. No sync windows are defined.

**Assessment**: Low priority. Relevant when production deployments need maintenance windows.

### GAP-07: DNS Applications Not Referenced from Root

**Current**: `dns-metallb.yml` and `dns-nodeport.yml` are in `infra/k8s/gitops/applications/` but root.yml points to `infra/k8s/gitops/app-of-apps/`. These DNS Applications are not children of the root.

**Assessment**: They must be applied separately or moved into the app-of-apps directory. Clarify ownership model.

---

## 3. Proposed Changes (This PR)

| Change | Files Affected | Risk |
|--------|---------------|------|
| Add RBX labels to root.yml (all 8 Applications) | root.yml | Low: labels are metadata-only |
| Add RBX labels to platform Applications | 4 istio + cert-manager + argocd + gateway-api | Low: labels are metadata-only |
| Add RBX labels to standalone Applications | robson-prod, dns-metallb, dns-nodeport | Low: labels are metadata-only |
| Add RBX labels to ApplicationSet | branches.yml | Low: labels are metadata-only |
| Create AppProject manifests | 3 new files in gitops/projects/ | No cluster impact: files only, not applied |
| Create gap analysis report | This file | Documentation only |
| Create migration plan | MIGRATION-PLAN.md | Documentation only |

---

## 4. Deferred to Future PRs

| Change | Reason | Suggested PR |
|--------|--------|--------------|
| Split root.yml into separate files | Structural change, needs careful ArgoCD sync testing | PR #N+1 |
| Add retry backoff to root children | Behavioral change, needs testing | PR #N+1 |
| Create policies/ directory with sync windows | Not urgent, no production schedule conflicts yet | PR #N+2 |
| Move DNS Applications into root or clarify ownership | Needs decision on whether DNS is bootstrapped or standalone | PR #N+1 |
| Migrate to AppProjects (apply to cluster) | Requires cluster access and ArgoCD reconfiguration | PR #N+2 |

---

## 5. Label Compliance Matrix (After This PR)

| Resource | rbx.env | rbx.product | rbx.agent_id | rbx.change_id |
|----------|---------|-------------|--------------|----------------|
| robson-root | production | platform | human | CHANGE_ID_PLACEHOLDER |
| istio-ambient | production | platform | human | CHANGE_ID_PLACEHOLDER |
| gateway-api-crds | production | platform | human | CHANGE_ID_PLACEHOLDER |
| robson-backend | production | robson | human | CHANGE_ID_PLACEHOLDER |
| robson-frontend | production | robson | human | CHANGE_ID_PLACEHOLDER |
| robson-branch-previews | preview | robson | human | CHANGE_ID_PLACEHOLDER |
| platform-cert-manager | production | platform | human | CHANGE_ID_PLACEHOLDER |
| platform-argocd | production | platform | human | CHANGE_ID_PLACEHOLDER |
| istio-base | production | platform | human | CHANGE_ID_PLACEHOLDER |
| istio-cni | production | platform | human | CHANGE_ID_PLACEHOLDER |
| istiod-ambient | production | platform | human | CHANGE_ID_PLACEHOLDER |
| istio-ztunnel | production | platform | human | CHANGE_ID_PLACEHOLDER |
| platform-cert-manager (app.yml) | production | platform | human | CHANGE_ID_PLACEHOLDER |
| platform-argocd (app.yml) | production | platform | human | CHANGE_ID_PLACEHOLDER |
| gateway-api-crds (app.yml) | production | platform | human | CHANGE_ID_PLACEHOLDER |
| robson-prod | production | robson | human | CHANGE_ID_PLACEHOLDER |
| dns-infrastructure-metallb | production | dns | human | CHANGE_ID_PLACEHOLDER |
| dns-infrastructure-nodeport | production | dns | human | CHANGE_ID_PLACEHOLDER |
| branches ApplicationSet | preview | robson | human | CHANGE_ID_PLACEHOLDER |
