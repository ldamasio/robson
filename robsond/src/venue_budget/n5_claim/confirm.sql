SELECT EXISTS (
    SELECT 1
    FROM n5_refresh_state AS s
    JOIN n5_refresh_keys AS k USING (venue_id, symbol, descriptor)
    JOIN n5_registry_root AS r USING (registry_id)
    WHERE s.venue_id = 'binance:spot' AND s.symbol = $1 AND s.descriptor = 'commission'
      AND r.registry_id = 1 AND r.schema_version = 38 AND r.manifest_version = 1
      AND r.initialized AND r.recovery_state = 'normal'
      AND r.initialization_id = $2 AND r.recovery_generation = $3
      AND k.registration_id = $4 AND k.registered_revision <= r.manifest_revision
      AND s.revision = $5 AND s.claim_id = $6 AND s.owner_process = $7
      AND s.content_digest = $8
      AND s.claim_domain IS NOT DISTINCT FROM
          ROW('boottime', $9::uuid, $10::bigint, $11::integer)::budget_clock_domain_v38
      AND s.claim_tick = $12::text::numeric AND s.claim_audit_at = $13
      AND s.eligibility_kind = 'same_domain'
      AND s.handoff_domain IS NULL AND s.handoff_tick IS NULL AND s.result_class IS NULL
) AS exact_claim
