# ADR: Argo CD GitOps Organization Strategy

**ID**: ADR-GITOPS-001
**Status**: Accepted
**Date**: 2026-02-15
**Authors**: Platform Engineering, RBX Systems
**Supersedes**: None

---

## Context

RBX Systems operates a k3s Kubernetes cluster across four VPS nodes, hosting multiple products (robson, strategos, thalamus) and shared platform infrastructure. Argo CD is already deployed and managing production workloads through a combination of:

- A root App-of-Apps (`infra/k8s/gitops/app-of-apps/root.yml`) that bootstraps nine child Applications covering platform components and product deployments.
- An ApplicationSet (`infra/k8s/gitops/applicationsets/branches.yml`) that generates preview environments from pull requests.
- Individual Application manifests for production and DNS deployments.

As the platform grows to support additional products and potentially additional clusters, we need a clear decision on how to organize Argo CD resources. The two primary patterns (App-of-Apps and ApplicationSet) can serve overlapping purposes, and without a clear strategy, teams risk duplicating management of the same resources, creating sync conflicts, or over-engineering the setup.

The current state works but is not documented. Engineers make ad-hoc decisions about whether to add a new Application YAML or modify an ApplicationSet. This ADR establishes the rules.

---

## Decision

**RBX Systems adopts a coexistence model where App-of-Apps and ApplicationSet serve distinct, non-overlapping roles.**

### App-of-Apps Scope

The root App-of-Apps manages:

1. **Platform bootstrap**: Argo CD self-management, cert-manager, Istio Ambient, Gateway API CRDs.
2. **ApplicationSet resources**: The ApplicationSet controller definitions themselves are children of the root App-of-Apps.
3. **Singleton Applications**: One-off services that do not follow a repeatable pattern.

The root App-of-Apps is the entry point. It is the first thing installed (after Argo CD bootstrap) and the last thing removed.

### ApplicationSet Scope

ApplicationSets manage:

1. **Branch preview environments**: Using the PR generator (already in place).
2. **Multi-product deployments**: Using git directory generators when more than one product shares the same deployment pattern (triggered when strategos or thalamus reach production).
3. **Multi-environment deployments**: Using list or matrix generators when more than two environments are needed for the same product.
4. **Multi-cluster deployments**: Using cluster generators when a second cluster is added.

### Ownership Rule

**A Kubernetes resource is managed by exactly one Argo CD Application.** That Application is created either by the App-of-Apps (explicit YAML) or by an ApplicationSet (generated). Never both.

### Migration Path

When a group of singleton Applications becomes large enough (more than five following the same pattern) or spans multiple environments (more than two), migrate them from explicit Application YAMLs to an ApplicationSet. Remove the old Application YAMLs from the App-of-Apps directory and add the ApplicationSet manifest instead.

---

## Alternatives Considered

### Alternative 1: App-of-Apps Only

Use explicit Application YAMLs for everything. No ApplicationSets.

**Rejected because**:
- Does not scale. With three products, three environments, and potential multi-cluster, the number of YAML files grows multiplicatively.
- Requires manual file creation for every new combination of product and environment.
- Branch previews would require a custom controller or CI script instead of the built-in PR generator.

### Alternative 2: ApplicationSet Only

Use ApplicationSets for everything. No App-of-Apps.

**Rejected because**:
- Platform components (cert-manager, Istio, Argo CD self-management) are heterogeneous. Forcing them into a template adds complexity without reducing repetition.
- Loses the clarity of explicit bootstrap. The root App-of-Apps is easy to read and audit.
- ApplicationSet requires the controller to be running, which creates a chicken-and-egg problem for bootstrapping Argo CD itself.

### Alternative 3: Separate Repos per Product

Each product gets its own GitOps repository with its own Argo CD configuration.

**Rejected because**:
- Adds operational overhead for a small team.
- Platform components need to be duplicated or managed in a third repo.
- Cross-product consistency becomes harder to enforce.
- The current single-repo model (`robson`) works well for the team size and product count.

### Alternative 4: Kustomize Overlays Instead of ApplicationSet

Use Kustomize overlays to generate per-environment manifests, and keep App-of-Apps pointing to each overlay.

**Rejected because**:
- Solves the environment dimension but not the product dimension.
- Does not provide automatic discovery of new directories or branches.
- ApplicationSet is a native Argo CD feature with better lifecycle management (prune, sync, health) than Kustomize-generated files consumed by App-of-Apps.

---

## Consequences

### Positive

- **Clear ownership**: Every resource has exactly one managing Application. No sync conflicts.
- **Incremental adoption**: The strategy does not require immediate changes. Phase 2 and beyond activate only when triggered by growth.
- **Reduced duplication**: ApplicationSet eliminates repetitive YAML when patterns emerge.
- **Auditability**: The root App-of-Apps provides a human-readable inventory of what runs on the cluster.
- **Branch previews for free**: The existing ApplicationSet pattern scales without modification.
- **Multi-cluster ready**: The cluster generator path is documented and ready when needed.

### Negative

- **Two mental models**: Engineers must understand both App-of-Apps and ApplicationSet. Mitigated by clear documentation and decision criteria.
- **Migration effort**: Moving from explicit Applications to ApplicationSet requires careful cutover to avoid resource deletion during the transition. Mitigated by documenting the migration procedure.
- **Template debugging**: ApplicationSet templates with Go syntax are harder to debug than explicit YAML. Mitigated by keeping templates simple and testing with `argocd appset generate`.
- **Prune risk**: ApplicationSet with auto-prune can delete resources if a generator stops matching. Mitigated by understanding prune behavior and using `preserveResourcesOnDeletion` where appropriate.

---

## Review Criteria

This decision should be revisited when any of the following conditions are met:

| Trigger | Threshold | Action |
|---------|-----------|--------|
| Product count in production | Reaches 3 | Evaluate if the git directory generator covers all products cleanly |
| Environment count per product | Exceeds 4 | Evaluate matrix generator or merge generator for complexity |
| Cluster count | Reaches 2 | Activate multi-cluster ApplicationSet. Validate cluster generator behavior |
| Agent-authored PRs | More than 10 per month | Evaluate if git file generator is needed for agent deployment descriptors |
| Sync conflicts | Any occurrence | Investigate ownership overlap. One resource, one owner |
| ApplicationSet count | Exceeds 10 | Evaluate if ApplicationSets themselves should be managed by an ApplicationSet (meta-pattern) |

**Scheduled review**: Every 6 months or when a trigger is hit, whichever comes first.

---

## Open Questions

1. **AppProject boundaries**: Should each product have its own Argo CD AppProject to enforce RBAC and source restrictions? Current setup uses the default project. This becomes important when multiple teams manage different products.

2. **Secret management**: Argo CD does not manage secrets natively. The current approach uses Kubernetes Secrets committed to Git (with limited sensitive data) or created manually. Evaluate Sealed Secrets, External Secrets Operator, or SOPS integration as the secret count grows.

3. **Sync windows**: Should production have maintenance windows where Argo CD does not auto-sync? Currently, auto-sync is always on. This may need revisiting if deployments cause user-facing disruptions during peak hours.

4. **Notification integration**: Argo CD supports notifications (Slack, email) on sync events. Evaluate adding notifications for failed syncs and drift detection.

5. **Image updater**: Argo CD Image Updater can watch container registries and update image tags without CI commits. Evaluate whether this simplifies the current `sed`-based manifest update workflow or introduces unacceptable indirection.

---

## References

- [ARGOCD-STRATEGY.md](./ARGOCD-STRATEGY.md) (companion strategy document)
- [EXAMPLES.md](./EXAMPLES.md) (YAML examples)
- [ADR-0004: GitOps Preview Environments](../../../docs/adr/ADR-0004-gitops-preview-environments.md)
- [ADR-0011: GitOps Automatic Manifest Updates](../../../docs/adr/ADR-0011-gitops-automatic-manifest-updates.md)
