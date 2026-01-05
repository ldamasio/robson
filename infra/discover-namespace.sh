#!/bin/bash
# Discover available namespaces and find the backend

echo "=================================================="
echo "🔍 Kubernetes Namespace & Cluster Discovery"
echo "=================================================="
echo ""

# Check kubectl connectivity
echo "1️⃣  Testing kubectl connectivity..."
if ! kubectl cluster-info &>/dev/null; then
    echo "❌ Cannot connect to Kubernetes cluster"
    echo ""
    echo "Possible issues:"
    echo "  - Cluster is not running"
    echo "  - KUBECONFIG not set correctly"
    echo "  - VPN/network issue"
    echo ""
    echo "Current KUBECONFIG:"
    echo "  ${KUBECONFIG:-~/.kube/config}"
    echo ""
    echo "Try:"
    echo "  export KUBECONFIG=~/.kube/config"
    echo "  or"
    echo "  export KUBECONFIG=~/.kube/config-rbx"
    exit 1
fi

echo "✅ Connected to cluster"
echo ""

# Get current context
echo "2️⃣  Current context:"
kubectl config current-context
echo ""

# List all namespaces
echo "3️⃣  Available namespaces:"
kubectl get namespaces -o custom-columns=NAME:.metadata.name,STATUS:.status.phase,AGE:.metadata.creationTimestamp
echo ""

# Find backend-related namespaces
echo "4️⃣  Backend-related namespaces:"
BACKEND_NS=$(kubectl get namespaces --no-headers 2>/dev/null | grep -iE "robson|backend|prod|production|api" | awk '{print $1}')

if [ -z "$BACKEND_NS" ]; then
    echo "⚠️  No obvious backend namespace found"
    echo ""
    echo "Please identify the correct namespace from the list above."
else
    echo "$BACKEND_NS"
    echo ""

    # For each backend namespace, check for pods
    for ns in $BACKEND_NS; do
        echo "---"
        echo "Checking namespace: $ns"
        POD_COUNT=$(kubectl get pods -n $ns --no-headers 2>/dev/null | wc -l)
        echo "  Pods: $POD_COUNT"

        if [ $POD_COUNT -gt 0 ]; then
            echo "  Deployments:"
            kubectl get deployments -n $ns --no-headers 2>/dev/null | awk '{print "    - " $1}'

            # Check for backend pods
            BACKEND_POD=$(kubectl get pods -n $ns --no-headers 2>/dev/null | grep -iE "backend|monolith|api" | head -1)
            if [ -n "$BACKEND_POD" ]; then
                POD_NAME=$(echo "$BACKEND_POD" | awk '{print $1}')
                echo "  ✅ Backend pod found: $POD_NAME"
                echo ""
                echo "  💡 Use this namespace:"
                echo "     export NAMESPACE='$ns'"
                echo "     export POD_NAME='$POD_NAME'"
                echo ""
                echo "  Test it:"
                echo "     kubectl exec -n $ns $POD_NAME -- python manage.py --version"
            fi
        fi
    done
fi

echo ""
echo "=================================================="
echo "✅ Discovery completed"
echo "=================================================="
