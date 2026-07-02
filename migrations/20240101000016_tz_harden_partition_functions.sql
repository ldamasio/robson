-- TZ-harden partition maintenance functions.
--
-- Incident 2026-07-02: event_log/snapshots partitions ran out at 2026-06 and
-- month_boundary_reset appends failed with "no partition of relation found".
-- Manual remediation then hit a second trap: the functions computed bounds as
-- DATE literals, which Postgres casts to timestamptz using the SESSION time
-- zone. The server default (+02) produced bounds overlapping the existing
-- UTC-aligned partitions. These replacements pin every bound to UTC
-- explicitly, so the functions behave identically from any session.
--
-- Also raises the created window: robsond now calls these at each month
-- boundary (see robsond daemon), keeping months_ahead of headroom.

CREATE OR REPLACE FUNCTION create_event_log_partitions(months_ahead INT)
RETURNS VOID AS $$
DECLARE
    start_month TIMESTAMP; -- naive UTC month start
    start_ts TIMESTAMPTZ;
    end_ts TIMESTAMPTZ;
    partition_name TEXT;
BEGIN
    -- Create current month + N future months (i=0 to months_ahead)
    FOR i IN 0..months_ahead LOOP
        start_month := DATE_TRUNC('month', (NOW() AT TIME ZONE 'UTC') + (i || ' months')::INTERVAL);
        start_ts := start_month AT TIME ZONE 'UTC';
        end_ts := (start_month + INTERVAL '1 month') AT TIME ZONE 'UTC';
        partition_name := 'event_log_' || TO_CHAR(start_month, 'YYYY_MM');

        IF NOT EXISTS (
            SELECT 1 FROM pg_class WHERE relname = partition_name
        ) THEN
            EXECUTE format(
                'CREATE TABLE %I PARTITION OF event_log FOR VALUES FROM (%L) TO (%L)',
                partition_name, start_ts, end_ts
            );

            RAISE NOTICE 'Created partition: %', partition_name;
        END IF;
    END LOOP;
END;
$$ LANGUAGE plpgsql;

COMMENT ON FUNCTION create_event_log_partitions IS 'Auto-create future event_log partitions (UTC-aligned, session-TZ independent)';

CREATE OR REPLACE FUNCTION create_snapshot_partitions(months_ahead INT)
RETURNS VOID AS $$
DECLARE
    start_month TIMESTAMP; -- naive UTC month start
    start_ts TIMESTAMPTZ;
    end_ts TIMESTAMPTZ;
    partition_name TEXT;
BEGIN
    FOR i IN 0..months_ahead LOOP
        start_month := DATE_TRUNC('month', (NOW() AT TIME ZONE 'UTC') + (i || ' months')::INTERVAL);
        start_ts := start_month AT TIME ZONE 'UTC';
        end_ts := (start_month + INTERVAL '1 month') AT TIME ZONE 'UTC';
        partition_name := 'snapshots_' || TO_CHAR(start_month, 'YYYY_MM');

        IF NOT EXISTS (
            SELECT 1 FROM pg_class WHERE relname = partition_name
        ) THEN
            EXECUTE format(
                'CREATE TABLE %I PARTITION OF snapshots FOR VALUES FROM (%L) TO (%L)',
                partition_name, start_ts, end_ts
            );

            RAISE NOTICE 'Created partition: %', partition_name;
        END IF;
    END LOOP;
END;
$$ LANGUAGE plpgsql;

COMMENT ON FUNCTION create_snapshot_partitions IS 'Auto-create future snapshot partitions (UTC-aligned, session-TZ independent)';
