-- A single anchored read of an exact M38 catalog. The extra key is an overflow
-- sentinel. Only configured keys can contribute full claim and snapshot data.
WITH active(symbol) AS (
    SELECT unnest($1::text[])
), bounded_keys AS MATERIALIZED (
    SELECT venue_id, symbol, descriptor, registry_id, registration_id,
           registered_revision, registered_at, provenance
    FROM n5_refresh_keys
    ORDER BY venue_id, symbol, descriptor
    LIMIT 4097
)
SELECT
    r.registry_id AS root_id,
    r.schema_version, r.manifest_version, r.manifest_revision, r.initialized,
    r.registered_count, r.initialization_id, r.recovery_generation,
    CASE WHEN r.recovery_state IN ('normal','quarantined') THEN r.recovery_state END AS recovery_state,
    CASE WHEN octet_length(r.canonical_digest) = 32 THEN r.canonical_digest END AS canonical_digest,
    CASE WHEN octet_length(k.venue_id) BETWEEN 1 AND 64 THEN k.venue_id END AS venue_id,
    CASE WHEN octet_length(k.symbol) BETWEEN 1 AND 64 THEN k.symbol END AS symbol,
    CASE WHEN k.descriptor = 'commission' THEN k.descriptor END AS descriptor,
    k.registry_id AS key_registry, k.registration_id, k.registered_revision, k.registered_at,
    CASE WHEN octet_length(k.provenance) BETWEEN 1 AND 64 THEN k.provenance END AS provenance,
    s.revision AS state_revision,
    a.symbol IS NOT NULL AS active,
    num_nonnulls(s_active.claim_id, s_active.owner_process, s_active.content_digest,
        s_active.claim_domain, s_active.claim_tick, s_active.claim_audit_at, s_active.eligibility_kind) AS claim_fields,
    num_nonnulls(s_active.handoff_domain, s_active.handoff_tick) AS handoff_fields,
    num_nonnulls(s_active.snapshot_claim_id, s_active.snapshot_revision, s_active.snapshot_owner,
        s_active.snapshot_tenant, s_active.snapshot_scope, s_active.snapshot_uid,
        s_active.snapshot_domain, s_active.snapshot_tick, s_active.snapshot_as_of,
        s_active.snapshot_validation_version, s_active.snapshot_hash, s_active.snapshot_payload) AS snapshot_fields,
    s_active.claim_id, s_active.owner_process,
    CASE WHEN octet_length(s_active.content_digest) = 32 THEN s_active.content_digest END AS content_digest,
    CASE WHEN (s_active.claim_domain).kind = 'boottime' THEN (s_active.claim_domain).kind END AS claim_kind,
    (s_active.claim_domain).boot_uuid AS claim_boot,
    (s_active.claim_domain).offset_seconds AS claim_offset_s,
    (s_active.claim_domain).offset_nanoseconds AS claim_offset_ns,
    CASE WHEN budget_integer_fits(s_active.claim_tick, 0, 9223372036854775807999999999)
        THEN s_active.claim_tick::text END AS claim_tick,
    s_active.claim_audit_at,
    CASE WHEN s_active.eligibility_kind IN ('first_use','same_domain','continuous_observation')
        THEN s_active.eligibility_kind END AS eligibility_kind,
    CASE WHEN (s_active.handoff_domain).kind = 'boottime'
        THEN (s_active.handoff_domain).kind END AS handoff_kind,
    (s_active.handoff_domain).boot_uuid AS handoff_boot,
    (s_active.handoff_domain).offset_seconds AS handoff_offset_s,
    (s_active.handoff_domain).offset_nanoseconds AS handoff_offset_ns,
    CASE WHEN budget_integer_fits(s_active.handoff_tick, 0, 9223372036854775807999999999)
        THEN s_active.handoff_tick::text END AS handoff_tick,
    CASE WHEN s_active.result_class IN (
        'success','no_send','transport_failure','venue_rejected','invalid_payload','policy_mismatch','unknown'
    ) THEN s_active.result_class END AS result_class,
    s_active.snapshot_claim_id, s_active.snapshot_revision, s_active.snapshot_owner, s_active.snapshot_tenant,
    CASE WHEN octet_length(s_active.snapshot_scope) BETWEEN 1 AND 128 THEN s_active.snapshot_scope END AS snapshot_scope,
    CASE WHEN budget_integer_fits(s_active.snapshot_uid, 0, 18446744073709551615)
        THEN s_active.snapshot_uid::text END AS snapshot_uid,
    CASE WHEN (s_active.snapshot_domain).kind = 'boottime'
        THEN (s_active.snapshot_domain).kind END AS snapshot_kind,
    (s_active.snapshot_domain).boot_uuid AS snapshot_boot,
    (s_active.snapshot_domain).offset_seconds AS snapshot_offset_s,
    (s_active.snapshot_domain).offset_nanoseconds AS snapshot_offset_ns,
    CASE WHEN budget_integer_fits(s_active.snapshot_tick, 0, 9223372036854775807999999999)
        THEN s_active.snapshot_tick::text END AS snapshot_tick,
    s_active.snapshot_as_of, s_active.snapshot_validation_version,
    CASE WHEN octet_length(s_active.snapshot_hash) = 32 THEN s_active.snapshot_hash END AS snapshot_hash,
    CASE WHEN octet_length(s_active.snapshot_payload) BETWEEN 1 AND 16384
        THEN s_active.snapshot_payload END AS snapshot_payload,
    COALESCE(
        (s_active.content_digest IS NOT NULL AND octet_length(s_active.content_digest) <> 32)
        OR (s_active.claim_domain IS DISTINCT FROM NULL AND (s_active.claim_domain).kind IS DISTINCT FROM 'boottime')
        OR (s_active.claim_tick IS NOT NULL AND NOT budget_integer_fits(s_active.claim_tick, 0, 9223372036854775807999999999))
        OR (s_active.eligibility_kind IS NOT NULL AND s_active.eligibility_kind NOT IN ('first_use','same_domain','continuous_observation'))
        OR (s_active.handoff_domain IS DISTINCT FROM NULL AND (s_active.handoff_domain).kind IS DISTINCT FROM 'boottime')
        OR (s_active.handoff_tick IS NOT NULL AND NOT budget_integer_fits(s_active.handoff_tick, 0, 9223372036854775807999999999))
        OR (s_active.result_class IS NOT NULL AND s_active.result_class NOT IN (
            'success','no_send','transport_failure','venue_rejected','invalid_payload','policy_mismatch','unknown'
        ))
        OR (s_active.snapshot_scope IS NOT NULL AND octet_length(s_active.snapshot_scope) NOT BETWEEN 1 AND 128)
        OR (s_active.snapshot_uid IS NOT NULL AND NOT budget_integer_fits(s_active.snapshot_uid, 0, 18446744073709551615))
        OR (s_active.snapshot_domain IS DISTINCT FROM NULL AND (s_active.snapshot_domain).kind IS DISTINCT FROM 'boottime')
        OR (s_active.snapshot_tick IS NOT NULL AND NOT budget_integer_fits(s_active.snapshot_tick, 0, 9223372036854775807999999999))
        OR (s_active.snapshot_hash IS NOT NULL AND octet_length(s_active.snapshot_hash) <> 32)
        OR (s_active.snapshot_payload IS NOT NULL AND octet_length(s_active.snapshot_payload) NOT BETWEEN 1 AND 16384),
        false
    ) AS invalid_active_projection
FROM (SELECT 1) AS anchor
LEFT JOIN n5_registry_root r ON r.registry_id = 1
LEFT JOIN bounded_keys k ON true
-- The parameterized lookup keeps the state plan under the limited key scan.
LEFT JOIN LATERAL (
    SELECT st.* FROM n5_refresh_state st
    WHERE (st.venue_id, st.symbol, st.descriptor) = (k.venue_id, k.symbol, k.descriptor)
    OFFSET 0
) AS s ON true
LEFT JOIN active a ON k.venue_id = 'binance:spot' AND k.descriptor = 'commission' AND a.symbol = k.symbol
-- Preserve the revision for every key but gate the wide row without a second lookup.
LEFT JOIN LATERAL (SELECT s.* WHERE a.symbol IS NOT NULL) AS s_active ON true
ORDER BY k.venue_id, k.symbol, k.descriptor
