-- Stop-policy versioning (issue #154, ADR-0050 §3/§4 slice 5).
--
-- A position's stop derivation is pinned at arm time so a deploy can never
-- retroact on live positions. Every existing row is legacy by construction:
-- NOT NULL DEFAULT 'legacy_uncapped' backfills the whole table, and only new
-- arms may write 'span_capped_v1'.
--
-- stop_buffer_bps_at_arm snapshots the ADR-0041 buffer at arm time: pinning
-- only the algorithm version would freeze the formula but not the price (a
-- ROBSON_STOP_BUFFER_BPS change across a restart would still move live
-- stops). NULL on pre-versioning rows: those follow the live config, the
-- historical behavior.

ALTER TABLE positions_current
    ADD COLUMN IF NOT EXISTS stop_policy TEXT NOT NULL DEFAULT 'legacy_uncapped',
    ADD COLUMN IF NOT EXISTS stop_buffer_bps_at_arm DECIMAL(20, 8);

ALTER TABLE positions_current
    ADD CONSTRAINT chk_positions_stop_policy
    CHECK (stop_policy IN ('legacy_uncapped', 'span_capped_v1'));

COMMENT ON COLUMN positions_current.stop_policy IS
    'Stop-policy version pinned at arm (issue #154): legacy_uncapped (historical derivation) or span_capped_v1 (ADR-0050 §3/§4). Never changes after arm.';
COMMENT ON COLUMN positions_current.stop_buffer_bps_at_arm IS
    'ADR-0041 executable-stop buffer (bps) snapshotted at arm; NULL on positions armed before stop-policy versioning (they follow the live config)';
