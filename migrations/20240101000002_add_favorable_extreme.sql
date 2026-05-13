-- Migration 003: Add favorable_extreme for anchored trailing stop
-- Adds columns needed for robson-engine::trailing_stop integration

BEGIN;

ALTER TABLE positions_current
  ADD COLUMN favorable_extreme DECIMAL(20, 8),
  ADD COLUMN extreme_at TIMESTAMPTZ;

COMMENT ON COLUMN positions_current.favorable_extreme IS 'Peak price (Long) or lowest price (Short) for anchored trailing stop';
COMMENT ON COLUMN positions_current.extreme_at IS 'When the favorable extreme was reached';

COMMIT;
