-- bronze-v1 §4 index prerequisite (Phase B), non-blocking strategy.
--
-- event_log: the parent index is created ON ONLY, a metadata-only
-- operation that takes no lock on any partition and leaves the parent
-- index invalid until every child is attached. Child indexes are built
-- CREATE INDEX CONCURRENTLY and attached in the gated Phase B provisioning
-- runbook (CONCURRENTLY cannot run inside a migration transaction, and the
-- historical partitions were DELETE-pruned but never rewritten, so their
-- physical heaps may still be large; a plain recursive build would hold a
-- SHARE lock on the whole hierarchy, blocking robsond writes, until
-- commit). Future partitions created by create_event_log_partitions
-- (CREATE TABLE ... PARTITION OF) automatically receive a matching child
-- index.
--
-- income_ledger is a small, never-bloated regular table; it gets a normal
-- build guarded by local timeouts: if the lock or the build cannot be
-- acquired fast, the migration fails closed and is retried in a window
-- instead of queueing behind writers indefinitely.

SET LOCAL lock_timeout = '2s';
SET LOCAL statement_timeout = '60s';

CREATE INDEX idx_event_log_bronze_cursor
    ON ONLY event_log (ingested_at ASC, event_id ASC);

-- COLLATE "C" pins bytewise text ordering for the cursor tiebreaker so the
-- contract's canonical row order never depends on the database locale.
CREATE INDEX idx_income_ledger_bronze_cursor
    ON income_ledger (created_at ASC, exchange_income_id COLLATE "C" ASC);

COMMENT ON INDEX idx_event_log_bronze_cursor IS
    'bronze-v1 §4 cursor index (parent, ON ONLY): children are built CONCURRENTLY and attached in the gated Phase B runbook; the exporter must verify indisvalid before first use';
COMMENT ON INDEX idx_income_ledger_bronze_cursor IS
    'bronze-v1 §4 cursor index: (created_at, exchange_income_id COLLATE "C") keyset pagination for the bronze exporter';
