WITH active(venue_id, symbol, descriptor) AS (
    SELECT * FROM unnest($1::text[], $2::text[], $3::text[])
), bounded_keys AS MATERIALIZED (
    SELECT venue_id, symbol, descriptor, registry_id, registration_id, registered_revision
    FROM n5_refresh_keys
    ORDER BY venue_id, symbol, descriptor
    LIMIT 4097
)
SELECT
    r.registry_id AS root_id,
    r.schema_version, r.manifest_version, r.manifest_revision, r.initialized,
    r.registered_count, r.canonical_digest, r.initialization_id,
    r.recovery_generation, r.recovery_state,
    k.venue_id, k.symbol, k.descriptor, k.registry_id AS key_registry,
    k.registration_id, k.registered_revision,
    s.revision AS state_revision,
    a.venue_id IS NOT NULL AS active,
    CASE WHEN a.venue_id IS NOT NULL THEN s.claim_id END AS claim_id,
    CASE WHEN a.venue_id IS NOT NULL THEN s.owner_process END AS owner_process,
    CASE WHEN a.venue_id IS NOT NULL THEN s.result_class END AS result_class,
    CASE WHEN a.venue_id IS NOT NULL THEN s.snapshot_claim_id END AS snapshot_claim_id,
    CASE WHEN a.venue_id IS NOT NULL THEN s.snapshot_revision END AS snapshot_revision,
    CASE WHEN a.venue_id IS NOT NULL THEN s.snapshot_owner END AS snapshot_owner,
    CASE WHEN a.venue_id IS NOT NULL THEN s.snapshot_tenant END AS snapshot_tenant,
    CASE WHEN a.venue_id IS NOT NULL THEN s.snapshot_scope END AS snapshot_scope,
    CASE WHEN a.venue_id IS NOT NULL THEN trunc(s.snapshot_uid)::text END AS snapshot_uid,
    CASE WHEN a.venue_id IS NOT NULL THEN (s.snapshot_domain).kind END AS clock_kind,
    CASE WHEN a.venue_id IS NOT NULL THEN (s.snapshot_domain).boot_uuid END AS boot_uuid,
    CASE WHEN a.venue_id IS NOT NULL THEN (s.snapshot_domain).offset_seconds END AS offset_seconds,
    CASE WHEN a.venue_id IS NOT NULL THEN (s.snapshot_domain).offset_nanoseconds END AS offset_ns,
    CASE WHEN a.venue_id IS NOT NULL THEN trunc(s.snapshot_tick)::text END AS snapshot_tick,
    CASE WHEN a.venue_id IS NOT NULL THEN s.snapshot_as_of END AS snapshot_as_of,
    CASE WHEN a.venue_id IS NOT NULL THEN s.snapshot_validation_version END AS validation_version,
    CASE WHEN a.venue_id IS NOT NULL THEN s.snapshot_hash END AS snapshot_hash,
    CASE WHEN a.venue_id IS NOT NULL THEN s.snapshot_payload END AS snapshot_payload,
    COALESCE(a.venue_id IS NOT NULL AND s.handoff_domain IS NOT NULL
        AND s.snapshot_domain IS NOT DISTINCT FROM s.claim_domain
        AND s.snapshot_domain IS NOT DISTINCT FROM s.handoff_domain
        AND s.handoff_tick <= s.snapshot_tick, false) AS same_receipt_domain
FROM (SELECT 1) AS anchor
LEFT JOIN n5_registry_root r ON r.registry_id = 1
LEFT JOIN bounded_keys k ON true
LEFT JOIN n5_refresh_state s USING (venue_id, symbol, descriptor)
LEFT JOIN active a ON (a.venue_id, a.symbol, a.descriptor) = (k.venue_id, k.symbol, k.descriptor)
ORDER BY k.venue_id, k.symbol, k.descriptor
