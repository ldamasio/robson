Robson Bot – Developer Guide

Overview
- Robson is open source. This guide standardizes local development, migrations/tests, and contribution practices, keeping production isolated (GitOps/CI/CD).

Project Layout (essentials)
- Backend (Django): `backends/monolith/`
  - `manage.py`, `backend/settings.py`
  - `api/models/` (refactored models: `base.py`, `trading.py`)
  - `api/tests/test_models.py`
  - `docker-compose.dev.yml` (local Postgres for dev)
  - `bin/dj` (dev helper script)
- Frontend (Vite/React): `frontends/web/`
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
cd backends/monolith
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
# na raiz do repositório
make dev-db-up
```
Direct alternative without Makefile:
```
docker compose -f backends/monolith/docker-compose.dev.yml up -d
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
docker compose -f backends/monolith/docker-compose.dev.yml up -d
docker compose -f backends/monolith/docker-compose.dev.yml down
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
cd backends/monolith
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
cd frontends/web
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
  - Useful commands (in backends/monolith):
    - `black .`
    - `isort .`
    - `flake8 api/ backends/monolith/backend/`
    - `mypy api/` (if types are adopted)
- Pre‑commit (optional)
  - Install once: `make pre-commit-install` (or `python -m pip install pre-commit && pre-commit install`)
  - Run on all files: `make lint` (wraps `pre-commit run --all-files`)
  - Default hooks include: trailing whitespace, EOF fix, YAML check, black, isort, and an English‑only checker for comments/docstrings.
