# DNS Deployment Status Report

**Date**: 2026-02-14
**Scenario**: NodePort with iptables redirect
**Status**: ✅ **MANIFESTS READY** - Awaiting cluster access for deployment

---

## Configuration Summary

### Infrastructure

| Component | Value |
|-----------|-------|
| **Deployment Scenario** | NodePort (Scenario B) |
| **NS1** | tiger @ 158.220.116.31 (NodePort 30053 → Port 53) |
| **NS2** | bengal @ 164.68.96.68 (NodePort 31053 → Port 53) |
| **Database** | PostgreSQL 16 (StatefulSet in dns namespace) |
| **DNS Software** | PowerDNS Authoritative 4.9 |
| **GitOps** | ArgoCD (manifests ready) |

### Nameserver Configuration

```
ns1.rbxsystems.ch  →  158.220.116.31:53  (tiger + iptables redirect)
ns2.rbxsystems.ch  →  164.68.96.68:53   (bengal + iptables redirect)
```

### Zone Delegation

```
strategos.gr  →  NS ns1.rbxsystems.ch
strategos.gr  →  NS ns2.rbxsystems.ch
```

---

## Manifests Created/Updated

### ✅ Completed Configurations

1. **Deployments**: `infra/apps/dns/overlays/nodeport/powerdns-deployment.yaml`
   - NS1: Fixed to `tiger` with labels `rbx.change_id`, `rbx.agent_id`, `rbx.env`
   - NS2: Fixed to `bengal` with labels `rbx.change_id`, `rbx.agent_id`, `rbx.env`

2. **Services**: `infra/apps/dns/overlays/nodeport/services.yaml`
   - NS1: NodePort 30053 (TCP + UDP)
   - NS2: NodePort 31053 (TCP + UDP)
   - API: ClusterIP (internal only)

3. **Kustomization**: `infra/apps/dns/overlays/nodeport/kustomization.yaml`
   - References base resources
   - Applies nodeport overlay
   - Labels: `deployment-scenario: nodeport`

4. **iptables Script**: `infra/apps/dns/overlays/nodeport/IPTABLES-REDIRECT.sh`
   - Automated redirect configuration
   - Hostname-aware (tiger vs bengal)
   - Persistent via iptables-persistent

5. **Documentation**: `infra/apps/dns/overlays/nodeport/README-NODEPORT.md`
   - Complete deployment guide
   - Troubleshooting procedures
   - Verification tests

### 📝 Pending Configurations

**BEFORE DEPLOYMENT**, update these placeholders:

1. **PostgreSQL Secrets** (`infra/apps/dns/base/postgresql.yaml`):
   ```bash
   # Generate password
   POSTGRES_PASSWORD=$(openssl rand -base64 32)

   # Replace in file:
   - CHANGEME_POSTGRES_ROOT_PASSWORD → $POSTGRES_PASSWORD
   - CHANGEME_PDNS_PASSWORD → $POSTGRES_PASSWORD
   ```

2. **PowerDNS API Key** (`infra/apps/dns/base/powerdns-config.yaml`):
   ```bash
   # Generate API key
   PDNS_API_KEY=$(openssl rand -base64 32)

   # Replace in file:
   - CHANGEME_PDNS_API_KEY → $PDNS_API_KEY
   ```

3. **Zone IPs** (`infra/apps/dns/base/zone-init-job.yaml`):
   ```bash
   # Replace:
   - CHANGEME_STRATEGOS_IP → actual strategos.gr web server IP
   # Optional:
   - THALAMUS_IP → (leave empty if not used)
   - ROBSON_IP → (leave empty if not used)
   ```

---

## Critical Path to Production

### Phase 1: Cluster Access ⚠️ **BLOCKED**

**Current Issue**: No kubectl access to k3s cluster from local environment.

**Resolution Options**:
1. SSH port-forward to master (tiger)
2. Modify kubeconfig to use public IP
3. Execute deployment from master node directly
4. Use ArgoCD UI/API if already deployed

**Required**: Working `kubectl get nodes` before proceeding.

---

### Phase 2: Deploy to Cluster

Once cluster access is established:

```bash
# 1. Update secrets (do this first!)
# Edit files with real passwords/keys

# 2. Apply via kubectl
kubectl apply -k infra/apps/dns/overlays/nodeport/

# 3. Verify pods
kubectl get pods -n dns -w
```

---

### Phase 3: Configure iptables Redirects ⚠️ **CRITICAL**

**Without this step, DNS will NOT work on port 53.**

#### Execute on tiger (158.220.116.31):
```bash
scp infra/apps/dns/overlays/nodeport/IPTABLES-REDIRECT.sh root@158.220.116.31:/tmp/
ssh root@158.220.116.31 "bash /tmp/IPTABLES-REDIRECT.sh"
```

#### Execute on bengal (164.68.96.68):
```bash
scp infra/apps/dns/overlays/nodeport/IPTABLES-REDIRECT.sh root@164.68.96.68:/tmp/
ssh root@164.68.96.68 "bash /tmp/IPTABLES-REDIRECT.sh"
```

**Verification**:
```bash
# Should work on port 53 after redirect:
dig @158.220.116.31 strategos.gr SOA
dig @164.68.96.68 strategos.gr SOA
```

---

### Phase 4: Glue Records ⚠️ **CRITICAL FOR GLOBAL DNS**

**BLOCKER**: Without glue records, `strategos.gr` delegation WILL NOT WORK globally.

#### Required DNS Records in `rbxsystems.ch` Zone

```dns
ns1.rbxsystems.ch.  3600  IN  A  158.220.116.31
ns2.rbxsystems.ch.  3600  IN  A  164.68.96.68
```

#### Verification

```bash
# Test globally
dig @8.8.8.8 ns1.rbxsystems.ch A +short
# Expected: 158.220.116.31

dig @8.8.8.8 ns2.rbxsystems.ch A +short
# Expected: 164.68.96.68

# If empty or wrong, glue records are missing/incorrect
```

#### Where to Add Glue Records

**Depends on rbxsystems.ch management**:

1. **If rbxsystems.ch DNS is managed by RBX Systems**:
   - Add A records to rbxsystems.ch zone
   - Ensure zone is served by authoritative nameservers

2. **If rbxsystems.ch is at external registrar**:
   - Login to registrar control panel
   - Add host/glue records for ns1 and ns2

3. **If rbxsystems.ch doesn't exist or no DNS**:
   - **CRITICAL**: Must create rbxsystems.ch DNS first
   - Or use different nameserver names under existing domain

**Action Required**: Confirm rbxsystems.ch DNS configuration before activating strategos.gr delegation.

---

### Phase 5: DNS Delegation at Registrar

**After** glue records are verified:

1. Login to Intername (strategos.gr registrar)
2. Update nameservers to:
   - `ns1.rbxsystems.ch`
   - `ns2.rbxsystems.ch`
3. Save changes
4. Wait for propagation (24-48 hours)

---

## Security Validation Checklist

Before going live:

- [ ] Recursion disabled (test: `dig @158.220.116.31 google.com A` → REFUSED)
- [ ] AXFR blocked (test: `dig @158.220.116.31 strategos.gr AXFR` → REFUSED)
- [ ] API not publicly accessible (ClusterIP only)
- [ ] Authoritative flag present (test: `dig @158.220.116.31 strategos.gr SOA` → `aa` flag)
- [ ] Both TCP and UDP working
- [ ] iptables rules persisted (survive reboot)
- [ ] Pod security contexts applied
- [ ] PostgreSQL password secured

---

## Test Plan

### Internal Tests (After Deployment)

```bash
# 1. Pods running
kubectl get pods -n dns
# Expected: postgresql-0, powerdns-ns1-xxx, powerdns-ns2-xxx (all Running)

# 2. Services created
kubectl get svc -n dns
# Expected: dns-ns1 (NodePort 30053), dns-ns2 (NodePort 31053), powerdns-api (ClusterIP)

# 3. Zone initialized
kubectl logs -n dns job/zone-init
# Expected: "Zone initialization complete!"
```

### External Tests (After iptables + Glue Records)

```bash
# 1. Direct nameserver queries
dig @158.220.116.31 strategos.gr NS
dig @164.68.96.68 strategos.gr NS
# Expected: ns1.rbxsystems.ch, ns2.rbxsystems.ch

# 2. SOA queries
dig @158.220.116.31 strategos.gr SOA
dig @164.68.96.68 strategos.gr SOA
# Expected: SOA record with serial number

# 3. A record queries
dig @158.220.116.31 www.strategos.gr A
dig @164.68.96.68 www.strategos.gr A
# Expected: Configured IP address

# 4. Authoritative flag check
dig @158.220.116.31 strategos.gr SOA | grep "flags:"
# Expected: "flags: qr aa rd" (aa = authoritative answer)

# 5. Recursion check (should fail)
dig @158.220.116.31 google.com A +short
# Expected: Empty or REFUSED

# 6. AXFR check (should fail)
dig @158.220.116.31 strategos.gr AXFR
# Expected: Transfer failed or REFUSED

# 7. TCP test
dig +tcp @158.220.116.31 strategos.gr SOA
# Expected: Same as UDP
```

### Global DNS Tests (After Registrar Update)

```bash
# 1. DNS trace from root
dig +trace strategos.gr
# Expected: Delegation chain ending at ns1/ns2.rbxsystems.ch

# 2. Public resolver tests
dig @8.8.8.8 strategos.gr NS
dig @1.1.1.1 strategos.gr NS
# Expected: ns1.rbxsystems.ch, ns2.rbxsystems.ch

# 3. Propagation check
# https://www.whatsmydns.net/#NS/strategos.gr
# Expected: Consistent results globally
```

---

## Risks and Mitigations

| Risk | Impact | Mitigation | Status |
|------|--------|------------|--------|
| No cluster access | BLOCKER | Establish SSH tunnel or direct access | ⚠️ **ACTIVE** |
| Glue records missing | CRITICAL | Verify rbxsystems.ch DNS before delegation | ⚠️ **TODO** |
| iptables not persisted | HIGH | Use iptables-persistent or systemd | ✅ Scripted |
| Secrets in plaintext | MEDIUM | Use SealedSecrets/SOPS (future improvement) | ⚠️ TODO |
| Single database instance | MEDIUM | PostgreSQL HA (future improvement) | ⚠️ TODO |
| Node failure | MEDIUM | DNS served by 2 nodes (acceptable HA) | ✅ Designed |
| Zone corruption | LOW | Daily PostgreSQL backups | ⚠️ TODO |

---

## Next Steps

### Immediate (Before Deployment)

1. **Establish cluster access**
   - Choose access method (SSH tunnel, modified kubeconfig, or remote execution)
   - Verify `kubectl get nodes` works

2. **Generate and update secrets**
   - PostgreSQL password
   - PowerDNS API key
   - Zone IP addresses

3. **Verify rbxsystems.ch DNS status**
   - Confirm where rbxsystems.ch is hosted
   - Plan glue record creation

### Deployment Day

4. **Deploy DNS infrastructure**
   ```bash
   kubectl apply -k infra/apps/dns/overlays/nodeport/
   ```

5. **Configure iptables redirects**
   - Run script on tiger and bengal
   - Verify persistence

6. **Test DNS resolution**
   - Internal tests (pods, services)
   - External tests (dig queries)

### Post-Deployment

7. **Add glue records to rbxsystems.ch**
8. **Update strategos.gr delegation at Intername**
9. **Monitor propagation**
10. **Set up external monitoring (UptimeRobot, etc.)**
11. **Configure PostgreSQL backups**

---

## Files Modified in This Configuration

```
infra/apps/dns/overlays/nodeport/
├── powerdns-deployment.yaml        ✅ Updated (tiger + bengal)
├── services.yaml                   ✅ Updated (NodePort 31053)
├── kustomization.yaml              ✅ Ready
├── firewall-configmap.yaml         ✅ Existing
├── IPTABLES-REDIRECT.sh            ✅ Created (executable)
└── README-NODEPORT.md              ✅ Created (deployment guide)

infra/apps/dns/
└── DEPLOYMENT-STATUS.md            ✅ This file
```

---

## Support and Troubleshooting

**Documentation**:
- Deployment guide: `infra/apps/dns/overlays/nodeport/README-NODEPORT.md`
- Migration runbook: `infra/docs/dns/MIGRATION-RUNBOOK.md`
- Rollback procedures: `infra/docs/dns/ROLLBACK.md`
- Architecture: `infra/docs/dns/ARCHITECTURE.md`

**Logs**:
```bash
kubectl logs -n dns -l app=powerdns --tail=100 -f
kubectl logs -n dns postgresql-0 --tail=100 -f
kubectl get events -n dns --sort-by='.lastTimestamp'
```

---

## Conclusion

**Status**: ✅ Manifests ready, ⚠️ Awaiting cluster access

**Blockers**:
1. Cluster access (kubectl connectivity)
2. Glue records verification (rbxsystems.ch DNS)

**Estimated Time to Production**:
- With cluster access: 2-3 hours (deployment + iptables + testing)
- With glue records ready: Immediate DNS activation possible
- Global propagation: 24-48 hours after registrar update

---

**Configuration Version**: 1.0
**Last Updated**: 2026-02-14
**Configured By**: GLM Executor (Claude Sonnet 4.5)
**Ready for**: Production Deployment (pending access)
