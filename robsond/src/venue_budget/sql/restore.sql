-- P01 is a separately labelled one-statement restoration UPSERT, NOT B01-B06.
-- 13 binds: tenant UUID,venue TEXT,observed UUID,new boot UUID,offset BIGINT,
-- offset ns INTEGER,original new-domain anchor TEXT,frozen audit TIMESTAMPTZ,
-- then the restoration read before it (boot UUID,offset BIGINT,offset ns
-- INTEGER,anchor TEXT) and its recorded_at in microseconds BIGINT: a row
-- changed since that read is not updated (0 rows, the start is refused), so
-- a concurrent restoration is never overwritten.
-- Requires existing bounded original duration, actual observation identity.
-- Original fields remain unchanged; no fabricated pause without a head.
-- recorded_at never moves backward (GREATEST): a backward wall step can never
-- make budget_pause_moves_forward refuse the restoration.
INSERT INTO venue_pauses(tenant_id,venue_id,status,code,pause_state,observation_id,
    original_domain,observed_tick,duration_ns,restoration_domain,
    restoration_anchor,deadline_tick,until_at,recorded_at)
SELECT p.tenant_id,p.venue_id,p.status,p.code,'bounded',p.observation_id,
       p.original_domain,p.observed_tick,p.duration_ns,
       ROW('boottime'::text,$4::uuid,$5::bigint,$6::integer)::budget_clock_domain_v38,
       $7::text::numeric,$7::text::numeric+p.duration_ns,p.until_at,$8::timestamptz
FROM venue_pauses p
WHERE p.tenant_id=$1::uuid AND p.venue_id=$2::text
  AND p.observation_id=$3::uuid AND p.pause_state='bounded'
  AND p.original_domain IS DISTINCT FROM NULL::budget_clock_domain_v38
  AND budget_integer_fits(p.duration_ns,0,9223372036854775807999999999) IS TRUE
  AND p.restoration_domain IS DISTINCT FROM
      ROW('boottime'::text,$4::uuid,$5::bigint,$6::integer)::budget_clock_domain_v38
ON CONFLICT(tenant_id,venue_id) DO UPDATE SET
    restoration_domain=EXCLUDED.restoration_domain,
    restoration_anchor=EXCLUDED.restoration_anchor,
    deadline_tick=EXCLUDED.deadline_tick,
    recorded_at=GREATEST(EXCLUDED.recorded_at,venue_pauses.recorded_at)
WHERE venue_pauses.observation_id=EXCLUDED.observation_id
  AND venue_pauses.restoration_domain IS DISTINCT FROM EXCLUDED.restoration_domain
  AND venue_pauses.restoration_domain IS NOT DISTINCT FROM
      ROW('boottime'::text,$9::uuid,$10::bigint,$11::integer)::budget_clock_domain_v38
  AND venue_pauses.restoration_anchor=$12::text::numeric
  AND (extract(epoch FROM venue_pauses.recorded_at)*1000000)::bigint=$13::bigint
  AND (venue_pauses.status,venue_pauses.code,venue_pauses.original_domain,
       venue_pauses.observed_tick,venue_pauses.duration_ns)
      IS NOT DISTINCT FROM
      (EXCLUDED.status,EXCLUDED.code,EXCLUDED.original_domain,
       EXCLUDED.observed_tick,EXCLUDED.duration_ns)
RETURNING tenant_id,venue_id,status,code,pause_state,observation_id,
          (original_domain).kind AS original_kind,
          (original_domain).boot_uuid AS original_boot,
          (original_domain).offset_seconds AS original_offset_seconds,
          (original_domain).offset_nanoseconds AS original_offset_ns,
          observed_tick::text AS observed_tick_text,duration_ns::text AS duration_text,
          (restoration_domain).kind AS restoration_kind,
          (restoration_domain).boot_uuid AS restoration_boot,
          (restoration_domain).offset_seconds AS restoration_offset_seconds,
          (restoration_domain).offset_nanoseconds AS restoration_offset_ns,
          restoration_anchor::text AS restoration_anchor_text,
          deadline_tick::text AS deadline_tick_text,
          until_at::text AS until_audit,
          (extract(epoch FROM recorded_at)*1000000)::bigint AS recorded_at_us;
