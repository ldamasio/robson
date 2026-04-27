# Migration Guide: Event-Sourced Stop-Loss Monitor

**Status**: Ready for Application
**Date**: 2024-12-25
**Related ADR**: ADR-0012

---

## Overview

This guide walks through applying the Event Sourcing migrations for the stop-loss monitor system.

**What's being migrated:**
- New append-only `stop_events` table (event store)
- New `stop_executions` table (materialized view/projection)
- New `tenant_config` table (risk guardrails)
- New `circuit_breaker_state` table (per-symbol circuit breaker)
- New `outbox` table (transactional outbox pattern)
- Updated `operation` table (add `stop_price`, `target_price` fields)

---

## Pre-Migration Checklist

**Before applying migrations:**

- [ ] Backup production database:
  ```bash
  pg_dump -h $RBS_PG_HOST -U $RBS_PG_USER -d $RBS_PG_DATABASE > backup_pre_event_sourcing.sql
  ```

- [ ] Verify no active stop-loss executions:
  ```sql
  SELECT COUNT(*) FROM operation WHERE status = 'ACTIVE';
  ```

- [ ] Check disk space (new tables will grow with events):
  ```bash
  df -h | grep postgres
  ```

- [ ] Review migration files:
  - `api/migrations/0015_event_sourcing_stop_monitor.py`
  - `api/migrations/0016_operation_absolute_stops.py`

---

## Migration Files Created

### 1. `0015_event_sourcing_stop_monitor.py`

Creates the core Event Sourcing tables:

**Tables:**
- `stop_events` (append-only event store)
- `stop_executions` (materialized view)
- `tenant_config` (risk guardrails)
- `circuit_breaker_state` (per-symbol circuit breaker)
- `outbox` (transactional outbox pattern)

**Indexes:**
- Optimized for event replay (`event_seq`)
- Multi-tenant queries (`client_id`, `occurred_at`)
- Event type filtering (`event_type`, `occurred_at`)
- Source attribution (`source`, `occurred_at`)
- Symbol queries (`symbol`, `occurred_at`)
- Unpublished outbox entries (`published=false`, `created_at`)

### 2. `0016_operation_absolute_stops.py`

Updates the `operation` table:

**New fields:**
- `stop_price` (Decimal 20,8) - Absolute stop level (FIXED)
- `target_price` (Decimal 20,8) - Absolute target level (FIXED)
- `stop_execution_token` (VARCHAR 64) - Idempotency token
- `last_stop_check_at` (TIMESTAMP) - Last monitor check
- `stop_check_count` (INT) - Number of checks

**Data migration:**
- Backfills `stop_price` from `stop_loss_percent` for existing operations
- Backfills `target_price` from `stop_gain_percent`
- Formula:
  - BUY: `stop_price = entry_price * (1 - stop_loss_percent / 100)`
  - SELL: `stop_price = entry_price * (1 + stop_loss_percent / 100)`

**Deprecation:**
- Marks `stop_loss_percent` and `stop_gain_percent` as `[DEPRECATED]` (kept for reference)

---

## Application Steps

### Step 1: Apply Migrations (Staging)

```bash
cd apps/backend/monolith

# Dry run (check what will be applied)
python manage.py migrate --plan

# Apply migrations
python manage.py migrate api 0015
python manage.py migrate api 0016

# Verify
python manage.py showmigrations api
```

**Expected output:**
```
[X] 0015_event_sourcing_stop_monitor
[X] 0016_operation_absolute_stops
```

### Step 2: Verify Tables Created

```sql
-- Check tables exist
\dt stop_events
\dt stop_executions
\dt tenant_config
\dt circuit_breaker_state
\dt outbox

-- Check indexes
\di idx_stop_events_op_seq
\di idx_stop_events_tenant
\di idx_stop_exec_op_status
\di idx_outbox_unpublished

-- Check operation fields
\d operation
-- Should see: stop_price, target_price, stop_execution_token, etc.
```

### Step 3: Verify Data Backfill

```sql
-- Check operations with backfilled stop_price
SELECT
    id,
    side,
    stop_loss_percent,
    stop_price,
    average_entry_price,
    status
FROM operation
WHERE stop_price IS NOT NULL
LIMIT 10;

-- Verify calculation
-- Example: BUY side, entry=$95,000, stop%=2%
-- Expected: stop_price = 95000 * (1 - 0.02) = $93,100
```

### Step 4: Initialize Default Configs

```sql
-- Create default tenant configs for existing clients
INSERT INTO tenant_config (
    client_id,
    trading_enabled,
    max_slippage_pct,
    slippage_pause_threshold_pct,
    max_executions_per_minute,
    max_executions_per_hour
)
SELECT
    id as client_id,
    TRUE as trading_enabled,
    5.0 as max_slippage_pct,
    10.0 as slippage_pause_threshold_pct,
    10 as max_executions_per_minute,
    100 as max_executions_per_hour
FROM client
WHERE id NOT IN (SELECT client_id FROM tenant_config);

-- Verify
SELECT * FROM tenant_config;
```

---

## Testing

### Test 1: Event Insertion

```python
from api.models import StopEvent, Operation

# Get an active operation
op = Operation.objects.filter(status='ACTIVE').first()

# Create a STOP_TRIGGERED event
event = StopEvent.objects.create(
    operation=op,
    client=op.client,
    symbol=op.symbol.name,
    event_type='STOP_TRIGGERED',
    trigger_price=op.stop_price,
    stop_price=op.stop_price,
    quantity=op.total_entry_quantity,
    side='SELL' if op.side == 'BUY' else 'BUY',
    execution_token=f"{op.id}:{op.stop_price}:{int(time.time()*1000)}",
    source='manual',
)

print(f"✅ Event created: {event.event_id}")
print(f"   Event sequence: {event.event_seq}")
```

### Test 2: Idempotency (Duplicate Prevention)

```python
from django.db import IntegrityError

# Try to create event with same token
try:
    duplicate = StopEvent.objects.create(
        # ... same fields as above ...
        execution_token=event.execution_token,  # Same token!
    )
except IntegrityError as e:
    print(f"✅ Idempotency working: {e}")
    # Expected: IntegrityError (unique constraint on execution_token)
```

### Test 3: Outbox Entry Creation

```python
from api.models import Outbox

# Create outbox entry for event
outbox_entry = Outbox.objects.create(
    event=event,
    routing_key=f"stop.trigger.{op.client_id}.{op.symbol.name}",
    exchange='stop_events',
    payload={
        'event_id': str(event.event_id),
        'event_type': event.event_type,
        'operation_id': op.id,
        'trigger_price': str(event.trigger_price),
    },
)

print(f"✅ Outbox entry created: {outbox_entry.outbox_id}")
print(f"   Published: {outbox_entry.published}")  # Should be False
```

### Test 4: Tenant Config

```python
from api.models import TenantConfig, Client

client = Client.objects.first()
config = TenantConfig.objects.get(client=client)

print(f"Trading enabled: {config.trading_enabled}")
print(f"Max slippage: {config.max_slippage_pct}%")
print(f"Max executions/min: {config.max_executions_per_minute}")
```

### Test 5: Circuit Breaker

```python
from api.models import CircuitBreakerStateModel

# Create circuit breaker state
cb = CircuitBreakerStateModel.objects.create(
    symbol='BTCUSDC',
    state='CLOSED',
    failure_count=0,
    failure_threshold=3,
    retry_delay_seconds=300,
)

print(f"✅ Circuit breaker created for {cb.symbol}")
print(f"   State: {cb.state}")
```

---

## Rollback Procedure

**If issues occur, rollback migrations:**

```bash
# Rollback both migrations
python manage.py migrate api 0014

# Restore database from backup (if needed)
psql -h $RBS_PG_HOST -U $RBS_PG_USER -d $RBS_PG_DATABASE < backup_pre_event_sourcing.sql
```

**What gets rolled back:**
- All new tables dropped
- Operation fields removed
- Data backfill reverted

---

## Production Deployment

### Pre-Deployment

1. **Test in staging** (apply all steps above)
2. **Monitor performance** (check query times, index usage)
3. **Verify backfill** (all operations have `stop_price`)

### Deployment Steps

```bash
# 1. Backup production database
pg_dump ... > backup_prod_$(date +%Y%m%d_%H%M%S).sql

# 2. Apply migrations during maintenance window
kubectl exec -it rbs-backend-monolith-prod-... -- bash
cd /app
python manage.py migrate api 0015
python manage.py migrate api 0016

# 3. Verify tables and data
psql -h ... -c "SELECT COUNT(*) FROM stop_events;"
psql -h ... -c "SELECT COUNT(*) FROM operation WHERE stop_price IS NOT NULL;"

# 4. Initialize tenant configs
psql -h ... -f /path/to/init_tenant_configs.sql

# 5. Monitor logs
kubectl logs -f rbs-backend-monolith-prod-... --tail=100
```

### Post-Deployment Validation

```sql
-- Check event counts (should be 0 initially)
SELECT COUNT(*) FROM stop_events;
SELECT COUNT(*) FROM stop_executions;
SELECT COUNT(*) FROM outbox;

-- Check tenant configs
SELECT client_id, trading_enabled, max_slippage_pct FROM tenant_config;

-- Check operation stop_price backfill
SELECT
    COUNT(*) as total,
    COUNT(stop_price) as with_stop_price,
    COUNT(stop_price) * 100.0 / COUNT(*) as backfill_pct
FROM operation;
-- Expected: backfill_pct close to 100% for active operations

-- Check circuit breakers (should be empty initially)
SELECT COUNT(*) FROM circuit_breaker_state;
```

---

## Next Steps

After successful migration:

1. ✅ **Update `monitor_stops.py`** to use `stop_price` (not `stop_loss_percent`)
2. ✅ **Implement event emission** in monitor
3. ✅ **Deploy Rust service** (WebSocket consumer + event emitter)
4. ✅ **Remove `--dry-run`** from CronJob
5. ✅ **Monitor event accumulation** (stop_events table growth)

---

## Monitoring

### Key Metrics to Track

```sql
-- Event accumulation rate
SELECT
    DATE_TRUNC('hour', occurred_at) as hour,
    event_type,
    COUNT(*) as event_count
FROM stop_events
GROUP BY hour, event_type
ORDER BY hour DESC;

-- Execution success rate
SELECT
    event_type,
    source,
    COUNT(*) as total
FROM stop_events
GROUP BY event_type, source;

-- Outbox backlog (should be low if worker is healthy)
SELECT
    COUNT(*) as unpublished_count,
    MIN(created_at) as oldest_unpublished
FROM outbox
WHERE published = FALSE;
```

### Alerts to Configure

- Outbox backlog > 100 entries (worker not publishing)
- Circuit breaker OPEN for >10 minutes (market issues)
- Event growth rate > 1000 events/hour (unusual activity)
- Execution failure rate > 10% (Binance API issues)

---

## FAQ

**Q: What happens to existing operations with `stop_loss_percent`?**
A: Migration 0016 backfills `stop_price` from the percentage. Both fields coexist; percentage is marked `[DEPRECATED]`.

**Q: Can I still use `stop_loss_percent` in new operations?**
A: Yes, but deprecated. You should set `stop_price` directly (absolute value). The monitor will use `stop_price` if present, fall back to percentage if not.

**Q: What if `stop_price` is NULL?**
A: Monitor skips the operation (no stop configured). Emit a warning log.

**Q: How much disk space will `stop_events` consume?**
A: Approximately 500 bytes per event. At 1000 events/day: ~180 MB/year. Plan for archival after 1 year.

**Q: Can I delete old events?**
A: Yes, but **only after archiving**. Events are append-only for audit compliance. Consider moving events older than 1 year to cold storage (S3, BigQuery).

**Q: What if outbox entries don't get published?**
A: The outbox worker will retry (up to `retry_count`). Check RabbitMQ connectivity and worker logs. Manual intervention: mark as published or delete after investigation.

---

## Troubleshooting

### Migration Fails at 0015

**Error**: `relation "stop_events" already exists`

**Solution**:
```bash
# Check if table exists from previous attempt
psql -c "\dt stop_events"

# If exists, drop manually and retry
psql -c "DROP TABLE IF EXISTS stop_events CASCADE;"
python manage.py migrate api 0015
```

### Backfill Shows 0 Operations Updated

**Issue**: No operations have `stop_loss_percent` and `average_entry_price`.

**Solution**:
```sql
-- Check active operations
SELECT
    COUNT(*) as total,
    COUNT(stop_loss_percent) as with_percent,
    COUNT(average_entry_price) as with_entry_price
FROM operation
WHERE status = 'ACTIVE';

-- If most are NULL, backfill won't work (expected for new system)
```

### Index Creation Times Out

**Issue**: Large `operation` table causes slow index creation.

**Solution**:
```sql
-- Create index CONCURRENTLY (allows concurrent reads/writes)
CREATE INDEX CONCURRENTLY idx_operation_status_stop
ON operation (status, stop_price)
WHERE status = 'ACTIVE' AND stop_price IS NOT NULL;
```

---

**Migration prepared by**: Claude Code AI Assistant
**Review status**: Pending
**Deployment target**: Staging → Production
