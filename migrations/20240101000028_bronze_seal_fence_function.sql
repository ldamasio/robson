-- bronze-v1 §3.2 sealing fence (Phase B).
--
-- The exporter calls this function in autocommit BEFORE acquiring its
-- REPEATABLE READ READ ONLY snapshot, and again AFTER the snapshot ends
-- (second fence + window re-count) before publishing a commit marker.
--
-- SECURITY DEFINER so the export role does not need pg_read_all_stats.
-- Ownership: created by the migration role (the robson application user);
-- all writers to event_log/income_ledger connect as this same user, so
-- their pg_stat_activity rows are fully visible to the definer. Sessions
-- the definer cannot read (other users, autovacuum/background workers in
-- this database, transient internal states) surface as
-- unreadable_sessions > 0, which the exporter treats as fail-closed (the
-- window stays open). The gated provisioning runbook SHOULD transfer
-- ownership to a dedicated NOLOGIN role holding pg_read_all_stats to
-- eliminate those availability false positives without granting stats
-- visibility to the exporter; after such a transfer, this migration must
-- never be re-run manually (CREATE OR REPLACE would fail on ownership,
-- by design).
--
-- STRICT: a NULL window_end returns NULL instead of a false-clear row
-- (comparisons against NULL would report zero old writers).
--
-- EXECUTE is granted to the bronze export role in the gated provisioning
-- runbook, not here (the role does not exist at migration time).

CREATE FUNCTION public.bronze_seal_fence(window_end timestamptz)
RETURNS TABLE (
    db_now              timestamptz,
    in_recovery         boolean,
    old_writer_xacts    bigint,
    unreadable_sessions bigint,
    prepared_xacts      bigint
)
LANGUAGE sql
VOLATILE
STRICT
SECURITY DEFINER
SET search_path = pg_catalog, pg_temp
AS $$
    SELECT
        clock_timestamp(),
        pg_is_in_recovery(),
        -- Transactions that began before window_end can still commit rows
        -- whose ingested_at/created_at (NOW() = xact start) falls inside
        -- the window. Any such transaction blocks sealing.
        (SELECT count(*)
           FROM pg_stat_activity a
          WHERE a.datname = current_database()
            AND a.pid <> pg_backend_pid()
            AND a.xact_start IS NOT NULL
            AND a.xact_start < window_end),
        -- Sessions in this database whose state the definer cannot read
        -- (masked other-user sessions, autovacuum/background workers,
        -- transient internal states). Fail closed: the exporter must not
        -- seal while any session is unreadable.
        (SELECT count(*)
           FROM pg_stat_activity a
          WHERE a.datname = current_database()
            AND a.pid <> pg_backend_pid()
            AND a.state IS NULL),
        -- Prepared transactions are invisible in pg_stat_activity
        -- (bronze-v1 §3.2: 2PC must be disabled or fenced here).
        (SELECT count(*)
           FROM pg_prepared_xacts p
          WHERE p.database = current_database());
$$;

REVOKE ALL ON FUNCTION public.bronze_seal_fence(timestamptz) FROM PUBLIC;

COMMENT ON FUNCTION public.bronze_seal_fence(timestamptz) IS
    'bronze-v1 §3.2 sealing fence: db clock, recovery flag, writer transactions older than window_end, unreadable sessions, prepared xacts. Fail-closed on any nonzero count or in_recovery. STRICT. EXECUTE granted only to the bronze export role (provisioning runbook).';
