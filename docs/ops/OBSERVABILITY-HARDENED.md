# Observability - Hardened Implementation

Production-ready observability with security hardening and proper semantics.

---

## Architecture

```
Public Internet
    ↓
Ingress (Traefik)
    ├─→ /healthz, /readyz → Backend (allowed)
    ├─→ /metrics → BLOCKED
    ├─→ /health/ → BLOCKED
    └─→ /api/*,  / → Backend (allowed)

Internal Network
    └─→ Service (ClusterIP)
        └─→ Backend Pod
            ├─→ /healthz (liveness - fast, no deps)
            ├─→ /readyz (readiness - checks DB/cache)
            ├─→ /metrics (Prometheus scrape)
            └─→ /health/ (detailed diagnostics)
```

---

## Endpoints

### Public Endpoints (via Ingress)

#### `/healthz` - Liveness Probe
**Purpose**: Kubernetes liveness check
**Response Time**: < 100ms
**Checks**: None (just process alive)
**Returns**: Always 200 OK if process running

```bash
curl https://api.robson.rbx.ia.br/healthz
# {"status":"alive","service":"robson-backend"}
```

**Use Case**: Kubernetes kills pod if this fails (pod restart needed)

---

#### `/readyz` - Readiness Probe
**Purpose**: Kubernetes readiness check
**Response Time**: < 1s
**Checks**: Database, Cache
**Returns**: 200 if ready, 503 if not

```bash
curl https://api.robson.rbx.ia.br/readyz
# {"status":"ready","service":"robson-backend","checks":{"database":"healthy","cache":"healthy"}}
```

**Use Case**: Kubernetes removes pod from load balancer if this fails

---

### Internal Endpoints (Cluster Only)

#### `/metrics` - Prometheus Metrics
**Purpose**: Application and infrastructure metrics
**Access**: Internal cluster only (blocked on ingress)
**Format**: Prometheus exposition format

```bash
# From inside cluster
kubectl -n robson exec deployment/rbs-backend-monolith-prod-deploy -- curl http://localhost:8000/metrics

# Prometheus scrapes via ServiceMonitor
```

**Metrics Available**:
- `django_http_requests_total_by_method_total`
- `django_http_requests_latency_seconds_by_view_method`
- `django_http_responses_total_by_status_view_method`
- `django_db_new_connections_total`
- `django_db_query_count`
- `django_cache_get_total`

---

#### `/health/` - Detailed Diagnostics
**Purpose**: Comprehensive health diagnostics
**Access**: Internal cluster only (blocked on ingress)
**Format**: JSON with detailed component status

```bash
# From inside cluster
kubectl -n robson exec deployment/rbs-backend-monolith-prod-deploy -- curl http://localhost:8000/health/

# Returns detailed status per component
```

---

## Security Hardening

### 1. Service Type Changed

**Before**: `type: LoadBalancer` (publicly exposed)
**After**: `type: ClusterIP` (internal only)

**Impact**: Backend Service no longer has public IP, only accessible via Ingress.

### 2. Ingress Path Blocking

**Implementation**: Explicit path rules in Ingress

```yaml
# /metrics and /health/ route to non-existent service → 404/503
- pathType: Prefix
  path: "/metrics"
  backend:
    service:
      name: blocked-endpoint
      port:
        number: 1
```

**Verification**:
```bash
# Should return 503/404
curl -I https://api.robson.rbx.ia.br/metrics
curl -I https://api.robson.rbx.ia.br/health/

# Should return 200
curl -I https://api.robson.rbx.ia.br/healthz
curl -I https://api.robson.rbx.ia.br/readyz
```

### 3. Correlation ID Implementation

**Header Propagation**:
- Request: `X-Request-ID` (read if present, generate if not)
- Response: `X-Request-ID` (always returned)
- Logs: `correlation_id` field in JSON

**Middleware**: `api.middleware.correlation_id.CorrelationIDMiddleware`

**Example**:
```bash
# Send request with ID
curl -H "X-Request-ID: test-123" https://api.robson.rbx.ia.br/api/ping/

# Response header will include: X-Request-ID: test-123
# Logs will include: {"correlation_id": "test-123", ...}
```

---

## Prometheus Integration

### ServiceMonitor (if Prometheus Operator exists)

**Check if Operator is installed**:
```bash
kubectl get crd | grep servicemonitors.monitoring.coreos.com
```

**If exists**:
- ServiceMonitor auto-discovers backend `/metrics`
- Scrape interval: 30s
- Timeout: 10s

**Verify scraping**:
```bash
# Check ServiceMonitor created
kubectl -n robson get servicemonitor

# Check Prometheus targets (if Prometheus UI accessible)
# Navigate to: http://prometheus.example.com/targets
# Look for: robson/rbs-backend-monolith-prod-monitor
```

### Manual Scrape Config (if Operator NOT installed)

If ServiceMonitor CRD doesn't exist, add manual scrape config to Prometheus:

```yaml
scrape_configs:
  - job_name: 'robson-backend'
    kubernetes_sd_configs:
      - role: pod
        namespaces:
          names:
            - robson
    relabel_configs:
      - source_labels: [__meta_kubernetes_pod_label_app]
        action: keep
        regex: rbs-backend-monolith-prod-deploy
      - source_labels: [__meta_kubernetes_pod_ip]
        target_label: __address__
        replacement: ${1}:8000
      - source_labels: [__meta_kubernetes_pod_name]
        target_label: pod
      - target_label: __metrics_path__
        replacement: /metrics
```

---

## Verification Commands

### 1. Verify Cluster Prerequisites

```bash
# Check ServiceMonitor CRD exists
kubectl get crd | grep servicemonitors.monitoring.coreos.com || echo "Prometheus Operator NOT installed"

# Check Prometheus namespace
kubectl get ns | egrep 'monitoring|prometheus' || echo "No Prometheus namespace found"

# Check Prometheus deployments
kubectl get deploy -A | egrep 'prometheus|operator' || echo "No Prometheus Operator found"
```

### 2. Verify Endpoints Inside Pod

```bash
POD=$(kubectl -n robson get pod -l app=rbs-backend-monolith-prod-deploy -o jsonpath='{.items[0].metadata.name}')

# Test /healthz (should return 200)
kubectl -n robson exec $POD -- curl -s -o /dev/null -w "%{http_code}" http://localhost:8000/healthz
# Expected: 200

# Test /readyz (should return 200 if DB/cache healthy)
kubectl -n robson exec $POD -- curl -s http://localhost:8000/readyz | jq
# Expected: {"status":"ready", "checks":{...}}

# Test /metrics (should return Prometheus metrics)
kubectl -n robson exec $POD -- curl -s http://localhost:8000/metrics | head -20
# Expected: # HELP django_http_requests_total...

# Test /health/ (should return detailed diagnostics)
kubectl -n robson exec $POD -- curl -s http://localhost:8000/health/ | jq
# Expected: {"status":"healthy", "checks":{...}}
```

### 3. Verify Public Ingress Blocks Internal Endpoints

```bash
# /metrics should be blocked (503 or 404)
curl -I https://api.robson.rbx.ia.br/metrics
# Expected: HTTP/1.1 503 Service Unavailable

# /health/ should be blocked (503 or 404)
curl -I https://api.robson.rbx.ia.br/health/
# Expected: HTTP/1.1 503 Service Unavailable

# /healthz should be allowed (200)
curl -I https://api.robson.rbx.ia.br/healthz
# Expected: HTTP/1.1 200 OK

# /readyz should be allowed (200)
curl -I https://api.robson.rbx.ia.br/readyz
# Expected: HTTP/1.1 200 OK
```

### 4. Verify Correlation ID in Logs

```bash
# Send request with custom request ID
curl -H "X-Request-ID: verify-test-12345" https://api.robson.rbx.ia.br/api/ping/

# Check logs contain correlation_id
kubectl -n robson logs deployment/rbs-backend-monolith-prod-deploy --tail=10 | jq 'select(.correlation_id=="verify-test-12345")'

# Expected: JSON log entry with "correlation_id": "verify-test-12345"
```

### 5. Verify Probes Working

```bash
# Check probe status in pod description
kubectl -n robson describe pod -l app=rbs-backend-monolith-prod-deploy | grep -A 5 "Liveness\|Readiness"

# Expected: No "Unhealthy" events

# Check recent events for probe failures
kubectl -n robson get events --field-selector involvedObject.kind=Pod --field-selector reason=Unhealthy

# Expected: No recent probe failures
```

### 6. Verify Service Type

```bash
kubectl -n robson get svc rbs-backend-monolith-prod-svc -o jsonpath='{.spec.type}'
# Expected: ClusterIP

kubectl -n robson get svc rbs-backend-monolith-prod-svc -o jsonpath='{.spec.clusterIP}'
# Expected: Internal IP (10.x.x.x)
```

### 7. Verify Prometheus Scraping (if Operator installed)

```bash
# Check ServiceMonitor exists
kubectl -n robson get servicemonitor rbs-backend-monolith-prod-monitor

# Check Service has correct labels
kubectl -n robson get svc rbs-backend-monolith-prod-svc -o yaml | grep -A 3 labels

# If Prometheus UI accessible, check targets
# http://prometheus.example.com/targets
# Look for: robson/rbs-backend-monolith-prod-monitor/0 (UP)
```

---

## Troubleshooting

### /healthz or /readyz returns 404

**Cause**: New endpoints not in deployed image yet
**Solution**: Wait for CI/CD to build new image with updated code

```bash
# Check current image
kubectl -n robson get deployment rbs-backend-monolith-prod-deploy \
  -o jsonpath='{.spec.template.spec.containers[0].image}'

# Should be: ldamasio/rbs-backend-monolith-prod:sha-XXXXXXX (recent commit)
```

### /metrics still accessible publicly

**Cause**: Ingress not updated
**Solution**: Verify Ingress rules

```bash
kubectl -n robson get ingress rbs-backend-prod-ingress -o yaml | grep -A 10 "/metrics"

# Should route to "blocked-endpoint" service
```

### Correlation ID not in logs

**Cause**: Middleware not loaded or DEBUG=True
**Solution**: Check settings

```bash
# Verify DEBUG=False in production
kubectl -n robson get deployment rbs-backend-monolith-prod-deploy \
  -o jsonpath='{.spec.template.spec.containers[0].env[?(@.name=="DEBUG")].value}'

# Expected: False

# Check logs are JSON formatted
kubectl -n robson logs deployment/rbs-backend-monolith-prod-deploy --tail=1

# Expected: Valid JSON with correlation_id field
```

### ServiceMonitor not scraping

**Cause 1**: Prometheus Operator not installed
**Solution**: Use manual scrape config (see above)

**Cause 2**: Label selector mismatch
**Solution**: Check Prometheus serviceMonitorSelector

```bash
# Get Prometheus spec (if using Operator)
kubectl get prometheus -A -o yaml | grep -A 5 serviceMonitorSelector

# Ensure ServiceMonitor has matching labels
kubectl -n robson get servicemonitor rbs-backend-monolith-prod-monitor -o yaml | grep -A 5 labels
```

---

## Summary

| Endpoint | Public Access | Purpose | Checks |
|----------|---------------|---------|--------|
| `/healthz` | ✅ Allowed | Liveness | None (fast) |
| `/readyz` | ✅ Allowed | Readiness | DB, Cache |
| `/metrics` | ❌ Blocked | Metrics | N/A (internal) |
| `/health/` | ❌ Blocked | Diagnostics | N/A (internal) |

**Security**:
- ✅ Service type: ClusterIP (not LoadBalancer)
- ✅ Internal endpoints blocked on Ingress
- ✅ Correlation IDs in logs and headers
- ✅ Proper liveness/readiness semantics

**Observability**:
- ✅ Prometheus metrics (internal scrape)
- ✅ JSON structured logs with correlation_id
- ✅ Health checks with dependency validation

---

**Last Updated**: 2025-12-31
**Related**: OBSERVABILITY.md, Security, SRE Best Practices
