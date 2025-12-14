#!/bin/bash
#
# Smoke tests for Robson CLI
# Tests basic functionality of both the C router and Go implementation
#

set -e

GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

TESTS_PASSED=0
TESTS_FAILED=0

# Helper function to run a test
run_test() {
    local test_name="$1"
    local test_command="$2"
    local expected_pattern="$3"

    echo -n "Test: $test_name ... "

    if OUTPUT=$(eval "$test_command" 2>&1); then
        if [[ -z "$expected_pattern" ]] || echo "$OUTPUT" | grep -q "$expected_pattern"; then
            echo -e "${GREEN}✓ PASSED${NC}"
            ((TESTS_PASSED++))
            return 0
        else
            echo -e "${RED}✗ FAILED${NC} (output didn't match expected pattern)"
            echo "  Expected pattern: $expected_pattern"
            echo "  Got: $OUTPUT"
            ((TESTS_FAILED++))
            return 1
        fi
    else
        echo -e "${RED}✗ FAILED${NC} (command failed)"
        echo "  Command: $test_command"
        echo "  Output: $OUTPUT"
        ((TESTS_FAILED++))
        return 1
    fi
}

echo "╔════════════════════════════════════════════════════════════╗"
echo "║           Robson CLI Smoke Tests                         ║"
echo "╚════════════════════════════════════════════════════════════╝"
echo ""

# Check if binaries exist
if [ ! -f "./robson-go" ]; then
    echo -e "${RED}Error: robson-go binary not found${NC}"
    echo "Run 'make build' first"
    exit 1
fi

echo "Testing robson-go (Go implementation)..."
echo "─────────────────────────────────────"

run_test "Help command" \
    "./robson-go help" \
    "Robson - Cryptocurrency Trading CLI"

run_test "Version command" \
    "./robson-go version" \
    "robson-go version"

run_test "Say command" \
    "./robson-go say Hello World" \
    "Robson says: Hello World"

run_test "Report command" \
    "./robson-go report" \
    "TRADING REPORT"

run_test "Buy command (no args)" \
    "./robson-go buy" \
    "BUY ORDER"

run_test "Sell command (no args)" \
    "./robson-go sell" \
    "SELL ORDER"

echo ""
echo "Testing agentic workflow..."
echo "─────────────────────────────────────"

run_test "Plan creation" \
    "./robson-go plan buy BTCUSDT 0.001" \
    "EXECUTION PLAN"

# Extract plan ID from plan output
PLAN_OUTPUT=$(./robson-go plan buy BTCUSDT 0.001 --json)
if command -v jq &> /dev/null; then
    PLAN_ID=$(echo "$PLAN_OUTPUT" | jq -r '.planID')

    run_test "Plan validation" \
        "./robson-go validate $PLAN_ID" \
        "PLAN VALIDATION"

    run_test "Plan execution" \
        "./robson-go execute $PLAN_ID" \
        "PLAN EXECUTION"
else
    echo -e "${YELLOW}⚠ Skipping validation/execution tests (jq not installed)${NC}"
fi

echo ""
echo "Testing JSON output..."
echo "─────────────────────────────────────"

if command -v jq &> /dev/null; then
    run_test "JSON help output" \
        "./robson-go help --json | jq -e '.command == \"help\"'" \
        ""

    run_test "JSON plan output" \
        "./robson-go plan buy BTCUSDT 0.001 --json | jq -e '.strategy == \"buy\"'" \
        ""
else
    echo -e "${YELLOW}⚠ Skipping JSON tests (jq not installed)${NC}"
fi

echo ""
echo "Testing C router (if available)..."
echo "─────────────────────────────────────"

if [ -f "../robson" ]; then
    run_test "C router: --help flag translation" \
        "../robson --help" \
        "Robson - Cryptocurrency Trading CLI"

    run_test "C router: direct help subcommand" \
        "../robson help" \
        "Robson - Cryptocurrency Trading CLI"

    run_test "C router: --say flag translation" \
        "../robson --say Testing" \
        "Robson says: Testing"
else
    echo -e "${YELLOW}⚠ C router (../robson) not found, skipping router tests${NC}"
    echo "  Run 'make build-all' to build the C router"
fi

echo ""
echo "╔════════════════════════════════════════════════════════════╗"
echo "║                    Test Summary                           ║"
echo "╚════════════════════════════════════════════════════════════╝"
echo ""
echo -e "  ${GREEN}Passed: $TESTS_PASSED${NC}"
echo -e "  ${RED}Failed: $TESTS_FAILED${NC}"
echo ""

if [ $TESTS_FAILED -eq 0 ]; then
    echo -e "${GREEN}✓ All tests passed!${NC}"
    exit 0
else
    echo -e "${RED}✗ Some tests failed${NC}"
    exit 1
fi
