# CRITICAL ISSUES: Staging Backend Deployment

**Status**: 🔴 **MULTIPLE CRITICAL CONFIGURATION ERRORS**
**Date**: 2024-12-25
**Severity**: HIGH - Prevents backend from running

---

## Issue #1: Missing imagePullSecrets

**File**: `infra/k8s/staging/backend/backend-staging.yaml`
**Lines**: 58 (spec.template.spec section)

**Problem**: Deployment doesn't include `imagePullSecrets` to pull from GHCR private registry.

**Current state**:
```yaml
spec:
  template:
    spec:
      initContainers:
      - name: wait-for-postgres
```

**Required fix**:
```yaml
spec:
  template:
    spec:
      imagePullSecrets:
      - name: ghcr-secret  # ← MISSING
      initContainers:
      - name: wait-for-postgres
```

**Impact**: Pods cannot pull Docker image → ImagePullBackOff

---

## Issue #2: securityContext Blocks Log Directory Creation

**File**: `infra/k8s/staging/backend/backend-staging.yaml`
**Lines**: 197-200

**Problem**: Container runs as non-root user 1000, but `/app/logs` directory needs creation.

**Current state**:
```yaml
securityContext:
  runAsNonRoot: true
  runAsUser: 1000
  fsGroup: 1000
```

**Impact**: `PermissionError: [Errno 13] Permission denied: '/app/logs'`

**Fix options**:

### Option A: Remove securityContext (Quick fix)
```yaml
# securityContext:  # ← COMMENTED OUT
#   runAsNonRoot: true
#   runAsUser: 1000
#   fsGroup: 1000
```

### Option B: Add volume for logs (Proper fix)
```yaml
spec:
  template:
    spec:
      securityContext:
        runAsNonRoot: true
        runAsUser: 1000
        fsGroup: 1000
      containers:
      - name: backend
        volumeMounts:
        - name: logs
          mountPath: /app/logs
      volumes:
      - name: logs
        emptyDir: {}
```

**Recommendation**: Use Option A for now (remove securityContext), implement Option B later.

---

## Issue #3: Health Endpoint May Not Exist

**File**: `infra/k8s/staging/backend/backend-staging.yaml`
**Lines**: 160, 173, 186 (livenessProbe, readinessProbe, startupProbe)

**Problem**: Probes use `/health/` endpoint which may not be implemented in Django.

**Current state**:
```yaml
livenessProbe:
  httpGet:
    path: /health/  # ← Does this endpoint exist?
    port: 8000
```

**Verification needed**:
```bash
# Check if endpoint exists in Django URLs
grep -r "health" apps/backend/monolith/*/urls.py
```

**Fix if endpoint missing**:

### Option A: Use TCP probe instead
```yaml
livenessProbe:
  tcpSocket:
    port: 8000
  initialDelaySeconds: 60
  periodSeconds: 30
  timeoutSeconds: 10
  failureThreshold: 3
```

### Option B: Create health endpoint
```python
# apps/backend/monolith/api/views/health.py
from django.http import JsonResponse
from django.views.decorators.http import require_GET

@require_GET
def health(request):
    return JsonResponse({"status": "ok"})
```

---

## Issue #4: Missing Environment Variables

**File**: `infra/k8s/staging/backend/backend-staging.yaml`
**Lines**: 84-147

**Problem**: Deployment expects `DATABASE_URL`, `REDIS_URL`, `RABBITMQ_URL` from secret, but `deploy-staging.sh` creates individual variables (`RBS_PG_*`, etc.).

**Expected secret keys** (from deployment yaml):
- DATABASE_URL
- REDIS_URL
- RABBITMQ_URL
- SECRET_KEY
- BINANCE_API_KEY
- BINANCE_API_SECRET

**Actual secret created** (from deploy-staging.sh):
```bash
kubectl create secret generic django-staging \
  --from-literal=SECRET_KEY="..." \
  --from-literal=DATABASE_URL="postgresql://..." \  # ✓ Exists
  --from-literal=REDIS_URL="redis://..." \          # ✓ Exists
  --from-literal=RABBITMQ_URL="amqp://..." \       # ✓ Exists
  --from-literal=BINANCE_API_KEY="testnet-placeholder" \  # ✓ Exists
  --from-literal=BINANCE_API_SECRET="testnet-placeholder"  # ✓ Exists
```

**Status**: ✅ This should be OK if `deploy-staging.sh` was run correctly.

**BUT**: Production image may also expect `RBS_*` prefixed variables. Need to add:

```yaml
env:
# ... existing DATABASE_URL, REDIS_URL, RABBITMQ_URL ...

# Add RBS-prefixed versions for production image compatibility
- name: RBS_SECRET_KEY
  valueFrom:
    secretKeyRef:
      name: django-staging
      key: SECRET_KEY

- name: RBS_PG_DATABASE
  value: "robson_staging"

- name: RBS_PG_USER
  valueFrom:
    secretKeyRef:
      name: postgres-staging
      key: POSTGRES_USER

- name: RBS_PG_PASSWORD
  valueFrom:
    secretKeyRef:
      name: postgres-staging
      key: POSTGRES_PASSWORD

- name: RBS_PG_HOST
  value: "postgres-staging"

- name: RBS_PG_PORT
  value: "5432"
```

---

## Issue #5: No Ingress Configuration

**Problem**: No Traefik Ingress or Istio VirtualService configured for staging.

**Impact**: External traffic cannot reach backend → 500 errors on https://api.staging.rbx.ia.br/

**Required**: Create Traefik Ingress

**File to create**: `infra/k8s/staging/ingress/traefik-staging.yaml`

```yaml
---
# Ingress for API (Traefik)
apiVersion: networking.k8s.io/v1
kind: Ingress
metadata:
  name: api-staging
  namespace: staging
  annotations:
    cert-manager.io/cluster-issuer: letsencrypt-prod
    traefik.ingress.kubernetes.io/router.entrypoints: websecure
    traefik.ingress.kubernetes.io/router.tls: "true"
spec:
  ingressClassName: traefik
  tls:
  - hosts:
    - api.staging.rbx.ia.br
    secretName: api-staging-tls
  rules:
  - host: api.staging.rbx.ia.br
    http:
      paths:
      - path: /
        pathType: Prefix
        backend:
          service:
            name: backend-staging
            port:
              number: 8000

---
# Ingress for Frontend (Traefik)
apiVersion: networking.k8s.io/v1
kind: Ingress
metadata:
  name: frontend-staging
  namespace: staging
  annotations:
    cert-manager.io/cluster-issuer: letsencrypt-prod
    traefik.ingress.kubernetes.io/router.entrypoints: websecure
    traefik.ingress.kubernetes.io/router.tls: "true"
spec:
  ingressClassName: traefik
  tls:
  - hosts:
    - staging.rbx.ia.br
    secretName: staging-tls
  rules:
  - host: staging.rbx.ia.br
    http:
      paths:
      - path: /
        pathType: Prefix
        backend:
          service:
            name: frontend-staging
            port:
              number: 3000
```

**Alternative**: If frontend not deployed yet, remove frontend ingress and just keep API ingress.

---

## Issue #6: Istio Gateway Not Used

**File**: `infra/k8s/staging/istio/gateway-staging.yaml`
**Status**: Created but not applied

**Problem**: Documentation mentions Istio Gateway but cluster uses Traefik.

**Resolution**:
- Keep Istio files for future reference
- Use Traefik Ingress (see Issue #5)
- Update kustomization.yaml to remove Istio resources

---

## Consolidated Fix Script

Save as `fix-staging-backend.sh`:

```bash
#!/bin/bash
# Fix all critical staging backend issues
# Run: bash fix-staging-backend.sh

set -e

SSH_HOST="root@158.220.116.31"
NAMESPACE="staging"

echo "🔧 Fixing Staging Backend Configuration..."
echo ""

# Fix #1: Add imagePullSecrets to deployment
echo "1. Adding imagePullSecrets to deployment..."
ssh $SSH_HOST "kubectl patch deployment backend-staging -n $NAMESPACE -p '{\"spec\":{\"template\":{\"spec\":{\"imagePullSecrets\":[{\"name\":\"ghcr-secret\"}]}}}}'"

# Fix #2: Remove securityContext (quick fix)
echo "2. Removing securityContext to allow log directory creation..."
ssh $SSH_HOST "kubectl patch deployment backend-staging -n $NAMESPACE --type=json -p='[{\"op\":\"remove\",\"path\":\"/spec/template/spec/securityContext\"}]'"

# Fix #3: Change health probes to TCP (in case /health/ doesn't exist)
echo "3. Changing health probes to TCP..."
ssh $SSH_HOST "kubectl patch deployment backend-staging -n $NAMESPACE --type=json -p='[
  {\"op\":\"replace\",\"path\":\"/spec/template/spec/containers/0/livenessProbe\",\"value\":{\"tcpSocket\":{\"port\":8000},\"initialDelaySeconds\":60,\"periodSeconds\":30,\"timeoutSeconds\":10,\"failureThreshold\":3}},
  {\"op\":\"replace\",\"path\":\"/spec/template/spec/containers/0/readinessProbe\",\"value\":{\"tcpSocket\":{\"port\":8000},\"initialDelaySeconds\":30,\"periodSeconds\":10,\"timeoutSeconds\":5,\"failureThreshold\":3}},
  {\"op\":\"replace\",\"path\":\"/spec/template/spec/containers/0/startupProbe\",\"value\":{\"tcpSocket\":{\"port\":8000},\"initialDelaySeconds\":10,\"periodSeconds\":10,\"timeoutSeconds\":5,\"failureThreshold\":12}}
]'"

# Fix #4: Add RBS_* environment variables
echo "4. Adding RBS-prefixed environment variables..."
ssh $SSH_HOST "kubectl set env deployment/backend-staging -n $NAMESPACE \
  RBS_SECRET_KEY=django-insecure-staging-$(openssl rand -base64 32) \
  RBS_PG_DATABASE=robson_staging \
  RBS_PG_HOST=postgres-staging \
  RBS_PG_PORT=5432"

# Get PostgreSQL credentials from secret
echo "5. Getting PostgreSQL credentials..."
PG_USER=$(ssh $SSH_HOST "kubectl get secret postgres-staging -n $NAMESPACE -o jsonpath='{.data.POSTGRES_USER}' | base64 -d")
PG_PASS=$(ssh $SSH_HOST "kubectl get secret postgres-staging -n $NAMESPACE -o jsonpath='{.data.POSTGRES_PASSWORD}' | base64 -d")

ssh $SSH_HOST "kubectl set env deployment/backend-staging -n $NAMESPACE \
  RBS_PG_USER=$PG_USER \
  RBS_PG_PASSWORD=$PG_PASS"

# Fix #5: Create Traefik Ingress
echo "6. Creating Traefik Ingress..."
cat <<EOF | ssh $SSH_HOST "kubectl apply -f -"
apiVersion: networking.k8s.io/v1
kind: Ingress
metadata:
  name: api-staging
  namespace: $NAMESPACE
  annotations:
    cert-manager.io/cluster-issuer: letsencrypt-prod
    traefik.ingress.kubernetes.io/router.entrypoints: websecure
    traefik.ingress.kubernetes.io/router.tls: "true"
spec:
  ingressClassName: traefik
  tls:
  - hosts:
    - api.staging.rbx.ia.br
    secretName: api-staging-tls
  rules:
  - host: api.staging.rbx.ia.br
    http:
      paths:
      - path: /
        pathType: Prefix
        backend:
          service:
            name: backend-staging
            port:
              number: 8000
EOF

# Wait for rollout
echo ""
echo "7. Waiting for deployment rollout..."
ssh $SSH_HOST "kubectl rollout status deployment/backend-staging -n $NAMESPACE --timeout=5m"

# Verify pods
echo ""
echo "8. Verifying pod status..."
ssh $SSH_HOST "kubectl get pods -n $NAMESPACE -l app=backend-staging"

echo ""
echo "✅ Fixes applied successfully!"
echo ""
echo "Next steps:"
echo "1. Check pod logs: kubectl logs -n $NAMESPACE -l app=backend-staging"
echo "2. Verify API: curl -k https://api.staging.rbx.ia.br/health/"
echo "3. Run migrations: kubectl exec -it -n $NAMESPACE deployment/backend-staging -- python manage.py migrate"
```

---

## Verification After Fixes

```bash
# 1. Pods should be Running
ssh root@158.220.116.31 "kubectl get pods -n staging"

# 2. Check deployment has imagePullSecrets
ssh root@158.220.116.31 "kubectl get deployment backend-staging -n staging -o jsonpath='{.spec.template.spec.imagePullSecrets}'"
# Expected: [{"name":"ghcr-secret"}]

# 3. Check deployment has no securityContext
ssh root@158.220.116.31 "kubectl get deployment backend-staging -n staging -o jsonpath='{.spec.template.spec.securityContext}'"
# Expected: (empty)

# 4. Check ingress exists
ssh root@158.220.116.31 "kubectl get ingress -n staging"

# 5. Test API endpoint
curl -k https://api.staging.rbx.ia.br/health/
```

---

## Summary

**Critical issues preventing backend from running**:

1. ❌ Missing `imagePullSecrets` → Cannot pull Docker image
2. ❌ `securityContext` blocks log directory → Permission denied
3. ⚠️ Health endpoint may not exist → Probes fail → Pod restart
4. ⚠️ Missing `RBS_*` env vars → Django may fail to start
5. ❌ No Ingress → External traffic cannot reach backend
6. ℹ️ Istio Gateway created but not used (cluster uses Traefik)

**Priority order**:
1. Add imagePullSecrets (CRITICAL)
2. Remove securityContext (CRITICAL)
3. Change probes to TCP (HIGH)
4. Add RBS_* env vars (HIGH)
5. Create Traefik Ingress (HIGH)
6. Verify health endpoint exists (MEDIUM)

---

**Next Action**: Run `fix-staging-backend.sh` to apply all fixes automatically.

**Last Updated**: 2024-12-25
