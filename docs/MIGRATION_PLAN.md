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
   - [x] Add frontend unit tests to CI (Vitest workflow)
4) Hexagonal refactor (incremental)
   - [x] Identify domain entities/services → move to `core/domain` (Symbol, Order)
   - [x] Bootstrap `core/application/ports.py` with initial contracts
   - [x] Wrap existing persistence as adapters under `core/adapters/driven` (DjangoOrderRepository)
   - [x] Wrap REST endpoints to call use cases in `core/application` (PlaceOrder)
   - [x] Add wiring factories in `core/wiring`
   - [x] Provide external stubs/adapters (BinanceMarketData, StubExecution, NoopEventBus, RealClock)
   - [x] Add unit tests for PlaceOrderUseCase (fakes; no DB)
   - [x] Add contract tests for DjangoOrderRepository (persist + list_recent)
5) Frontend alignment
   - [x] Add `src/{domain,ports,adapters,application}` and map API calls via ports (created domain/ports/adapters; wired Strategies)
   - [x] Replace direct fetches with `TradeService`-like interfaces (Strategies.jsx)
   - [x] Configure envs (`VITE_API_BASE_URL`, `VITE_WS_URL`) and update README
   - [x] Normalize env usage in Dataframe.jsx (use VITE_API_BASE_URL)
   - [x] Normalize env usage in Patrimony.jsx and Balance.jsx
   - [x] Parameterize Binance WS URL in ActualPrice.jsx via `VITE_WS_URL_BINANCE`
   - [x] Add minimal contract tests for adapters (mock fetch for TradeHttp via Vitest)
   - [x] Add WebSocket port+adapter and contract test (MarketWS)
6) GitOps/Infra alignment
   - [ ] Restructure manifests to `infra/k8s/{base,overlays}` using Helm charts
   - [ ] Add ArgoCD app-of-apps and ApplicationSets
   - [ ] Update image build contexts in any GitOps refs to new `apps/*` paths
   - [ ] Configure cert-manager and external-dns for `robson.rbx.ia.br`
   - [ ] Configure Istio Ambient (optional) for mTLS/ingress gateways

8) Infra specifics (Contabo + k3s + Ansible + Helm + GitOps previews)
   - [ ] Ansible cluster bootstrap (k3s)
       - [ ] `infra/ansible/inventory/contabo` with 4 VPS hosts (roles: server/agent)
       - [ ] `infra/ansible/roles/k3s` to install and join nodes; idempotent; supports adding new VPS
       - [ ] `infra/ansible/site.yml` to run full cluster bootstrap (k3s + prerequisites)
   - [ ] Base platform via Helm
       - [ ] cert-manager, external-dns, ingress/mesh (Istio Ambient if chosen)
       - [ ] ArgoCD install via Helm and App of Apps (gitops root)
   - [ ] Service packaging via Helm
       - [ ] Charts for `apps/backend/monolith` and `apps/frontend` with values for host/image/tag/env
   - [ ] GitOps previews per branch (non-main)
       - [ ] ArgoCD ApplicationSet using Git generator to create env per branch (exclude `main`)
       - [ ] Namespace pattern `h-<branch>` and host `h-<branch>.robson.rbx.ia.br`
       - [ ] Ingress templated from values; TLS via cert-manager; DNS via external-dns
       - [ ] Branch name sanitization (lowercase, alnum and dashes)
   - [ ] CI integration for previews
       - [ ] Build/push images for non-main with tag `<branch>-<sha>`
       - [ ] Expose image tag to ApplicationSet via values or ArgoCD Image Updater
       - [ ] Auto-sync enabled; destroy env on branch deletion
7) Documentation updates
   - [x] Update `docs/DEVELOPER.md` paths
   - [x] Add Hexagonal code examples in `apps/backend/core`
   - [x] Add contribution notes for adapters and ports (docs/CONTRIBUTING-ADAPTERS.md)
   - [x] Record decision: add ADR for Hexagonal Architecture (Ports & Adapters)

Notes
- Keep migrations and Django app structure working while extracting domain/application code.
- Avoid breaking API/URLs during refactor; adapt views to use use cases internally.
- Use import boundaries (e.g., import-linter) later to enforce layer rules.
- Ensure Python importability for `apps.*` packages (added `__init__.py`).
- DNS: prefer delegated zone for `robson.rbx.ia.br` and wildcard or external-dns automation for `h-*.robson.rbx.ia.br`.
- Secrets: manage with SealedSecrets or SOPS; bootstrap secrets with Ansible Vault as needed.
