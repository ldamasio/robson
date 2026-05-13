# Phase 9: Event Log + Projections + Snapshots

**Status**: In Progress
**Started**: 2024-01-15
**Target Completion**: 4 weeks

## Executive Summary

Phase 9 implements a Postgres-based event sourcing system for Robson v2 to provide:
- **Audit Trail**: Immutable record of all trading decisions and exchange interactions
- **Debugging**: Replay position lifecycle to diagnose issues
- **Reconciliation**: Compare our intent vs exchange fills
- **Compliance**: Meet financial regulations for non-custodial trading
- **Correctness**: Deterministic state reconstruction

### Why Skip Phases 7-8?

**Decision**: Implement Postgres append-only event log NOW instead of distributed streaming infrastructure.

**Rationale**:
1. **Non-Custodial**: We don't hold user funds → simpler audit requirements
2. **Scale**: Current volume (~100K events/day) easily handled by Postgres
3. **Correctness First**: Need replay and debugging for production readiness
4. **Future-Proof**: Can add Kafka/streaming later as overlay without schema changes

See `docs/adr/ADR-0012-event-sourcing.md` for detailed justification.

---

## Architecture Overview

### Core Components

```
┌─────────────────────────────────────────────────────────────┐
│                     CLI / API Command                        │
└────────────────────────┬────────────────────────────────────┘
                         │
                         ▼
┌─────────────────────────────────────────────────────────────┐
│                   robsond (Orchestration)                    │
│  ┌──────────────────────────────────────────────────────┐   │
│  │  1. Validate Command                                  │   │
│  │  2. Call Engine for Decision                          │   │
│  │  3. TX: Append Event + Update Projections            │   │
│  │  4. Execute Action (via Connector)                    │   │
│  └──────────────────────────────────────────────────────┘   │
└────────────────────────┬────────────────────────────────────┘
                         │
                         ▼
┌─────────────────────────────────────────────────────────────┐
│                PostgreSQL (ParadeDB)                         │
│                                                               │
│  ┌──────────────────┐  ┌──────────────────┐  ┌───────────┐  │
│  │   event_log      │  │  Projections     │  │ snapshots │  │
│  │ (append-only)    │  │ (current state)  │  │ (checkpoints)│
│  │                  │  │                  │  │           │  │
│  │ - Partitioned    │  │ - orders_current │  │ - account │  │
│  │ - Monthly        │  │ - positions_*    │  │ - strategy│  │
│  │ - S3 archival    │  │ - balances_*     │  │ - position│  │
│  └──────────────────┘  └──────────────────┘  └───────────┘  │
└─────────────────────────────────────────────────────────────┘
```

### Data Flow

```
Command → [TX BEGIN]
  ├─ 1. Append Event to event_log (with idempotency check)
  ├─ 2. Update Projection (orders_current, positions_current, etc.)
  ├─ 3. Maybe Create Snapshot (if trigger condition met)
  └─ [TX COMMIT]

Background Jobs:
  - Create future partitions (monthly cron)
  - Archive old partitions to S3 (quarterly)
  - Reconciliation (hourly)
  - Event search indexer (optional)
```

---

## Completed Work

### ✅ 1. Database Migration (`migrations/002_event_log_phase9.sql`)

**Tables Created**:
- `event_log` - Append-only event log (partitioned by `ingested_at`)
- `orders_current` - Current state of orders
- `positions_current` - Current state of positions
- `balances_current` - Latest sampled balances
- `risk_state_current` - Risk metrics per account/strategy
- `strategy_state_current` - Strategy configuration and performance
- `snapshots` - Periodic checkpoints (partitioned)
- `stream_state` - Sequence tracking per stream
- `commands` - Command idempotency
- `fills` - Exchange fill deduplication

**Functions**:
- `next_seq(stream_key, tenant_id)` - Atomic sequence generation
- `create_event_log_partitions(months_ahead)` - Auto-create partitions
- `create_snapshot_partitions(months_ahead)` - Auto-create snapshot partitions
- `check_partition_coverage()` - Monitoring

**Indexes**: 20+ indexes for multi-tenant queries, stream reads, correlation lookups

**Initial Partitions**: Jan-Mar 2024 created

### ✅ 2. Event Log Crate (`robson-eventlog/`)

**Modules**:
- `types.rs` - Core event types (`Event`, `EventEnvelope`, `ActorType`)
- `idempotency.rs` - Semantic payload hashing for deduplication (✅ **Tests Passing**)
- `append.rs` - Event appending with optimistic concurrency
- `query.rs` - Event querying with flexible filters

**Features**:
- Idempotency via SHA256 hash of normalized payload
- Optimistic concurrency control with sequence numbers
- Multi-tenant isolation
- Correlation IDs (trace, causation, command, workflow)
- Typed actor tracking (CLI, Daemon, System, Exchange)

**Status**: Core implementation complete, needs integration tests

---

## Event Taxonomy (44 Event Types)

### Critical Events (Phase 9 MVP)

| Domain | Event Type | Emitted By | Payload Highlights |
|--------|-----------|------------|-------------------|
| **Orders** | ORDER_SUBMITTED | Executor | order_id, client_order_id, symbol, quantity |
| | ORDER_ACKED | Connector | exchange_order_id, status |
| | FILL_RECEIVED | Connector | exchange_trade_id, fill_price, fill_quantity, fee |
| **Positions** | POSITION_OPENED | Engine | position_id, entry_price, technical_stop |
| | POSITION_CLOSED | Engine | realized_pnl, exit_reason |
| **Decisions** | DECISION_PROPOSED | Engine | signal_id, quantity (from risk calc) |
| | INTENT_CREATED | Executor | intent_id, intent_type |
| **Risk** | RISK_CHECK_PASSED | Risk Engine | checks_performed |
| | RISK_CHECK_FAILED | Risk Engine | failed_checks, severity |
| **Balances** | BALANCE_SAMPLED | Daemon | asset, free, locked, total |

See full taxonomy in design document above (section 2).

---

## Idempotency Scheme

### Producer Idempotency (Our Side)

**Formula**:
```
idempotency_key = "idem_" + SHA256(
    tenant_id + stream_key + command_id + normalize(payload)
)
```

**Normalization Rules**:
- Remove: `*_at` timestamps, `actor_id`, `actor_type`, `request_id`
- Keep: All business data (prices, quantities, symbols, entity IDs)

**Database Constraint**:
```sql
CONSTRAINT uk_event_log_idempotency_key UNIQUE (idempotency_key)
```

**Behavior**: On duplicate → return existing `event_id` (idempotent retry)

### Exchange Idempotency

| Robson Field | Binance Field | Purpose |
|--------------|---------------|---------|
| `client_order_id` | `newClientOrderId` | Order placement deduplication (24h window) |
| `exchange_order_id` | `orderId` | Binance's internal ID |
| `exchange_trade_id` | `tradeId` | Fill deduplication |

**Fill Deduplication**:
```sql
CONSTRAINT uk_fills_exchange_trade_id UNIQUE (tenant_id, exchange_trade_id)
INSERT ... ON CONFLICT DO NOTHING
```

### Correlation IDs

- **trace_id**: Per CLI invocation (tracks all events from one user action)
- **causation_id**: Event that caused this event (cause-effect chain)
- **command_id**: CLI command UUID
- **workflow_id**: Detector run / reconciliation cycle

---

## Projection Update Mechanism

### Single-Transaction Pattern

```rust
async fn handle_command(command: Command) -> Result<()> {
    let mut tx = db.begin().await?;

    // 1. Append event
    let event = build_event(command)?;
    let event_id = append_event_tx(&mut tx, stream_key, None, event).await?;

    // 2. Apply to projections
    apply_event_to_projections(&mut tx, &event).await?;

    // 3. Maybe snapshot
    maybe_create_snapshot(&mut tx, &event).await?;

    // 4. Commit (atomic!)
    tx.commit().await?;

    Ok(())
}
```

### Optimistic Concurrency

```rust
// Verify expected sequence
let next_seq = get_next_seq(tx, stream_key, tenant_id, Some(expected_seq)).await?;
// ↑ Fails with ConcurrentModification if another process updated stream

// Retry logic (application layer)
for retry in 0..MAX_RETRIES {
    match append_event(pool, stream_key, Some(current_seq), event).await {
        Ok(event_id) => return Ok(event_id),
        Err(EventLogError::ConcurrentModification { actual, .. }) => {
            current_seq = actual;  // Update and retry
        }
        Err(e) => return Err(e),
    }
}
```

### Replay for Recovery

```rust
async fn rebuild_projections(
    pool: &PgPool,
    tenant_id: Uuid,
    stream_key: Option<&str>,
    from_snapshot: bool,
) -> Result<()> {
    let start_seq = if from_snapshot {
        let snapshot = load_latest_snapshot(pool, tenant_id).await?;
        restore_projections_from_snapshot(&snapshot).await?;
        snapshot.as_of_seq
    } else {
        0
    };

    // Stream events
    let events = query_events(pool, QueryOptions::new(tenant_id)
        .stream(stream_key.unwrap_or("*"))
        .seq_range(start_seq + 1, i64::MAX)
    ).await?;

    // Replay
    for event in events {
        apply_event_to_projections(&mut tx, &event).await?;
    }

    Ok(())
}
```

---

## Partitioning & Archival

### Partition Strategy

**Approach**: Range partition by `ingested_at` (monthly)

**Why `ingested_at` vs `occurred_at`?**
- `ingested_at` is immutable and DB-controlled (no clock skew)
- `occurred_at` can be backfilled or corrected
- Monotonic partition assignment

**Auto-Creation** (scheduled monthly):
```bash
#!/bin/bash
# cron: 0 0 1 * * (first day of month)
psql $DATABASE_URL -c "SELECT create_event_log_partitions(3);"
```

### S3 Archival (Quarterly for 3+ month old partitions)

**Process**:
1. Export partition to gzipped CSV
2. Upload to Contabo S3 (`s3://robson-event-archive/event_log/`)
3. Verify upload
4. Detach partition from parent table
5. Drop partition (after backup confirmation)

**Restore**:
1. Download from S3
2. Create partition
3. Attach to parent table
4. Import data

**Lifecycle**:
- Hot: Last 3 months (query directly)
- Warm: 3-12 months (S3, available via foreign table)
- Cold: 12+ months (S3 Glacier, manual restore for forensics)

---

## Remaining Work

### 🚧 High Priority (Week 2-3)

#### 1. Projector Crate (`robson-projector/`)
- [ ] Create crate structure
- [ ] Implement `apply_event_to_projections()` dispatcher
- [ ] Add handlers for critical events (10 types):
  - `ORDER_SUBMITTED`, `ORDER_ACKED`, `FILL_RECEIVED`
  - `POSITION_OPENED`, `POSITION_CLOSED`
  - `DECISION_PROPOSED`, `INTENT_CREATED`
  - `RISK_CHECK_PASSED`, `RISK_CHECK_FAILED`
  - `BALANCE_SAMPLED`
- [ ] Unit tests with in-memory DB

#### 2. Snapshotter Crate (`robson-snapshotter/`)
- [ ] Create crate structure
- [ ] Implement `create_snapshot()` (account, strategy, position scopes)
- [ ] Implement `load_snapshot()` + `restore_projections()`
- [ ] Snapshot triggers:
  - Event-count based (every N events)
  - Time-interval based (every T minutes)
  - Workflow boundary (end of cycle)
- [ ] Tests for snapshot + replay

#### 3. Replayer Crate (`robson-replayer/`)
- [ ] Create crate structure
- [ ] Implement `rebuild_projections()` (full tenant or single stream)
- [ ] Support replay from snapshot
- [ ] Validation: compare replayed state vs current projections
- [ ] CLI command: `robson replay --account-id <uuid> --from-snapshot`

#### 4. Integration (Wire to Daemon)
- [ ] Update `robsond` to use event log for all state changes
- [ ] Replace direct DB writes with `append_event() + apply_projections()`
- [ ] Handle idempotent retries (network failures)
- [ ] Integration tests:
  - Order lifecycle (submit → ack → fill → position open)
  - Idempotent duplicate detection
  - Concurrent modification retry

### ⚠️ Medium Priority (Week 4)

#### 5. Partition Management Scripts
- [ ] `scripts/create-partitions.sh` (cron monthly)
- [ ] `scripts/archive-partition.sh` (manual/quarterly)
- [ ] `scripts/restore-partition.sh` (forensics)
- [ ] `scripts/check-partition-coverage.sh` (monitoring)
- [ ] S3 bucket setup (Contabo)
- [ ] S3 lifecycle policy (Glacier after 1 year)

#### 6. CLI Event Commands
- [ ] `robson events list --stream <key> --limit 100`
- [ ] `robson events search --trace-id <uuid>`
- [ ] `robson events search --position-id <uuid>`
- [ ] `robson events replay --account-id <uuid>`

#### 7. Documentation
- [ ] ADR-0012: Event Sourcing (design rationale)
- [ ] PHASE_9.md: Implementation guide
- [ ] Runbook: Event log archival
- [ ] Runbook: Projection rebuild
- [ ] Update CLAUDE.md with Phase 9 context

### 🔮 Optional (Post-MVP)

#### 8. ParadeDB Event Search
- [ ] Create `event_search` table
- [ ] Async indexer (background worker)
- [ ] BM25 full-text search
- [ ] Vector embeddings for semantic search
- [ ] CLI: `robson events search --query "insufficient balance"`
- [ ] Feature flag: `event-search`

#### 9. Event Handlers (Remaining 34 event types)
- [ ] Strategy lifecycle (4 events)
- [ ] Signals (5 events)
- [ ] Connector/Exchange (5 events)
- [ ] Margin (4 events)
- [ ] Observability (4 events)
- [ ] Tenant/Identity (3 events - defer)

---

## Testing Strategy

### Unit Tests
- ✅ Idempotency key normalization (passing)
- [ ] Event append with optimistic concurrency
- [ ] Projection handlers (per event type)
- [ ] Snapshot create + restore

### Integration Tests
- [ ] Full command flow (CLI → Event → Projection → Binance)
- [ ] Idempotent retry scenarios
- [ ] Concurrent modification handling
- [ ] Replay from events
- [ ] Replay from snapshot

### Performance Tests
- [ ] Append throughput (target: 1000 events/sec)
- [ ] Query performance (partitioned table)
- [ ] Projection update latency
- [ ] Snapshot size and restore time

---

## Metrics & Monitoring

### Key Metrics
- Event append rate (events/sec)
- Event lag (time between occurred_at and ingested_at)
- Projection update latency
- Partition coverage (months ahead)
- Idempotent duplicate rate
- Concurrent modification retry rate

### Alerts
- Partition coverage < 2 months ahead
- Event lag > 5 seconds
- Projection update failures
- Snapshot creation failures
- S3 archival failures

---

## Migration Plan

### Phase 9a: Schema Setup (Week 1)
1. Review and test migration locally
2. Run migration on staging
3. Verify partitions created
4. Test event append + query

### Phase 9b: Core Implementation (Week 2)
1. Implement projector + snapshotter
2. Integration tests
3. Wire to daemon (feature flag)
4. Deploy to staging

### Phase 9c: Hardening (Week 3)
1. Implement replayer
2. Performance testing
3. Idempotency stress tests
4. Deploy to production (read-only mode)

### Phase 9d: Operational (Week 4)
1. Partition management scripts
2. S3 archival setup
3. Monitoring dashboards
4. Documentation
5. Enable write mode in production

---

## Dependencies

### Rust Crates
- `sqlx` 0.7 (Postgres, JSON, UUID, Decimal)
- `tokio` 1.35 (async runtime)
- `serde` 1.0 + `serde_json` (serialization)
- `uuid` 1.6 (identifiers)
- `ulid` 1.1 (time-sortable IDs)
- `chrono` 0.4 (timestamps)
- `rust_decimal` 1.33 (precision math)
- `sha2` 0.10 (hashing)
- `thiserror` 1.0 (error handling)

### Database
- PostgreSQL 14+ (or ParadeDB compatible)
- Extensions: `uuid-ossp`, `pg_cron` (optional)

### Infrastructure
- Contabo S3-compatible object storage
- Cron for scheduled jobs

---

## Files Created (This Session)

```
v2/
├── migrations/
│   └── 002_event_log_phase9.sql          ✅ COMPLETE
│
├── robson-eventlog/                       ✅ COMPLETE (needs integration tests)
│   ├── Cargo.toml
│   ├── src/
│   │   ├── lib.rs
│   │   ├── types.rs                      ✅ Event types defined
│   │   ├── idempotency.rs                ✅ Tests passing
│   │   ├── append.rs                     ✅ Core logic complete
│   │   └── query.rs                      ✅ Query builder complete
│   └── tests/                            ⚠️ TODO: Integration tests
│
└── docs/
    └── PHASE_9_SUMMARY.md                ✅ THIS FILE
```

---

## Next Steps

1. **Create `robson-projector` crate**
   - Dispatcher for event types
   - Handlers for critical 10 event types
   - Tests for projection updates

2. **Create `robson-snapshotter` crate**
   - Snapshot creation per scope
   - Restore logic
   - Trigger conditions

3. **Create `robson-replayer` crate**
   - Full tenant replay
   - Single stream replay
   - Validation checks

4. **Update Cargo workspace**
   - Add new crates to `v2/Cargo.toml`
   - Update dependencies

5. **Wire to `robsond`**
   - Replace direct writes with event append
   - Add feature flag for gradual rollout
   - Integration tests

6. **Create operational scripts**
   - Partition management
   - S3 archival
   - Monitoring

---

## Questions & Decisions

### Open Questions
1. **Snapshot frequency**: Every 100 events or every 1 hour? (Decision: Make configurable)
2. **S3 bucket naming**: `robson-events-{env}` or `robson-event-archive`? (Decision: `robson-event-archive` with env prefix in path)
3. **Event search**: Build now or defer to Phase 10? (Decision: Optional feature flag)

### Decisions Made
- ✅ Use Postgres event log (not Kafka) for Phase 9
- ✅ Partition by `ingested_at` (not `occurred_at`)
- ✅ Monthly partitions (not daily or yearly)
- ✅ Idempotency via semantic payload hash (not UUID alone)
- ✅ Single-transaction event + projection update (not eventual consistency)
- ✅ ULID for event_id (not BIGSERIAL, better for distributed future)

---

## Success Criteria

Phase 9 is complete when:
- ✅ All 44 event types defined and documented
- ✅ Event log accepts and stores events with idempotency
- ✅ Projections update atomically with events
- ✅ Snapshots can be created and restored
- ✅ Replay rebuilds projections correctly
- ✅ Partitions auto-create monthly
- ✅ S3 archival works for old partitions
- ✅ Integration tests pass (order lifecycle, idempotency, concurrency)
- ✅ Performance meets targets (1000 events/sec append)
- ✅ Monitoring dashboards operational
- ✅ Documentation complete (ADR, runbooks, CLAUDE.md)
- ✅ Deployed to production (gradual rollout with feature flag)

---

**Document Version**: 1.0
**Last Updated**: 2024-01-15
**Author**: Claude Code (Sonnet 4.5)
**Reviewers**: TBD
