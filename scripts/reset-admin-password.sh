#!/bin/bash
#
# Reset Admin Password Script for Robson Production
#
# This script helps reset the admin user password in production Kubernetes environment.
#
# Usage:
#   ./scripts/reset-admin-password.sh [--username USERNAME] [--password PASSWORD] [--namespace NAMESPACE]
#
# Examples:
#   ./scripts/reset-admin-password.sh --username admin --password newpass123
#   ./scripts/reset-admin-password.sh --username admin  # Will prompt for password
#   ./scripts/reset-admin-password.sh --namespace staging --username admin
#

set -euo pipefail

# Default values
NAMESPACE="${NAMESPACE:-robson}"
USERNAME="${USERNAME:-admin}"
PASSWORD="${PASSWORD:-}"
CREATE_IF_NOT_EXISTS="${CREATE_IF_NOT_EXISTS:-false}"
MAKE_SUPERUSER="${MAKE_SUPERUSER:-false}"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Parse arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        --namespace)
            NAMESPACE="$2"
            shift 2
            ;;
        --username)
            USERNAME="$2"
            shift 2
            ;;
        --password)
            PASSWORD="$2"
            shift 2
            ;;
        --create-if-not-exists)
            CREATE_IF_NOT_EXISTS="true"
            shift
            ;;
        --make-superuser)
            MAKE_SUPERUSER="true"
            shift
            ;;
        --help)
            echo "Usage: $0 [OPTIONS]"
            echo ""
            echo "Options:"
            echo "  --namespace NAMESPACE     Kubernetes namespace (default: robson)"
            echo "  --username USERNAME       Username to reset password (default: admin)"
            echo "  --password PASSWORD       New password (if not provided, will prompt)"
            echo "  --create-if-not-exists    Create user if it doesn't exist"
            echo "  --make-superuser          Make user a superuser (only if creating)"
            echo "  --help                   Show this help message"
            echo ""
            exit 0
            ;;
        *)
            echo -e "${RED}Error: Unknown option: $1${NC}"
            echo "Use --help for usage information"
            exit 1
            ;;
    esac
done

# Function to print colored messages
info() {
    echo -e "${GREEN}[INFO]${NC} $1"
}

warn() {
    echo -e "${YELLOW}[WARN]${NC} $1"
}

error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

# Check if kubectl is available
if ! command -v kubectl &> /dev/null; then
    error "kubectl is not installed or not in PATH"
    exit 1
fi

# Get backend pod name
info "Finding backend pod in namespace: $NAMESPACE"
POD_NAME=$(kubectl get pods -n "$NAMESPACE" -l app=backend -o jsonpath='{.items[0].metadata.name}' 2>/dev/null || echo "")

if [ -z "$POD_NAME" ]; then
    # Try alternative label selector
    POD_NAME=$(kubectl get pods -n "$NAMESPACE" -l app=backend-monolith -o jsonpath='{.items[0].metadata.name}' 2>/dev/null || echo "")
fi

if [ -z "$POD_NAME" ]; then
    error "Could not find backend pod in namespace '$NAMESPACE'"
    error "Available pods:"
    kubectl get pods -n "$NAMESPACE" || true
    exit 1
fi

info "Found backend pod: $POD_NAME"

# Check if pod is running
POD_STATUS=$(kubectl get pod "$POD_NAME" -n "$NAMESPACE" -o jsonpath='{.status.phase}' 2>/dev/null || echo "")
if [ "$POD_STATUS" != "Running" ]; then
    warn "Pod status is: $POD_STATUS (expected: Running)"
    read -p "Continue anyway? (y/N) " -n 1 -r
    echo
    if [[ ! $REPLY =~ ^[Yy]$ ]]; then
        exit 1
    fi
fi

# Build command
CMD="python manage.py reset_admin_password --username $USERNAME"

if [ -n "$PASSWORD" ]; then
    CMD="$CMD --password '$PASSWORD'"
fi

if [ "$CREATE_IF_NOT_EXISTS" = "true" ]; then
    CMD="$CMD --create-if-not-exists"
fi

if [ "$MAKE_SUPERUSER" = "true" ]; then
    CMD="$CMD --make-superuser"
fi

# Execute command
info "Resetting password for user: $USERNAME"
info "Executing command in pod: $POD_NAME"

if kubectl exec -it -n "$NAMESPACE" "$POD_NAME" -- bash -c "$CMD"; then
    info "Password reset successful!"
    info ""
    info "You can now login with:"
    info "  Username: $USERNAME"
    if [ -n "$PASSWORD" ]; then
        info "  Password: $PASSWORD"
    else
        info "  Password: (the one you entered)"
    fi
else
    error "Password reset failed!"
    exit 1
fi

