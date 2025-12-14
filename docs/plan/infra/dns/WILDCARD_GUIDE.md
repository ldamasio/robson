# DNS Configuration for rbx.ia.br

## Limitation: Registro.br does not support wildcards

Registro.br's Advanced DNS does not accept wildcard records (`*` or `*.subdomain`).  
As a result, we use **explicit A records** for each subdomain.

---

## Current DNS Strategy

### Gateway IP

All application subdomains point to the **tiger** server (K3s gateway with Istio):

```
158.220.116.31
```

### Production Records (Registro.br)

| Type | Name | Value | Purpose |
|------|------|-------|---------|
| A | `@` (apex) | 158.220.116.31 | `rbx.ia.br` - Company landing |
| A | `robson` | 158.220.116.31 | `robson.rbx.ia.br` - Product landing |
| A | `app.robson` | 158.220.116.31 | `app.robson.rbx.ia.br` - Frontend SPA |
| A | `backend.robson` | 158.220.116.31 | `backend.robson.rbx.ia.br` - API |

### Server Management Records

| Type | Name | Value | Purpose |
|------|------|-------|---------|
| A | `tiger` | 158.220.116.31 | K3s server (gateway) |
| A | `bengal` | 164.68.96.68 | K3s agent |
| A | `pantera` | 149.102.139.33 | K3s agent |
| A | `eagle` | 167.86.92.97 | K3s agent |

---

## Preview Environments (UAT/Homologação)

Since wildcards are not supported, preview environments require **manual DNS registration**.

### When to create a preview DNS record

- Branch is selected for UAT/homologação
- Stakeholders need external access to review features
- QA team needs a stable URL for testing

### How to add a preview environment

1. **Choose the branch** to be promoted to preview
2. **Create DNS record** in Registro.br:

| Type | Name | Value |
|------|------|-------|
| A | `h-<branch-name>.robson` | 158.220.116.31 |

Example for branch `feature/login-sso`:

| Type | Name | Value |
|------|------|-------|
| A | `h-feature-login-sso.robson` | 158.220.116.31 |

3. **Wait for DNS propagation** (usually 1-5 minutes)
4. **Verify**:

```bash
dig +short h-feature-login-sso.robson.rbx.ia.br
# Should return: 158.220.116.31
```

### Naming convention for preview hosts

The ApplicationSet generates hosts using this pattern:

```
h-{{ branch | lowercase | replace "/" "-" | replace "_" "-" }}.robson.rbx.ia.br
```

Examples:
- `feature/dark-mode` → `h-feature-dark-mode.robson.rbx.ia.br`
- `bugfix/auth_fix` → `h-bugfix-auth-fix.robson.rbx.ia.br`
- `release/v2.0` → `h-release-v2-0.robson.rbx.ia.br`

---

## Validation Commands

```bash
# Production hosts
dig +short rbx.ia.br
dig +short robson.rbx.ia.br
dig +short app.robson.rbx.ia.br
dig +short backend.robson.rbx.ia.br

# All should return: 158.220.116.31
```

---

## Future: Delegate to Cloudflare

To enable automatic DNS for all preview environments, consider delegating the `robson.rbx.ia.br` subzone to Cloudflare:

1. Create zone `robson.rbx.ia.br` in Cloudflare (free tier)
2. Add NS records in Registro.br:
   - `robson` NS → Cloudflare nameservers
3. Configure wildcard `*.robson.rbx.ia.br` in Cloudflare
4. Optionally integrate with external-dns for full automation

See `EXTERNAL_DNS.md` for details on external-dns integration.

---

## TLS Certificates

TLS is handled by cert-manager with HTTP-01 challenge:
- Each host gets its own certificate automatically
- Certificates are stored as Kubernetes Secrets
- Renewal is automatic (cert-manager handles it)

The Helm charts already configure Certificate resources for the `host` value.
