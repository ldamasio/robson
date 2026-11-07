-- 20240101000011 aggregates without GROUP BY, so on empty tables it still
-- inserts a current-month row with capital_base = 0. 20240101000021 then stamps
-- boundary_reset_at on that row, so handle_month_boundary skips the month and a
-- fresh database keeps the zero base until the next month's reset.
--
-- Remove the row. With no row, load_monthly_state falls back to configured
-- capital and the next boundary poll runs the month's reset.
--
-- The filter leaves every row that records something:
-- - Earlier months are history. "Current" is the earlier of the UTC month, which
--   the daemon uses, and the session's month, which 20240101000011 used: west of
--   UTC the session can still be in the month before.
-- - An event on the month's boundary stream (MonthBoundaryReset or
--   CapitalBaseRecalibrated) means the zero came from the account.
-- - A counter, a peak or a budget model set by the operator would be lost.

DELETE FROM monthly_state AS ms
WHERE ms.capital_base = 0
  AND ms.carried_risk = 0
  AND ms.realized_loss = 0
  AND ms.trades_opened = 0
  AND ms.month_peak_net = 0
  AND ms.monthly_budget_model = 'hwm_v1'
  AND make_date(ms.year, ms.month, 1) >= LEAST(
      date_trunc('month', NOW() AT TIME ZONE 'UTC'),
      date_trunc('month', NOW()::TIMESTAMP)
  )
  AND NOT EXISTS (
      SELECT 1
      FROM event_log AS e
      WHERE e.stream_key = 'system:month_boundary:'
          || lpad(ms.year::TEXT, 4, '0') || '-' || lpad(ms.month::TEXT, 2, '0')
  );
