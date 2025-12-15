# Registro.br DNS Limitations

## Wildcard Records Not Supported

Registro.br's "DNS Avançado" (Advanced DNS) does **not accept wildcard records**.

Tested and rejected:
- `*` → "Nome do record inválido"
- `*.robson` → "Nome do record inválido"

This is a known limitation of Registro.br's DNS management interface.

---

## Workaround: Explicit A Records

Instead of wildcards, we create individual A records for each subdomain:

### Production (always configured)

```
robson.rbx.ia.br        → 158.220.116.31
app.robson.rbx.ia.br    → 158.220.116.31
api.robson.rbx.ia.br    → 158.220.116.31
```

### Preview Environments (manual, on-demand)

When a branch is selected for UAT/homologação:

```
h-<branch>.robson.rbx.ia.br → 158.220.116.31
```

Example:
```
h-feature-login-sso.robson.rbx.ia.br → 158.220.116.31
```

---

## Alternative: Delegate Subzone

For automatic wildcard support, delegate `robson.rbx.ia.br` to Cloudflare:

1. Create zone in Cloudflare
2. In Registro.br, add NS records:
   ```
   robson NS → cloudflare-ns1
   robson NS → cloudflare-ns2
   ```
3. Configure wildcard in Cloudflare: `*.robson.rbx.ia.br`

See `EXTERNAL_DNS.md` for full automation with external-dns.

---

## Current Records in Registro.br

| Type | Name | Value | Purpose |
|------|------|-------|---------|
| A | `@` | 158.220.116.31 | Apex (rbx.ia.br) |
| A | `tiger` | 158.220.116.31 | K3s server |
| A | `bengal` | 164.68.96.68 | K3s agent |
| A | `pantera` | 149.102.139.33 | K3s agent |
| A | `eagle` | 167.86.92.97 | K3s agent |
| A | `robson` | 158.220.116.31 | Product landing |
| A | `app.robson` | 158.220.116.31 | Frontend |
| A | `api.robson` | 158.220.116.31 | API |
