#!/bin/bash
# Fix Staging Backend - Apply All Critical Fixes
# Run: bash fix-staging-backend.sh
# Created: 2024-12-25

set -e

SSH_HOST="root@158.220.116.31"
NAMESPACE="staging"

echo "=========================================="
echo "FIX STAGING BACKEND - CRITICAL ISSUES"
echo "=========================================="
echo ""
echo "This script will:"
echo "1. Apply fixed backend deployment (with imagePullSecrets, RBS_* env vars, TCP probes)"
echo "2. Create Traefik ingress for external access"
echo "3. Restart backend pods"
echo "4. Verify deployment"
echo ""
read -p "Continue? (y/n) " -n 1 -r
echo
if [[ ! $REPLY =~ ^[Yy]$ ]]; then
    echo "Aborted."
    exit 1
fi

echo ""
echo "=========================================="
echo "STEP 1: Apply Fixed Backend Deployment"
echo "=========================================="
echo ""

# Apply the fixed backend deployment
scp infra/k8s/staging/backend/backend-staging.yml $SSH_HOST:/tmp/backend-staging-fixed.yml
ssh $SSH_HOST "kubectl apply -f /tmp/backend-staging-fixed.yaml"

echo "✅ Backend deployment updated"
echo ""

echo "=========================================="
echo "STEP 2: Create Traefik Ingress"
echo "=========================================="
echo ""

# Apply the Traefik ingress
scp infra/k8s/staging/ingress/traefik-staging.yml $SSH_HOST:/tmp/traefik-staging.yml
ssh $SSH_HOST "kubectl apply -f /tmp/traefik-staging.yaml"

echo "✅ Traefik ingress created"
echo ""

echo "=========================================="
echo "STEP 3: Restart Backend Pods"
echo "=========================================="
echo ""

# Force restart to pick up new configuration
ssh $SSH_HOST "kubectl rollout restart deployment/backend-staging -n $NAMESPACE"

echo "⏳ Waiting for rollout to complete..."
ssh $SSH_HOST "kubectl rollout status deployment/backend-staging -n $NAMESPACE --timeout=5m" || {
    echo "⚠️ Rollout timed out or failed. Checking pod status..."
    ssh $SSH_HOST "kubectl get pods -n $NAMESPACE"
    echo ""
    echo "Checking pod logs..."
    ssh $SSH_HOST "kubectl logs -n $NAMESPACE -l app=backend-staging --tail=50" || true
    exit 1
}

echo "✅ Rollout complete"
echo ""

echo "=========================================="
echo "STEP 4: Verify Deployment"
echo "=========================================="
echo ""

echo "Pod Status:"
ssh $SSH_HOST "kubectl get pods -n $NAMESPACE"
echo ""

echo "Ingress Status:"
ssh $SSH_HOST "kubectl get ingress -n $NAMESPACE"
echo ""

echo "Service Endpoints:"
ssh $SSH_HOST "kubectl get endpoints backend-staging -n $NAMESPACE"
echo ""

echo "=========================================="
echo "STEP 5: Post-Deployment Tasks"
echo "=========================================="
echo ""

echo "🔍 Checking if pods are running..."
RUNNING_PODS=$(ssh $SSH_HOST "kubectl get pods -n $NAMESPACE -l app=backend-staging -o jsonpath='{.items[*].status.phase}'" | grep -o "Running" | wc -l)

if [ "$RUNNING_PODS" -ge 1 ]; then
    echo "✅ Backend pods are running!"
    echo ""

    echo "📋 Running migrations..."
    ssh $SSH_HOST "kubectl exec -n $NAMESPACE deployment/backend-staging -- python manage.py migrate --noinput" || {
        echo "⚠️ Migrations failed. Check logs."
    }
    echo ""

    echo "📊 Running backfill command..."
    ssh $SSH_HOST "kubectl exec -n $NAMESPACE deployment/backend-staging -- python manage.py backfill_stop_price" || {
        echo "⚠️ Backfill failed. This might be expected if no data to backfill."
    }
    echo ""
else
    echo "❌ Backend pods are NOT running yet. Check logs:"
    echo "kubectl logs -n $NAMESPACE -l app=backend-staging --tail=100"
fi

echo "=========================================="
echo "VERIFICATION CHECKLIST"
echo "=========================================="
echo ""

echo "✅ 1. Backend deployment applied with:"
echo "   - imagePullSecrets: ghcr-secret"
echo "   - RBS_* environment variables"
echo "   - TCP health probes (instead of HTTP)"
echo "   - securityContext removed"
echo ""

echo "✅ 2. Traefik ingress created for:"
echo "   - api.staging.rbx.ia.br → backend-staging:8000"
echo "   - rabbitmq.staging.rbx.ia.br → rabbitmq-staging:15672"
echo ""

echo "⏳ 3. TLS certificates:"
echo "   Run: ssh $SSH_HOST 'kubectl get certificate -n $NAMESPACE'"
echo "   (may take 5-10 minutes to be issued)"
echo ""

echo "=========================================="
echo "MANUAL VERIFICATION STEPS"
echo "=========================================="
echo ""

echo "1. Check pod logs:"
echo "   ssh $SSH_HOST 'kubectl logs -n $NAMESPACE -l app=backend-staging --tail=50'"
echo ""

echo "2. Test API endpoint:"
echo "   curl -k https://api.staging.rbx.ia.br/health/"
echo "   (or any other endpoint)"
echo ""

echo "3. Check migrations:"
echo "   ssh $SSH_HOST 'kubectl exec -n $NAMESPACE deployment/backend-staging -- python manage.py showmigrations api'"
echo ""

echo "4. Monitor pod status:"
echo "   ssh $SSH_HOST 'kubectl get pods -n $NAMESPACE -w'"
echo ""

echo "=========================================="
echo "FIX SCRIPT COMPLETE"
echo "=========================================="
echo ""

echo "If pods are still crashing:"
echo "1. Get crash logs:"
echo "   ssh $SSH_HOST 'kubectl logs -n $NAMESPACE -l app=backend-staging --previous --tail=100'"
echo ""
echo "2. Check for missing secrets:"
echo "   ssh $SSH_HOST 'kubectl get secret -n $NAMESPACE'"
echo ""
echo "3. Review troubleshooting guide:"
echo "   docs/infrastructure/TROUBLESHOOTING-STAGING-BACKEND.md"
echo ""

echo "Good luck! 🚀"
