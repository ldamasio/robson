-- Persist the entry-time invalidation guard level (ADR-0042).
--
-- The guard is a clamp layer applied after technical-stop analysis and before
-- the ADR-0041 executable buffer. It is stored while active so restarts replay
-- the same effective stop, and is cleared on the first trailing-stop advance.

ALTER TABLE positions_current
    ADD COLUMN IF NOT EXISTS invalidation_guard_level DECIMAL(20, 8);

COMMENT ON COLUMN positions_current.invalidation_guard_level IS
    'Entry-time invalidation guard level (recent adverse extreme) when active (ADR-0042); NULL after first trailing-stop advance or when disabled';
