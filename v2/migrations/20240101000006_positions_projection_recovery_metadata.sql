-- Migration: Add projection metadata required for crash recovery of open positions
-- Created: 2026-04-04
-- Purpose: Preserve enough lifecycle data in positions_current to reconstruct
--          Entering and Exiting states during PostgreSQL projection recovery.

ALTER TABLE positions_current
ADD COLUMN entry_signal_id UUID,
ADD COLUMN exit_reason TEXT;

COMMENT ON COLUMN positions_current.entry_signal_id IS
'Signal ID associated with the entry order. Used to faithfully reconstruct Entering state on crash recovery.';

COMMENT ON COLUMN positions_current.exit_reason IS
'Reason recorded when a position transitions to exiting. Used to reconstruct Exiting state on crash recovery.';
