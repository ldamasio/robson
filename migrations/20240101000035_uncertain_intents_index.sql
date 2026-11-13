-- no-transaction
--
-- ADR-0056 step 7, PR F: the index on the intents a release tombstones, and
-- the pending list's uncertain intents: stamped, with no outcome, not proven
-- unsent and no disposition.
--
-- Built CONCURRENTLY so it never blocks the journal's writes to order_intents
-- while the daemon runs during the deploy's migration hook: a plain CREATE
-- INDEX takes a lock that conflicts with every insert and update for as long
-- as the build scans the table, and a journal commit waits at most 500 ms. The
-- first line tells SQLx 0.8.6 to run this file outside a transaction, which
-- CREATE INDEX CONCURRENTLY requires; SQLx sends it as one simple query, so
-- the file holds this one statement and nothing else.
--
-- A build that fails partway leaves an invalid index and no migration row;
-- the next run's IF NOT EXISTS then accepts that index as it is. Queries stay
-- correct without a valid index, only slower: check pg_index.indisvalid for
-- idx_order_intents_uncertain_open after the deploy, and if it is false, drop
-- the index and create it again by hand with this same statement.
CREATE INDEX CONCURRENTLY IF NOT EXISTS idx_order_intents_uncertain_open
    ON order_intents (tenant_id, uncertain_generation)
    WHERE uncertain_generation IS NOT NULL
      AND venue_outcome IS NULL
      AND NOT not_sent
      AND disposition_kind IS NULL;
