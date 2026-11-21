-- A03: $1 audit, $2 generation, $3 high-water (nullable), $4 expected cursor
-- (nullable), $5 expected count, $6 new cursor, $7 new count. The active venue
-- binding is compared under a short FOR SHARE lock before the audit mutation;
-- a concurrent fresh setup or same-cursor writer leaves moved=0.
-- Exactly one row/result.
WITH venue AS MATERIALIZED (
    SELECT v.venue_id FROM budget_venue_state v
    WHERE v.venue_id='binance:spot' AND v.audit_id=$1::uuid
      AND v.quiescence_generation=$2::bigint AND v.source_high_water IS NOT DISTINCT FROM $3::uuid
    FOR SHARE
), moved AS (
    UPDATE budget_source_audits s SET cursor=$6::uuid,examined_count=$7::bigint
    FROM venue
    WHERE s.audit_id=$1::uuid AND s.venue_id=venue.venue_id AND s.quiescence_generation=$2::bigint
      AND s.high_water IS NOT DISTINCT FROM $3::uuid AND s.cursor IS NOT DISTINCT FROM $4::uuid
      AND s.examined_count=$5::bigint AND s.status='running'
    RETURNING s.cursor,s.examined_count,s.status
)
SELECT (SELECT count(*) FROM moved) AS moved,
       (SELECT cursor FROM moved) AS cursor,
       (SELECT examined_count FROM moved) AS examined_count,
       (SELECT status FROM moved) AS status;
