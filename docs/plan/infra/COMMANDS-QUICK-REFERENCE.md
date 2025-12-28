# Quick Command Reference - Copy & Paste

All commands ready to copy-paste for quick deployment.

---

## 🔧 STEP 1: Prepare SSH Keys

```bash
# Clean old host keys
ssh-keygen -R 158.220.116.31
ssh-keygen -R 164.68.96.68
ssh-keygen -R 149.102.139.33
ssh-keygen -R 167.86.92.97

# Get your public key (SAVE THIS OUTPUT)
cat ~/.ssh/id_ed25519.pub
```

---

## 🔐 STEP 2: Create Vault

```bash
cd /c/app/notes/robson/infra/ansible

# Archive old vault
git mv group_vars/all/vault.yml group_vars/all/vault.yml.orphaned-2024-12

# Create new vault
podman run --rm -it \
  -v "C:/app/notes/robson/infra/ansible:/work" -w /work \
  docker.io/alpine/ansible:latest \
  ansible-vault create group_vars/all/vault.yml
```

**Paste this content** (replace YOUR_PUBKEY with output from step 1):

```yaml
---
vault_ssh_port: 22
vault_admin_pubkey: "YOUR_PUBKEY_HERE"
```

Save: ESC, `:wq`, ENTER

---

## 📝 STEP 3: Edit Inventory

Edit file: `infra/ansible/inventory/contabo/hosts.ini`

Replace entire content with:

```ini
[k3s_server]
tiger  ansible_host=158.220.116.31 ansible_user=root

[k3s_agent]
bengal ansible_host=164.68.96.68 ansible_user=root
pantera ansible_host=149.102.139.33 ansible_user=root
eagle   ansible_host=167.86.92.97 ansible_user=root

[k3s_gateway]
tiger
```

**Note**: Passwords are in `passwords.yml` (not in Git)

---

## 🔍 STEP 4: Test Connectivity

```bash
cd /c/app/notes/robson/infra/ansible

podman run --rm -it \
  -e ANSIBLE_HOST_KEY_CHECKING=False \
  -v "C:/app/notes/robson/infra/ansible:/work" -w /work \
  docker.io/alpine/ansible:latest \
  ansible -i inventory/contabo/hosts.ini all -m ping \
  --extra-vars "@inventory/contabo/passwords.yml" \
  --ask-vault-pass
```

Expected: `SUCCESS => { "ping": "pong" }` for all 4 hosts

---

## 🚀 STEP 5: Install k3s Server

```bash
cd /c/app/notes/robson/infra/ansible

podman run --rm -it \
  -e ANSIBLE_HOST_KEY_CHECKING=False \
  -v "C:/app/notes/robson/infra/ansible:/work" -w /work \
  docker.io/alpine/ansible:latest \
  ansible-playbook -i inventory/contabo/hosts.ini \
  playbooks/k3s-simple-install.yml \
  --ask-vault-pass \
  --extra-vars "@inventory/contabo/passwords.yml" \
  --limit k3s_server
```

---

## 🔑 STEP 6: Get k3s Token

```bash
# Get token from server (SAVE THIS OUTPUT)
ssh root@158.220.116.31 "cat /var/lib/rancher/k3s/server/node-token"
```

---

## 📝 STEP 7: Add Token to Vault

```bash
cd /c/app/notes/robson/infra/ansible

podman run --rm -it \
  -v "C:/app/notes/robson/infra/ansible:/work" -w /work \
  docker.io/alpine/ansible:latest \
  ansible-vault edit group_vars/all/vault.yml
```

Add this line (replace with your token):

```yaml
vault_k3s_token: "K10YOUR_TOKEN_HERE"
```

Save: ESC, `:wq`, ENTER

---

## 🚀 STEP 8: Install k3s Agents

```bash
cd /c/app/notes/robson/infra/ansible

podman run --rm -it \
  -e ANSIBLE_HOST_KEY_CHECKING=False \
  -v "C:/app/notes/robson/infra/ansible:/work" -w /work \
  docker.io/alpine/ansible:latest \
  ansible-playbook -i inventory/contabo/hosts.ini \
  playbooks/k3s-simple-install.yml \
  --ask-vault-pass \
  --extra-vars "@inventory/contabo/passwords.yml" \
  --limit k3s_agent
```

---

## 📦 STEP 9: Get Kubeconfig

```bash
# Copy kubeconfig
scp root@158.220.116.31:/etc/rancher/k3s/k3s.yaml ~/.kube/config-robson

# Fix server IP
sed -i 's/127.0.0.1/158.220.116.31/' ~/.kube/config-robson

# Set as default
export KUBECONFIG=~/.kube/config-robson

# Test cluster
kubectl get nodes
```

Expected: 4 nodes Ready

---

## 🎯 STEP 10: Install ArgoCD

```bash
# Create namespace
kubectl create namespace argocd

# Install ArgoCD
kubectl apply -n argocd -f https://raw.githubusercontent.com/argoproj/argo-cd/v2.10.0/manifests/install.yaml

# Wait for ready
kubectl wait --for=condition=available --timeout=300s deployment/argocd-server -n argocd

# Get admin password (SAVE THIS)
kubectl -n argocd get secret argocd-initial-admin-secret -o jsonpath="{.data.password}" | base64 -d
echo
```

---

## 📦 STEP 11: Install Platform Components

```bash
# cert-manager
kubectl apply -f https://github.com/cert-manager/cert-manager/releases/download/v1.13.0/cert-manager.yaml
kubectl wait --for=condition=available --timeout=300s deployment/cert-manager -n cert-manager

# Gateway API CRDs
kubectl apply -f https://github.com/kubernetes-sigs/gateway-api/releases/download/v1.0.0/standard-install.yaml
```

---

## 🔐 STEP 12: Create Secrets

```bash
# Create namespace
kubectl create namespace robson

# Create secret (REPLACE ALL CHANGE_ME VALUES)
kubectl create secret generic rbs-django-secret \
  --namespace=robson \
  --from-literal=RBS_SECRET_KEY='CHANGE_ME_DJANGO_SECRET' \
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

---

## 🐳 STEP 13: Update Image Tags

Get latest SHA from: https://github.com/ldamasio/robson/actions

```bash
cd /c/app/notes/robson

# Replace with actual SHA (e.g., sha-a1b2c3d)
sed -i 's/sha-CHANGEME/sha-YOUR_SHA_HERE/g' infra/k8s/prod/*.yml

# Commit
git add infra/k8s/prod/*.yml
git commit -m "deploy: update images to production SHA"
git push
```

---

## 🚀 STEP 14: Deploy Application

```bash
# Apply ArgoCD Application
kubectl apply -f infra/k8s/gitops/applications/robson-prod.yml

# Watch pods
kubectl get pods -n robson -w
```

Expected: 3 pods Running

---

## 🌐 STEP 15: Configure DNS

At Registro.br panel, add:

```
A    api.robson.rbx.ia.br     158.220.116.31    3600
A    app.robson.rbx.ia.br     158.220.116.31    3600
```

---

## ✅ STEP 16: Verify

```bash
# Wait 10-15 min for DNS propagation
dig +short api.robson.rbx.ia.br

# Check certificates
kubectl get certificate -n robson

# Test HTTPS
curl -I https://api.robson.rbx.ia.br
curl -I https://app.robson.rbx.ia.br
```

---

## 🎉 SUCCESS!

Your production cluster is ready when:

- `kubectl get nodes` shows 4 Ready nodes
- `kubectl get pods -n robson` shows 3 Running pods
- DNS resolves correctly
- HTTPS endpoints return valid certificates
