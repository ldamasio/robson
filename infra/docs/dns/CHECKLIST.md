# DNS Deployment Checklist

## Pre-Deployment Phase

### Discovery
- [ ] Discovery script executed (`./infra/scripts/dns-discovery.sh`)
- [ ] Cluster connectivity verified
- [ ] MetalLB availability determined
- [ ] PostgreSQL existence checked
- [ ] Node count and IPs documented
- [ ] Deployment scenario selected (MetalLB or NodePort)

### Planning
- [ ] NS1 IP address allocated: `__________`
- [ ] NS2 IP address allocated: `__________`
- [ ] strategos.gr web IP confirmed: `__________`
- [ ] Optional subdomain IPs noted:
  - [ ] thalamus.strategos.gr: `__________`
  - [ ] robson.strategos.gr: `__________`

### Secrets Generation
- [ ] PostgreSQL password generated and saved securely
- [ ] PowerDNS API key generated and saved securely
- [ ] Secrets file created: `~/dns-secrets.txt`
- [ ] Secrets file permissions set to 600

### Glue Records
- [ ] rbxsystems.ch zone accessible
- [ ] ns1.rbxsystems.ch A record verified/created → NS1_IP
- [ ] ns2.rbxsystems.ch A record verified/created → NS2_IP
- [ ] Glue records propagated (dig ns1.rbxsystems.ch A)

---

## Configuration Phase

### Base Configuration
- [ ] PostgreSQL secret updated with password
- [ ] PowerDNS secret updated with API key
- [ ] Zone init job updated with strategos.gr IP
- [ ] Optional subdomain IPs configured in zone-init-job.yaml

### MetalLB Scenario (if applicable)
- [ ] metallb-config.yaml updated with NS1/NS2 IPs
- [ ] services.yaml updated with NS1/NS2 IP annotations
- [ ] Network interface verified in L2Advertisement (default: eth0)
- [ ] IPAddressPool CIDR notation correct (/32)

### NodePort Scenario (if applicable)
- [ ] Node hostnames identified
  - [ ] NODE1: `__________`
  - [ ] NODE2: `__________`
- [ ] powerdns-deployment.yaml updated with node selectors
- [ ] firewall-configmap.yaml updated with IPs and hostnames
- [ ] UFW rules applied on NS1 VPS (port 53 TCP/UDP)
- [ ] UFW rules applied on NS2 VPS (port 53 TCP/UDP)
- [ ] iptables PREROUTING rules added (if needed)
- [ ] Firewall rules persisted (netfilter-persistent save)

---

## Deployment Phase

### ArgoCD Application
- [ ] Correct Application file selected:
  - [ ] dns-metallb.yml (if MetalLB)
  - [ ] dns-nodeport.yml (if NodePort)
- [ ] Application applied: `kubectl apply -f <application-file>`
- [ ] ArgoCD sync started
- [ ] Sync status monitored: `argocd app get dns-infrastructure-<scenario>`

### Resource Verification
- [ ] Namespace 'dns' created
- [ ] PostgreSQL StatefulSet deployed
- [ ] PostgreSQL pod running: `kubectl get pods -n dns`
- [ ] PowerDNS NS1 deployment created
- [ ] PowerDNS NS2 deployment created
- [ ] PowerDNS pods running (2+ per NS for MetalLB, 1 per NS for NodePort)
- [ ] Services created:
  - [ ] dns-ns1 (LoadBalancer or NodePort)
  - [ ] dns-ns2 (LoadBalancer or NodePort)
  - [ ] powerdns-api (ClusterIP)
- [ ] EXTERNAL-IP assigned (MetalLB) OR NodePort numbers visible
- [ ] PodDisruptionBudget created

### Zone Initialization
- [ ] Zone init job applied: `kubectl apply -f zone-init-job.yaml`
- [ ] Job completed successfully: `kubectl get jobs -n dns`
- [ ] Job logs reviewed: `kubectl logs -n dns job/zone-init`
- [ ] Zone records verified in PostgreSQL:
  ```bash
  kubectl exec -n dns postgresql-0 -- psql -U pdns_strategos \
    -d strategos_dns -c "SELECT name, type, content FROM records;"
  ```

---

## Testing Phase

### Internal Testing
- [ ] PowerDNS API accessible via port-forward
- [ ] API returns zone data: `curl -H "X-API-Key: $PDNS_API_KEY" http://localhost:8081/api/v1/servers/localhost/zones/strategos.gr`
- [ ] Pod-to-pod DNS resolution works
- [ ] PostgreSQL connectivity confirmed from PowerDNS pods

### External Testing - Basic
- [ ] NS1 responds to SOA query: `dig @${NS1_IP} strategos.gr SOA`
- [ ] NS2 responds to SOA query: `dig @${NS2_IP} strategos.gr SOA`
- [ ] NS records returned: `dig @${NS1_IP} strategos.gr NS`
- [ ] A records returned: `dig @${NS1_IP} www.strategos.gr A`
- [ ] TCP queries work: `dig +tcp @${NS1_IP} strategos.gr SOA`

### External Testing - Security
- [ ] Authoritative flag (AA) present in responses
- [ ] Recursion disabled:
  ```bash
  dig @${NS1_IP} google.com A
  # Should return REFUSED or SERVFAIL
  ```
- [ ] AXFR blocked:
  ```bash
  dig @${NS1_IP} strategos.gr AXFR
  # Should return REFUSED
  ```
- [ ] API not accessible externally (only ClusterIP)

### External Testing - Redundancy
- [ ] Both NS1 and NS2 responding independently
- [ ] Responses consistent between NS1 and NS2
- [ ] UDP and TCP both functional on both nameservers

---

## DNS Delegation Phase

### Glue Records Final Verification
- [ ] `dig ns1.rbxsystems.ch A` returns NS1_IP
- [ ] `dig ns2.rbxsystems.ch A` returns NS2_IP
- [ ] Glue records propagated globally:
  ```bash
  dig @8.8.8.8 ns1.rbxsystems.ch A
  dig @1.1.1.1 ns2.rbxsystems.ch A
  ```

### Registrar Configuration
- [ ] Logged into Intername control panel
- [ ] Current nameservers documented (for rollback)
- [ ] Nameservers updated to:
  - [ ] ns1.rbxsystems.ch
  - [ ] ns2.rbxsystems.ch
- [ ] Changes saved and confirmed
- [ ] Change notification email received

### Propagation Monitoring
- [ ] DNS trace from root working: `dig +trace strategos.gr`
- [ ] Propagation checked via whatsmydns.net
- [ ] Queries from Google DNS work: `dig @8.8.8.8 strategos.gr NS`
- [ ] Queries from Cloudflare DNS work: `dig @1.1.1.1 strategos.gr NS`
- [ ] Waited 1-2 hours for initial propagation
- [ ] Waited 24-48 hours for full global propagation

---

## Post-Deployment Phase

### Monitoring Setup
- [ ] ServiceMonitor created for Prometheus (if available)
- [ ] External monitoring configured (UptimeRobot, StatusCake, etc.)
- [ ] Alerts configured for:
  - [ ] NS1 query failures
  - [ ] NS2 query failures
  - [ ] SOA query failures
  - [ ] Pod crashes
  - [ ] PostgreSQL unavailability

### Backup Configuration
- [ ] PostgreSQL backup CronJob created
- [ ] Backup schedule verified (daily recommended)
- [ ] Backup PVC created and mounted
- [ ] Test backup executed manually
- [ ] Backup restoration tested
- [ ] Backups uploaded to Object Storage (if available)

### Documentation
- [ ] Deployment scenario documented: `__________` (MetalLB or NodePort)
- [ ] NS1 IP documented: `__________`
- [ ] NS2 IP documented: `__________`
- [ ] Node assignments documented (NodePort only):
  - [ ] NS1 on node: `__________`
  - [ ] NS2 on node: `__________`
- [ ] Secrets saved to password manager
- [ ] Passwords file (`~/dns-secrets.txt`) backed up and encrypted
- [ ] Runbook updated with actual deployment values
- [ ] Lessons learned documented

### Security Review
- [ ] Recursion confirmed disabled in production
- [ ] AXFR confirmed blocked in production
- [ ] API confirmed not publicly accessible
- [ ] Firewall rules reviewed (NodePort scenario)
- [ ] Network policies applied (if supported)
- [ ] Pod security standards enforced (baseline minimum)
- [ ] Secrets properly managed (not committed to Git in plain text)

---

## Operational Readiness

### Team Readiness
- [ ] Operations team notified of new DNS infrastructure
- [ ] Runbook shared with team
- [ ] Rollback procedure reviewed and understood
- [ ] Emergency contacts documented
- [ ] On-call person assigned for first 48 hours

### Change Management
- [ ] Change request approved (if required)
- [ ] Stakeholders notified of DNS change
- [ ] Maintenance window scheduled (if required)
- [ ] Rollback plan reviewed and approved
- [ ] Communication plan prepared for issues

### Continuous Improvement
- [ ] Post-deployment review scheduled
- [ ] Metrics baseline captured
- [ ] Performance benchmarks recorded
- [ ] Future improvements identified:
  - [ ] DNSSEC consideration
  - [ ] Anycast consideration
  - [ ] Additional geographic redundancy
  - [ ] Automated zone management

---

## Sign-Off

| Role | Name | Date | Signature |
|------|------|------|-----------|
| Deployment Engineer | __________ | __________ | __________ |
| DNS Administrator | __________ | __________ | __________ |
| Operations Manager | __________ | __________ | __________ |

---

## Critical Reminders

**BEFORE SWITCHING DNS:**
- ✅ Both nameservers responding
- ✅ Glue records verified and propagated
- ✅ Zone records correct
- ✅ Recursion disabled
- ✅ AXFR blocked
- ✅ Monitoring configured
- ✅ Rollback plan ready

**AFTER SWITCHING DNS:**
- ⏰ Monitor closely for 1 hour
- ⏰ Check propagation at 1h, 6h, 24h
- ⏰ Review metrics daily for first week
- ⏰ Schedule post-deployment review

**EMERGENCY:**
- 🚨 If DNS down: Execute Rollback Scenario 2
- 🚨 Revert nameservers at registrar FIRST
- 🚨 Capture all logs before cleanup
- 🚨 Notify stakeholders immediately

---

**Document Version:** 1.0
**Last Updated:** 2026-02-14
**Author:** Claude Code (Planner)
