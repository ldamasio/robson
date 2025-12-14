Robson Bot – Developer Guide

Overview
- Robson is open source. This guide standardizes local development, migrations/tests, and contribution practices, keeping production isolated (GitOps/CI/CD).

Project Layout (essentials)
- Backend (Django): `apps/backend/monolith/`
  - `manage.py`, `backend/settings.py`
  - `api/models/` (refactored models: `base.py`, `trading.py`)
  - `api/tests/test_models.py`
  - `docker-compose.dev.yml` (local Postgres for dev)
  - `bin/dj` (dev helper script)
- Frontend (Vite/React): `apps/frontend/`
- Docs: `docs/`
  - `DEVELOPER.md` (this file)
  - `AUTH_FLOW.md`
  - `vendor/` (reference submodules, e.g., Binance)

Prerequisites
- Python 3.12+
- Node.js (front; see versions in the project)
- Docker + Docker Compose
- Postgres client (optional, for psql)

Backend quick start
1) Create venv and install deps
```
cd apps/backend/monolith
cp .env.development.example .env
python -m venv .venv
source .venv/bin/activate
python -m pip install -r requirements.txt
```
Important: ensure Binance testnet keys exist in `.env` (can be dummy), since settings read them without secrets:
```
RBS_BINANCE_API_KEY_TEST=dev-test-api-key
RBS_BINANCE_SECRET_KEY_TEST=dev-test-secret-key
```
2) Start local Postgres (Docker)
Run from the repo root:
```
# at repo root
make dev-db-up
```
Direct alternative without Makefile:
```
docker compose -f apps/backend/monolith/docker-compose.dev.yml up -d
```

Postgres from the repo root
```
# at repo root
make dev-db-up       # start dev Postgres
make dev-db-logs     # follow container logs
make dev-db-down     # stop container
make dev-db-destroy  # remove container and volume
```
Direct alternative without Makefile:
```
docker compose -f apps/backend/monolith/docker-compose.dev.yml up -d
docker compose -f apps/backend/monolith/docker-compose.dev.yml down
```

Clean‑slate reset (fast path)
- If you don’t need to preserve data and want to avoid interactive `makemigrations` prompts, use the full dev reset (drops Postgres volume and `api` migrations):
```
# from repo root
make dev-reset-api
```
The target performs:
- `docker compose down -v` and `up -d` for dev Postgres
- removes all migration files in `api/migrations` (except `__init__.py`)
- recreates and applies migrations from the current models
Then run tests as usual:
```
cd apps/backend/monolith
./bin/dj test
```
3) Run migrations and tests with the helper script
```
chmod +x bin/dj
./bin/dj makemigrations api
./bin/dj migrate
./bin/dj test
```
4) Runserver
```
./bin/dj runserver
```

Building the CLI
Robson Bot includes a command-line interface implementing the **agentic workflow**: PLAN → VALIDATE → EXECUTE

The CLI consists of:
- **C router** (`main.c`) - Thin wrapper that delegates to robson-go
- **Go CLI** (`cli/robson-go`) - Main CLI implementation using Cobra framework

Prerequisites for CLI:
- GCC (C compiler)
- Go 1.20+ (for building robson-go)

1) Build CLI from repo root:
```bash
# Build both C router and Go CLI
make build-cli

# Or build individually
make build-c     # gcc -o robson main.c
make build-go    # cd cli && go build -o robson-go .
```

2) Run smoke tests:
```bash
make test-cli
```

3) Install to system PATH (optional):
```bash
make install-cli
# Installs to /usr/local/bin by default
# Override with: make install-cli INSTALL_PATH=/custom/path
```

4) Verify installation:
```bash
robson --help
```

CLI Agentic Workflow
The CLI enforces a safe three-step workflow for trading operations:

**PLAN** - Create execution blueprint
```bash
robson plan buy BTCUSDT 0.001 --limit 50000
# Output: Plan ID: abc123def456
```
- Creates an execution plan (NO real orders)
- Generates unique plan ID
- Specifies strategy and parameters

**VALIDATE** - Paper trading stage
```bash
robson validate abc123def456 --client-id 1 --strategy-id 5
# Output: Validation report (PASS/FAIL/WARNING)
```
- Validates operational and financial constraints
- Checks:
  - Tenant isolation (client_id is MANDATORY)
  - Risk configuration (drawdown, stop-loss, position sizing)
  - Operation parameters (symbol, quantity, price)
- Output: Clear PASS/FAIL/WARNING report

**EXECUTE** - Final execution (SAFE BY DEFAULT)
```bash
# DRY-RUN mode (default, simulation only)
robson execute abc123def456 --client-id 1

# LIVE mode (real orders - requires explicit acknowledgement)
robson execute abc123def456 --client-id 1 --live --acknowledge-risk
```
- **DRY-RUN** (default):
  - Simulates execution
  - NO real orders placed
  - Always allowed
  - Useful for testing and verification
- **LIVE** (real orders):
  - Requires `--live` flag
  - Requires `--acknowledge-risk` flag
  - Requires prior validation
  - Places REAL orders on exchange
  - Enforces execution limits

CLI Architecture
```
robson (C)
  └─> robson-go (Go + Cobra)
       └─> python manage.py {validate_plan,execute_plan} (Django)
```

The CLI delegates to Django management commands, ensuring all business logic remains in the application layer.

Django Management Commands (Advanced)
You can also invoke validation/execution directly via Django:

```bash
# Validation
python manage.py validate_plan \
  --plan-id abc123 \
  --client-id 1 \
  --strategy-id 5 \
  --operation-type buy \
  --symbol BTCUSDT \
  --quantity 0.001 \
  --price 50000

# Execution (DRY-RUN)
python manage.py execute_plan \
  --plan-id abc123 \
  --client-id 1

# Execution (LIVE)
python manage.py execute_plan \
  --plan-id abc123 \
  --client-id 1 \
  --live \
  --acknowledge-risk \
  --validated \
  --validation-passed
```

CLI Troubleshooting
- **"robson-go: command not found"**
  - Ensure `robson-go` is in your PATH
  - Or run from repo root: `./cli/robson-go`
  - Or reinstall: `make install-cli`

- **"Django manage.py not found"**
  - CLI looks for manage.py in standard locations
  - Ensure you're running from repo root
  - Or set DJANGO_MANAGE_PY environment variable

- **Build failures**
  - C compilation: Ensure gcc is installed
  - Go build: Run `cd cli && go mod download`
  - Check build logs for specific errors

- **Validation/Execution failures**
  - Check that Django backend is running
  - Verify database is accessible
  - Review validation report for specific issues
  - Ensure client_id exists in database

Helper Script `bin/dj`
- Purpose: shorten commands and add guard rails to prevent using production DB.
- Requires `.env` pointing to local Postgres (localhost).
- Useful commands:
  - `./bin/dj db:up | db:down | db:destroy` – control local Postgres via Makefile
  - `./bin/dj makemigrations [app]` – create migrations
  - `./bin/dj migrate` – apply migrations
  - `./bin/dj test` – run API model tests
  - `./bin/dj runserver` – start local server

Databases (Dev vs Prod)
- Never use the production DB in dev/tests.
- In dev: use local Postgres via `docker-compose.dev.yml` (port 5432, localhost). `.env` variables:
  - `RBS_PG_HOST=localhost`, `RBS_PG_PORT=5432`, `RBS_PG_DATABASE=robson_dev`, `RBS_PG_USER=robson`, `RBS_PG_PASSWORD=robson`
- To reset: `make dev-db-destroy` and re‑apply migrations.

Migration policy
- Prefer explicit migrations over ambiguous auto‑renames.
- When renaming fields, use `migrations.RenameField` and add `RunPython` for data if needed.
- Avoid `--fake-initial` unless you know it matches the DB schema.
- In dev, if no valuable data, dropping the DB can simplify.

Tests
- Run API tests:
```
./bin/dj test
```
- Django will create a temporary test DB in local Postgres.
- Write domain‑focused tests (e.g., `tests/test_models.py`).

Frontend (quick)
```
cd apps/frontend
nvm use 14
npm i
npm start
```

Third‑party integrations & docs
- Submodules and reference material live in `docs/vendor`.
- To sync Binance docs: `make sync-binance-docs` (see Makefile).

Contributing
- Suggested workflow:
  - Fork → feature branch → PR with small scope and tests.
  - Describe impact (schema/migrations, endpoints, breaking changes) in the PR body.
- Code
  - Keep domain organization (`api/models/trading.py`, etc.).
  - Reuse via common mixins/managers (`api/models/base.py`).
  - Avoid external services in tests; use flags (e.g., `TRADING_ENABLED=False`).
- Migrations
  - Include relevant migrations and consider `RunPython` for data when needed.
- Security & Data
  - Never commit secrets. Use local `.env` and keep `.env.example` updated.
- Production
  - Production deploys via GitOps/CI (GitHub Actions + ArgoCD + k3s). Do not use `bin/dj` for prod.
 - AI collaboration
   - Follow docs/AI_WORKFLOW.md: English‑only, Conventional Commits, and always propose a semantic commit message after changes.

Support
- Open issues with clear reproduction, logs, and environment version.
- PRs are welcome! Please read this guide before submitting.

Coding Style
- Philosophy
  - Simple, readable code with single responsibility per module/object.
  - Use type hints when they aid clarity/maintenance.
  - Consistent naming (`snake_case` in Python; `PascalCase` for classes; `UPPER_SNAKE_CASE` for constants).
- Structure
  - Imports: stdlib → third‑party → local. Avoid circular imports.
  - Models: prefer common mixins/base (`api/models/base.py`).
  - Views/APIs: consistent endpoints (snake_case), domain‑based separation.
- Docstrings & Comments
  - Docstrings for public functions/methods, concise and helpful.
  - Comments for “why”, not “what” (the code explains “what”).
- Tooling (optional, recommended)
  - Black (formatter), isort (imports), Flake8 (lint), Mypy (types).
  - Install in dev venv: `python -m pip install black isort flake8 mypy`
  - Useful commands (in apps/backend/monolith):
    - `black .`
    - `isort .`
    - `flake8 api/ apps/backend/monolith/backend/`
    - `mypy api/` (if types are adopted)
- Pre‑commit (optional)
  - You may use pre-commit locally if you prefer, but it is not required and is not part of CI.

Kubernetes Tooling (Optional)
- **K9s** (terminal UI for Kubernetes):
  - Recommended for developers who interact with deployed environments (staging, preview, production).
  - Used for cluster inspection, pod debugging, and log tailing.
  - See [../infra/K9S-OPERATIONS.md](../infra/K9S-OPERATIONS.md) for installation and workflows.
  - Note: K9s is read-mostly; permanent changes must go through GitOps (not manual edits).
