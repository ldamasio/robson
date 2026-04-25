# Frontend Deploy Runbook (k3s in-cluster)

**Scope:** apps/frontend → ghcr.io/ldamasio/robson-frontend-v2
→ k3s rbx-infra cluster → public domains robson.rbx.ia.br
(pt-BR) + robson.rbxsystems.ch (en).

## Architecture

- SvelteKit static build wrapped in nginx:alpine container
- Image published to ghcr.io on push to main (or
  workflow_dispatch)
- ArgoCD Application in rbx-infra watches the image and syncs
  deployment to k3s
- Ingress with cert-manager + Let's Encrypt for TLS
- Backend (robsond) is at api.robson.rbx.ia.br (separate
  subdomain, CORS allows the two frontend origins)

## Prerequisites (operator)

### B1 — GHCR access (replaces Contabo bucket)
- Repo Settings → Actions → Workflow permissions:
  "Read and write permissions" enabled
- GHCR package visibility: public (read) — set after first push
  via GitHub UI Packages → robson-frontend-v2 → Visibility

### B2 — GitHub variable
- `PUBLIC_ROBSON_API_BASE_PROD` = `https://api.robson.rbx.ia.br`
  (set via `gh variable set`)

### B3 — k8s manifests in rbx-infra
Operator copies skeleton from `docs/k8s/frontend/` in this repo
into rbx-infra at the appropriate GitOps path. Adjust namespace,
ClusterIssuer name, ingress class, and image pull secret to
match cluster reality. Required objects:
- Namespace (or reuse robson namespace)
- Deployment with 2 replicas, image
  `ghcr.io/ldamasio/robson-frontend-v2:latest`, port 8080
- Service ClusterIP port 80 → 8080
- Ingress with TLS, cert-manager.io/cluster-issuer annotation,
  two hosts (rbx.ia.br + rbxsystems.ch), each with TLS
- Certificate (or rely on ingress shim if cluster uses
  ingress-shim from cert-manager)

### B4 — Image pull secret (only if package private)
- If GHCR package private: create dockerconfigjson Secret in
  namespace, attach via deployment.spec.imagePullSecrets
- If public: skip — k3s pulls anonymously

### B5 — DNS via rbx-infra dns-tofu-env.sh
- robson.rbx.ia.br      A or CNAME → cluster ingress IP/hostname
- robson.rbxsystems.ch  A or CNAME → cluster ingress IP/hostname
- api.robson.rbx.ia.br  A or CNAME → cluster ingress IP/hostname
  (already exists if backend already public)

### B6 — ArgoCD Application
- Application manifest in rbx-infra ArgoCD GitOps tree
- source.repoURL = rbx-infra repo, path = path of frontend
  manifests
- destination.namespace = chosen namespace
- syncPolicy.automated.prune + selfHeal recommended

### B7 — Backend CORS
- robsond must allow origins:
    https://robson.rbx.ia.br
    https://robson.rbxsystems.ch
- Methods: GET, POST, DELETE, OPTIONS
- Headers: Authorization, Content-Type
- SSE accept ?token= query param (already true in code)

## How to deploy

First-time:
1. Set var `PUBLIC_ROBSON_API_BASE_PROD`.
2. Push commit to main touching apps/frontend/**.
3. Workflow builds + pushes image to ghcr.io.
4. Apply k8s manifests via ArgoCD (sync the Application).
5. cert-manager emits cert (1–3min).
6. curl -I https://robson.rbx.ia.br → 200.

Subsequent deploys:
- Push to main → image rebuilt → ArgoCD reconciles latest tag.
- Or pin to specific sha tag via ArgoCD UI for canary/rollback.

## Rollback

- ArgoCD UI: pin Application image to previous sha-XXXXXXX tag.
- Or revert commit on main → workflow rebuilds → ArgoCD syncs.
- Or kubectl rollout undo deployment/robson-frontend-v2 -n <ns>
  (last-resort manual).

## Verification after deploy

    curl -I https://robson.rbx.ia.br
    curl -I https://robson.rbxsystems.ch
    curl -I https://robson.rbx.ia.br/healthz   # 200 ok
    # Browser: cold load /login, paste real token, redirect to
    # /dashboard, observe SSE events.

## Known gaps

- No CDN (in-cluster nginx; add Cloudflare in front later if
  latency global matters)
- Locale switcher UI deferred (host-based default sufficient)
- Hash chain UI (FE-P3) and history endpoints (FE-P2) pending
