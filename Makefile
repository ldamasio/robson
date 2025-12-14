BINANCE_DOCS_REPO ?= https://github.com/binance/binance-spot-api-docs
BINANCE_DOCS_DIR  ?= docs/vendor/binance-spot
BINANCE_DOCS_REF  ?= master

.PHONY: sync-binance-docs
sync-binance-docs:
	@mkdir -p $$(dirname "$(BINANCE_DOCS_DIR)")
	@if git config --file .gitmodules --name-only --get-regexp '^submodule\.$(BINANCE_DOCS_DIR)\.path$$' >/dev/null 2>&1; then \
		echo "Submodule already registered: updating..."; \
		git submodule sync --recursive; \
		git submodule update --init --recursive; \
	else \
		echo "Registering submodule at $(BINANCE_DOCS_DIR) ..."; \
		git submodule add "$(BINANCE_DOCS_REPO)" "$(BINANCE_DOCS_DIR)" || true; \
		git submodule update --init --recursive; \
	fi; \
	echo "Pinning to $(BINANCE_DOCS_REF) ..."; \
	git -C "$(BINANCE_DOCS_DIR)" fetch --all --tags --prune; \
	git -C "$(BINANCE_DOCS_DIR)" checkout --quiet "$(BINANCE_DOCS_REF)"; \
	git -C "$(BINANCE_DOCS_DIR)" pull --ff-only || true; \
	echo "✓ Binance docs ready at $(BINANCE_DOCS_DIR)"

# ==============================
# CLI Build (C Router + Go)
# ==============================

CLI_DIR      ?= cli
CLI_BIN_C    ?= robson
CLI_BIN_GO   ?= robson-go
INSTALL_PATH ?= /usr/local/bin

.PHONY: build-cli build-c build-go clean-cli install-cli test-cli

build-cli: build-c build-go
	@echo "✓ CLI built successfully"
	@echo "  - C router: $(CLI_BIN_C)"
	@echo "  - Go CLI:   $(CLI_DIR)/$(CLI_BIN_GO)"
	@echo ""
	@echo "Next steps:"
	@echo "  1. Test: make test-cli"
	@echo "  2. Install: make install-cli"

build-c:
	@echo "Building C router..."
	@gcc -o $(CLI_BIN_C) main.c
	@echo "✓ C router built: $(CLI_BIN_C)"

build-go:
	@echo "Building Go CLI..."
	@cd $(CLI_DIR) && go mod download
	@cd $(CLI_DIR) && go build -o $(CLI_BIN_GO) .
	@echo "✓ Go CLI built: $(CLI_DIR)/$(CLI_BIN_GO)"

clean-cli:
	@echo "Cleaning CLI binaries..."
	@rm -f $(CLI_BIN_C)
	@rm -f $(CLI_DIR)/$(CLI_BIN_GO)
	@echo "✓ CLI binaries removed"

install-cli: build-cli
	@echo "Installing CLI to $(INSTALL_PATH)..."
	@sudo cp $(CLI_BIN_C) $(INSTALL_PATH)/$(CLI_BIN_C)
	@sudo cp $(CLI_DIR)/$(CLI_BIN_GO) $(INSTALL_PATH)/$(CLI_BIN_GO)
	@sudo chmod +x $(INSTALL_PATH)/$(CLI_BIN_C)
	@sudo chmod +x $(INSTALL_PATH)/$(CLI_BIN_GO)
	@echo "✓ CLI installed to $(INSTALL_PATH)"
	@echo ""
	@echo "Verify installation:"
	@echo "  robson --help"

test-cli: build-cli
	@echo "Running CLI smoke tests..."
	@cd $(CLI_DIR) && ./smoke-test.sh
	@echo "✓ CLI smoke tests passed"

# ==============================
# Dev helpers (Django + Postgres)
# ==============================

MONO_DIR ?= apps/backend/monolith
DC_DEV   ?= $(MONO_DIR)/docker-compose.dev.yml

.PHONY: dev-db-up dev-db-down dev-db-destroy dev-db-logs dev-makemigrations dev-migrate dev-test

dev-db-up:
	@docker compose -f $(DC_DEV) up -d
	@echo "✓ Postgres dev up (localhost:5432, db=robson_dev user=robson)"

dev-db-down:
	@docker compose -f $(DC_DEV) down
	@echo "✓ Postgres dev stopped"

dev-db-destroy:
	@docker compose -f $(DC_DEV) down -v
	@echo "✓ Postgres dev removed (including volume)"

dev-db-logs:
	@docker compose -f $(DC_DEV) logs -f

dev-makemigrations:
	@cd $(MONO_DIR) && python manage.py makemigrations api

dev-migrate:
	@cd $(MONO_DIR) && python manage.py migrate

dev-test:
	@cd $(MONO_DIR) && python manage.py test api.tests.test_models -v 2

# Reset dev DB and api migrations (clean slate)
.PHONY: dev-reset-api
dev-reset-api:
	@echo "⚠️  This will DROP the dev database volume and REMOVE api migrations (except __init__.py)."
	@echo "    Proceeding for a clean slate aligned with current models..."
	@docker compose -f $(DC_DEV) down -v
	@docker compose -f $(DC_DEV) up -d
	@echo "🧹 Removing api migrations (except __init__.py) ..."
	@find $(MONO_DIR)/api/migrations -type f -name "*.py" ! -name "__init__.py" -delete || true
	@find $(MONO_DIR)/api/migrations -type d -name "__pycache__" -exec rm -rf {} + || true
	@echo "🧱 Rebuilding migrations ..."
	@cd $(MONO_DIR) && python manage.py makemigrations api
	@cd $(MONO_DIR) && python manage.py migrate
	@echo "✓ Reset complete. You can now run: make dev-test"

# ==============================
# K9s Helpers (Kubernetes Terminal UI)
# ==============================

.PHONY: k9s k9s-ns k9s-preview

k9s:
	@echo "🚀 Launching K9s with current kubeconfig context..."
	@echo "   (Ensure KUBECONFIG is set or ~/.kube/config is configured)"
	@k9s

k9s-ns:
	@if [ -z "$(NAMESPACE)" ]; then \
		echo "❌ Error: NAMESPACE is required."; \
		echo "   Usage: make k9s-ns NAMESPACE=<name>"; \
		exit 1; \
	fi
	@echo "🚀 Launching K9s for namespace: $(NAMESPACE)"
	@./infra/scripts/k9s-ns.sh "$(NAMESPACE)"

k9s-preview:
	@if [ -z "$(BRANCH)" ]; then \
		echo "❌ Error: BRANCH is required."; \
		echo "   Usage: make k9s-preview BRANCH=<branch-name>"; \
		exit 1; \
	fi
	@echo "🚀 Launching K9s for preview environment: $(BRANCH)"
	@./infra/scripts/k9s-preview.sh "$(BRANCH)"

# ==============================
# AI Governance Validation
# ==============================

.PHONY: validate
validate:
	@echo "🔍 Validating AI Governance Framework..."
	@./.ai-agents/validate.sh
