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
   - [x] Add Helm chart skeletons for backend and frontend (Gateway API)
   - [x] Add ApplicationSet skeleton for branch previews
   - [x] Add App-of-Apps (ArgoCD) root with child apps (backend/front) and ApplicationSet
   - [x] Add CI workflow to build preview images for non-main branches
   - [ ] Restructure manifests to `infra/k8s/{base,overlays}` using Helm charts
   - [ ] Add ArgoCD app-of-apps and ApplicationSets
   - [ ] Update image build contexts in any GitOps refs to new `apps/*` paths
   - [ ] Configure cert-manager and external-dns for `robson.rbx.ia.br`
   - [ ] Install and configure Istio (Ambient Mode) with Gateway API (mandatory)

8) Infra specifics (Contabo + k3s + Ansible + Helm + GitOps previews)
   - [x] Ansible baseline security & bootstrap
       - [x] `infra/ansible/roles/bootstrap` (admin user, SSH hardening with Vault port, UFW; safe reconnect)
       - [x] `infra/ansible/group_vars/all/{main.yml,vault.yml}` with Vault-managed secrets
       - [x] `infra/ansible/inventory/contabo` with server/agent groups
       - [x] `infra/ansible/roles/k3s` minimal tasks
       - [x] `infra/ansible/site.yml` (bootstrap precedes k3s)
   - [ ] Base platform via Helm
       - [ ] cert-manager, external-dns
       - [ ] Istio Ambient Mode (sidecarless): install istio-base/istiod with ambient, ztunnel daemonset and CNI; enable mTLS by default
       - [ ] Gateway API CRDs and Istio integration: GatewayClass `istio`, per-env Gateways and HTTPRoutes
       - [ ] ArgoCD install via Helm and App of Apps (gitops root)
   - [ ] Service packaging via Helm
       - [ ] Charts for `apps/backend/monolith` and `apps/frontend` with values for host/image/tag/env
       - [ ] Gateway API resources (Gateway/HTTPRoute/TLS) templated in charts (no Ingress)
   - [ ] GitOps previews per branch (non-main)
       - [ ] ArgoCD ApplicationSet using Git generator to create env per branch (exclude `main`)
       - [ ] Namespace pattern `h-<branch>` (labels/annotations to opt-in Ambient: `istio.io/dataplane-mode: ambient`)
       - [ ] Host `h-<branch>.robson.rbx.ia.br`
       - [ ] Gateway API manifests from values; TLS via cert-manager (Certificate + ReferencePolicy to Gateway)
       - [ ] external-dns manages DNS for Gateway public IP/LoadBalancer
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
   - [x] Record decision: add ADR for Istio Ambient + Gateway API
   - [x] Record decision: add ADR for GitOps preview envs per branch
   - [x] Add infra overview and instructions (infra/README.md)

Notes
- Keep migrations and Django app structure working while extracting domain/application code.
- Avoid breaking API/URLs during refactor; adapt views to use use cases internally.
- Use import boundaries (e.g., import-linter) later to enforce layer rules.
- Ensure Python importability for `apps.*` packages (added `__init__.py`).
- DNS: prefer delegated zone for `robson.rbx.ia.br` and wildcard or external-dns automation for `h-*.robson.rbx.ia.br`.
- Istio Ambient + Gateway API: use Gateway resources for ingress, Waypoint proxies for L7 where needed, and Namespace-level ambient opt-in. Avoid classic Ingress objects.
- Secrets: manage with SealedSecrets or SOPS; bootstrap secrets with Ansible Vault as needed.
