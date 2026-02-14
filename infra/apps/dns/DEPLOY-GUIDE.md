# DNS Deployment Guide - Corporate Standards

**Target**: Deploy authoritative DNS for strategos.gr using NodePort on tiger/bengal
**Date**: 2026-02-14
**Deployment Method**: Remote execution on master node (secure, no public 6443)

---

## Pre-Deployment: Git Workflow

### 1. Create Feature Branch (DO NOT push to main)

```bash
# From repository root
cd /home/psyctl/apps/robson

# Create feature branch
git checkout -b feat/dns-nodeport-tiger-bengal

# Stage changes
git add infra/apps/dns/
git add infra/docs/dns/
git add infra/docs/adr/ADR-0014-authoritative-dns-in-cluster.md

# Verify no secrets leaked
git diff --cached | grep -i "changeme"
# Should find CHANGEME placeholders - OK
# Should NOT find actual passwords/keys

git diff --cached | grep -E "password.*[A-Za-z0-9]{20,}|api-key.*[A-Za-z0-9]{20,}"
# Should return empty - no real secrets
```

### 2. Commit with Corporate Standards

**Message** (no em-dashes, concise):
```bash
git commit -m "feat(dns): nodeport ns1 tiger ns2 bengal for authoritative dns

- NS1 pinned to tiger (158.220.116.31) with NodePort 30053
- NS2 pinned to bengal (164.68.96.68) with NodePort 31053
- Service selectors isolate per nameserver
- rbx labels as placeholders for audit correlation
- iptables redirect script for port 53
- docs and runbooks

Co-Authored-By: Claude Sonnet 4.5 <noreply@anthropic.com>"
```

### 3. Push Feature Branch

```bash
git push -u origin feat/dns-nodeport-tiger-bengal
```

### 4. Create Pull Request

```bash
# Via GitHub CLI
gh pr create --title "feat(dns): Authoritative DNS for strategos.gr (NodePort)" \
  --body "## Summary

Deploy authoritative DNS for strategos.gr zone.

**Architecture**: NodePort with iptables redirect
**NS1**: tiger @ 158.220.116.31
**NS2**: bengal @ 164.68.96.68

## Changes
- PowerDNS deployments pinned to specific nodes
- NodePort services (30053, 31053)
- iptables redirect script for port 53
- Complete documentation and runbooks

## Testing
- [ ] Manifests validated (kustomize build)
- [ ] No secrets leaked (verified)
- [ ] Selectors correct (traffic isolation)
- [ ] Deploy tested in cluster
- [ ] DNS queries validated
- [ ] Security tests passed (recursion/AXFR)

## Deployment Notes
- Secrets created via kubectl (not in Git)
- iptables redirect required on tiger/bengal
- Glue records must exist in rbxsystems.ch
- See: infra/apps/dns/DEPLOY-GUIDE.md

🤖 Generated with [Claude Code](https://claude.com/claude-code)"

# Or via GitHub web UI
# https://github.com/ldamasio/robson/pull/new/feat/dns-nodeport-tiger-bengal
```

---

## Deployment: Remote Execution (Secure Method)

### Why Remote Execution?

- ✅ No need to expose port 6443 publicly
- ✅ No SSH tunnel maintenance
- ✅ Direct access to cluster from master
- ✅ Secrets never leave the cluster
- ✅ Corporate security best practice

---

## Step 1: Copy Manifests to Master

```bash
# From local machine
cd /home/psyctl/apps/robson

# Copy DNS manifests to tiger
scp -r infra/apps/dns root@158.220.116.31:/tmp/

# Verify copy
ssh root@158.220.116.31 "ls -la /tmp/dns/overlays/nodeport/"
```

---

## Step 2: Connect to Master and Verify Cluster

```bash
# SSH to tiger (master node)
ssh root@158.220.116.31

# Set kubeconfig (k3s default location)
export KUBECONFIG=/etc/rancher/k3s/k3s.yaml

# Verify cluster access
kubectl get nodes -o wide

# Expected output:
# NAME      STATUS   ROLES                  AGE   VERSION   INTERNAL-IP      EXTERNAL-IP
# tiger     Ready    control-plane,master   ...   v1.x.x    158.220.116.31   <none>
# bengal    Ready    <none>                 ...   v1.x.x    164.68.96.68     <none>
# pantera   Ready    <none>                 ...   v1.x.x    149.102.139.33   <none>
# eagle     Ready    <none>                 ...   v1.x.x    167.86.92.97     <none>

# Verify hostnames
kubectl get nodes -o jsonpath='{.items[*].metadata.name}'
# Should include: tiger bengal
```

---

## Step 3: Create Secrets (Without Committing to Git)

**Corporate Standard**: Secrets created imperatively, never in Git.

```bash
# Generate strong passwords
POSTGRES_PASSWORD=$(openssl rand -base64 32)
PDNS_API_KEY=$(openssl rand -base64 32)

# Save to secure location (password manager, NOT Git)
cat > ~/dns-secrets-$(date +%Y%m%d-%H%M%S).txt <<EOF
# DNS Secrets - $(date)
# KEEP SECURE - STORE IN PASSWORD MANAGER

POSTGRES_PASSWORD=$POSTGRES_PASSWORD
PDNS_API_KEY=$PDNS_API_KEY

# Deployed to: k3s cluster @ tiger
# Namespace: dns
# Date: $(date)
EOF

chmod 600 ~/dns-secrets-*.txt

echo "Secrets saved to: ~/dns-secrets-*.txt"
echo "IMPORTANT: Store in password manager, then delete this file"
```

### Create Kubernetes Secrets

```bash
# Create namespace first
kubectl create namespace dns --dry-run=client -o yaml | kubectl apply -f -

# Create PostgreSQL secret
kubectl -n dns create secret generic postgresql-secret \
  --from-literal=postgres-password="$POSTGRES_PASSWORD" \
  --from-literal=password="$POSTGRES_PASSWORD" \
  --from-literal=username="pdns_strategos" \
  --from-literal=database="strategos_dns" \
  --dry-run=client -o yaml | kubectl apply -f -

# Create PowerDNS secret
kubectl -n dns create secret generic powerdns-secret \
  --from-literal=api-key="$PDNS_API_KEY" \
  --dry-run=client -o yaml | kubectl apply -f -

# Verify secrets created
kubectl -n dns get secrets
# Should show: postgresql-secret, powerdns-secret

# Clear variables from shell history (security)
unset POSTGRES_PASSWORD
unset PDNS_API_KEY
history -c
```

**IMPORTANT**: After storing in password manager, delete the temp file:
```bash
# After saving to password manager:
rm ~/dns-secrets-*.txt
```

---

## Step 4: Update Zone IPs (If Needed)

If strategos.gr IP is known, update zone-init-job:

```bash
# Edit the job manifest
vi /tmp/dns/base/zone-init-job.yaml

# Find line ~143 and update:
# - name: STRATEGOS_IP
#   value: "ACTUAL_IP_HERE"  # Replace with real IP

# Or use sed:
STRATEGOS_IP="203.0.113.10"  # REPLACE WITH REAL IP
sed -i "s/CHANGEME_STRATEGOS_IP/$STRATEGOS_IP/" /tmp/dns/base/zone-init-job.yaml

# Verify
grep STRATEGOS_IP /tmp/dns/base/zone-init-job.yaml
```

---

## Step 5: Update Governance Labels (Optional)

Set change_id and agent_id for audit trail:

```bash
# Set governance labels
CHANGE_ID="dns-init-$(date +%Y%m%d)"
AGENT_ID="human-admin-$(whoami)"

# Update deployment labels
sed -i "s/CHANGE_ID_PLACEHOLDER/$CHANGE_ID/" /tmp/dns/overlays/nodeport/powerdns-deployment.yaml
sed -i "s/AGENT_ID_PLACEHOLDER/$AGENT_ID/" /tmp/dns/overlays/nodeport/powerdns-deployment.yaml

# Verify
grep "rbx\." /tmp/dns/overlays/nodeport/powerdns-deployment.yaml
```

**Or** leave placeholders and update later via kustomize/CI.

---

## Step 6: Deploy via Kustomize

```bash
# Validate manifests first (dry-run)
kubectl apply -k /tmp/dns/overlays/nodeport/ --dry-run=client

# Check for errors
# If OK, proceed with actual deployment:

kubectl apply -k /tmp/dns/overlays/nodeport/

# Expected output:
# namespace/dns created (or unchanged)
# configmap/powerdns-config created
# secret/... configured (from Step 3)
# deployment.apps/powerdns-ns1 created
# deployment.apps/powerdns-ns2 created
# service/dns-ns1 created
# service/dns-ns2 created
# service/powerdns-api created
# statefulset.apps/postgresql created
# poddisruptionbudget.policy/powerdns-pdb created
```

---

## Step 7: Monitor Deployment

```bash
# Watch pods come up
kubectl -n dns get pods -w

# Expected pods:
# NAME                            READY   STATUS    RESTARTS   AGE
# postgresql-0                    1/1     Running   0          30s
# powerdns-ns1-xxxxx-yyyyy        1/1     Running   0          30s
# powerdns-ns2-xxxxx-zzzzz        1/1     Running   0          30s

# Verify pod placement
kubectl -n dns get pods -o wide

# Verify NS1 is on tiger:
# powerdns-ns1-xxxxx   1/1   Running   0   1m   10.42.0.x   tiger   <none>

# Verify NS2 is on bengal:
# powerdns-ns2-xxxxx   1/1   Running   0   1m   10.42.1.x   bengal  <none>

# Check services
kubectl -n dns get svc

# Expected:
# NAME           TYPE        CLUSTER-IP      EXTERNAL-IP   PORT(S)
# dns-ns1        NodePort    10.43.x.x       <none>        53:30053/UDP,53:30053/TCP
# dns-ns2        NodePort    10.43.x.x       <none>        53:31053/UDP,53:31053/TCP
# powerdns-api   ClusterIP   10.43.x.x       <none>        8081/TCP
# postgresql     ClusterIP   10.43.x.x       <none>        5432/TCP

# Check logs
kubectl -n dns logs -l app=powerdns --tail=50
kubectl -n dns logs postgresql-0 --tail=50
```

---

## Step 8: Initialize DNS Zone

```bash
# Apply zone initialization job
kubectl apply -f /tmp/dns/base/zone-init-job.yaml

# Watch job progress
kubectl -n dns get jobs
kubectl -n dns logs -f job/zone-init

# Expected output:
# Waiting for PostgreSQL to be ready...
# PostgreSQL is ready!
# Initializing strategos.gr zone...
# Executing zone initialization SQL...
# Zone initialization complete!
# Verifying zone records...
# (table with records)

# Verify zone was created
kubectl -n dns exec postgresql-0 -- \
  psql -U pdns_strategos -d strategos_dns \
  -c "SELECT name, type, content FROM records ORDER BY type, name;"

# Should show:
# - SOA record
# - NS records (ns1.rbxsystems.ch, ns2.rbxsystems.ch)
# - A records (strategos.gr, www.strategos.gr, etc.)
```

---

## Step 9: Configure iptables Redirect

**CRITICAL**: Without this, DNS only works on NodePort (30053/31053), not port 53.

### On tiger (NS1)

```bash
# Still on tiger, run:
bash /tmp/dns/overlays/nodeport/IPTABLES-REDIRECT.sh

# Expected output:
# Configuring tiger (NS1) - 158.220.116.31
# Redirect: 53 -> 30053
# ✓ Redirect configured for tiger
# Current NAT rules for DNS:
# (iptables output showing redirects)
# ✓ Rules persisted

# Verify redirect
iptables -t nat -L PREROUTING -n -v | grep 53

# Should show rules redirecting port 53 to 30053
```

### On bengal (NS2)

```bash
# Copy script to bengal
scp /tmp/dns/overlays/nodeport/IPTABLES-REDIRECT.sh root@164.68.96.68:/tmp/

# Connect to bengal and run
ssh root@164.68.96.68

# Run script
bash /tmp/IPTABLES-REDIRECT.sh

# Expected output:
# Configuring bengal (NS2) - 164.68.96.68
# Redirect: 53 -> 31053
# ✓ Redirect configured for bengal
# Current NAT rules for DNS:
# (iptables output showing redirects)
# ✓ Rules persisted

# Verify
iptables -t nat -L PREROUTING -n -v | grep 53

# Exit bengal
exit
```

---

## Step 10: Test DNS Resolution

### From tiger (or any external machine)

```bash
# Test NS1 (tiger)
dig @158.220.116.31 strategos.gr SOA
dig @158.220.116.31 strategos.gr NS
dig @158.220.116.31 www.strategos.gr A

# Test NS2 (bengal)
dig @164.68.96.68 strategos.gr SOA
dig @164.68.96.68 strategos.gr NS
dig @164.68.96.68 www.strategos.gr A

# Verify authoritative flag (AA)
dig @158.220.116.31 strategos.gr SOA | grep "flags:"
# Expected: flags: qr aa rd; QUERY: 1, ANSWER: 1
#                     ^^
#                     authoritative answer flag

# Test TCP (should work)
dig +tcp @158.220.116.31 strategos.gr SOA
dig +tcp @164.68.96.68 strategos.gr SOA
```

---

## Step 11: Security Validation

**CRITICAL**: Verify DNS is NOT an open resolver.

```bash
# Test recursion (should fail)
dig @158.220.116.31 google.com A

# Expected: No answer or REFUSED
# Should NOT resolve external domains

# Test AXFR (should fail)
dig @158.220.116.31 strategos.gr AXFR

# Expected: Transfer failed or REFUSED

# Verify API is internal only
curl http://158.220.116.31:8081/api/v1/servers
# Should timeout or refuse (not exposed)

# From inside cluster (should work):
kubectl -n dns port-forward svc/powerdns-api 8081:8081 &
curl http://localhost:8081/api/v1/servers
# Should return API response (internal access OK)
pkill -f "port-forward.*powerdns-api"
```

---

## Step 12: Glue Records Verification

**BLOCKER**: Without glue records, global DNS will NOT work.

### Check Current Status

```bash
# Test from Google DNS
dig @8.8.8.8 ns1.rbxsystems.ch A +short
# Expected: 158.220.116.31

dig @8.8.8.8 ns2.rbxsystems.ch A +short
# Expected: 164.68.96.68

# If empty or wrong IPs, glue records are missing!
```

### Create Glue Records

**Location**: Depends on rbxsystems.ch management.

**Option A - If rbxsystems.ch has authoritative DNS managed by RBX**:
1. Add A records to rbxsystems.ch zone:
   ```
   ns1.rbxsystems.ch.  3600  IN  A  158.220.116.31
   ns2.rbxsystems.ch.  3600  IN  A  164.68.96.68
   ```

**Option B - If rbxsystems.ch is at external registrar**:
1. Login to registrar control panel
2. Navigate to DNS/Host records for rbxsystems.ch
3. Add glue records:
   - Host: `ns1.rbxsystems.ch` → IP: `158.220.116.31`
   - Host: `ns2.rbxsystems.ch` → IP: `164.68.96.68`

**Wait for propagation** (can take minutes to hours):
```bash
# Test from multiple resolvers
dig @8.8.8.8 ns1.rbxsystems.ch A +short
dig @1.1.1.1 ns1.rbxsystems.ch A +short
dig @8.8.4.4 ns1.rbxsystems.ch A +short

# All should return: 158.220.116.31
```

---

## Step 13: Activate DNS Delegation (Final Step)

**ONLY AFTER** glue records are verified and propagated:

1. Login to Intername (strategos.gr registrar)
2. Navigate to DNS settings for strategos.gr
3. Update nameservers:
   - Primary: `ns1.rbxsystems.ch`
   - Secondary: `ns2.rbxsystems.ch`
4. Save changes
5. Wait for propagation (24-48 hours for full global propagation)

### Monitor Propagation

```bash
# Test DNS delegation
dig +trace strategos.gr

# Expected output showing:
# 1. Root servers
# 2. .gr TLD servers
# 3. Delegation to ns1/ns2.rbxsystems.ch
# 4. Final answer from your DNS servers

# Test from public resolvers
dig @8.8.8.8 strategos.gr NS
dig @1.1.1.1 strategos.gr NS

# Check propagation globally
# https://www.whatsmydns.net/#NS/strategos.gr
```

---

## Post-Deployment

### Cleanup

```bash
# On tiger:
# After confirming everything works:

# Delete temp files
rm -rf /tmp/dns/
rm ~/dns-secrets-*.txt  # After storing in password manager

# Clear shell history (security)
history -c
```

### Monitoring Setup

```bash
# Set up external monitoring (UptimeRobot, StatusCake, etc.)
# Monitor:
# - dig @158.220.116.31 strategos.gr SOA (every 5 min)
# - dig @164.68.96.68 strategos.gr SOA (every 5 min)
# Alert on failures
```

### Backup Configuration

```bash
# Schedule PostgreSQL backups
# Create CronJob for daily pg_dump
# Upload to Object Storage

# Example (to be automated):
kubectl -n dns exec postgresql-0 -- \
  pg_dump -U pdns_strategos strategos_dns > \
  /backup/strategos-dns-$(date +%Y%m%d).sql
```

---

## Rollback Procedure

If issues occur:

```bash
# 1. Disable ArgoCD auto-sync (if using ArgoCD)
kubectl -n argocd patch application dns-infrastructure-nodeport \
  --type merge -p '{"spec":{"syncPolicy":{"automated":null}}}'

# 2. Delete deployment
kubectl delete -k /tmp/dns/overlays/nodeport/

# 3. Remove iptables rules
# On tiger:
iptables -t nat -D PREROUTING -p udp --dport 53 -j REDIRECT --to-ports 30053
iptables -t nat -D PREROUTING -p tcp --dport 53 -j REDIRECT --to-ports 30053
netfilter-persistent save

# On bengal:
ssh root@164.68.96.68
iptables -t nat -D PREROUTING -p udp --dport 53 -j REDIRECT --to-ports 31053
iptables -t nat -D PREROUTING -p tcp --dport 53 -j REDIRECT --to-ports 31053
netfilter-persistent save
exit

# 4. Revert nameservers at registrar
# (if DNS delegation was already changed)
```

See also: `infra/docs/dns/ROLLBACK.md`

---

## Success Checklist

- [ ] Feature branch created (not pushed to main)
- [ ] PR created and reviewed
- [ ] Manifests deployed to cluster
- [ ] Pods running on correct nodes (tiger/bengal)
- [ ] Secrets created imperatively (not in Git)
- [ ] Zone initialized successfully
- [ ] iptables redirects configured
- [ ] DNS resolves on port 53 (both NS)
- [ ] Authoritative flag present (AA)
- [ ] Recursion disabled (verified)
- [ ] AXFR blocked (verified)
- [ ] TCP and UDP working
- [ ] Glue records verified globally
- [ ] DNS delegation activated
- [ ] Monitoring configured
- [ ] Backups scheduled
- [ ] Temp files cleaned up
- [ ] Secrets stored in password manager

---

**Document Version**: 1.0
**Last Updated**: 2026-02-14
**Method**: Remote execution (secure, no public 6443)
**Corporate Standard**: ✅ PR workflow, no secrets in Git, imperative secret creation
