-- B04: 8 binds: venue TEXT, tenant UUID, actual audit UUID, quiescence BIGINT,
-- boot UUID, offset seconds BIGINT, offset ns INTEGER, native start-tick TEXT.
-- One row/result, rows_affected=1. Consumer requires state_ok and audit_ok.
-- Each CTE bound precedes aggregation. No custom-domain/NUMERIC result OID.
WITH active AS MATERIALIZED (
    SELECT attempt_id,budget_clock_domain,budget_created_tick
    FROM budget_daily_charges
    WHERE venue_id=$1::text AND daily_units=1 AND expiry_proof_id IS NULL
    ORDER BY attempt_id LIMIT 10001
), e60 AS MATERIALIZED (
    SELECT process_epoch,start_tick
    FROM budget_epochs
    WHERE venue_id=$1::text AND (clock_domain).kind='boottime'
      AND (clock_domain).boot_uuid=$5::uuid
      AND (clock_domain).offset_seconds=$6::bigint
      AND (clock_domain).offset_nanoseconds=$7::integer
      AND start_tick BETWEEN $8::text::numeric-60000000000 AND $8::text::numeric
    ORDER BY start_tick DESC,process_epoch DESC LIMIT 9
), e10 AS MATERIALIZED (
    SELECT process_epoch,start_tick
    FROM budget_epochs
    WHERE venue_id=$1::text AND (clock_domain).kind='boottime'
      AND (clock_domain).boot_uuid=$5::uuid
      AND (clock_domain).offset_seconds=$6::bigint
      AND (clock_domain).offset_nanoseconds=$7::integer
      AND start_tick BETWEEN $8::text::numeric-10000000000 AND $8::text::numeric
    ORDER BY start_tick DESC,process_epoch DESC LIMIT 3
)
SELECT s.venue_id,s.initialization_state,s.initialization_id,s.audit_id,
       s.quiescence_generation,s.schema_version,s.proof_version,
       s.recovery_generation,s.recovery_status,s.cohort_id,s.source_high_water,
       s.epoch_count_60,s.epoch_count_10,s.recorded_at::text AS state_recorded_at,
       (s.initialization_state='complete' AND s.initialization_id IS NOT NULL
        AND s.schema_version=38 AND s.proof_version=1
        AND s.audit_id=$3::uuid AND s.quiescence_generation=$4::bigint) IS TRUE AS state_ok,
       (a.audit_id=s.audit_id AND a.venue_id=s.venue_id
        AND a.quiescence_generation=s.quiescence_generation
        AND a.status='complete' AND a.failure_reason IS NULL
        AND a.completed_at IS NOT NULL AND a.schema_version=38
        AND a.proof_generation=1
        AND a.high_water IS NOT DISTINCT FROM s.source_high_water
        AND a.cursor IS NOT DISTINCT FROM a.high_water) IS TRUE AS audit_ok,
       a.examined_count AS audit_examined_count,
       (SELECT count(*) FROM active) AS active_count,
       ARRAY(SELECT attempt_id FROM active ORDER BY attempt_id) AS active_ids,
       ARRAY(SELECT budget_clock_domain IS DISTINCT FROM NULL::budget_clock_domain_v38
             FROM active ORDER BY attempt_id) AS active_domain_present,
       ARRAY(SELECT (budget_clock_domain).kind FROM active ORDER BY attempt_id) AS active_kind,
       ARRAY(SELECT (budget_clock_domain).boot_uuid FROM active ORDER BY attempt_id) AS active_boot,
       ARRAY(SELECT (budget_clock_domain).offset_seconds FROM active ORDER BY attempt_id) AS active_offset_seconds,
       ARRAY(SELECT (budget_clock_domain).offset_nanoseconds FROM active ORDER BY attempt_id) AS active_offset_ns,
       ARRAY(SELECT budget_created_tick::text FROM active ORDER BY attempt_id) AS active_tick_text,
       ARRAY(SELECT budget_integer_fits(budget_created_tick,0,9223372036854775807999999999)
             FROM active ORDER BY attempt_id) AS active_tick_fits,
       ARRAY(SELECT process_epoch FROM e60 ORDER BY start_tick DESC,process_epoch DESC) AS epoch60_ids,
       ARRAY(SELECT start_tick::text FROM e60 ORDER BY start_tick DESC,process_epoch DESC) AS epoch60_tick_text,
       ARRAY(SELECT budget_integer_fits(start_tick,0,9223372036854775807999999999)
             FROM e60 ORDER BY start_tick DESC,process_epoch DESC) AS epoch60_tick_fits,
       ARRAY(SELECT process_epoch FROM e10 ORDER BY start_tick DESC,process_epoch DESC) AS epoch10_ids,
       ARRAY(SELECT start_tick::text FROM e10 ORDER BY start_tick DESC,process_epoch DESC) AS epoch10_tick_text,
       ARRAY(SELECT budget_integer_fits(start_tick,0,9223372036854775807999999999)
             FROM e10 ORDER BY start_tick DESC,process_epoch DESC) AS epoch10_tick_fits,
       EXISTS(SELECT 1 FROM budget_epochs f
              WHERE f.venue_id=$1::text AND (f.clock_domain).kind='boottime'
                AND (f.clock_domain).boot_uuid=$5::uuid
                AND (f.clock_domain).offset_seconds=$6::bigint
                AND (f.clock_domain).offset_nanoseconds=$7::integer
                AND f.start_tick>$8::text::numeric) AS future_same_domain_epoch,
       p.tenant_id IS NOT NULL AS pause_present,p.status AS pause_status,
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
FROM budget_venue_state s
LEFT JOIN budget_source_audits a ON a.audit_id=s.audit_id
LEFT JOIN venue_pauses p ON p.tenant_id=$2::uuid AND p.venue_id=s.venue_id
WHERE s.venue_id=$1::text;
