-- ADR-0045 §1: typed income-ledger reconciliation.
--
-- Canonical, item-typed decomposition of every balance movement (Binance
-- USD-M `GET /fapi/v1/income`). Replaces the scalar wallet-drift check as
-- the source of truth for reconciling money — a residual on a total cannot
-- be decomposed after the fact; matching happens at the item level, where
-- the exchange already provides it.

CREATE TABLE income_ledger (
    id                  UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    -- Exchange-assigned id for this income item (`tranId`). Unique per item
    -- — the idempotent-ingestion key so a re-poll of an overlapping window
    -- never double-inserts.
    exchange_income_id  TEXT NOT NULL UNIQUE,
    -- NULL for account-level items with no symbol (e.g. TRANSFER).
    symbol              TEXT,
    income_type         TEXT NOT NULL,
    amount              NUMERIC(20,8) NOT NULL,
    asset               TEXT NOT NULL,
    -- Linkage to the originating trade, when the exchange provides one.
    -- Always NULL for FUNDING_FEE (funding never links to a governed fill).
    -- Preserved even where not yet used for matching (see income_ledger.rs
    -- doc comment) so a future, more precise matcher can use it.
    exchange_trade_id   TEXT,
    income_time         TIMESTAMPTZ NOT NULL,
    -- NULL = unmatched (a named, persistent anomaly per ADR-0045 §2).
    matched_event_id    UUID,
    matched_at          TIMESTAMPTZ,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Ingestion checkpoint: `MAX(income_time)` across the table doubles as the
-- resume point after a restart, so no separate checkpoint table is needed.
CREATE INDEX idx_income_ledger_income_time ON income_ledger(income_time);
CREATE INDEX idx_income_ledger_unmatched ON income_ledger(income_time) WHERE matched_at IS NULL;
CREATE INDEX idx_income_ledger_symbol_time ON income_ledger(symbol, income_time);
