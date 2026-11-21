WITH supplied AS (
    SELECT $1::text AS symbol, $2::uuid AS init, $3::bigint AS generation,
           $4::uuid AS registration, $5::bigint AS revision, $6::uuid AS claim_id,
           $7::uuid AS owner_process, $8::bytea AS digest,
           ROW('boottime', $9::uuid, $10::bigint, $11::integer)::budget_clock_domain_v38 AS domain,
           $12::text::numeric AS claim_tick, $13::timestamptz AS audit_at,
           $14::text::numeric AS handoff_tick, $15::text AS result,
           $16::uuid AS tenant, $17::text AS scope, $18::text::numeric AS uid,
           $19::text::numeric AS snapshot_tick, $20::timestamptz AS as_of,
           $21::smallint AS version, $22::bytea AS hash, $23::bytea AS payload
), current_claim AS MATERIALIZED (
    SELECT s.*
    FROM supplied AS i
    JOIN n5_refresh_state AS s ON s.venue_id = 'binance:spot'
        AND s.symbol = i.symbol AND s.descriptor = 'commission'
    JOIN n5_refresh_keys AS k
        ON (k.venue_id, k.symbol, k.descriptor) = (s.venue_id, s.symbol, s.descriptor)
    JOIN n5_registry_root AS r ON r.registry_id = k.registry_id
    WHERE r.registry_id = 1 AND r.schema_version = 38 AND r.manifest_version = 1
      AND r.initialized AND r.recovery_state = 'normal'
      AND r.initialization_id = i.init AND r.recovery_generation = i.generation
      AND k.registration_id = i.registration AND k.registered_revision <= r.manifest_revision
      AND s.revision = i.revision AND s.claim_id = i.claim_id
      AND s.owner_process = i.owner_process AND s.content_digest = i.digest
      AND s.claim_domain IS NOT DISTINCT FROM i.domain
      AND s.claim_tick = i.claim_tick AND s.claim_audit_at = i.audit_at
      AND s.eligibility_kind = 'same_domain'
), exact_replay AS (
    SELECT 1
    FROM current_claim AS s CROSS JOIN supplied AS i
    WHERE s.result_class = i.result
      AND s.handoff_tick IS NOT DISTINCT FROM i.handoff_tick
      AND s.handoff_domain IS NOT DISTINCT FROM CASE WHEN i.handoff_tick IS NOT NULL THEN i.domain END
      AND (i.result <> 'success' OR
           (s.snapshot_claim_id, s.snapshot_revision, s.snapshot_owner, s.snapshot_tenant,
            s.snapshot_scope, s.snapshot_uid, s.snapshot_domain, s.snapshot_tick,
            s.snapshot_as_of, s.snapshot_validation_version, s.snapshot_hash, s.snapshot_payload)
           IS NOT DISTINCT FROM
           (i.claim_id, i.revision, i.owner_process, i.tenant, i.scope, i.uid, i.domain,
            i.snapshot_tick, i.as_of, i.version, i.hash, i.payload))
)
