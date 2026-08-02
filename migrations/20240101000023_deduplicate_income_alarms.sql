-- Issue #140: persist alarm transitions and digest throttling.

ALTER TABLE income_ledger
    ADD COLUMN alarmed_at TIMESTAMPTZ;

CREATE INDEX idx_income_ledger_unalarmed
    ON income_ledger(income_time)
    WHERE matched_at IS NULL AND acked_at IS NULL AND alarmed_at IS NULL;

-- One row coordinates digest emission across polls, daemon restarts, and
-- accidental concurrent workers. The row is claimed with an atomic UPDATE.
CREATE TABLE income_ledger_alarm_state (
    singleton       BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK (singleton),
    last_digest_at  TIMESTAMPTZ
);

INSERT INTO income_ledger_alarm_state (singleton) VALUES (TRUE);
