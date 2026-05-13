# Phase 9: Next Steps

## Database Lifecycle

### Prerequisites
- PostgreSQL 16+ (or Docker/ParadeDB)
- Rust toolchain
- DATABASE_URL environment variable (for db commands)

### Running Migrations

```bash
# Set DATABASE_URL
export DATABASE_URL="postgresql://robson:robson@localhost:5432/robson_v2"

# Run migrations via robsond CLI
robsond db migrate

# Check migration status
robsond db status

# Initialize minimal tenant/account/strategy data
robsond db init

# Or with specific IDs
robsond db init --tenant-id <uuid> --account-id <uuid>
```

### Manual Migration (Alternative)

```bash
# Option A: Docker Postgres
docker run --rm -p 5432:5432 -e POSTGRES_PASSWORD=test postgres:16

# Option B: Local Postgres
sudo systemctl start postgresql
sudo -u postgres createdb robson_test

# Apply migration directly via psql
psql -h localhost -U postgres -d postgres -f v2/migrations/002_event_log_phase9.sql

# Verify partitions
psql -h localhost -U postgres -d postgres -c "SELECT * FROM check_partition_coverage();"
```

### Running Tests

```bash
# Quick compile check (no DB needed)
cargo check -p robson-eventlog
cargo check -p robson-projector
cargo check -p robson-db

# Lib tests (no DB needed)
cargo test -p robson-eventlog --lib
cargo test -p robson-projector --lib

# Integration tests with testcontainers (self-contained, spins up Postgres)
cargo test -p robson-projector --test projection_cursor_test

# Integration tests with external DB (requires DATABASE_URL)
DATABASE_URL="postgresql://postgres:test@localhost/postgres" \
  cargo test -p robson-projector --test integration_test -- --ignored
```

### Testcontainers Tests

The `projection_cursor_test` uses testcontainers to automatically spin up PostgreSQL:

```bash
# Requires Docker running
cargo test -p robson-projector --test projection_cursor_test
```

This test verifies:
- Valid events are applied to projections
- Invalid events (invariant violations) block processing
- Cursor does NOT advance past failing events

---

## Current Status

✅ **robson-eventlog**: Event log with append, query, idempotency
✅ **robson-projector**: 10 event handlers (orders, fills, positions, balances, risk, strategy)
✅ **robson-db**: Migration runner, status checker, minimal data seeding
✅ **robson-testkit**: Test helpers for tenant/account/event seeding
✅ **robsond db CLI**: `migrate`, `status`, `init` commands

---

## Completed: Projector → Daemon Wiring

The projection worker is now wired into robsond:
- ✅ Connection pooling via DATABASE_URL
- ✅ Event polling via tenant_id + stream_key cursor
- ✅ Projection task in background tokio spawn
- ✅ Error handling with cursor checkpointing (only advances on success)
- ✅ Graceful shutdown with CancellationToken

Configuration:
- `DATABASE_URL`: Postgres connection string
- `PROJECTION_TENANT_ID`: Tenant UUID for event polling
- `PROJECTION_STREAM_KEY`: Stream key to poll (e.g., "account:test")
- `PROJECTION_POLL_INTERVAL_MS`: Poll interval (default: 100ms)

---

## Next Steps

### Production Readiness
- [ ] **Snapshot Replay**: Bootstrap projections from snapshot + event log
- [ ] **Backfill**: Import historical data from legacy systems
- [ ] **Monitoring**: Metrics for projection lag, events/sec, error rates
- [ ] **S3 Archival**: Cold storage for old event partitions

### Testing
- [ ] **Load Testing**: Simulate high-volume event ingestion
- [ ] **Failure Scenarios**: Test network partitions, DB restarts
- [ ] **Idempotency**: Verify duplicate event handling
