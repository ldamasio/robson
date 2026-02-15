# ArgoCD Standalone Applications (Legacy)

> **Migration Notice**: All Application manifests have been moved to
> `infra/k8s/gitops/app-of-apps/` as part of the Phase 3 GitOps migration.
> They are now managed by the root App-of-Apps pattern.

## What moved

| Application | Old Location | New Location |
|---|---|---|
| `robson-prod` | `applications/robson-prod.yml` | `app-of-apps/robson-prod.yml` |
| `dns-infrastructure-metallb` | `applications/dns-metallb.yml` | `app-of-apps/dns-metallb.yml` |
| `dns-infrastructure-nodeport` | `applications/dns-nodeport.yml` | `app-of-apps/dns-nodeport.yml` |

## Why

Previously, standalone Applications in this directory required manual
`kubectl apply` to update. By moving them into the `app-of-apps/` directory,
they are now automatically managed by the root App-of-Apps and benefit from
GitOps-driven reconciliation.

## References

- [Phase 3 Validation Checklist](../../docs/gitops/VALIDATION-PHASE3.md)
- [Migration Plan](../../docs/gitops/MIGRATION-PLAN.md)
