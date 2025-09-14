# Migration Plan — Hexagonal Monorepo (Archived)

Status: Completed
Archived: This plan has been executed and is kept for historical reference. Current architecture and workflows are documented in:
- docs/ARCHITECTURE.md
- docs/adr/* (Hexagonal, Istio Ambient + Gateway API, GitOps Previews, Ansible Bootstrap)
- infra/README.md and docs/infra/*

Scope (achieved)
- Monorepo structure with services under `apps/`, infra under `infra/`, docs under `docs/`.
- Backend Django monolith organized with Hexagonal (Ports & Adapters) core.
- Frontend (Vite/React) with Ports & Adapters.
- GitOps skeleton with ArgoCD App-of-Apps, ApplicationSet for per-branch previews.
- Platform manifests for Gateway API CRDs, Istio Ambient and cert-manager.

Steps (final state)
1) Docs and skeleton
- [x] `docs/ARCHITECTURE.md`
- [x] `apps/backend/core/{domain,application,adapters,wiring}`
- [x] `apps/frontend/README.md`
- [x] Root `README.md`

2) Relocate code
- [x] `backends/monolith` → `apps/backend/monolith`
- [x] `frontends/web` → `apps/frontend` (legacy cleaned)
- [x] `k8s` → `infra/k8s`
- [x] `backends/database` → `infra/data/postgres`
- [x] `backends/cronjob` → `apps/backend/cronjob`
- [x] `backends/nginx_monolith` → `apps/backend/nginx_monolith`
- [x] Cleaned legacy `backends/` and `frontends/` dirs

3) Tooling and CI
- [x] Paths updated in docker-compose and Makefile
- [x] CI paths fixed; frontend tests (Vitest) added
- [x] Preview images workflow (<branch>-<short_sha>) for non-main

4) Backend (Hexagonal)
- [x] Domain entities (Symbol, Order) extracted
- [x] Ports/contracts defined
- [x] Use case PlaceOrder
- [x] Adapters: Django repo, Binance MD (stub exec), event bus (noop), clock
- [x] Wiring/container
- [x] REST view integrated with use case
- [x] Tests: unit (use case) and contract (repo)

5) Frontend (Ports & Adapters)
- [x] Domain, ports, adapters (HTTP/WS), application layer
- [x] Strategies via TradeHttp; envs normalized (VITE_*)
- [x] Tests: TradeHttp + MarketWS

6) GitOps/Infra alignment
- [x] Helm charts for backend/frontend with Gateway/HTTPRoute + TLS
- [x] ApplicationSet for previews (branch sanitization, ambient labels, per-NS Issuer)
- [x] App-of-Apps with backend/frontend, ApplicationSet, cert-manager, Istio ambient, Gateway API CRDs
- [x] TLS via cert-manager (HTTP-01 with Gateway API); DNS strategy: wildcard at Registro.br

8) Infra specifics (Contabo + k3s + Ansible + Helm + GitOps)
- [x] Ansible baseline security (`bootstrap` role: SSH hardening + UFW + admin key via Vault)
- [x] Ansible k3s role scaffold and site.yml
- [x] Platform manifests prepared (to be applied during cluster bootstrap)

Documentation updates
- [x] ADR-0002 Hexagonal, ADR-0003 Istio Ambient + Gateway API, ADR-0004 GitOps Previews, ADR-0005 Ansible Bootstrap
- [x] infra/README.md, docs/infra/*
- [x] Archived legacy monolith MIGRATION_GUIDE.md to docs/history/

Notes
- DNS: wildcard-only at Registro.br is adopted; external-dns remains optional for future dynamic DNS.
- Cluster bootstrap (Ansible, ArgoCD install, platform apply) is an operational step and will be run separately from this migration.

