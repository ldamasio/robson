
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

## Monorepo and Architecture

This repository is being migrated to a monorepo layout adopting Hexagonal Architecture (Ports & Adapters).

High-level structure:

```
apps/
  backend/            # Django monolith with hexagonal core under apps/backend/core
  frontend/           # React (Vite) app
infra/                # Terraform, Ansible, K8s, GitOps, Observability, DB
docs/                 # ADRs, architecture, developer guides
```

Read more: `docs/ARCHITECTURE.md`.

Legacy paths like `backends/monolith` and `frontends/web` are being moved to `apps/backend/monolith` and `apps/frontend` respectively.

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

Deploys de produção são feitos via GitOps/CI (GitHub Actions + ArgoCD + k3s). O script `./bin/dj` e o `docker-compose.dev.yml` são destinados apenas ao desenvolvimento local.

Notes
- The `./bin/dj` script is for local development only. Production deploys should be performed via your GitOps/CI pipeline (e.g., GitHub Actions + ArgoCD + k3s).
- The local Postgres runs with Docker Compose using `apps/backend/monolith/docker-compose.dev.yml` and credentials from `apps/backend/monolith/.env`.
  - Makefile helpers: `make dev-db-up`, `make dev-db-down`, `make dev-db-destroy`, `make dev-test`.
