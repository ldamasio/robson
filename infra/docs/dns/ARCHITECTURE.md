# Authoritative DNS Architecture for strategos.gr

## Executive Summary

This document describes the architecture for hosting authoritative DNS within the k3s cluster at RBX Systems for the zone `strategos.gr`.

**Domain Status:**
- Domain: `strategos.gr`
- Registrar: Intername
- Configured Nameservers:
  - `ns1.rbxsystems.ch`
  - `ns2.rbxsystems.ch`

**Critical Requirement:** These nameservers MUST respond authoritatively for `strategos.gr` zone.

---

## Architecture Decision Tree

### Phase 1: Discovery

Execute the discovery script to determine the optimal architecture:

```bash
./infra/scripts/dns-discovery.sh
```

This script will check:
1. MetalLB CRDs presence
2. Existing LoadBalancer services
3. Node public IPs
4. Network capabilities

### Phase 2: Conditional Architecture

#### Scenario A: MetalLB Available (PREFERRED)

**Architecture:**
```
┌─────────────────────────────────────────────┐
│           DNS Query (UDP/TCP 53)            │
└──────────────────┬──────────────────────────┘
                   │
        ┌──────────┴──────────┐
        │                     │
   ┌────▼────┐          ┌────▼────┐
   │  ns1.   │          │  ns2.   │
   │ IP1:53  │          │ IP2:53  │
   └────┬────┘          └────┬────┘
        │                     │
   LoadBalancer         LoadBalancer
   (MetalLB L2)         (MetalLB L2)
        │                     │
   ┌────▼────────────────────▼────┐
   │      PowerDNS Pods            │
   │  (2 replicas per NS)          │
   │  Anti-affinity enabled        │
   └───────────────┬───────────────┘
                   │
            ┌──────▼──────┐
            │  PostgreSQL  │
            │  (strategos) │
            └──────────────┘
```

**Components:**
- MetalLB with IPAddressPool containing 2 public IPs
- Two separate LoadBalancer Services (ns1, ns2)
- PowerDNS Authoritative with 2 replicas per service
- PostgreSQL backend for zone storage
- Anti-affinity rules ensuring pod distribution

**Pros:**
- True HA with automatic failover
- Clean IP management
- Easy to scale
- Best practice for DNS

**Cons:**
- Requires MetalLB installation/configuration
- Needs 2 dedicated public IPs

#### Scenario B: NodePort Fallback

**Architecture:**
```
┌─────────────────────────────────────────────┐
│           DNS Query (UDP/TCP 53)            │
└──────────────────┬──────────────────────────┘
                   │
        ┌──────────┴──────────┐
        │                     │
   ┌────▼────┐          ┌────▼────┐
   │  VPS 1  │          │  VPS 2  │
   │ IP1:53  │          │ IP2:53  │
   └────┬────┘          └────┬────┘
        │                     │
   NodePort:30053       NodePort:30053
        │                     │
   ┌────▼─────┐         ┌────▼─────┐
   │  pdns-   │         │  pdns-   │
   │   ns1    │         │   ns2    │
   │ (fixed)  │         │ (fixed)  │
   └────┬─────┘         └────┬─────┘
        │                     │
        └──────────┬──────────┘
                   │
            ┌──────▼──────┐
            │  PostgreSQL  │
            │  (strategos) │
            └──────────────┘
```

**Components:**
- Two Deployments with nodeSelector pinning to specific VPS
- NodePort services on port 30053
- UFW rules opening 53 → 30053 on each VPS
- PowerDNS Authoritative (1 pod per VPS)
- PostgreSQL backend for zone storage

**Pros:**
- No MetalLB dependency
- Works with any Kubernetes cluster
- Direct VPS IP usage

**Cons:**
- Manual nodeSelector management
- Less flexible scaling
- Manual firewall configuration
- Single pod per VPS (acceptable for DNS)

---

## Component Specifications

### PowerDNS Authoritative

**Version:** 4.9.x
**Image:** `powerdns/pdns-auth-49:latest`

**Configuration:**
```yaml
pdns:
  config:
    # Security
    local-address: "0.0.0.0"
    local-port: 53
    setgid: pdns
    setuid: pdns
    guardian: yes

    # Authoritative only
    primary: yes
    secondary: no

    # Security hardening
    disable-axfr: yes
    allow-axfr-ips: ""
    allow-notify-from: ""
    allow-unsigned-notify: no

    # No recursion
    recursor: ""

    # API (ClusterIP only)
    api: yes
    api-key: "${API_KEY}"
    webserver: yes
    webserver-address: 0.0.0.0
    webserver-port: 8081
    webserver-allow-from: 0.0.0.0/0

    # Backend
    launch: gpgsql
    gpgsql-host: "${POSTGRES_HOST}"
    gpgsql-port: 5432
    gpgsql-dbname: "${POSTGRES_DB}"
    gpgsql-user: "${POSTGRES_USER}"
    gpgsql-password: "${POSTGRES_PASSWORD}"
    gpgsql-dnssec: no
```

**Resource Limits:**
```yaml
resources:
  requests:
    memory: "128Mi"
    cpu: "100m"
  limits:
    memory: "256Mi"
    cpu: "500m"
```

**Health Checks:**
```yaml
livenessProbe:
  tcpSocket:
    port: 53
  initialDelaySeconds: 10
  periodSeconds: 10

readinessProbe:
  tcpSocket:
    port: 53
  initialDelaySeconds: 5
  periodSeconds: 5
```

### PostgreSQL

**Option 1: Existing PostgreSQL**
- Create database: `strategos_dns`
- Create user: `pdns_strategos`
- Grant minimal permissions

**Option 2: New PostgreSQL (Bitnami)**
```yaml
helm install postgresql bitnami/postgresql \
  --namespace dns \
  --set auth.database=strategos_dns \
  --set auth.username=pdns_strategos \
  --set auth.password="${SECURE_PASSWORD}" \
  --set primary.persistence.size=5Gi
```

### DNS Zone: strategos.gr

**SOA Record:**
```
strategos.gr.  IN  SOA  ns1.rbxsystems.ch. hostmaster.strategos.gr. (
    2024021401  ; Serial (YYYYMMDDNN)
    7200        ; Refresh (2h)
    3600        ; Retry (1h)
    1209600     ; Expire (2w)
    3600        ; Minimum TTL (1h)
)
```

**NS Records:**
```
strategos.gr.  IN  NS  ns1.rbxsystems.ch.
strategos.gr.  IN  NS  ns2.rbxsystems.ch.
```

**Glue Records (at rbxsystems.ch zone):**
```
ns1.rbxsystems.ch.  IN  A  <IP1>
ns2.rbxsystems.ch.  IN  A  <IP2>
```

**A Records:**
```
strategos.gr.           IN  A  <WEB_SERVER_IP>
www.strategos.gr.       IN  A  <WEB_SERVER_IP>
thalamus.strategos.gr.  IN  A  <THALAMUS_IP>
robson.strategos.gr.    IN  A  <ROBSON_IP>
```

---

## Security Hardening

### 1. PowerDNS Configuration

- [x] `disable-axfr=yes` - Block zone transfers
- [x] `allow-axfr-ips=""` - No exceptions
- [x] `allow-notify-from=""` - Block NOTIFY
- [x] `recursor=""` - No recursion
- [x] API accessible only via ClusterIP (internal)
- [x] API key authentication enabled

### 2. Network Policies

```yaml
apiVersion: networking.k8s.io/v1
kind: NetworkPolicy
metadata:
  name: pdns-network-policy
  namespace: dns
spec:
  podSelector:
    matchLabels:
      app: powerdns
  policyTypes:
    - Ingress
    - Egress
  ingress:
    # Allow DNS queries from anywhere
    - from:
      - namespaceSelector: {}
      ports:
      - protocol: UDP
        port: 53
      - protocol: TCP
        port: 53
    # Allow API access only from same namespace
    - from:
      - podSelector:
          matchLabels:
            role: dns-admin
      ports:
      - protocol: TCP
        port: 8081
  egress:
    # Allow PostgreSQL connection
    - to:
      - podSelector:
          matchLabels:
            app: postgresql
      ports:
      - protocol: TCP
        port: 5432
    # Allow DNS resolution
    - to:
      - namespaceSelector: {}
      ports:
      - protocol: UDP
        port: 53
```

### 3. Firewall Rules (NodePort Scenario)

**VPS 1 (ns1):**
```bash
ufw allow 53/tcp
ufw allow 53/udp
```

**VPS 2 (ns2):**
```bash
ufw allow 53/tcp
ufw allow 53/udp
```

### 4. Pod Security Standards

```yaml
apiVersion: v1
kind: Namespace
metadata:
  name: dns
  labels:
    pod-security.kubernetes.io/enforce: restricted
    pod-security.kubernetes.io/audit: restricted
    pod-security.kubernetes.io/warn: restricted
```

---

## High Availability

### Anti-Affinity Rules

Ensure pods are distributed across nodes:

```yaml
affinity:
  podAntiAffinity:
    requiredDuringSchedulingIgnoredDuringExecution:
      - labelSelector:
          matchExpressions:
            - key: app
              operator: In
              values:
                - powerdns
        topologyKey: kubernetes.io/hostname
```

### PodDisruptionBudget

Prevent simultaneous pod disruptions:

```yaml
apiVersion: policy/v1
kind: PodDisruptionBudget
metadata:
  name: pdns-pdb
  namespace: dns
spec:
  minAvailable: 1
  selector:
    matchLabels:
      app: powerdns
```

---

## Monitoring and Observability

### Metrics

PowerDNS exposes metrics via API:
- Query rate
- Response time
- Cache hit ratio
- Backend query time
- Error rates

**Prometheus Integration:**
```yaml
apiVersion: v1
kind: ServiceMonitor
metadata:
  name: powerdns
  namespace: dns
spec:
  selector:
    matchLabels:
      app: powerdns
  endpoints:
    - port: api
      path: /api/v1/servers/localhost/statistics
```

### Logging

PowerDNS logs to stdout/stderr (captured by Kubernetes):
```bash
kubectl logs -n dns -l app=powerdns --tail=100 -f
```

---

## Disaster Recovery

### Backup Strategy

**PostgreSQL Backups:**
```bash
# Daily backup
kubectl exec -n dns postgresql-0 -- \
  pg_dump -U pdns_strategos strategos_dns > \
  backup-$(date +%Y%m%d).sql

# Upload to Object Storage
s3cmd put backup-$(date +%Y%m%d).sql \
  s3://rbx-backups/dns/
```

### Recovery Procedure

1. Restore PostgreSQL database
2. Verify zone data integrity
3. Restart PowerDNS pods
4. Validate DNS resolution
5. Monitor query logs

---

## Testing Checklist

### Pre-Deployment

- [ ] MetalLB or NodePort decision documented
- [ ] IP addresses allocated
- [ ] PostgreSQL database created
- [ ] Zone data prepared
- [ ] Firewall rules reviewed (NodePort only)

### Post-Deployment

- [ ] DNS pods running
- [ ] PostgreSQL connectivity verified
- [ ] Zone loaded correctly
- [ ] SOA record queryable
- [ ] NS records queryable
- [ ] A records queryable
- [ ] Authoritative flag (AA) present
- [ ] No recursion available
- [ ] AXFR blocked
- [ ] Both nameservers responding
- [ ] TCP and UDP working

### External Validation

- [ ] `dig @IP1 strategos.gr NS`
- [ ] `dig @IP2 strategos.gr NS`
- [ ] `dig @IP1 strategos.gr SOA`
- [ ] `dig @IP2 www.strategos.gr A`
- [ ] `dig +tcp @IP1 strategos.gr ANY`
- [ ] Glue records verified at registrar
- [ ] Public DNS resolution working

---

## Rollback Plan

See [ROLLBACK.md](ROLLBACK.md) for detailed rollback procedures.

**Quick Rollback:**
```bash
# Disable ArgoCD auto-sync
argocd app set dns-infrastructure --auto-sync-policy none

# Delete Application
kubectl delete application dns-infrastructure -n argocd

# Clean up resources
kubectl delete namespace dns
```

---

## Next Steps After Deployment

1. **Verify Glue Records:** Ensure `ns1.rbxsystems.ch` and `ns2.rbxsystems.ch` have A records pointing to allocated IPs
2. **Update Registrar:** Confirm nameservers at Intername are correctly configured
3. **Monitor Propagation:** Use `dig +trace strategos.gr` to verify delegation
4. **Set Up Monitoring:** Configure alerts for DNS query failures
5. **Document IPs:** Record allocated IPs in password manager/runbook
6. **Schedule Backups:** Automate PostgreSQL backup to Object Storage

---

## References

- [PowerDNS Authoritative Documentation](https://doc.powerdns.com/authoritative/)
- [MetalLB Documentation](https://metallb.universe.tf/)
- [DNS Best Practices (RFC 2182)](https://datatracker.ietf.org/doc/html/rfc2182)
- [ADR-0014: Authoritative DNS Architecture](../adr/ADR-0014-authoritative-dns.md)

---

**Document Version:** 1.0
**Last Updated:** 2026-02-14
**Author:** Claude Code (Planner)
**Status:** Ready for Implementation
