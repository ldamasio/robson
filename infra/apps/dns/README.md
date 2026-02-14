# DNS Infrastructure for strategos.gr

This directory contains Kubernetes manifests for hosting authoritative DNS for the `strategos.gr` domain within the k3s cluster.

## Quick Start

### 1. Run Discovery

Determine which deployment scenario to use:

```bash
cd /home/psyctl/apps/robson
./infra/scripts/dns-discovery.sh
```

This will output either:
- **Scenario A**: MetalLB LoadBalancer (preferred)
- **Scenario B**: NodePort with fixed nodes (fallback)

### 2. Follow Runbook

See comprehensive deployment guide:

```bash
less infra/docs/dns/MIGRATION-RUNBOOK.md
```

Or use the checklist:

```bash
less infra/docs/dns/CHECKLIST.md
```

## Architecture

Two deployment scenarios:

### Scenario A: MetalLB (Preferred)

- Uses MetalLB L2Advertisement
- Two LoadBalancer services with dedicated IPs
- 2 replicas per nameserver (4 PowerDNS pods total)
- Automatic failover

**Deploy**: `kubectl apply -f infra/k8s/gitops/applications/dns-metallb.yml`

### Scenario B: NodePort (Fallback)

- Uses NodePort services (30053, 30054)
- 1 pod per nameserver (pinned to specific nodes)
- Manual firewall configuration on VPS
- Direct VPS IP usage

**Deploy**: `kubectl apply -f infra/k8s/gitops/applications/dns-nodeport.yml`

## Directory Structure

```
dns/
├── README.md                  # This file
├── base/                      # Shared resources
│   ├── namespace.yaml         # dns namespace
│   ├── postgresql.yaml        # PostgreSQL backend
│   ├── powerdns-config.yaml   # PowerDNS configuration
│   ├── zone-init-job.yaml     # Zone initialization
│   └── pdb.yaml               # PodDisruptionBudget
└── overlays/
    ├── metallb/               # Scenario A
    │   ├── powerdns-deployment.yaml
    │   ├── services.yaml
    │   ├── metallb-config.yaml
    │   └── kustomization.yaml
    └── nodeport/              # Scenario B
        ├── powerdns-deployment.yaml
        ├── services.yaml
        ├── firewall-configmap.yaml
        └── kustomization.yaml
```

## Prerequisites

### Both Scenarios

- [ ] k3s cluster accessible
- [ ] ArgoCD installed
- [ ] 2 public IP addresses allocated
- [ ] PostgreSQL password generated
- [ ] PowerDNS API key generated
- [ ] Glue records verified at rbxsystems.ch

### Scenario A (MetalLB) Only

- [ ] MetalLB installed in cluster
- [ ] IPAddressPool configured
- [ ] L2Advertisement functional

### Scenario B (NodePort) Only

- [ ] SSH access to VPS nodes
- [ ] UFW configured and running
- [ ] Node hostnames identified

## Configuration

### Required Secrets

Before deployment, update these placeholders:

**In `base/postgresql.yaml`:**
- `CHANGEME_POSTGRES_ROOT_PASSWORD`
- `CHANGEME_PDNS_PASSWORD`

**In `base/powerdns-config.yaml`:**
- `CHANGEME_PDNS_API_KEY`

**In `base/zone-init-job.yaml`:**
- `CHANGEME_STRATEGOS_IP`

### Required IPs (Scenario-Specific)

**Scenario A (`overlays/metallb/`):**

In `metallb-config.yaml` and `services.yaml`:
- `NS1_IP_PLACEHOLDER`
- `NS2_IP_PLACEHOLDER`

**Scenario B (`overlays/nodeport/`):**

In `powerdns-deployment.yaml`:
- `NODE1_HOSTNAME_PLACEHOLDER`
- `NODE2_HOSTNAME_PLACEHOLDER`

## Deployment

### Via ArgoCD (Recommended)

**Scenario A:**
```bash
kubectl apply -f infra/k8s/gitops/applications/dns-metallb.yml
argocd app sync dns-infrastructure-metallb
```

**Scenario B:**
```bash
kubectl apply -f infra/k8s/gitops/applications/dns-nodeport.yml
argocd app sync dns-infrastructure-nodeport
```

### Via Kustomize (Manual)

**Scenario A:**
```bash
kubectl apply -k overlays/metallb/
```

**Scenario B:**
```bash
kubectl apply -k overlays/nodeport/
```

## Post-Deployment

### Verify Deployment

```bash
# Check pods
kubectl get pods -n dns

# Check services
kubectl get svc -n dns

# Check logs
kubectl logs -n dns -l app=powerdns --tail=50
```

### Run Zone Initialization

```bash
kubectl apply -f base/zone-init-job.yaml
kubectl logs -n dns -f job/zone-init
```

### Test DNS Resolution

```bash
# External test
dig @<NS1_IP> strategos.gr SOA
dig @<NS2_IP> strategos.gr NS

# Verify authoritative flag
dig @<NS1_IP> strategos.gr SOA | grep "flags:"
# Should show: flags: qr aa rd
```

## Operations

### View Logs

```bash
# PowerDNS
kubectl logs -n dns -l app=powerdns --tail=100 -f

# PostgreSQL
kubectl logs -n dns postgresql-0 --tail=100 -f
```

### Access PowerDNS API

```bash
# Port forward
kubectl port-forward -n dns svc/powerdns-api 8081:8081

# Query zone (in another terminal)
export PDNS_API_KEY="<your-api-key>"
curl -H "X-API-Key: ${PDNS_API_KEY}" \
  http://localhost:8081/api/v1/servers/localhost/zones/strategos.gr
```

### Backup Zone

```bash
# Manual backup
kubectl exec -n dns postgresql-0 -- \
  pg_dump -U pdns_strategos strategos_dns > backup-$(date +%Y%m%d).sql

# Restore
kubectl cp backup-20260214.sql dns/postgresql-0:/tmp/
kubectl exec -n dns postgresql-0 -- \
  psql -U pdns_strategos -d strategos_dns -f /tmp/backup-20260214.sql
```

### Update Zone Records

```bash
# Connect to PostgreSQL
kubectl exec -it -n dns postgresql-0 -- \
  psql -U pdns_strategos -d strategos_dns

# Add A record
INSERT INTO records (domain_id, name, type, content, ttl, auth)
VALUES (
  (SELECT id FROM domains WHERE name = 'strategos.gr'),
  'new.strategos.gr',
  'A',
  '1.2.3.4',
  3600,
  true
);

# View records
SELECT name, type, content FROM records
WHERE domain_id = (SELECT id FROM domains WHERE name = 'strategos.gr');
```

### Scale Replicas (MetalLB only)

```bash
kubectl scale deployment -n dns powerdns-ns1 --replicas=3
kubectl scale deployment -n dns powerdns-ns2 --replicas=3
```

## Troubleshooting

### Pods Not Starting

```bash
# Check events
kubectl get events -n dns --sort-by='.lastTimestamp'

# Describe pod
kubectl describe pod -n dns <pod-name>
```

### DNS Not Resolving

```bash
# Test internal resolution
kubectl exec -n dns <powerdns-pod> -- dig @localhost strategos.gr SOA

# Check service
kubectl get svc -n dns dns-ns1 dns-ns2

# NodePort: Check firewall
ssh admin@<node-ip> "sudo ufw status | grep 53"
```

### Database Issues

```bash
# Check PostgreSQL logs
kubectl logs -n dns postgresql-0

# Test connection
kubectl exec -n dns postgresql-0 -- psql -U pdns_strategos -d strategos_dns -c "SELECT version();"
```

## Rollback

See detailed rollback procedures:

```bash
less infra/docs/dns/ROLLBACK.md
```

**Quick rollback:**

```bash
# Stop auto-sync
argocd app set dns-infrastructure-<scenario> --auto-sync-policy none

# Delete application
kubectl delete application -n argocd dns-infrastructure-<scenario>

# Clean up
kubectl delete namespace dns
```

## Documentation

- **[ARCHITECTURE.md](../../docs/dns/ARCHITECTURE.md)** - Detailed architecture and design decisions
- **[MIGRATION-RUNBOOK.md](../../docs/dns/MIGRATION-RUNBOOK.md)** - Step-by-step deployment guide
- **[ROLLBACK.md](../../docs/dns/ROLLBACK.md)** - Rollback procedures
- **[CHECKLIST.md](../../docs/dns/CHECKLIST.md)** - Pre/post deployment checklist
- **[ADR-0014](../../docs/adr/ADR-0014-authoritative-dns-in-cluster.md)** - Architecture Decision Record

## Security

- ✅ Recursion disabled (`recursor=""`)
- ✅ AXFR blocked (`disable-axfr=yes`)
- ✅ API internal only (ClusterIP)
- ✅ Pod security standards enforced (baseline)
- ✅ Non-root user in containers
- ✅ Secrets properly managed

## Monitoring

### Health Checks

PowerDNS has liveness and readiness probes on port 53 (TCP).

### Metrics

PowerDNS exposes metrics via API:
```bash
curl -H "X-API-Key: ${PDNS_API_KEY}" \
  http://localhost:8081/api/v1/servers/localhost/statistics
```

### External Monitoring

Set up external checks for:
- NS1 SOA query
- NS2 SOA query
- Authoritative flag presence
- Query response time

## Support

- **Issues**: Create issue in repository
- **Questions**: Check documentation first
- **Emergencies**: Follow rollback procedures

---

**Version**: 1.0
**Last Updated**: 2026-02-14
**Maintainer**: RBX Systems Infrastructure Team
