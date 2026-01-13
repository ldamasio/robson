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

dev-reseed:
	@cd $(MONO_DIR) && python manage.py seed_production_data
	@echo "✓ Database re-seeded with production-like data"

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
# ktop Helpers (Kubernetes top-style Monitor)
# ==============================

.PHONY: ktop ktop-ns ktop-preview

ktop:
	@echo "🚀 Launching ktop with current kubeconfig context..."
	@echo "   (Ensure KUBECONFIG is set or ~/.kube/config is configured)"
	@ktop

ktop-ns:
	@if [ -z "$(NAMESPACE)" ]; then \
		echo "❌ Error: NAMESPACE is required."; \
		echo "   Usage: make ktop-ns NAMESPACE=<name>"; \
		exit 1; \
	fi
	@echo "🚀 Launching ktop for namespace: $(NAMESPACE)"
	@./infra/scripts/ktop-ns.sh "$(NAMESPACE)"

ktop-preview:
	@if [ -z "$(BRANCH)" ]; then \
		echo "❌ Error: BRANCH is required."; \
		echo "   Usage: make ktop-preview BRANCH=<branch-name>"; \
		exit 1; \
	fi
	@echo "🚀 Launching ktop for preview environment: $(BRANCH)"
	@./infra/scripts/ktop-preview.sh "$(BRANCH)"

# ==============================
# AI Governance Validation
# ==============================

.PHONY: validate
validate:
	@echo "🔍 Validating AI Governance Framework..."
	@./.ai-agents/validate.sh

# ==============================
# Code Quality (Pre-commit)
# ==============================

.PHONY: pre-commit-install pre-commit-run pre-commit-update pre-commit-all \
        format-python format-go format-js lint-python lint-go lint-js \
        quality-all

# Install pre-commit hooks locally
pre-commit-install:
	@echo "Installing pre-commit hooks..."
	@pre-commit install
	@pre-commit install --hook-type commit-msg
	@echo "✓ Pre-commit hooks installed"
	@echo ""
	@echo "Hooks will run automatically on: git commit"
	@echo "Run manually: make pre-commit-run"

# Run pre-commit on all files
pre-commit-run:
	@echo "Running pre-commit hooks on all files..."
	@pre-commit run --all-files

# Update pre-commit hook versions
pre-commit-update:
	@echo "Updating pre-commit hook versions..."
	@pre-commit autoupdate
	@echo "✓ Hooks updated (review .pre-commit-config.yaml)"

# Run all pre-commit checks (alias)
pre-commit-all: pre-commit-run

# Format Python code
format-python:
	@echo "Formatting Python code..."
	@cd apps/backend/monolith && black . && isort .
	@echo "✓ Python code formatted"

# Format Go code
format-go:
	@echo "Formatting Go code..."
	@gofmt -w cli/
	@echo "✓ Go code formatted"

# Format JS/JSX code
format-js:
	@echo "Formatting JS/JSX code..."
	@cd apps/frontend && npx prettier --write "src/**/*.{js,jsx,json,css}"
	@echo "✓ JS/JSX code formatted"

# Lint Python code
lint-python:
	@echo "Linting Python code..."
	@ruff check apps/backend/
	@echo "✓ Python lint passed"

# Lint Go code
lint-go:
	@echo "Linting Go code..."
	@gofmt -l cli/ | read || echo "✓ Go lint passed (no formatting needed)"

# Lint JS/JSX code
lint-js:
	@echo "Linting JS/JSX code..."
	@cd apps/frontend && npx eslint src/
	@echo "✓ JS/JSX lint passed"

# Run all quality checks
quality-all: format-python format-go format-js lint-python lint-go lint-js
	@echo "✓ All quality checks passed"

# ==============================
# Deep Storage / Data Lake
# ==============================

DATA_DIR ?= data
PYTHON ?= python3
SPARK_HOME ?= /opt/spark  # or set SPARK_HOME environment variable
DATE ?= $(shell date +%Y-%m-%d)

# Spark Image Build Configuration
SPARK_IMAGE_DIR ?= infra/images/spark
SPARK_IMAGE_NAME ?= ghcr.io/ldamasio/rbs-spark
SPARK_IMAGE_TAG ?= 3.5.0-phase0
SPARK_FULL_IMAGE ?= $(SPARK_IMAGE_NAME):$(SPARK_IMAGE_TAG)

.PHONY: spark-image-build spark-image-push spark-image-build-push datalake-deploy datalake-deploy-namespaces datalake-deploy-policies \
        datalake-run-bronze datalake-run-silver datalake-run-gold \
        datalake-smoke-test datalake-rollback datalake-status datalake-clean

# Build custom Spark image with baked-in dependencies
spark-image-build:
	@echo "🏗️  Building custom Spark image: $(SPARK_FULL_IMAGE)"
	@echo "   Dockerfile: $(SPARK_IMAGE_DIR)/Dockerfile"
	docker build -t $(SPARK_FULL_IMAGE) $(SPARK_IMAGE_DIR)/
	@echo "✅ Spark image built: $(SPARK_FULL_IMAGE)"
	@echo ""
	@echo "Verify with:"
	@echo "  docker run --rm $(SPARK_FULL_IMAGE) spark-submit --version"
	@echo ""
	@echo "Next: make spark-image-push"

# Push Spark image to registry
spark-image-push:
	@echo "📤 Pushing Spark image to registry: $(SPARK_FULL_IMAGE)"
	docker push $(SPARK_FULL_IMAGE)
	@echo "✅ Spark image pushed: $(SPARK_FULL_IMAGE)"
	@echo ""
	@echo "Image available at: https://github.com/ldamasio?tab=packages"
	@echo ""
	@echo "Next: Update job manifests to use: $(SPARK_FULL_IMAGE)"

# Build and push Spark image (combined)
spark-image-build-push: spark-image-build
	@make spark-image-push

# Deploy deep storage infrastructure (namespaces, network policies)
datalake-deploy: datalake-deploy-namespaces datalake-deploy-policies
	@echo "✅ Deep storage infrastructure deployed"

datalake-deploy-namespaces:
	@echo "Deploying datalake namespaces..."
	kubectl apply -f infra/k8s/datalake/namespaces/
	@echo "✅ Namespaces created: datalake-system, analytics-jobs"

datalake-deploy-policies:
	@echo "Deploying datalake network policies..."
	kubectl apply -f infra/k8s/datalake/network-policies/
	@echo "✅ Network policies applied"

# Run bronze ingestion job (Django → S3)
datalake-run-bronze:
	@echo "Running bronze ingestion for $(DATE)..."
	kubectl create job bronze-manual-$(shell date +%s) \
		-n analytics-jobs \
		--from=cronjob/bronze-ingest \
		--dry-run=client -o yaml | kubectl apply -f -
	@echo "✅ Bronze job started"

# Run silver transformation job (bronze → silver)
datalake-run-silver:
	@echo "Running silver transformation for $(DATE)..."
	kubectl create job silver-manual-$(shell date +%s) \
		-n analytics-jobs \
		--from=cronjob/silver-transform \
		--dry-run=client -o yaml | kubectl apply -f -
	@echo "✅ Silver job started"

# Run gold feature generation job (silver → gold)
datalake-run-gold:
	@echo "Running gold feature generation for $(DATE)..."
	kubectl create job gold-manual-$(shell date +%s) \
		-n analytics-jobs \
		--from=cronjob/gold-features \
		--dry-run=client -o yaml | kubectl apply -f -
	@echo "✅ Gold job started"

# Smoke test: validate deep storage deployment
datalake-smoke-test:
	@echo "Running deep storage smoke test..."
	@echo "1. Checking namespaces..."
	@kubectl get ns datalake-system analytics-jobs
	@echo "2. Checking network policies..."
	@kubectl get networkpolicies -n datalake-system
	@kubectl get networkpolicies -n analytics-jobs
	@echo "3. Checking Hive Metastore..."
	@kubectl get pods -n datalake-system -l app=hive-metastore
	@echo "4. Checking recent jobs..."
	@kubectl get jobs -n analytics-jobs --sort-by=.metadata.creationTimestamp | tail -5
	@echo "✅ Smoke test passed"

# Rollback: delete deep storage infrastructure
datalake-rollback:
	@echo "⚠️  WARNING: This will delete all deep storage infrastructure"
	@echo "Press Ctrl+C to cancel, or wait 5 seconds to continue..."
	@sleep 5
	kubectl delete namespace analytics-jobs datalake-system
	@echo "✅ Deep storage infrastructure removed"

# Status: check deep storage health
datalake-status:
	@echo "=== Deep Storage Status ==="
	@echo "Namespaces:"
	@kubectl get ns datalake-system analytics-jobs
	@echo ""
	@echo "Hive Metastore:"
	@kubectl get pods -n datalake-system -l app=hive-metastore
	@echo ""
	@echo "Recent Jobs:"
	@kubectl get jobs -n analytics-jobs --sort-by=.metadata.creationTimestamp | tail -5
	@echo ""
	@echo "Pod Resources:"
	@kubectl top pods -n datalake-system 2>/dev/null || echo "  (metrics not available)"
	@kubectl top pods -n analytics-jobs 2>/dev/null || echo "  (metrics not available)"

# Clean: remove failed jobs and old data
datalake-clean:
	@echo "Cleaning failed jobs..."
	kubectl delete jobs -n analytics-jobs --field-selector status.failed=true
	@echo "✅ Failed jobs removed"
	@echo "⚠️  Note: S3 data cleanup must be done manually via AWS CLI"
