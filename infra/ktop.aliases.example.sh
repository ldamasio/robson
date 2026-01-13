#!/usr/bin/env bash
#
# ktop.aliases.example.sh - Example shell aliases for ktop operations
#
# USAGE:
#   This file provides convenient shell aliases for ktop operations.
#   It is NOT sourced automatically - you must explicitly source it
#   in your shell configuration (.bashrc, .zshrc, etc.) or manually
#   in your current shell session.
#
#   To use:
#   1. Copy this file to your home directory (optional):
#      cp infra/ktop.aliases.example.sh ~/.ktop-aliases.sh
#
#   2. Source it in your shell config:
#      echo "source ~/.ktop-aliases.sh" >> ~/.bashrc  # or ~/.zshrc
#
#   Or source it directly from the repo (assumes you're in repo root):
#      source infra/ktop.aliases.example.sh
#
# REQUIREMENTS:
#   - ktop must be installed and in PATH
#   - KUBECONFIG must be set (or default ~/.kube/config configured)
#   - Helper scripts must exist: infra/scripts/ktop-ns.sh, infra/scripts/ktop-preview.sh
#
# CUSTOMIZATION:
#   Feel free to modify these aliases to suit your workflow.
#   For example, if you frequently work with specific namespaces,
#   you can add aliases like:
#     alias ktop-backend='ktop --namespace robson-backend'
#     alias ktop-frontend='ktop --namespace robson-frontend'

# ==============================
# Generic ktop Aliases
# ==============================

# Launch ktop with current kubeconfig context
alias ktop-robson='ktop'

# Launch ktop showing all namespaces
alias ktop-all='ktop --all-namespaces'

# ==============================
# Namespace-Specific Aliases
# ==============================

# Launch ktop for a specific namespace using the helper script
# Usage: ktop-namespace <namespace-name>
ktop-namespace() {
    if [ $# -eq 0 ]; then
        echo "Usage: ktop-namespace <namespace-name>"
        return 1
    fi
    # Assumes repo root or PATH includes infra/scripts/
    if [ -f "./infra/scripts/ktop-ns.sh" ]; then
        ./infra/scripts/ktop-ns.sh "$1"
    elif command -v ktop-ns.sh &> /dev/null; then
        ktop-ns.sh "$1"
    else
        echo "Error: ktop-ns.sh not found. Ensure you're in the repo root or add scripts to PATH."
        return 1
    fi
}

# Shorthand alias for namespace function
alias ktopns='ktop-namespace'

# ==============================
# Preview Environment Aliases
# ==============================

# Launch ktop for a preview environment (per-branch namespace)
# Usage: ktop-preview-branch <branch-name>
# Example: ktop-preview-branch feature/stop-loss-orders
ktop-preview-branch() {
    if [ $# -eq 0 ]; then
        echo "Usage: ktop-preview-branch <branch-name>"
        return 1
    fi
    # Assumes repo root or PATH includes infra/scripts/
    if [ -f "./infra/scripts/ktop-preview.sh" ]; then
        ./infra/scripts/ktop-preview.sh "$1"
    elif command -v ktop-preview.sh &> /dev/null; then
        ktop-preview.sh "$1"
    else
        echo "Error: ktop-preview.sh not found. Ensure you're in the repo root or add scripts to PATH."
        return 1
    fi
}

# Shorthand alias for preview function
alias ktopprev='ktop-preview-branch'

# ==============================
# Common Namespace Quick Access
# ==============================

# CUSTOMIZE THESE ALIASES FOR YOUR ENVIRONMENT
# If you have common namespace names, add quick aliases here:

# Example: Robson production namespace
# alias ktop-prod='ktop --namespace robson'

# Example: Istio system namespace
# alias ktop-istio='ktop --namespace istio-system'

# Example: ArgoCD namespace
# alias ktop-argocd='ktop --namespace argocd'

# ==============================
# Helpful Utilities
# ==============================

# Show all namespaces and let user select one for ktop
# (Requires kubectl and fzf for interactive selection)
ktop-select-namespace() {
    if ! command -v kubectl &> /dev/null; then
        echo "Error: kubectl is not installed."
        return 1
    fi
    if ! command -v fzf &> /dev/null; then
        echo "Error: fzf is not installed. Install it for interactive selection."
        echo "Fallback: Use 'ktop-namespace <namespace>' directly."
        return 1
    fi
    SELECTED_NS=$(kubectl get namespaces -o jsonpath='{.items[*].metadata.name}' | tr ' ' '\n' | fzf --prompt="Select namespace for ktop: ")
    if [ -n "$SELECTED_NS" ]; then
        echo "Launching ktop for namespace: $SELECTED_NS"
        ktop --namespace "$SELECTED_NS"
    else
        echo "No namespace selected."
    fi
}

# Alias for namespace selector
alias ktopsel='ktop-select-namespace'

# ==============================
# End of Aliases
# ==============================

echo "ktop aliases loaded. Available commands:"
echo "  - ktop-robson                  : Launch ktop with current context"
echo "  - ktop-all                     : Launch ktop for all namespaces"
echo "  - ktop-namespace <ns>          : Launch ktop for specific namespace"
echo "  - ktopns <ns>                  : Shorthand for ktop-namespace"
echo "  - ktop-preview-branch <branch> : Launch ktop for preview environment"
echo "  - ktopprev <branch>            : Shorthand for ktop-preview-branch"
echo "  - ktop-select-namespace        : Interactive namespace selection (requires fzf)"
echo "  - ktopsel                      : Shorthand for ktop-select-namespace"
echo ""
echo "See infra/KTOP-OPERATIONS.md for full documentation."
