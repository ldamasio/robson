# ArgoCD Production Deployment Checklist

Use this checklist to ensure a smooth initial deployment of Robson to production.

---

## Phase 1: Pre-Deployment (Local)

### Git Repository
- [ ] All CI/CD changes committed and pushed
- [ ] Workflow file uses SHA tags
- [ ] Manifests updated with actual SHA tags (not `sha-CHANGEME`)
- [ ] All changes reviewed and tested locally

### Validation Commands
```bash
# Check workflow syntax
python -c "import yaml; yaml.safe_load(open('.github/workflows/main.yml'))"

# Validate K8s manifests
kubectl apply --dry-run=client -f infra/k8s/prod/

# Check ArgoCD Application manifest
kubectl apply --dry-run=client -f infra/k8s/gitops/applications/robson-prod.yaml
```

---

## Phase 2: Cluster Preparation

### Access
- [ ] SSH access to tiger node configured
- [ ] kubectl configured with cluster context
- [ ] ArgoCD CLI installed locally (optional but recommended)

### ArgoCD Installation
- [ ] ArgoCD installed in cluster
- [ ] ArgoCD UI accessible (port-forward or ingress)
- [ ] ArgoCD admin credentials obtained

```bash
# Check ArgoCD
kubectl get pods -n argocd
kubectl -n argocd get secret argocd-initial-admin-secret -o jsonpath="{.data.password}" | base64 -d
```

### Namespace Setup
- [ ] Namespace `robson` created
- [ ] Secret `rbs-django-secret` created with all keys

```bash
# Create namespace
kubectl create namespace robson

# Verify
kubectl get namespace robson
```

---

## Phase 3: Secrets Configuration

### Required Secret Keys
- [ ] `RBS_SECRET_KEY`
- [ ] `RBS_BINANCE_API_KEY_TEST`
- [ ] `RBS_BINANCE_SECRET_KEY_TEST`
- [ ] `RBS_BINANCE_API_KEY_PROD`
- [ ] `RBS_BINANCE_SECRET_KEY_PROD`
- [ ] `RBS_BINANCE_API_URL_TEST`
- [ ] `POSTGRES_DATABASE`
- [ ] `POSTGRES_USER`
- [ ] `POSTGRES_PASSWORD`
- [ ] `POSTGRES_HOST`
- [ ] `POSTGRES_PORT`

### Validation
```bash
# Check secret exists
kubectl get secret -n robson rbs-django-secret

# Verify keys (names only, not values)
kubectl get secret -n robson rbs-django-secret -o jsonpath='{.data}' | jq 'keys'
```

---

## Phase 4: Image Tags

### Update Manifests
- [ ] Frontend manifest has real SHA tag
- [ ] Backend monolith manifest has real SHA tag
- [ ] Backend nginx manifest has real SHA tag

### Get Latest SHA
```bash
# From git
git rev-parse --short HEAD

# Or from GitHub Actions (after CI run)
# Go to Actions tab → Latest run → Summary → Images Published
```

### Update Files
```bash
# Replace sha-CHANGEME with actual SHA
# Example: sha-a1b2c3d
sed -i 's/sha-CHANGEME/sha-YOUR_SHA_HERE/g' infra/k8s/prod/*.yml

# Verify changes
grep "image:.*ldamasio" infra/k8s/prod/*.yml

# Commit and push
git add infra/k8s/prod/*.yml
git commit -m "deploy: initial production deployment with sha-YOUR_SHA"
git push
```

---

## Phase 5: Deploy ArgoCD Application

### Apply Application
- [ ] Application manifest applied to cluster

```bash
kubectl apply -f infra/k8s/gitops/applications/robson-prod.yaml
```

### Verify Application
- [ ] Application created in ArgoCD
- [ ] Application sync started

```bash
# Check application
argocd app get robson-prod

# Watch sync progress
argocd app watch robson-prod
```

---

## Phase 6: Verify Deployment

### Check Sync Status
- [ ] Application shows "Synced"
- [ ] Application shows "Healthy"

```bash
argocd app get robson-prod
```

### Check Kubernetes Resources
- [ ] All deployments created
- [ ] All services created
- [ ] All pods running

```bash
# Check all resources
kubectl get all -n robson

# Check specific deployments
kubectl get deployment -n robson
kubectl get pods -n robson
kubectl get svc -n robson
```

### Check Pod Health
- [ ] Frontend pod(s) running
- [ ] Backend monolith pod(s) running
- [ ] Backend nginx pod(s) running
- [ ] No CrashLoopBackOff or ImagePullBackOff

```bash
# Check pod status
kubectl get pods -n robson

# Check pod logs (if any issues)
kubectl logs -n robson -l app=rbs-frontend-prod-deploy --tail=50
kubectl logs -n robson -l app=rbs-backend-monolith-prod-deploy --tail=50
kubectl logs -n robson -l app=rbs-backend-nginx-prod-deploy --tail=50
```

---

## Phase 7: Functional Testing

### Port Forward (Testing)
- [ ] Frontend accessible via port-forward
- [ ] Backend API accessible via port-forward

```bash
# Test frontend
kubectl port-forward -n robson svc/rbs-frontend-prod-svc 8080:80 &
curl http://localhost:8080

# Test backend
kubectl port-forward -n robson svc/rbs-backend-monolith-prod-svc 8000:8000 &
curl http://localhost:8000/api/
```

### Application Endpoints
- [ ] Health checks passing (if configured)
- [ ] Database connectivity working
- [ ] API endpoints responding

---

## Phase 8: Ingress/Gateway (If Configured)

### Check Ingress Resources
- [ ] Gateway resources created
- [ ] HTTPRoute resources created
- [ ] DNS pointing to correct IP

```bash
# Check Gateway API resources
kubectl get gateway -n robson
kubectl get httproute -n robson

# Or traditional Ingress
kubectl get ingress -n robson
```

### External Access
- [ ] Frontend accessible via public URL
- [ ] Backend API accessible via public URL
- [ ] TLS certificates valid (if configured)

---

## Phase 9: Monitoring Setup

### ArgoCD Monitoring
- [ ] Application added to ArgoCD dashboard
- [ ] Notifications configured (optional)

### Kubernetes Monitoring
- [ ] Pod metrics available
- [ ] Resource usage acceptable

```bash
# Check resource usage
kubectl top pods -n robson
kubectl top nodes
```

---

## Phase 10: Documentation

### Update Documentation
- [ ] Document deployment date and SHA
- [ ] Update runbooks with actual values
- [ ] Share access credentials with team (securely)

### Create Runbook Entry
```bash
# Add to deployment log
echo "$(date): Initial production deployment - sha-YOUR_SHA" >> docs/deployment-log.md
```

---

## Troubleshooting Reference

If any step fails, refer to:

- **[ArgoCD Initial Setup Guide](argocd-initial-setup.md)** - Detailed troubleshooting
- **[CI/CD Image Tagging](ci-cd-image-tagging.md)** - Image tag issues
- **ArgoCD UI** - Visual status and logs
- **kubectl describe** - Detailed resource information

Common issues:
1. **ImagePullBackOff**: Wrong SHA tag or image doesn't exist in Docker Hub
2. **CrashLoopBackOff**: Check application logs and secret values
3. **Pending pods**: Check node resources and taints
4. **Out of Sync**: Force sync or check for manual changes in cluster

---

## Rollback Plan

If deployment fails, rollback procedure:

1. **Quick rollback**: Update manifests to previous SHA
2. **Complete rollback**: Revert git commit
3. **Emergency**: Delete ArgoCD Application

```bash
# Method 1: Update to previous SHA
git revert HEAD
git push

# Method 2: Delete application (keeps resources)
kubectl delete application robson-prod -n argocd

# Method 3: Delete everything
kubectl delete namespace robson
kubectl delete application robson-prod -n argocd
```

---

## Success Criteria

✅ Deployment is successful when:

1. All pods are in `Running` state
2. ArgoCD shows `Synced` and `Healthy`
3. Application is accessible (port-forward or ingress)
4. No error logs in pods
5. Database connectivity working
6. All services responding to health checks

---

## Next Steps After Successful Deployment

- [ ] Set up automated backups (PostgreSQL, secrets)
- [ ] Configure monitoring alerts
- [ ] Set up log aggregation
- [ ] Document operational procedures
- [ ] Train team on deployment process
- [ ] Plan for future updates and rollbacks

---

**Maintained by**: Robson DevOps Team  
**Last Updated**: 2024-12-20  
**Version**: 1.0
