-- Migration 015: Add binance_position_id to detected_positions
--
-- Fixes schema drift: DetectedPositionDto (robson-store) maps
-- binance_position_id, but the detected_positions table created in migration
-- 003 never carried this column. Migration 005 added binance_position_id only
-- to the `positions` table. As a result, the position monitor failed at boot
-- while loading persisted detected_positions:
--
--     no column found for name: binance_position_id
--
-- The column is added NULLABLE on purpose. Legacy rows created before this
-- migration have no known Binance-assigned id, and we refuse to fabricate one
-- (an empty string would be a false external identity in a safety-net table).
-- Such rows stay NULL until the next detection/save cycle, when save() writes
-- the real id. The store DTO models this as Option<String> and the domain
-- conversion falls back to the composite position_id for legacy rows.

ALTER TABLE detected_positions
    ADD COLUMN binance_position_id VARCHAR(255);

CREATE INDEX idx_detected_positions_binance_id
    ON detected_positions(binance_position_id)
    WHERE binance_position_id IS NOT NULL;

COMMENT ON COLUMN detected_positions.binance_position_id IS
'Binance USD-M internal position id for a detected rogue position. Nullable: legacy rows (pre-migration 015) carry NULL until the next detection/save cycle repopulates it.';
