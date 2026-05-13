#!/usr/bin/env bash
# Claude Code Stop Hook
# Runs full validation when ending a Claude Code session
#
# This hook runs COMPLETE verification (format, lint, tests)
# to ensure all changes are production-ready
#
# Environment variables:
#   CLAUDE_HOOK_DISABLED=1  - Disable this hook
#   CLAUDE_HOOK_FAST=1      - Skip tests, only format/lint
#
# Exit codes:
#   0 - Validation passed
#   1 - Validation failed (shows errors but doesn't block)

set -e

# Check if hook is disabled
if [ "${CLAUDE_HOOK_DISABLED}" = "1" ]; then
    exit 0
fi

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

# Configuration
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
V2_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"

echo ""
echo -e "${BLUE}═══════════════════════════════════════════════════${NC}"
echo -e "${BLUE}    Robson v2 - Session Validation${NC}"
echo -e "${BLUE}═══════════════════════════════════════════════════${NC}"
echo ""

# Change to v2 root
cd "${V2_ROOT}"

# Determine mode (fast or full)
VERIFY_FLAGS=""
if [ "${CLAUDE_HOOK_FAST}" = "1" ]; then
    VERIFY_FLAGS="--fast"
    echo -e "${YELLOW}→ Running FAST validation (no tests)${NC}"
else
    echo -e "${BLUE}→ Running FULL validation (format, lint, tests)${NC}"
fi

# Run verification script
if [ -f "./scripts/verify.sh" ]; then
    if ./scripts/verify.sh $VERIFY_FLAGS; then
        echo ""
        echo -e "${GREEN}╔════════════════════════════════════════════╗${NC}"
        echo -e "${GREEN}║  ✓ All validations passed!                ║${NC}"
        echo -e "${GREEN}║  Your changes are ready to commit.        ║${NC}"
        echo -e "${GREEN}╚════════════════════════════════════════════╝${NC}"
        echo ""
        exit 0
    else
        echo ""
        echo -e "${RED}╔════════════════════════════════════════════╗${NC}"
        echo -e "${RED}║  ✗ Validation failed!                      ║${NC}"
        echo -e "${RED}║  Please fix errors before committing.     ║${NC}"
        echo -e "${RED}╚════════════════════════════════════════════╝${NC}"
        echo ""
        echo -e "${YELLOW}Quick fixes:${NC}"
        echo "  Format:  cargo fmt --all"
        echo "  Lint:    cargo clippy --fix --allow-dirty"
        echo "  Tests:   cargo test --all"
        echo ""
        echo -e "${YELLOW}To disable validation:${NC}"
        echo "  export CLAUDE_HOOK_DISABLED=1"
        echo ""
        exit 1
    fi
else
    echo -e "${YELLOW}⚠ Verification script not found${NC}"
    echo "Expected: ./scripts/verify.sh"
    exit 0
fi
