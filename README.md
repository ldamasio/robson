
Robson Bot

[![Backend Tests](https://github.com/ldamasio/robson/actions/workflows/backend-tests.yml/badge.svg)](https://github.com/ldamasio/robson/actions/workflows/backend-tests.yml)

Just another crypto robot

ROBSON BOT is an open source algo trade project. It is a robot specialized in cryptocurrency trading (automatic buying and selling of digital assets), programmed, with backend and data modeling in Python, to monitor the market in real time, using asynchronous communication between the exchange and the application, that is, your dashboard and your “brain”. With this, Robson Bot is capable of making intelligent decisions based on a set of strategies guided by probabilistic analysis and technical analysis. The open source project includes a risk management system, tools for disseminating trade signals and functions as a platform, enabling multiple users with security and data isolation (multi-tenant).

The Robson Bot is a tool for researchers, traders that monitors stocks to trigger signals or automate order flows for the binance crypto stock market.

## Research, communication and trade functions.

Designed as a cryptocurrency robot, it also has the ability to communicate and interact via Metaverse, providing services and remuneration to its users, with instructions for risk management.

## Command interface

The command interface makes it possible to activate a Dashboard with its main indicators or special features for you to carry out day-to-day activities.

## The Dashboard offers special string conversion calculators. 

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

git clone https://github.com/ldamasio/robson.git

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
