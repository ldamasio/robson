-- Add entry_mode and approval_mode to positions_current so that the daemon
-- can recover entry policies for Armed positions after a restart.
--
-- Before this migration, the entry_policy_resolved event was treated as
-- audit-only and these values were only held in an in-memory HashMap inside
-- PositionManager. A daemon restart would lose them, leaving recovered Armed
-- positions with no detector task and no entry policy.

ALTER TABLE positions_current
    ADD COLUMN IF NOT EXISTS entry_mode    TEXT,
    ADD COLUMN IF NOT EXISTS approval_mode TEXT;
