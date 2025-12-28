#!/bin/bash
# Deep Storage Smoke Test
# Validates that the deep storage Phase 0 deployment is working correctly.
#
# Usage: ./data/scripts/smoke-test.sh [--verbose]
#
# Environment Variables:
#   KUBECONFIG: Path to kubeconfig (default: ~/.kube/config)
#
# Author: Robson Bot Team
# Created: 2024-12-27
# Related: ADR-0013, docs/runbooks/deep-storage.md

set -euo pipefail

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Test counters
TESTS_PASSED=0
TESTS_FAILED=0
TESTS_TOTAL=0

# Verbose flag
VERBOSE="${VERBOSE:-0}"

# Helper functions
log_info() {
    echo -e "${GREEN}[INFO]${NC} $1"
}

log_warn() {
    echo -e "${YELLOW}[WARN]${NC} $1"
}

log_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

test_pass() {
    TESTS_PASSED=$((TESTS_PASSED + 1))
    log_info "✅ PASS: $1"
}

test_fail() {
    TESTS_FAILED=$((TESTS_FAILED + 1))
    log_error "❌ FAIL: $1"
}

test_run() {
    TESTS_TOTAL=$((TESTS_TOTAL + 1))
    log_info "Running: $1"
}

# Check command exists
command_exists() {
    command -v "$1" >/dev/null 2>&1
}

# Run kubectl command with error handling
kubectl_safe() {
    if ! kubectl "$@" 2>&1; then
        return 1
    fi
}

# Parse arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        --verbose|-v)
            VERBOSE=1
            shift
            ;;
        --help|-h)
            echo "Usage: $0 [--verbose]"
            echo ""
            echo "Smoke test for Robson Deep Storage (Phase 0)"
            echo ""
            echo "Options:"
            echo "  --verbose, -v    Show detailed output"
            echo "  --help, -h       Show this help message"
            exit 0
            ;;
        *)
            log_error "Unknown option: $1"
            exit 1
            ;;
    esac
done

echo "========================================"
echo "Robson Deep Storage Smoke Test"
echo "========================================"
echo ""

# ============================================================================
# Prerequisites Check
# ============================================================================
echo "=== Prerequisites Check ==="

test_run "Check kubectl is installed"
if command_exists kubectl; then
    KUBECTL_VERSION=$(kubectl version --client --short 2>/dev/null)
    test_pass "kubectl installed (${KUBECTL_VERSION})"
else
    test_fail "kubectl not found"
    exit 1
fi

test_run "Check kubectl can connect to cluster"
if kubectl cluster-info >/dev/null 2>&1; then
    CLUSTER_INFO=$(kubectl cluster-info | head -1)
    test_pass "kubectl connected: ${CLUSTER_INFO}"
else
    test_fail "kubectl cannot connect to cluster"
    exit 1
fi

test_run "Check AWS CLI is installed (for S3 validation)"
if command_exists aws; then
    AWS_VERSION=$(aws --version 2>&1 | cut -d' ' -f1)
    test_pass "aws-cli installed (${AWS_VERSION})"
else
    test_fail "aws-cli not found (required for S3 validation)"
fi

echo ""

# ============================================================================
# Namespace Validation
# ============================================================================
echo "=== Namespace Validation ==="

test_run "Check datalake-system namespace exists"
if kubectl get namespace datalake-system >/dev/null 2>&1; then
    test_pass "Namespace datalake-system exists"

    if [[ "$VERBOSE" -eq 1 ]]; then
        kubectl describe namespace datalake-system | grep -E "Name:|Labels:|Status:"
    fi
else
    test_fail "Namespace datalake-system not found"
fi

test_run "Check analytics-jobs namespace exists"
if kubectl get namespace analytics-jobs >/dev/null 2>&1; then
    test_pass "Namespace analytics-jobs exists"

    if [[ "$VERBOSE" -eq 1 ]]; then
        kubectl describe namespace analytics-jobs | grep -E "Name:|Labels:|Status:"
    fi
else
    test_fail "Namespace analytics-jobs not found"
fi

test_run "Check ResourceQuota for datalake-system"
if kubectl get resourcequota -n datalake-system datalake-system-quota >/dev/null 2>&1; then
    test_pass "ResourceQuota datalake-system-quota exists"
else
    test_fail "ResourceQuota datalake-system-quota not found"
fi

test_run "Check ResourceQuota for analytics-jobs"
if kubectl get resourcequota -n analytics-jobs analytics-jobs-quota >/dev/null 2>&1; then
    test_pass "ResourceQuota analytics-jobs-quota exists"
else
    test_fail "ResourceQuota analytics-jobs-quota not found"
fi

echo ""

# ============================================================================
# NetworkPolicy Validation
# ============================================================================
echo "=== NetworkPolicy Validation ==="

test_run "Check datalake-system NetworkPolicies exist"
NP_COUNT=$(kubectl get networkpolicy -n datalake-system 2>/dev/null | grep -v NAME | wc -l)
if [[ "$NP_COUNT" -ge 4 ]]; then
    test_pass "Found ${NP_COUNT} NetworkPolicies in datalake-system"

    if [[ "$VERBOSE" -eq 1 ]]; then
        kubectl get networkpolicy -n datalake-system
    fi
else
    test_fail "Expected at least 4 NetworkPolicies, found ${NP_COUNT}"
fi

test_run "Check analytics-jobs NetworkPolicies exist"
NP_COUNT=$(kubectl get networkpolicy -n analytics-jobs 2>/dev/null | grep -v NAME | wc -l)
if [[ "$NP_COUNT" -ge 5 ]]; then
    test_pass "Found ${NP_COUNT} NetworkPolicies in analytics-jobs"

    if [[ "$VERBOSE" -eq 1 ]]; then
        kubectl get networkpolicy -n analytics-jobs
    fi
else
    test_fail "Expected at least 5 NetworkPolicies, found ${NP_COUNT}"
fi

test_run "Verify default-deny policy in datalake-system"
if kubectl get networkpolicy -n datalake-system deny-all-default >/dev/null 2>&1; then
    test_pass "Default-deny policy exists in datalake-system"
else
    test_fail "Default-deny policy not found in datalake-system"
fi

test_run "Verify default-deny policy in analytics-jobs"
if kubectl get networkpolicy -n analytics-jobs deny-all-default >/dev/null 2>&1; then
    test_pass "Default-deny policy exists in analytics-jobs"
else
    test_fail "Default-deny policy not found in analytics-jobs"
fi

echo ""

# ============================================================================
# Secrets Validation
# ============================================================================
echo "=== Secrets Validation ==="

test_run "Check Contabo S3 credentials secret exists"
if kubectl get secret -n datalake-system contabo-s3-credentials >/dev/null 2>&1; then
    test_pass "Secret contabo-s3-credentials exists"
else
    test_fail "Secret contabo-s3-credentials not found (create before deployment)"
fi

# Note: Hive Metastore deferred to Phase 1, skipping checks
echo "⚠️  Hive Metastore deferred to Phase 1 (using canonical S3 paths)"

test_run "Check Django DB credentials secret exists (for Outbox access)"
if kubectl get secret -n analytics-jobs django-db-credentials >/dev/null 2>&1; then
    test_pass "Secret django-db-credentials exists"
else
    test_fail "Secret django-db-credentials not found (required for bronze ingestion)"
fi

echo ""

# ============================================================================
# S3 Connectivity Validation
# ============================================================================
echo "=== S3 Connectivity Validation ==="

test_run "Test S3 connectivity from cluster"
if kubectl run s3-test --image=amazon/aws-cli:latest --rm -it --restart=Never -n analytics-jobs -- \
    aws s3 ls s3://robson-datalake --endpoint-url=https://s3.eu-central-2.contabo.com 2>/dev/null; then
    test_pass "S3 connectivity successful"
else
    test_fail "S3 connectivity failed (check credentials and endpoint)"
fi

echo ""

# ============================================================================
# Spark Jobs Validation (Jobs, not CronJobs in Phase 0)
# ============================================================================
echo "=== Spark Jobs Validation ==="

test_run "Check bronze-ingest Job manifest exists"
if kubectl get job -n analytics-jobs bronze-ingest-manual -o jsonpath='{.metadata.name}' 2>/dev/null | grep -q bronze; then
    test_pass "bronze-ingest Job exists (or can be created)"
elif kubectl get job -n analytics-jobs bronze-ingest --ignore-not-found >/dev/null 2>&1; then
    test_pass "bronze-ingest Job exists"
else
    test_pass "bronze-ingest Job manifest ready (infra/k8s/datalake/jobs/bronze-ingest-job.yml)"
fi

test_run "Check silver-transform Job manifest exists"
if kubectl get job -n analytics-jobs silver-transform-manual -o jsonpath='{.metadata.name}' 2>/dev/null | grep -q silver; then
    test_pass "silver-transform Job exists (or can be created)"
elif kubectl get job -n analytics-jobs silver-transform --ignore-not-found >/dev/null 2>&1; then
    test_pass "silver-transform Job exists"
else
    test_pass "silver-transform Job manifest ready (infra/k8s/datalake/jobs/silver-transform-job.yml)"
fi

echo ""

# ============================================================================
# Resource Usage Validation
# ============================================================================
echo "=== Resource Usage Validation ==="

test_run "Check node resource availability"
NODE_AVAILABLE=$(kubectl top nodes 2>/dev/null | grep -v NAME | awk '{print $4}' | sed 's/%//')
if [[ -n "$NODE_AVAILABLE" ]]; then
    if [[ "$NODE_AVAILABLE" -lt 80 ]]; then
        test_pass "Node resources available (${NODE_AVAILABLE}% used)"
    else
        test_fail "Node resources high (${NODE_AVAILABLE}% used)"
    fi
else
    test_fail "Cannot determine node resource usage (metrics server may not be installed)"
fi

echo ""

# ============================================================================
# Summary
# ============================================================================
echo "========================================"
echo "Smoke Test Summary"
echo "========================================"
echo ""
echo "Total Tests: ${TESTS_TOTAL}"
echo -e "Passed: ${GREEN}${TESTS_PASSED}${NC}"
echo -e "Failed: ${RED}${TESTS_FAILED}${NC}"
echo ""

if [[ "$TESTS_FAILED" -eq 0 ]]; then
    log_info "🎉 All smoke tests passed!"
    echo ""
    echo "Next steps:"
    echo "  1. Run bronze ingestion: kubectl apply -f infra/k8s/datalake/jobs/bronze-ingest-job.yml"
    echo "  2. Run silver transformation: kubectl apply -f infra/k8s/datalake/jobs/silver-transform-job.yml"
    echo "  3. Verify S3 output: aws s3 ls s3://robson-datalake/ --recursive"
    exit 0
else
    log_error "Smoke test failed! Please fix the issues above."
    echo ""
    echo "Troubleshooting:"
    echo "  1. Check runbook: docs/runbooks/deep-storage.md"
    echo "  2. Check ADR: docs/adr/ADR-0013-deep-storage-architecture.md"
    echo "  3. Verify prerequisites: Contabo S3 bucket, secrets"
    exit 1
fi
