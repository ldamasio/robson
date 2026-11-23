-- K01 (pause keeper, PS-2b): 11 binds: tenant UUID,venue TEXT,status SMALLINT,
-- observation UUID,boot UUID,offset BIGINT,offset ns INTEGER,observed tick
-- TEXT,duration ns TEXT,until TIMESTAMPTZ,recorded_at TIMESTAMPTZ. One row:
-- applied (0/1) and whether the applied row is exactly what was written.
-- The DO UPDATE ... WHERE is budget_pause_moves_forward's acceptance (migration
-- 38), condition by condition, so the trigger never fires here: (a) the key is
-- the conflict target; (b) recorded_at = GREATEST; (c) the same observation
-- only with equal identity fields; (d) the same restoration domain only with
-- a deadline no earlier; (e) another domain only with a remaining duration no
-- shorter than the stored duration. A skipped update returns applied = 0.
WITH w AS (
INSERT INTO venue_pauses(tenant_id,venue_id,status,code,pause_state,observation_id,
    original_domain,observed_tick,duration_ns,restoration_domain,
    restoration_anchor,deadline_tick,until_at,recorded_at)
SELECT $1::uuid,$2::text,$3::smallint,NULL,'bounded',$4::uuid,d,$8::text::numeric,
       $9::text::numeric,d,$8::text::numeric,$8::text::numeric+$9::text::numeric,
       $10::timestamptz,$11::timestamptz
FROM (SELECT ROW('boottime'::text,$5::uuid,$6::bigint,$7::integer)::budget_clock_domain_v38 AS d) v
ON CONFLICT(tenant_id,venue_id) DO UPDATE SET
    status=EXCLUDED.status,code=EXCLUDED.code,pause_state=EXCLUDED.pause_state,
    observation_id=EXCLUDED.observation_id,original_domain=EXCLUDED.original_domain,
    observed_tick=EXCLUDED.observed_tick,duration_ns=EXCLUDED.duration_ns,
    restoration_domain=EXCLUDED.restoration_domain,
    restoration_anchor=EXCLUDED.restoration_anchor,deadline_tick=EXCLUDED.deadline_tick,
    until_at=EXCLUDED.until_at,
    recorded_at=GREATEST(EXCLUDED.recorded_at,venue_pauses.recorded_at)
WHERE (venue_pauses.observation_id<>EXCLUDED.observation_id
       OR (venue_pauses.status,venue_pauses.code,venue_pauses.original_domain,
           venue_pauses.observed_tick,venue_pauses.duration_ns) IS NOT DISTINCT FROM
          (EXCLUDED.status,EXCLUDED.code,EXCLUDED.original_domain,
           EXCLUDED.observed_tick,EXCLUDED.duration_ns))
  AND (venue_pauses.restoration_domain IS DISTINCT FROM EXCLUDED.restoration_domain
       OR EXCLUDED.deadline_tick>=venue_pauses.deadline_tick)
  AND (venue_pauses.restoration_domain IS NOT DISTINCT FROM EXCLUDED.restoration_domain
       OR EXCLUDED.deadline_tick-EXCLUDED.restoration_anchor>=venue_pauses.duration_ns)
RETURNING (observation_id,status,code,pause_state,original_domain,observed_tick,
           duration_ns,restoration_domain,restoration_anchor,deadline_tick,until_at)
          IS NOT DISTINCT FROM
          ($4::uuid,$3::smallint,NULL::integer,'bounded'::text,
           ROW('boottime'::text,$5::uuid,$6::bigint,$7::integer)::budget_clock_domain_v38,
           $8::text::numeric,$9::text::numeric,
           ROW('boottime'::text,$5::uuid,$6::bigint,$7::integer)::budget_clock_domain_v38,
           $8::text::numeric,$8::text::numeric+$9::text::numeric,$10::timestamptz)
          AND recorded_at>=$11::timestamptz AS exact)
SELECT count(*) AS applied,bool_and(exact) AS exact FROM w;
