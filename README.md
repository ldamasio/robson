# Robson — Execution & Risk Engine for Leveraged Markets

[![Backend Tests](https://github.com/ldamasio/robson/actions/workflows/backend-tests.yml/badge.svg)](https://github.com/ldamasio/robson/actions/workflows/backend-tests.yml)

Robson is an execution and risk management engine designed for leveraged cryptocurrency markets. It is not a trading bot. It does not generate signals, predict prices, or optimize entries.

Robson is concerned with what happens **after** a trading decision is made: how orders are executed, how risk is enforced, how positions are managed through their lifecycle, and how failures are handled safely under volatile conditions.

The system provides a multi-tenant runtime with deterministic execution semantics, explicit risk controls, full auditability, and a clear separation between signal interpretation and order execution.

## Why Robson Exists

Most open-source trading systems conflate signal generation with execution. The result is software where risk management is an afterthought bolted onto an indicator library.

Robson inverts this. The execution and risk layers are the primary concern. Signal interpretation exists as an input boundary, not as the core of the system.

This design reflects a simple observation: in leveraged markets, **how** you execute matters more than **what** you execute. A sound signal with poor execution, missing stop logic, or uncontrolled position sizing will lose capital. Robson exists to make the execution path deterministic, auditable, and safe by default.

## Architecture

The system follows a **Hexagonal Architecture (Ports & Adapters)** within a Django monolith, with clear domain boundaries between execution, risk, and external integrations.

```
apps/
  backend/
    monolith/
      api/
        application/      # Hexagonal core (ports, use cases, adapters)
        models/           # Domain models and state persistence
        views/            # REST API surface
        tests/            # Test suite
  frontend/               # Operations dashboard (React/Vite)
cli/                      # Execution CLI (Go + C router)
main.c                    # CLI entrypoint (C router)
infra/                    # Terraform, Ansible, K8s, GitOps, Observability, DB
docs/                     # ADRs, architecture, developer guides
```

### Core Subsystems

**Execution Engine** — Manages the full order lifecycle: plan creation, pre-execution validation, dry-run simulation, and live execution. All state transitions are explicit and auditable. The engine enforces a strict `PLAN -> VALIDATE -> EXECUTE` pipeline that prevents unvalidated orders from reaching the exchange.

**Risk Engine** — Enforces position-level and portfolio-level constraints before and during execution. This includes market stop exits, liquidation distance checks, maximum position sizing, and controlled teardown of positions that violate risk parameters. Every exit carries an explicit reason code.

**Signal Layer** — An input boundary, not a decision-maker. Robson accepts signals (pattern detections, external triggers, manual commands) and routes them through validation and risk checks before any execution occurs. The signal layer includes a deterministic, idempotent pattern detection engine (Hammer, Inverted Hammer, Bullish/Bearish Engulfing, Morning Star, Head & Shoulders, Inverted H&S) that operates as a diagnostic tool, not an autonomous trading agent.

**Event and State System** — All position state transitions, risk events, execution outcomes, and external flows (deposits, withdrawals) are recorded as an append-only audit trail. Portfolio valuation is tracked in BTC terms to reflect actual purchasing power independent of fiat inflation.

**API and Multi-Tenant Layer** — A REST API provides programmatic access to execution plans, portfolio state, and risk parameters. The system supports multiple isolated tenants with per-client data boundaries.

### Determinism and Traceability

Every execution path through the system produces a traceable sequence of events: plan creation, validation result, risk check outcome, execution attempt, and final state. There are no implicit side effects. The same inputs under the same market conditions produce the same execution behavior.

## Risk Management

Risk is not a feature of Robson. It is the architecture.

**Market Stop Exits** — Positions carry explicit stop parameters. When market conditions breach these thresholds, the system initiates controlled exits without waiting for external signals.

**Liquidation Protection** — For leveraged positions, the system monitors liquidation distance and enforces minimum margin requirements. Positions approaching liquidation thresholds are flagged or closed before the exchange liquidation engine intervenes.

**Explicit Exit Reasons** — Every closed position carries a typed exit reason (stop hit, risk limit, manual close, validation failure, timeout). There are no silent exits.

**Controlled Position Lifecycle** — Positions move through defined states with validated transitions. A position cannot be modified without passing through the risk layer. Orphaned or inconsistent positions are detected and surfaced.

**Dry-Run by Default** — The execution pipeline defaults to simulation mode. Live execution requires explicit flags (`--live --acknowledge-risk`) and a prior passing validation. This makes it structurally difficult to execute unintended orders.

## Observability

**Event Tracking** — All system events (order submissions, risk checks, state transitions, external sync operations) are recorded with timestamps, context, and causality links.

**State Transitions** — Position and order state changes are logged as discrete events, enabling reconstruction of the full lifecycle of any position at any point in time.

**Portfolio Audit Trail** — External capital flows (deposits, withdrawals) are synchronized from the exchange and recorded. Portfolio valuation history is maintained for forensic analysis.

**Debugging** — The `PLAN -> VALIDATE -> EXECUTE` pipeline produces structured output at each stage, making it possible to diagnose failures without reproducing market conditions.

### REST API

```
GET /api/portfolio/btc/total/              # Current portfolio value (BTC)
GET /api/portfolio/btc/profit/             # Profit since inception (BTC)
GET /api/portfolio/btc/history/            # Historical valuation series
GET /api/portfolio/deposits-withdrawals/   # External capital flows
```

### CLI

```bash
# Execution pipeline
robson plan buy BTCUSDT 0.001 --limit 50000
robson validate <plan-id> --client-id 1 --strategy-id 5
robson execute <plan-id> --client-id 1                          # dry-run (default)
robson execute <plan-id> --client-id 1 --live --acknowledge-risk  # live

# Pattern detection (diagnostic)
python manage.py detect_patterns BTCUSDT 15m --all
python manage.py detect_patterns BTCUSDT 1h --candlestick
python manage.py detect_patterns BTCUSDT 4h --chart

# Portfolio state
python manage.py portfolio_btc --profit
python manage.py sync_deposits --days-back 90
```

### CLI Architecture

```
robson (C router)
  └─> robson-go (Go + Cobra)
       └─> python manage.py {validate_plan,execute_plan} (Django)
```

The CLI is a thin routing layer. All business logic, risk validation, and execution semantics remain in the application core.

## Positioning

**Robson is not a trading bot.**

It does not tell you what to buy. It does not scan for opportunities. It does not promise returns.

Robson is:

- An **execution system** that manages the lifecycle of orders from plan to settlement
- A **risk-aware runtime** that enforces safety invariants on every position
- An **experimental platform** for building and testing financial execution infrastructure

It is designed for engineers and researchers who need a controlled, auditable environment for studying execution behavior in leveraged markets.

## Development

### Prerequisites

```bash
git clone https://github.com/ldamasio/robson.git
```

### Backend

```bash
cd apps/backend/monolith/
cp .env.development.example .env
python -m venv .venv
source .venv/bin/activate
pip install --upgrade pip
pip install -r requirements.txt
export DJANGO_SETTINGS_MODULE=backend.settings

# Database
cd .. && make dev-db-up && cd monolith
./bin/dj makemigrations api
./bin/dj migrate
./bin/dj test
./bin/dj runserver
```

### Frontend

```bash
cd apps/frontend
nvm use 14
npm i
npm start
```

### Build CLI

```bash
make build-cli
make test-cli
make install-cli    # optional: install to system PATH
```

### Task Runner

Install [just](https://just.systems) for daily development tasks:

```bash
just --list         # see all tasks
just setup          # first-time setup
just db-up          # start database
just db-migrate     # run migrations
just test           # run all tests
just dev-backend    # start backend server
just dev-frontend   # start frontend server
just info           # environment info
```

## Deployment

Production deployments are performed via GitOps (GitHub Actions + ArgoCD + k3s) with Traefik ingress and cert-manager-managed TLS. Shared infrastructure automation belongs outside this repository; this repository focuses on the Robson application.

The `./bin/dj` script and `docker-compose.dev.yml` are for local development only.

See `docs/infra/K3S-CLUSTER-GUIDE.md` and `docs/runbooks/argocd-initial-setup.md` for deployment details.

## Contributing

Robson is open source. Contributions are welcome.

- `docs/DEVELOPER.md` — development setup and workflow
- `docs/STYLE_GUIDE.md` — code conventions
- `docs/ARCHITECTURE.md` — system design
- `docs/COMMAND-RUNNERS.md` — CLI tool guidelines

## License

Open source. See repository for license details.
