# Infra Overview

Stack
- Compute: Contabo VPS (4 nodes initially) running k3s.
- Provisioning: Ansible (idempotent; easy node joins for elasticity).
- Packaging: Helm (platform and apps).
- GitOps: ArgoCD (App of Apps) + ApplicationSet (per-branch previews).
- Mesh & Ingress: Istio Ambient Mode + Gateway API.
- DNS/TLS:
  - Option A (simple): wildcard DNS at Registro.br
    - Create `*.robson.rbx.ia.br` → Gateway LB IP (covers `h-<branch>.robson.rbx.ia.br`).
    - TLS:
      - Automated per-host via cert-manager HTTP-01 (Gateway API solver) — watch Let’s Encrypt rate limits.
      - Or provide a wildcard cert `*.robson.rbx.ia.br` as a Secret and reference in the Gateway.
  - Option B (dynamic DNS): external-dns + cert-manager for `robson.rbx.ia.br`
    - Delegate subzone to a supported provider or use RFC2136; external-dns manages records.

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
- Set secrets: edit `infra/ansible/group_vars/all/vault.yml` with `vault_ssh_port` and `vault_admin_pubkey`, then encrypt with `ansible-vault encrypt`.
- Run: `ansible-playbook -i inventory/contabo site.yml`
- Roles:
  - `bootstrap`: secure baseline (admin user + SSH hardening + UFW); changes SSH port to the Vault-defined port safely.
  - `k3s`: install server/agents and join; store kubeconfig.

Platform via Helm
- Install cert-manager (HTTP-01 with Gateway API solver) and optionally external-dns.
- Install Gateway API CRDs (v1.1.0) and Istio Ambient components (istio-base, istiod with ambient, ztunnel, cni).
- Install Istio (Ambient Mode): base, istiod with ambient enabled, ztunnel DS, CNI.
- Install ArgoCD via Helm; apply App of Apps.

GitOps Previews (ApplicationSet)
- Generate Applications per branch (exclude `main`) with:
  - namespace `h-<branch>` (label: `istio.io/dataplane-mode=ambient`)
  - values: image tag `<branch>-<sha>`, host `h-<branch>.robson.rbx.ia.br`
  - Gateway API resources (Gateway/HTTPRoute) templated from chart values
- Auto-sync enabled; delete on branch removal.
- Sanitization: branch names are normalized (lowercase, '/' and '_' → '-') to form namespace/host.

App Charts (Helm)
- Values: `image.repository`, `image.tag`, `host`, `env`, `resources`.
- Gateway API: define `Gateway` (per env) and `HTTPRoute` mapping host → Service.
- TLS: charts create a Certificate (ClusterIssuer `letsencrypt-http01`) and add HTTPS listener referencing the Secret.

Security & Secrets
- Use SealedSecrets or SOPS for Kubernetes secrets.
- Bootstrap sensitive values via Ansible Vault as needed.

Operations & Debugging
- For hands-on cluster inspection, pod debugging, and log tailing, see [K9S-OPERATIONS.md](K9S-OPERATIONS.md).
- K9s is a terminal UI for Kubernetes that complements GitOps workflows (read-mostly, debug-only).
