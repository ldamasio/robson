# Phase 9: Next Steps

## Local Development

### Prerequisites
- PostgreSQL 16+ (or Docker)
- psql client
- Rust toolchain

### Running Migration 002 Locally

```bash
# Option A: Docker Postgres
docker run --rm -p 5432:5432 -e POSTGRES_PASSWORD=test postgres:16

# Option B: Local Postgres
sudo systemctl start postgresql
sudo -u postgres createdb robson_test

# Apply migration
psql -h localhost -U postgres -d postgres -f v2/migrations/002_event_log_phase9.sql

# Verify partitions
psql -h localhost -U postgres -d postgres -c "SELECT * FROM check_partition_coverage();"
```

### Running Tests

```bash
# Quick compile check (no DB needed)
cargo check -p robson-eventlog
cargo check -p robson-projector

# Lib tests (no DB needed)
cargo test -p robson-eventlog --lib
cargo test -p robson-projector --lib

# Integration tests (requires DATABASE_URL)
DATABASE_URL="postgresql://postgres:test@localhost/postgres" \
  cargo test -p robson-projector --test integration_test -- --ignored
```

---

## Current Status

✅ **robson-eventlog**: Event log with append, query, idempotency
✅ **robson-projector**: 10 event handlers (orders, fills, positions, balances, risk, strategy)

---

## Next: Projector → Daemon Wiring

Before implementing replay/catchup, wire the projector into robsond:

- [ ] **1. Connection Pool**: Add `sqlx::PgPool` to `robsond/src/state.rs`
- [ ] **2. Event Stream**: Create `robsond/src/event_stream.rs` - polls `event_log` for new events
- [ ] **3. Projector Task**: Spawn tokio task calling `apply_event_to_projections` for each event
- [ ] **4. Error Handling**: Log but don't crash on `ProjectionError`
- [ ] **5. Metrics**: Track events processed/sec, last processed seq per stream
- [ ] **6. Health Endpoint**: GET /health returns projection lag (max `seq - last_seq`)
- [ ] **7. Shutdown**: Graceful drain of in-flight events on SIGTERM
- [ ] **8. Config**: Add `database_url` to `robsond/src/config.rs`

After wiring: replay/catchup can bootstrap from snapshot + event log.
