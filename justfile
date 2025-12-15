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
[confirm("⚠️  This will DELETE all local database data. Continue?")]
db-destroy:
    @echo "🗑️  Destroying database..."
    docker compose -f apps/backend/monolith/docker-compose.dev.yml down -v
    @echo "✅ Database destroyed"

# Reset database: destroy + recreate + migrate (REQUIRES CONFIRMATION)
[confirm("⚠️  This will RESET the database and REMOVE migrations. Continue?")]
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
