#!/usr/bin/env bash
#
# ktop-ns.sh - Launch ktop scoped to a specific namespace
#
# Usage: ./ktop-ns.sh <namespace>
#
# This script validates that ktop is installed and launches it
# scoped to the provided namespace.
#
# Requirements:
# - ktop must be installed and available in PATH
# - KUBECONFIG must be set (or default ~/.kube/config configured)
# - The namespace should exist in the cluster (ktop will show error if not)

set -euo pipefail

# Color output helpers
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Function to print error messages
error() {
    echo -e "${RED}Error: $1${NC}" >&2
    exit 1
}

# Function to print info messages
info() {
    echo -e "${GREEN}$1${NC}"
}

# Function to print warning messages
warn() {
    echo -e "${YELLOW}$1${NC}"
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

# Check if ktop is installed
if ! command -v ktop &> /dev/null; then
    error "ktop is not installed or not in PATH.\nInstall ktop: https://github.com/vladimirvivien/ktop\nSee also: infra/KTOP-OPERATIONS.md"
fi

# Check if kubeconfig is accessible (optional check, kubectl not required)
if [ -n "${KUBECONFIG:-}" ]; then
    info "Using KUBECONFIG: $KUBECONFIG"
elif [ -f "$HOME/.kube/config" ]; then
    info "Using default kubeconfig: ~/.kube/config"
else
    warn "No KUBECONFIG set and ~/.kube/config not found. ktop may fail to connect."
fi

# Launch ktop scoped to the namespace
info "Launching ktop for namespace: $NAMESPACE"
ktop --namespace "$NAMESPACE"
