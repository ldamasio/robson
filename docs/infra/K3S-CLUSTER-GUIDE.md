# K3s Cluster Deployment Guide

**Guide for AI Agents to deploy applications to the production k3s cluster.**

This document explains how to deploy new applications to the shared k3s cluster managed by ArgoCD.

---

## 🏗️ Cluster Overview

### Infrastructure

| Node | Hostname | IP | Role |
|------|----------|-----|------|
| tiger | `tiger.rbx.ia.br` | `158.220.116.31` | control-plane (server) |
| bengal | `bengal.rbx.ia.br` | `164.68.96.68` | agent |
| pantera | `pantera.rbx.ia.br` | `149.102.139.33` | agent |
| eagle | `eagle.rbx.ia.br` | `167.86.92.97` | agent |

### Platform Components

| Component | Version | Purpose |
|-----------|---------|---------|
| **k3s** | 1.33.6 | Kubernetes distribution |
| **Traefik** | (bundled) | Ingress controller |
| **ArgoCD** | 2.10.0 | GitOps continuous deployment |
| **cert-manager** | 1.13.0 | Automatic SSL certificates (Let's Encrypt) |

### Management URLs

| Service | URL | Credentials |
|---------|-----|-------------|
| ArgoCD UI | https://argocd.robson.rbx.ia.br | admin / `6LzfEG9USLpv2cz0` |

---

## 🚀 How to Deploy a New Application

### Prerequisites

Your repository must have:
1. **Dockerfile** - To build the container image
2. **GitHub Actions workflow** - To build and push to Docker Hub
3. **Docker Hub credentials** - Secrets `DOCKERHUB_USER` and `DOCKERHUB_PWD`

### Step 1: Verify Your CI/CD Pipeline

Your GitHub Actions should push images to Docker Hub with tags like:
- `your-dockerhub-user/your-app:latest`
- `your-dockerhub-user/your-app:0.0.{run_number}` (optional versioning)

**Example workflow (Next.js):**

```yaml
name: Production CI/CD Pipeline

on:
  push:
    branches: ["main"]

jobs:
  build-and-push:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - name: Docker Login
        uses: docker/login-action@v3
        with:
          username: ${{ secrets.DOCKERHUB_USER }}
          password: ${{ secrets.DOCKERHUB_PWD }}

      - name: Build and push Docker image
        uses: docker/build-push-action@v5
        with:
          context: ./
          file: ./docker/Dockerfile
          push: true
          tags: |
            ${{ secrets.DOCKERHUB_USER }}/my-app:latest
            ${{ secrets.DOCKERHUB_USER }}/my-app:${{ github.sha }}
```

### Step 2: Create Kubernetes Manifests

Create these files in your repository (or in a dedicated infra repo):

#### 2.1 Namespace (optional - can share with others)

```yaml
# k8s/namespace.yaml
apiVersion: v1
kind: Namespace
metadata:
  name: websites
```

#### 2.2 Deployment

```yaml
# k8s/deployment.yaml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: my-app
  namespace: websites
  labels:
    app: my-app
spec:
  replicas: 1
  selector:
    matchLabels:
      app: my-app
  template:
    metadata:
      labels:
        app: my-app
    spec:
      containers:
      - name: my-app
        image: ldamasio/my-app:latest  # Your Docker Hub image
        ports:
        - containerPort: 3000  # Next.js default port
        resources:
          requests:
            memory: "256Mi"
            cpu: "100m"
          limits:
            memory: "512Mi"
            cpu: "500m"
```

#### 2.3 Service

```yaml
# k8s/service.yaml
apiVersion: v1
kind: Service
metadata:
  name: my-app
  namespace: websites
  labels:
    app: my-app
spec:
  type: ClusterIP
  ports:
  - port: 80
    targetPort: 3000
    protocol: TCP
  selector:
    app: my-app
```

#### 2.4 ClusterIssuer (for SSL)

```yaml
# k8s/clusterissuer.yaml
apiVersion: cert-manager.io/v1
kind: ClusterIssuer
metadata:
  name: my-app-letsencrypt
spec:
  acme:
    email: your-email@example.com
    server: https://acme-v02.api.letsencrypt.org/directory
    privateKeySecretRef:
      name: my-app-letsencrypt-key
    solvers:
    - http01:
        ingress:
          class: traefik
```

#### 2.5 Ingress (with SSL)

```yaml
# k8s/ingress.yaml
apiVersion: networking.k8s.io/v1
kind: Ingress
metadata:
  name: my-app-ingress
  namespace: websites
  annotations:
    cert-manager.io/cluster-issuer: "my-app-letsencrypt"
spec:
  ingressClassName: traefik
  tls:
  - hosts:
    - myapp.example.com
    secretName: my-app-tls
  rules:
  - host: myapp.example.com
    http:
      paths:
      - path: /
        pathType: Prefix
        backend:
          service:
            name: my-app
            port:
              number: 80
```

### Step 3: Create ArgoCD Application

Add an ArgoCD Application manifest to deploy via GitOps:

```yaml
# argocd/my-app.yaml
apiVersion: argoproj.io/v1alpha1
kind: Application
metadata:
  name: my-app
  namespace: argocd
spec:
  project: default
  source:
    repoURL: https://github.com/your-user/your-repo
    targetRevision: main
    path: k8s  # Path to your k8s manifests
  destination:
    server: https://kubernetes.default.svc
    namespace: websites
  syncPolicy:
    automated:
      prune: true
      selfHeal: true
    syncOptions:
      - CreateNamespace=true
```

### Step 4: Configure DNS

Add an A record in your DNS provider:

| Type | Name | Value |
|------|------|-------|
| A | `myapp.example.com` | `158.220.116.31` |

### Step 5: Deploy

**Option A: Apply ArgoCD Application directly**

```bash
ssh root@158.220.116.31 "kubectl apply -f /path/to/argocd/my-app.yaml"
```

**Option B: Add to this repository's ArgoCD applications**

Add your Application manifest to:
```
robson/infra/k8s/gitops/applications/my-app.yaml
```

---

## 📋 Example: Deploying leandrodamasio.com

### Repository Info

- **GitHub**: https://github.com/ldamasio/lda-front
- **Docker Image**: `ldamasio/lda-front:latest`
- **Domain**: `leandrodamasio.com`
- **Port**: 3000 (Next.js)

### Required Manifests

```yaml
# Deployment
apiVersion: apps/v1
kind: Deployment
metadata:
  name: lda-front
  namespace: websites
spec:
  replicas: 1
  selector:
    matchLabels:
      app: lda-front
  template:
    metadata:
      labels:
        app: lda-front
    spec:
      containers:
      - name: lda-front
        image: ldamasio/lda-front:latest
        ports:
        - containerPort: 3000
---
# Service
apiVersion: v1
kind: Service
metadata:
  name: lda-front
  namespace: websites
spec:
  type: ClusterIP
  ports:
  - port: 80
    targetPort: 3000
  selector:
    app: lda-front
---
# Ingress
apiVersion: networking.k8s.io/v1
kind: Ingress
metadata:
  name: lda-front-ingress
  namespace: websites
  annotations:
    cert-manager.io/cluster-issuer: "lda-front-letsencrypt"
spec:
  ingressClassName: traefik
  tls:
  - hosts:
    - leandrodamasio.com
    - www.leandrodamasio.com
    secretName: lda-front-tls
  rules:
  - host: leandrodamasio.com
    http:
      paths:
      - path: /
        pathType: Prefix
        backend:
          service:
            name: lda-front
            port:
              number: 80
  - host: www.leandrodamasio.com
    http:
      paths:
      - path: /
        pathType: Prefix
        backend:
          service:
            name: lda-front
            port:
              number: 80
```

### DNS Configuration

| Type | Name | Value |
|------|------|-------|
| A | `leandrodamasio.com` | `158.220.116.31` |
| A | `www.leandrodamasio.com` | `158.220.116.31` |

---

## 📋 Example: Deploying rbx.ia.br

### Repository Info

- **GitHub**: https://github.com/rbxrobotica/rbx-robotica-frontend
- **Docker Image**: `ldamasio/rbx-frontend-prod:latest`
- **Domain**: `rbx.ia.br` and `www.rbx.ia.br`
- **Port**: 3000 (Next.js)

### Required Manifests

```yaml
# Deployment
apiVersion: apps/v1
kind: Deployment
metadata:
  name: rbx-frontend
  namespace: websites
spec:
  replicas: 1
  selector:
    matchLabels:
      app: rbx-frontend
  template:
    metadata:
      labels:
        app: rbx-frontend
    spec:
      containers:
      - name: rbx-frontend
        image: ldamasio/rbx-frontend-prod:latest
        ports:
        - containerPort: 3000
---
# Service
apiVersion: v1
kind: Service
metadata:
  name: rbx-frontend
  namespace: websites
spec:
  type: ClusterIP
  ports:
  - port: 80
    targetPort: 3000
  selector:
    app: rbx-frontend
---
# Ingress
apiVersion: networking.k8s.io/v1
kind: Ingress
metadata:
  name: rbx-frontend-ingress
  namespace: websites
  annotations:
    cert-manager.io/cluster-issuer: "rbx-frontend-letsencrypt"
spec:
  ingressClassName: traefik
  tls:
  - hosts:
    - rbx.ia.br
    - www.rbx.ia.br
    secretName: rbx-frontend-tls
  rules:
  - host: rbx.ia.br
    http:
      paths:
      - path: /
        pathType: Prefix
        backend:
          service:
            name: rbx-frontend
            port:
              number: 80
  - host: www.rbx.ia.br
    http:
      paths:
      - path: /
        pathType: Prefix
        backend:
          service:
            name: rbx-frontend
            port:
              number: 80
```

---

## 🔧 Useful Commands

### SSH Access

```bash
# Connect to k3s server
ssh root@158.220.116.31
```

### kubectl Commands

```bash
# List all pods
kubectl get pods --all-namespaces

# List pods in a namespace
kubectl get pods -n websites

# View logs
kubectl logs -n websites deployment/my-app

# Describe a resource
kubectl describe deployment my-app -n websites

# Apply manifests
kubectl apply -f my-manifest.yaml

# Delete resources
kubectl delete -f my-manifest.yaml

# Restart deployment
kubectl rollout restart deployment/my-app -n websites
```

### ArgoCD Commands

```bash
# List applications
kubectl get applications -n argocd

# Sync an application
kubectl patch application my-app -n argocd --type merge -p '{"operation":{"sync":{}}}'

# Check application status
kubectl get application my-app -n argocd -o yaml
```

---

## 🔒 Security Notes

1. **Never commit secrets to Git** - Use Kubernetes Secrets or external secret managers
2. **Use HTTPS** - All ingresses should have TLS configured via cert-manager
3. **Resource limits** - Always set CPU/memory limits to prevent resource exhaustion
4. **Image tags** - Prefer SHA-based tags over `latest` for production stability

### Binance API IP Restrictions

The production Binance API key (`RBS_BINANCE_API_KEY_PROD`) is **restricted to cluster IPs only**:

| Node | IP Address | Allowed |
|------|------------|---------|
| tiger | 158.220.116.31 | ✅ |
| bengal | 164.68.96.68 | ✅ |
| pantera | 149.102.139.33 | ✅ |
| eagle | 167.86.92.97 | ✅ |
| Local dev machines | * | ❌ |

⚠️ **Trading commands can ONLY be executed from within the cluster.**

If you get `APIError(code=-2015)`, your IP is not authorized. Execute via:
```bash
ssh root@158.220.116.31
kubectl exec -n robson <backend-pod> -- python manage.py <command>
```

---

## 📞 Support

- **Cluster Admin**: Leandro Damasio (ldamasio@gmail.com)
- **ArgoCD UI**: https://argocd.robson.rbx.ia.br
- **Repository**: https://github.com/ldamasio/robson

---

**Last Updated**: 2024-12-21


