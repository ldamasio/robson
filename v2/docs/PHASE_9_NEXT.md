# Phase 9: Next Steps

Quick reference for continuing Phase 9 implementation.

---

## 1. Running the Migration Locally

### Prerequisites

```bash
# Install SQLx CLI (if not already installed)
cargo install sqlx-cli --no-default-features --features postgres

# Ensure PostgreSQL 14+ is running
psql --version
```

### Database Setup

```bash
# Create database
createdb robson_v2_dev

# Set DATABASE_URL
export DATABASE_URL="postgresql://localhost/robson_v2_dev"

# Run migration
cd v2
sqlx migrate run --source migrations

# Verify partitions created
psql $DATABASE_URL -c "SELECT tablename FROM pg_tables WHERE schemaname = 'public' AND tablename LIKE '%_202%' ORDER BY tablename;"

# Check partition coverage
psql $DATABASE_URL -c "SELECT * FROM check_partition_coverage();"
```

---

## 2. Testing robson-eventlog

### Compile Check

```bash
cargo check -p robson-eventlog
```

### Run Tests

```bash
# Run all tests (unit only, no DB yet)
cargo test -p robson-eventlog

# Run with output
cargo test -p robson-eventlog -- --nocapture

# Run specific test
cargo test -p robson-eventlog test_idempotency_key_deterministic
```

### Lint

```bash
cargo clippy -p robson-eventlog -- -D warnings
```

**Current Status**: ✅ 5/5 tests passing (idempotency tests)

---

## 3. Next Step: Create robson-projector Crate

### Checklist (8 items)

- [ ] **1. Create crate structure**
  ```bash
  mkdir -p robson-projector/src/handlers
  ```

- [ ] **2. Add to workspace** (`v2/Cargo.toml` members)

- [ ] **3. Create `Cargo.toml`** with dependencies:
  - robson-domain, robson-eventlog
  - sqlx, tokio, serde, uuid, chrono, rust_decimal

- [ ] **4. Implement `src/lib.rs`** - Public API exports

- [ ] **5. Implement `src/apply.rs`** - Main dispatcher:
  ```rust
  pub async fn apply_event_to_projections(tx: &mut Transaction, event: &EventEnvelope) -> Result<()>
  ```

- [ ] **6. Implement handlers** (10 critical event types):
  - `handlers/orders.rs`: ORDER_SUBMITTED, ORDER_ACKED, FILL_RECEIVED
  - `handlers/positions.rs`: POSITION_OPENED, POSITION_CLOSED
  - `handlers/decisions.rs`: DECISION_PROPOSED, INTENT_CREATED
  - `handlers/risk.rs`: RISK_CHECK_PASSED, RISK_CHECK_FAILED
  - `handlers/balances.rs`: BALANCE_SAMPLED

- [ ] **7. Write unit tests** - Mock transaction, verify SQL updates

- [ ] **8. Integration test** (optional) - Requires test DB
  - Append event + verify projection updated atomically

### Implementation Order

1. Start with **orders.rs** (simplest: 3 events)
2. Then **positions.rs** (core domain: 2 events)
3. Then **balances.rs**, **risk.rs**, **decisions.rs**

### Estimated Effort

- ~2-3 hours for crate scaffolding + dispatcher
- ~30 min per handler module
- Total: ~4-5 hours for MVP (10 event types)
