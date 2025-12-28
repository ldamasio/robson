# Troubleshooting Guide: Staging Backend CrashLoopBackOff

**Last Updated**: 2024-12-25
**Issue**: Backend pods in staging namespace are crashing repeatedly
**Priority**: 🔴 CRITICAL - Blocks all API functionality

---

## Quick Diagnosis

Run this single command to get the most important information:

```bash
ssh root@158.220.116.31 "kubectl logs -n staging -l app=backend-staging --tail=100 --previous"
```

This shows the logs from the **previous crashed container**, which contains the error that caused the crash.

---

## Step-by-Step Troubleshooting

### Step 1: Check Current Pod Status

```bash
ssh root@158.220.116.31 "kubectl get pods -n staging"
```

**Expected output** (broken state):
```
NAME                                READY   STATUS             RESTARTS
backend-staging-XXXXXXXXX-XXXXX     0/1     CrashLoopBackOff   5 (2m ago)
postgres-staging-XXXXXXXX-XXXXX     1/1     Running            0
redis-staging-XXXXXXXXXXX-XXXXX     1/1     Running            0
rabbitmq-staging-XXXXXXXX-XXXXX     1/1     Running            0
```

**Good state** (after fix):
```
NAME                                READY   STATUS    RESTARTS
backend-staging-XXXXXXXXX-XXXXX     1/1     Running   0
postgres-staging-XXXXXXXX-XXXXX     1/1     Running   0
redis-staging-XXXXXXXXXXX-XXXXX     1/1     Running   0
rabbitmq-staging-XXXXXXXX-XXXXX     1/1     Running   0
```

---

### Step 2: Get Crash Logs

```bash
# Get logs from previous crash (MOST IMPORTANT)
ssh root@158.220.116.31 "kubectl logs -n staging -l app=backend-staging --tail=100 --previous"

# Get current logs (if pod is running briefly before crashing)
ssh root@158.220.116.31 "kubectl logs -n staging -l app=backend-staging --tail=100"

# Get describe output (shows restart count, last state)
ssh root@158.220.116.31 "kubectl describe pod -n staging -l app=backend-staging"
```

---

### Step 3: Analyze Common Crash Patterns

#### Pattern 1: Database Connection Error

**Log pattern**:
```
django.db.utils.OperationalError: could not connect to server: Connection refused
    Is the server running on host "postgres-staging" (10.x.x.x) and accepting
    TCP/IP connections on port 5432?
```

**Diagnosis**: PostgreSQL not ready or wrong credentials

**Fix**:
```bash
# 1. Check if PostgreSQL is actually running
ssh root@158.220.116.31 "kubectl get pods -n staging -l app=postgres-staging"

# 2. Check if secret values are correct
ssh root@158.220.116.31 "kubectl get secret postgres-staging -n staging -o yaml"

# 3. Verify backend can resolve postgres-staging DNS
ssh root@158.220.116.31 "kubectl exec -n staging deployment/backend-staging -- nslookup postgres-staging"

# 4. Add init container to wait for PostgreSQL (see Fix 1.1 below)
```

#### Pattern 2: Missing Environment Variables

**Log pattern**:
```
jinja2.exceptions.UndefinedError: 'RBS_SECRET_KEY' is undefined
```

**Diagnosis**: Environment variable not set in deployment

**Fix**:
```bash
# Check which env vars are actually set
ssh root@158.220.116.31 "kubectl exec -n staging deployment/backend-staging -- env | grep RBS"

# If missing, patch deployment (see Fix 2.1 below)
```

#### Pattern 3: Migration Failure

**Log pattern**:
```
django.db.utils.ProgrammingError: relation "api_stopmonitorevent" does not exist
```

**Diagnosis**: Migrations not applied yet

**Fix**:
```bash
# Run migrations manually
ssh root@158.220.116.31 "kubectl exec -it -n staging deployment/backend-staging -- python manage.py migrate"

# Or add migration job (see Fix 3.1 below)
```

#### Pattern 4: Python Import Error

**Log pattern**:
```
ModuleNotFoundError: No module named 'binance'
```

**Diagnosis**: Dependency missing from Docker image

**Fix**:
```bash
# Rebuild image with dependencies
cd apps/backend/monolith
docker build -f docker/Dockerfile_django -t ghcr.io/ldamasio/rbs-backend-monolith:staging-latest .
docker push ghcr.io/ldamasio/rbs-backend-monolith:staging-latest

# Force pod restart
ssh root@158.220.116.31 "kubectl rollout restart deployment/backend-staging -n staging"
```

#### Pattern 5: Permission Error

**Log pattern**:
```
PermissionError: [Errno 13] Permission denied: '/app/logs'
```

**Diagnosis**: Container running as non-root without proper volume permissions

**Fix**: Already applied - `securityContext` removed from deployment

#### Pattern 6: Gunicorn Command Error

**Log pattern**:
```
gunicorn: error: unrecognized arguments: --worker-connections=1000
```

**Diagnosis**: Gunicorn command in Dockerfile has wrong syntax

**Fix**:
```dockerfile
# Correct command (in docker/Dockerfile_django)
CMD ["gunicorn", "-b", "0.0.0.0:8000", "--worker-class=gevent", "--worker-connections=1000", "--workers=5", "backend.wsgi:application"]
```

#### Pattern 7: Health Check Timeout

**Log pattern**: No error in logs, but pod shows `Unhealthy` in describe

```
Liveness probe failed: Get "http://10.x.x.x:8000/health": dial tcp 10.x.x.x:8000: connect: connection refused
```

**Diagnosis**: Health check endpoint doesn't exist or startup too slow

**Fix**: Add startup probe with longer timeout (see Fix 7.1 below)

---

## Detailed Fixes

### Fix 1.1: Add Init Container to Wait for PostgreSQL

Edit `infra/k8s/staging/backend/backend-staging.yml`:

```yaml
spec:
  template:
    spec:
      initContainers:
      - name: wait-for-postgres
        image: busybox:1.36
        command: ['sh', '-c', 'until nc -z postgres-staging 5432; do echo waiting for postgres; sleep 2; done']
      containers:
      - name: backend
        # ... rest of container spec
```

Apply:
```bash
ssh root@158.220.116.31 "kubectl apply -f /path/to/backend-staging.yaml"
```

### Fix 2.1: Add Missing Environment Variable

```bash
# Example: Add BINANCE_TESTNET variable
ssh root@158.220.116.31 "kubectl set env deployment/backend-staging -n staging BINANCE_TESTNET=true"
```

Or edit secret and restart:
```bash
# Edit secret
ssh root@158.220.116.31 "kubectl edit secret django-staging -n staging"

# Restart deployment to pick up new values
ssh root@158.220.116.31 "kubectl rollout restart deployment/backend-staging -n staging"
```

### Fix 3.1: Create Migration Job

Create `infra/k8s/staging/backend/job-migrate.yml`:

```yaml
apiVersion: batch/v1
kind: Job
metadata:
  name: migrate-staging
  namespace: staging
spec:
  template:
    spec:
      restartPolicy: OnFailure
      imagePullSecrets:
      - name: ghcr-secret
      containers:
      - name: migrate
        image: ghcr.io/ldamasio/rbs-backend-monolith:staging-latest
        command: ["python", "manage.py", "migrate", "--noinput"]
        envFrom:
        - secretRef:
            name: django-staging
        env:
        - name: RBS_PG_DATABASE
          value: robson_staging
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
          value: postgres-staging
        - name: RBS_PG_PORT
          value: "5432"
  backoffLimit: 3
```

Apply:
```bash
ssh root@158.220.116.31 "kubectl apply -f infra/k8s/staging/backend/job-migrate.yml"
ssh root@158.220.116.31 "kubectl wait --for=condition=complete job/migrate-staging -n staging --timeout=300s"
```

### Fix 7.1: Add Startup Probe

Edit `infra/k8s/staging/backend/backend-staging.yml`:

```yaml
spec:
  template:
    spec:
      containers:
      - name: backend
        # ... other config
        startupProbe:
          httpGet:
            path: /health
            port: 8000
          failureThreshold: 30  # 30 * 10 = 300 seconds = 5 minutes
          periodSeconds: 10
        livenessProbe:
          httpGet:
            path: /health
            port: 8000
          initialDelaySeconds: 30
          periodSeconds: 10
          timeoutSeconds: 5
          failureThreshold: 3
        readinessProbe:
          httpGet:
            path: /health
            port: 8000
          initialDelaySeconds: 10
          periodSeconds: 5
          timeoutSeconds: 3
```

**NOTE**: Requires `/health` endpoint in Django. If not exists, use `tcpSocket` instead:

```yaml
        startupProbe:
          tcpSocket:
            port: 8000
          failureThreshold: 30
          periodSeconds: 10
```

---

## Comprehensive Diagnostic Script

Save as `diagnose-staging.sh` and run locally:

```bash
#!/bin/bash
# Comprehensive diagnostic for staging backend issues

set -e

SSH_HOST="root@158.220.116.31"
NAMESPACE="staging"
APP_LABEL="app=backend-staging"

echo "=========================================="
echo "STAGING BACKEND DIAGNOSTIC"
echo "=========================================="
echo ""

echo "1. Pod Status"
echo "----------------------------------------"
ssh $SSH_HOST "kubectl get pods -n $NAMESPACE"
echo ""

echo "2. Pod Describe (Events)"
echo "----------------------------------------"
ssh $SSH_HOST "kubectl describe pod -n $NAMESPACE -l $APP_LABEL | grep -A 20 Events:"
echo ""

echo "3. Previous Crash Logs (Last 50 lines)"
echo "----------------------------------------"
ssh $SSH_HOST "kubectl logs -n $NAMESPACE -l $APP_LABEL --tail=50 --previous" || echo "No previous logs (pod hasn't crashed yet)"
echo ""

echo "4. Current Logs (Last 50 lines)"
echo "----------------------------------------"
ssh $SSH_HOST "kubectl logs -n $NAMESPACE -l $APP_LABEL --tail=50" || echo "Pod not running"
echo ""

echo "5. Environment Variables (RBS_*)"
echo "----------------------------------------"
ssh $SSH_HOST "kubectl exec -n $NAMESPACE deployment/backend-staging -- env | grep RBS || true"
echo ""

echo "6. PostgreSQL Connectivity"
echo "----------------------------------------"
ssh $SSH_HOST "kubectl exec -n $NAMESPACE deployment/postgres-staging -- pg_isready -U robson_staging -d robson_staging"
echo ""

echo "7. PostgreSQL Tables (api_*)"
echo "----------------------------------------"
ssh $SSH_HOST "kubectl exec -n $NAMESPACE deployment/postgres-staging -- psql -U robson_staging -d robson_staging -c '\dt api_*'"
echo ""

echo "8. Django Migrations Status"
echo "----------------------------------------"
ssh $SSH_HOST "kubectl exec -n $NAMESPACE deployment/backend-staging -- python manage.py showmigrations api || echo 'Cannot connect to pod'"
echo ""

echo "9. Service Endpoints"
echo "----------------------------------------"
ssh $SSH_HOST "kubectl get endpoints backend-staging -n $NAMESPACE"
echo ""

echo "10. Resource Usage"
echo "----------------------------------------"
ssh $SSH_HOST "kubectl top pods -n $NAMESPACE"
echo ""

echo "=========================================="
echo "DIAGNOSTIC COMPLETE"
echo "=========================================="
```

Run:
```bash
bash diagnose-staging.sh > staging-diagnostic.txt 2>&1
cat staging-diagnostic.txt
```

---

## Quick Fixes Checklist

Use this checklist to systematically fix the most common issues:

```bash
# ✅ 1. Verify PostgreSQL is running
ssh root@158.220.116.31 "kubectl get pods -n staging -l app=postgres-staging"

# ✅ 2. Verify secrets exist and have values
ssh root@158.220.116.31 "kubectl get secret django-staging -n staging -o jsonpath='{.data}' | jq"

# ✅ 3. Check if backend pods have crashed recently
ssh root@158.220.116.31 "kubectl get pods -n staging -l app=backend-staging"

# ✅ 4. Get crash logs
ssh root@158.220.116.31 "kubectl logs -n staging -l app=backend-staging --previous --tail=100"

# ✅ 5. Check deployment configuration
ssh root@158.220.116.31 "kubectl get deployment backend-staging -n staging -o yaml | grep -A 50 'env:'"

# ✅ 6. Verify image can be pulled
ssh root@158.220.116.31 "kubectl get events -n staging | grep -i 'pull'"

# ✅ 7. Check resource quota
ssh root@158.220.116.31 "kubectl describe quota staging-quota -n staging"

# ✅ 8. Force restart (after fixing config)
ssh root@158.220.116.31 "kubectl rollout restart deployment/backend-staging -n staging"

# ✅ 9. Watch rollout status
ssh root@158.220.116.31 "kubectl rollout status deployment/backend-staging -n staging --timeout=5m"

# ✅ 10. Verify pods are healthy
ssh root@158.220.116.31 "kubectl get pods -n staging -l app=backend-staging"
```

---

## Emergency Recovery

If backend is completely broken and you need to start fresh:

### Option 1: Delete and Recreate Deployment

```bash
# 1. Delete deployment
ssh root@158.220.116.31 "kubectl delete deployment backend-staging -n staging"

# 2. Delete secrets (will regenerate)
ssh root@158.220.116.31 "kubectl delete secret django-staging -n staging"

# 3. Re-run deployment script
bash deploy-staging.sh
```

### Option 2: Scale Down and Debug

```bash
# 1. Scale to 0 (stops crash loop)
ssh root@158.220.116.31 "kubectl scale deployment backend-staging -n staging --replicas=0"

# 2. Fix configuration
# (edit manifests, update secrets, etc.)

# 3. Scale back up
ssh root@158.220.116.31 "kubectl scale deployment backend-staging -n staging --replicas=2"
```

### Option 3: Manual Migration Job

```bash
# 1. Scale backend to 0
ssh root@158.220.116.31 "kubectl scale deployment backend-staging -n staging --replicas=0"

# 2. Run migration job
ssh root@158.220.116.31 "kubectl apply -f infra/k8s/staging/backend/job-migrate.yml"

# 3. Wait for completion
ssh root@158.220.116.31 "kubectl wait --for=condition=complete job/migrate-staging -n staging --timeout=5m"

# 4. Scale backend back up
ssh root@158.220.116.31 "kubectl scale deployment backend-staging -n staging --replicas=2"
```

---

## Post-Fix Verification

After applying a fix, verify with these commands:

```bash
# 1. Pods should be Running
ssh root@158.220.116.31 "kubectl get pods -n staging"
# Expected: backend-staging-XXXXX   1/1   Running

# 2. No recent restarts
ssh root@158.220.116.31 "kubectl get pods -n staging -o jsonpath='{range .items[*]}{.metadata.name}{\" \"}{.status.containerStatuses[0].restartCount}{\"\\n\"}{end}'"
# Expected: backend-staging-XXXXX 0

# 3. Logs show successful startup
ssh root@158.220.116.31 "kubectl logs -n staging -l app=backend-staging --tail=20"
# Expected: "Listening at: http://0.0.0.0:8000"

# 4. Health endpoint responds (if exists)
ssh root@158.220.116.31 "kubectl exec -n staging deployment/backend-staging -- wget -qO- http://localhost:8000/health"
# Expected: {"status": "ok"} or similar

# 5. API endpoint responds via ingress
curl -k https://api.staging.rbx.ia.br/health
# Expected: 200 OK (even without TLS cert)

# 6. Check migration status
ssh root@158.220.116.31 "kubectl exec -n staging deployment/backend-staging -- python manage.py showmigrations api"
# Expected: All migrations marked [X]
```

---

## Known Issues and Workarounds

### Issue: ImagePullBackOff

**Symptom**: `ErrImagePull` or `ImagePullBackOff` in pod status

**Cause**: `ghcr-secret` not configured or expired GITHUB_TOKEN

**Fix**:
```bash
# Recreate secret with fresh token
source ~/.bashrc  # Load GITHUB_TOKEN
kubectl create secret docker-registry ghcr-secret \
  --docker-server=ghcr.io \
  --docker-username=ldamasio \
  --docker-password=$GITHUB_TOKEN \
  -n staging --dry-run=client -o yaml | kubectl apply -f -

# Restart deployment
ssh root@158.220.116.31 "kubectl rollout restart deployment/backend-staging -n staging"
```

### Issue: Exceeded Quota

**Symptom**: `forbidden: exceeded quota: staging-quota`

**Cause**: Total resource limits exceed namespace quota

**Temporary Fix**:
```bash
# Suspend stop-monitor CronJob
ssh root@158.220.116.31 "kubectl patch cronjob stop-monitor-staging -n staging -p '{\"spec\":{\"suspend\":true}}'"
```

**Permanent Fix**: Increase quota or reduce pod limits

### Issue: TLS Certificate Not Ready

**Symptom**: Browser shows "Not Secure" or "Invalid Certificate"

**Cause**: cert-manager still processing Let's Encrypt challenge

**Check status**:
```bash
ssh root@158.220.116.31 "kubectl get certificate -n staging"
ssh root@158.220.116.31 "kubectl describe certificate staging-rbx-ia-br-tls -n staging"
```

**Expected**: Will be ready within 5-10 minutes. If longer, check cert-manager logs.

---

## Contact Information

**Primary**: Leandro Damásio
**Repository**: https://github.com/ldamasio/robson
**Cluster**: root@158.220.116.31 (k3s)
**Namespace**: `staging`

---

**Last Updated**: 2024-12-25
**Status**: Document created during active troubleshooting session
**Next Action**: Run diagnostic script and analyze crash logs
