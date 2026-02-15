# ArgoCD GitOps Strategy for RBX Systems

**Status**: Living document
**Owner**: Platform Engineering
**Last updated**: 2026-02-15

---

## 1. Objective

RBX Systems uses GitOps with Argo CD as the single mechanism for applying Kubernetes manifests to production clusters. This document defines the strategy for organizing Argo CD resources across all RBX products (robson, strategos, thalamus) and shared infrastructure running on k3s across four VPS nodes.

**Core principle**: Git is the source of truth. No manual `kubectl apply` in production. Every change enters through a pull request, gets reviewed, and reaches the cluster via Argo CD synchronization.

---

## 2. Core Concepts

### Application

The fundamental unit in Argo CD. An Application connects a Git source (repository + path + revision) to a Kubernetes destination (cluster + namespace). It defines what to deploy, where to deploy it, and how to synchronize.

### App-of-Apps

A pattern where one Argo CD Application manages other Application manifests. The "root" Application points to a directory containing child Application YAMLs. Argo CD reads those YAMLs and creates the child Applications automatically.

**Characteristics**:
- Each child Application is an explicit YAML file
- Full control over every Application definition
- Changes require editing or adding individual YAML files
- Good for small, well-known sets of Applications

### ApplicationSet

An Argo CD controller that generates Applications from templates combined with generators. Instead of writing N Application YAMLs by hand, you write one template and a generator that produces N Applications from data sources (Git directories, lists, pull requests, cluster selectors).

**Characteristics**:
- One template produces many Applications
- Generators discover targets automatically (directories, branches, clusters)
- Reduces repetition at the cost of abstraction
- Good for scaling across many apps or environments

### Generators

Data sources that feed an ApplicationSet template. Common generators:

| Generator | Source | Use Case |
|-----------|--------|----------|
| Git Directory | Directories in a repo path | One Application per service directory |
| Git File | JSON/YAML files in a repo | One Application per config file |
| List | Inline key-value pairs | Explicit environment or cluster list |
| Pull Request | Open PRs from GitHub/GitLab | Preview environments per branch |
| Cluster | Registered Argo CD clusters | Deploy to all clusters |
| Matrix | Combination of two generators | Cross-product (apps x environments) |
| Merge | Overlay of two generators | Base config + per-target overrides |

### Templates

The Application blueprint inside an ApplicationSet. Uses Go template syntax with variables from the generator. One template, combined with generator output, produces one Application per generated entry.

---

## 3. Recommended Strategy: Coexistence with Clear Roles

RBX Systems adopts a **coexistence model** where App-of-Apps and ApplicationSet serve different purposes in the same cluster.

### App-of-Apps: Bootstrap and Boundaries

App-of-Apps handles:

- **Cluster bootstrap**: The root Application that brings Argo CD to life and installs foundational platform components (cert-manager, Istio, Gateway API CRDs, Argo CD itself).
- **Platform boundary**: A clear, auditable list of platform-level services that rarely change and require explicit review.
- **One-off Applications**: Services that do not follow a repeatable pattern (custom CRDs, external integrations, singleton infrastructure).

The root Application at `infra/k8s/gitops/app-of-apps/root.yml` already fulfills this role today. It manages nine child Applications including Istio Ambient, cert-manager, Gateway API CRDs, and Argo CD self-management.

### ApplicationSet: Scale and Standardization

ApplicationSet handles:

- **Multi-product deployment**: Generating Applications for robson, strategos, and thalamus from a shared template.
- **Multi-environment deployment**: Producing staging, production, and preview instances from a single definition.
- **Branch previews**: The existing `branches.yml` ApplicationSet already generates preview environments per pull request.
- **Future multi-cluster**: When RBX Systems expands beyond one k3s cluster, ApplicationSet with cluster generators will deploy across clusters without manual Application creation.

### How They Interact

```
Root App-of-Apps (bootstrap)
├── Platform: cert-manager, Istio, Gateway API, ArgoCD (explicit Applications)
├── Product ApplicationSets (one per pattern)
│   ├── Branch Previews ApplicationSet (PR generator)
│   ├── Product Environments ApplicationSet (git directory or list generator)
│   └── DNS ApplicationSet (list generator, when > 2 DNS configs)
└── One-off Applications (singleton services, custom integrations)
```

The root App-of-Apps references ApplicationSet manifests as children. Argo CD creates the ApplicationSet resources, which then generate their own Applications. There is no conflict because:

1. The root manages ApplicationSet **resources** (the controller definitions)
2. The ApplicationSets manage the **generated Applications** (the actual deployments)
3. No resource is managed by both patterns simultaneously

---

## 4. Decision Criteria

### When to Use App-of-Apps

| Criterion | Threshold |
|-----------|-----------|
| Number of managed Applications | Fewer than 5 of the same type |
| Environment count | 1 or 2 environments |
| Change frequency | Rarely changes (platform components) |
| Uniqueness | Each Application has distinct configuration |
| Auditability requirement | Need to see exact definition in a single file |

**Use App-of-Apps when** you need explicit control and the set of Applications is small, stable, and heterogeneous.

### When to Use ApplicationSet

| Criterion | Threshold |
|-----------|-----------|
| Number of similar Applications | More than 5 following the same pattern |
| Environment count | More than 2 environments |
| Products sharing the same template | More than 1 product (robson + strategos) |
| Branch preview environments | Any number (PR generator) |
| Cluster count | 2 or more clusters |
| Automated PR workflows | Agents or CI opening PRs that create Applications |

**Use ApplicationSet when** you have repeatable patterns and the cost of maintaining individual YAML files exceeds the cost of template abstraction.

### RBX Systems Specific Thresholds

- **Today (1 cluster, 1 product in production)**: App-of-Apps for platform bootstrap. ApplicationSet for branch previews. Individual Applications for DNS and production.
- **When strategos or thalamus enter production**: Migrate product deployments to an ApplicationSet with git directory generator.
- **When a second cluster is added**: ApplicationSet with cluster generator becomes the default for all product deployments.
- **When agents begin opening PRs for deployments**: ApplicationSet with PR or git file generator ensures new entries are picked up automatically.

---

## 5. Anti-patterns

### Dual Management

Never manage the same Kubernetes resource from both an App-of-Apps child and an ApplicationSet-generated Application. This causes sync loops, unexpected deletions, and drift. One resource, one owner.

### Over-templating Early

Do not introduce ApplicationSet for two Applications. The overhead of template debugging, generator configuration, and reduced readability is not justified until you have at least five similar Applications or three environments.

### Ignoring Prune Behavior

ApplicationSet with `automated.prune: true` will delete Applications (and their resources) when the generator stops producing them. For example, removing a directory that a git directory generator was watching will delete the corresponding Application and all its resources. Understand prune behavior before enabling it.

### Mixing Helm Values in Templates

Keep Helm values in `values.yaml` files committed to Git, not inline in ApplicationSet templates. Inline values are hard to diff, review, and override per environment.

### Manual kubectl in Production

Any change applied via `kubectl apply` or `kubectl edit` directly will be reverted by Argo CD self-heal. This is by design. All changes must go through Git.

### Circular Self-management Without Bootstrap

Argo CD managing its own installation requires a bootstrap step. The first install must be done via Helm or `kubectl apply`. After that, Argo CD can manage itself through an Application pointing to its own chart. Do not skip the initial bootstrap.

---

## 6. Adoption Roadmap

### Phase 1: Current State (Complete)

- Root App-of-Apps manages platform components
- Individual Applications for robson production and DNS
- ApplicationSet for branch preview environments
- CI/CD updates manifests with SHA-tagged images

### Phase 2: Documentation and Structure (In Progress)

- Document the strategy (this file)
- Organize `infra/k8s/gitops/` with clear directory separation
- Establish labeling standards for audit

### Phase 3: Multi-product ApplicationSet

**Trigger**: Second product (strategos or thalamus) enters production.

- Create a git directory generator ApplicationSet that scans `infra/k8s/products/`
- Each product gets a subdirectory with its manifests or Helm values
- The root App-of-Apps adds the new ApplicationSet as a child

### Phase 4: Environment Matrix

**Trigger**: More than two environments need the same product.

- Introduce a matrix generator combining products and environments
- Use merge generator for per-environment overrides (resource limits, replicas, feature flags)

### Phase 5: Multi-cluster

**Trigger**: Second k3s cluster is provisioned.

- Register the new cluster in Argo CD
- Add cluster generator to the product ApplicationSet
- Evaluate cluster-scoped vs namespace-scoped Application distribution

### Phase 6: Agent-driven Deployments

**Trigger**: Automated agents begin proposing deployment changes via PR.

- Standardize PR labels for agent-authored changes
- ApplicationSet with git file generator reads deployment descriptors committed by agents
- Review workflow includes automated validation gates

---

## 7. Suggested Directory Structure

```
infra/
├── k8s/
│   ├── gitops/
│   │   ├── app-of-apps/
│   │   │   └── root.yml                  # Bootstrap: platform + ApplicationSet refs
│   │   ├── applications/
│   │   │   ├── robson-prod.yml            # Explicit singleton Applications
│   │   │   ├── dns-metallb.yml
│   │   │   └── dns-nodeport.yml
│   │   ├── applicationsets/
│   │   │   ├── branches.yml              # PR preview generator
│   │   │   ├── products.yml              # (Phase 3) Git directory generator
│   │   │   └── environments.yml          # (Phase 4) List or matrix generator
│   │   ├── policies/
│   │   │   ├── sync-windows.yml          # Maintenance windows
│   │   │   └── resource-overrides.yml    # Custom health checks
│   │   └── rbac/
│   │       ├── project-robson.yml        # AppProject for robson
│   │       ├── project-strategos.yml     # AppProject for strategos
│   │       └── project-platform.yml      # AppProject for platform components
│   ├── platform/                          # Platform service manifests
│   │   ├── argocd/
│   │   ├── cert-manager/
│   │   ├── gateway-api-crds/
│   │   └── istio-ambient/
│   ├── prod/                              # Production manifests (robson)
│   ├── staging/                           # Staging manifests
│   └── products/                          # (Phase 3) Per-product directories
│       ├── robson/
│       │   ├── prod/values.yaml
│       │   └── staging/values.yaml
│       ├── strategos/
│       │   └── prod/values.yaml
│       └── thalamus/
│           └── prod/values.yaml
├── charts/                                # Helm charts
│   ├── robson-backend/
│   └── robson-frontend/
├── apps/                                  # Infrastructure apps (DNS, etc.)
│   └── dns/
└── docs/
    └── gitops/
        ├── ARGOCD-STRATEGY.md             # This file
        ├── ARGOCD-DECISION-RECORD.md      # ADR
        ├── EXAMPLES.md                    # YAML examples
        ├── RUNBOOK-GITOPS-CHANGES.md      # Change runbook
        └── GLOSSARY.md                    # Term definitions
```

---

## 8. Operations

### Adding a New Application

**For a singleton (unique configuration)**:

1. Create an Application YAML in `infra/k8s/gitops/applications/`
2. Reference it from the root App-of-Apps if it should be bootstrapped automatically
3. Open a PR, get review, merge
4. Argo CD syncs the root, discovers the new child, creates the Application

**For a product following the standard pattern (Phase 3+)**:

1. Create a directory under `infra/k8s/products/<product-name>/`
2. Add a `values.yaml` or Kustomize overlay
3. The git directory generator ApplicationSet detects the new directory automatically
4. Open a PR, get review, merge
5. Argo CD creates the Application on next sync

### Adding a New Environment

**With list generator**:

1. Edit the ApplicationSet that uses a list generator
2. Add a new entry to the list with environment-specific values
3. Open a PR, get review, merge
4. The ApplicationSet generates a new Application for the environment

**With git directory generator**:

1. Create a subdirectory for the environment under the product directory
2. The generator discovers it automatically
3. Open a PR, get review, merge

### Adding a New Cluster

1. Register the cluster in Argo CD: `argocd cluster add <context-name>`
2. If using a cluster generator ApplicationSet, the new cluster is picked up automatically
3. If using explicit Applications, duplicate and modify the destination for the new cluster
4. Verify with `argocd app list` that Applications target the correct cluster

---

## 9. Label Standards for Audit

All Argo CD Applications and ApplicationSets must include the following labels for traceability:

```yaml
metadata:
  labels:
    rbx.change_id: "PR-42"           # PR number or ticket ID that introduced the change
    rbx.agent_id: "human"            # "human", "ci-bot", "claude-code", or agent identifier
    rbx.env: "production"            # Target environment: production, staging, preview
    rbx.product: "robson"            # Product name: robson, strategos, thalamus, platform
    app.kubernetes.io/managed-by: "argocd"
    app.kubernetes.io/part-of: "rbx-systems"
```

**Rules**:

- `rbx.change_id` is set by the PR author or CI pipeline. For agent-authored PRs, this is the PR number.
- `rbx.agent_id` identifies who created the change. Human engineers use "human". CI pipelines use their bot name. AI agents use their identifier.
- `rbx.env` matches the target environment. Preview environments use "preview".
- `rbx.product` groups Applications by business domain.

These labels enable queries like:

```bash
# All Applications for robson in production
argocd app list -l rbx.product=robson,rbx.env=production

# All changes made by CI bot
argocd app list -l rbx.agent_id=ci-bot

# All Applications from a specific PR
argocd app list -l rbx.change_id=PR-42
```

---

## 10. PR Workflow Integration

### Human Engineers

1. Create a feature branch
2. Modify manifests or Helm values under `infra/`
3. Open a PR against `main`
4. CI validates YAML syntax and runs dry-run diff (if configured)
5. Reviewer approves
6. Merge to `main`
7. Argo CD detects the change (via webhook, under 30 seconds) and syncs

### AI Agents

Agents proposing infrastructure changes must follow the same PR workflow with additional constraints:

1. Agent creates a branch with prefix `agent/` (e.g., `agent/update-robson-replicas`)
2. Agent commits changes with `rbx.agent_id` in the commit message trailer
3. Agent opens a PR with the label `agent-authored`
4. A human reviewer must approve before merge (agents cannot approve their own PRs)
5. After merge, the standard GitOps flow applies

**Agent PR template**:

```markdown
## Summary
[Agent-generated description of the change]

## Change Type
- [ ] Application configuration
- [ ] New Application
- [ ] ApplicationSet modification
- [ ] Platform component update

## Risk Assessment
- [ ] Low: cosmetic or non-functional change
- [ ] Medium: configuration change with rollback path
- [ ] High: structural change affecting sync behavior

## Labels
- rbx.agent_id: <agent-identifier>
- rbx.change_id: <this-PR-number>
```

---

## 11. References

- [Argo CD Documentation](https://argo-cd.readthedocs.io/)
- [ApplicationSet Controller](https://argo-cd.readthedocs.io/en/stable/operator-manual/applicationset/)
- [App-of-Apps Pattern](https://argo-cd.readthedocs.io/en/stable/operator-manual/cluster-bootstrapping/)
- [ADR-0004: GitOps Preview Environments](../../docs/adr/ADR-0004-gitops-preview-environments.md)
- [ADR-0011: GitOps Automatic Manifest Updates](../../../docs/adr/ADR-0011-gitops-automatic-manifest-updates.md)
- [ARGOCD-DECISION-RECORD.md](./ARGOCD-DECISION-RECORD.md)
- [EXAMPLES.md](./EXAMPLES.md)
- [RUNBOOK-GITOPS-CHANGES.md](./RUNBOOK-GITOPS-CHANGES.md)
