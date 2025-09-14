Ansible Bootstrap + k3s — Execution Plan

Status: Ready to execute
Scope: Secure‑bootstrap Ubuntu 24.04 VPSs (Contabo), then install k3s (1 server + 3 agents). No Istio/ArgoCD yet.

Hosts and roles
- 8GB (server): 164.68.96.68
- 8GB (agent): 158.220.116.31
- 4GB (agents): 149.102.139.33, 167.86.92.97

Decisions
- Admin user: robson
- Hardened SSH port (Vault): 49731 (no digit 2)
- SSH key (ed25519):
  - ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIB4vRegRYQrXL+MZu/WJtJ1CXH4hhrSGXN897Yei9rTk ldamasio@gmail.com
- Flow: bootstrap security on all hosts → install k3s on server → capture node-token → join agents → validate cluster

Inventory (infra/ansible/inventory/contabo/hosts.ini)
```
[k3s_server]
164.68.96.68 ansible_user=root

[k3s_agent]
158.220.116.31 ansible_user=root
149.102.139.33 ansible_user=root
167.86.92.97 ansible_user=root
```

Vault and vars (infra/ansible/group_vars/all)
- main.yml (already maps Vault):
  - ssh_port: "{{ vault_ssh_port }}"
  - admin_user: robson
  - admin_pubkey: "{{ vault_admin_pubkey }}"
- vault.yml (fill and then encrypt with ansible-vault):
  - vault_ssh_port: 49731
  - vault_admin_pubkey: "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIB4vRegRYQrXL+MZu/WJtJ1CXH4hhrSGXN897Yei9rTk ldamasio@gmail.com"
  - (later) vault_k3s_token: "K10...:server:..." (add after Step 4)

Step 0 — Prepare key and Vault
- Ensure your key exists:
  - If missing: `ssh-keygen -t ed25519 -C "ldamasio@gmail.com"`
  - Show pubkey: `cat ~/.ssh/id_ed25519.pub`
- Edit and encrypt Vault:
  - `vi infra/ansible/group_vars/all/vault.yml`
  - `ansible-vault encrypt infra/ansible/group_vars/all/vault.yml`

Step 1 — First ping (root + password)
```
cd infra/ansible
ansible -i inventory/contabo/hosts.ini all -m ping -u root -k
```

Step 2 — Bootstrap security (all VPS)
- Creates user robson, installs your authorized_keys, disables password login, enables UFW, moves SSH to 49731, resets SSH connection safely.
```
ansible-playbook -i inventory/contabo/hosts.ini site.yml -u root -k
```
- Verify: `ssh -p 49731 robson@164.68.96.68` (and others)

Step 3 — Install k3s on server only
```
ansible-playbook -i inventory/contabo/hosts.ini site.yml \
  -u robson --private-key ~/.ssh/id_ed25519 \
  -e ansible_port=49731 --limit k3s_server
```

Step 4 — Capture node-token and save to Vault
```
ssh -p 49731 robson@164.68.96.68 'sudo cat /var/lib/rancher/k3s/server/node-token'
ansible-vault edit infra/ansible/group_vars/all/vault.yml
# Add: vault_k3s_token: "K10...:server:..."
```

Step 5 — Join agents
```
ansible-playbook -i inventory/contabo/hosts.ini site.yml \
  -u robson --private-key ~/.ssh/id_ed25519 \
  -e ansible_port=49731 --limit k3s_agent
```

Step 6 — Validate cluster
```
scp -P 49731 robson@164.68.96.68:/etc/rancher/k3s/k3s.yaml ~/.kube/config-robson
export KUBECONFIG=~/.kube/config-robson
kubectl get nodes -o wide
```
Expected: 1 control‑plane + 3 workers Ready

Notes
- Idempotent: plays can be re-run; bootstrap resets Ansible connection after the SSH port change.
 - UFW: SSH is allowed; basic k3s ports open automatically (server: 6443/tcp, 9345/tcp; all: 8472/udp). Gateway ports 80/443/tcp open when `open_gateway_ports=true` (default enabled on server; or define `k3s_gateway` group).
 - Ubuntu 24.04: roles target Ubuntu/Debian (apt, openssh-server, ufw). Service name "ssh" is valid.
 - Secrets: use ansible-vault, never commit plaintext tokens/passwords.

Next (out of scope for this run)
- Install ArgoCD via Helm; apply App‑of‑Apps.
- Install Istio Ingress/Gateway (Service LoadBalancer) to get public IP.
- Configure Registro.br wildcard `*.robson.rbx.ia.br` → `<Gateway_IP>`.
- Validate cert-manager HTTP‑01 issuance.
