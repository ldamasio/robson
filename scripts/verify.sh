#!/usr/bin/env bash
# Robson Rust workspace verification script
# Runs formatting, lint, compilation, and optional tests for the Rust workspace.
#
# Usage:
#   ./scripts/verify.sh          # Full verification
#   ./scripts/verify.sh --fast   # Skip tests
#
# Exit codes:
#   0 - All checks passed
#   1 - One or more checks failed

set -Eeuo pipefail

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Configuration
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

# Parse arguments
FAST_MODE=false

for arg in "$@"; do
    case "$arg" in
        --fast)
            FAST_MODE=true
            ;;
        --help)
            echo "Usage: $0 [--fast]"
            echo ""
            echo "Options:"
            echo "  --fast    Skip tests"
            echo "  --help    Show this help message"
            exit 0
            ;;
        *)
            echo -e "${RED}Unknown option: $arg${NC}"
            echo "Use --help for usage information"
            exit 1
            ;;
    esac
done

# Helper functions
print_header() {
    echo -e "\n${BLUE}==>${NC} ${1}"
}

print_success() {
    echo -e "${GREEN}✓${NC} ${1}"
}

print_error() {
    echo -e "${RED}✗${NC} ${1}"
}

print_warning() {
    echo -e "${YELLOW}⚠${NC} ${1}"
}

# Track overall success
ALL_PASSED=true

# Change to repository root
cd "${REPO_ROOT}"

# ============================================================================
# RUST VERIFICATION
# ============================================================================

verify_rust() {
    print_header "Verifying Rust workspace..."

    # 1. Check formatting
    print_header "Running rustfmt (check mode)..."
    if cargo fmt --all --check; then
        print_success "Rust formatting OK"
    else
        print_error "Rust formatting failed - run 'cargo fmt --all' to fix"
        ALL_PASSED=false
        return 1
    fi

    # 2. Run Clippy (strict mode)
    print_header "Running clippy (strict mode)..."
    # Note: -D warnings = deny warnings (treat as errors)
    # Note: --all-targets = check lib, bin, tests, examples
    if cargo clippy --all-targets --all-features -- -D warnings; then
        print_success "Clippy passed (no warnings)"
    else
        print_error "Clippy failed - fix warnings above"
        ALL_PASSED=false
        return 1
    fi

    # 3. Run tests (unless fast mode)
    if [ "$FAST_MODE" = false ]; then
        print_header "Running cargo test..."
        if cargo test --all --all-features; then
            print_success "All Rust tests passed"
        else
            print_error "Rust tests failed"
            ALL_PASSED=false
            return 1
        fi
    else
        print_warning "Skipping tests (fast mode enabled)"
    fi

    # 4. Check compilation (release mode, fast check)
    print_header "Checking release build..."
    if cargo check --release --all-targets; then
        print_success "Release build check passed"
    else
        print_error "Release build check failed"
        ALL_PASSED=false
        return 1
    fi

    print_success "Rust verification complete!"
}

# ============================================================================
# MAIN EXECUTION
# ============================================================================

print_header "Robson Rust Verification"
echo "Mode: $([ "$FAST_MODE" = true ] && echo "FAST" || echo "FULL")"
echo "Root: ${REPO_ROOT}"
echo ""

verify_rust || true

# Final summary
echo ""
echo "========================================"
if [ "$ALL_PASSED" = true ]; then
    print_success "All verifications passed! ✨"
    exit 0
else
    print_error "Some verifications failed!"
    echo ""
    echo "Quick fixes:"
    echo "  - Format: cargo fmt --all"
    echo "  - Lint: cargo clippy --fix --allow-dirty"
    exit 1
fi
