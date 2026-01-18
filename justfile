# Robson Bot - Development Task Runner
#
# This focuses on DAILY DEVELOPMENT tasks.
# For BUILD/INSTALL tasks, see Makefile.
# For TRADING operations, use `robson` CLI.
#
# Quick start:
#   just --list       # See all available tasks
#   just setup        # First-time setup
#   just test         # Run all tests
#
# Architecture: docs/COMMAND-RUNNERS.md

# List all available tasks (default)
default:
    @just --list

# ============================================================================
# Setup & Dependencies
# ============================================================================

# First-time setup: install all dependencies
setup: setup-python setup-node setup-go
    @echo ""
    @echo "✅ Development environment ready!"
    @echo ""
    @echo "Next steps:"
    @echo "  1. Build CLI:        make build-cli"
    @echo "  2. Start database:   just db-up"
    @echo "  3. Run migrations:   just db-migrate"
    @echo "  4. Run tests:        just test"
    @echo "  5. Start dev server: just dev-backend"

# Install Python dependencies (Django backend)
setup-python:
    @echo "📦 Installing Python dependencies..."
    cd apps/backend/monolith && pip install -r requirements.txt

# Install Node.js dependencies (React frontend)
setup-node:
    @echo "📦 Installing Node.js dependencies..."
    cd apps/frontend && npm install

# Install Go dependencies (CLI)
setup-go:
    @echo "📦 Installing Go dependencies..."
    cd cli && go mod download

# ============================================================================
# Build (delegates to Make)
# ============================================================================

# Build everything (delegates to Make for compilation)
build:
    @echo "🔨 Building via Make..."
    make build-cli
    @echo ""
    @echo "Tip: For system-wide installation, run: make install-cli"

# Clean build artifacts (delegates to Make)
clean:
    make clean-cli

# ============================================================================
# Testing
# ============================================================================

# Run all tests (backend + frontend + CLI)
test: test-backend test-frontend test-cli

# Run backend tests (Django)
test-backend:
    @echo "🧪 Running backend tests..."
    cd apps/backend/monolith && python manage.py test -v 2

# Run frontend tests (Vitest)
test-frontend:
    @echo "🧪 Running frontend tests..."
    cd apps/frontend && npm test

# Run CLI smoke tests
test-cli:
    @echo "🧪 Running CLI smoke tests..."
    @if [ ! -f cli/robson-go ]; then \
        echo "⚠️  CLI not built. Run: make build-cli"; \
        exit 1; \
    fi
    cd cli && ./smoke-test.sh

# Watch mode: run backend tests on file changes
test-watch:
    @echo "👀 Watching backend tests..."
    cd apps/backend/monolith && python manage.py test --keepdb --parallel --failfast

# ============================================================================
# Database (Development)
# ============================================================================

# Start development database (Postgres via Docker)
db-up:
    @echo "🚀 Starting Postgres..."
    docker compose -f apps/backend/monolith/docker-compose.dev.yml up -d
    @echo ""
    @echo "✅ Database running:"
    @echo "   Host: localhost:5432"
    @echo "   DB:   robson_dev"
    @echo "   User: robson"

# Stop development database
db-down:
    @echo "🛑 Stopping Postgres..."
    docker compose -f apps/backend/monolith/docker-compose.dev.yml down

# Show database logs
db-logs:
    docker compose -f apps/backend/monolith/docker-compose.dev.yml logs -f

# Create Django migrations
db-makemigrations:
    @echo "🔄 Creating migrations..."
    cd apps/backend/monolith && python manage.py makemigrations api

# Apply Django migrations
db-migrate:
    @echo "🔄 Applying migrations..."
    cd apps/backend/monolith && python manage.py migrate

# Database status
db-status:
    @echo "📊 Database status:"
    @docker compose -f apps/backend/monolith/docker-compose.dev.yml ps
    @echo ""
    @echo "Pending migrations:"
    @cd apps/backend/monolith && python manage.py showmigrations api | grep '\[ \]' || echo "  ✅ All migrations applied"

# Open database shell (psql)
db-shell:
    docker compose -f apps/backend/monolith/docker-compose.dev.yml exec postgres psql -U robson -d robson_dev

# Destroy database and volumes (REQUIRES CONFIRMATION)
[confirm]
db-destroy:
    @echo "🗑️  Destroying database..."
    docker compose -f apps/backend/monolith/docker-compose.dev.yml down -v
    @echo "✅ Database destroyed"

# Reset database: destroy + recreate + migrate (REQUIRES CONFIRMATION)
[confirm]
db-reset: db-destroy db-up
    @echo "⏳ Waiting for Postgres to be ready..."
    @sleep 3
    @echo ""
    @echo "🧹 Removing api migrations (except __init__.py)..."
    @find apps/backend/monolith/api/migrations -type f -name "*.py" ! -name "__init__.py" -delete || true
    @find apps/backend/monolith/api/migrations -type d -name "__pycache__" -exec rm -rf {} + 2>/dev/null || true
    @echo ""
    @echo "🔄 Creating fresh migrations..."
    @just db-makemigrations
    @echo ""
    @echo "🔄 Applying migrations..."
    @just db-migrate
    @echo ""
    @echo "✅ Database reset complete!"

# ============================================================================
# Development Servers
# ============================================================================

# Start Django development server
dev-backend: db-up
    @echo "🚀 Starting Django dev server..."
    @echo "   URL: http://localhost:8000"
    @echo ""
    cd apps/backend/monolith && python manage.py runserver

# Start frontend development server
dev-frontend:
    @echo "🚀 Starting React dev server..."
    @echo "   URL: http://localhost:5173"
    @echo ""
    cd apps/frontend && npm run dev

# Open Django shell
shell:
    cd apps/backend/monolith && python manage.py shell

# ============================================================================
# Code Quality
# ============================================================================

# Format code (Python, Go, JavaScript)
fmt:
    @echo "🎨 Formatting code..."
    @echo "  → Go (gofmt)"
    @cd cli && go fmt ./...
    @echo "  ✅ Go formatted"
    @# TODO: Add Python (black/ruff) and JS (prettier) when configured

# Lint code
lint:
    @echo "🔍 Linting code..."
    @echo "  → Go (go vet)"
    @cd cli && go vet ./...
    @echo "  ✅ Go linted"
    @# TODO: Add Python (ruff) and JS (eslint) when configured

# Run validation checks (AI governance, etc.)
validate:
    @echo "🔍 Running validation checks..."
    @if [ -f .ai-agents/validate.sh ]; then \
        ./.ai-agents/validate.sh; \
    else \
        echo "⚠️  .ai-agents/validate.sh not found"; \
        echo "   Create it or run specific linters manually"; \
    fi

# ============================================================================
# Infrastructure & Kubernetes
# ============================================================================

# Launch K9s (Kubernetes terminal UI)
k9s:
    @echo "🚀 Launching K9s..."
    k9s

# Launch K9s for specific namespace
k9s-ns NAMESPACE:
    @echo "🚀 Launching K9s for namespace: {{NAMESPACE}}"
    ./infra/scripts/k9s-ns.sh "{{NAMESPACE}}"

# Launch K9s for preview environment
k9s-preview BRANCH:
    @echo "🚀 Launching K9s for preview: {{BRANCH}}"
    ./infra/scripts/k9s-preview.sh "{{BRANCH}}"

# ============================================================================
# Documentation
# ============================================================================

# Sync Binance API docs (delegates to Make)
docs-sync-binance:
    make sync-binance-docs

# ============================================================================
# Worktrees & Sessions
# ============================================================================

# Create a git worktree and tmux session (claude|codex|shell)
wt-new AGENT NAME BRANCH:
    ./devtools/robson-wt-new.sh "{{AGENT}}" "{{NAME}}" "{{BRANCH}}"

# ============================================================================
# Domain Actions (Thin wrappers - prefer using `robson` directly)
# ============================================================================

# Show trading workflow example
trading-help:
    @echo "╔════════════════════════════════════════════════════════════╗"
    @echo "║           ROBSON AGENTIC TRADING WORKFLOW                 ║"
    @echo "╚════════════════════════════════════════════════════════════╝"
    @echo ""
    @echo "For trading operations, use the 'robson' CLI directly:"
    @echo ""
    @echo "1. PLAN (create execution blueprint)"
    @echo "   robson plan buy BTCUSDT 0.001 --limit 50000"
    @echo ""
    @echo "2. VALIDATE (paper trading checks)"
    @echo "   robson validate <plan-id> --client-id 1"
    @echo ""
    @echo "3. EXECUTE (DRY-RUN by default)"
    @echo "   robson execute <plan-id> --client-id 1"
    @echo ""
    @echo "4. EXECUTE LIVE (requires explicit acknowledgment)"
    @echo "   robson execute <plan-id> --client-id 1 --live --acknowledge-risk"
    @echo ""
    @echo "See: robson --help"
    @echo ""

# ============================================================================
# Utilities
# ============================================================================

# Show environment info
info:
    @echo "╔════════════════════════════════════════════════════════════╗"
    @echo "║              ROBSON DEVELOPMENT ENVIRONMENT                ║"
    @echo "╚════════════════════════════════════════════════════════════╝"
    @echo ""
    @echo "Python:"
    @python --version || echo "  ❌ Not found"
    @echo ""
    @echo "Node.js:"
    @node --version || echo "  ❌ Not found"
    @echo ""
    @echo "Go:"
    @go version || echo "  ❌ Not found"
    @echo ""
    @echo "Docker:"
    @docker --version || echo "  ❌ Not found"
    @echo ""
    @echo "Kubectl:"
    @kubectl version --client --short 2>/dev/null || echo "  ❌ Not found"
    @echo ""
    @echo "K9s:"
    @k9s version 2>/dev/null | head -n 1 || echo "  ❌ Not found"
    @echo ""
    @echo "CLI:"
    @if [ -f robson ] && [ -f cli/robson-go ]; then \
        echo "  ✅ Built (robson + robson-go)"; \
    else \
        echo "  ⚠️  Not built (run: make build-cli)"; \
    fi
    @echo ""
    @echo "Database:"
    @if docker compose -f apps/backend/monolith/docker-compose.dev.yml ps | grep -q "Up"; then \
        echo "  ✅ Running"; \
    else \
        echo "  ⚠️  Not running (run: just db-up)"; \
    fi
    @echo ""

# Quick health check
health: info
    @echo "Running quick health checks..."
    @echo ""
    @just test-cli
    @echo ""
    @echo "✅ Environment is healthy!"

# ============================================================================
# Robson v2 (Rust rewrite)
# ============================================================================

# v2: Build robsond binary
v2-build:
    @echo "🔨 Building robsond..."
    cd v2 && cargo build --release --bin robsond

# v2: Format code
v2-fmt:
    @echo "🎨 Formatting v2 code..."
    cd v2 && cargo fmt --all

# v2: Check compilation
v2-check:
    @echo "🔍 Checking v2 compilation..."
    cd v2 && cargo check --workspace -q

# v2: Database lifecycle (Podman-first)
v2-db-up:
    #!/usr/bin/env bash
    # Start ParadeDB or Postgres container for v2
    IMAGE="${PARADEDB_IMAGE:-ghcr.io/paradedb/paradedb:latest}"
    CONTAINER_NAME="robson-v2-db"

    # Check if already running
    if podman ps -q -f name=${CONTAINER_NAME} | grep -q .; then
        echo "Database already running"
        exit 0
    fi

    podman run -d \
        --name ${CONTAINER_NAME} \
        -p 5432:5432 \
        -e POSTGRES_USER=robson \
        -e POSTGRES_PASSWORD=robson \
        -e POSTGRES_DB=robson_v2 \
        ${IMAGE}

    # Wait for database to be ready
    echo "Waiting for database to be ready..."
    for i in {1..30}; do
        if podman exec -i ${CONTAINER_NAME} pg_isready -U robson >/dev/null 2>&1; then
            echo "Database is ready"
            exit 0
        fi
        sleep 1
    done
    echo "Failed to start database"
    exit 1

v2-db-down:
    podman rm -f robson-v2-db || true

v2-db-migrate:
    #!/usr/bin/env bash
    # Apply migration 002 for v2
    podman exec -i robson-v2-db psql -U robson -d robson_v2 < v2/migrations/002_event_log_phase9.sql

v2-db-psql:
    podman exec -it robson-v2-db psql -U robson -d robson_v2

# v2: Test projection worker (end-to-end)
v2-test-projection-worker:
    #!/usr/bin/env bash
    # End-to-end test: insert 2 events and run projection worker
    set -e

    TENANT_ID="$(podman exec -i robson-v2-db psql -U robson -d robson_v2 -t -c \
        "SELECT gen_random_uuid()::text" | tr -d ' ')"

    # Insert event 1: valid BALANCE_SAMPLED
    podman exec -i robson-v2-db psql -U robson -d robson_v2 -c \
        "INSERT INTO event_idempotency (tenant_id, idempotency_key, event_id) \
         VALUES ('$TENANT_ID'::uuid, 'balance-test-1', gen_random_uuid()) \
         ON CONFLICT (tenant_id, idempotency_key) DO NOTHING"

    podman exec -i robson-v2-db psql -U robson -d robson_v2 -c \
        "INSERT INTO event_log (tenant_id, stream_key, seq, event_type, payload, payload_schema_version, \
         occurred_at, ingested_at, idempotency_key, actor_type, actor_id, event_id) \
         SELECT '$TENANT_ID'::uuid, 'account:test', 1, 'BALANCE_SAMPLED', \
         jsonb_build_object('balance_id', gen_random_uuid(), 'tenant_id', '$TENANT_ID'::uuid, \
         'account_id', gen_random_uuid(), 'asset', 'USDT', 'free', '10000.00', 'locked', '0.00', 'sampled_at', NOW()), \
         1, NOW(), NOW(), 'balance-test-1', 'CLI', 'test-user', \
         (SELECT event_id FROM event_idempotency WHERE idempotency_key = 'balance-test-1' AND tenant_id = '$TENANT_ID'::uuid)"

    # Insert event 2: invalid POSITION_OPENED (stop_distance=0)
    podman exec -i robson-v2-db psql -U robson -d robson_v2 -c \
        "INSERT INTO event_idempotency (tenant_id, idempotency_key, event_id) \
         VALUES ('$TENANT_ID'::uuid, 'position-test-1', gen_random_uuid()) \
         ON CONFLICT (tenant_id, idempotency_key) DO NOTHING"

    podman exec -i robson-v2-db psql -U robson -d robson_v2 -c \
        "INSERT INTO event_log (tenant_id, stream_key, seq, event_type, payload, payload_schema_version, \
         occurred_at, ingested_at, idempotency_key, actor_type, actor_id, event_id) \
         SELECT '$TENANT_ID'::uuid, 'position:test', 2, 'POSITION_OPENED', \
         jsonb_build_object('position_id', gen_random_uuid(), 'tenant_id', '$TENANT_ID'::uuid, \
         'account_id', (SELECT (payload->>'account_id')::uuid FROM event_log WHERE seq = 1 AND tenant_id = '$TENANT_ID'::uuid), \
         'symbol', 'BTCUSDT', 'side', 'long', 'entry_price', '95000', 'entry_quantity', '0.1', \
         'technical_stop_price', '94000', 'technical_stop_distance', '0', 'entry_filled_at', NOW()), \
         1, NOW(), NOW(), 'position-test-1', 'CLI', 'test-user', \
         (SELECT event_id FROM event_idempotency WHERE idempotency_key = 'position-test-1' AND tenant_id = '$TENANT_ID'::uuid)"

    # Run projection worker briefly
    DATABASE_URL="postgresql://robson:robson@localhost:5432/robson_v2" \
    PROJECTION_TENANT_ID="$TENANT_ID" \
    PROJECTION_STREAM_KEY="account:test" \
    timeout 3 ./v2/target/release/robsond || true

    # Verification SQL outputs
    echo ""
    echo "=== Verification ==="
    echo ""
    echo "Event log counts:"
    podman exec -i robson-v2-db psql -U robson -d robson_v2 -t -c \
        "SELECT event_type, COUNT(*) FROM event_log WHERE tenant_id = '$TENANT_ID'::uuid GROUP BY event_type;"
    echo ""
    echo "Balances current:"
    podman exec -i robson-v2-db psql -U robson -d robson_v2 -t -c \
        "SELECT asset, free, locked, total FROM balances_current WHERE tenant_id = '$TENANT_ID'::uuid;"
    echo ""
    echo "Positions current (should be 0 - invariant blocked):"
    podman exec -i robson-v2-db psql -U robson -d robson_v2 -t -c \
        "SELECT COUNT(*) FROM positions_current WHERE tenant_id = '$TENANT_ID'::uuid;"

# v2: Test idempotency (regression test)
v2-test-idempotency:
    #!/usr/bin/env bash
    # Test global idempotency: inserting same event twice should not duplicate
    set -e

    TENANT_ID="$(podman exec -i robson-v2-db psql -U robson -d robson_v2 -t -c \
        "SELECT gen_random_uuid()::text" | tr -d ' ')"

    # First insert - should succeed
    echo "First insert (should succeed)..."
    podman exec -i robson-v2-db psql -U robson -d robson_v2 -c \
        "INSERT INTO event_idempotency (tenant_id, idempotency_key, event_id) \
         VALUES ('$TENANT_ID'::uuid, 'idempotency-test-1', gen_random_uuid()) \
         ON CONFLICT (tenant_id, idempotency_key) DO NOTHING"

    podman exec -i robson-v2-db psql -U robson -d robson_v2 -c \
        "INSERT INTO event_log (tenant_id, stream_key, seq, event_type, payload, payload_schema_version, \
         occurred_at, ingested_at, idempotency_key, actor_type, actor_id, event_id) \
         SELECT '$TENANT_ID'::uuid, 'account:idempotency-test', 1, 'BALANCE_SAMPLED', \
         jsonb_build_object('balance_id', gen_random_uuid(), 'tenant_id', '$TENANT_ID'::uuid, \
         'account_id', gen_random_uuid(), 'asset', 'USDT', 'free', '5000.00', 'locked', '0.00', 'sampled_at', NOW()), \
         1, NOW(), NOW(), 'idempotency-test-1', 'CLI', 'test-user', \
         (SELECT event_id FROM event_idempotency WHERE idempotency_key = 'idempotency-test-1' AND tenant_id = '$TENANT_ID'::uuid)"

    # Second insert - should be blocked by event_idempotency
    echo "Second insert (should be blocked by idempotency)..."
    RESULT=$(podman exec -i robson-v2-db psql -U robson -d robson_v2 -t -c \
        "INSERT INTO event_idempotency (tenant_id, idempotency_key, event_id) \
         VALUES ('$TENANT_ID'::uuid, 'idempotency-test-1', gen_random_uuid()) \
         ON CONFLICT (tenant_id, idempotency_key) DO NOTHING \
         RETURNING 'inserted'")

    if [ "$RESULT" = "inserted" ]; then
        echo "ERROR: Second insert should have been blocked!"
        exit 1
    fi

    # Verify only one row in event_log
    echo ""
    echo "=== Verification (should be 1 row) ==="
    podman exec -i robson-v2-db psql -U robson -d robson_v2 -t -c \
        "SELECT COUNT(*) FROM event_log WHERE tenant_id = '$TENANT_ID'::uuid AND idempotency_key = 'idempotency-test-1';"
