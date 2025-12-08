#!/usr/bin/env bash
#
# k9s-preview.sh - Launch K9s for a per-branch preview environment
#
# Usage: ./k9s-preview.sh <branch-name>
#
# This script computes the preview namespace from the branch name
# using the same normalization rules as the ArgoCD ApplicationSet:
# - Lowercase the branch name
# - Replace '/' and '_' with '-'
# - Prepend 'h-' prefix
#
# Example:
#   Branch: feature/stop-loss-orders
#   Namespace: h-feature-stop-loss-orders
#
# Requirements:
# - K9s must be installed and available in PATH
# - KUBECONFIG must be set (or default ~/.kube/config configured)
# - The preview namespace should exist (created by ArgoCD ApplicationSet)

set -euo pipefail

# Color output helpers
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Function to print error messages
error() {
    echo -e "${RED}❌ Error: $1${NC}" >&2
    exit 1
}

# Function to print info messages
info() {
    echo -e "${GREEN}ℹ️  $1${NC}"
}

# Function to print warning messages
warn() {
    echo -e "${YELLOW}⚠️  $1${NC}"
}

# Check if branch argument is provided
if [ $# -eq 0 ]; then
    error "Branch name argument is required.\nUsage: $0 <branch-name>"
fi

BRANCH="$1"

# Validate that branch is not empty
if [ -z "$BRANCH" ]; then
    error "Branch name cannot be empty.\nUsage: $0 <branch-name>"
fi

# Check if K9s is installed
if ! command -v k9s &> /dev/null; then
    error "K9s is not installed or not in PATH.\nInstall K9s: https://k9scli.io/topics/install/\nSee also: infra/K9S-OPERATIONS.md"
fi

# Check if kubeconfig is accessible (optional check, kubectl not required)
if [ -n "${KUBECONFIG:-}" ]; then
    info "Using KUBECONFIG: $KUBECONFIG"
elif [ -f "$HOME/.kube/config" ]; then
    info "Using default kubeconfig: ~/.kube/config"
else
    warn "No KUBECONFIG set and ~/.kube/config not found. K9s may fail to connect."
fi

# Normalize branch name to namespace format
# Rules (from infra/README.md ApplicationSet):
# - Lowercase
# - Replace '/' with '-'
# - Replace '_' with '-'
NORMALIZED_BRANCH=$(echo "$BRANCH" | tr '[:upper:]' '[:lower:]' | tr '/_' '--')

# Compute preview namespace (prefix: h-)
PREVIEW_NAMESPACE="h-${NORMALIZED_BRANCH}"

# Display resolved namespace
info "Branch: $BRANCH"
info "Preview Namespace: $PREVIEW_NAMESPACE"
echo ""

# Launch K9s scoped to the preview namespace
info "Launching K9s for preview environment..."
k9s --namespace "$PREVIEW_NAMESPACE"
