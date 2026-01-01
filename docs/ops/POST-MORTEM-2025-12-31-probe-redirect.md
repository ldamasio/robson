# Post-Mortem: Kubernetes Probe Failures Due to HTTP→HTTPS Redirect

**Date**: 2025-12-31
**Status**: RESOLVED
**Severity**: P1 (Production outage)
**Duration**: ~2 hours
**Affected Services**: Backend API (rbs-backend-monolith-prod-deploy)

---

## Incident Summary

### What Broke
Kubernetes liveness and readiness probes failed repeatedly, causing backend pods to restart in a loop. ArgoCD reported `OutOfSync` with `SyncError` preventing deployment reconciliation.

### Visible Symptoms
- Pods: CrashLoopBackOff / Restarting
- ArgoCD: `OutOfSync | Missing | SyncError`
- Error: `livenessProbe.tcpSocket: Forbidden: may not specify more than 1 handler type`
- Deployment: NotFound in cluster (deleted by ArgoCD during failed sync attempt)
- User-facing: API intermittently unavailable during pod restarts

---

## Root Cause (Technical)

### Primary Issue: HTTP→HTTPS Redirect on Probe Endpoints

Django setting `SECURE_SSL_REDIRECT=True` enforced by `SecurityMiddleware` redirects all HTTP requests to HTTPS with 301 status.

**Probe Flow**:
```
1. kubelet sends: GET http://pod:8000/healthz (scheme: HTTP)
2. Django SecurityMiddleware: 301 → https://pod:8000/healthz
3. kubelet follows redirect: GET https://pod:8000/healthz
4. Pod port 8000 serves plain HTTP (no TLS)
5. Connection fails: EOF (TLS handshake on non-TLS port)
6. Probe marked failed
7. After failureThreshold (3): Pod killed and restarted
```

**Evidence**:
```bash
$ kubectl port-forward pod/$POD 18000:8000
$ curl -i http://127.0.0.1:18000/healthz
HTTP/1.1 301 Moved Permanently
Location: https://127.0.0.1:18000/healthz
```

### Secondary Issue: ArgoCD GitOps Drift

Manual `kubectl patch` applied `tcpSocket` probes to work around redirect issue. This created drift between Git manifest (httpGet) and live state (tcpSocket).

When ArgoCD attempted to sync:
1. Git manifest: `livenessProbe.httpGet`
2. Live state: `livenessProbe.tcpSocket`
3. Kubernetes strategic merge: Both handlers present in merged manifest
4. Kubernetes API rejection: `Forbidden: may not specify more than 1 handler type`
5. Deployment deleted during reconciliation, not recreated (failed validation)

---

## Why ArgoCD Went OutOfSync

**Timeline**:
1. Probes failed with 301 → EOF
2. Manual patch: Changed probes to `tcpSocket` (bypasses HTTP layer)
3. Pods stabilized (tcpSocket only checks port open, not HTTP response)
4. Git push triggered ArgoCD sync
5. ArgoCD attempted to apply Git manifest (httpGet) over live state (tcpSocket)
6. Kubernetes merge created invalid manifest with both handlers
7. API Server rejected manifest
8. ArgoCD cached failed operation state
9. Deployment deleted, not recreated (validation error)
10. Application status: `OutOfSync | Missing | SyncError`

**Critical Mistake**: Manual `kubectl patch` bypassed GitOps workflow, creating unrecoverable drift.

---

## Fix Implemented

### 1. ProbeNoRedirectMiddleware (Code Fix)

**File**: `apps/backend/monolith/api/middleware/probe_no_redirect.py`

```python
class ProbeNoRedirectMiddleware:
    def __call__(self, request):
        if request.path in ('/healthz', '/readyz'):
            # Set header that SECURE_PROXY_SSL_HEADER checks
            # Makes SecurityMiddleware think request is HTTPS
            request.META['HTTP_X_FORWARDED_PROTO'] = 'https'

        response = self.get_response(request)
        return response
```

**Placement**: BEFORE `SecurityMiddleware` in `settings.MIDDLEWARE`

**Effect**:
- Probe endpoints return 200 over HTTP (no redirect)
- Regular endpoints still redirect to HTTPS (security preserved)
- Safe: Probe endpoints contain no user data, accessed only by kubelet

### 2. GitOps Recovery (Infrastructure Fix)

**Actions**:
1. Verified Git manifest clean (only httpGet, no tcpSocket)
2. Added annotation to force ArgoCD refresh: `robson.rbx.ia.br/probe-fix: "2025-12-31-httpGet-only"`
3. Cleared ArgoCD cached operation state: `kubectl patch application robson-prod --type json -p='[{"op": "remove", "path": "/status/operationState"}]'`
4. Triggered fresh sync: `kubectl patch application robson-prod --type merge -p '{"operation":{"sync":{...}}}'`
5. ArgoCD created Deployment from clean Git manifest
6. CI/CD built new image with middleware fix
7. ArgoCD synced new image tag
8. Pods deployed with httpGet probes + middleware fix

---

## Verification

### Pre-Fix
```bash
$ curl -i http://pod:8000/healthz
HTTP/1.1 301 Moved Permanently  # ← FAIL
Location: https://...
```

### Post-Fix
```bash
$ curl -i http://pod:8000/healthz
HTTP/1.1 200 OK  # ← SUCCESS
{"status":"alive","service":"robson-backend"}

$ curl -i http://pod:8000/readyz
HTTP/1.1 200 OK  # ← SUCCESS
{"status":"ready","service":"robson-backend","checks":{"database":"healthy","cache":"healthy"}}
```

### Middleware Verification
```bash
$ kubectl exec deploy/rbs-backend-monolith-prod-deploy -- \
  python -c "from django.conf import settings; \
  print('ProbeNoRedirect loaded:', \
  'api.middleware.probe_no_redirect.ProbeNoRedirectMiddleware' in settings.MIDDLEWARE)"

ProbeNoRedirect loaded: True
```

### ArgoCD Status
```bash
$ kubectl get application robson-prod -n argocd
NAME          SYNC STATUS   HEALTH STATUS
robson-prod   Synced        Healthy
```

### Pod Stability
```bash
$ kubectl get pods -n robson -l app=rbs-backend-monolith-prod-deploy
NAME                                                READY   STATUS    RESTARTS   AGE
rbs-backend-monolith-prod-deploy-5c66966fc4-tb6qv   1/1     Running   0          15m
```

---

## Preventive Rules

### 1. GitOps Discipline
- **NEVER** `kubectl patch` production resources directly
- **ALWAYS** commit changes to Git, let ArgoCD sync
- Emergency fixes: Use `kubectl apply -f` with Git-committed manifest, then git push immediately

### 2. Probe Design
- Probe endpoints MUST NOT trigger authentication, redirects, or business logic
- Use dedicated `/healthz` (liveness) and `/readyz` (readiness) endpoints
- `/healthz`: Fast, no dependencies (process alive check)
- `/readyz`: Check critical dependencies (DB, cache)
- Both: Return JSON, no redirects, no auth required

### 3. Security + Probes
- When using `SECURE_SSL_REDIRECT=True`:
  - Probe endpoints MUST bypass redirect (via middleware or configuration)
  - Document why bypass is safe (no user data, internal-only access)
- Test probes over plain HTTP: `kubectl port-forward pod/$POD 18000:8000 && curl http://localhost:18000/healthz`

### 4. CI/CD + ArgoCD
- All manifest changes go through Git
- CI/CD updates image tags via GitOps commits
- ArgoCD auto-syncs every 3 minutes
- Manual sync only for urgent deployments: `kubectl patch application` (not resources directly)

### 5. Incident Response
- Check ArgoCD first: `kubectl get application -n argocd`
- If OutOfSync: Compare Git vs Live: `kubectl diff -f manifest.yml`
- Clear operation state if stale: `kubectl patch application --type json -p='[{"op":"remove","path":"/status/operationState"}]'`
- Never delete resources manually; let ArgoCD reconcile

---

## Final Stable State

```yaml
Deployment: rbs-backend-monolith-prod-deploy
Image:      ldamasio/rbs-backend-monolith-prod:sha-1e29f02
Replicas:   1/1 Ready
Restarts:   0

Probes:
  livenessProbe:
    httpGet:
      path: /healthz
      port: 8000
      scheme: HTTP
    initialDelaySeconds: 30
    periodSeconds: 10

  readinessProbe:
    httpGet:
      path: /readyz
      port: 8000
      scheme: HTTP
    initialDelaySeconds: 10
    periodSeconds: 5

Middleware:
  - django_prometheus.middleware.PrometheusBeforeMiddleware
  - api.middleware.correlation_id.CorrelationIDMiddleware
  - api.middleware.probe_no_redirect.ProbeNoRedirectMiddleware  # ← FIX
  - corsheaders.middleware.CorsMiddleware
  - django.middleware.security.SecurityMiddleware

ArgoCD:
  Sync Status:   Synced
  Health Status: Healthy
  Revision:      1e29f02f (main)
```

---

## Commits

- `6aa6cbc0` - fix(k8s): bypass SSL redirect for probe endpoints /healthz and /readyz
- `1e29f02f` - fix(k8s): add annotation to force ArgoCD refresh after probe conflict

---

## Lessons Learned

1. **SecurityMiddleware globally redirects HTTP→HTTPS**: Probe endpoints need explicit bypass
2. **GitOps drift is unrecoverable without manual intervention**: ArgoCD cannot merge conflicting handlers
3. **Manual kubectl patches break GitOps**: Always commit to Git first
4. **Probe failures cascade**: Bad probes → pod restarts → service disruption
5. **Test probes in isolation**: Port-forward and curl directly to pod, not via Ingress

---

**Status**: RESOLVED
**Next Review**: 2026-01-15 (verify no probe failures in 2 weeks)
**Owner**: Platform Team
