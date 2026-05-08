# Frontend Deploy via rbx-infra — Implementation Guide

**Date**: 2026-05-08
**Status**: LIVE since 2026-04-25
**Related**: ADR-0030 (stack), ADR-0033 (hosting pivot), ADR-0025 (auth), ADR-0027 (CORS), ADR-0031 (nginx non-root)

---

## Purpose

Canonical end-to-end description of how the Robson SvelteKit frontend reaches production via the `rbx-infra` GitOps pipeline. Operational procedures live in `docs/runbooks/frontend-deploy.md`; this guide explains the architecture, the two-repo split, and the pipeline so an engineer or agent can reason about the system without reading every commit.

---

## Architecture

```
robson repo                      rbx-infra repo                ArgoCD                     k3s cluster
-----------                      --------------                ------                     -----------
apps/frontend/  ───build───►  ghcr.io/ldamasio/                                           Deployment
  Dockerfile        push       robson-frontend-v2:sha-X                                   (nginx:alpine)
  nginx.conf                                                                              ▲
  SvelteKit src                       │                                                   │
                                      │                                                   │
.github/workflows/             apps/prod/robson/                                          Service
  frontend-deploy.yml ─bump──►   robson-frontend-v2-deploy.yml ──watch──► reconcile ──►   ▲
                                 robson-frontend-v2-svc.yml                               │
                                 robson-frontend-v2-ingress.yml                           Ingress (Traefik)
                                                                                           ▲ TLS via cert-manager
                                                                                           │
                                                                                          robson.rbx.ia.br
                                                                                          robson.rbxsystems.ch
```

Two repositories share the deploy responsibility:

| Repo | Owns | Triggers |
|---|---|---|
| `robson` | Source code, Dockerfile, nginx.conf, build pipeline | Code change → CI builds image, then bumps tag in rbx-infra |
| `rbx-infra` | Kubernetes manifests, ArgoCD Application, ingress, namespace | Tag bump → ArgoCD reconciles |

The split mirrors the Robson backend deploy (same pattern as `robsond`) and matches the rule in `rbx-infra/CLAUDE.md`: "Application Repos → rbx-infra → ArgoCD → k3s Cluster".

---

## Components

### In `robson/`

| Path | Purpose |
|---|---|
| `apps/frontend/` | SvelteKit app, Svelte 5 + runes, `@sveltejs/adapter-static` |
| `apps/frontend/Dockerfile` | Multi-stage: pnpm build → copy `build/` into `nginx:alpine` image |
| `apps/frontend/nginx.conf` | Static-file SPA fallback, `/healthz` endpoint, runs as UID 101 |
| `.github/workflows/frontend-deploy.yml` | Build + push image to GHCR, then bump tag in rbx-infra |
| `.github/workflows/frontend-tests.yml` | PR gate (lint, typecheck, unit tests, Playwright) |

### In `rbx-infra/`

| Path | Purpose |
|---|---|
| `apps/prod/robson/robson-frontend-v2-deploy.yml` | Deployment (2 replicas, nginx, non-root UID 101, emptyDir caches) |
| `apps/prod/robson/robson-frontend-v2-svc.yml` | ClusterIP service, port 80 → 8080 |
| `apps/prod/robson/robson-frontend-v2-ingress.yml` | Traefik ingress, dual-host TLS, cert-manager.io annotation |
| `apps/prod/robson/middleware-https.yml` | HTTPS-redirect Traefik middleware |
| `apps/prod/robson/issuer.yml` | cert-manager ClusterIssuer (Let's Encrypt prod) |
| `apps/prod/robson/kustomization.yml` | Aggregates the manifests |
| `gitops/app-of-apps/robson.yml` (or equivalent) | ArgoCD Application pointing at `apps/prod/robson` |

---

## Pipeline

```
push to robson/main (apps/frontend/**)
  │
  ▼
GitHub Actions: frontend-deploy.yml
  ├─ docker build apps/frontend/ ──► ghcr.io/ldamasio/robson-frontend-v2:sha-<short>
  │   build-arg: PUBLIC_ROBSON_API_BASE = vars.PUBLIC_ROBSON_API_BASE_PROD
  ├─ docker push to GHCR
  └─ clone rbx-infra (GITOPS_TOKEN) ──► sed image tag in robson-frontend-v2-deploy.yml ──► commit ──► push origin main
  │
  ▼
ArgoCD reconciles rbx-infra/apps/prod/robson
  ├─ kubectl apply Deployment with new image
  ├─ rolling update (2 replicas)
  └─ readiness gate via /healthz on :8080
  │
  ▼
Live at https://robson.rbx.ia.br + https://robson.rbxsystems.ch
```

### Authentication on the pipeline

| Token | Scope | Where |
|---|---|---|
| `GITHUB_TOKEN` (auto) | Push to GHCR (packages: write) | `permissions:` block of the workflow |
| `GITOPS_TOKEN` (manual) | Push to `rbx-infra` main | `secrets.GITOPS_TOKEN`, scoped to `rbx-infra` repo |
| `PUBLIC_ROBSON_API_BASE_PROD` | Backend URL baked into the bundle | `vars.PUBLIC_ROBSON_API_BASE_PROD` (GitHub variable, public) |

---

## Hosts and TLS

| Host | Purpose | Locale default |
|---|---|---|
| `robson.rbx.ia.br` | Frontend (BR) | `pt-BR` |
| `robson.rbxsystems.ch` | Frontend (CH) | `en` |
| `api.robson.rbx.ia.br` | Backend (`robsond`) | n/a |

DNS is managed via `rbx-infra/bootstrap/ansible/dns-tofu-env.sh` (PowerDNS sovereign 2-VPS, see `project_dns_secrets_pattern`). All three hosts resolve to the cluster ingress IP.

TLS is handled by cert-manager + Let's Encrypt via the `letsencrypt-prod` ClusterIssuer. Each host gets its own Certificate (`robson-frontend-v2-rbx-ia-br-tls`, `robson-frontend-v2-rbxsystems-ch-tls`). HTTP traffic is redirected to HTTPS by the `robson-redirect-https` Traefik middleware.

---

## Auth and CORS

The frontend is fully static (`adapter-static`). It authenticates against `robsond` with a Bearer token issued by the operator (ADR-0025). Token is stored in `sessionStorage` and sent as `Authorization: Bearer <token>` on REST and `?token=<token>` on the SSE stream.

Because the frontend lives at `robson.rbx.ia.br` / `robson.rbxsystems.ch` and the backend at `api.robson.rbx.ia.br`, requests are cross-origin. `robsond` enforces an env-driven CORS allow-list (ADR-0027) that includes both frontend origins. The allow-list is configured in `rbx-infra/apps/prod/robson/robsond-config.yml`.

---

## nginx hardening

The container runs as non-root (UID 101) per ADR-0031. nginx writes its cache and PID file under `/var/cache/nginx` and `/run`, which are root-owned in the image. Two `emptyDir` volumes mount empty filesystems at those paths so the unprivileged user can write. See `docs/runbooks/FRONTEND-NGINX-TROUBLESHOOTING.md` for failure modes.

The deployment manifest pins `runAsUser: 101` explicitly (named-user fallback fails kubelet's `runAsNonRoot` check on some k3s versions) and drops all capabilities.

---

## Verification after a deploy

```bash
# Image landed in rbx-infra
git -C ~/apps/rbx-infra log --oneline -1 apps/prod/robson/robson-frontend-v2-deploy.yml

# ArgoCD synced
kubectl get application robson -n argocd -o jsonpath='{.status.sync.status} {.status.health.status}'

# Pods healthy
kubectl get pods -n robson -l app.kubernetes.io/name=robson-frontend-v2

# Public reachability
curl -I https://robson.rbx.ia.br
curl -I https://robson.rbxsystems.ch
curl -I https://robson.rbx.ia.br/healthz   # 200 ok
```

The login page validates the operator token by calling `GET /health` on `api.robson.rbx.ia.br` with the `Authorization` header. A successful login redirects to `/dashboard`.

---

## Rollback

In order of preference:

1. **ArgoCD UI** — pin the Application to the previous SHA tag. Lowest blast radius; takes effect within one reconcile cycle.
2. **Revert in robson** — revert the offending commit on `main`. CI rebuilds, opens a new bump on rbx-infra, ArgoCD reconciles forward.
3. **Manual bump in rbx-infra** — edit `robson-frontend-v2-deploy.yml` to the previous tag and push. Use only when the GHCR image already exists and the robson CI is unavailable.
4. **`kubectl rollout undo`** — last resort; bypasses GitOps and ArgoCD will re-sync to whatever rbx-infra says, overwriting the rollout. Only useful as a 60-second emergency stop while you commit a real fix.

The image is tagged by short SHA (`sha-<7chars>`) so any historical version is addressable as long as GHCR retains it.

---

## Where the previous plan lived

The original FE-P1 plan (2026-04-23) targeted Contabo Object Storage with `aws s3 sync` and an optional Cloudflare front. ADR-0033 records the pivot to k3s on 2026-04-25. ADR-0030 carries Amendment 2 documenting the supersedence. The historical close-out of the MVP itself is in `docs/implementation/FE-P1-FRONTEND-MVP.md` — its "Infrastructure Gaps" and "EP-008" sections are preserved as audit trail and explicitly marked HISTORICAL.

---

## Pending follow-ups

| Item | Tracked in | Notes |
|---|---|---|
| Drop the `-v2` suffix from image and resource names | FE-P1 close-out | Coordinated PR pair across robson + rbx-infra; cosmetic, no behavior change |
| Migrate Ingress → Gateway API HTTPRoute | rbx-infra ADR-v3-018 deviation, FE-P3 | Cluster-wide pattern; not frontend-specific |
| Locale switcher UI in the header | FE-P2 | Host-based default suffices for now |
| Image registry move to `ghcr.io/rbxrobotica/*` | rbx-infra `docs/CONTAINER-REGISTRY.md` | Frontend image still under `ldamasio/`; deviation acknowledged in `kustomization.yml` |

---

## References

- ADR-0030 Amendment 2 — frontend stack with hosting pivot
- ADR-0033 — hosting pivot Contabo S3 → k3s
- ADR-0025 — Bearer-token auth
- ADR-0027 — CORS allow-list
- ADR-0031 — nginx non-root in k8s
- `docs/runbooks/frontend-deploy.md` — operational runbook
- `docs/runbooks/FRONTEND-NGINX-TROUBLESHOOTING.md` — nginx pod failures
- `docs/implementation/FE-P1-FRONTEND-MVP.md` — historical MVP close-out
- `rbx-infra/CLAUDE.md` — GitOps repo conventions
