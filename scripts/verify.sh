#!/usr/bin/env bash
# Robson v2 Verification Script
# Runs all quality checks (format, lint, test) for Rust workspace and CLI
#
# Usage:
#   ./scripts/verify.sh          # Full verification (all checks)
#   ./scripts/verify.sh --fast   # Fast mode (skip tests, only format/lint)
#   ./scripts/verify.sh --rust   # Rust only
#   ./scripts/verify.sh --cli    # CLI only
#
# Exit codes:
#   0 - All checks passed
#   1 - One or more checks failed

set -e  # Exit on first error

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Configuration
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
V2_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
CLI_DIR="${V2_ROOT}/cli"

# Parse arguments
FAST_MODE=false
RUST_ONLY=false
CLI_ONLY=false

for arg in "$@"; do
    case $arg in
        --fast)
            FAST_MODE=true
            shift
            ;;
        --rust)
            RUST_ONLY=true
            shift
            ;;
        --cli)
            CLI_ONLY=true
            shift
            ;;
        --help)
            echo "Usage: $0 [--fast] [--rust] [--cli]"
            echo ""
            echo "Options:"
            echo "  --fast    Skip tests, only run format/lint checks"
            echo "  --rust    Only verify Rust workspace"
            echo "  --cli     Only verify CLI (TypeScript)"
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

# Change to v2 root directory
cd "${V2_ROOT}"

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
# CLI VERIFICATION (Bun + TypeScript)
# ============================================================================

verify_cli() {
    print_header "Verifying CLI (Bun + TypeScript)..."

    # Check if CLI directory exists
    if [ ! -d "${CLI_DIR}" ]; then
        print_warning "CLI directory not found at ${CLI_DIR}, skipping CLI verification"
        return 0
    fi

    cd "${CLI_DIR}"

    # 1. Check if bun is installed
    if ! command -v bun &> /dev/null; then
        print_error "Bun is not installed - install from https://bun.sh"
        ALL_PASSED=false
        return 1
    fi

    # 2. Install dependencies (if needed)
    if [ ! -d "node_modules" ]; then
        print_header "Installing CLI dependencies..."
        if bun install; then
            print_success "CLI dependencies installed"
        else
            print_error "Failed to install CLI dependencies"
            ALL_PASSED=false
            return 1
        fi
    fi

    # 3. TypeScript type checking
    print_header "Running TypeScript type check..."
    if bun run tsc --noEmit; then
        print_success "TypeScript type check passed"
    else
        print_error "TypeScript type check failed"
        ALL_PASSED=false
        return 1
    fi

    # 4. Run tests (if they exist and not in fast mode)
    if [ "$FAST_MODE" = false ]; then
        # Check if test script exists in package.json
        if bun run test --version &> /dev/null 2>&1 || grep -q '"test"' package.json; then
            print_header "Running CLI tests..."
            if bun test; then
                print_success "CLI tests passed"
            else
                print_error "CLI tests failed"
                ALL_PASSED=false
                return 1
            fi
        else
            print_warning "No test script found, skipping CLI tests"
        fi
    else
        print_warning "Skipping tests (fast mode enabled)"
    fi

    # 5. Build check
    print_header "Checking CLI build..."
    if bun run build; then
        print_success "CLI build passed"
    else
        print_error "CLI build failed"
        ALL_PASSED=false
        return 1
    fi

    cd "${V2_ROOT}"
    print_success "CLI verification complete!"
}

# ============================================================================
# MAIN EXECUTION
# ============================================================================

print_header "Robson v2 Verification"
echo "Mode: $([ "$FAST_MODE" = true ] && echo "FAST" || echo "FULL")"
echo "Root: ${V2_ROOT}"
echo ""

# Run verifications based on flags
if [ "$CLI_ONLY" = true ]; then
    verify_cli || true
elif [ "$RUST_ONLY" = true ]; then
    verify_rust || true
else
    # Run both
    verify_rust || true
    verify_cli || true
fi

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
    echo "  - TypeScript: cd cli && bun run tsc --noEmit"
    exit 1
fi
