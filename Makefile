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
# AI Governance Validation
# ==============================

.PHONY: validate
validate:
	@echo "🔍 Validating AI Governance Framework..."
	@./.ai-agents/validate.sh
