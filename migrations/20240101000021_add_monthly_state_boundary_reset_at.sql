ALTER TABLE monthly_state
  ADD COLUMN boundary_reset_at TIMESTAMPTZ;

-- Mark historical months as already reset so a mid-month deployment does not
-- retroactively execute MonthBoundaryReset for the current or prior months.
UPDATE monthly_state
SET boundary_reset_at = created_at
WHERE boundary_reset_at IS NULL;
