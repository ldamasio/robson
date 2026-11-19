-- One frequency row, its immutable registration and the current recovery root.
-- The old published snapshot is independent of the advancing claim.
WITH advanced AS (
    UPDATE n5_refresh_state AS s
    SET revision = s.revision + 1,
        claim_id = $17, owner_process = $18, content_digest = $19,
        claim_domain = ROW('boottime', $9::uuid, $10::bigint, $11::integer)::budget_clock_domain_v38,
        claim_tick = $20::text::numeric, claim_audit_at = $21,
        eligibility_kind = 'same_domain', handoff_domain = NULL, handoff_tick = NULL,
        result_class = NULL
    WHERE s.venue_id = 'binance:spot' AND s.symbol = $1 AND s.descriptor = 'commission'
      AND s.revision = $5 AND s.revision > 0 AND s.revision < 9223372036854775807
      AND s.claim_id = $6 AND s.owner_process = $7 AND s.content_digest = $8
      AND s.claim_domain IS NOT DISTINCT FROM
          ROW('boottime', $9::uuid, $10::bigint, $11::integer)::budget_clock_domain_v38
      AND s.claim_tick = $12::text::numeric AND s.claim_audit_at = $13 AND s.eligibility_kind = $14
      AND s.handoff_domain IS NOT DISTINCT FROM CASE WHEN $15::text IS NOT NULL
          THEN ROW('boottime', $9::uuid, $10::bigint, $11::integer)::budget_clock_domain_v38 END
      AND s.handoff_tick IS NOT DISTINCT FROM $15::text::numeric
      AND s.result_class IS NOT DISTINCT FROM $16::text
      AND s.claim_id <> $17
      AND budget_integer_fits($20::text::numeric, 0, 9223372036854775807999999999)
      AND $20::text::numeric >= COALESCE(s.handoff_tick, s.claim_tick + 60000000000)
                                + 86400000000000
      AND EXISTS (
          SELECT 1
          FROM n5_refresh_keys AS k
          JOIN n5_registry_root AS r USING (registry_id)
          WHERE (k.venue_id, k.symbol, k.descriptor) = (s.venue_id, s.symbol, s.descriptor)
            AND k.registration_id = $4 AND r.registry_id = 1
            AND r.schema_version = 38 AND r.manifest_version = 1
            AND r.initialized AND r.recovery_state = 'normal'
            AND r.initialization_id = $2 AND r.recovery_generation = $3
            AND k.registered_revision <= r.manifest_revision
      )
    RETURNING revision
)
SELECT (SELECT revision FROM advanced) AS revision
