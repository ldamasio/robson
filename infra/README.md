# Infra Overview

Stack
- Compute: Contabo VPS (4 nodes initially) running k3s.
- Provisioning: Ansible (idempotent; easy node joins for elasticity).
- Packaging: Helm (platform and apps).
- GitOps: ArgoCD (App of Apps) + ApplicationSet (per-branch previews).
- Mesh & Ingress: Istio Ambient Mode + Gateway API.
- DNS/TLS: external-dns + cert-manager for `robson.rbx.ia.br`.

Layout
```
infra/
  ansible/
    inventory/contabo            # hosts, groups (k3s_server, k3s_agent)
    roles/k3s                    # install/join tasks
    site.yml                     # bootstrap playbook
  k8s/
    platform/
      istio-ambient/            # helm values/manifests for istio ambient
      cert-manager/
      external-dns/
      argocd/
    gitops/
      app-of-apps/              # root Applications
      applicationsets/branches.yaml  # per-branch preview envs
  charts/
    robson-backend/
    robson-frontend/
```

Ansible (bootstrap)
- Define hosts: `infra/ansible/inventory/contabo/hosts.ini`
- Run: `ansible-playbook -i inventory/contabo site.yml`
- Role `k3s` should install server/agents and join; store kubeconfig.

Platform via Helm
- Install cert-manager, external-dns.
- Install Istio (Ambient Mode): base, istiod with ambient enabled, ztunnel DS, CNI.
- Install ArgoCD via Helm; apply App of Apps.

GitOps Previews (ApplicationSet)
- Generate Applications per branch (exclude `main`) with:
  - namespace `h-<branch>` (label: `istio.io/dataplane-mode=ambient`)
  - values: image tag `<branch>-<sha>`, host `h-<branch>.robson.rbx.ia.br`
  - Gateway API resources (Gateway/HTTPRoute) templated from chart values
- Auto-sync enabled; delete on branch removal.

App Charts (Helm)
- Values: `image.repository`, `image.tag`, `host`, `env`, `resources`.
- Gateway API: define `Gateway` (per env) and `HTTPRoute` mapping host → Service.
- TLS secret referenced by Gateway; Certificate managed by cert-manager.

Security & Secrets
- Use SealedSecrets or SOPS for Kubernetes secrets.
- Bootstrap sensitive values via Ansible Vault as needed.

