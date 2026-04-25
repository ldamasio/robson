# Frontend Deploy Runbook

**Scope:** apps/frontend-v2 → Contabo S3 (bucket `robson-app`) → public domains `robson.rbx.ia.br` + `robson.rbxsystems.ch`

## Prerequisites (must be satisfied before first run)

### B1 — Contabo bucket `robson-app`

- Create in Contabo Object Storage (EU region).
- Enable website hosting: `index=index.html`, `error=index.html` (SPA fallback).
- Grant public-read on objects.
- Endpoint: `https://eu2.contabostorage.com`

### B2 — GitHub Actions secrets

Repository-level secrets:

- `AWS_ACCESS_KEY_ID` — Contabo S3 access key (deploy-only IAM)
- `AWS_SECRET_ACCESS_KEY` — Contabo S3 secret

Repository-level variables:

- `PUBLIC_ROBSON_API_BASE_PROD` — public URL where robsond is reachable (e.g., `https://api.robson.rbx.ia.br`)

### B3 — DNS CNAMEs (via rbx-infra tofu)

- `robson.rbx.ia.br` CNAME `robson-app.eu2.contabostorage.com`
- `robson.rbxsystems.ch` CNAME `robson-app.eu2.contabostorage.com`

Use `dns-tofu-env.sh` wrapper per project DNS secrets pattern.

### B4 — TLS strategy (decision pending, ADR-0025 open item)

Option A: accept Contabo TLS with domain mismatch warning (MVP-fast)
Option B: Cloudflare free tier in front, TLS via Cloudflare

Until decided, site reachable by IP path but browser TLS warning expected on custom domain.

### B5 — Backend CORS + public reachability

`robsond` must:

- Respond to requests from Origin `https://robson.rbx.ia.br` and `https://robson.rbxsystems.ch`
- Accept Bearer token on REST + `?token=` query param on SSE (EventSource limitation)
- Be reachable from the public domain set in `PUBLIC_ROBSON_API_BASE_PROD`

## How to deploy

```bash
gh workflow run "Frontend Deploy" \
  --ref main \
  -f environment=production \
  -f dry_run=true
```

Inspect the dry run output; if it looks correct, re-run with `-f dry_run=false`.

## Rollback

**Option 1 (recommended):** redeploy previous commit.

```bash
git checkout <previous-commit>
gh workflow run "Frontend Deploy" -f dry_run=false
```

**Option 2 (emergency):** remove bucket contents and point DNS back to legacy host. Requires human access to Contabo console.

## Verification after deploy

```bash
curl -I https://robson.rbx.ia.br           # 200 + text/html
curl -I https://robson.rbxsystems.ch       # 200 + text/html
```

Browser: manually confirm Bearer token login succeeds and `/dashboard` renders without console errors.

## Known gaps (track separately)

- No CDN (Cloudflare deferred per B4)
- No cache invalidation step
- No release tagging (could add once deploy settles)
- Locale switcher UI deferred (host-based locale works today)
