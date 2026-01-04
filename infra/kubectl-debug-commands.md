# Kubectl Debug Commands for 404 Error Investigation

## Environment Variables
```bash
export NAMESPACE="robson"
export DEPLOYMENT="rbs-backend-monolith-prod-deploy"
export APP_LABEL="app=rbs-backend-monolith"
```

## 1. Check Deployment Status

### Get current deployment image and status
```bash
kubectl get deployment $DEPLOYMENT -n $NAMESPACE -o wide
```

### Get detailed deployment info with current image SHA
```bash
kubectl describe deployment $DEPLOYMENT -n $NAMESPACE | grep -A 5 "Image:"
```

### Extract just the image SHA
```bash
kubectl get deployment $DEPLOYMENT -n $NAMESPACE -o jsonpath='{.spec.template.spec.containers[0].image}'
```

### Check rollout status
```bash
kubectl rollout status deployment/$DEPLOYMENT -n $NAMESPACE
```

## 2. Check Pod Status

### List all backend pods
```bash
kubectl get pods -n $NAMESPACE -l $APP_LABEL -o wide
```

### Get detailed pod information
```bash
kubectl describe pod -n $NAMESPACE -l $APP_LABEL
```

### Check pod restarts and age
```bash
kubectl get pods -n $NAMESPACE -l $APP_LABEL -o custom-columns=NAME:.metadata.name,RESTARTS:.status.containerStatuses[0].restartCount,AGE:.metadata.creationTimestamp
```

## 3. Check Pod Logs

### Get pod startup logs (check for import errors)
```bash
POD_NAME=$(kubectl get pods -n $NAMESPACE -l $APP_LABEL -o jsonpath='{.items[0].metadata.name}')
kubectl logs $POD_NAME -n $NAMESPACE --tail=200 | head -100
```

### Search for trading-intent registration in logs
```bash
kubectl logs $POD_NAME -n $NAMESPACE --tail=500 | grep -i "trading.intent\|TRADING_INTENT"
```

### Check for Python import errors
```bash
kubectl logs $POD_NAME -n $NAMESPACE --tail=500 | grep -i "error\|exception\|traceback" | head -50
```

### Follow logs in real-time
```bash
kubectl logs -f $POD_NAME -n $NAMESPACE
```

## 4. Verify URLs Inside Pod

### Execute show_urls command inside the pod
```bash
kubectl exec -n $NAMESPACE $POD_NAME -- python manage.py show_urls | grep "trading-intents"
```

### Check if TRADING_INTENT_VIEWS_AVAILABLE is True
```bash
kubectl exec -n $NAMESPACE $POD_NAME -- python -c "
import django
import os
os.environ.setdefault('DJANGO_SETTINGS_MODULE', 'backend.settings')
django.setup()
from api.urls import TRADING_INTENT_VIEWS_AVAILABLE
print(f'TRADING_INTENT_VIEWS_AVAILABLE = {TRADING_INTENT_VIEWS_AVAILABLE}')
"
```

### Test URL resolution inside pod
```bash
kubectl exec -n $NAMESPACE $POD_NAME -- python -c "
import django
import os
os.environ.setdefault('DJANGO_SETTINGS_MODULE', 'backend.settings')
django.setup()
from django.urls import resolve
match = resolve('/api/trading-intents/create/')
print(f'URL resolves to: {match.func}')
"
```

## 5. Check Traefik Configuration

### List all IngressRoute resources
```bash
kubectl get ingressroute -n $NAMESPACE
```

### Get detailed IngressRoute for backend
```bash
kubectl get ingressroute -n $NAMESPACE -o yaml | grep -A 30 "api.robson"
```

### Check Middleware resources
```bash
kubectl get middleware -n $NAMESPACE -o yaml
```

### Check if there are any path-based routing rules
```bash
kubectl get ingressroute -n $NAMESPACE -o yaml | grep -B 5 -A 5 "match:"
```

## 6. Check Service and Endpoints

### Get service details
```bash
kubectl get svc -n $NAMESPACE -l $APP_LABEL -o wide
```

### Check service endpoints (verify pods are registered)
```bash
kubectl get endpoints -n $NAMESPACE -l $APP_LABEL
```

### Describe service for full details
```bash
kubectl describe svc -n $NAMESPACE -l $APP_LABEL
```

## 7. Test Connectivity

### Port-forward to pod and test directly
```bash
kubectl port-forward -n $NAMESPACE $POD_NAME 8000:8000
# Then in another terminal:
curl -X POST http://localhost:8000/api/trading-intents/create/ \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer YOUR_TOKEN" \
  -d '{"symbol": 1, "strategy": 1}'
```

### Test from within the pod
```bash
kubectl exec -n $NAMESPACE $POD_NAME -- curl -X POST http://localhost:8000/api/trading-intents/create/ \
  -H "Content-Type: application/json" \
  -d '{"symbol": 1, "strategy": 1}'
```

## 8. Check Recent Events

### Get events for the namespace
```bash
kubectl get events -n $NAMESPACE --sort-by='.lastTimestamp' | tail -20
```

### Get events for the deployment
```bash
kubectl describe deployment $DEPLOYMENT -n $NAMESPACE | grep -A 10 "Events:"
```

## 9. Force Reload/Restart

### Restart deployment (rolling restart, zero downtime)
```bash
kubectl rollout restart deployment/$DEPLOYMENT -n $NAMESPACE
```

### Force delete pod (will be recreated immediately)
```bash
kubectl delete pod $POD_NAME -n $NAMESPACE
```

### Scale down and up (more aggressive)
```bash
kubectl scale deployment/$DEPLOYMENT -n $NAMESPACE --replicas=0
sleep 5
kubectl scale deployment/$DEPLOYMENT -n $NAMESPACE --replicas=2
```

## 10. Check ArgoCD Sync Status

### Get ArgoCD application status
```bash
argocd app get robson-backend
```

### Check if ArgoCD is out of sync
```bash
argocd app diff robson-backend
```

### Force sync
```bash
argocd app sync robson-backend --prune
```

### Sync and wait
```bash
argocd app sync robson-backend --prune --wait
```

## 11. Advanced Debugging

### Get full pod YAML for analysis
```bash
kubectl get pod $POD_NAME -n $NAMESPACE -o yaml > pod-debug.yaml
```

### Check pod environment variables
```bash
kubectl exec -n $NAMESPACE $POD_NAME -- env | grep -i "django\|debug\|cors"
```

### Check if Django is in DEBUG mode
```bash
kubectl exec -n $NAMESPACE $POD_NAME -- python -c "
import os
os.environ.setdefault('DJANGO_SETTINGS_MODULE', 'backend.settings')
from django.conf import settings
print(f'DEBUG = {settings.DEBUG}')
print(f'ALLOWED_HOSTS = {settings.ALLOWED_HOSTS}')
"
```

### List all registered URLs in production
```bash
kubectl exec -n $NAMESPACE $POD_NAME -- python manage.py show_urls > production-urls.txt
```

## 12. Compare Dev vs Prod

### Get pod image SHA
```bash
PROD_SHA=$(kubectl get deployment $DEPLOYMENT -n $NAMESPACE -o jsonpath='{.spec.template.spec.containers[0].image}' | grep -oP 'sha-\K[a-f0-9]+')
echo "Production SHA: $PROD_SHA"
```

### Compare with local git
```bash
LOCAL_SHA=$(git rev-parse HEAD | cut -c1-7)
echo "Local SHA: $LOCAL_SHA"

if [ "$PROD_SHA" == "$LOCAL_SHA" ]; then
    echo "✅ Prod is up to date with local"
else
    echo "⚠️  Prod is behind local"
    echo "Commits not in prod:"
    git log --oneline ${PROD_SHA}..${LOCAL_SHA}
fi
```

## 13. One-liner Diagnostic

### Quick health check
```bash
echo "=== Deployment ===" && \
kubectl get deployment $DEPLOYMENT -n $NAMESPACE && \
echo -e "\n=== Pods ===" && \
kubectl get pods -n $NAMESPACE -l $APP_LABEL && \
echo -e "\n=== Service ===" && \
kubectl get svc -n $NAMESPACE -l $APP_LABEL && \
echo -e "\n=== IngressRoute ===" && \
kubectl get ingressroute -n $NAMESPACE && \
echo -e "\n=== Recent Logs ===" && \
kubectl logs -n $NAMESPACE -l $APP_LABEL --tail=10
```

## Notes

- Replace `YOUR_TOKEN` with actual JWT token from frontend
- If using kubeconfig other than default, add `--kubeconfig=/path/to/config`
- For production namespace, ensure you're using correct context: `kubectl config use-context production`
- Some commands may require additional RBAC permissions

## Quick Troubleshooting Flow

1. **Check deployment is up to date**: Compare image SHA with git commit
2. **Check pod is running**: `kubectl get pods`
3. **Check pod logs for errors**: `kubectl logs` looking for import errors
4. **Verify URLs are registered**: `kubectl exec -- python manage.py show_urls`
5. **Check Traefik routing**: `kubectl get ingressroute`
6. **Test direct pod access**: `kubectl port-forward` and curl
7. **If all looks good**: Restart deployment with `kubectl rollout restart`
