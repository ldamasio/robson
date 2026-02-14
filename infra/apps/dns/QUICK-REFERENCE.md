# DNS Deployment - Quick Reference

**One-page cheat sheet for experienced operators**

---

## Git Workflow

```bash
# Create feature branch
git checkout -b feat/dns-nodeport-tiger-bengal
git add infra/apps/dns/ infra/docs/
git commit -m "feat(dns): nodeport ns1 tiger ns2 bengal for authoritative dns"
git push -u origin feat/dns-nodeport-tiger-bengal

# Create PR (not merge to main directly)
gh pr create --title "feat(dns): Authoritative DNS for strategos.gr"
```

---

## Remote Deployment (Secure)

```bash
# Copy to master
scp -r infra/apps/dns root@158.220.116.31:/tmp/

# SSH to master
ssh root@158.220.116.31
export KUBECONFIG=/etc/rancher/k3s/k3s.yaml

# Verify cluster
kubectl get nodes -o wide
```

---

## Create Secrets (Imperative - No Git)

```bash
# Generate
POSTGRES_PASSWORD=$(openssl rand -base64 32)
PDNS_API_KEY=$(openssl rand -base64 32)

# Create namespace
kubectl create namespace dns --dry-run=client -o yaml | kubectl apply -f -

# PostgreSQL secret
kubectl -n dns create secret generic postgresql-secret \
  --from-literal=postgres-password="$POSTGRES_PASSWORD" \
  --from-literal=password="$POSTGRES_PASSWORD" \
  --from-literal=username="pdns_strategos" \
  --from-literal=database="strategos_dns" \
  --dry-run=client -o yaml | kubectl apply -f -

# PowerDNS secret
kubectl -n dns create secret generic powerdns-secret \
  --from-literal=api-key="$PDNS_API_KEY" \
  --dry-run=client -o yaml | kubectl apply -f -

# Save to password manager, then:
unset POSTGRES_PASSWORD PDNS_API_KEY
history -c
```

---

## Optional: Update Governance Labels

```bash
CHANGE_ID="dns-init-$(date +%Y%m%d)"
AGENT_ID="human-admin-$(whoami)"

sed -i "s/CHANGE_ID_PLACEHOLDER/$CHANGE_ID/" /tmp/dns/overlays/nodeport/powerdns-deployment.yaml
sed -i "s/AGENT_ID_PLACEHOLDER/$AGENT_ID/" /tmp/dns/overlays/nodeport/powerdns-deployment.yaml
```

---

## Deploy

```bash
# Dry run first
kubectl apply -k /tmp/dns/overlays/nodeport/ --dry-run=client

# Deploy
kubectl apply -k /tmp/dns/overlays/nodeport/

# Watch
kubectl -n dns get pods -w
kubectl -n dns get svc
kubectl -n dns get pods -o wide  # Verify tiger/bengal placement
```

---

## Initialize Zone

```bash
kubectl apply -f /tmp/dns/base/zone-init-job.yaml
kubectl -n dns logs -f job/zone-init
```

---

## Configure iptables Redirect (CRITICAL!)

```bash
# On tiger (NS1)
bash /tmp/dns/overlays/nodeport/IPTABLES-REDIRECT.sh

# On bengal (NS2)
scp /tmp/dns/overlays/nodeport/IPTABLES-REDIRECT.sh root@164.68.96.68:/tmp/
ssh root@164.68.96.68 "bash /tmp/IPTABLES-REDIRECT.sh"
```

---

## Test DNS

```bash
# Basic queries
dig @158.220.116.31 strategos.gr SOA
dig @164.68.96.68 strategos.gr NS

# Authoritative flag check
dig @158.220.116.31 strategos.gr SOA | grep "flags:"
# Should show: aa (authoritative answer)

# Security tests
dig @158.220.116.31 google.com A  # Should fail (no recursion)
dig @158.220.116.31 strategos.gr AXFR  # Should fail (AXFR blocked)

# TCP test
dig +tcp @158.220.116.31 strategos.gr SOA
```

---

## Glue Records (BLOCKER!)

```bash
# Verify
dig @8.8.8.8 ns1.rbxsystems.ch A +short  # Must return: 158.220.116.31
dig @8.8.8.8 ns2.rbxsystems.ch A +short  # Must return: 164.68.96.68

# If empty: Create glue records in rbxsystems.ch zone BEFORE delegation
```

---

## Activate Delegation (FINAL STEP)

**Only after glue records verified:**

1. Login to Intername (strategos.gr registrar)
2. Update nameservers: ns1.rbxsystems.ch, ns2.rbxsystems.ch
3. Wait 24-48h for propagation

```bash
# Monitor
dig +trace strategos.gr
dig @8.8.8.8 strategos.gr NS
```

---

## Rollback

```bash
kubectl delete -k /tmp/dns/overlays/nodeport/

# Remove iptables (tiger)
iptables -t nat -D PREROUTING -p udp --dport 53 -j REDIRECT --to-ports 30053
iptables -t nat -D PREROUTING -p tcp --dport 53 -j REDIRECT --to-ports 30053
netfilter-persistent save

# Remove iptables (bengal)
ssh root@164.68.96.68 "iptables -t nat -D PREROUTING -p udp --dport 53 -j REDIRECT --to-ports 31053; iptables -t nat -D PREROUTING -p tcp --dport 53 -j REDIRECT --to-ports 31053; netfilter-persistent save"
```

---

## Key Files

| File | Purpose |
|------|---------|
| `DEPLOY-GUIDE.md` | Complete deployment guide |
| `AÇÕES-MANUAIS-NECESSÁRIAS.md` | Secret placeholders to fill |
| `README-NODEPORT.md` | NodePort-specific details |
| `DEPLOYMENT-STATUS.md` | Current status and blockers |

---

## Configuration Summary

- **NS1**: tiger @ 158.220.116.31 (NodePort 30053 → 53)
- **NS2**: bengal @ 164.68.96.68 (NodePort 31053 → 53)
- **Database**: PostgreSQL in dns namespace
- **Secrets**: Created imperatively (NOT in Git)
- **Zone**: strategos.gr → ns1/ns2.rbxsystems.ch
