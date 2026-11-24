DO $migration50$
BEGIN
    -- ADR-0056 step 8l-1, S8-4 (5 of 5): durable month invalidations. The only reason is a fill of an order
    -- with no governed leg, booked in the UTC month of its venue trade time; evidence_key makes the insert
    -- idempotent without NULLS NOT DISTINCT (PostgreSQL 12). Late evidence needs no row: it is derived at read
    -- time from immutable fills.
    CREATE TABLE public.accounting_invalidations (
        invalidation_id UUID NOT NULL,
        tenant_id UUID NOT NULL,
        venue_id TEXT NOT NULL,
        month DATE NOT NULL,
        reason TEXT NOT NULL,
        evidence_key TEXT NOT NULL,
        account_scope TEXT,
        symbol TEXT,
        exchange_order_id TEXT,
        trade_id TEXT,
        trade_time TIMESTAMPTZ,
        CONSTRAINT accounting_invalidations_key PRIMARY KEY (invalidation_id),
        CONSTRAINT accounting_invalidations_once UNIQUE (tenant_id, venue_id, month, reason, evidence_key),
        CONSTRAINT accounting_invalidations_version_row FOREIGN KEY (tenant_id, venue_id)
            REFERENCES public.accounting_state (tenant_id, venue_id),
        CONSTRAINT accounting_invalidations_ids CHECK (
            invalidation_id <> '00000000-0000-0000-0000-000000000000'::UUID
            AND tenant_id <> '00000000-0000-0000-0000-000000000000'::UUID),
        CONSTRAINT accounting_invalidations_month CHECK ((EXTRACT(DAY FROM month) = 1) IS TRUE),
        CONSTRAINT accounting_invalidations_reason CHECK (reason IN ('unattributed_fill')),
        CONSTRAINT accounting_invalidations_evidence_key CHECK (btrim(evidence_key) <> ''),
        CONSTRAINT accounting_invalidations_fill CHECK (
            reason <> 'unattributed_fill'
            OR (num_nulls(account_scope, symbol, exchange_order_id, trade_id, trade_time) = 0
                AND month = date_trunc('month', trade_time AT TIME ZONE 'UTC')::DATE))
    );

    CREATE TRIGGER accounting_invalidations_are_immutable BEFORE UPDATE OR DELETE
        ON public.accounting_invalidations
        FOR EACH ROW EXECUTE FUNCTION public.ledger_row_is_immutable();
END;
$migration50$;
