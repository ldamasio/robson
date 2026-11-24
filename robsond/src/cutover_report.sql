-- The cutover report's database part (ADR-0056 step 8m-3, 8m PLAN-r4 §6.2): C7's durable clauses, C8, C9a-d, SD14
-- and C10's previous basis, as one statement of a REPEATABLE READ, READ ONLY transaction; it only reads. $1 tenant,
-- $2 venue, $3 this process's epoch, $4 the terminal last event types of a position stream. The month is the
-- statement's UTC month; every value a check compares is returned with its verdict.
WITH m AS (
    SELECT d::DATE AS month, d AT TIME ZONE 'UTC' AS since, (d + INTERVAL '1 month') AT TIME ZONE 'UTC' AS until
    FROM date_trunc('month', now() AT TIME ZONE 'UTC') AS d),
epochs AS (
    SELECT epoch_id, clean_stop_at IS NOT NULL AND marker_version = 2 AS clean, row_number() OVER (ORDER BY epoch_seq DESC) AS n
    FROM public.process_epochs WHERE tenant_id = $1 ORDER BY epoch_seq DESC LIMIT 2),
-- event_log's attached partitions, by their bounds: a missing or detached one hides its events from C9a.
parts AS (
    SELECT substring(b FROM 'FROM \(''([^'']+)''\)')::TIMESTAMPTZ AS lo, substring(b FROM 'TO \(''([^'']+)''\)')::TIMESTAMPTZ AS hi
    FROM pg_inherits i JOIN pg_class c ON c.oid = i.inhrelid, pg_get_expr(c.relpartbound, c.oid) AS b
    WHERE i.inhparent = 'public.event_log'::REGCLASS),
-- The last event of each of the tenant's position streams (C9b).
streams AS (
    SELECT DISTINCT ON (stream_key) stream_key, event_type FROM public.event_log
    WHERE tenant_id = $1 AND stream_key LIKE 'position:%' ORDER BY stream_key, seq DESC),
-- The capital field of the month's latest reset or recalibration, whoever appended it (C9c).
latest AS (
    SELECT CASE e.event_type WHEN 'month_boundary_reset' THEN e.payload ->> 'capital_base'
                ELSE e.payload ->> 'new_capital_base' END::NUMERIC AS capital
    FROM public.event_log e, m
    WHERE e.stream_key = 'system:month_boundary:' || to_char(m.month, 'YYYY-MM')
      AND e.event_type IN ('month_boundary_reset', 'capital_base_recalibrated')
    ORDER BY e.seq DESC LIMIT 1),
f AS (
    SELECT m.month, s.capital_base, s.boundary_reset_at IS NOT NULL AS reset_landed, s.month_peak_net,
        (SELECT capital FROM latest) AS event_capital,
        (SELECT count(*) FROM public.recovery_incidents WHERE tenant_id = $1 AND released_at IS NULL) AS incidents,
        (SELECT array_agg(reason ORDER BY reason) FROM public.recovery_incidents
            WHERE tenant_id = $1 AND released_at IS NULL) AS incident_reasons,
        (SELECT count(*) FROM public.stream_suspensions WHERE tenant_id = $1) AS suspensions,
        (SELECT count(*) FROM public.order_intents_blocking_rollback) AS boundary_b,
        COALESCE((SELECT epoch_id = $3 FROM epochs WHERE n = 1), FALSE) AS latest_epoch_is_this,
        COALESCE((SELECT clean FROM epochs WHERE n = 2), FALSE) AS predecessor_clean,
        (SELECT count(*) FROM public.funding_sagas WHERE tenant_id = $1 AND state NOT IN ('REFRESHED', 'FAILED')) AS sagas,
        (SELECT count(*) FROM public.event_log e WHERE e.tenant_id = $1 AND e.stream_key LIKE 'position:%'
            AND (e.occurred_at >= m.since AND e.occurred_at < m.until
                 OR e.ingested_at >= m.since AND e.ingested_at < m.until)) AS position_events,
        EXISTS (SELECT 1 FROM parts WHERE lo <= m.since AND hi > now()) AS partitions_attached,
        (SELECT count(*) FROM public.positions_current
            WHERE tenant_id = $1 AND state IN ('armed', 'entering', 'active', 'exiting')) AS open_rows,
        (SELECT count(*) FROM streams t WHERE t.event_type <> ALL ($4::TEXT[]) AND NOT EXISTS (
            SELECT 1 FROM public.positions_current p WHERE 'position:' || p.position_id::TEXT = t.stream_key)) AS open_streams,
        (SELECT count(*) FROM public.positions_current WHERE tenant_id = $1
            AND (entry_filled_at >= m.since AND entry_filled_at < m.until OR closed_at >= m.since AND closed_at < m.until))
            AS month_rows,
        s.monthly_budget_model AS month_model,
        -- An absent accounting row is the one certify creates with net-from-start (§6.2).
        COALESCE((SELECT budget_model FROM public.accounting_state WHERE tenant_id = $1 AND venue_id = $2),
                 'net_from_start_non_expanding_v1') AS accounting_model
    FROM m LEFT JOIN public.monthly_state s
        ON s.year = EXTRACT(YEAR FROM m.month) AND s.month = EXTRACT(MONTH FROM m.month))
SELECT jsonb_build_object(
    'month', f.month,
    'previous', f.capital_base::TEXT,
    'checks', jsonb_build_object(
        'c7', f.incidents = 0 AND f.suspensions = 0 AND f.boundary_b = 0 AND f.latest_epoch_is_this AND f.predecessor_clean,
        'c8', f.sagas = 0,
        'c9a', f.position_events = 0 AND f.partitions_attached,
        'c9b', f.open_rows = 0 AND f.open_streams = 0,
        'c9c', COALESCE(f.reset_landed AND f.month_peak_net = 0 AND f.capital_base = f.event_capital, FALSE),
        'c9d', f.month_rows = 0,
        'sd14', COALESCE(f.month_model = 'net_from_start_non_expanding_v1', FALSE) AND f.accounting_model = f.month_model),
    'values', to_jsonb(f) - 'month' - 'capital_base' - 'incident_reasons'
        || jsonb_build_object('event_capital', f.event_capital::TEXT, 'month_peak_net', f.month_peak_net::TEXT),
    'incident_reasons', f.incident_reasons,
    -- The venue's run to show: the complete one, else the pending one, else the last abandoned one.
    'run', (SELECT jsonb_build_object('cutover_id', cutover_id, 'state', state, 'abandon_reason', abandon_reason)
            FROM public.venue_cutovers WHERE tenant_id = $1 AND venue_id = $2
            ORDER BY state = 'complete' DESC, state = 'pending' DESC, created_at DESC LIMIT 1))
FROM f
