-- A05: $1 audit, $2 generation, $3 high-water (nullable; the run cursor equals
-- it), $4 exact examined count, $5 initialization UUID returned by B03 under
-- the held venue lock. Three independent ascending PK populations, each capped
-- at 1,000,001 rows before any join or aggregate, read in this statement's
-- fresh snapshot; then at most one guarded terminal UPDATE. No repair.
-- Exactly one row/result.
WITH source AS MATERIALIZED (
    SELECT a.attempt_id FROM submit_attempts a
    WHERE a.attempt_id <= $3::uuid
    ORDER BY a.attempt_id LIMIT 1000001
), clock AS MATERIALIZED (
    SELECT c.attempt_id,c.intent_id,c.tenant_id,c.venue_id,c.attempt_created_at,
           c.budget_clock_domain,c.budget_created_tick,c.budget_recorded_at
    FROM spot_intent_clock c
    ORDER BY c.attempt_id LIMIT 1000001
), charge AS MATERIALIZED (
    -- Whole population: cancels and retired tombstones included, no filter.
    SELECT d.attempt_id FROM budget_daily_charges d
    ORDER BY d.attempt_id LIMIT 1000001
), counts AS MATERIALIZED (
    SELECT (SELECT count(*) FROM source) AS source_count,
           (SELECT count(*) FROM clock) AS clock_count,
           (SELECT count(*) FROM charge) AS charge_count
), checked AS MATERIALIZED (
    -- At most three keyed counterpart probes per clock; only when not overflowed.
    SELECT p.attempt_id IS NOT NULL AND (p.attempt_id <= $3::uuid) IS NOT TRUE AS above,
           (p.attempt_id IS NULL OR i.intent_id IS NULL OR d.attempt_id IS NULL
            OR c.venue_id<>'binance:spot' OR p.venue_id<>'binance:spot'
            OR ROW(i.venue_id,i.account_scope,i.client_order_id)
                 IS DISTINCT FROM ROW(p.venue_id,p.account_scope,p.client_order_id)
            OR ROW(c.tenant_id,c.venue_id,c.attempt_created_at)
                 IS DISTINCT FROM ROW(i.tenant_id,p.venue_id,p.created_at)
            OR ROW(d.venue_id,d.tenant_id,d.intent_id,d.account_scope,d.client_order_id,d.kind,
                   d.source_created_at,d.budget_clock_domain,d.budget_created_tick,d.budget_recorded_at,d.daily_units)
                 IS DISTINCT FROM ROW(p.venue_id,i.tenant_id,i.intent_id,p.account_scope,p.client_order_id,i.kind,
                   p.created_at,c.budget_clock_domain,c.budget_created_tick,c.budget_recorded_at,
                   CASE WHEN i.kind='cancel' THEN 0::smallint ELSE 1::smallint END)) IS TRUE AS bad
    FROM clock c
    LEFT JOIN LATERAL (
        SELECT a.attempt_id,a.venue_id,a.account_scope,a.client_order_id,a.created_at
        FROM submit_attempts a WHERE a.attempt_id=c.attempt_id LIMIT 1
    ) p ON true
    LEFT JOIN LATERAL (
        SELECT o.intent_id,o.tenant_id,o.venue_id,o.account_scope,o.client_order_id,o.kind
        FROM order_intents o WHERE o.intent_id=c.intent_id LIMIT 1
    ) i ON true
    LEFT JOIN LATERAL (
        SELECT q.attempt_id,q.venue_id,q.tenant_id,q.intent_id,q.account_scope,q.client_order_id,
               q.kind,q.source_created_at,q.budget_clock_domain,q.budget_created_tick,
               q.budget_recorded_at,q.daily_units
        FROM budget_daily_charges q WHERE q.attempt_id=c.attempt_id LIMIT 1
    ) d ON true
    WHERE (SELECT GREATEST(source_count,clock_count,charge_count) FROM counts) <= 1000000
), verdict AS MATERIALIZED (
    SELECT n.source_count,n.clock_count,n.charge_count,
           (SELECT count(*) FILTER (WHERE above) FROM checked) AS above_clocks,
           (SELECT count(*) FILTER (WHERE bad AND NOT above) FROM checked) AS bad_clocks
    FROM counts n
), decided AS MATERIALIZED (
    SELECT v.*,
           CASE WHEN GREATEST(v.source_count,v.clock_count,v.charge_count) > 1000000
                  OR v.source_count <> $4::bigint OR v.above_clocks > 0 THEN NULL
                WHEN v.bad_clocks > 0 THEN 'failed'
                WHEN v.charge_count <> v.clock_count THEN 'failed'
                ELSE 'complete' END AS status,
           CASE WHEN GREATEST(v.source_count,v.clock_count,v.charge_count) > 1000000
                  OR v.source_count <> $4::bigint OR v.above_clocks > 0 THEN NULL
                WHEN v.bad_clocks > 0 THEN 'terminal_clock_inconsistent'
                WHEN v.charge_count <> v.clock_count THEN 'terminal_charge_count_mismatch'
                ELSE NULL END AS reason
    FROM verdict v
), venue AS MATERIALIZED (
    SELECT v.venue_id FROM budget_venue_state v
    WHERE v.venue_id='binance:spot' AND v.audit_id=$1::uuid
      AND v.quiescence_generation=$2::bigint AND v.source_high_water IS NOT DISTINCT FROM $3::uuid
      AND v.initialization_state='complete' AND v.initialization_id=$5::uuid
    FOR SHARE
), moved AS (
    UPDATE budget_source_audits s
    SET status=x.status,failure_reason=x.reason,completed_at=CURRENT_TIMESTAMP
    FROM venue,decided x
    WHERE x.status IS NOT NULL
      AND s.audit_id=$1::uuid AND s.venue_id=venue.venue_id AND s.quiescence_generation=$2::bigint
      AND s.high_water IS NOT DISTINCT FROM $3::uuid AND s.cursor IS NOT DISTINCT FROM $3::uuid
      AND s.examined_count=$4::bigint AND s.status='running'
    RETURNING s.audit_id,s.venue_id,s.quiescence_generation,s.high_water,s.cursor,s.examined_count,
              s.status,s.failure_reason,s.completed_at,s.completed_at=CURRENT_TIMESTAMP AS server_completed
)
SELECT x.source_count,x.clock_count,x.charge_count,x.above_clocks,x.bad_clocks,
       x.status AS verdict,x.reason AS verdict_reason,
       (SELECT count(*) FROM venue) AS bindings,
       (SELECT count(*) FROM moved) AS moved,
       (SELECT audit_id FROM moved) AS audit_id,
       (SELECT venue_id FROM moved) AS venue_id,
       (SELECT quiescence_generation FROM moved) AS quiescence_generation,
       (SELECT high_water FROM moved) AS high_water,
       (SELECT cursor FROM moved) AS cursor,
       (SELECT examined_count FROM moved) AS examined_count,
       (SELECT status FROM moved) AS status,
       (SELECT failure_reason FROM moved) AS failure_reason,
       (SELECT completed_at FROM moved) AS completed_at,
       (SELECT server_completed FROM moved) AS server_completed
FROM decided x;
