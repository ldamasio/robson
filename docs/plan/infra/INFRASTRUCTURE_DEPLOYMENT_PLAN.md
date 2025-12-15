# Infrastructure Deployment Plan — Robson Bot

**Status**: Ready to execute
**Scope**: Full infrastructure deployment from bare VPS to production GitOps
**Duration**: 12-16 hours (full path with hardening)
**Last Updated**: 2025-11-23

---

## Quick Reference

| Frente | Objetivo | Duração | Criticidade | Dependências |
|--------|----------|---------|-------------|--------------|
| **F1** | Bootstrap k3s (Ansible + Vault) | 1-2h | 🔴 BLOQUEANTE | None |
| **F2** | Platform (Gateway API + Istio + cert-manager) | 2-3h | 🔴 BLOQUEANTE | F1 |
| **F3** | GitOps (ArgoCD + App-of-Apps) | 1-2h | 🔴 BLOQUEANTE | F2 |
| **F4** | DNS/TLS (Wildcard + Certificates) | 1h + 10min DNS | 🔴 BLOQUEANTE | F3 |
| **F5** | Legacy Migration | 2-4h | 🟡 RECOMENDADO | F4 |
| **F6** | Post-Deploy (Observability + Backup) | 3-4h | 🟢 OPCIONAL | F3 |

**Critical Path (Production Minimum)**: F1 → F2 → F3 → F4 = **6-8 hours**

---

## Cluster Topology

**Target**: 4 VPS Contabo (Ubuntu 24.04)

```
┌─────────────────────────────────────────────┐
│ tiger (158.220.116.31)                      │
│ - Role: k3s server + Gateway                │
│ - RAM: 8GB                                  │
│ - Services: k3s API, Istio Gateway (80/443) │
└─────────────────────────────────────────────┘
          │
          ├──────────────────────┬──────────────────────┐
          ▼                      ▼                      ▼
┌──────────────────┐  ┌──────────────────┐  ┌──────────────────┐
│ bengal           │  │ pantera          │  │ eagle            │
│ 164.68.96.68     │  │ 149.102.139.33   │  │ 167.86.92.97     │
│ - Role: agent    │  │ - Role: agent    │  │ - Role: agent    │
│ - RAM: 8GB       │  │ - RAM: 4GB       │  │ - RAM: 4GB       │
└──────────────────┘  └──────────────────┘  └──────────────────┘
```

**Networking**:
- SSH: Port 5831 (hardened, from vault)
- k3s API: 6443/tcp (server only)
- Gateway: 80/tcp, 443/tcp (tiger only)
- Flannel VXLAN: 8472/udp (all nodes)

**Domains**:
- Base: `robson.rbx.ia.br`
- Backend: `api.robson.rbx.ia.br`
- Frontend: `app.robson.rbx.ia.br`
- Previews: `h-<branch>.robson.rbx.ia.br` (wildcard)

---

## Roadmap (DAG)

```
┌────────────────────────────────┐
│ F1: Bootstrap k3s              │
│ (Ansible + Vault)              │
│ Duration: 1-2h                 │
└────────────┬───────────────────┘
             │
             ▼
┌────────────────────────────────┐
│ F2: Platform Install           │
│ (Gateway API, Istio, cert-mgr) │
│ Duration: 2-3h                 │
└────────────┬───────────────────┘
             │
             ▼
┌────────────────────────────────┐
│ F3: GitOps Setup               │
│ (ArgoCD + App-of-Apps)         │
│ Duration: 1-2h                 │
└────────────┬───────────────────┘
             │
             ▼
┌────────────────────────────────┐
│ F4: DNS/TLS Configuration      │
│ (Wildcard + Let's Encrypt)     │
│ Duration: 1h + DNS propagation │
└───────┬──────────┬─────────────┘
        │          │
        ▼          ▼
  ┌─────────┐  ┌──────────────┐
  │ F5: Leg │  │ F6: Post-Dep │
  │ Migration│  │ (Monitoring) │
  │ 2-4h    │  │ 3-4h         │
  └─────────┘  └──────────────┘
```

**Parallel Execution**: F5 and F6 can run concurrently after F4.

---

## F1: Bootstrap k3s (Ansible + Vault)

**Objective**: Operational k3s cluster with 1 server + 3 agents, hardened SSH, active firewall.

### Prerequisites
- [ ] SSH root access to all 4 nodes (port 22)
- [ ] ed25519 SSH key generated (`~/.ssh/id_ed25519`)
- [ ] Ansible ≥2.10 installed locally
- [ ] Vault password defined

### Execution Steps

#### 1. Prepare Vault
```bash
cd infra/ansible

# Create/edit vault
ansible-vault create group_vars/all/vault.yml
# Or edit existing: ansible-vault edit group_vars/all/vault.yml

# Add to vault:
# vault_ssh_port: 5831
# vault_admin_pubkey: "ssh-ed25519 AAAA... user@host"
# (vault_k3s_token will be added later)
```

#### 2. Test Connectivity
```bash
ansible -i inventory/contabo/hosts.ini all -m ping
```

#### 3. Run Bootstrap
```bash
ansible-playbook -i inventory/contabo/hosts.ini site.yml \
  --ask-vault-pass \
  --tags bootstrap
```

**Actions**:
- Creates `robson` admin user
- Hardens SSH (port 5831, no password auth)
- Configures UFW (deny incoming, allow SSH + k3s ports)

**Validation**:
```bash
ssh -p 5831 robson@158.220.116.31 "sudo ufw status"
```

#### 4. Install k3s Server
```bash
ansible-playbook -i inventory/contabo/hosts.ini site.yml \
  --ask-vault-pass \
  --tags k3s \
  --limit k3s_server
```

#### 5. Capture k3s Token
```bash
ssh -p 5831 robson@158.220.116.31 \
  "sudo cat /var/lib/rancher/k3s/server/node-token"

# Save to vault
ansible-vault edit group_vars/all/vault.yml
# Add: vault_k3s_token: "K10abc..."
```

#### 6. Install k3s Agents
```bash
ansible-playbook -i inventory/contabo/hosts.ini site.yml \
  --ask-vault-pass \
  --tags k3s \
  --limit k3s_agent
```

#### 7. Get Kubeconfig
```bash
scp -P 5831 robson@158.220.116.31:/etc/rancher/k3s/k3s.yaml \
  ~/.kube/config-robson

sed -i 's/127.0.0.1/158.220.116.31/' ~/.kube/config-robson

export KUBECONFIG=~/.kube/config-robson
kubectl get nodes
```

### Done Criteria
- [ ] 4 nodes Ready (1 control-plane + 3 workers)
- [ ] SSH only on port 5831 with key auth
- [ ] UFW active with minimal ports
- [ ] kubeconfig works locally

### Rollback
```bash
ssh -p 5831 robson@158.220.116.31 "/usr/local/bin/k3s-uninstall.sh"
ssh -p 5831 robson@164.68.96.68 "/usr/local/bin/k3s-agent-uninstall.sh"
# (repeat for pantera, eagle)
```

---

## F2: Platform Install (Gateway API + Istio + cert-manager)

**Objective**: Install infrastructure CRDs and components.

### Prerequisites
- [ ] F1 complete (cluster operational)
- [ ] kubeconfig active (`export KUBECONFIG=~/.kube/config-robson`)

### Execution Steps

#### 1. Install ArgoCD (Bootstrap)
```bash
kubectl create namespace argocd

kubectl apply -n argocd -f https://raw.githubusercontent.com/argoproj/argo-cd/v2.10.0/manifests/install.yaml

kubectl wait --for=condition=available --timeout=300s \
  deployment/argocd-server -n argocd

# Get admin password
kubectl -n argocd get secret argocd-initial-admin-secret \
  -o jsonpath="{.data.password}" | base64 -d
```

**Port-forward for Web UI**:
```bash
kubectl port-forward svc/argocd-server -n argocd 8080:443
# Access: https://localhost:8080 (admin / [password])
```

#### 2. Apply Platform Applications
```bash
# Gateway API CRDs
kubectl apply -f infra/k8s/platform/gateway-api-crds/app.yaml

# cert-manager
kubectl apply -f infra/k8s/platform/cert-manager/app.yaml

# Istio Ambient (order matters: base → CNI → istiod → ztunnel)
kubectl apply -f infra/k8s/platform/istio-ambient/base.yaml
kubectl apply -f infra/k8s/platform/istio-ambient/cni.yaml
kubectl apply -f infra/k8s/platform/istio-ambient/istiod.yaml
kubectl apply -f infra/k8s/platform/istio-ambient/ztunnel.yaml
```

**Validation**:
```bash
# Gateway API CRDs
kubectl get crd | grep gateway

# cert-manager (3 pods Running)
kubectl get pods -n cert-manager

# Istio Ambient
kubectl get pods -n istio-system
kubectl get daemonset -n istio-system ztunnel  # Should show 4/4 ready
```

#### 3. Apply ClusterIssuer (Corrected)

**Issue**: Current `cluster-issuer.yaml` only references `robson-backend-gateway`.

**Fix**:
```bash
cat > /tmp/cluster-issuer-fixed.yaml <<'EOF'
apiVersion: cert-manager.io/v1
kind: ClusterIssuer
metadata:
  name: letsencrypt-http01
spec:
  acme:
    email: ldamasio@gmail.com
    server: https://acme-v02.api.letsencrypt.org/directory
    privateKeySecretRef:
      name: letsencrypt-http01
    solvers:
      - http01:
          gatewayHTTPRoute:
            labels:
              acme-solver: "true"
EOF

kubectl apply -f /tmp/cluster-issuer-fixed.yaml
```

**Validation**:
```bash
kubectl get clusterissuer letsencrypt-http01 -o yaml
# Check: status.conditions[type=Ready] = True
```

#### 4. (Optional) Create Staging ClusterIssuer
```bash
cat > /tmp/cluster-issuer-staging.yaml <<'EOF'
apiVersion: cert-manager.io/v1
kind: ClusterIssuer
metadata:
  name: letsencrypt-staging
spec:
  acme:
    email: ldamasio@gmail.com
    server: https://acme-staging-v02.api.letsencrypt.org/directory
    privateKeySecretRef:
      name: letsencrypt-staging
    solvers:
      - http01:
          gatewayHTTPRoute:
            labels:
              acme-solver: "true"
EOF

kubectl apply -f /tmp/cluster-issuer-staging.yaml
```

### Done Criteria
- [ ] Gateway API CRDs installed
- [ ] cert-manager operational (3 pods Running)
- [ ] Istio Ambient complete (istiod + ztunnel on 4 nodes)
- [ ] ClusterIssuer `letsencrypt-http01` Ready
- [ ] ArgoCD Web UI accessible

### Rollback
```bash
kubectl delete application -n argocd --all

kubectl delete -f infra/k8s/platform/istio-ambient/ztunnel.yaml
kubectl delete -f infra/k8s/platform/istio-ambient/istiod.yaml
kubectl delete -f infra/k8s/platform/istio-ambient/cni.yaml
kubectl delete -f infra/k8s/platform/istio-ambient/base.yaml

kubectl delete -f infra/k8s/platform/cert-manager/app.yaml
kubectl delete namespace cert-manager
```

---

## F3: GitOps Setup (ArgoCD + App-of-Apps)

**Objective**: Configure ArgoCD to manage production via App-of-Apps and auto-generate preview environments.

### Prerequisites
- [ ] F2 complete (platform installed)
- [ ] ArgoCD accessible

### Execution Steps

#### 1. Apply Root Application
```bash
export KUBECONFIG=~/.kube/config-robson

kubectl apply -f infra/k8s/gitops/app-of-apps/root.yaml

kubectl get application -n argocd robson-root
```

**This creates 7 child Applications**:
1. `gateway-api-crds`
2. `istio-ambient`
3. `platform-cert-manager`
4. `robson-backend` (prod)
5. `robson-frontend` (prod)
6. `robson-branch-previews` (ApplicationSet)

#### 2. Sync Applications
```bash
# Platform (already installed in F2, but now managed by GitOps)
argocd app sync gateway-api-crds
argocd app sync istio-ambient
argocd app sync platform-cert-manager

# Production apps
argocd app sync robson-backend
argocd app sync robson-frontend
```

**Validation**:
```bash
kubectl get applications -n argocd
# All should show: Synced, Healthy

kubectl get pods -n robson
# Should show: robson-backend-*, robson-frontend-*

kubectl get gateway -n robson
kubectl get httproute -n robson
kubectl get certificate -n robson
```

#### 3. Verify ApplicationSet (Previews)
```bash
kubectl get applicationset -n argocd robson-branch-previews

# Test with dummy branch (optional)
git checkout -b feature/test-preview
git push origin feature/test-preview

# Wait up to 3 min for ArgoCD to detect
watch kubectl get applications -n argocd
# Should create: robson-feature-test-preview

# Cleanup test
git branch -D feature/test-preview
git push origin --delete feature/test-preview
```

#### 4. Configure Webhook (Optional, for instant sync)

**GitHub**:
1. Repo → Settings → Webhooks → Add webhook
2. Payload URL: `https://[ARGOCD_IP]/api/webhook`
3. Content type: `application/json`
4. Secret: [GENERATE_STRONG_TOKEN]
5. Events: `push`, `pull_request`

**ArgoCD**:
```bash
kubectl -n argocd edit configmap argocd-cm
# Add:
# data:
#   webhook.github.secret: "[TOKEN_ABOVE]"
```

### Done Criteria
- [ ] Root Application `robson-root` Synced + Healthy
- [ ] 7 child Applications active
- [ ] Backend + frontend pods Running in `robson` namespace
- [ ] ApplicationSet generating apps for feature branches
- [ ] (Optional) Webhook configured

### Rollback
```bash
# Delete Root Application (cascade deletes children)
kubectl delete application -n argocd robson-root

# Or pause auto-sync
kubectl patch application -n argocd robson-root \
  --type=merge -p '{"spec":{"syncPolicy":{"automated":null}}}'
```

---

## F4: DNS/TLS Configuration (Wildcard + Let's Encrypt)

**Objective**: Configure DNS and validate TLS certificates.

### Prerequisites
- [ ] F3 complete (apps deployed)
- [ ] Gateway LoadBalancer has external IP
- [ ] Access to Registro.br DNS panel

### Execution Steps

#### 1. Get Gateway IP
```bash
export KUBECONFIG=~/.kube/config-robson

kubectl get gateway -n robson robson-backend-gateway -o jsonpath='{.status.addresses[0].value}'
# Output: 158.220.116.31 (tiger node IP)
```

**Note**: k3s LoadBalancer uses node IP. For production, consider MetalLB for stable IPs.

#### 2. Configure DNS at Registro.br

**Required DNS Records**:
```
Type    Name                            Value               TTL
A       api.robson.rbx.ia.br           158.220.116.31      3600
A       app.robson.rbx.ia.br           158.220.116.31      3600
A       *.robson.rbx.ia.br             158.220.116.31      3600  (wildcard)
```

**Validation (wait ~10 min for DNS propagation)**:
```bash
dig +short api.robson.rbx.ia.br
# Should return: 158.220.116.31

dig +short h-test.robson.rbx.ia.br
# Should return: 158.220.116.31 (via wildcard)
```

#### 3. Validate TLS Certificates
```bash
# Check Certificate resources
kubectl get certificate -n robson
# NAME                  READY   SECRET                   AGE
# robson-backend-cert   True    robson-backend-tls       5m
# robson-frontend-cert  True    robson-frontend-tls      5m

# Describe certificate
kubectl describe certificate -n robson robson-backend-cert
# Status.Conditions: Ready=True

# Verify Secret generated
kubectl get secret -n robson robson-backend-tls
# Type: kubernetes.io/tls
```

#### 4. Test HTTPS Endpoints
```bash
# Test backend (may return 503 if app not ready, but TLS should work)
curl -I https://api.robson.rbx.ia.br

# Verify certificate issuer
curl -vI https://api.robson.rbx.ia.br 2>&1 | grep issuer
# Should show: issuer: C=US; O=Let's Encrypt; CN=R3
```

#### 5. Troubleshoot TLS (if Certificate not Ready)
```bash
# cert-manager logs
kubectl logs -n cert-manager deployment/cert-manager -f

# CertificateRequest
kubectl get certificaterequest -n robson
kubectl describe certificaterequest -n robson [NAME]

# HTTP-01 Challenge
kubectl get challenge -n robson
kubectl describe challenge -n robson [NAME]

# Solver pod logs
kubectl logs -n robson [challenge-solver-pod]
```

**Common Issues**:
- **DNS not resolving**: Wait for propagation or fix records
- **Gateway not routing**: Check `kubectl get httproute`
- **Firewall blocking port 80**: Verify UFW allows 80/443 on gateway node
- **Rate limit**: Use staging ClusterIssuer first

### Done Criteria
- [ ] DNS resolving `api.robson.rbx.ia.br` → Gateway IP
- [ ] Wildcard `*.robson.rbx.ia.br` resolving
- [ ] Certificates `Ready=True`
- [ ] HTTPS working (curl returns 200/503, not TLS error)

### Rollback
```bash
# DNS: Remove A records at Registro.br

# Certificates (will regenerate automatically)
kubectl delete certificate -n robson --all
```

---

## F5: Legacy Migration (infra/k8s/prod/)

**Objective**: Extract useful config from legacy, migrate to Helm charts, retire old manifests.

### Prerequisites
- [ ] F4 complete (new apps working with DNS/TLS)
- [ ] Traffic verified on new domains

### Execution Steps

#### 1. Audit Legacy Content

**Useful from legacy**:
- Environment variables (BINANCE keys, DB creds) → Extract to SealedSecrets
- Resource limits (RAM/CPU) → Add to Helm `values.yaml`
- Prometheus annotations → Add to deployment templates

**Obsolete**:
- `*-ingress.yml` → Replaced by Gateway API
- `*-clusterissuer.yml` → Duplicates, already have single ClusterIssuer
- `*-svc.yml` (LoadBalancer) → Helm charts use ClusterIP + Gateway

#### 2. Create Secret Template (for SealedSecrets in F6)
```bash
mkdir -p infra/k8s/secrets

cat > infra/k8s/secrets/rbs-django-secret.yaml <<'EOF'
apiVersion: v1
kind: Secret
metadata:
  name: rbs-django-secret
  namespace: robson
type: Opaque
stringData:
  RBS_SECRET_KEY: "CHANGE_ME"
  RBS_BINANCE_API_KEY_TEST: "CHANGE_ME"
  RBS_BINANCE_SECRET_KEY_TEST: "CHANGE_ME"
  RBS_BINANCE_API_KEY_PROD: "CHANGE_ME"
  RBS_BINANCE_SECRET_KEY_PROD: "CHANGE_ME"
  RBS_BINANCE_API_URL_TEST: "https://testnet.binance.vision/api"
  POSTGRES_DATABASE: "rbsdb"
  POSTGRES_USER: "postgres"
  POSTGRES_PASSWORD: "CHANGE_ME"
  POSTGRES_HOST: "postgres.robson.svc.cluster.local"
  POSTGRES_PORT: "5432"
EOF

# DO NOT COMMIT WITH REAL VALUES
# Use SealedSecrets (see F6)
```

#### 3. Update Helm Charts with Legacy Values

**Backend** (`infra/charts/robson-backend/values.yaml`):
```yaml
resources:
  requests:
    memory: "1000Mi"
    cpu: "750m"
  limits:
    memory: "1000Mi"
    cpu: "1500m"

env:
  - name: RBS_SECRET_KEY
    valueFrom:
      secretKeyRef:
        name: rbs-django-secret
        key: RBS_SECRET_KEY
  # [add all env vars from legacy deployment]

readinessProbe:
  httpGet:
    path: /admin/login/
    port: 8000
  initialDelaySeconds: 15
  periodSeconds: 10
```

**Frontend** (`infra/charts/robson-frontend/values.yaml`):
```yaml
resources:
  requests:
    memory: "100Mi"
    cpu: "250m"
  limits:
    memory: "200Mi"
    cpu: "500m"

podAnnotations:
  prometheus.io/scrape: "true"
  prometheus.io/port: "80"
  prometheus.io/path: "/metrics"
```

**Update templates** to use these values (see Patch 4 in main plan).

#### 4. Redeploy Apps with New Values
```bash
git add infra/charts/
git commit -m "feat(charts): migrate resource limits and env vars from legacy

Migrated from infra/k8s/prod/:
- Resource requests/limits
- Environment variables
- Readiness probes
- Prometheus annotations

Refs: F5-migration"

git push origin main

# ArgoCD auto-syncs
argocd app sync robson-backend
argocd app sync robson-frontend
```

#### 5. Disable Legacy (Gradual)
```bash
# Scale to 0 (test)
kubectl scale deployment -n default rbs-backend-monolith-prod-deploy --replicas=0
kubectl scale deployment -n default rbs-frontend-prod-deploy --replicas=0

# Monitor for 24h

# Delete if OK
kubectl delete -f infra/k8s/prod/
```

#### 6. Archive Legacy in Repo
```bash
mkdir -p infra/k8s/archive
git mv infra/k8s/prod/ infra/k8s/archive/prod-deprecated-2025-11

git commit -m "chore(infra): archive legacy Ingress-based manifests

Migrated to Helm charts with Gateway API.
Archived for reference.

Refs: F5-migration"

git push origin main
```

### Done Criteria
- [ ] Secrets migrated to SealedSecrets or vault
- [ ] Helm charts contain all useful values from legacy
- [ ] New apps running with correct resource limits
- [ ] Legacy disabled (replicas=0 or deleted)
- [ ] Legacy manifests archived in Git

### Rollback
```bash
# Reactivate legacy
kubectl apply -f infra/k8s/archive/prod-deprecated-2025-11/
kubectl scale deployment -n default rbs-backend-monolith-prod-deploy --replicas=1
```

---

## F6: Post-Deploy (Observability + Backup + Hardening)

**Objective**: Install non-blocking components: monitoring, backups, secrets management.

### Prerequisites
- [ ] F5 complete (production stable)
- [ ] Apps running ≥24h without incidents

### Execution Steps

#### 6.1: Install SealedSecrets
```bash
export KUBECONFIG=~/.kube/config-robson

# Install controller
kubectl apply -f https://github.com/bitnami-labs/sealed-secrets/releases/download/v0.24.0/controller.yaml

kubectl wait --for=condition=available --timeout=120s \
  deployment/sealed-secrets-controller -n kube-system

# Install kubeseal CLI
wget https://github.com/bitnami-labs/sealed-secrets/releases/download/v0.24.0/kubeseal-0.24.0-linux-amd64.tar.gz
tar -xvzf kubeseal-0.24.0-linux-amd64.tar.gz
sudo mv kubeseal /usr/local/bin/
```

**CRITICAL: Backup Master Key**
```bash
kubectl get secret -n kube-system \
  -l sealedsecrets.bitnami.com/sealed-secrets-key=active \
  -o yaml > sealed-secrets-master-key-backup.yaml

# Store OFFLINE (1Password, physical vault)
# NEVER COMMIT TO GIT
```

**Seal and Apply Secret**:
```bash
# Seal the secret template from F5
kubeseal -f infra/k8s/secrets/rbs-django-secret.yaml \
  -w infra/k8s/secrets/rbs-django-sealed-secret.yaml \
  --controller-namespace=kube-system

# Commit sealed version
git add infra/k8s/secrets/rbs-django-sealed-secret.yaml
git commit -m "feat(secrets): add sealed Django secret"
git push

# Apply
kubectl apply -f infra/k8s/secrets/rbs-django-sealed-secret.yaml

# Verify unsealed secret created
kubectl get secret -n robson rbs-django-secret
```

#### 6.2: Install Prometheus Stack
```bash
helm repo add prometheus-community https://prometheus-community.github.io/helm-charts
helm repo update

helm install kube-prometheus-stack prometheus-community/kube-prometheus-stack \
  --namespace monitoring --create-namespace \
  --set prometheus.prometheusSpec.serviceMonitorSelectorNilUsesHelmValues=false \
  --set grafana.adminPassword=[SECURE_PASSWORD]
```

**Access Grafana**:
```bash
kubectl port-forward -n monitoring svc/kube-prometheus-stack-grafana 3000:80
# http://localhost:3000
# Credentials: admin / [password]
```

#### 6.3: Install Loki (Logging)
```bash
helm repo add grafana https://grafana.github.io/helm-charts
helm repo update

helm install loki grafana/loki-stack \
  --namespace monitoring \
  --set promtail.enabled=true \
  --set grafana.enabled=false  # Already installed
```

**Add Loki datasource in Grafana**:
- Grafana UI → Configuration → Data Sources → Add Loki
- URL: `http://loki.monitoring.svc.cluster.local:3100`

#### 6.4: Install Velero (Backup)
```bash
# Install CLI
wget https://github.com/vmware-tanzu/velero/releases/download/v1.12.0/velero-v1.12.0-linux-amd64.tar.gz
tar -xvzf velero-v1.12.0-linux-amd64.tar.gz
sudo mv velero-v1.12.0-linux-amd64/velero /usr/local/bin/

# Install server (example with MinIO)
kubectl apply -f https://raw.githubusercontent.com/vmware-tanzu/velero/main/examples/minio/00-minio-deployment.yaml

velero install \
  --provider aws \
  --plugins velero/velero-plugin-for-aws:v1.8.0 \
  --bucket velero \
  --secret-file ./credentials-velero \
  --use-volume-snapshots=false \
  --backup-location-config region=minio,s3ForcePathStyle="true",s3Url=http://minio.velero.svc:9000
```

**Create Backup Schedule**:
```bash
velero schedule create daily-backup \
  --schedule="0 2 * * *" \
  --include-namespaces robson,istio-system,cert-manager

# Test manual backup
velero backup create test-backup --include-namespaces robson
velero backup describe test-backup
```

#### 6.5: Hardening (NetworkPolicies + PodSecurity)

**Default deny**:
```bash
cat > /tmp/deny-all.yaml <<'EOF'
apiVersion: networking.k8s.io/v1
kind: NetworkPolicy
metadata:
  name: deny-all-ingress
  namespace: robson
spec:
  podSelector: {}
  policyTypes:
    - Ingress
EOF

kubectl apply -f /tmp/deny-all.yaml
```

**Allow backend from gateway**:
```bash
cat > /tmp/allow-backend.yaml <<'EOF'
apiVersion: networking.k8s.io/v1
kind: NetworkPolicy
metadata:
  name: allow-backend-from-gateway
  namespace: robson
spec:
  podSelector:
    matchLabels:
      app: robson-backend
  ingress:
    - from:
        - namespaceSelector:
            matchLabels:
              name: istio-system
      ports:
        - protocol: TCP
          port: 8000
EOF

kubectl apply -f /tmp/allow-backend.yaml
```

**PodSecurityStandards**:
```bash
kubectl label namespace robson \
  pod-security.kubernetes.io/enforce=restricted \
  pod-security.kubernetes.io/audit=restricted \
  pod-security.kubernetes.io/warn=restricted
```

### Done Criteria
- [ ] SealedSecrets installed, master key backed up
- [ ] Prometheus + Grafana collecting metrics
- [ ] Loki collecting logs
- [ ] Velero with daily backup schedule
- [ ] NetworkPolicies applied
- [ ] PodSecurityStandards enforced

### Rollback
```bash
# Remove components
kubectl delete namespace velero
helm uninstall kube-prometheus-stack -n monitoring
helm uninstall loki -n monitoring
kubectl delete -f https://github.com/bitnami-labs/sealed-secrets/releases/download/v0.24.0/controller.yaml
```

---

## Critical Risks & Mitigations

### 🔴 Critical Risks

| Risk | Impact | Mitigation |
|------|--------|------------|
| **Vault not encrypted** | SSH keys, k3s token exposed | ✅ Use `ansible-vault encrypt` mandatory; validate with `ansible-vault view` |
| **DNS not propagating** | TLS fails, apps inaccessible | ✅ Test with `dig` before applying Certificates; use staging ClusterIssuer |
| **Let's Encrypt rate limit** | Certs blocked 7 days | ✅ Use staging first; validate HTTP-01 challenge without TLS |
| **Gateway IP changes** | DNS points to wrong IP | ✅ Document IP in operations docs; consider MetalLB |
| **ArgoCD no auth** | Cluster exposed | ✅ Change admin password; configure RBAC; enable TLS on Gateway |
| **SealedSecrets key lost** | Secrets unrecoverable | ✅ Offline backup of master key; test restore in dev |

### 🟡 Medium Risks

| Risk | Impact | Mitigation |
|------|--------|------------|
| **No resource limits** | OOM kills, cluster unstable | ✅ Apply limits from legacy (F5); configure LimitRanges |
| **Single replica** | Downtime in rolling updates | ✅ Set `replicaCount: 2` in prod; add PodDisruptionBudget |
| **PostgreSQL not managed** | Data loss, no backups | ⚠️ Install CloudNativePG operator or use external RDS; Velero for PVCs |
| **Legacy still active** | Drift between old/new | ✅ Scale to 0 before deleting; monitor 24h |

---

## Emergency Commands

### Rollback Total (Destroy Cluster)
```bash
# CAUTION: DESTRUCTIVE
ssh -p 5831 robson@158.220.116.31 "/usr/local/bin/k3s-uninstall.sh"
ssh -p 5831 robson@164.68.96.68 "/usr/local/bin/k3s-agent-uninstall.sh"
ssh -p 5831 robson@149.102.139.33 "/usr/local/bin/k3s-agent-uninstall.sh"
ssh -p 5831 robson@167.86.92.97 "/usr/local/bin/k3s-agent-uninstall.sh"
```

### Pause GitOps
```bash
kubectl patch application -n argocd robson-root \
  --type=merge -p '{"metadata":{"finalizers":null}}'
kubectl delete application -n argocd robson-root
```

### Disable cert-manager (Rate Limit Hit)
```bash
kubectl scale deployment -n cert-manager cert-manager --replicas=0
kubectl delete clusterissuer letsencrypt-http01
```

### Access Cluster if SSH Fails
```bash
# Via Contabo console (KVM/VNC)
# Login as root (port 22 still open if ssh_keep_port_22=true)
ssh -p 22 root@158.220.116.31

# Revert UFW
ufw disable
```

---

## Pending Decisions

| Variable | File | Status | Action Required |
|----------|------|--------|-----------------|
| `vault_ssh_port` | `group_vars/all/vault.yml` | ⚠️ Not defined | Add `5831` to vault |
| `vault_admin_pubkey` | `group_vars/all/vault.yml` | ⚠️ Not defined | Generate ed25519 key and add |
| `vault_k3s_token` | `group_vars/all/vault.yml` | ⚠️ Obtained after F1 | Capture from server after install |
| `rbs-django-secret` | `infra/k8s/secrets/` | ❌ Not versioned | Create SealedSecret in F6 |
| Gateway LB IP | Documentation | ⚠️ Not documented | Record in `docs/OPERATIONS.md` after F2 |
| ArgoCD admin password | `platform/argocd/app.yaml` | 🔴 Default insecure | Change after install (F2) |
| DNS wildcard | Registro.br | ⚠️ Manual config | Execute in F4 |

---

## Pre-Execution Checklist

### Before F1
- [ ] ed25519 SSH key generated (`ssh-keygen -t ed25519`)
- [ ] SSH root access to 4 nodes confirmed (port 22)
- [ ] Ansible ≥2.10 installed (`ansible --version`)
- [ ] Vault password defined and stored securely
- [ ] Backup of 4 nodes (if critical data exists)

### Before F4
- [ ] Access to Registro.br DNS panel
- [ ] Decision: Manual wildcard or external-dns?
- [ ] Gateway LoadBalancer IP documented

### Before F5
- [ ] New apps tested and validated (F3 + F4 complete)
- [ ] Secrets migration plan (SealedSecrets or SOPS)
- [ ] Maintenance window scheduled (low traffic)

### Before F6
- [ ] Storage backend for Velero defined (MinIO, S3, GCS)
- [ ] Grafana password defined (change `admin`)
- [ ] Backup retention requirements (7/30/90 days)

---

## Documentation to Create (Post-Implementation)

| Document | Content | Priority |
|----------|---------|----------|
| `docs/OPERATIONS.md` | IPs, credentials, deploy procedures | 🔴 High |
| `docs/RUNBOOK.md` | Troubleshooting, rollback, disaster recovery | 🔴 High |
| `docs/ARCHITECTURE.md` | Network diagram, TLS flow, Istio Ambient | 🟡 Medium |
| `docs/adr/ADR-0007-sealed-secrets.md` | Decision: SealedSecrets vs SOPS | 🟢 Low |
| `infra/README.md` (update) | Add F1-F6 as reference | 🔴 High |

---

## References

- **Existing Plans**:
  - `docs/plan/infra/ANSIBLE_BOOTSTRAP_PLAN.md` - F1 details
  - `docs/plan/infra/TLS_CERT_MANAGER_HTTP01.md` - F4 TLS details
  - `docs/plan/infra/dns/` - DNS configuration guides

- **Codebase Docs**:
  - `CLAUDE.md` - Project context for AI agents
  - `docs/AGENTS.md` - Complete AI development guide
  - `docs/ARCHITECTURE.md` - Architecture overview
  - `docs/adr/` - Architecture Decision Records

- **Infrastructure Code**:
  - `infra/ansible/` - Bootstrap and k3s roles
  - `infra/k8s/platform/` - Platform components
  - `infra/k8s/gitops/` - ArgoCD configuration
  - `infra/charts/` - Helm charts for apps

---

**Status**: Ready for execution
**Author**: Claude Code (Sonnet 4.5)
**Mode**: Interactive planning → Autonomous execution (per frente)
**Tags**: `plan:infra` `deployment` `gitops` `k3s` `istio-ambient`
