-- MIG-v3#12: Add realized_loss and trades_opened to monthly_state
--
-- These columns persist the monthly risk accumulators that were previously
-- recomputed from in-memory closed positions on every risk check.
-- DEFAULT 0 makes the migration non-destructive: existing rows and code that
-- doesn't reference these columns continue to work.

ALTER TABLE monthly_state
  ADD COLUMN realized_loss  NUMERIC(20,8) NOT NULL DEFAULT 0 CHECK (realized_loss >= 0),
  ADD COLUMN trades_opened  INTEGER       NOT NULL DEFAULT 0 CHECK (trades_opened >= 0);
