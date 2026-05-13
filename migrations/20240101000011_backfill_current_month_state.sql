-- MIG-v3#12: Backfill current month's monthly_state from closed positions.
--
-- Only considers state = 'closed' positions. Explicitly excludes:
-- - 'active', 'open' (in-flight)
-- - 'entering', 'armed' (pending entries)
-- - 'exiting' (in-flight exits)
--
-- ADR-0024: realized_loss is the sum of absolute net losses (wins do NOT offset).
-- Idempotent: ON CONFLICT DO NOTHING (safe to re-run).
--
-- Run once after deploying MIG-v3#12 migration.

INSERT INTO monthly_state (year, month, capital_base, realized_loss, trades_opened, carried_risk, created_at)
SELECT
  EXTRACT(YEAR FROM NOW())::SMALLINT,
  EXTRACT(MONTH FROM NOW())::SMALLINT,
  0,  -- capital_base must be set by operator or MonthBoundaryReset handler
  COALESCE(SUM(
    CASE
      WHEN (realized_pnl - total_fees) < 0 THEN ABS(realized_pnl - total_fees)
      ELSE 0
    END
  ), 0),
  COUNT(*),
  0,  -- carried_risk must be set by operator or MonthBoundaryReset handler
  NOW()
FROM positions_current
WHERE state = 'closed'
  AND EXTRACT(YEAR FROM closed_at) = EXTRACT(YEAR FROM NOW())
  AND EXTRACT(MONTH FROM closed_at) = EXTRACT(MONTH FROM NOW())
ON CONFLICT (year, month) DO NOTHING;
