# ADR-0028: Hosting Pivot — Contabo S3 → k3s In-Cluster

**Date:** 2026-04-25
**Status:** Accepted

## Context

FE-P1 code-complete landed on main with a Contabo Object Storage deployment target. During operator provisioning, Contabo S3 revealed several limitations:

- No native website hosting (no index/error document support)
- Tenant-wide credentials (no per-bucket IAM scoping)
- No ACL management via UI
- TLS termination mismatch on custom domain CNAMEs

The operator provisioned a Contabo bucket but the limitations made it unsuitable for a production SPA frontend.

## Decision

Pivot frontend hosting from Contabo S3 static to k3s in-cluster deployment:

- Frontend becomes a containerized nginx:alpine serving the SvelteKit static build
- Container image published to ghcr.io (`ghcr.io/ldamasio/robson-frontend-v2`)
- ArgoCD GitOps manages the deployment in rbx-infra
- TLS via cert-manager + Let's Encrypt (resolves the TLS gap from the previous runbook B4)
- No CDN initially; Cloudflare can be added later if global latency matters

## Consequences

**Preserved:**
- Bearer-token auth (ADR-0025 A1.2) unchanged
- Host-based locale default (rbx.ia.br → pt-BR, rbxsystems.ch → en)
- Dual-domain architecture
- FE-P1 stack already on main — only deploy mechanism changes

**New:**
- Dockerfile + nginx.conf added to apps/frontend/
- GitHub Actions workflow rewritten: builds Docker image, pushes to GHCR
- k8s skeleton manifests in docs/k8s/frontend/ for operator to copy into rbx-infra
- ArgoCD reconciles on image tag update (no kubectl in CI)
- Health endpoint `/healthz` for k8s probes

**Trade-offs:**
- Higher operational complexity (container + k8s vs static files)
- nginx serves from memory/disk in-cluster, no edge caching
- GHCR as registry dependency (free tier, GitHub-native)
