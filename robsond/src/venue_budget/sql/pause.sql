-- N03 (non-trading bootstrap): 2 binds: tenant UUID, venue TEXT, fixed by the
-- private caller to binance:spot. Exactly one row/result, rows_affected=1,
-- whether a pause is stored or not: snapshot.sql's pause_* columns (B04),
-- decoded by the same PauseValue. Nothing of C2/2b's state is read.
SELECT p.tenant_id IS NOT NULL AS pause_present,p.status AS pause_status,
       p.code AS pause_code,p.pause_state,p.observation_id AS pause_observation_id,
       p.original_domain IS DISTINCT FROM NULL::budget_clock_domain_v38 AS pause_original_present,
       (p.original_domain).kind AS pause_original_kind,
       (p.original_domain).boot_uuid AS pause_original_boot,
       (p.original_domain).offset_seconds AS pause_original_offset_seconds,
       (p.original_domain).offset_nanoseconds AS pause_original_offset_ns,
       p.observed_tick::text AS pause_observed_tick_text,
       budget_integer_fits(p.observed_tick,0,9223372036854775807999999999) AS pause_observed_tick_fits,
       p.duration_ns::text AS pause_duration_text,
       budget_integer_fits(p.duration_ns,0,9223372036854775807999999999) AS pause_duration_fits,
       p.restoration_domain IS DISTINCT FROM NULL::budget_clock_domain_v38 AS pause_restoration_present,
       (p.restoration_domain).kind AS pause_restoration_kind,
       (p.restoration_domain).boot_uuid AS pause_restoration_boot,
       (p.restoration_domain).offset_seconds AS pause_restoration_offset_seconds,
       (p.restoration_domain).offset_nanoseconds AS pause_restoration_offset_ns,
       p.restoration_anchor::text AS pause_anchor_text,
       budget_integer_fits(p.restoration_anchor,0,9223372036854775807999999999) AS pause_anchor_fits,
       p.deadline_tick::text AS pause_deadline_text,
       budget_integer_fits(p.deadline_tick,0,9223372036854775807999999999) AS pause_deadline_fits,
       p.until_at::text AS pause_until_audit,p.recorded_at::text AS pause_recorded_audit,
       (extract(epoch FROM p.recorded_at)*1000000)::bigint AS pause_recorded_at_us
FROM (VALUES (1)) AS one(n)
LEFT JOIN venue_pauses p ON p.tenant_id=$1::uuid AND p.venue_id=$2::text;
