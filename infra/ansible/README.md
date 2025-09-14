# Ansible – Bootstrap Ubuntu VPS + k3s

This README summarizes how to harden fresh Contabo Ubuntu 24.04 VPSs and bring up a k3s cluster (1 server + 3 agents) using the playbooks and roles in this repo.

## Prerequisites
- Local tools: `ansible` and `ansible-vault`
- Fresh VPSs (root password for first connection)
- Your SSH key (ed25519). Show it with `cat ~/.ssh/id_ed25519.pub`
- Vault password for encrypt/decrypt

## Inventory
- Edit `infra/ansible/inventory/contabo/hosts.ini` (already filled with your hosts):
  - Server (8GB): `164.68.96.68`
  - Agents (8GB/4GB): `158.220.116.31`, `149.102.139.33`, `167.86.92.97`

## Vault variables
- File: `infra/ansible/group_vars/all/vault.yml`
- Required keys:
  - `vault_ssh_port`: hardened SSH port (e.g., `49731`)
  - `vault_admin_pubkey`: your ed25519 public key (single line)
  - (later) `vault_k3s_token`: value from `/var/lib/rancher/k3s/server/node-token` on the server
- Encrypt the file after editing:
  - `ansible-vault encrypt infra/ansible/group_vars/all/vault.yml`
  - To edit later: `ansible-vault edit infra/ansible/group_vars/all/vault.yml`

## Quick commands (first run)
- Ping all hosts (root + password):
  - `cd infra/ansible`
  - `ansible -i inventory/contabo/hosts.ini all -m ping -u root -k`

- Bootstrap security on all hosts (creates `robson`, installs your key, disables password login, enables UFW, moves SSH to vault port):
  - `ansible-playbook -i inventory/contabo/hosts.ini site.yml -u root -k`
  - Verify SSH: `ssh -p 49731 robson@164.68.96.68`

- Install k3s server only:
  - `ansible-playbook -i inventory/contabo/hosts.ini site.yml -u robson --private-key ~/.ssh/id_ed25519 -e ansible_port=49731 --limit k3s_server`

- Capture node-token and save to Vault:
  - `ssh -p 49731 robson@164.68.96.68 'sudo cat /var/lib/rancher/k3s/server/node-token'`
  - `ansible-vault edit infra/ansible/group_vars/all/vault.yml` (add `vault_k3s_token`)

- Join agents to the cluster:
  - `ansible-playbook -i inventory/contabo/hosts.ini site.yml -u robson --private-key ~/.ssh/id_ed25519 -e ansible_port=49731 --limit k3s_agent`

- Validate cluster from your machine:
  - `scp -P 49731 robson@164.68.96.68:/etc/rancher/k3s/k3s.yaml ~/.kube/config-robson`
  - `export KUBECONFIG=~/.kube/config-robson`
  - `kubectl get nodes -o wide`

## Notes & troubleshooting
- Idempotent: you can re-run the playbooks; the bootstrap role resets the Ansible connection after changing the SSH port.
- UFW: by default, SSH is allowed; and basic k3s ports are opened automatically (`bootstrap_open_k3s_ports=true`):
  - server: 6443/tcp (API), 9345/tcp (supervisor)
  - all nodes: 8472/udp (flannel VXLAN)
  - optional: 10250/tcp on agents (`k3s_allow_kubelet_port=true`)
  - When you install an ingress/gateway, open TCP `80/443` on gateway nodes.
- Service name is `ssh` on Ubuntu 24.04 (handled by the role).
- Secrets: never commit tokens/passwords unencrypted; always use `ansible-vault`.

## Next steps (outside this README)
- Install ArgoCD via Helm and apply the App-of-Apps.
- Install Istio ingress/gateway (Service: LoadBalancer) to obtain a public IP.
- Configure Registro.br wildcard `*.robson.rbx.ia.br` to the gateway IP and validate TLS via cert-manager HTTP-01.
