ADR-0005: Ansible Bootstrap & Hardening (Ubuntu VPS)

Status: Accepted
Date: 2025-09-14

Context
- VPS nodes (Ubuntu Server) must be provisioned securely and consistently before joining k3s.
- SSH security, firewall policy, and admin access must be enforced idempotently via IaC.

Decision
- Create an Ansible `bootstrap` role to:
  - Ensure admin user with passwordless sudo and provisioned `authorized_keys`.
  - Configure OpenSSH: custom port from Ansible Vault, disable password auth, forbid root password login, enable pubkey auth.
  - Enable UFW: default deny incoming, allow outgoing, and allow SSH on the configured port.
  - Restart SSH and reset Ansible connection to new port.
- Store sensitive values (SSH port, keys) in `group_vars/all/vault.yml` encrypted with Ansible Vault.

Consequences
- Positive: repeatable secure baseline; prevents lockout by ensuring key-based admin access before disabling password login.
- Trade-offs: requires Vault management and careful connection handling during port changes.

Implementation Notes
- Files:
  - `infra/ansible/roles/bootstrap/` (tasks as above)
  - `infra/ansible/group_vars/all/vault.yml` (encrypted) and `main.yml` mapping non-sensitive vars
  - `infra/ansible/site.yml` runs `bootstrap` before `k3s`
- After bootstrap, platform components (Istio Ambient, Gateway API, cert-manager, external-dns, ArgoCD) are installed via Helm.

Related
- ADR-0003 Istio Ambient + Gateway API
- ADR-0004 GitOps Preview Environments
- infra/README.md

