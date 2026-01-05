#!/bin/bash
# Test production API directly without kubectl access

API_URL="${API_URL:-https://api.robson.rbx.ia.br}"
TOKEN="${1:-}"

echo "=================================================="
echo "🧪 Testing Production API: $API_URL"
echo "=================================================="
echo ""

if [ -z "$TOKEN" ]; then
    echo "⚠️  No authentication token provided"
    echo "Usage: $0 <JWT_TOKEN>"
    echo ""
    echo "To get a token:"
    echo "  1. Login at https://app.robson.rbx.ia.br"
    echo "  2. Open browser DevTools > Application > Local Storage"
    echo "  3. Copy the 'authTokens' access token"
    echo ""
    echo "Or login via API:"
    echo "  curl -X POST $API_URL/api/auth/token/ \\"
    echo "    -H 'Content-Type: application/json' \\"
    echo "    -d '{\"username\":\"your_user\",\"password\":\"your_pass\"}'"
    echo ""
fi

echo "1️⃣  Testing API connectivity..."
HTTP_CODE=$(curl -s -o /dev/null -w "%{http_code}" "$API_URL/api/ping/" || echo "000")

if [ "$HTTP_CODE" = "200" ]; then
    echo "✅ API is reachable (HTTP $HTTP_CODE)"
elif [ "$HTTP_CODE" = "000" ]; then
    echo "❌ Cannot connect to API (network error)"
    echo "   Check if $API_URL is accessible"
    exit 1
else
    echo "⚠️  API responded with HTTP $HTTP_CODE"
fi

echo ""
echo "2️⃣  Testing server time endpoint..."
curl -s "$API_URL/api/server-time/" | jq '.' || echo "Failed to parse JSON"

echo ""
echo "3️⃣  Testing trading-intents endpoints (without auth)..."
echo ""

# Test GET /api/trading-intents/ (should return 401 Unauthorized)
echo "GET /api/trading-intents/"
HTTP_CODE=$(curl -s -o /tmp/test-response.json -w "%{http_code}" "$API_URL/api/trading-intents/")
echo "  HTTP: $HTTP_CODE"
if [ -f /tmp/test-response.json ]; then
    echo "  Response:"
    cat /tmp/test-response.json | jq '.' 2>/dev/null || cat /tmp/test-response.json
    rm /tmp/test-response.json
fi
echo ""

# Test POST /api/trading-intents/create/ (should return 401 or 403, NOT 404!)
echo "POST /api/trading-intents/create/"
HTTP_CODE=$(curl -s -o /tmp/test-response.json -w "%{http_code}" \
    -X POST "$API_URL/api/trading-intents/create/" \
    -H "Content-Type: application/json" \
    -d '{"symbol":1,"strategy":1}')
echo "  HTTP: $HTTP_CODE"

if [ "$HTTP_CODE" = "404" ]; then
    echo "  ❌ ENDPOINT NOT FOUND (404)"
    echo "     This confirms the bug - endpoint should return 401/403, not 404"
elif [ "$HTTP_CODE" = "401" ] || [ "$HTTP_CODE" = "403" ]; then
    echo "  ✅ Endpoint exists (returning auth error as expected)"
else
    echo "  ⚠️  Unexpected response: $HTTP_CODE"
fi

if [ -f /tmp/test-response.json ]; then
    echo "  Response:"
    cat /tmp/test-response.json | jq '.' 2>/dev/null || cat /tmp/test-response.json
    rm /tmp/test-response.json
fi
echo ""

# Test with authentication if token provided
if [ -n "$TOKEN" ]; then
    echo "4️⃣  Testing with authentication..."
    echo ""

    echo "GET /api/trading-intents/ (authenticated)"
    HTTP_CODE=$(curl -s -o /tmp/test-response.json -w "%{http_code}" \
        -H "Authorization: Bearer $TOKEN" \
        "$API_URL/api/trading-intents/")
    echo "  HTTP: $HTTP_CODE"
    if [ -f /tmp/test-response.json ]; then
        echo "  Response:"
        cat /tmp/test-response.json | jq '.' 2>/dev/null || cat /tmp/test-response.json
        rm /tmp/test-response.json
    fi
    echo ""

    echo "POST /api/trading-intents/create/ (authenticated)"
    HTTP_CODE=$(curl -s -o /tmp/test-response.json -w "%{http_code}" \
        -X POST "$API_URL/api/trading-intents/create/" \
        -H "Authorization: Bearer $TOKEN" \
        -H "Content-Type: application/json" \
        -d '{"symbol":1,"strategy":1}')
    echo "  HTTP: $HTTP_CODE"

    if [ "$HTTP_CODE" = "404" ]; then
        echo "  ❌ CRITICAL: Authenticated request also returns 404"
        echo "     The endpoint is definitely not registered in production"
    elif [ "$HTTP_CODE" = "400" ]; then
        echo "  ✅ Endpoint exists (validation error - expected with dummy data)"
    elif [ "$HTTP_CODE" = "201" ]; then
        echo "  ✅ Endpoint works! (created successfully)"
    fi

    if [ -f /tmp/test-response.json ]; then
        echo "  Response:"
        cat /tmp/test-response.json | jq '.' 2>/dev/null || cat /tmp/test-response.json
        rm /tmp/test-response.json
    fi
fi

echo ""
echo "5️⃣  Checking available endpoints..."
echo ""
echo "GET /api/strategies/ (should work)"
HTTP_CODE=$(curl -s -o /tmp/test-response.json -w "%{http_code}" "$API_URL/api/strategies/" || echo "000")
echo "  HTTP: $HTTP_CODE"
rm -f /tmp/test-response.json

echo ""
echo "GET /api/symbols/ (should work if SYMBOL_VIEWS_AVAILABLE)"
HTTP_CODE=$(curl -s -o /tmp/test-response.json -w "%{http_code}" "$API_URL/api/symbols/" || echo "000")
echo "  HTTP: $HTTP_CODE"
rm -f /tmp/test-response.json

echo ""
echo "=================================================="
echo "✅ API Test Completed"
echo "=================================================="
echo ""
echo "Summary:"
if [ "$HTTP_CODE" = "404" ]; then
    echo "  ❌ The /api/trading-intents/create/ endpoint returns 404"
    echo "  → This means TRADING_INTENT_VIEWS_AVAILABLE is likely False in production"
    echo "  → Or the deployment doesn't have the latest code"
    echo ""
    echo "Next steps:"
    echo "  1. Check ArgoCD to see if latest commit is deployed"
    echo "  2. Check production pod logs for import errors"
    echo "  3. Verify gitops sync status"
else
    echo "  ✅ Endpoint appears to be working"
    echo "  → HTTP $HTTP_CODE is expected for this test"
fi
