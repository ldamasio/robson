# DNS Infrastructure Rollback Runbook

## Overview

This document provides step-by-step procedures for rolling back the authoritative DNS infrastructure for strategos.gr in case of deployment issues or service degradation.

---

## Rollback Scenarios

### Scenario 1: Pre-Production Rollback (Before Public DNS Switch)

**Impact:** None - DNS still pointing to old servers

**Steps:**

1. **Disable ArgoCD Auto-Sync:**
   ```bash
   # For MetalLB deployment
   argocd app set dns-infrastructure-metallb --auto-sync-policy none

   # OR for NodePort deployment
   argocd app set dns-infrastructure-nodeport --auto-sync-policy none
   ```

2. **Delete ArgoCD Application:**
   ```bash
   # This will remove all resources
   kubectl delete application -n argocd dns-infrastructure-metallb
   # OR
   kubectl delete application -n argocd dns-infrastructure-nodeport
   ```

3. **Verify Cleanup:**
   ```bash
   kubectl get all -n dns
   # Should show no resources or pods terminating
   ```

4. **Force Delete Namespace (if stuck):**
   ```bash
   kubectl delete namespace dns --force --grace-period=0
   ```

5. **Clean Up PVCs:**
   ```bash
   kubectl get pvc -n dns
   kubectl delete pvc -n dns --all
   ```

**Result:** Clean slate, can retry deployment from scratch

---

### Scenario 2: Post-Production Rollback (DNS Already Switched)

**Impact:** HIGH - strategos.gr DNS will be down until old servers are restored

**Critical:** This requires immediate action to restore DNS service

**Steps:**

#### Phase 1: Restore Old DNS Configuration (URGENT)

1. **Revert Nameservers at Registrar:**
   - Login to Intername immediately
   - Change strategos.gr nameservers back to old values
   - Document new/changed nameservers first for reference

2. **Verify Old DNS Servers Still Running:**
   ```bash
   dig @<OLD_NS1_IP> strategos.gr SOA
   dig @<OLD_NS2_IP> strategos.gr SOA
   ```

3. **If Old Servers Down:**
   - **EMERGENCY:** Restore old DNS server infrastructure
   - Use backups of old DNS configuration
   - This is critical path - DNS must be restored

#### Phase 2: Disable New DNS Infrastructure

4. **Disable ArgoCD Auto-Sync:**
   ```bash
   argocd app set dns-infrastructure-<scenario> --auto-sync-policy none
   ```

5. **Stop DNS Services (Keep Data):**
   ```bash
   # Scale down deployments to zero
   kubectl scale deployment -n dns powerdns-ns1 --replicas=0
   kubectl scale deployment -n dns powerdns-ns2 --replicas=0

   # Keep PostgreSQL running for investigation
   ```

6. **Remove LoadBalancer/NodePort Services:**
   ```bash
   # This frees up IPs or ports
   kubectl delete svc -n dns dns-ns1 dns-ns2
   ```

#### Phase 3: Investigation

7. **Capture Logs:**
   ```bash
   # PowerDNS logs
   kubectl logs -n dns -l app=powerdns --all-containers=true > /tmp/powerdns-rollback.log

   # PostgreSQL logs
   kubectl logs -n dns postgresql-0 > /tmp/postgresql-rollback.log

   # Events
   kubectl get events -n dns --sort-by='.lastTimestamp' > /tmp/dns-events.log
   ```

8. **Export Zone Data:**
   ```bash
   # Backup current zone state
   kubectl exec -n dns postgresql-0 -- pg_dump -U pdns_strategos strategos_dns > /tmp/strategos-dns-backup.sql
   ```

9. **Identify Root Cause:**
   - Review logs for errors
   - Check DNS query responses
   - Verify network connectivity
   - Check firewall rules (NodePort scenario)

#### Phase 4: Clean Up (After DNS Restored)

10. **Delete Application:**
    ```bash
    kubectl delete application -n argocd dns-infrastructure-<scenario>
    ```

11. **Delete Namespace:**
    ```bash
    kubectl delete namespace dns
    ```

**Result:** Old DNS restored, new infrastructure removed, logs captured for analysis

---

### Scenario 3: Partial Rollback (Keep Infrastructure, Fix Issues)

**Impact:** MEDIUM - DNS may be degraded but not completely down

**Use Case:** Minor issues that can be fixed without full rollback

**Steps:**

1. **Identify Issue:**
   - One nameserver not responding
   - Zone records incorrect
   - Performance issues

2. **For Zone Record Issues:**
   ```bash
   # Re-run zone initialization
   kubectl delete job -n dns zone-init

   # Update zone-init-job.yaml with correct IPs
   kubectl apply -f infra/apps/dns/base/zone-init-job.yaml

   # Watch job
   kubectl logs -n dns -f job/zone-init
   ```

3. **For Service Issues:**
   ```bash
   # Restart specific deployment
   kubectl rollout restart deployment -n dns powerdns-ns1
   # OR
   kubectl rollout restart deployment -n dns powerdns-ns2

   # Watch rollout
   kubectl rollout status deployment -n dns powerdns-ns1
   ```

4. **For Database Issues:**
   ```bash
   # Restart PostgreSQL
   kubectl delete pod -n dns postgresql-0

   # Wait for restart
   kubectl wait --for=condition=ready pod -n dns postgresql-0 --timeout=120s
   ```

5. **Verify Fix:**
   ```bash
   # Test DNS resolution
   dig @${NS1_IP} strategos.gr SOA
   dig @${NS2_IP} strategos.gr SOA
   ```

**Result:** Issues resolved without full rollback

---

## Rollback Decision Matrix

| Situation | Impact | Action | Priority |
|-----------|--------|--------|----------|
| Deployment fails before going live | None | Scenario 1 | Low |
| DNS not resolving after switch | CRITICAL | Scenario 2 | URGENT |
| One nameserver down | Medium | Scenario 3 | High |
| Incorrect zone records | Low-Medium | Scenario 3 | Medium |
| Performance degradation | Medium | Scenario 3 | Medium |
| Security issue discovered | High | Scenario 2 | High |

---

## Emergency Contacts

**During Rollback:**
- Document all actions taken
- Notify stakeholders immediately
- Capture all logs before cleanup
- Identify root cause before retry

**Post-Rollback:**
- Conduct post-mortem
- Update runbooks with lessons learned
- Fix identified issues
- Plan retry with mitigations

---

## Firewall Rollback (NodePort Scenario)

If using NodePort, rollback firewall rules:

**On NS1 VPS:**
```bash
ssh admin@${NS1_IP}
sudo ufw delete allow 53/tcp
sudo ufw delete allow 53/udp
sudo ufw status numbered
exit
```

**On NS2 VPS:**
```bash
ssh admin@${NS2_IP}
sudo ufw delete allow 53/tcp
sudo ufw delete allow 53/udp
sudo ufw status numbered
exit
```

**If iptables rules were added:**
```bash
# List rules
sudo iptables -t nat -L -n -v --line-numbers

# Delete PREROUTING rules (by line number)
sudo iptables -t nat -D PREROUTING <line-number>

# Persist
sudo netfilter-persistent save
```

---

## MetalLB Rollback

If using MetalLB, clean up IP pool:

```bash
# Delete L2Advertisement
kubectl delete l2advertisement -n metallb-system dns-l2-advertisement

# Delete IPAddressPool
kubectl delete ipaddresspool -n metallb-system dns-pool

# IPs are now released and can be used elsewhere
```

---

## Data Recovery

### Recover Zone Data from Backup

If you need to restore zone data:

```bash
# Copy backup into PostgreSQL pod
kubectl cp /tmp/strategos-dns-backup.sql dns/postgresql-0:/tmp/

# Restore
kubectl exec -n dns postgresql-0 -- psql -U pdns_strategos -d strategos_dns -f /tmp/strategos-dns-backup.sql

# Verify
kubectl exec -n dns postgresql-0 -- psql -U pdns_strategos -d strategos_dns -c "SELECT * FROM records;"
```

### Recover from Git

All configuration is in Git:

```bash
# Reset to previous commit
git log --oneline infra/apps/dns/
git checkout <commit-hash> -- infra/apps/dns/

# Reapply
kubectl apply -k infra/apps/dns/overlays/<scenario>/
```

---

## Testing Before Retry

After rollback and fixes, test before production switch:

1. **Deploy to Test Environment:**
   ```bash
   # Use different namespace
   kubectl create namespace dns-test
   # Deploy there first
   ```

2. **Test Extensively:**
   ```bash
   # All tests from MIGRATION-RUNBOOK.md Phase 5-6
   ```

3. **Simulate Load:**
   ```bash
   # Use dnsperf or similar tools
   ```

4. **Verify Glue Records:**
   ```bash
   # Absolutely critical before switching
   ```

---

## Rollback Verification Checklist

After rollback:

- [ ] Old DNS servers responding (if Scenario 2)
- [ ] Registrar nameservers correct
- [ ] strategos.gr resolving publicly
- [ ] New infrastructure cleaned up
- [ ] Logs captured and saved
- [ ] Root cause identified
- [ ] Post-mortem scheduled
- [ ] Stakeholders notified
- [ ] Documentation updated

---

## Prevention for Next Attempt

1. **Use Blue-Green Deployment:**
   - Keep old DNS running during transition
   - Test new DNS with test domains first
   - Switch traffic gradually

2. **Validate Extensively:**
   - Test all DNS record types
   - Verify from multiple locations
   - Load test before production

3. **Monitor Closely:**
   - Set up monitoring before switch
   - Have rollback plan ready
   - Schedule switch during low traffic

4. **Communication:**
   - Notify stakeholders of change window
   - Have backup person available
   - Document all steps taken

---

**Document Version:** 1.0
**Last Updated:** 2026-02-14
**Author:** Claude Code (Planner)
