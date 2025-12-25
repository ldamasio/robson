# Migration Locks Analysis - Event Sourcing

**ADR-0012**: Lock analysis for production safety

---

## Lock Types in Migrations

### Migration 0015: Event Sourcing Tables

**Creates new tables** (no existing data = no lock issues):
- `stop_events`
- `stop_executions`
- `tenant_config`
- `circuit_breaker_state`
- `outbox`

**Lock Level**: `ACCESS EXCLUSIVE` on each table **during creation only**

**Impact**: ✅ **SAFE** (new tables, no existing traffic)

**Indexes**: Created during table creation (no concurrent traffic yet)

---

### Migration 0016: Operation Table Alterations

**Operations performed:**

#### 1. ADD COLUMN (5 new columns)

```sql
ALTER TABLE operation ADD COLUMN stop_price NUMERIC(20, 8);
ALTER TABLE operation ADD COLUMN target_price NUMERIC(20, 8);
ALTER TABLE operation ADD COLUMN stop_execution_token VARCHAR(64);
ALTER TABLE operation ADD COLUMN last_stop_check_at TIMESTAMP;
ALTER TABLE operation ADD COLUMN stop_check_count INT DEFAULT 0;
```

**Lock Level**:
- PostgreSQL 11+: `ACCESS EXCLUSIVE` briefly (metadata lock)
- With `DEFAULT`: May require table rewrite (⚠️ SLOW on large tables)

**Impact**:
- ⚠️ **MEDIUM RISK** if `operation` table is large (>1M rows)
- Table locked during rewrite (~seconds to minutes depending on size)

**Mitigation**:
```sql
-- Option 1: Add column WITHOUT default first
ALTER TABLE operation ADD COLUMN stop_check_count INT;  -- No rewrite

-- Then set default (metadata-only in PG 11+)
ALTER TABLE operation ALTER COLUMN stop_check_count SET DEFAULT 0;

-- Then backfill in batches (no lock)
UPDATE operation SET stop_check_count = 0 WHERE stop_check_count IS NULL;
```

**Recommended for Production**: Split into 2 steps (add column, then set default)

---

#### 2. CREATE INDEX with Partial Condition

```sql
CREATE INDEX idx_operation_status_stop
ON operation (status, stop_price)
WHERE status = 'ACTIVE' AND stop_price IS NOT NULL;
```

**Lock Level**:
- Without `CONCURRENTLY`: `SHARE` lock (blocks writes) ⚠️
- With `CONCURRENTLY`: No blocking lock ✅

**Current Migration**: Uses standard `CREATE INDEX` (blocking)

**Impact**: ⚠️ **HIGH RISK** (blocks writes to `operation` table)

**Fix Required**:
```python
# In migration, use:
migrations.RunSQL(
    "CREATE INDEX CONCURRENTLY idx_operation_status_stop "
    "ON operation (status, stop_price) "
    "WHERE status = 'ACTIVE' AND stop_price IS NOT NULL;",
    reverse_sql="DROP INDEX CONCURRENTLY idx_operation_status_stop;"
)
```

**Note**: `CONCURRENTLY` cannot be used inside a transaction block.

**Django Migration Workaround**:
```python
class Migration(migrations.Migration):
    atomic = False  # ⭐ Required for CONCURRENTLY

    operations = [
        migrations.RunSQL(
            "CREATE INDEX CONCURRENTLY ...",
            reverse_sql="DROP INDEX CONCURRENTLY ...",
        ),
    ]
```

---

#### 3. Data Migration (Backfill)

```python
def backfill_stop_price(apps, schema_editor):
    Operation = apps.get_model('api', 'Operation')

    for op in Operation.objects.filter(stop_price__isnull=True):
        # Calculate stop_price...
        op.save(update_fields=['stop_price'])
```

**Lock Level**:
- Row-level `FOR UPDATE` on each operation
- No table lock

**Impact**:
- ✅ **SAFE** (row-level locks, short duration)
- ⚠️ May be slow if many operations (recommend batching)

**Optimization**:
```python
def backfill_stop_price_batched(apps, schema_editor):
    Operation = apps.get_model('api', 'Operation')

    batch_size = 1000
    operations = Operation.objects.filter(stop_price__isnull=True)

    total = operations.count()
    for i in range(0, total, batch_size):
        batch = operations[i:i+batch_size]

        # Bulk update (single query per batch)
        updates = []
        for op in batch:
            if op.stop_loss_percent and op.average_entry_price:
                # Calculate...
                updates.append(op)

        Operation.objects.bulk_update(updates, ['stop_price', 'target_price'])

        print(f"Backfilled {min(i+batch_size, total)}/{total} operations")
```

---

## Production Deployment Strategy

### Option 1: Zero-Downtime (Recommended)

**Step 1**: Add columns (no default)
```python
# Migration 0016a
operations = [
    migrations.AddField('operation', 'stop_price', ...),  # No DEFAULT
    migrations.AddField('operation', 'target_price', ...),
    migrations.AddField('operation', 'stop_execution_token', ...),
    migrations.AddField('operation', 'last_stop_check_at', ...),
    migrations.AddField('operation', 'stop_check_count', ...),  # No DEFAULT
]
```

**Step 2**: Set defaults (metadata-only)
```python
# Migration 0016b
operations = [
    migrations.RunSQL(
        "ALTER TABLE operation ALTER COLUMN stop_check_count SET DEFAULT 0;",
    ),
]
```

**Step 3**: Backfill in batches (outside migration)
```bash
# Run as management command (can monitor progress)
python manage.py backfill_stop_price --batch-size 1000
```

**Step 4**: Create indexes CONCURRENTLY (separate migration)
```python
# Migration 0016c
class Migration(migrations.Migration):
    atomic = False  # ⭐ Required

    operations = [
        migrations.RunSQL(
            "CREATE INDEX CONCURRENTLY idx_operation_status_stop ...",
        ),
    ]
```

**Total Downtime**: ~0 seconds (only brief metadata locks)

---

### Option 2: Maintenance Window (Simpler)

**Schedule**: Off-peak hours (low traffic)

**Steps**:
1. Announce maintenance window (15 minutes)
2. Put app in read-only mode
3. Run migrations (all steps together)
4. Verify backfill
5. Enable writes
6. Monitor for issues

**Total Downtime**: 5-15 minutes (depending on table size)

---

## Lock Summary Table

| Operation | Lock Type | Duration | Blocking? | Mitigation |
|-----------|-----------|----------|-----------|------------|
| **CREATE TABLE** (new tables) | ACCESS EXCLUSIVE | <1s | No (new table) | None needed |
| **ADD COLUMN** (no default) | ACCESS EXCLUSIVE | <1s | No | ✅ Safe |
| **ADD COLUMN** (with default) | ACCESS EXCLUSIVE | Seconds-Minutes | ⚠️ Yes | Split into 2 steps |
| **CREATE INDEX** | SHARE | Seconds-Minutes | ⚠️ Yes (blocks writes) | Use `CONCURRENTLY` |
| **CREATE INDEX CONCURRENTLY** | None | Seconds-Minutes | ✅ No | Requires `atomic=False` |
| **Backfill (UPDATE)** | Row-level | Depends on rows | ⚠️ Minor (row locks) | Batch updates |

---

## Recommended Migration Split

### Current (0016)
```
0016_operation_absolute_stops.py
├── ADD COLUMN (5 fields with defaults)  ⚠️ Locks table
├── Backfill data                        ⚠️ Slow
└── CREATE INDEX                         ⚠️ Blocks writes
```

### Recommended Split

**0016a_add_columns.py** (no defaults)
```python
operations = [
    migrations.AddField('operation', 'stop_price', null=True),  # No DEFAULT
    migrations.AddField('operation', 'target_price', null=True),
    migrations.AddField('operation', 'stop_execution_token', null=True),
    migrations.AddField('operation', 'last_stop_check_at', null=True),
    migrations.AddField('operation', 'stop_check_count', null=True),  # No DEFAULT
]
```
**Lock**: <1s metadata lock ✅

**0016b_set_defaults.py**
```python
operations = [
    migrations.RunSQL(
        "ALTER TABLE operation ALTER COLUMN stop_check_count SET DEFAULT 0;",
    ),
]
```
**Lock**: <1s metadata lock ✅

**0016c_backfill_data.py** (management command, not migration)
```bash
python manage.py backfill_stop_price --batch-size 1000
```
**Lock**: Row-level only ✅

**0016d_create_indexes.py**
```python
class Migration(migrations.Migration):
    atomic = False  # ⭐ Required for CONCURRENTLY

    operations = [
        migrations.RunSQL(
            "CREATE INDEX CONCURRENTLY idx_operation_status_stop "
            "ON operation (status, stop_price) "
            "WHERE status = 'ACTIVE' AND stop_price IS NOT NULL;",
        ),
    ]
```
**Lock**: None (concurrent) ✅

---

## Testing in Staging

### Simulate Production Load

```bash
# 1. Populate staging with realistic data
python manage.py create_test_operations --count 100000

# 2. Run migration with timing
\timing on
BEGIN;
ALTER TABLE operation ADD COLUMN stop_price NUMERIC(20, 8);
COMMIT;
-- Expected: <1s for metadata-only

# 3. Test index creation
CREATE INDEX CONCURRENTLY idx_test ON operation (status);
-- Monitor: pg_stat_progress_create_index
SELECT * FROM pg_stat_progress_create_index;

# 4. Test backfill performance
\timing on
UPDATE operation SET stop_check_count = 0 WHERE stop_check_count IS NULL;
-- Expected: ~1s per 10k rows
```

---

## Monitoring During Migration

```sql
-- Check active locks
SELECT
    locktype,
    relation::regclass,
    mode,
    granted,
    pid,
    query
FROM pg_locks
JOIN pg_stat_activity USING (pid)
WHERE relation = 'operation'::regclass;

-- Monitor index creation progress
SELECT
    phase,
    blocks_done,
    blocks_total,
    tuples_done,
    tuples_total
FROM pg_stat_progress_create_index;

-- Check table bloat after migration
SELECT
    schemaname,
    tablename,
    pg_size_pretty(pg_total_relation_size(schemaname||'.'||tablename)) as size
FROM pg_tables
WHERE tablename = 'operation';
```

---

## Rollback Procedure

If migration causes issues:

```bash
# 1. Rollback migration
python manage.py migrate api 0014

# 2. Verify columns dropped
psql -c "\d operation"
# Should NOT show: stop_price, target_price, etc.

# 3. Drop indexes manually if needed
DROP INDEX CONCURRENTLY IF EXISTS idx_operation_status_stop;

# 4. Vacuum table (reclaim space)
VACUUM FULL operation;  # ⚠️ Requires ACCESS EXCLUSIVE lock
# OR (safer):
VACUUM operation;  # No lock, but less space reclaimed
```

---

## Conclusion

**Current Migration (0016)**: ⚠️ **NOT production-safe** without modifications

**Required Changes**:
1. Split `ADD COLUMN` with defaults into 2 steps
2. Use `CREATE INDEX CONCURRENTLY` with `atomic=False`
3. Move backfill to management command (batch updates)

**Estimated Impact** (after fixes):
- **Locks**: <5s total (metadata-only)
- **Downtime**: 0 seconds (all operations non-blocking)
- **Duration**: 5-30 minutes (depending on table size, but non-blocking)

**Recommendation**: Apply **Option 1** (zero-downtime) for production.
