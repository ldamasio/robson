# ADR-0012: Event Sourcing with Postgres Event Log

## Status
**Accepted** (2024-01-15)

## Context

Robson v2 is a non-custodial cryptocurrency trading platform that executes trades on Binance via API using user-provided API keys. As a financial system, we have critical requirements:

1. **Audit Trail**: Immutable record of all trading decisions and exchange interactions
2. **Debugging**: Ability to replay position lifecycle to diagnose issues
3. **Reconciliation**: Compare our trading intent vs actual exchange fills
4. **Compliance**: Meet financial regulations even as a non-custodial platform
5. **Correctness**: Deterministic state reconstruction for testing and recovery
6. **Multi-Tenancy**: Strong tenant isolation at the data layer

### Current State (Pre-Phase 9)

**Database Schema**:
- Mutable projection tables: `positions`, `orders`, `events`
- Events table is append-only but lacks formal event sourcing
- No sequence numbers or stream partitioning
- No snapshots for optimization
- Limited idempotency (only by UUID)

**Problems**:
1. **Cannot replay history**: Events exist but projections can't be rebuilt from them
2. **Reconciliation gaps**: No correlation between our commands and exchange responses
3. **Debugging challenges**: "Why did the system make this decision?" requires log archaeology
4. **Idempotency gaps**: Duplicate network requests can create duplicate database entries
5. **Audit trail incomplete**: No tracking of causation chains or workflow correlation

### Options Considered

We evaluated three approaches:

1. **Full Event Store** (EventStoreDB, Kafka)
2. **Postgres Event Log** (chosen)
3. **Audit Table Only** (status quo)

---

## Decision

We will implement **Postgres-based event sourcing** with:

1. **Append-only event log** (`event_log` table, partitioned by month)
2. **Projection tables** (`orders_current`, `positions_current`, etc.) for fast queries
3. **Periodic snapshots** for optimization
4. **S3 archival** for long-term storage (Contabo object storage)

### Why Postgres (Not Kafka/EventStoreDB)?

**Scale Reality**:
- Non-custodial → volume driven by user actions, not market data streaming
- Expected: 1000 strategies × 100 events/day = 100K events/day
- Postgres handles this easily in a single partitioned table

**Correctness > Scale**:
- Need ACID transactions for `append_event() + update_projection()`
- Trading systems require deterministic replay
- Distributed streaming adds eventual consistency complexity

**Operational Simplicity**:
- One database to monitor, backup, restore
- SQL queries for debugging (no custom query language)
- Standard Postgres tooling (pg_dump, pgBackRest, WAL archival)
- ParadeDB adds search/vectors WITHOUT new infrastructure

**Future-Proof**:
```
Phase 9: Postgres event log (source of truth)
  ↓
Phase 10: Add Kafka as async overlay
  - CDC from Postgres → Kafka
  - Keep Postgres as primary store
  - Kafka for analytics, webhooks, integrations
  - Zero changes to event schema
```

---

## Architecture

### Core Components

```
┌─────────────────────────────────────────────────────┐
│              Command (CLI/API)                       │
└────────────────┬────────────────────────────────────┘
                 │
                 ▼
┌─────────────────────────────────────────────────────┐
│  [Transaction BEGIN]                                 │
│    1. Append Event to event_log                     │
│       - Optimistic concurrency (sequence check)     │
│       - Idempotency key deduplication               │
│    2. Apply Event to Projections                    │
│       - orders_current, positions_current, etc.     │
│    3. Maybe Create Snapshot                         │
│       - Every N events or T minutes                 │
│  [Transaction COMMIT]                               │
└────────────────┬────────────────────────────────────┘
                 │
                 ▼
┌─────────────────────────────────────────────────────┐
│              PostgreSQL (ParadeDB)                   │
│                                                      │
│  event_log (partitioned)   projections   snapshots  │
│  - Immutable               - Mutable     - Checkpoints
│  - Append-only             - Fast reads  - Replay opt
│  - Monthly partitions      - Indexes     - Per scope
│  - S3 archival (3+ months)                          │
└─────────────────────────────────────────────────────┘
```

### Event Log Schema

```sql
CREATE TABLE event_log (
    event_id UUID PRIMARY KEY,
    tenant_id UUID NOT NULL,

    stream_key TEXT NOT NULL,  -- "position:{uuid}"
    seq BIGINT NOT NULL,       -- Monotonic per stream

    event_type VARCHAR(100),
    payload JSONB,

    occurred_at TIMESTAMPTZ,   -- Business time
    ingested_at TIMESTAMPTZ,   -- DB write time

    idempotency_key TEXT UNIQUE,

    trace_id UUID,             -- Per CLI invocation
    causation_id UUID,         -- Event that caused this
    command_id UUID,           -- CLI command
    workflow_id UUID,          -- Detector run

    UNIQUE (stream_key, seq)
) PARTITION BY RANGE (ingested_at);
```

### Key Design Choices

**1. Partition by `ingested_at` (not `occurred_at`)**:
- `ingested_at` is immutable and DB-controlled (no clock skew)
- `occurred_at` can be backfilled or corrected (e.g., exchange timestamp)
- Monotonic partition assignment (no repartitioning needed)

**2. Monthly Partitions (not daily/yearly)**:
- Balance partition count vs partition size
- Easier to manage than daily (fewer partitions)
- Easier to archive than yearly (smaller files)
- ~3M events/partition at 100K/day (~1-2 GB with JSONB)

**3. Idempotency via Semantic Hash (not UUID alone)**:
```rust
idempotency_key = SHA256(
    tenant_id + stream_key + command_id + normalize(payload)
)
// normalize() removes timestamps, actor IDs, request IDs
```
- True semantic deduplication (ignores non-deterministic fields)
- Prevents duplicate events from network retries
- Unique constraint enforces at DB level

**4. Single-Transaction Update (not eventual consistency)**:
- Append event + update projection in one TX (ACID)
- Avoids race conditions and inconsistent states
- Simpler reasoning: "If event exists, projection is updated"

**5. Snapshots per Stream (not global)**:
- Per account, strategy, or position
- Enables fast replay: load snapshot + recent events
- Configurable triggers (event count, time interval, workflow boundary)

---

## Event Taxonomy

**Total**: 44 event types across 10 domains

**Critical Events** (Phase 9 MVP):
- **Orders**: `ORDER_SUBMITTED`, `ORDER_ACKED`, `FILL_RECEIVED`
- **Positions**: `POSITION_OPENED`, `POSITION_CLOSED`
- **Decisions**: `DECISION_PROPOSED`, `INTENT_CREATED`
- **Risk**: `RISK_CHECK_PASSED`, `RISK_CHECK_FAILED`
- **Balances**: `BALANCE_SAMPLED`

**Event Naming Convention**: `<DOMAIN>_<ENTITY>_<PAST_TENSE_ACTION>`

Examples:
- ✅ `POSITION_ARMED` (not `ARM_POSITION`)
- ✅ `FILL_RECEIVED` (not `RECEIVE_FILL`)
- ✅ `RISK_LIMIT_EXCEEDED` (not `CHECK_RISK`)

---

## Idempotency Strategy

### Three Levels of Idempotency

**1. Producer Idempotency (Our Side)**:
```sql
CONSTRAINT uk_event_log_idempotency_key UNIQUE (idempotency_key)
```
- CLI retries with same command → same `idempotency_key` → return existing event
- Network failures handled gracefully

**2. Exchange Idempotency (Binance)**:
```rust
client_order_id: "robson_{ulid}"  // Binance deduplicates within 24h
```
- Order placement retries → Binance returns same `orderId`

**3. Fill Deduplication**:
```sql
CONSTRAINT uk_fills_exchange_trade_id UNIQUE (tenant_id, exchange_trade_id)
```
- WebSocket + REST polling both report fills → deduplicate by `tradeId`

---

## Correlation IDs

Every event has multiple correlation dimensions:

**trace_id**: Per user action (CLI invocation)
- Query: "Show me all events from this user command"

**causation_id**: Event that caused this event
- Query: "What caused this order to be placed?"
- Builds cause-effect chain

**command_id**: CLI command UUID
- Idempotency: Prevent duplicate commands
- Audit: Link back to user's original intent

**workflow_id**: Detector run / reconciliation cycle
- Query: "What signals did detector run #42 produce?"
- Group related events

---

## Partitioning & Archival

### Retention Strategy

- **Hot**: Last 3 months (query directly from Postgres)
- **Warm**: 3-12 months (archived to S3, available via foreign table)
- **Cold**: 12+ months (S3 Glacier, manual restore for forensics)

### Auto-Partition Creation

```sql
CREATE FUNCTION create_event_log_partitions(months_ahead INT)
-- Scheduled monthly via cron
-- Creates partitions 3 months ahead
```

### S3 Archival Process

```bash
# Quarterly for 3+ month old partitions
1. Export partition to CSV.gz
2. Upload to Contabo S3 (s3://robson-event-archive/)
3. Verify upload
4. Detach partition
5. Drop partition
```

### S3 Lifecycle

```json
{
  "Transitions": [{ "Days": 365, "StorageClass": "GLACIER" }],
  "Expiration": { "Days": 2555 }  // 7 years
}
```

---

## Consequences

### Positive ✅

1. **Complete Audit Trail**
   - Immutable record of all trading decisions
   - Compliance with financial regulations
   - Tamper-evident (optional hash chain)

2. **Debugging via Replay**
   - Rebuild state from events: "Why did position enter here?"
   - Regression testing: replay with different engine version
   - Time-travel queries: "What was position state at 2024-01-15 10:00?"

3. **Reconciliation**
   - Compare intent vs exchange fills
   - Detect missing fills, duplicate fills, price slippage
   - Automated reconciliation reports

4. **Idempotent Operations**
   - CLI retries safe (network failures)
   - Exchange duplicate responses handled
   - No duplicate orders or fills

5. **Multi-Tenant Isolation**
   - `tenant_id` on every event
   - Optional Row-Level Security (RLS)
   - Partitioned queries by default

6. **Operational Simplicity**
   - One database to manage
   - Standard SQL for queries and debugging
   - Proven Postgres tooling

7. **Future-Proof**
   - Can add Kafka later via CDC
   - Can add real-time analytics via materialized views
   - Event schema is forward/backward compatible

### Negative ⚠️

1. **Write Amplification**
   - Every command writes: event + projection update + maybe snapshot
   - Mitigated by: batch operations, async snapshots

2. **Partition Management Overhead**
   - Monthly partition creation (automated via cron)
   - Quarterly archival (semi-automated scripts)
   - Monitoring needed (partition coverage alerts)

3. **S3 Archival Complexity**
   - Manual orchestration (no Postgres auto-expiry)
   - Restore requires manual steps
   - Mitigated by: documented runbooks, scripts

4. **Storage Costs**
   - Event log grows indefinitely (100K events/day × 365 days = 36M/year)
   - Mitigated by: S3 archival (~$0.01/GB), Glacier for long-term

5. **Query Performance**
   - Full-table scans expensive on large event log
   - Mitigated by: partitioning, indexes, projections for "current state"

### Risks & Mitigations

| Risk | Impact | Mitigation |
|------|--------|------------|
| Partition coverage gap | Event append fails | Automated creation + monitoring alert |
| S3 archival failure | Storage costs increase | Retry logic + manual verification |
| Replay performance | Long recovery time | Snapshots every N events |
| Event schema evolution | Breaking changes | Schema versioning + backward compat |

---

## Alternatives Considered

### Alternative 1: EventStoreDB

**Pros**:
- Purpose-built event store
- Mature projections system
- Built-in streams and subscriptions

**Cons**:
- New infrastructure dependency (deployment, monitoring, scaling)
- Operational complexity (cluster management)
- Learning curve for team
- Overkill for current scale (100K events/day)

**Decision**: Rejected. Defer to Phase 10 if scale requires.

---

### Alternative 2: Kafka + Kafka Streams

**Pros**:
- Industry standard for event streaming
- High throughput and durability
- Rich ecosystem (Connect, Streams, KSQL)

**Cons**:
- Requires ZooKeeper (or KRaft mode)
- Schema registry needed
- Eventual consistency (harder to reason about)
- Operational overhead (broker management, rebalancing)
- Not a database (need separate storage for projections)

**Decision**: Rejected for Phase 9. Can add later as overlay via CDC.

---

### Alternative 3: MongoDB Change Streams

**Pros**:
- Built-in change data capture
- Flexible schema (JSONB-like)
- Horizontal scaling

**Cons**:
- No multi-document ACID (event + projection not atomic)
- Query limitations vs SQL
- Less mature for financial systems
- Team expertise in Postgres

**Decision**: Rejected. Postgres ACID guarantees more valuable.

---

### Alternative 4: Audit Table Only (Status Quo)

**Pros**:
- Simplest approach
- Already have `events` table
- No new infrastructure

**Cons**:
- Cannot rebuild projections from events (no replay)
- No formal stream partitioning or sequence numbers
- Limited idempotency (UUID only, not semantic)
- No snapshots (full replay always required)
- Debugging still requires log archaeology

**Decision**: Rejected. Insufficient for production readiness.

---

## Implementation

See `docs/PHASE_9_SUMMARY.md` for:
- Detailed implementation plan (4 weeks)
- Event taxonomy (44 types)
- Idempotency scheme
- Projection update mechanism
- Partitioning & archival process
- Testing strategy

**Crates**:
- `robson-eventlog` - Event appending, querying, idempotency
- `robson-projector` - Event-to-projection handlers
- `robson-snapshotter` - Snapshot creation and restoration
- `robson-replayer` - Projection rebuild from events

**Migration**: `v2/migrations/002_event_log_phase9.sql`

---

## Success Metrics

Phase 9 is successful when:

1. **Functional**:
   - ✅ All events stored with idempotency
   - ✅ Projections update atomically
   - ✅ Replay rebuilds projections correctly
   - ✅ Snapshots reduce replay time by 90%+

2. **Performance**:
   - ✅ Event append: 1000 events/sec
   - ✅ Projection update latency: < 10ms
   - ✅ Query by stream: < 50ms (1K events)
   - ✅ Replay from snapshot: < 5 sec

3. **Operational**:
   - ✅ Partitions auto-create monthly
   - ✅ S3 archival works for 3+ month partitions
   - ✅ Monitoring dashboards operational
   - ✅ Runbooks complete

4. **Quality**:
   - ✅ Integration tests pass (order lifecycle, idempotency)
   - ✅ Concurrency tests pass (optimistic locking)
   - ✅ Replay tests pass (deterministic state)

---

## References

- [Martin Fowler: Event Sourcing](https://martinfowler.com/eaaDev/EventSourcing.html)
- [Greg Young: CQRS Documents](https://cqrs.files.wordpress.com/2010/11/cqrs_documents.pdf)
- [Postgres Partitioning](https://www.postgresql.org/docs/current/ddl-partitioning.html)
- [Binance API Idempotency](https://binance-docs.github.io/apidocs/spot/en/#limits)

---

**Decision Date**: 2024-01-15
**Approved By**: Architecture Team
**Review Date**: 2024-06-01 (6 months after deployment)
