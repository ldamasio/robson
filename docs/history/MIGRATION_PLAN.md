# Migration Plan — Hexagonal Monorepo (Archived)

Status: Completed
Archived: This plan has been executed and is kept for historical reference. Current architecture and workflows are documented in:
- docs/ARCHITECTURE.md
- docs/adr/* (application decisions only)
- docs/infra/* and docs/runbooks/*

Scope (achieved)
- Monorepo structure with services under `apps/`, infra under `infra/`, docs under `docs/`.
- Backend Django monolith organized with Hexagonal (Ports & Adapters) core.
- Frontend (Vite/React) with Ports & Adapters.
- GitOps skeleton with ArgoCD for declarative deployment.
- Platform manifests and shared infrastructure experiments were tracked at the time in this repository and later moved out of scope.

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
- [x] Branch-tagged images workflow for non-main

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
- [x] Helm charts for backend/frontend with TLS-enabled deployment paths
- [x] ArgoCD deployment structure for the application
- [x] cert-manager-based certificate automation

8) Infra specifics
- [x] Shared infrastructure work was documented here during the migration and later removed from the application repository scope

Documentation updates
- [x] ADR-0002 Hexagonal
- [x] Application-facing operations docs
- [x] Archived legacy monolith MIGRATION_GUIDE.md to docs/history/

Notes
- DNS and cluster bootstrap details are outside the current scope of the application repository.
