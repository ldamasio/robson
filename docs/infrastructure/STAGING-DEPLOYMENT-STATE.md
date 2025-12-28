# Staging Deployment State - Session Record

**Last Updated**: 2024-12-25
**Session**: Event Sourcing Stop Monitor - Staging Deployment
**User**: Leandro Damásio
**Status**: 🔴 **BACKEND PODS CRASHING** - Investigation Required

---

## Executive Summary

Staging environment deployment is **80% complete** but backend pods are in **CrashLoopBackOff** state, preventing the API from serving requests.

**What's Working**:
- ✅ Namespace created with ResourceQuota and LimitRange
- ✅ Network Policies (7 policies) - Complete isolation from production
- ✅ PostgreSQL, Redis, RabbitMQ - All running healthy
- ✅ Secrets configured (postgres-staging, redis-staging, rabbitmq-staging, django-staging)
- ✅ Docker image built with Event Sourcing (migrations 0015-0018)
- ✅ ImagePullSecret configured for GHCR
- ✅ Deployment manifest applied
- ✅ DNS records created at registro.br (21 A records)
- ✅ DNS propagation complete

**What's Broken**:
- ❌ Backend pods crashing repeatedly (CrashLoopBackOff)
- ❌ API returning 500 error (consequence of crashed pods)
- ❌ Frontend not deployed (404 expected)
- ⚠️ TLS certificates processing (cert-manager working on it)

---

## Current Infrastructure State

### Namespace: `staging`

**ResourceQuota**:
```yaml
requests.cpu: 4
requests.memory: 8Gi
limits.cpu: 8
limits.memory: 16Gi
persistentvolumeclaims: 10
```

**LimitRange** (per container):
```yaml
default:
  cpu: 500m
  memory: 512Mi
defaultRequest:
  cpu: 100m
  memory: 128Mi
max:
  cpu: 2
  memory: 4Gi
```

### Network Policies (7 total)

1. **default-deny-all-ingress** - Block all inbound traffic by default
2. **default-deny-all-egress** - Block all outbound traffic by default
3. **block-production** - Explicitly block production namespace (`robson`)
4. **allow-dns** - Allow DNS queries (kube-dns:53)
5. **allow-internet** - Allow HTTPS (443) to internet
6. **allow-backend-to-deps** - Backend → PostgreSQL, Redis, RabbitMQ
7. **allow-ingress-to-backend** - Traefik → Backend

### Deployed Services

| Service | Status | Replicas | Image | Port |
|---------|--------|----------|-------|------|
| **postgres-staging** | ✅ Running | 1/1 | paradedb/paradedb:0.13.4 | 5432 |
| **redis-staging** | ✅ Running | 1/1 | redis:7.2-alpine | 6379 |
| **rabbitmq-staging** | ✅ Running | 1/1 | rabbitmq:3.13-management-alpine | 5672, 15672 |
| **backend-staging** | ❌ CrashLoopBackOff | 0/2 | ghcr.io/ldamasio/rbs-backend-monolith:staging-latest | 8000 |

### Secrets Configured

**postgres-staging**:
- POSTGRES_USER=robson_staging
- POSTGRES_PASSWORD=(auto-generated 32-char)
- POSTGRES_DB=robson_staging

**redis-staging**:
- REDIS_PASSWORD=(auto-generated 24-char)

**rabbitmq-staging**:
- RABBITMQ_DEFAULT_USER=robson_staging
- RABBITMQ_DEFAULT_PASS=(auto-generated 32-char)

**django-staging**:
- RBS_SECRET_KEY=(django-insecure-staging-* + auto-generated)
- SECRET_KEY=(same value for compatibility)
- RBS_PG_DATABASE=robson_staging
- RBS_PG_USER=robson_staging
- RBS_PG_PASSWORD=(from postgres-staging secret)
- RBS_PG_HOST=postgres-staging
- RBS_PG_PORT=5432
- RBS_REDIS_URL=redis://:PASSWORD@redis-staging:6379/0
- RBS_RABBITMQ_URL=amqp://robson_staging:PASSWORD@rabbitmq-staging:5672
- BINANCE_API_KEY=testnet-placeholder
- BINANCE_API_SECRET=testnet-placeholder
- BINANCE_TESTNET=true

**ghcr-secret** (imagePullSecret):
- docker-server: ghcr.io
- docker-username: ldamasio
- docker-password: $GITHUB_TOKEN

---

## DNS Configuration

All 21 A records pointing to **158.220.116.31** (k3s cluster):

### Staging Subdomains
- staging.rbx.ia.br
- api.staging.rbx.ia.br
- ws.staging.rbx.ia.br
- criticws.staging.rbx.ia.br
- rabbitmq.staging.rbx.ia.br
- grafana.staging.rbx.ia.br

### Production Subdomains
- rbx.ia.br
- api.rbx.ia.br
- ws.rbx.ia.br
- criticws.rbx.ia.br
- rabbitmq.rbx.ia.br
- grafana.rbx.ia.br

### Observability
- prometheus.rbx.ia.br
- grafana.rbx.ia.br
- alertmanager.rbx.ia.br
- loki.rbx.ia.br
- tempo.rbx.ia.br
- pyroscope.rbx.ia.br
- argocd.rbx.ia.br
- vault.rbx.ia.br
- monitoring.rbx.ia.br

**DNS Propagation**: ✅ Complete (verified with `dig`)

---

## Docker Image Build

**Image**: `ghcr.io/ldamasio/rbs-backend-monolith:staging-latest`

**Build Context**:
```bash
cd apps/backend/monolith
docker build -f docker/Dockerfile_django -t ghcr.io/ldamasio/rbs-backend-monolith:staging-latest .
docker push ghcr.io/ldamasio/rbs-backend-monolith:staging-latest
```

**Includes**:
- Python 3.12 slim-bookworm
- All dependencies from requirements.txt
- Event Sourcing migrations (0015-0018):
  - 0015_event_sourcing_stop_monitor
  - 0016_add_stop_price_columns
  - 0017_set_stop_check_default
  - 0018_create_stop_indexes_concurrent
- Gunicorn + Gevent (5 workers, 1000 connections each)

---

## Issues Encountered and Fixed

### ✅ Issue 1: SSH Background Task Timeouts
- **Error**: Git Bash SSH commands timed out
- **Fix**: User provided password manually; used synchronous commands

### ✅ Issue 2: Docker Build Context Path
- **Error**: `COPY ../requirements.txt .` failed
- **Fix**: Changed build context from repo root to `apps/backend/monolith`

### ✅ Issue 3: ImagePullBackOff - Private Registry
- **Error**: Pods couldn't pull from GHCR (403 Forbidden)
- **Fix**: Created `ghcr-secret` with GITHUB_TOKEN

### ✅ Issue 4: Environment Variable Mismatch
- **Error**: `UndefinedValueError: RBS_SECRET_KEY not found`
- **Fix**: Added both `RBS_SECRET_KEY` and `SECRET_KEY` to secret

### ✅ Issue 5: PostgreSQL Connection Variables
- **Error**: `UndefinedValueError: RBS_PG_DATABASE not found`
- **Fix**: Added `RBS_PG_*` environment variables

### ✅ Issue 6: Permission Denied Creating Logs Directory
- **Error**: `PermissionError: [Errno 13] Permission denied: '/app/logs'`
- **Fix**: Removed `securityContext` (can be improved later with proper volume permissions)

### ✅ Issue 7: Kustomize Path Resolution
- **Error**: Kustomize can't access parent directories
- **Fix**: Applied manifests individually instead of `kubectl apply -k`

### ✅ Issue 8: Resource Quota Exceeded
- **Error**: Backend resource limits too high
- **Fix**: Suspended stop-monitor CronJob temporarily

---

## Current Critical Issue: Backend Pods Crashing

### Symptoms

**Pod Status** (as of last check):
```
NAME                                READY   STATUS             RESTARTS        AGE
backend-staging-7fccf4d6c7-fnqxb    0/1     CrashLoopBackOff   8 (2m38s ago)   27m
backend-staging-9865bcbdd-8dwnb     0/1     CrashLoopBackOff   4 (3m10s ago)   12m
backend-staging-d69c945d5-hsv6l     0/1     CrashLoopBackOff   5 (2m9s ago)    5m47s
```

**User-Reported Symptoms**:
- http://staging.rbx.ia.br/ → 404 (expected, frontend not deployed)
- https://api.staging.rbx.ia.br/ → 500 Server Error (consequence of crashed pods)
- TLS shows "Not Secure" (cert-manager processing)

### Investigation Checklist

**Need to check** (in order of priority):

1. **Pod logs** - Get crash reason:
   ```bash
   kubectl logs -n staging -l app=backend-staging --tail=200
   kubectl logs -n staging -l app=backend-staging --previous
   ```

2. **Database connectivity**:
   ```bash
   kubectl exec -it -n staging deployment/backend-staging -- sh
   # Inside container:
   python manage.py check --database default
   ```

3. **Migration status**:
   ```bash
   kubectl exec -it -n staging deployment/postgres-staging -- psql -U robson_staging -d robson_staging -c "\dt api_*"
   ```

4. **Environment variables**:
   ```bash
   kubectl exec -it -n staging deployment/backend-staging -- env | grep RBS
   ```

5. **Dependencies missing**:
   - Check if all Python packages are installed
   - Check if PostgreSQL extensions are available

### Likely Root Causes

Based on previous issues:

1. **Database connection failure**:
   - PostgreSQL not ready when backend starts
   - Wrong credentials (unlikely, already fixed)
   - ParadeDB extensions missing

2. **Migration failure**:
   - Migrations 0015-0018 failing to apply
   - Concurrent index creation issue (0018)

3. **Missing Python dependencies**:
   - requirements.txt not fully installed in Docker image
   - Binance SDK or other critical package missing

4. **Startup script error**:
   - Gunicorn command malformed
   - Django settings error

5. **Health check misconfiguration**:
   - Liveness/readiness probes failing prematurely
   - Startup timeout too short

---

## Migrations Status

**Expected migrations**:
- 0001-0014: Existing migrations (should be in database)
- 0015_event_sourcing_stop_monitor: NEW - StopMonitorEvent, StopMonitorExecution tables
- 0016_add_stop_price_columns: NEW - stop_price, stop_percent columns
- 0017_set_stop_check_default: NEW - default value for stop_check_count
- 0018_create_stop_indexes_concurrent: NEW - concurrent indexes

**Verification needed**:
```bash
kubectl exec -it -n staging deployment/backend-staging -- python manage.py showmigrations
```

---

## Next Steps (Ordered by Priority)

### Immediate (Critical)

1. **Get backend pod logs** to identify crash reason
2. **Check database connectivity** from backend container
3. **Verify migrations applied** successfully
4. **Fix crash issue** based on logs
5. **Restart pods** after fix

### Short-term (Important)

6. **Wait for TLS certificates** (cert-manager processing)
7. **Deploy frontend** to staging (or remove frontend routes from ingress)
8. **Test API endpoints** after backend is healthy
9. **Run backfill command**: `python manage.py backfill_stop_price`
10. **Enable stop-monitor CronJob** (currently suspended)

### Medium-term (Enhancement)

11. **Add health check endpoints** to backend
12. **Configure proper liveness/readiness probes**
13. **Add startup probe** with longer timeout
14. **Re-add securityContext** with proper volume permissions
15. **Optimize resource requests/limits** based on actual usage

### Long-term (Future Phases)

**PHASE 2: Backup & Disaster Recovery** (user explicitly requested):
- PostgreSQL backup scripts (pg_dump daily)
- S3/Backblaze B2 upload automation
- Point-in-Time Recovery (PITR) with WAL archiving
- Read replicas for dev/analytics
- Restore testing (monthly)

**PHASE 3: GitOps CI/CD** (user explicitly requested):
- GitHub Actions for automated builds
- Branch strategy: `main` → `staging-latest`, `tags` → production versions
- ArgoCD integration for auto-sync
- Rollback procedures

---

## Troubleshooting Commands

### Check Pod Status
```bash
kubectl get pods -n staging
kubectl describe pod -n staging <pod-name>
```

### Get Logs
```bash
# Current logs
kubectl logs -n staging -l app=backend-staging --tail=200

# Previous crash logs
kubectl logs -n staging -l app=backend-staging --previous

# Follow logs
kubectl logs -n staging -l app=backend-staging -f
```

### Exec into Container
```bash
# Backend pod
kubectl exec -it -n staging deployment/backend-staging -- sh

# PostgreSQL pod
kubectl exec -it -n staging deployment/postgres-staging -- sh
```

### Check Database
```bash
# From PostgreSQL pod
kubectl exec -it -n staging deployment/postgres-staging -- psql -U robson_staging -d robson_staging

# List tables
\dt api_*

# Check migrations table
SELECT * FROM django_migrations ORDER BY applied DESC LIMIT 10;
```

### Check Services
```bash
kubectl get svc -n staging
kubectl describe svc backend-staging -n staging
```

### Check Ingress
```bash
kubectl get ingress -n staging
kubectl describe ingress backend-staging -n staging
```

### Check Secrets
```bash
kubectl get secrets -n staging
kubectl describe secret django-staging -n staging

# View secret values (base64 encoded)
kubectl get secret django-staging -n staging -o yaml
```

### Resource Usage
```bash
kubectl top pods -n staging
kubectl describe quota staging-quota -n staging
```

---

## Files Modified in This Session

### Documentation
- **docs/infrastructure/STAGING-ARCHITECTURE.md** - Complete architecture specification
- **docs/infrastructure/DNS-MAPA-COMPLETO.md** - DNS mapping for all 21 records
- **docs/infrastructure/DNS-REGISTROS-REGISTRO-BR.md** - registro.br guide
- **docs/infrastructure/STAGING-DEPLOYMENT-STATE.md** - THIS FILE

### Kubernetes Manifests
- **infra/k8s/namespaces/staging.yml** - Namespace, ResourceQuota, LimitRange
- **infra/k8s/staging/network-policies/isolation.yml** - 7 network policies
- **infra/k8s/staging/postgres/postgres-staging.yml** - PostgreSQL deployment
- **infra/k8s/staging/redis/redis-staging.yml** - Redis deployment
- **infra/k8s/staging/rabbitmq/rabbitmq-staging.yml** - RabbitMQ deployment
- **infra/k8s/staging/backend/backend-staging.yml** - Backend deployment
- **infra/k8s/staging/backend/cronjob-stop-monitor.yml** - Stop monitor CronJob
- **infra/k8s/staging/istio/gateway-staging.yml** - Istio Gateway (NOT USED - Traefik instead)
- **infra/k8s/staging/istio/certificate-staging.yml** - Istio Certificate (NOT USED)
- **infra/k8s/staging/kustomization.yml** - Kustomize config
- **infra/k8s/staging/secrets/SECRETS-README.md** - Secrets documentation
- **infra/k8s/staging/DEPLOY-STAGING.md** - Deployment guide

### Scripts
- **deploy-staging.sh** - Automated deployment script

---

## Important Notes

### Production Isolation

**CRITICAL**: Staging is COMPLETELY isolated from production:

1. **Separate namespace** (`staging` vs `robson`)
2. **Network policy blocking production** namespace explicitly
3. **Separate database instances** with different credentials
4. **Separate secrets** with auto-generated passwords
5. **Separate DNS subdomains** (*.staging.rbx.ia.br)
6. **Separate PVCs** for data storage
7. **Binance Testnet** (not production API)

### Environment Variable Naming

Production Docker image expects **BOTH** formats:
- Standard: `SECRET_KEY`, `DATABASE_URL`
- Prefixed: `RBS_SECRET_KEY`, `RBS_PG_*`

Always provide both to ensure compatibility.

### Traefik vs Istio

**NOTE**: Cluster uses **Traefik Ingress Controller**, NOT Istio Gateway.

The Istio Gateway manifests were created but are NOT applied. Routing is done via Traefik Ingress resources.

---

## Session Continuation Instructions

**For future Claude Code sessions**:

1. **Read this file first** to understand current state
2. **Check pod status** to see if backend is still crashing
3. **Get pod logs** to identify root cause
4. **Fix crash issue** before proceeding with other tasks
5. **Refer to troubleshooting commands** section above
6. **Update this file** after making changes

**SSH Access**:
- Host: root@158.220.116.31
- User will provide password manually if timeout occurs
- Use synchronous commands when possible to avoid background timeouts

**GITHUB_TOKEN**:
- Already exported in user's `~/.bashrc`
- Use `source ~/.bashrc` before Docker commands if needed

---

## Success Criteria

Staging deployment will be considered **COMPLETE** when:

- ✅ All pods are Running (0 CrashLoopBackOff)
- ✅ Migrations 0015-0018 applied successfully
- ✅ https://api.staging.rbx.ia.br/health returns 200 OK
- ✅ TLS certificates issued (HTTPS shows "Secure")
- ✅ Backfill command executed successfully
- ✅ Stop monitor CronJob running every minute
- ✅ Frontend deployed or routes removed from ingress

---

**Last Status Check**: 2024-12-25
**Backend Pods**: ❌ CrashLoopBackOff (multiple restarts)
**PostgreSQL**: ✅ Running
**Redis**: ✅ Running
**RabbitMQ**: ✅ Running
**API Endpoint**: ❌ 500 Error
**Frontend**: ❌ 404 (expected)
**TLS**: ⚠️ Processing

**NEXT ACTION REQUIRED**: Get backend pod logs to identify crash reason.
