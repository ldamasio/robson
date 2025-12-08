#!/usr/bin/env bash
#
# k9s-ns.sh - Launch K9s scoped to a specific namespace
#
# Usage: ./k9s-ns.sh <namespace>
#
# This script validates that K9s is installed and launches it
# scoped to the provided namespace.
#
# Requirements:
# - K9s must be installed and available in PATH
# - KUBECONFIG must be set (or default ~/.kube/config configured)
# - The namespace should exist in the cluster (K9s will show error if not)

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

# Check if namespace argument is provided
if [ $# -eq 0 ]; then
    error "Namespace argument is required.\nUsage: $0 <namespace>"
fi

NAMESPACE="$1"

# Validate that namespace is not empty
if [ -z "$NAMESPACE" ]; then
    error "Namespace cannot be empty.\nUsage: $0 <namespace>"
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

# Launch K9s scoped to the namespace
info "Launching K9s for namespace: $NAMESPACE"
k9s --namespace "$NAMESPACE"
