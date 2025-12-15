# external-dns Provider Options

## Current Strategy: Manual DNS Records

Registro.br does not support wildcard DNS records (`*` or `*.subdomain`), so we use **explicit A records** for each subdomain.

- **Production hosts** (`app.robson`, `api.robson`, etc.) are pre-configured
- **Preview environments** are added manually when a branch is selected for UAT/homologação

See `WILDCARD_GUIDE.md` for the complete DNS configuration.

---

## Future: Automated DNS with external-dns

For full automation of preview environments, we can delegate the `robson.rbx.ia.br` subzone to a provider that supports external-dns.

### Option A: Cloudflare (Recommended)

1. **Create subzone** `robson.rbx.ia.br` in Cloudflare (free tier)
2. **Delegate from Registro.br**:
   - Add NS records: `robson` → Cloudflare nameservers
3. **Configure external-dns** with Cloudflare API token:
   ```yaml
   provider: cloudflare
   domainFilters:
     - robson.rbx.ia.br
   ```
4. **Enable wildcard** `*.robson.rbx.ia.br` in Cloudflare for immediate resolution
5. **Optional**: external-dns can create individual records for each preview

### Option B: AWS Route53

Same delegation approach with Route53 hosted zone:
```yaml
provider: aws
domainFilters:
  - robson.rbx.ia.br
```

### Option C: RFC2136 (TSIG)

If using a DNS server that supports dynamic updates:
```yaml
provider: rfc2136
rfc2136:
  host: <dns-server>
  port: 53
  tsigKeyname: <key>
  tsigSecret: <secret>
```

Note: Registro.br does not support RFC2136/TSIG.

---

## external-dns Helm Values (when ready)

```yaml
# values-external-dns.yaml
provider: cloudflare
domainFilters:
  - robson.rbx.ia.br
env:
  - name: CF_API_TOKEN
    valueFrom:
      secretKeyRef:
        name: cloudflare-api-token
        key: token
policy: sync
txtOwnerId: robson-cluster
```

Store the Cloudflare API token as a SealedSecret:
```bash
kubectl create secret generic cloudflare-api-token \
  --from-literal=token=<your-token> \
  --dry-run=client -o yaml | kubeseal -o yaml > sealed-cf-token.yaml
```

---

## References

- https://kubernetes-sigs.github.io/external-dns/latest/
- https://developers.cloudflare.com/dns/
