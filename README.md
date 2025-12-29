
Robson Bot

[![Backend Tests](https://github.com/ldamasio/robson/actions/workflows/backend-tests.yml/badge.svg)](https://github.com/ldamasio/robson/actions/workflows/backend-tests.yml)

Just another crypto robot

ROBSON BOT is an open source algo trade project. It is a robot specialized in cryptocurrency trading (automatic buying and selling of digital assets), programmed, with backend and data modeling in Python, to monitor the market in real time, using asynchronous communication between the exchange and the application, that is, your dashboard and your “brain”. With this, Robson Bot is capable of making intelligent decisions based on a set of strategies guided by probabilistic analysis and technical analysis. The open source project includes a risk management system, tools for disseminating trade signals and functions as a platform, enabling multiple users with security and data isolation (multi-tenant).

The Robson Bot is a tool for researchers, traders that monitors stocks to trigger signals or automate order flows for the binance crypto stock market.

## Research, communication and trade functions

Designed as a cryptocurrency robot, it also has the ability to communicate and interact via Metaverse, providing services and remuneration to its users, with instructions for risk management.

## Command interface

The command interface makes it possible to activate a Dashboard with its main indicators or special features for you to carry out day-to-day activities.

## The Dashboard offers special string conversion calculators

For example, if you need to withdraw an amount of BRL, but would like to convert your USDT to ADA before transferring, in addition to needing to anticipate spread values from other financial services.

## CLI Quick Start

Robson Bot provides a command-line interface that implements an **agentic workflow** for safe trading execution:

**PLAN → VALIDATE → EXECUTE**

This mirrors professional trading: formulate ideas, paper trade (validate), then execute with intent.

### Building the CLI

```bash
# Build both C router and Go CLI
make build-cli

# Run smoke tests
make test-cli

# Install to system PATH (optional)
make install-cli
```

### Using the CLI

```bash
# 1. PLAN - Create an execution plan (no real orders)
robson plan buy BTCUSDT 0.001 --limit 50000

# Output: Plan ID: abc123def456

# 2. VALIDATE - Check operational and financial constraints
robson validate abc123def456 --client-id 1 --strategy-id 5

# Output: ✅ PASS or ❌ FAIL with detailed report

# 3. EXECUTE - Execute the plan (DRY-RUN by default)
# DRY-RUN (safe, simulation only)
robson execute abc123def456 --client-id 1

# LIVE (real orders - requires explicit acknowledgement)
robson execute abc123def456 --client-id 1 --live --acknowledge-risk
```

**Safety by default:**

- **DRY-RUN** is the default mode (simulation, no real orders)
- **LIVE** requires both `--live` AND `--acknowledge-risk` flags
- LIVE execution requires prior validation
- All executions are audited

### CLI Architecture

```
robson (C)
  └─> robson-go (Go + Cobra)
       └─> python manage.py {validate_plan,execute_plan} (Django)
```

The CLI is a thin router that delegates to Django management commands, ensuring all business logic remains in the application layer.

## Command-Line Tools

Robson Bot provides **three complementary command-line tools**, each optimized for different tasks:

### 1. `robson` - Trading Operations (Domain CLI)

**Use for:** All trading and business operations

```bash
# Agentic trading workflow
robson plan buy BTCUSDT 0.001
robson validate <plan-id> --client-id 1
robson execute <plan-id> --client-id 1

# Get help
robson --help
```

### 2. `just` - Development Tasks (Task Runner)

**Use for:** Daily development workflow

Install `just`:

```bash
# macOS
brew install just

# Linux
curl --proto '=https' --tlsv1.2 -sSf https://just.systems/install.sh | bash

# Windows (via scoop)
scoop install just
```

Common tasks:

```bash
# See all available tasks
just --list

# First-time setup
just setup

# Start database
just db-up

# Run migrations
just db-migrate

# Run all tests
just test

# Start dev servers
just dev-backend    # Terminal 1
just dev-frontend   # Terminal 2

# Environment info
just info
```

### 3. `make` - Build & Install

**Use for:** Compiling binaries and system-wide installation

```bash
# Build CLI
make build-cli

# Install CLI to system PATH
make install-cli

# Sync vendor documentation
make sync-binance-docs
```

### Quick Reference: Which Tool When?

| Task | Command |
|------|---------|
| **Trading operations** | `robson plan/validate/execute` |
| **Build CLI** | `make build-cli` |
| **Install CLI** | `make install-cli` |
| **Run tests** | `just test` |
| **Database setup** | `just db-up && just db-migrate` |
| **Start dev server** | `just dev-backend` |
| **Reset database** | `just db-reset` |
| **Environment check** | `just info` |

**Architecture guide:** See [`docs/COMMAND-RUNNERS.md`](docs/COMMAND-RUNNERS.md) for detailed guidelines on which tool to use when.

## BTC Portfolio Tracking

Robson Bot now supports **complete portfolio tracking in BTC terms**, the preferred metric for crypto investors.

### Why BTC?

Crypto investors prefer to measure their wealth in BTC (not USD) because:

- BTC is the base currency of the crypto market
- USD inflation can distort portfolio performance
- BTC shows true purchasing power in the crypto ecosystem

### Features

#### Backend Services

- **Pattern Detection Engine (CORE 1.0)**: Deterministic, idempotent pattern detection (7 patterns: Hammer, Inverted Hammer, Bullish/Bearish Engulfing, Morning Star, Head & Shoulders, Inverted H&S)
- **BTCConversionService**: Multi-route price discovery (direct pair, USDT, BUSD)
- **PortfolioBTCService**: Complete portfolio valuation in BTC
- **Binance Sync**: Automatic deposit/withdrawal synchronization
- **Audit Trail**: All external flows are recorded and audited

#### REST API Endpoints

```bash
GET /api/portfolio/btc/total/          # Current value in BTC
GET /api/portfolio/btc/profit/         # Profit since inception
GET /api/portfolio/btc/history/        # Historical chart data
GET /api/portfolio/deposits-withdrawals/  # Transaction list
```

#### CLI Commands

```bash
# Pattern detection (CORE 1.0)
python manage.py detect_patterns BTCUSDT 15m --all      # All patterns
python manage.py detect_patterns BTCUSDT 1h --candlestick  # Candlestick only
python manage.py detect_patterns BTCUSDT 4h --chart     # Chart patterns only

# Show portfolio in BTC
python manage.py portfolio_btc

# Show profit in BTC
python manage.py portfolio_btc --profit

# Sync deposits/withdrawals from Binance (last 90 days)
python manage.py sync_deposits --days-back 90
```

#### Profit Formula

```
Profit (BTC) = Current Balance (BTC) + Withdrawals (BTC) - Deposits (BTC)
```

This formula considers:

- **Current holdings**: What you have now
- **Withdrawals**: Past profits taken out (count as gains)
- **Deposits**: Your capital input (investment)

#### Frontend Dashboard

The **Portfolio tab** (💼 Portfolio) provides:

- **Overview**: Total value, profit metrics, account breakdown
- **History**: Interactive chart with timeline filtering (7d, 30d, 90d, 1y)
- **Transactions**: Filterable table of deposits/withdrawals

All values are displayed in BTC with:

- Color-coded profit (green) / loss (red)
- Auto-refresh every 60 seconds
- Clean tab-based navigation

### Quick Start

```bash
# 1. Apply database migration
python manage.py migrate api

# 2. Sync historical deposits
python manage.py sync_deposits --days-back 90

# 3. View portfolio in BTC
python manage.py portfolio_btc --profit

# 4. Open dashboard: http://localhost:3000/dashboard
# Navigate to "💼 Portfolio" tab
```

### Documentation

- See [`CHANGELOG.md`](CHANGELOG.md) for detailed changes
- See [`docs/AGENTS.md`](docs/AGENTS.md) for architecture details

## Monorepo and Architecture

This repository follows a monorepo layout with **Hexagonal Architecture (Ports & Adapters)** integrated within the Django monolith.

High-level structure:

```
apps/
  backend/
    monolith/
      api/
        application/      # Hexagonal core (ports, use cases, adapters)
        models/           # Django models
        views/            # REST endpoints
        tests/            # Test suite
  frontend/               # React (Vite) app
cli/                      # Go-based CLI (robson-go)
main.c                    # C router (robson)
infra/                    # Terraform, Ansible, K8s, GitOps, Observability, DB
docs/                     # ADRs, architecture, developer guides
```

**Key principle:** Hexagonal architecture is implemented **INSIDE** Django at `apps/backend/monolith/api/application/`, not as an external package. This provides clear separation of concerns while maintaining a single runtime.

Read more: `docs/ARCHITECTURE.md`.

## INSTALL

Some tips for development environment

### Clone robson repository

git clone <https://github.com/ldamasio/robson.git>

### Try run docker-compose

docker-compose up -d --build

### Development Backend Environment

Recommended local dev setup (Postgres via Docker + helper script):

```
cd apps/backend/monolith/
# 1) Prepare .env for development (localhost Postgres)
cp .env.development.example .env

# 2) Create venv and install deps
python -m venv .venv
source .venv/bin/activate
python -m pip install --upgrade pip
# python -m pip install --upgrade setuptools
python -m pip install -r requirements.txt
export DJANGO_SETTINGS_MODULE=backend.settings

# 3) Start local Postgres (Docker Compose)
cd ..
make dev-db-up
cd monolith

# 4) Migrate and run tests using the helper script
chmod +x bin/dj
./bin/dj makemigrations api
./bin/dj migrate
./bin/dj test

# 5) Run server
./bin/dj runserver
```

### Development Frontend Environment

cd apps/frontend
nvm use 14
npm i
npm start

To update vendor docs in the future, run:

```bash
make sync-binance-docs
```

## Contributing

Robson is 100% open source and contributions are welcome. For how to prepare your dev environment, run tests, create migrations, and submit PRs, see:

- docs/DEVELOPER.md
- docs/STYLE_GUIDE.md

Production deployments are performed via GitOps/CI (GitHub Actions + ArgoCD + k3s) using Istio (Ambient Mode) with Gateway API. Each branch ≠ main creates an automatic staging environment at `h-<branch>.robson.rbx.ia.br`. The `./bin/dj` script and `docker-compose.dev.yml` are intended for local development only. See `infra/README.md`.

Notes

- The `./bin/dj` script is for local development only. Production deploys should be performed via your GitOps/CI pipeline (e.g., GitHub Actions + ArgoCD + k3s).
- The local Postgres runs with Docker Compose using `apps/backend/monolith/docker-compose.dev.yml` and credentials from `apps/backend/monolith/.env`.
  - Makefile helpers: `make dev-db-up`, `make dev-db-down`, `make dev-db-destroy`, `make dev-test`.
