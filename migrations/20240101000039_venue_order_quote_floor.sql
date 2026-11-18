-- Trusted lower bound on an order's cumulative quote. cumulative_quote keeps
-- its meaning: the exact total for executed_cum, or NULL when unknown. The
-- floor is the greatest exact total ever accepted for the order, kept when
-- growing execution makes the exact total unknown, so a later lower total is
-- refused instead of accepted. NULL means no exact total was ever recorded;
-- existing rows are not backfilled with an inferred value.
--
-- The check is row-local and holds for every value either writer produces:
-- a pre-0039 writer never names quote_floor, so it can only lower
-- cumulative_quote below a recorded floor by accepting a contradictory
-- total, which this constraint refuses (fail closed).
--
-- Every existing row starts with a NULL floor, which satisfies both checks, so
-- they are added NOT VALID to skip a scan of venue_orders. PostgreSQL still
-- enforces NOT VALID checks on every insert and update.
ALTER TABLE venue_orders
    ADD COLUMN quote_floor NUMERIC,
    ADD CONSTRAINT venue_orders_quote_floor_fit
        CHECK (ledger_amount_fits(quote_floor) AND quote_floor >= 0) NOT VALID,
    ADD CONSTRAINT venue_orders_quote_floor_bound
        CHECK (quote_floor IS NULL OR cumulative_quote IS NULL OR cumulative_quote >= quote_floor) NOT VALID;
