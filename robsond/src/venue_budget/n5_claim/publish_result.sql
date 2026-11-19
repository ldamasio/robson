, published AS (
    UPDATE n5_refresh_state AS s
    SET handoff_domain = CASE WHEN i.handoff_tick IS NOT NULL THEN i.domain END,
        handoff_tick = i.handoff_tick,
        result_class = i.result,
        snapshot_claim_id = CASE WHEN i.result = 'success' THEN i.claim_id ELSE s.snapshot_claim_id END,
        snapshot_revision = CASE WHEN i.result = 'success' THEN i.revision ELSE s.snapshot_revision END,
        snapshot_owner = CASE WHEN i.result = 'success' THEN i.owner_process ELSE s.snapshot_owner END,
        snapshot_tenant = CASE WHEN i.result = 'success' THEN i.tenant ELSE s.snapshot_tenant END,
        snapshot_scope = CASE WHEN i.result = 'success' THEN i.scope ELSE s.snapshot_scope END,
        snapshot_uid = CASE WHEN i.result = 'success' THEN i.uid ELSE s.snapshot_uid END,
        snapshot_domain = CASE WHEN i.result = 'success' THEN i.domain ELSE s.snapshot_domain END,
        snapshot_tick = CASE WHEN i.result = 'success' THEN i.snapshot_tick ELSE s.snapshot_tick END,
        snapshot_as_of = CASE WHEN i.result = 'success' THEN i.as_of ELSE s.snapshot_as_of END,
        snapshot_validation_version = CASE WHEN i.result = 'success' THEN i.version
                                           ELSE s.snapshot_validation_version END,
        snapshot_hash = CASE WHEN i.result = 'success' THEN i.hash ELSE s.snapshot_hash END,
        snapshot_payload = CASE WHEN i.result = 'success' THEN i.payload ELSE s.snapshot_payload END
    FROM current_claim AS prior CROSS JOIN supplied AS i
    WHERE (s.venue_id, s.symbol, s.descriptor) = (prior.venue_id, prior.symbol, prior.descriptor)
      AND prior.result_class IS NULL AND s.result_class IS NULL
      -- Recheck the live row after a lock wait. M38 prevents identity changes
      -- within a revision and permits a new handoff or a revision advance.
      AND s.revision = i.revision AND s.claim_id = i.claim_id
      AND (s.handoff_tick IS NULL OR
           (s.handoff_tick IS NOT DISTINCT FROM i.handoff_tick
            AND s.handoff_domain IS NOT DISTINCT FROM i.domain))
      AND (i.handoff_tick IS NULL OR i.handoff_tick BETWEEN i.claim_tick AND i.claim_tick + 60000000000)
      AND (i.result <> 'no_send' OR i.handoff_tick IS NULL)
      AND (i.result <> 'success' OR
           (i.handoff_tick IS NOT NULL AND i.snapshot_tick >= i.handoff_tick
            AND i.version = 1 AND octet_length(i.hash) = 32
            AND octet_length(i.payload) BETWEEN 1 AND 16384))
    RETURNING 1
)
SELECT CASE WHEN EXISTS (SELECT 1 FROM published) THEN 'published'
            WHEN EXISTS (SELECT 1 FROM exact_replay) THEN 'replayed' END AS disposition
