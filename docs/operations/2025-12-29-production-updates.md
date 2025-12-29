# Production Updates - 2025-12-29

**Status**: ⏳ Pending deployment to VPS
**Date**: 2025-12-29

---

## Summary of Changes

This document records the changes made during the December 29, 2025 session and the steps needed to deploy them.

### 1. Closed Leveraged Position with Profit ✅

- **Entry**: $87,193.34
- **Exit**: ~$90,058
- **Profit**: +3.28% (with 3x leverage = ~+9.9% on margin)
- **Order ID**: 7667466531

### 2. Stop Monitor CronJob - Dry-Run Removed ✅

**File**: `infra/k8s/prod/rbs-stop-monitor-cronjob.yml`

Changed from:

```yaml
command:
- python
- manage.py
- monitor_stops
- --dry-run  # Was enabled
```

To:

```yaml
command:
- python
- manage.py
- monitor_stops
# PRODUCTION MODE ACTIVE: Executing real stop orders!
# Changed from --dry-run to active mode on 2025-12-29
# - --dry-run  # DISABLED - Now executing real stops
```

### 3. Trailing Stop CronJob Created ✅

**File**: `infra/k8s/prod/rbs-trailing-stop-cronjob.yml`

New CronJob that runs every minute to adjust trailing stops using the "Hand-Span" algorithm:

- **Schedule**: `* * * * *` (every minute)
- **Command**: `python manage.py adjust_trailing_stops`
- **Algorithm**:
  - 1 span profit → Move stop to break-even
  - 2+ spans profit → Trail by (spans - 1) × span
  - Stop only moves monotonically (never retreats)

### 4. AI Chat with Groq Implemented ✅

New conversational AI assistant integrated into the logged-in dashboard.

**Backend Files**:

- `apps/backend/core/domain/conversation.py` - Domain entities
- `apps/backend/core/application/ports/chat_ports.py` - Port definitions
- `apps/backend/core/application/use_cases/chat_with_robson.py` - Use case
- `apps/backend/core/adapters/driven/ai/groq_adapter.py` - Groq LLM adapter
- `apps/backend/monolith/api/views/chat_views.py` - REST endpoints

**Frontend Files**:

- `apps/frontend/src/components/logged/RobsonChat.jsx` - Chat component
- `apps/frontend/src/components/logged/RobsonChat.css` - Styling

**API Endpoints**:

- `POST /api/chat/` - Send message and receive AI response
- `GET /api/chat/status/` - Check AI service availability
- `GET /api/chat/context/` - Get current trading context

---

## Pending VPS Commands

### Step 1: Create Groq API Secret

```bash
kubectl create secret generic rbs-groq-secret \
  --from-literal=GROQ_API_KEY=<YOUR_GROQ_API_KEY> \
  -n robson \
  --dry-run=client -o yaml | kubectl apply -f -
```

**Verify**:

```bash
kubectl get secrets -n robson | grep groq
```

### Step 2: Apply Updated CronJobs

```bash
# Pull latest code
cd /path/to/robson
git pull origin main

# Apply stop monitor (now without dry-run)
kubectl apply -f infra/k8s/prod/rbs-stop-monitor-cronjob.yml

# Apply new trailing stop cronjob
kubectl apply -f infra/k8s/prod/rbs-trailing-stop-cronjob.yml

# Verify CronJobs
kubectl get cronjobs -n robson
```

**Expected output**:

```
NAME                         SCHEDULE    SUSPEND   ACTIVE   LAST SCHEDULE
rbs-stop-monitor-cronjob     * * * * *   False     0        <time>
rbs-trailing-stop-cronjob    * * * * *   False     0        <time>
```

### Step 3: Update Backend Deployment with Groq Secret

Add this environment variable to `infra/k8s/prod/rbs-backend-monolith-prod-deploy.yml`:

```yaml
- name: GROQ_API_KEY
  valueFrom:
    secretKeyRef:
      name: rbs-groq-secret
      key: GROQ_API_KEY
```

Then apply:

```bash
kubectl apply -f infra/k8s/prod/rbs-backend-monolith-prod-deploy.yml

# Verify pod restart
kubectl rollout status deployment/rbs-backend-monolith-prod-deploy -n robson
```

### Step 4: Verify Everything is Working

**Check CronJob executions**:

```bash
# Stop monitor
kubectl get jobs -n robson -l app=rbs-stop-monitor --sort-by=.status.startTime | tail -5

# Trailing stop
kubectl get jobs -n robson -l app=rbs-trailing-stop --sort-by=.status.startTime | tail -5

# View logs
kubectl logs -n robson -l app=rbs-stop-monitor --tail=50
kubectl logs -n robson -l app=rbs-trailing-stop --tail=50
```

**Test chat endpoint**:

```bash
# Get auth token first (replace with actual credentials)
TOKEN=$(curl -s -X POST https://api.robson.rbx.ia.br/api/token/ \
  -H "Content-Type: application/json" \
  -d '{"username":"admin","password":"yourpassword"}' | jq -r '.access')

# Test chat status
curl -s -H "Authorization: Bearer $TOKEN" \
  https://api.robson.rbx.ia.br/api/chat/status/

# Expected: {"available": true, "model": "llama3-8b-8192", "provider": "Groq"}
```

---

## Architecture Decisions

### Why Groq First?

- **Free tier available** - No cost for initial testing
- **Fast inference** - Low latency responses
- **Compatible API** - Easy migration to other providers later
- **Future plan**: Multi-cloud (OpenAI, Anthropic, DeepSeek) with user-customizable tokens

### Why Separate CronJobs?

- **Separation of concerns**:
  - `monitor_stops` - Executes stop-loss and take-profit orders
  - `adjust_trailing_stops` - Adjusts stop prices based on market movement
- **Independent scaling** - Can adjust frequencies independently
- **Clearer logging** - Easier to debug issues

---

## Related Documentation

- [PRODUCTION_TRADING.md](../PRODUCTION_TRADING.md) - Production trading guide
- [ADR-0002-hexagonal-architecture.md](../adr/ADR-0002-hexagonal-architecture.md) - Architecture
- [First Leveraged Position](./2025-12-24-first-leveraged-position.md) - Previous operation

---

**Last Updated**: 2025-12-29
