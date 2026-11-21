DO $migration54$
BEGIN
    -- ADR-0056 step 8p-1, S8-5 (4 of 4): the activation basis of a cutover (written by 8m: pending in tx2,
    -- certified at completion with as_of equal to its cutover anchor's), basis = min(previous basis, quote
    -- total), at most one certified per venue. The boundary snapshots start after the certified as_of.
    CREATE TABLE public.activation_bases (
        tenant_id UUID NOT NULL,
        venue_id TEXT NOT NULL,
        cutover_id UUID NOT NULL,
        month DATE NOT NULL,
        previous_basis NUMERIC NOT NULL,
        quote_total NUMERIC NOT NULL,
        basis NUMERIC NOT NULL,
        as_of TIMESTAMPTZ,
        state TEXT NOT NULL,
        created_at TIMESTAMPTZ NOT NULL,
        certified_at TIMESTAMPTZ,
        CONSTRAINT activation_bases_key PRIMARY KEY (tenant_id, venue_id, cutover_id),
        CONSTRAINT activation_bases_version_row FOREIGN KEY (tenant_id, venue_id)
            REFERENCES public.accounting_state (tenant_id, venue_id),
        -- The cutover's anchor (cash_anchors_cutover_once). cutover_id's foreign key to venue_cutovers comes
        -- with S8-6 (8m), as cash_anchors.cutover_id's.
        CONSTRAINT activation_bases_anchor FOREIGN KEY (tenant_id, venue_id, cutover_id)
            REFERENCES public.cash_anchors (tenant_id, venue_id, cutover_id),
        CONSTRAINT activation_bases_ids CHECK (
            tenant_id <> '00000000-0000-0000-0000-000000000000'::UUID
            AND cutover_id <> '00000000-0000-0000-0000-000000000000'::UUID),
        CONSTRAINT activation_bases_month CHECK (EXTRACT(DAY FROM month) = 1),
        CONSTRAINT activation_bases_amounts CHECK (
            ledger_amount_fits(previous_basis) AND ledger_amount_fits(quote_total) AND ledger_amount_fits(basis)
            AND previous_basis >= 0 AND quote_total >= 0),
        CONSTRAINT activation_bases_basis CHECK (basis = LEAST(previous_basis, quote_total)),
        CONSTRAINT activation_bases_as_of CHECK (
            as_of >= (month::TIMESTAMP AT TIME ZONE 'UTC')
            AND as_of < ((month + INTERVAL '1 month') AT TIME ZONE 'UTC')),
        CONSTRAINT activation_bases_state CHECK (state IN ('pending', 'certified')),
        CONSTRAINT activation_bases_certified CHECK (
            (state = 'certified') = (certified_at IS NOT NULL) AND (state <> 'certified' OR as_of IS NOT NULL))
    );

    CREATE TRIGGER activation_bases_move_forward BEFORE INSERT OR UPDATE OR DELETE ON public.activation_bases
        FOR EACH ROW EXECUTE FUNCTION public.activation_basis_moves_forward();

    CREATE UNIQUE INDEX idx_activation_bases_certified ON public.activation_bases (tenant_id, venue_id)
        WHERE state = 'certified';
END;
$migration54$;
