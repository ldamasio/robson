-- Issue #140: auditable operator acknowledgement for unmatched income items.
--
-- Acknowledgement is intentionally distinct from matching. It silences the
-- operational alarm without inventing governed evidence or deleting the
-- exchange record.

ALTER TABLE income_ledger
    ADD COLUMN acked_at TIMESTAMPTZ,
    ADD COLUMN ack_reason TEXT,
    ADD COLUMN acked_by TEXT,
    ADD CONSTRAINT chk_income_ledger_ack_complete CHECK (
        (acked_at IS NULL AND ack_reason IS NULL AND acked_by IS NULL)
        OR (
            acked_at IS NOT NULL
            AND NULLIF(BTRIM(ack_reason), '') IS NOT NULL
            AND CHAR_LENGTH(ack_reason) <= 2000
            AND NULLIF(BTRIM(acked_by), '') IS NOT NULL
            AND CHAR_LENGTH(acked_by) <= 255
        )
    );

DROP INDEX idx_income_ledger_unmatched;
CREATE INDEX idx_income_ledger_unmatched
    ON income_ledger(income_time)
    WHERE matched_at IS NULL AND acked_at IS NULL;
