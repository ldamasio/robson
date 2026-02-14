-- Migration: Add binance_position_id for Core <-> Binance reconciliation
-- Created: 2026-02-14
-- Purpose: Link Core Trading positions to Binance exchange positions for Safety Net coordination

-- Add binance_position_id column to positions table
ALTER TABLE positions 
ADD COLUMN binance_position_id VARCHAR(255);

-- Index for fast lookup by Binance ID
CREATE INDEX idx_positions_binance_id 
ON positions(binance_position_id) 
WHERE binance_position_id IS NOT NULL;

-- Comments for documentation
COMMENT ON COLUMN positions.binance_position_id IS 
'Binance internal position ID for linking Core positions to exchange positions. Used by Safety Net to exclude Core-managed positions and prevent double execution.';

-- Verification query (run after migration)
-- SELECT COUNT(*) as total_positions, 
--        COUNT(binance_position_id) as positions_with_binance_id
-- FROM positions;
