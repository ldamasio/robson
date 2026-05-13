# Robson V2 Kubernetes Manifests

This directory contains Kubernetes manifests for deploying Robson v2 daemon to production.

## Structure

```
v2/k8s/prod/
├── namespace.yml                    # robson-v2 namespace
├── robsond-configmap.yml            # Configuration (non-sensitive)
├── robsond-secret.yml.template      # Secret template (NEVER commit actual secret!)
├── robsond-deployment.yml           # Deployment (single replica)
├── robsond-service.yml              # ClusterIP service
├── robsond-rbac.yml                 # ServiceAccount + Role + RoleBinding
└── kustomization.yml                # Kustomize configuration
```

## Quick Start

### 1. Create Secret

```bash
# Copy template
cp robsond-secret.yml.template robsond-secret.yml

# Edit and fill in actual values
vi robsond-secret.yml

# Apply secret (FIRST, before deployment)
kubectl apply -f robsond-secret.yml
```

**Required values:**
- `database-url`: PostgreSQL connection string for robson_v2 database
- `binance-api-key`: Binance API key (production or testnet)
- `binance-api-secret`: Binance API secret

### 2. Deploy with Kustomize

```bash
# Preview changes
kubectl apply -k . --dry-run=client

# Apply all manifests
kubectl apply -k .
```

### 3. Verify Deployment

```bash
# Check pod status
kubectl get pods -n robson-v2

# Check health probes
kubectl port-forward -n robson-v2 svc/robsond-service 8080:8080
curl http://localhost:8080/healthz  # Should return 200 OK
curl http://localhost:8080/readyz   # Should return 200 OK (checks DB + Binance)

# View logs
kubectl logs -f -n robson-v2 deployment/robsond
```

## Configuration

### ConfigMap (robsond-configmap.yml)

Non-sensitive configuration:
- Binance API base URL
- Log levels
- Safety Net settings (polling interval, symbols)

To update configuration:

```bash
kubectl edit configmap robsond-config -n robson-v2
# Restart pod to apply changes
kubectl rollout restart deployment/robsond -n robson-v2
```

### Secret (robsond-secret.yml)

Sensitive credentials:
- Database URL
- Binance API key/secret

**NEVER commit actual secret values to git!**

To update secret:

```bash
kubectl edit secret robsond-secret -n robson-v2
# Restart pod to apply changes
kubectl rollout restart deployment/robsond -n robson-v2
```

## Health Checks

The deployment includes three types of health checks:

1. **Liveness Probe** (`/healthz`): Checks if process is alive
   - Failure threshold: 3 consecutive failures
   - Restart pod on failure

2. **Readiness Probe** (`/readyz`): Checks if service is ready
   - Checks: Database connectivity, Binance API reachability
   - Remove from service endpoints if not ready

3. **Startup Probe** (`/healthz`): Gives extra time for initial startup
   - Max 150 seconds (30 failures × 5s interval)

## Scaling

Current deployment uses **single replica** (no leader election yet):

```yaml
spec:
  replicas: 1
  strategy:
    type: Recreate  # No rolling updates
```

For high availability (future):
1. Implement leader election (e.g., using Kubernetes Lease API)
2. Change strategy to `RollingUpdate`
3. Increase replicas: `kubectl scale deployment robsond --replicas=2 -n robson-v2`

## Resource Limits

Default resource allocation:

```yaml
resources:
  requests:
    memory: "256Mi"
    cpu: "250m"
  limits:
    memory: "512Mi"
    cpu: "500m"
```

Adjust based on actual usage:

```bash
# View resource usage
kubectl top pod -n robson-v2
```

## Monitoring

Prometheus scraping enabled via annotations:

```yaml
annotations:
  prometheus.io/scrape: "true"
  prometheus.io/port: "8080"
  prometheus.io/path: "/metrics"
```

(Note: `/metrics` endpoint not yet implemented, placeholder for future)

## Troubleshooting

### Pod not starting

```bash
# Check events
kubectl describe pod -n robson-v2 -l app=robsond

# Check logs
kubectl logs -n robson-v2 -l app=robsond --tail=100
```

### Health probes failing

```bash
# Port-forward and test directly
kubectl port-forward -n robson-v2 svc/robsond-service 8080:8080

# Test liveness
curl -v http://localhost:8080/healthz

# Test readiness (should check DB + Binance)
curl -v http://localhost:8080/readyz
```

### Database connection issues

```bash
# Test database connectivity from pod
kubectl exec -it -n robson-v2 deployment/robsond -- bash
# Inside pod:
# apt-get update && apt-get install -y postgresql-client
# psql $DATABASE_URL -c "SELECT 1;"
```

### Binance API issues

```bash
# Check logs for API errors
kubectl logs -n robson-v2 -l app=robsond | grep -i binance
```

## Cleanup

To remove the deployment:

```bash
# Delete all resources
kubectl delete -k .

# Or delete namespace (removes everything)
kubectl delete namespace robson-v2
```

## Production Checklist

Before deploying to production:

- [ ] Database migrated to v2 schema (`v2/migrations/*.sql`)
- [ ] Secret created with production Binance API keys
- [ ] Resource limits tuned based on staging metrics
- [ ] Monitoring alerts configured
- [ ] Backup strategy in place
- [ ] Rollback plan documented
- [ ] v1 CronJobs disabled (to prevent double execution)
- [ ] 48+ hours of staging validation completed

## Related Documentation

- [V2 Architecture](../../docs/ARCHITECTURE.md)
- [Execution Plan](../../docs/plan/PHASE-9-10-PRODUCTION-READINESS.md)
- [ADR-0014: Safety Net Coordination](../../docs/adr/ADR-0014-safety-net-core-trading-coordination.md)
