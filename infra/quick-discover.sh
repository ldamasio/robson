#!/bin/bash
# Quick discovery script - finds backend pods and shows correct commands

NAMESPACE="${1:-robson}"

echo "🔍 Quick Discovery for namespace: $NAMESPACE"
echo ""

# List all pods
echo "All pods:"
kubectl get pods -n $NAMESPACE 2>/dev/null || { echo "❌ Namespace not found or no access"; exit 1; }

echo ""
echo "Deployments:"
kubectl get deployments -n $NAMESPACE 2>/dev/null

echo ""
echo "---"
echo "Backend pods (if any):"
BACKEND_POD=$(kubectl get pods -n $NAMESPACE 2>/dev/null | grep -i "backend\|monolith\|api" | head -1)

if [ -n "$BACKEND_POD" ]; then
    POD_NAME=$(echo "$BACKEND_POD" | awk '{print $1}')
    echo "$BACKEND_POD"
    echo ""
    echo "✅ Found: $POD_NAME"
    echo ""
    echo "Labels:"
    kubectl get pod $POD_NAME -n $NAMESPACE --show-labels 2>/dev/null
    echo ""
    echo "---"
    echo "💡 Use this in your commands:"
    echo "   export POD_NAME='$POD_NAME'"
    echo ""
    echo "Test it:"
    echo "   kubectl exec -n $NAMESPACE $POD_NAME -- python manage.py show_urls | grep trading-intents"
else
    echo "❌ No backend pods found"
fi
