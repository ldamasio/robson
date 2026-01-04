#!/bin/bash
#
# Find backend pods - discovers the correct labels and names
#

set -e

NAMESPACE="robson"

echo "=================================================="
echo "🔍 Finding Backend Pods in namespace: $NAMESPACE"
echo "=================================================="
echo ""

# Check if namespace exists
echo "1️⃣  Checking if namespace exists..."
if kubectl get namespace $NAMESPACE &>/dev/null; then
    echo "✅ Namespace '$NAMESPACE' exists"
else
    echo "❌ Namespace '$NAMESPACE' not found"
    echo ""
    echo "Available namespaces:"
    kubectl get namespaces
    exit 1
fi

echo ""
echo "2️⃣  Listing ALL pods in namespace $NAMESPACE..."
ALL_PODS=$(kubectl get pods -n $NAMESPACE --no-headers 2>/dev/null || echo "")

if [ -z "$ALL_PODS" ]; then
    echo "❌ No pods found in namespace $NAMESPACE"
    echo ""
    echo "Checking deployments in namespace:"
    kubectl get deployments -n $NAMESPACE
    exit 1
else
    echo "$ALL_PODS"
fi

echo ""
echo "3️⃣  Looking for backend-related pods..."
BACKEND_PODS=$(kubectl get pods -n $NAMESPACE --no-headers 2>/dev/null | grep -i "backend\|monolith\|rbs" || echo "")

if [ -z "$BACKEND_PODS" ]; then
    echo "⚠️  No backend pods found with keywords: backend, monolith, rbs"
else
    echo "$BACKEND_PODS"
    echo ""

    # Get first backend pod name
    FIRST_POD=$(echo "$BACKEND_PODS" | head -1 | awk '{print $1}')
    echo "4️⃣  Getting labels for pod: $FIRST_POD"
    kubectl get pod $FIRST_POD -n $NAMESPACE --show-labels

    echo ""
    echo "5️⃣  Detailed labels:"
    kubectl get pod $FIRST_POD -n $NAMESPACE -o jsonpath='{.metadata.labels}' | jq '.' 2>/dev/null || kubectl get pod $FIRST_POD -n $NAMESPACE -o jsonpath='{.metadata.labels}'
fi

echo ""
echo "6️⃣  Checking deployments..."
kubectl get deployments -n $NAMESPACE

echo ""
echo "7️⃣  Checking services..."
kubectl get svc -n $NAMESPACE

echo ""
echo "=================================================="
echo "✅ Discovery completed"
echo "=================================================="
echo ""
echo "To use the correct label in your commands:"
echo "  export POD_NAME=\$(kubectl get pods -n $NAMESPACE -l <CORRECT_LABEL> -o jsonpath='{.items[0].metadata.name}')"
echo ""
echo "Or use pod name directly:"
echo "  export POD_NAME=$FIRST_POD"
