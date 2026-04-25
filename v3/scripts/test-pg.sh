#!/usr/bin/env bash
# test-pg.sh — Run Postgres-backed integration tests for Robson v2
#
# DATABASE_URL must be set in the environment before calling this script.
# How you obtain it depends on the environment:
#
#   Local dev:   just v2-db-up  (provisions container via IaC-equivalent justfile)
#                DATABASE_URL is then exported by the calling justfile target.
#
#   CI:          DATABASE_URL set as CI secret / env var (e.g. GitHub Actions secret).
#
#   Staging:     DATABASE_URL from Ansible vault (rbx-infra bootstrap output).
#
# This script does NOT provision databases, start containers, or resolve credentials.
# That is the responsibility of the infrastructure layer (rbx-infra Ansible or local IaC).
#
# Usage:
#   DATABASE_URL=postgres://... bash scripts/test-pg.sh
#   DATABASE_URL=postgres://... bash scripts/test-pg.sh --test crash_recovery
#
# What runs:
#   cargo test --features postgres -- --ignored [extra args]
#
# How sqlx::test works:
#   Each test receives an isolated PgPool. sqlx::test:
#     1. Connects to the server at DATABASE_URL
#     2. Creates a temporary database per test (needs CREATEDB privilege on the user)
#     3. Runs all migrations from v2/migrations/ automatically
#     4. Runs the test
#     5. Drops the temporary database
#   No manual migration step is needed. No persistent state is left behind.
#
# WARNING: NEVER point DATABASE_URL at a production database.
#          sqlx::test creates and drops databases on the target server.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
V2_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

info()    { echo -e "${BLUE}==>${NC} $*"; }
success() { echo -e "${GREEN}✓${NC} $*"; }
warn()    { echo -e "${YELLOW}⚠${NC} $*"; }
fail()    { echo -e "${RED}✗${NC} $*" >&2; }

# ─── Require DATABASE_URL ──────────────────────────────────────────────────────
if [[ -z "${DATABASE_URL:-}" ]]; then
    fail "DATABASE_URL is not set."
    echo ""
    echo "DATABASE_URL must be provided by the infrastructure layer:"
    echo ""
    echo "  Local dev (Podman container):"
    echo "    just v2-db-up    # provisions test database"
    echo "    just v2-test-pg  # sets DATABASE_URL and runs this script"
    echo ""
    echo "  CI:"
    echo "    Set DATABASE_URL as a CI environment variable / secret."
    echo "    The database server must be provisioned before the test step."
    echo ""
    echo "  Staging / production-equivalent:"
    echo "    DATABASE_URL comes from rbx-infra Ansible vault output."
    echo "    Never run integration tests against the live production database."
    echo ""
    echo "  Manual:"
    echo "    DATABASE_URL='postgresql://user:pass@host/dbname' bash scripts/test-pg.sh"
    echo "    The user must have CREATEDB privilege (sqlx::test creates per-test databases)."
    echo ""
    echo "See: v2/README.md — 'PostgreSQL Integration Tests'"
    exit 1
fi

# ─── Safety guard ─────────────────────────────────────────────────────────────
# Refuse URLs with known production naming patterns.
# Extend this list with your org's conventions if needed.
_url_check="${DATABASE_URL}"
for _pattern in "prod" "production" "live"; do
    if [[ "${_url_check}" == *"${_pattern}"* ]]; then
        fail "DATABASE_URL contains '${_pattern}' — refusing to run against a production database."
        fail "sqlx::test creates and drops databases on the target server."
        fail "Use a local or staging database only."
        exit 1
    fi
done

# ─── Run tests ─────────────────────────────────────────────────────────────────
cd "${V2_ROOT}"

echo ""
info "Running Postgres integration tests..."
info "  Feature  : --features postgres"
info "  Filter   : --ignored  (all tests that require DATABASE_URL)"
info "  Extra    : ${*:-none}"
echo ""
warn "sqlx::test creates temporary databases on the server — expected, safe, ephemeral."
echo ""

cargo test --features postgres -- --ignored "$@"

echo ""
success "All Postgres integration tests passed."
echo ""
echo "Coverage:"
echo "  robsond/crash_recovery   — daemon restores positions from projection on restart"
echo "  robsond/replay_test      — queries_current rebuilds byte-for-byte from event log"
echo "  robsond/daemon           — restart invalidates AwaitingApproval queries"
echo "  robsond/projection_worker — checkpoint persists across worker restart"
echo "  robson-store             — PgProjectionReader restores all position states"
echo "  robson-projector         — event handlers materialize projections correctly"
