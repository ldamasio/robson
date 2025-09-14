# Migration Plan — Hexagonal Monorepo

Goal: migrate the existing repository to a monorepo with Hexagonal Architecture (Ports & Adapters), preserving functionality while improving modularity and testability.

Status: In progress

Scope
- Move backend Django monolith to `apps/backend/monolith` and introduce `apps/backend/core/*` for hexagonal layers.
- Move frontend (Vite/React) to `apps/frontend` and prepare client-side ports/adapters.
- Consolidate infra under `infra/` (k8s, DB init SQLs, etc.).
- Update docs and developer tooling to match paths.

Steps
1) Docs and skeleton
   - [x] Add `docs/ARCHITECTURE.md`
   - [x] Create `apps/backend/core/{domain,application,adapters,wiring}` skeleton
   - [x] Create `apps/frontend/README.md`
   - [x] Update root `README.md`
2) Relocate code
   - [x] Move `backends/monolith` → `apps/backend/monolith`
   - [x] Move `frontends/web` → `apps/frontend` (legacy README kept as `README.LEGACY.md`)
   - [x] Move `k8s` → `infra/k8s`
   - [x] Move `backends/database` SQLs → `infra/data/postgres`
   - [x] Move `backends/cronjob` → `apps/backend/cronjob`
   - [x] Move `backends/nginx_monolith` → `apps/backend/nginx_monolith`
3) Update tooling and compose
   - [x] Update `docker-compose.yml` paths
   - [x] Update `Makefile` dev paths
   - [x] Fix env var placeholders in `docker-compose.yml`
   - [x] Verify and update CI workflows referencing old paths (`.github/workflows/*`)
4) Hexagonal refactor (incremental)
   - [x] Identify domain entities/services → move to `core/domain` (Symbol, Order)
   - [x] Bootstrap `core/application/ports.py` with initial contracts
   - [x] Wrap existing persistence as adapters under `core/adapters/driven` (DjangoOrderRepository)
   - [x] Wrap REST endpoints to call use cases in `core/application` (PlaceOrder)
   - [x] Add wiring factories in `core/wiring`
   - [x] Provide external stubs/adapters (BinanceMarketData, StubExecution, NoopEventBus, RealClock)
5) Frontend alignment
   - [x] Add `src/{domain,ports,adapters,application}` and map API calls via ports (created domain/ports/adapters; wired Strategies)
   - [x] Replace direct fetches with `TradeService`-like interfaces (Strategies.jsx)
   - [x] Configure envs (`VITE_API_BASE_URL`, `VITE_WS_URL`) and update README
   - [x] Normalize env usage in Dataframe.jsx (use VITE_API_BASE_URL)
   - [x] Normalize env usage in Patrimony.jsx and Balance.jsx
   - [ ] Add minimal contract tests for adapters (mock fetch/WS)
6) GitOps/Infra alignment
   - [ ] Restructure manifests to `infra/k8s/{base,overlays}` or Helm/Kustomize
   - [ ] Add ArgoCD app-of-apps if applicable
   - [ ] Update image build contexts in any GitOps refs to new `apps/*` paths
7) Documentation updates
   - [x] Update `docs/DEVELOPER.md` paths
   - [x] Add Hexagonal code examples in `apps/backend/core`
   - [ ] Add contribution notes for adapters and ports
   - [x] Record decision: add ADR for Hexagonal Architecture (Ports & Adapters)

Notes
- Keep migrations and Django app structure working while extracting domain/application code.
- Avoid breaking API/URLs during refactor; adapt views to use use cases internally.
- Use import boundaries (e.g., import-linter) later to enforce layer rules.
- Ensure Python importability for `apps.*` packages (added `__init__.py`).
