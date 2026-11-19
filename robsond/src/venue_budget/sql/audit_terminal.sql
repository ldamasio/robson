-- A05: $1 audit, $2 generation, $3 high-water (nullable; the run cursor equals
-- it), $4 exact examined count, $5 initialization UUID returned by B03 under
-- the held venue lock. Three independent ascending PK populations, each capped
-- at 1,000,001 rows before any join or aggregate, read in this statement's
-- fresh snapshot; then at most one guarded terminal UPDATE. No repair.
-- Exactly one row/result.
WITH source AS MATERIALIZED (
    -- Keep one slot for each global source row before applying Spot scope.
    SELECT ARRAY(
        SELECT CASE WHEN a.venue_id='binance:spot' THEN a.attempt_id END
        FROM submit_attempts a WHERE a.attempt_id <= $3::uuid
        ORDER BY a.attempt_id LIMIT 1000001
    ) AS all_keys
), key_sets AS MATERIALIZED (
    SELECT cardinality(s.all_keys)::bigint AS source_count,
           array_remove(s.all_keys, NULL::uuid) AS spot_keys,
           ARRAY(SELECT c.attempt_id FROM spot_intent_clock c
                 ORDER BY c.attempt_id LIMIT 1000001) AS clock_keys,
           ARRAY(SELECT d.attempt_id FROM budget_daily_charges d
                 ORDER BY d.attempt_id LIMIT 1000001) AS charge_keys
    FROM source s
), verdict AS MATERIALIZED (
    -- The same fenced forward lineage already verified the full row for
    -- every Spot key. The whole ordered key sets close both inverse edges.
    SELECT k.source_count,cardinality(k.spot_keys)::bigint AS spot_count,
           cardinality(k.clock_keys)::bigint AS clock_count,
           cardinality(k.charge_keys)::bigint AS charge_count,
           k.spot_keys=k.clock_keys AS clock_match,
           k.spot_keys=k.charge_keys AS charge_match
    FROM key_sets k
), decided AS MATERIALIZED (
    SELECT v.*,
           CASE WHEN GREATEST(v.source_count,v.clock_count,v.charge_count) > 1000000
                  OR v.source_count <> $4::bigint THEN NULL
                WHEN NOT v.clock_match THEN 'failed'
                WHEN NOT v.charge_match THEN 'failed'
                ELSE 'complete' END AS status,
           CASE WHEN GREATEST(v.source_count,v.clock_count,v.charge_count) > 1000000
                  OR v.source_count <> $4::bigint THEN NULL
                WHEN NOT v.clock_match THEN 'terminal_clock_inconsistent'
                WHEN NOT v.charge_match THEN 'terminal_charge_key_mismatch'
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
SELECT x.source_count,x.spot_count,x.clock_count,x.charge_count,x.clock_match,x.charge_match,
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
