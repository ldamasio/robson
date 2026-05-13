-- Add 'cancelled' to positions_current state CHECK constraint.
--
-- Positions disarmed before any entry order was placed now transition to
-- 'cancelled' instead of 'closed'. This preserves the semantic distinction
-- between a position that traded (closed) and one that never entered the
-- market (cancelled).
--
-- The partial index for active positions is not changed: 'cancelled' is
-- terminal and is never included in active-position queries.

ALTER TABLE positions_current
    DROP CONSTRAINT IF EXISTS positions_current_state_check;

ALTER TABLE positions_current
    ADD CONSTRAINT positions_current_state_check
    CHECK (state IN ('armed', 'entering', 'active', 'exiting', 'closed', 'error', 'cancelled'));
