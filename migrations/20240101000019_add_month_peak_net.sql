-- ADR-0046: Persist monthly high-water mark of governed equity net.

ALTER TABLE monthly_state
  ADD COLUMN month_peak_net NUMERIC(20,8) NOT NULL DEFAULT 0;
