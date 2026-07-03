-- Persist the exchange-side protective stop linkage (ADR-0039 / ADR-0041).
--
-- Incident 2026-07-03: positions_current had no insurance-order column, so
-- every robsond restart hydrated Active positions with insurance_stop_id =
-- NULL. The startup heal then re-placed a fresh stop (orphaning the previous
-- one) and the linkage to the FILLED stop was lost, leaving the position's
-- reconciled close without its evidence anchor.
--
-- The column stores the exchange's conditional-order id (algoId) as text.

ALTER TABLE positions_current
    ADD COLUMN IF NOT EXISTS insurance_stop_id TEXT;

COMMENT ON COLUMN positions_current.insurance_stop_id IS
    'Exchange conditional-order id (algoId) of the live protective insurance stop (ADR-0039); NULL when none is live';
