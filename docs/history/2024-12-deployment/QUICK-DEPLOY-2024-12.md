# Quick Production Deployment - December 2024

**Status**: In Progress  
**Started**: 2024-12-20  
**VPS State**: Fresh Ubuntu 24.04 install on all 4 nodes  
**Target**: Production-ready k3s cluster in ~4 hours  

---

## ✅ Checkpoint: What's Done

- [x] 4 VPS reinstalled (Ubuntu 24.04)
- [x] SSH root access confirmed (port 22)
- [x] Network connectivity validated (ping successful)
- [x] Old vault archived
- [ ] New vault created
- [ ] Ansible inventory configured
- [ ] k3s installed
- [ ] ArgoCD deployed
- [ ] Applications running

---

## 📋 PHASE 1: Ansible Setup (30 min)

### 1.1 Clean SSH Known Hosts

```bash
# Remove old host keys
ssh-keygen -R 158.220.116.31
ssh-keygen -R 164.68.96.68
ssh-keygen -R 149.102.139.33
ssh-keygen -R 167.86.92.97
```

### 1.2 Get Your SSH Public Key

```bash
cat ~/.ssh/id_ed25519.pub
```

**Save this output** - you'll need it for the vault.

Example output:
```
ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAA... your-email@hostname
```

### 1.3 Create New Ansible Vault

```bash
cd /c/app/notes/robson/infra/ansible

# Archive old vault (already done if you followed earlier steps)
# git mv group_vars/all/vault.yml group_vars/all/vault.yml.orphaned-2024-12

# Create new vault
podman run --rm -it \
  -v "$(pwd):/work" -w /work \
  docker.io/alpine/ansible:latest \
  ansible-vault create group_vars/all/vault.yml
```

**When prompted for password**: Choose a **STRONG password** and **SAVE IT** securely (1Password, KeePass, etc).

**Content to paste** (replace YOUR_PUBKEY with the key from step 1.2):

```yaml
---
# Ansible Vault - Created 2024-12-20
# Maintainer: Leandro Damásio
# Password saved in: [DOCUMENT WHERE YOU SAVED IT]

# SSH Configuration (simplified - port 22 for quick deploy)
vault_ssh_port: 22

# Admin user public key (from: cat ~/.ssh/id_ed25519.pub)
vault_admin_pubkey: "YOUR_PUBKEY_HERE"

# k3s cluster token (will be added after server install - leave commented for now)
# vault_k3s_token: "K10..."
```

**Save and exit**: ESC, `:wq`, ENTER

### 1.4 Verify Vault

```bash
# View vault (should ask for password)
podman run --rm -it \
  -v "$(pwd):/work" -w /work \
  docker.io/alpine/ansible:latest \
  ansible-vault view group_vars/all/vault.yml

# Should show your content
```

### 1.5 Edit Ansible Inventory

Edit: `infra/ansible/inventory/contabo/hosts.ini`

Replace with:

```ini
[k3s_server]
# 8GB node (server)
tiger  ansible_host=158.220.116.31 ansible_user=root ansible_ssh_pass=TIGER_ROOT_PASSWORD_HERE

[k3s_agent]
# 8GB node (agent)
bengal ansible_host=164.68.96.68 ansible_user=root ansible_ssh_pass=BENGAL_ROOT_PASSWORD_HERE
# 4GB nodes (agents)
pantera ansible_host=149.102.139.33 ansible_user=root ansible_ssh_pass=PANTERA_ROOT_PASSWORD_HERE
eagle   ansible_host=167.86.92.97 ansible_user=root ansible_ssh_pass=EAGLE_ROOT_PASSWORD_HERE

[k3s_gateway]
tiger
```

**Replace** the 4 passwords with the actual root passwords from your notes.

⚠️ **Security Note**: This is temporary for quick deployment. Phase 3 will remove passwords and use keys only.

### 1.6 Test Ansible Connectivity

```bash
cd /c/app/notes/robson/infra/ansible

# Ping test
podman run --rm -it \
  -v "$(pwd):/work" -w /work \
  docker.io/alpine/ansible:latest \
  ansible -i inventory/contabo/hosts.ini all -m ping

# Expected: SUCCESS | pong for all 4 hosts
```

---

## 🚀 PHASE 2: k3s Installation (1 hour)

### 2.1 Install k3s Server (tiger)

```bash
cd /c/app/notes/robson/infra/ansible

# Run playbook for server only
podman run --rm -it \
  -v "$(pwd):/work" -w /work \
  docker.io/alpine/ansible:latest \
  ansible-playbook -i inventory/contabo/hosts.ini \
  playbooks/k3s-simple-install.yml \
  --ask-vault-pass \
  --limit k3s_server
```

### 2.2 Capture k3s Token

```bash
# SSH to tiger and get token
ssh root@158.220.116.31 "cat /var/lib/rancher/k3s/server/node-token"

# Output will be like: K10abc123...::server:xyz789...
# SAVE THIS TOKEN
```

### 2.3 Add Token to Vault

```bash
cd /c/app/notes/robson/infra/ansible

# Edit vault
podman run --rm -it \
  -v "$(pwd):/work" -w /work \
  docker.io/alpine/ansible:latest \
  ansible-vault edit group_vars/all/vault.yml

# Uncomment and add the token:
# vault_k3s_token: "K10abc123...::server:xyz789..."
```

### 2.4 Install k3s Agents

```bash
# Run playbook for agents
podman run --rm -it \
  -v "$(pwd):/work" -w /work \
  docker.io/alpine/ansible:latest \
  ansible-playbook -i inventory/contabo/hosts.ini \
  playbooks/k3s-simple-install.yml \
  --ask-vault-pass \
  --limit k3s_agent
```

### 2.5 Get Kubeconfig

```bash
# Copy kubeconfig from tiger
scp root@158.220.116.31:/etc/rancher/k3s/k3s.yaml ~/.kube/config-robson

# Fix server IP (replace 127.0.0.1 with tiger's IP)
sed -i 's/127.0.0.1/158.220.116.31/' ~/.kube/config-robson

# Set as default
export KUBECONFIG=~/.kube/config-robson

# Test
kubectl get nodes

# Expected: 4 nodes (1 control-plane, 3 workers) in Ready state
```

---

## 📦 PHASE 3: ArgoCD Installation (30 min)

### 3.1 Install ArgoCD

```bash
# Create namespace
kubectl create namespace argocd

# Install ArgoCD
kubectl apply -n argocd -f https://raw.githubusercontent.com/argoproj/argo-cd/v2.10.0/manifests/install.yaml

# Wait for pods
kubectl wait --for=condition=available --timeout=300s \
  deployment/argocd-server -n argocd

# Get admin password
kubectl -n argocd get secret argocd-initial-admin-secret \
  -o jsonpath="{.data.password}" | base64 -d
echo

# SAVE THIS PASSWORD
```

### 3.2 Access ArgoCD UI (Optional)

```bash
# Port forward
kubectl port-forward svc/argocd-server -n argocd 8080:443

# Access: https://localhost:8080
# User: admin
# Password: [from step 3.1]
```

---

## 🎯 PHASE 4: Deploy Applications (1 hour)

### 4.1 Install cert-manager

```bash
# Install cert-manager
kubectl apply -f https://github.com/cert-manager/cert-manager/releases/download/v1.13.0/cert-manager.yaml

# Wait for pods
kubectl wait --for=condition=available --timeout=300s \
  deployment/cert-manager -n cert-manager

# Create ClusterIssuer
kubectl apply -f infra/k8s/platform/cert-manager/cluster-issuer.yml
```

### 4.2 Install Gateway API CRDs

```bash
kubectl apply -f https://github.com/kubernetes-sigs/gateway-api/releases/download/v1.0.0/standard-install.yaml
```

### 4.3 Create Secrets

```bash
# Create robson namespace
kubectl create namespace robson

# Create Django secret (REPLACE VALUES)
kubectl create secret generic rbs-django-secret \
  --namespace=robson \
  --from-literal=RBS_SECRET_KEY='CHANGE_ME_DJANGO_SECRET_KEY' \
  --from-literal=RBS_BINANCE_API_KEY_TEST='CHANGE_ME' \
  --from-literal=RBS_BINANCE_SECRET_KEY_TEST='CHANGE_ME' \
  --from-literal=RBS_BINANCE_API_KEY_PROD='CHANGE_ME' \
  --from-literal=RBS_BINANCE_SECRET_KEY_PROD='CHANGE_ME' \
  --from-literal=RBS_BINANCE_API_URL_TEST='https://testnet.binance.vision' \
  --from-literal=POSTGRES_DATABASE='rbsdb' \
  --from-literal=POSTGRES_USER='robson' \
  --from-literal=POSTGRES_PASSWORD='CHANGE_ME_SECURE_PASSWORD' \
  --from-literal=POSTGRES_HOST='postgres.robson.svc.cluster.local' \
  --from-literal=POSTGRES_PORT='5432'
```

### 4.4 Update Image Tags

Get latest SHA from GitHub Actions:
1. Go to: https://github.com/ldamasio/robson/actions
2. Find latest successful run
3. Note the SHA (e.g., `sha-a1b2c3d`)

Update manifests:
```bash
cd /c/app/notes/robson

# Replace sha-CHANGEME with actual SHA
sed -i 's/sha-CHANGEME/sha-YOUR_SHA_HERE/g' infra/k8s/prod/*.yml

# Commit
git add infra/k8s/prod/*.yml
git commit -m "deploy: update images to production SHA"
git push
```

### 4.5 Deploy via ArgoCD

```bash
# Apply ArgoCD Application
kubectl apply -f infra/k8s/gitops/applications/robson-prod.yml

# Watch deployment
kubectl get pods -n robson -w

# Expected: 3 pods running (frontend, backend-monolith, backend-nginx)
```

---

## 🌐 PHASE 5: DNS Configuration (30 min)

### 5.1 Get Gateway IP

```bash
kubectl get svc -n robson
# Look for LoadBalancer service IP (should be 158.220.116.31 - tiger)
```

### 5.2 Configure DNS at Registro.br

Login to Registro.br DNS panel and add:

```
Type    Name                        Value               TTL
A       api.robson.rbx.ia.br       158.220.116.31      3600
A       app.robson.rbx.ia.br       158.220.116.31      3600
```

### 5.3 Wait for DNS Propagation (10-15 min)

```bash
# Test DNS resolution
dig +short api.robson.rbx.ia.br
# Should return: 158.220.116.31

dig +short app.robson.rbx.ia.br
# Should return: 158.220.116.31
```

### 5.4 Verify TLS Certificates

```bash
# Check certificates
kubectl get certificate -n robson

# Should show: Ready=True for both certificates

# Test HTTPS
curl -I https://api.robson.rbx.ia.br
curl -I https://app.robson.rbx.ia.br
```

---

## ✅ SUCCESS CRITERIA

Production is ready when:

- [ ] All 4 nodes show `Ready` in `kubectl get nodes`
- [ ] ArgoCD Application shows `Synced` and `Healthy`
- [ ] 3 pods running in `robson` namespace
- [ ] DNS resolves correctly
- [ ] HTTPS works (Let's Encrypt certificates valid)
- [ ] Application accessible via browser

---

## 🔧 TROUBLESHOOTING

### Pods not starting

```bash
# Check pod status
kubectl get pods -n robson

# Describe pod for events
kubectl describe pod -n robson <pod-name>

# Check logs
kubectl logs -n robson <pod-name>
```

### Certificate not Ready

```bash
# Check certificate status
kubectl describe certificate -n robson

# Check cert-manager logs
kubectl logs -n cert-manager deployment/cert-manager

# Check HTTP-01 challenge
kubectl get challenge -n robson
```

### ArgoCD not syncing

```bash
# Force sync
kubectl patch application -n argocd robson-prod \
  --type=merge -p '{"operation":{"sync":{"syncStrategy":{"hook":{}}}}}'

# Check sync errors in ArgoCD UI
```

---

## 📚 NEXT STEPS (After Production Works)

### Week 1: Security Hardening
- [ ] Change SSH to custom port
- [ ] Disable root login
- [ ] Create admin user with sudo
- [ ] Enable UFW firewall
- [ ] Remove passwords from inventory (use SSH keys only)

### Week 2: Monitoring
- [ ] Install Prometheus stack
- [ ] Configure Grafana dashboards
- [ ] Set up alerts

### Week 3: Backups
- [ ] Install Velero
- [ ] Configure daily backups
- [ ] Test restore procedure

---

## 📝 NOTES

- All passwords saved in: [DOCUMENT YOUR PASSWORD MANAGER LOCATION]
- Vault password: [DOCUMENT WHERE SAVED]
- Root passwords: [DOCUMENT WHERE SAVED]
- ArgoCD admin password: [DOCUMENT WHERE SAVED]

---

**Last Updated**: 2024-12-20  
**Status**: Phase 1 - Ansible Setup
