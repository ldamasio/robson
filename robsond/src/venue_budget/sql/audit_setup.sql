-- A01: $1 venue TEXT, $2 fresh audit UUID, $3 generation and $4 audit UUID
-- (nullable) exactly as returned by B03 under its held venue row lock.
-- Exactly one row/result; zero rows is a refused binding or overflow. The
-- global PK high-water is read in this statement's fresh post-lock snapshot.
WITH high AS MATERIALIZED (
    SELECT (SELECT a.attempt_id FROM submit_attempts a ORDER BY a.attempt_id DESC LIMIT 1) AS high_water
), venue AS (
    UPDATE budget_venue_state v
    SET quiescence_generation=v.quiescence_generation+1,audit_id=$2::uuid,source_high_water=h.high_water
    FROM high h
    WHERE v.venue_id=$1::text AND v.quiescence_generation=$3::bigint
      AND v.audit_id IS NOT DISTINCT FROM $4::uuid
      AND v.quiescence_generation<9223372036854775807
      AND NOT EXISTS(SELECT 1 FROM budget_source_audits s WHERE s.audit_id=$2::uuid)
    RETURNING v.venue_id,v.quiescence_generation,v.audit_id,v.source_high_water,v.proof_version
)
INSERT INTO budget_source_audits(audit_id,venue_id,quiescence_generation,high_water,cursor,
    examined_count,status,failure_reason,started_at,completed_at,schema_version,proof_generation)
SELECT audit_id,venue_id,quiescence_generation,source_high_water,NULL,0,'running',NULL,
       CURRENT_TIMESTAMP,NULL,38,proof_version
FROM venue
RETURNING audit_id,venue_id,quiescence_generation,high_water,cursor,examined_count,status;
