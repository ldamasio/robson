# Vault Template - DO NOT COMMIT WITH REAL VALUES

This is a template for creating the Ansible vault.

## Steps to create vault:

1. Copy your SSH public key:
```bash
cat ~/.ssh/id_ed25519.pub
```

2. Create vault:
```bash
cd infra/ansible
podman run --rm -it \
  -v "$(pwd):/work" -w /work \
  docker.io/alpine/ansible:latest \
  ansible-vault create group_vars/all/vault.yml
```

3. Paste this content (replace YOUR_PUBKEY):

```yaml
---
# Ansible Vault - Created 2024-12-20
# Maintainer: Leandro Damásio

# SSH Configuration (simplified - port 22)
vault_ssh_port: 22

# Admin user public key
vault_admin_pubkey: "YOUR_PUBKEY_HERE"

# k3s cluster token (add after server install)
# vault_k3s_token: "K10..."
```

4. Save vault password securely (1Password, KeePass, etc.)

## View vault:

```bash
podman run --rm -it \
  -v "$(pwd):/work" -w /work \
  docker.io/alpine/ansible:latest \
  ansible-vault view group_vars/all/vault.yml
```

## Edit vault (to add k3s token later):

```bash
podman run --rm -it \
  -v "$(pwd):/work" -w /work \
  docker.io/alpine/ansible:latest \
  ansible-vault edit group_vars/all/vault.yml
```
