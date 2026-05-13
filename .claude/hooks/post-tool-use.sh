#!/usr/bin/env bash
# Claude Code Post-Tool-Use Hook
# Runs after Write/Edit operations to validate code quality
#
# This hook runs FAST checks only (no tests) to keep the feedback loop tight
# For full validation, see the 'stop' hook
#
# Environment variables:
#   CLAUDE_HOOK_FAST=1      - Enable fast mode (format + lint only, no tests)
#   CLAUDE_HOOK_DISABLED=1  - Disable this hook entirely
#
# Exit codes:
#   0 - Validation passed (continues session)
#   1 - Validation failed (blocks with error message)

set -e

# Check if hook is disabled
if [ "${CLAUDE_HOOK_DISABLED}" = "1" ]; then
    exit 0
fi

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

# Configuration
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
V2_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"

# Get tool information from environment
TOOL_NAME="${CLAUDE_TOOL_NAME:-unknown}"
FILE_PATH="${CLAUDE_FILE_PATH:-}"

# Only run validation for Write/Edit operations on Rust files
if [[ "$TOOL_NAME" != "Write" ]] && [[ "$TOOL_NAME" != "Edit" ]]; then
    exit 0
fi

# Only validate Rust files (.rs) and TypeScript files (.ts, .tsx)
if [[ -n "$FILE_PATH" ]]; then
    if [[ ! "$FILE_PATH" =~ \.(rs|ts|tsx)$ ]]; then
        exit 0  # Not a Rust/TS file, skip validation
    fi
fi

# Run fast validation (format check only, no tests)
cd "${V2_ROOT}"

# Determine what to validate based on file type
if [[ "$FILE_PATH" =~ \.rs$ ]]; then
    # Rust file - run rustfmt check only (very fast)
    echo -e "${YELLOW}→ Validating Rust formatting...${NC}"

    if cargo fmt --all --check &> /dev/null; then
        echo -e "${GREEN}✓ Rust formatting OK${NC}"
        exit 0
    else
        echo -e "${RED}✗ Rust formatting failed${NC}"
        echo ""
        echo "Quick fix: Run 'cargo fmt --all' to format code"
        echo ""
        echo "To disable this hook, set: export CLAUDE_HOOK_DISABLED=1"
        exit 1
    fi
elif [[ "$FILE_PATH" =~ \.(ts|tsx)$ ]]; then
    # TypeScript file - run type check
    echo -e "${YELLOW}→ Validating TypeScript types...${NC}"

    cd "${V2_ROOT}/cli"

    if bun run tsc --noEmit &> /dev/null; then
        echo -e "${GREEN}✓ TypeScript types OK${NC}"
        exit 0
    else
        echo -e "${RED}✗ TypeScript type errors found${NC}"
        echo ""
        echo "Run 'cd cli && bun run tsc --noEmit' to see errors"
        echo ""
        echo "To disable this hook, set: export CLAUDE_HOOK_DISABLED=1"
        exit 1
    fi
fi

# If we get here, validation passed
exit 0
