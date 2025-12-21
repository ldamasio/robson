# ArgoCD Applications

This directory contains ArgoCD Application manifests for deploying Robson components.

## Structure

```
applications/
├── README.md              # This file
└── robson-prod.yaml       # Production application
```

## Current Architecture: Simple Application (Not App-of-Apps)

We're using a **single Application** that points directly to `infra/k8s/prod/`.

**Why not App-of-Apps?**
- Simplicity for initial deployment
- Easier to understand and debug
- Sufficient for small-to-medium projects
- Can be migrated later if needed

## Deploying for the First Time

### Prerequisites

1. ArgoCD installed in the cluster
2. Namespace and secrets created
3. Image tags updated in manifests (from `sha-CHANGEME` to actual SHA)

### Apply the Application

```bash
kubectl apply -f infra/k8s/gitops/applications/robson-prod.yaml
```

### Verify

```bash
# Check ArgoCD Application
argocd app get robson-prod

# Check Kubernetes resources
kubectl get all -n robson
```

## Sync Policy

The application uses **automated sync** with:

- **prune: true** - Removes resources deleted from Git
- **selfHeal: true** - Reverts manual changes in cluster
- **CreateNamespace: true** - Auto-creates namespace if missing

## Updating the Application

To deploy a new version:

1. Update image tags in `infra/k8s/prod/*.yml`
2. Commit and push to main
3. ArgoCD syncs automatically within ~3 minutes

## Troubleshooting

See [ArgoCD Initial Setup Guide](../../docs/runbooks/argocd-initial-setup.md) for:

- Common issues and solutions
- Rollback procedures
- Monitoring commands

## Future Evolution

When the project grows, consider:

- **App-of-Apps pattern**: Split into multiple Applications
- **ApplicationSets**: For preview environments
- **Projects**: Separate prod/staging/dev
- **Sync waves**: Control deployment order

## References

- [ArgoCD Documentation](https://argo-cd.readthedocs.io/)
- [Initial Setup Guide](../../docs/runbooks/argocd-initial-setup.md)
- [CI/CD Tagging Strategy](../../docs/runbooks/ci-cd-image-tagging.md)
