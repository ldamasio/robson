#!/bin/bash
#
# Debug script for 404 error on /api/trading-intents/create/ in production
#
# This script helps diagnose why the endpoint returns 404 in production
# but works correctly in development.
#

set -e

echo "=================================================="
echo "🔍 Debugging /api/trading-intents/create/ 404"
echo "=================================================="
echo ""

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Configuration (can be overridden with environment variables)
NAMESPACE="${NAMESPACE:-robson}"
DEPLOYMENT="${DEPLOYMENT:-rbs-backend-monolith-prod-deploy}"
APP_LABEL="${APP_LABEL:-app=rbs-backend-monolith}"

# Step 1: Check current deployment image
echo "1️⃣  Checking current deployment in production..."
echo ""

if command -v kubectl &> /dev/null && kubectl cluster-info &> /dev/null; then
    echo "✅ kubectl is available and cluster is reachable"

    # Get current image
    CURRENT_IMAGE=$(kubectl get deployment $DEPLOYMENT -n $NAMESPACE -o jsonpath='{.spec.template.spec.containers[0].image}' 2>/dev/null || echo "NOT_FOUND")

    if [ "$CURRENT_IMAGE" != "NOT_FOUND" ]; then
        echo -e "${GREEN}Current image:${NC} $CURRENT_IMAGE"

        # Extract SHA
        IMAGE_SHA=$(echo $CURRENT_IMAGE | grep -oP 'sha-\K[a-f0-9]+' || echo "unknown")
        echo -e "${GREEN}Image SHA:${NC} $IMAGE_SHA"

        # Check if this matches latest commit
        LATEST_COMMIT=$(git rev-parse HEAD | cut -c1-7)
        echo -e "${GREEN}Latest commit:${NC} $LATEST_COMMIT"

        if [[ "$CURRENT_IMAGE" == *"$LATEST_COMMIT"* ]]; then
            echo -e "${GREEN}✅ Deployment is up to date!${NC}"
        else
            echo -e "${YELLOW}⚠️  Deployment may be outdated${NC}"
            echo "   Current: sha-$IMAGE_SHA"
            echo "   Latest:  sha-$LATEST_COMMIT"
        fi
    else
        echo -e "${RED}❌ Could not find deployment $DEPLOYMENT in namespace $NAMESPACE${NC}"
    fi

    echo ""
    echo "2️⃣  Checking pod status..."
    kubectl get pods -n $NAMESPACE -l $APP_LABEL -o wide

    echo ""
    echo "3️⃣  Checking pod logs for URL registration..."
    POD_NAME=$(kubectl get pods -n $NAMESPACE -l $APP_LABEL -o jsonpath='{.items[0].metadata.name}' 2>/dev/null || echo "NOT_FOUND")

    if [ "$POD_NAME" != "NOT_FOUND" ]; then
        echo -e "${BLUE}Pod:${NC} $POD_NAME"
        echo ""
        echo "Looking for trading-intent URL registration..."
        kubectl logs $POD_NAME -n $NAMESPACE --tail=200 | grep -i "trading.intent\|TRADING_INTENT" || echo "⚠️  No trading-intent logs found in pod startup"
    fi

    echo ""
    echo "4️⃣  Checking Traefik IngressRoute configuration..."
    echo ""

    # Check for IngressRoute resources
    INGRESS_ROUTES=$(kubectl get ingressroute -n $NAMESPACE 2>/dev/null || echo "NOT_FOUND")

    if [ "$INGRESS_ROUTES" != "NOT_FOUND" ]; then
        echo -e "${GREEN}IngressRoute resources found:${NC}"
        kubectl get ingressroute -n $NAMESPACE -o wide

        echo ""
        echo "Checking backend IngressRoute details..."
        kubectl get ingressroute -n $NAMESPACE -o yaml | grep -A 20 "robson-backend\|api.robson" || echo "No backend routes found"
    else
        echo -e "${YELLOW}⚠️  No Traefik IngressRoute found${NC}"
    fi

    echo ""
    echo "5️⃣  Checking Traefik Middleware configuration..."
    echo ""

    MIDDLEWARES=$(kubectl get middleware -n $NAMESPACE 2>/dev/null || echo "NOT_FOUND")

    if [ "$MIDDLEWARES" != "NOT_FOUND" ]; then
        echo -e "${GREEN}Middleware resources found:${NC}"
        kubectl get middleware -n $NAMESPACE
    else
        echo -e "${YELLOW}⚠️  No Traefik Middleware found${NC}"
    fi

    echo ""
    echo "6️⃣  Checking Service configuration..."
    echo ""

    kubectl get svc -n $NAMESPACE -l $APP_LABEL -o wide

    echo ""
    echo "Service endpoints:"
    kubectl get endpoints -n $NAMESPACE -l $APP_LABEL

    echo ""
    echo "7️⃣  Testing connectivity to pod directly..."
    echo ""

    if [ "$POD_NAME" != "NOT_FOUND" ]; then
        echo "Attempting to exec into pod and test URL registration..."
        kubectl exec -n $NAMESPACE $POD_NAME -- python manage.py show_urls 2>/dev/null | grep "trading-intents" || echo "⚠️  Could not verify URLs in pod"
    fi

else
    echo -e "${YELLOW}⚠️  kubectl not available or cluster not reachable${NC}"
    echo "   This script needs kubectl access to check production"
fi

echo ""
echo "=================================================="
echo "8️⃣  Local verification (development)"
echo "=================================================="
echo ""

cd apps/backend/monolith

echo "Testing URL resolution..."
DJANGO_SETTINGS_MODULE=backend.settings .venv/bin/python -c "
import django
django.setup()

from django.urls import resolve

try:
    match = resolve('/api/trading-intents/create/')
    print('✅ URL resolves to:', match.func.__name__)
    print('   View module:', match.func.__module__)
except Exception as e:
    print('❌ URL resolution failed:', e)
" 2>&1 | grep -v "^🚀\|^📊\|^🔒\|^🌐\|^🔑\|^--\|^⚠️.*chat"

echo ""
echo "Testing view import..."
DJANGO_SETTINGS_MODULE=backend.settings .venv/bin/python -c "
import django
django.setup()

try:
    from api.views.trading_intent_views import create_trading_intent
    print('✅ create_trading_intent view imported successfully')

    # Check if conditional import succeeded
    from api.urls import TRADING_INTENT_VIEWS_AVAILABLE
    print(f'✅ TRADING_INTENT_VIEWS_AVAILABLE = {TRADING_INTENT_VIEWS_AVAILABLE}')
except Exception as e:
    print('❌ Failed to import:', e)
    import traceback
    traceback.print_exc()
" 2>&1 | grep -v "^🚀\|^📊\|^🔒\|^🌐\|^🔑\|^--\|^⚠️.*chat"

echo ""
echo "=================================================="
echo "9️⃣  Recommendations"
echo "=================================================="
echo ""

if command -v kubectl &> /dev/null && kubectl cluster-info &> /dev/null; then
    echo "Based on the checks above:"
    echo ""
    echo -e "${BLUE}If deployment is outdated:${NC}"
    echo "  → Wait for GitOps to sync (automatic)"
    echo "  → Or trigger manual ArgoCD sync:"
    echo "    $ argocd app sync robson-backend"
    echo ""
    echo -e "${BLUE}If TRADING_INTENT_VIEWS_AVAILABLE is False in pod:${NC}"
    echo "  → Check pod startup logs for import errors:"
    echo "    $ kubectl logs -n $NAMESPACE $POD_NAME | head -100"
    echo "  → Look for Python import errors or missing dependencies"
    echo ""
    echo -e "${BLUE}If Traefik routing is the issue:${NC}"
    echo "  → Verify IngressRoute has correct path matching"
    echo "  → Check if middleware is blocking POST requests"
    echo "  → Verify service is pointing to correct pods"
    echo ""
    echo -e "${BLUE}If everything looks good but still 404:${NC}"
    echo "  → Restart deployment to force reload:"
    echo "    $ kubectl rollout restart deployment/$DEPLOYMENT -n $NAMESPACE"
    echo "  → Or delete pod to force recreate:"
    echo "    $ kubectl delete pod $POD_NAME -n $NAMESPACE"
    echo ""
else
    echo "Since kubectl is not available, recommended actions:"
    echo ""
    echo "1. Check ArgoCD dashboard to verify sync status"
    echo "2. Check if latest commit (554c9be2 or later) is deployed"
    echo "3. If not synced, trigger manual sync in ArgoCD"
    echo "4. If synced but still 404, restart the deployment"
fi

echo ""
echo "=================================================="
echo "✅ Debug script completed"
echo "=================================================="
