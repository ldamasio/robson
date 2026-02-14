# DNS Migration Runbook: strategos.gr

## Pre-Deployment Checklist

- [ ] Discovery script executed (`infra/scripts/dns-discovery.sh`)
- [ ] Deployment scenario decided (MetalLB or NodePort)
- [ ] Public IPs allocated for ns1 and ns2
- [ ] PostgreSQL password generated
- [ ] PowerDNS API key generated
- [ ] strategos.gr web server IP confirmed
- [ ] Glue records verified at rbxsystems.ch zone
- [ ] Backup of current DNS configuration taken

---

## Deployment Steps

### Phase 1: Discovery and Planning

1. **Run Discovery Script:**
   ```bash
   cd /home/psyctl/apps/robson
   ./infra/scripts/dns-discovery.sh
   ```

2. **Document Results:**
   - Record recommended scenario (A or B)
   - Note detected public IPs
   - Check PostgreSQL availability

3. **Generate Secrets:**
   ```bash
   # PostgreSQL password
   export POSTGRES_PASSWORD=$(openssl rand -base64 32)
   echo "PostgreSQL Password: ${POSTGRES_PASSWORD}" >> ~/dns-secrets.txt

   # PowerDNS API key
   export PDNS_API_KEY=$(openssl rand -base64 32)
   echo "PowerDNS API Key: ${PDNS_API_KEY}" >> ~/dns-secrets.txt

   # Secure the file
   chmod 600 ~/dns-secrets.txt
   ```

4. **Determine IP Addresses:**
   ```bash
   # Record these values
   export NS1_IP="IP_FOR_NS1"
   export NS2_IP="IP_FOR_NS2"
   export STRATEGOS_IP="IP_FOR_STRATEGOS_WEB"
   ```

---

### Phase 2: Scenario-Specific Configuration

#### Scenario A: MetalLB

1. **Update MetalLB Configuration:**
   ```bash
   cd infra/apps/dns/overlays/metallb

   # Edit metallb-config.yaml
   sed -i "s/NS1_IP_PLACEHOLDER/${NS1_IP}/g" metallb-config.yaml
   sed -i "s/NS2_IP_PLACEHOLDER/${NS2_IP}/g" metallb-config.yaml

   # Edit services.yaml
   sed -i "s/NS1_IP_PLACEHOLDER/${NS1_IP}/g" services.yaml
   sed -i "s/NS2_IP_PLACEHOLDER/${NS2_IP}/g" services.yaml
   ```

2. **Update Secrets:**
   ```bash
   cd ../../base

   # Update PostgreSQL secret
   sed -i "s/CHANGEME_POSTGRES_ROOT_PASSWORD/${POSTGRES_PASSWORD}/g" postgresql.yaml
   sed -i "s/CHANGEME_PDNS_PASSWORD/${POSTGRES_PASSWORD}/g" postgresql.yaml

   # Update PowerDNS secret
   sed -i "s/CHANGEME_PDNS_API_KEY/${PDNS_API_KEY}/g" powerdns-config.yaml

   # Update zone IPs
   sed -i "s/CHANGEME_STRATEGOS_IP/${STRATEGOS_IP}/g" zone-init-job.yaml
   ```

3. **Deploy via ArgoCD:**
   ```bash
   # Apply the Application
   kubectl apply -f infra/k8s/gitops/applications/dns-metallb.yml

   # Watch deployment
   watch argocd app get dns-infrastructure-metallb
   ```

4. **Skip to Phase 3**

#### Scenario B: NodePort

1. **Get Node Hostnames:**
   ```bash
   # List nodes to get actual hostnames
   kubectl get nodes -o custom-columns=NAME:.metadata.name,INTERNAL-IP:.status.addresses[0].address

   # Record the hostnames
   export NODE1_HOSTNAME="<first-node-name>"
   export NODE2_HOSTNAME="<second-node-name>"
   ```

2. **Update NodePort Configuration:**
   ```bash
   cd infra/apps/dns/overlays/nodeport

   # Update deployment with node selectors
   sed -i "s/NODE1_HOSTNAME_PLACEHOLDER/${NODE1_HOSTNAME}/g" powerdns-deployment.yaml
   sed -i "s/NODE2_HOSTNAME_PLACEHOLDER/${NODE2_HOSTNAME}/g" powerdns-deployment.yaml

   # Update firewall documentation
   sed -i "s/NODE1_HOSTNAME_PLACEHOLDER/${NODE1_HOSTNAME}/g" firewall-configmap.yaml
   sed -i "s/NODE2_HOSTNAME_PLACEHOLDER/${NODE2_HOSTNAME}/g" firewall-configmap.yaml
   sed -i "s/NODE1_IP_PLACEHOLDER/${NS1_IP}/g" firewall-configmap.yaml
   sed -i "s/NODE2_IP_PLACEHOLDER/${NS2_IP}/g" firewall-configmap.yaml
   ```

3. **Update Secrets:**
   ```bash
   cd ../../base

   # Update PostgreSQL secret
   sed -i "s/CHANGEME_POSTGRES_ROOT_PASSWORD/${POSTGRES_PASSWORD}/g" postgresql.yaml
   sed -i "s/CHANGEME_PDNS_PASSWORD/${POSTGRES_PASSWORD}/g" postgresql.yaml

   # Update PowerDNS secret
   sed -i "s/CHANGEME_PDNS_API_KEY/${PDNS_API_KEY}/g" powerdns-config.yaml

   # Update zone IPs
   sed -i "s/CHANGEME_STRATEGOS_IP/${STRATEGOS_IP}/g" zone-init-job.yaml
   ```

4. **Configure Firewalls on VPS Nodes:**
   ```bash
   # SSH to first node
   ssh admin@${NS1_IP}
   sudo ufw allow 53/tcp comment 'DNS TCP for ns1.rbxsystems.ch'
   sudo ufw allow 53/udp comment 'DNS UDP for ns1.rbxsystems.ch'
   sudo ufw status numbered
   exit

   # SSH to second node
   ssh admin@${NS2_IP}
   sudo ufw allow 53/tcp comment 'DNS TCP for ns2.rbxsystems.ch'
   sudo ufw allow 53/udp comment 'DNS UDP for ns2.rbxsystems.ch'
   sudo ufw status numbered
   exit
   ```

5. **Deploy via ArgoCD:**
   ```bash
   # Apply the Application
   kubectl apply -f infra/k8s/gitops/applications/dns-nodeport.yml

   # Watch deployment
   watch argocd app get dns-infrastructure-nodeport
   ```

---

### Phase 3: Verify Deployment

1. **Check Pod Status:**
   ```bash
   kubectl get pods -n dns

   # Should see:
   # - postgresql-0 (Running)
   # - powerdns-ns1-xxx (Running, 1-2 replicas)
   # - powerdns-ns2-xxx (Running, 1-2 replicas)
   ```

2. **Check Services:**
   ```bash
   kubectl get svc -n dns

   # MetalLB: Look for EXTERNAL-IP on dns-ns1 and dns-ns2
   # NodePort: Look for NodePort numbers (30053, 30054)
   ```

3. **Check Logs:**
   ```bash
   # PostgreSQL
   kubectl logs -n dns postgresql-0 --tail=50

   # PowerDNS NS1
   kubectl logs -n dns -l nameserver=ns1 --tail=50

   # PowerDNS NS2
   kubectl logs -n dns -l nameserver=ns2 --tail=50
   ```

---

### Phase 4: Initialize Zone

1. **Run Zone Initialization Job:**
   ```bash
   kubectl apply -f infra/apps/dns/base/zone-init-job.yaml

   # Watch job progress
   kubectl logs -n dns -f job/zone-init
   ```

2. **Verify Zone Records:**
   ```bash
   # Connect to PostgreSQL
   kubectl exec -n dns postgresql-0 -- psql -U pdns_strategos -d strategos_dns -c \
     "SELECT name, type, content FROM records WHERE domain_id = (SELECT id FROM domains WHERE name = 'strategos.gr') ORDER BY type, name;"
   ```

---

### Phase 5: Internal Testing

1. **Test from PowerDNS API:**
   ```bash
   # Port-forward to API
   kubectl port-forward -n dns svc/powerdns-api 8081:8081

   # Query API (in another terminal)
   curl -H "X-API-Key: ${PDNS_API_KEY}" \
     http://localhost:8081/api/v1/servers/localhost/zones/strategos.gr
   ```

2. **Test DNS Resolution (Internal):**
   ```bash
   # Get pod IPs
   kubectl get pods -n dns -o wide

   # Test from inside cluster
   kubectl run -it --rm dns-test --image=alpine --restart=Never -- sh
   apk add bind-tools
   dig @<powerdns-pod-ip> strategos.gr SOA
   dig @<powerdns-pod-ip> strategos.gr NS
   dig @<powerdns-pod-ip> www.strategos.gr A
   exit
   ```

---

### Phase 6: External Testing

1. **Test DNS from External Machine:**
   ```bash
   # Test NS1
   dig @${NS1_IP} strategos.gr SOA
   dig @${NS1_IP} strategos.gr NS
   dig @${NS1_IP} www.strategos.gr A
   dig +tcp @${NS1_IP} strategos.gr ANY

   # Test NS2
   dig @${NS2_IP} strategos.gr SOA
   dig @${NS2_IP} strategos.gr NS
   dig @${NS2_IP} www.strategos.gr A
   dig +tcp @${NS2_IP} strategos.gr ANY
   ```

2. **Verify Authoritative Flag:**
   ```bash
   # Look for "aa" flag in response (authoritative answer)
   dig @${NS1_IP} strategos.gr SOA | grep "flags:"
   # Should show: flags: qr aa rd; QUERY: 1, ANSWER: 1
   ```

3. **Test Recursion Disabled:**
   ```bash
   # Query external domain (should fail)
   dig @${NS1_IP} google.com A
   # Should return REFUSED or no answer
   ```

4. **Test AXFR Blocked:**
   ```bash
   # Attempt zone transfer (should fail)
   dig @${NS1_IP} strategos.gr AXFR
   # Should return REFUSED
   ```

---

### Phase 7: Glue Records Verification

1. **Check rbxsystems.ch Zone:**
   ```bash
   # Verify glue records exist
   dig ns1.rbxsystems.ch A
   dig ns2.rbxsystems.ch A

   # Should return:
   # ns1.rbxsystems.ch  IN  A  ${NS1_IP}
   # ns2.rbxsystems.ch  IN  A  ${NS2_IP}
   ```

2. **If Glue Records Missing:**
   - Contact rbxsystems.ch zone administrator
   - Add A records:
     ```
     ns1.rbxsystems.ch.  IN  A  ${NS1_IP}
     ns2.rbxsystems.ch.  IN  A  ${NS2_IP}
     ```

---

### Phase 8: Registrar Verification

1. **Verify Nameservers at Intername:**
   - Login to Intername control panel
   - Navigate to strategos.gr domain
   - Verify nameservers are set to:
     - `ns1.rbxsystems.ch`
     - `ns2.rbxsystems.ch`

2. **Test DNS Delegation:**
   ```bash
   # Full trace from root
   dig +trace strategos.gr

   # Should show:
   # 1. Root servers
   # 2. .gr TLD servers
   # 3. Delegation to ns1/ns2.rbxsystems.ch
   # 4. Final answer from your servers
   ```

---

### Phase 9: Public Propagation

1. **Check Global DNS Propagation:**
   ```bash
   # Use online tools or command line
   # https://www.whatsmydns.net/#NS/strategos.gr

   # From different locations
   dig @8.8.8.8 strategos.gr NS
   dig @1.1.1.1 strategos.gr NS
   ```

2. **Monitor TTL Expiry:**
   - Old DNS records may be cached for up to TTL duration
   - Typical TTL: 3600 seconds (1 hour)
   - Full propagation: 24-48 hours

---

### Phase 10: Monitoring Setup

1. **Configure DNS Monitoring:**
   ```bash
   # Add ServiceMonitor for Prometheus (if available)
   kubectl apply -f - <<EOF
   apiVersion: monitoring.coreos.com/v1
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
   EOF
   ```

2. **Set Up External Monitoring:**
   - Use service like UptimeRobot or StatusCake
   - Monitor:
     - `dig @${NS1_IP} strategos.gr SOA`
     - `dig @${NS2_IP} strategos.gr SOA`
   - Alert on failures

---

## Post-Deployment Tasks

1. **Document Deployment:**
   - Record NS1 IP: ${NS1_IP}
   - Record NS2 IP: ${NS2_IP}
   - Record deployment scenario (MetalLB/NodePort)
   - Save passwords to password manager

2. **Schedule Backups:**
   ```bash
   # Create backup CronJob
   kubectl apply -f - <<EOF
   apiVersion: batch/v1
   kind: CronJob
   metadata:
     name: postgresql-backup
     namespace: dns
   spec:
     schedule: "0 2 * * *"  # Daily at 2 AM
     jobTemplate:
       spec:
         template:
           spec:
             containers:
             - name: backup
               image: postgres:16-alpine
               command:
               - /bin/sh
               - -c
               - |
                 BACKUP_FILE="/backup/strategos-dns-\$(date +%Y%m%d-%H%M%S).sql"
                 pg_dump -h postgresql.dns.svc.cluster.local -U pdns_strategos -d strategos_dns > "\${BACKUP_FILE}"
                 echo "Backup saved to \${BACKUP_FILE}"
               env:
               - name: PGPASSWORD
                 valueFrom:
                   secretKeyRef:
                     name: postgresql-secret
                     key: password
               volumeMounts:
               - name: backup
                 mountPath: /backup
             restartPolicy: OnFailure
             volumes:
             - name: backup
               persistentVolumeClaim:
                 claimName: dns-backup-pvc
   EOF
   ```

3. **Review Security:**
   - Confirm recursion disabled
   - Confirm AXFR blocked
   - Confirm API only accessible internally
   - Review firewall rules

4. **Update Documentation:**
   - Add to internal wiki/runbook
   - Document any deviations from plan
   - Record lessons learned

---

## Troubleshooting

### Pods Not Starting

**Check Events:**
```bash
kubectl get events -n dns --sort-by='.lastTimestamp'
```

**Check Logs:**
```bash
kubectl logs -n dns <pod-name>
```

**Common Issues:**
- PostgreSQL not ready: Wait for PostgreSQL to start first
- Missing secrets: Verify secrets are created correctly
- Resource constraints: Check node resources

### DNS Not Resolving

**Check Service:**
```bash
kubectl get svc -n dns
# Verify EXTERNAL-IP (MetalLB) or NodePort assigned
```

**Test Internal Resolution:**
```bash
kubectl exec -n dns <powerdns-pod> -- dig @localhost strategos.gr SOA
```

**Check Firewall (NodePort):**
```bash
# On VPS node
sudo ufw status
sudo netstat -tulpn | grep :53
```

### Zone Not Loading

**Check PostgreSQL Connection:**
```bash
kubectl exec -n dns postgresql-0 -- psql -U pdns_strategos -d strategos_dns -c "SELECT * FROM domains;"
```

**Re-run Zone Init Job:**
```bash
kubectl delete job -n dns zone-init
kubectl apply -f infra/apps/dns/base/zone-init-job.yaml
kubectl logs -n dns -f job/zone-init
```

---

## Rollback Procedure

See [ROLLBACK.md](ROLLBACK.md) for detailed rollback steps.

**Quick Rollback:**
```bash
# Disable auto-sync
argocd app set dns-infrastructure-<scenario> --auto-sync-policy none

# Delete Application
kubectl delete application -n argocd dns-infrastructure-<scenario>

# Clean up namespace
kubectl delete namespace dns

# Revert registrar nameservers (if changed)
```

---

## Success Criteria

- [ ] PowerDNS pods running (2+ replicas per NS)
- [ ] PostgreSQL running and accessible
- [ ] Zone loaded with all records
- [ ] External DNS queries return correct answers
- [ ] Authoritative flag (AA) present in responses
- [ ] Recursion disabled (REFUSED for external queries)
- [ ] AXFR blocked (REFUSED for zone transfers)
- [ ] Both NS1 and NS2 responding
- [ ] TCP and UDP working
- [ ] Glue records verified
- [ ] Public propagation confirmed
- [ ] Monitoring configured

---

**Document Version:** 1.0
**Last Updated:** 2026-02-14
**Author:** Claude Code (Planner)
