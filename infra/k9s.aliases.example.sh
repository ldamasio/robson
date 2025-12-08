#!/usr/bin/env bash
#
# k9s.aliases.example.sh - Example shell aliases for K9s operations
#
# USAGE:
#   This file provides convenient shell aliases for K9s operations.
#   It is NOT sourced automatically - you must explicitly source it
#   in your shell configuration (.bashrc, .zshrc, etc.) or manually
#   in your current shell session.
#
#   To use:
#   1. Copy this file to your home directory (optional):
#      cp infra/k9s.aliases.example.sh ~/.k9s-aliases.sh
#
#   2. Source it in your shell config:
#      echo "source ~/.k9s-aliases.sh" >> ~/.bashrc  # or ~/.zshrc
#
#   Or source it directly from the repo (assumes you're in repo root):
#      source infra/k9s.aliases.example.sh
#
# REQUIREMENTS:
#   - K9s must be installed and in PATH
#   - KUBECONFIG must be set (or default ~/.kube/config configured)
#   - Helper scripts must exist: infra/scripts/k9s-ns.sh, infra/scripts/k9s-preview.sh
#
# CUSTOMIZATION:
#   Feel free to modify these aliases to suit your workflow.
#   For example, if you frequently work with specific namespaces,
#   you can add aliases like:
#     alias k9s-backend='k9s --namespace robson-backend'
#     alias k9s-frontend='k9s --namespace robson-frontend'

# ==============================
# Generic K9s Aliases
# ==============================

# Launch K9s with current kubeconfig context
alias k9s-robson='k9s'

# Alternative: if 'k9s' is not already defined, define it
# (Uncomment if needed)
# alias k9s='k9s'

# ==============================
# Namespace-Specific Aliases
# ==============================

# Launch K9s for a specific namespace using the helper script
# Usage: k9s-namespace <namespace-name>
k9s-namespace() {
    if [ $# -eq 0 ]; then
        echo "Usage: k9s-namespace <namespace-name>"
        return 1
    fi
    # Assumes repo root or PATH includes infra/scripts/
    if [ -f "./infra/scripts/k9s-ns.sh" ]; then
        ./infra/scripts/k9s-ns.sh "$1"
    elif command -v k9s-ns.sh &> /dev/null; then
        k9s-ns.sh "$1"
    else
        echo "Error: k9s-ns.sh not found. Ensure you're in the repo root or add scripts to PATH."
        return 1
    fi
}

# Shorthand alias for namespace function
alias k9sns='k9s-namespace'

# ==============================
# Preview Environment Aliases
# ==============================

# Launch K9s for a preview environment (per-branch namespace)
# Usage: k9s-preview-branch <branch-name>
# Example: k9s-preview-branch feature/stop-loss-orders
k9s-preview-branch() {
    if [ $# -eq 0 ]; then
        echo "Usage: k9s-preview-branch <branch-name>"
        return 1
    fi
    # Assumes repo root or PATH includes infra/scripts/
    if [ -f "./infra/scripts/k9s-preview.sh" ]; then
        ./infra/scripts/k9s-preview.sh "$1"
    elif command -v k9s-preview.sh &> /dev/null; then
        k9s-preview.sh "$1"
    else
        echo "Error: k9s-preview.sh not found. Ensure you're in the repo root or add scripts to PATH."
        return 1
    fi
}

# Shorthand alias for preview function
alias k9sprev='k9s-preview-branch'

# ==============================
# Common Namespace Quick Access
# ==============================

# CUSTOMIZE THESE ALIASES FOR YOUR ENVIRONMENT
# If you have common namespace names, add quick aliases here:

# Example: ArgoCD namespace
# alias k9s-argocd='k9s --namespace argocd'

# Example: Istio system namespace
# alias k9s-istio='k9s --namespace istio-system'

# Example: Robson backend namespace (adjust name if different)
# alias k9s-backend='k9s --namespace robson-backend'

# Example: Robson frontend namespace (adjust name if different)
# alias k9s-frontend='k9s --namespace robson-frontend'

# ==============================
# Helpful Utilities
# ==============================

# Show all namespaces and let user select one for K9s
# (Requires kubectl and fzf for interactive selection)
k9s-select-namespace() {
    if ! command -v kubectl &> /dev/null; then
        echo "Error: kubectl is not installed."
        return 1
    fi
    if ! command -v fzf &> /dev/null; then
        echo "Error: fzf is not installed. Install it for interactive selection."
        echo "Fallback: Use 'k9s-namespace <namespace>' directly."
        return 1
    fi
    SELECTED_NS=$(kubectl get namespaces -o jsonpath='{.items[*].metadata.name}' | tr ' ' '\n' | fzf --prompt="Select namespace: ")
    if [ -n "$SELECTED_NS" ]; then
        echo "Launching K9s for namespace: $SELECTED_NS"
        k9s --namespace "$SELECTED_NS"
    else
        echo "No namespace selected."
    fi
}

# Alias for namespace selector
alias k9ssel='k9s-select-namespace'

# ==============================
# End of Aliases
# ==============================

echo "✓ K9s aliases loaded. Available commands:"
echo "  - k9s-robson                  : Launch K9s with current context"
echo "  - k9s-namespace <ns>          : Launch K9s for specific namespace"
echo "  - k9sns <ns>                  : Shorthand for k9s-namespace"
echo "  - k9s-preview-branch <branch> : Launch K9s for preview environment"
echo "  - k9sprev <branch>            : Shorthand for k9s-preview-branch"
echo "  - k9s-select-namespace        : Interactive namespace selection (requires fzf)"
echo "  - k9ssel                      : Shorthand for k9s-select-namespace"
echo ""
echo "See infra/K9S-OPERATIONS.md for full documentation."
