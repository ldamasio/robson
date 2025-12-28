# ArgoCD Initial Setup - Robson Production

## Overview

This guide walks through the initial deployment of Robson to production using ArgoCD in a simple, non App-of-Apps pattern.

---

## Prerequisites

- [ ] k3s cluster running (tiger + agents)
- [ ] ArgoCD installed in the cluster
- [ ] kubectl configured with cluster access
- [ ] Namespace `robson` created (or will be auto-created)
- [ ] Secret `rbs-django-secret` created in namespace `robson`
- [ ] Images built and pushed to Docker Hub with SHA tags

---

## Step 1: Verify ArgoCD is Running

```bash
# Check ArgoCD pods
kubectl get pods -n argocd

# Get ArgoCD admin password (if needed)
kubectl -n argocd get secret argocd-initial-admin-secret -o jsonpath="{.data.password}" | base64 -d
echo

# Port forward to access UI (optional)
kubectl port-forward svc/argocd-server -n argocd 8080:443
# Access: https://localhost:8080
```

---

## Step 2: Create Namespace and Secret

```bash
# Create namespace
kubectl create namespace robson

# Create secret (replace values with actual credentials)
kubectl create secret generic rbs-django-secret \
  --namespace=robson \
  --from-literal=RBS_SECRET_KEY='your-django-secret-key-here' \
  --from-literal=RBS_BINANCE_API_KEY_TEST='binance-testnet-api-key' \
  --from-literal=RBS_BINANCE_SECRET_KEY_TEST='binance-testnet-secret-key' \
  --from-literal=RBS_BINANCE_API_KEY_PROD='binance-prod-api-key' \
  --from-literal=RBS_BINANCE_SECRET_KEY_PROD='binance-prod-secret-key' \
  --from-literal=RBS_BINANCE_API_URL_TEST='https://testnet.binance.vision' \
  --from-literal=POSTGRES_DATABASE='rbsdb' \
  --from-literal=POSTGRES_USER='robson' \
  --from-literal=POSTGRES_PASSWORD='secure-postgres-password' \
  --from-literal=POSTGRES_HOST='postgres.robson.svc.cluster.local' \
  --from-literal=POSTGRES_PORT='5432'

# Verify secret
kubectl get secret rbs-django-secret -n robson
```

---

## Step 3: Update Manifests with Real SHA Tags

Before applying the ArgoCD Application, update the image tags in `infra/k8s/prod/*.yml`:

```bash
# Get latest SHA from GitHub Actions or git
LATEST_SHA=$(git rev-parse --short HEAD)
echo "Latest SHA: sha-${LATEST_SHA}"

# Update manifests (manually or via sed)
# Example:
# sed -i "s/sha-CHANGEME/sha-${LATEST_SHA}/g" infra/k8s/prod/rbs-frontend-prod-deploy.yml
# sed -i "s/sha-CHANGEME/sha-${LATEST_SHA}/g" infra/k8s/prod/rbs-backend-monolith-prod-deploy.yml
# sed -i "s/sha-CHANGEME/sha-${LATEST_SHA}/g" infra/k8s/prod/rbs-backend-nginx-prod-deploy.yml

# Commit and push
git add infra/k8s/prod/*.yml
git commit -m "deploy: update images to sha-${LATEST_SHA}"
git push
```

---

## Step 4: Apply ArgoCD Application

```bash
# Apply the Application manifest
kubectl apply -f infra/k8s/gitops/applications/robson-prod.yml

# Output should be:
# application.argoproj.io/robson-prod created
```

---

## Step 5: Verify Deployment

### Check ArgoCD Application Status

```bash
# Get application status
argocd app get robson-prod

# Expected output:
# Name:               robson-prod
# Project:            default
# Server:             https://kubernetes.default.svc
# Namespace:          robson
# URL:                https://argocd.example.com/applications/robson-prod
# Repo:               https://github.com/ldamasio/robson
# Target:             main
# Path:               infra/k8s/prod
# SyncWindow:         Sync Allowed
# Sync Policy:        Automated (Prune)
# Sync Status:        Synced to main (abc123)
# Health Status:      Healthy
```

### Check Kubernetes Resources

```bash
# Check all resources in robson namespace
kubectl get all -n robson

# Expected output:
# NAME                                                  READY   STATUS    RESTARTS   AGE
# pod/rbs-frontend-prod-deploy-xxxxx                    1/1     Running   0          2m
# pod/rbs-backend-monolith-prod-deploy-xxxxx            1/1     Running   0          2m
# pod/rbs-backend-nginx-prod-deploy-xxxxx               1/1     Running   0          2m
#
# NAME                                       TYPE        CLUSTER-IP      EXTERNAL-IP   PORT(S)    AGE
# service/rbs-frontend-prod-svc              ClusterIP   10.43.x.x       <none>        80/TCP     2m
# service/rbs-backend-monolith-prod-svc      ClusterIP   10.43.x.x       <none>        8000/TCP   2m
# ...
```

### Check Pod Logs

```bash
# Frontend logs
kubectl logs -n robson -l app=rbs-frontend-prod-deploy --tail=50

# Backend logs
kubectl logs -n robson -l app=rbs-backend-monolith-prod-deploy --tail=50

# Nginx logs
kubectl logs -n robson -l app=rbs-backend-nginx-prod-deploy --tail=50
```

### Check Pod Events

```bash
# If pods are not starting, check events
kubectl describe pod -n robson <pod-name>
```

---

## Step 6: Access the Application

### Via Port Forward (Testing)

```bash
# Frontend
kubectl port-forward -n robson svc/rbs-frontend-prod-svc 8080:80
# Access: http://localhost:8080

# Backend API
kubectl port-forward -n robson svc/rbs-backend-monolith-prod-svc 8000:8000
# Access: http://localhost:8000/api/
```

### Via Ingress (Production)

Check the Ingress/Gateway resources:

```bash
kubectl get ingress -n robson
# or
kubectl get gateway -n robson
kubectl get httproute -n robson
```

---

## Troubleshooting

### Application Not Syncing

```bash
# Check sync status
argocd app get robson-prod

# Force sync
argocd app sync robson-prod

# Check sync errors
argocd app sync robson-prod --dry-run
```

### Pods Not Starting

```bash
# Check pod status
kubectl get pods -n robson

# Describe pod for events
kubectl describe pod -n robson <pod-name>

# Check logs
kubectl logs -n robson <pod-name>

# Common issues:
# 1. ImagePullBackOff: Check image tag exists in Docker Hub
# 2. CrashLoopBackOff: Check application logs and secret values
# 3. Pending: Check node resources (kubectl describe nodes)
```

### Secret Not Found

```bash
# Verify secret exists
kubectl get secret -n robson rbs-django-secret

# Check secret keys
kubectl get secret -n robson rbs-django-secret -o jsonpath='{.data}' | jq

# Recreate if needed (delete first)
kubectl delete secret -n robson rbs-django-secret
# Then run Step 2 again
```

### ArgoCD Out of Sync

```bash
# Check what's different
argocd app diff robson-prod

# Sync with replace (force)
argocd app sync robson-prod --replace

# Refresh application (re-read Git)
argocd app refresh robson-prod
```

---

## Updating the Application

### Promoting a New Image

1. **Push to main** → CI builds new image with SHA tag
2. **Update manifests** with new SHA tag
3. **Commit and push** → ArgoCD auto-syncs

```bash
# Example workflow
# 1. CI runs and produces sha-a1b2c3d

# 2. Update manifest
sed -i 's/sha-OLD_SHA/sha-a1b2c3d/g' infra/k8s/prod/rbs-backend-monolith-prod-deploy.yml

# 3. Commit
git add infra/k8s/prod/rbs-backend-monolith-prod-deploy.yml
git commit -m "deploy: promote backend to sha-a1b2c3d"
git push

# 4. Watch ArgoCD sync
argocd app watch robson-prod
```

### Rolling Back

```bash
# Find previous working SHA from git history
git log --oneline infra/k8s/prod/rbs-backend-monolith-prod-deploy.yml

# Revert to previous commit or manually update SHA
git revert <commit-hash>
# or
# Edit file, commit, push

# ArgoCD will auto-sync to the previous version
```

---

## Monitoring

### Watch Sync Status

```bash
# Continuously watch application
argocd app watch robson-prod

# Or use kubectl
watch kubectl get pods -n robson
```

### Check Application Health

```bash
# Get detailed health status
argocd app get robson-prod --show-operation

# Check in ArgoCD UI
# https://argocd.example.com/applications/robson-prod
```

---

## Next Steps

- [ ] Configure Istio Gateway for external access
- [ ] Set up Prometheus monitoring
- [ ] Configure alerts for pod failures
- [ ] Set up log aggregation (Loki)
- [ ] Implement Sealed Secrets or External Secrets
- [ ] Add HPA (Horizontal Pod Autoscaler) if needed

---

## References

- [ArgoCD Application Spec](https://argo-cd.readthedocs.io/en/stable/operator-manual/declarative-setup/#applications)
- [CI/CD Image Tagging Guide](./ci-cd-image-tagging.md)
- [Production Manifests](../../infra/k8s/prod/)
