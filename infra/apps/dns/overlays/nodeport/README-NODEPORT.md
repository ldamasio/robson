# DNS NodePort Deployment Configuration

## Configuration Summary

**Deployment Scenario**: NodePort with iptables redirect
**Created**: 2026-02-14
**Status**: Ready for deployment

### Node Assignment

| Nameserver | Node | IP | NodePort | Public Port |
|------------|------|-----|----------|-------------|
| NS1 | tiger | 158.220.116.31 | 30053 | 53 (via redirect) |
| NS2 | bengal | 164.68.96.68 | 31053 | 53 (via redirect) |

### Glue Records Required

**CRITICAL**: Before DNS delegation works, these records MUST exist in `rbxsystems.ch` zone:

```dns
ns1.rbxsystems.ch.  IN  A  158.220.116.31
ns2.rbxsystems.ch.  IN  A  164.68.96.68
```

---

## Deployment Steps

### 1. Prerequisites Check

```bash
# Verify nodes exist
kubectl get nodes -o wide | grep -E "tiger|bengal"

# Expected output:
# tiger   Ready    control-plane   ...   158.220.116.31
# bengal  Ready    <none>          ...   164.68.96.68
```

### 2. Generate Secrets

```bash
# PostgreSQL password
POSTGRES_PASSWORD=$(openssl rand -base64 32)
echo "PostgreSQL password: $POSTGRES_PASSWORD"

# PowerDNS API key
PDNS_API_KEY=$(openssl rand -base64 32)
echo "PowerDNS API key: $PDNS_API_KEY"

# Save securely
cat > ~/dns-secrets.txt <<EOF
POSTGRES_PASSWORD=$POSTGRES_PASSWORD
PDNS_API_KEY=$PDNS_API_KEY
EOF
chmod 600 ~/dns-secrets.txt
```

### 3. Update Secret Values

Edit the following files and replace placeholders:

**File**: `infra/apps/dns/base/postgresql.yaml`
- Replace `CHANGEME_POSTGRES_ROOT_PASSWORD` with `$POSTGRES_PASSWORD`
- Replace `CHANGEME_PDNS_PASSWORD` with `$POSTGRES_PASSWORD`

**File**: `infra/apps/dns/base/powerdns-config.yaml`
- Replace `CHANGEME_PDNS_API_KEY` with `$PDNS_API_KEY`

**File**: `infra/apps/dns/base/zone-init-job.yaml`
- Replace `CHANGEME_STRATEGOS_IP` with actual strategos.gr web server IP

### 4. Deploy via kubectl

```bash
# From repository root
cd /home/psyctl/apps/robson

# Apply manifests
kubectl apply -k infra/apps/dns/overlays/nodeport/

# Watch deployment
kubectl get pods -n dns -w
```

**Expected pods:**
- `postgresql-0` (Running)
- `powerdns-ns1-xxxxx` (Running on tiger)
- `powerdns-ns2-xxxxx` (Running on bengal)

### 5. Configure iptables Redirects

**CRITICAL**: DNS will NOT work on port 53 until this step is completed.

#### Option A: Automated Script

Copy and run the script on each node:

```bash
# Copy script to tiger
scp infra/apps/dns/overlays/nodeport/IPTABLES-REDIRECT.sh root@158.220.116.31:/tmp/

# Run on tiger
ssh root@158.220.116.31 "bash /tmp/IPTABLES-REDIRECT.sh"

# Copy script to bengal
scp infra/apps/dns/overlays/nodeport/IPTABLES-REDIRECT.sh root@164.68.96.68:/tmp/

# Run on bengal
ssh root@164.68.96.68 "bash /tmp/IPTABLES-REDIRECT.sh"
```

#### Option B: Manual Configuration

**On tiger (158.220.116.31):**
```bash
ssh root@158.220.116.31

# Add redirect rules
iptables -t nat -A PREROUTING -p udp --dport 53 -j REDIRECT --to-ports 30053
iptables -t nat -A PREROUTING -p tcp --dport 53 -j REDIRECT --to-ports 30053

# Persist rules
apt-get install -y iptables-persistent
netfilter-persistent save

# Verify
iptables -t nat -L PREROUTING -n -v | grep 53
```

**On bengal (164.68.96.68):**
```bash
ssh root@164.68.96.68

# Add redirect rules
iptables -t nat -A PREROUTING -p udp --dport 53 -j REDIRECT --to-ports 31053
iptables -t nat -A PREROUTING -p tcp --dport 53 -j REDIRECT --to-ports 31053

# Persist rules
apt-get install -y iptables-persistent
netfilter-persistent save

# Verify
iptables -t nat -L PREROUTING -n -v | grep 53
```

### 6. Initialize Zone

```bash
# Run zone initialization job
kubectl apply -f infra/apps/dns/base/zone-init-job.yaml

# Watch job logs
kubectl logs -n dns -f job/zone-init

# Expected output: Zone records created successfully
```

### 7. Verify DNS Resolution

```bash
# Test from external machine (not from cluster)

# Test NS1 (tiger)
dig @158.220.116.31 strategos.gr NS
dig @158.220.116.31 strategos.gr SOA
dig @158.220.116.31 www.strategos.gr A

# Test NS2 (bengal)
dig @164.68.96.68 strategos.gr NS
dig @164.68.96.68 strategos.gr SOA
dig @164.68.96.68 www.strategos.gr A

# Verify authoritative flag (should show "aa")
dig @158.220.116.31 strategos.gr SOA | grep "flags:"
# Expected: flags: qr aa rd; QUERY: 1, ANSWER: 1

# Verify recursion is disabled (should fail or return REFUSED)
dig @158.220.116.31 google.com A +short
# Expected: empty or REFUSED
```

### 8. Test TCP and UDP

```bash
# Test TCP explicitly
dig +tcp @158.220.116.31 strategos.gr SOA
dig +tcp @164.68.96.68 strategos.gr SOA

# Both should work
```

### 9. Test AXFR Blocked

```bash
# Attempt zone transfer (should be refused)
dig @158.220.116.31 strategos.gr AXFR

# Expected: Transfer failed or REFUSED
```

---

## Troubleshooting

### Pods Not Scheduling

```bash
# Check node labels
kubectl get nodes --show-labels | grep -E "tiger|bengal"

# Check pod events
kubectl describe pod -n dns powerdns-ns1-xxxxx
kubectl describe pod -n dns powerdns-ns2-xxxxx

# If nodeSelector not matching, verify hostname
kubectl get nodes -o jsonpath='{.items[*].metadata.name}'
```

### DNS Not Resolving on Port 53

```bash
# Check if redirect is active
ssh root@158.220.116.31 "iptables -t nat -L PREROUTING -n -v | grep 53"
ssh root@164.68.96.68 "iptables -t nat -L PREROUTING -n -v | grep 53"

# Test NodePort directly (should work)
dig @158.220.116.31 -p 30053 strategos.gr SOA
dig @164.68.96.68 -p 31053 strategos.gr SOA

# If NodePort works but port 53 doesn't, redirect is missing
```

### PostgreSQL Connection Issues

```bash
# Check PostgreSQL pod
kubectl logs -n dns postgresql-0

# Test connection from PowerDNS pod
kubectl exec -it -n dns powerdns-ns1-xxxxx -- sh
apk add postgresql-client
psql -h postgresql.dns.svc.cluster.local -U pdns_strategos -d strategos_dns
```

### Firewall Blocking Queries

```bash
# Check UFW status on nodes
ssh root@158.220.116.31 "ufw status"
ssh root@164.68.96.68 "ufw status"

# Ensure DNS is allowed (should be allowed by default for OUTPUT)
# If blocked, allow:
ssh root@158.220.116.31 "ufw allow 53/tcp && ufw allow 53/udp"
ssh root@164.68.96.68 "ufw allow 53/tcp && ufw allow 53/udp"
```

---

## Glue Records Setup

### Critical Prerequisite

Before activating DNS delegation for `strategos.gr`, ensure glue records exist.

#### Check Current Status

```bash
dig ns1.rbxsystems.ch A +short
dig ns2.rbxsystems.ch A +short
```

**Expected output:**
```
158.220.116.31
164.68.96.68
```

If empty or wrong IPs, glue records are missing or incorrect.

#### How to Add Glue Records

**Option 1**: If `rbxsystems.ch` is managed by RBX Systems:
1. Add A records to `rbxsystems.ch` zone:
   ```
   ns1  IN  A  158.220.116.31
   ns2  IN  A  164.68.96.68
   ```
2. Ensure `rbxsystems.ch` zone is served by authoritative nameservers

**Option 2**: If `rbxsystems.ch` is at external registrar:
1. Login to registrar control panel
2. Navigate to DNS management for `rbxsystems.ch`
3. Add host records (glue records):
   - Host: `ns1.rbxsystems.ch` → IP: `158.220.116.31`
   - Host: `ns2.rbxsystems.ch` → IP: `164.68.96.68`

**Option 3**: If using registrar nameservers:
1. Create A records at the registrar:
   ```
   ns1.rbxsystems.ch.  3600  IN  A  158.220.116.31
   ns2.rbxsystems.ch.  3600  IN  A  164.68.96.68
   ```

### Verify Global Resolution

```bash
# Test from multiple resolvers
dig @8.8.8.8 ns1.rbxsystems.ch A +short
dig @1.1.1.1 ns1.rbxsystems.ch A +short

# Both should return 158.220.116.31

dig @8.8.8.8 ns2.rbxsystems.ch A +short
dig @1.1.1.1 ns2.rbxsystems.ch A +short

# Both should return 164.68.96.68
```

---

## Monitoring

### Pod Health

```bash
# Check all pods
kubectl get pods -n dns

# Check logs
kubectl logs -n dns -l app=powerdns --tail=100 -f
```

### DNS Query Logs

```bash
# PowerDNS logs DNS queries
kubectl logs -n dns powerdns-ns1-xxxxx | grep "Question"
kubectl logs -n dns powerdns-ns2-xxxxx | grep "Question"
```

### External Monitoring

Set up external monitoring (e.g., UptimeRobot) to check:
- `dig @158.220.116.31 strategos.gr SOA` (every 5 minutes)
- `dig @164.68.96.68 strategos.gr SOA` (every 5 minutes)

Alert on failures.

---

## Rollback

If issues occur:

```bash
# Delete deployment
kubectl delete -k infra/apps/dns/overlays/nodeport/

# Remove iptables rules
ssh root@158.220.116.31 "iptables -t nat -D PREROUTING -p udp --dport 53 -j REDIRECT --to-ports 30053"
ssh root@158.220.116.31 "iptables -t nat -D PREROUTING -p tcp --dport 53 -j REDIRECT --to-ports 30053"
ssh root@164.68.96.68 "iptables -t nat -D PREROUTING -p udp --dport 53 -j REDIRECT --to-ports 31053"
ssh root@164.68.96.68 "iptables -t nat -D PREROUTING -p tcp --dport 53 -j REDIRECT --to-ports 31053"

# Persist removal
ssh root@158.220.116.31 "netfilter-persistent save"
ssh root@164.68.96.68 "netfilter-persistent save"
```

---

## Success Criteria

- [ ] Both PowerDNS pods running and healthy
- [ ] PostgreSQL pod running
- [ ] Zone initialized with correct records
- [ ] iptables redirects active on both nodes
- [ ] DNS resolves on port 53 from external IPs
- [ ] Authoritative flag (AA) present
- [ ] Recursion disabled (external queries refused)
- [ ] AXFR blocked
- [ ] Both TCP and UDP working
- [ ] Glue records verified globally
- [ ] No errors in pod logs

---

**Document Version**: 1.0
**Last Updated**: 2026-02-14
**Configuration**: tiger (NS1) + bengal (NS2) + NodePort + iptables redirect
