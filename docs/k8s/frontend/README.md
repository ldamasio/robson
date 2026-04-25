# Frontend k8s Manifests (Reference Skeletons)

These are **reference skeletons** — do NOT apply from this repo.

Operator copies these files into the rbx-infra GitOps tree and adjusts:

- `<NAMESPACE>` → actual namespace (e.g. `robson`)
- `<CLUSTER_ISSUER>` → ClusterIssuer name (e.g. `letsencrypt-prod`)
- `<INGRESS_CLASS>` → ingress class (e.g. `nginx` or omit if default)
- `<IMAGE_PULL_SECRET>` → image pull secret name (or remove if GHCR public)

## Files

| File | Purpose |
|------|---------|
| deployment.yaml | 2-replica Deployment serving static build via nginx |
| service.yaml | ClusterIP Service, port 80 → 8080 |
| ingress.yaml | Ingress with TLS for both domains |
| kustomization.yaml | Kustomize resource list + common labels |
