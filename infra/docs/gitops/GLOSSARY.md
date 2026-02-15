# GitOps Glossary for RBX Systems

Short definitions of terms used across the GitOps documentation.

---

| Term | Definition |
|------|------------|
| **Application** | The core Argo CD resource. Connects a Git source (repo, path, revision) to a Kubernetes destination (cluster, namespace). One Application manages one set of resources. |
| **ApplicationSet** | An Argo CD resource that generates multiple Applications from a template and one or more generators. Reduces repetition when many Applications follow the same pattern. |
| **App-of-Apps** | A pattern where one Application manages other Application manifests. The parent points to a directory containing child Application YAMLs. Used for bootstrapping and organizing platform components. |
| **Auto-sync** | Argo CD feature that automatically applies changes when Git diverges from the cluster state. Controlled by `syncPolicy.automated` in the Application spec. |
| **Bootstrap** | The initial installation step that brings Argo CD into the cluster. Done once via Helm or kubectl. After bootstrap, Argo CD can manage itself. |
| **Cluster generator** | An ApplicationSet generator that produces entries from registered Argo CD clusters. Used for multi-cluster deployments. |
| **Drift** | The difference between the desired state in Git and the actual state in the cluster. Argo CD detects drift and can auto-correct it (self-heal). |
| **Finalizer** | A Kubernetes mechanism that runs cleanup logic before a resource is deleted. Argo CD uses `resources-finalizer.argocd.argoproj.io` to delete managed resources when an Application is removed. |
| **Generator** | A data source inside an ApplicationSet that produces entries (key-value pairs). Each entry, combined with the template, creates one Application. Types: git directory, git file, list, pull request, cluster, matrix, merge. |
| **Git directory generator** | Produces one entry per subdirectory at a given path in a Git repository. Adding a new directory automatically creates a new Application. |
| **GitOps** | An operational model where Git is the single source of truth for infrastructure and application configuration. Changes are applied by reconciling Git state with cluster state. |
| **Health check** | Argo CD's assessment of whether a managed resource is functioning correctly. Built-in checks exist for Deployments, StatefulSets, Services, and other resource types. Custom checks can be defined. |
| **List generator** | Produces entries from an inline list of key-value pairs. Used when targets are known and enumerable (environments, clusters). |
| **Matrix generator** | Combines two generators to produce a cross-product of entries. Example: products x environments. |
| **Merge generator** | Overlays entries from one generator onto another. Used for base configuration with per-target overrides. |
| **PR generator** | An ApplicationSet generator that produces entries from open pull requests in a Git repository. Used for preview environments. |
| **Prune** | The Argo CD action of deleting resources from the cluster that no longer exist in Git. Enabled by `automated.prune: true` in sync policy. |
| **Reconciliation** | The process where Argo CD compares Git state with cluster state and determines what actions to take (create, update, delete). |
| **Self-heal** | Argo CD feature that reverts manual changes made directly to the cluster, restoring the state defined in Git. Enabled by `automated.selfHeal: true`. |
| **Source of truth** | The authoritative location for configuration. In GitOps, this is always the Git repository. Cluster state is derived, not authoritative. |
| **Sync** | The act of applying the desired state from Git to the cluster. Can be automatic (auto-sync) or manual (triggered by a user). |
| **Sync window** | A time-based policy that controls when Argo CD is allowed to sync. Used to prevent deployments during maintenance or peak hours. |
| **Template** | The Application blueprint inside an ApplicationSet. Uses Go template syntax with variables from generator entries. |
| **Webhook** | An HTTP callback from GitHub to Argo CD that notifies of new commits. Eliminates polling delay (3 minutes) and triggers near-instant sync. |

---

## RBX-specific Terms

| Term | Definition |
|------|------------|
| **rbx.change_id** | Label identifying the PR or ticket that introduced a change. Format: `PR-<number>` or ticket ID. |
| **rbx.agent_id** | Label identifying who authored the change. Values: `human`, `ci-bot`, `claude-code`, or other agent identifiers. |
| **rbx.env** | Label for the target environment. Values: `production`, `staging`, `preview`. |
| **rbx.product** | Label for the business domain. Values: `robson`, `strategos`, `thalamus`, `platform`. |
| **Root Application** | The top-level App-of-Apps that bootstraps all platform components and ApplicationSets. Located at `infra/k8s/gitops/app-of-apps/root.yml`. |
| **Break-glass** | Emergency procedure allowing direct cluster changes outside the standard PR workflow. Requires authorization and post-incident documentation. |

---

## References

- [ARGOCD-STRATEGY.md](./ARGOCD-STRATEGY.md)
- [RUNBOOK-GITOPS-CHANGES.md](./RUNBOOK-GITOPS-CHANGES.md)
