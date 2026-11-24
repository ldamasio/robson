DO $migration49$
BEGIN
    -- ADR-0056 step 8l-1, S8-4 (4 of 5): the flows a re-anchor attests and the dust each closed leg leaves.
    -- Both are immutable. A withdrawal is dated at the earliest permissible time (its checkpoint cut rule) and
    -- a contribution at the latest that precedes the new anchor (O1's receipt plus u); each belongs to the UTC
    -- month of its economic date. Only an attested re-anchor has flows: the foreign key carries the mode, and a
    -- recovery re-anchor (decision Q2 = R) can have none. W_pool is the sum of pooled = D + R.
    CREATE TABLE public.cash_attested_flows (
        tenant_id UUID NOT NULL,
        venue_id TEXT NOT NULL,
        economic_date TIMESTAMPTZ NOT NULL,
        flow_id UUID NOT NULL,
        reanchor_id UUID NOT NULL,
        direction TEXT NOT NULL,
        amount NUMERIC NOT NULL,
        date_rule TEXT NOT NULL,
        attributed_month DATE NOT NULL,
        reanchor_mode TEXT NOT NULL,
        CONSTRAINT cash_attested_flows_key PRIMARY KEY (tenant_id, venue_id, economic_date, flow_id),
        CONSTRAINT cash_attested_flows_once UNIQUE (reanchor_id, direction),
        CONSTRAINT cash_attested_flows_version_row FOREIGN KEY (tenant_id, venue_id)
            REFERENCES public.accounting_state (tenant_id, venue_id),
        CONSTRAINT cash_attested_flows_reanchor FOREIGN KEY (reanchor_id, tenant_id, venue_id, reanchor_mode)
            REFERENCES public.cash_reanchors (reanchor_id, tenant_id, venue_id, mode),
        CONSTRAINT cash_attested_flows_reanchor_mode CHECK (reanchor_mode = 'attested'),
        CONSTRAINT cash_attested_flows_ids CHECK (
            tenant_id <> '00000000-0000-0000-0000-000000000000'::UUID
            AND flow_id <> '00000000-0000-0000-0000-000000000000'::UUID),
        CONSTRAINT cash_attested_flows_direction CHECK (direction IN ('withdrawal', 'contribution')),
        CONSTRAINT cash_attested_flows_amount CHECK (ledger_amount_fits(amount) AND amount > 0),
        CONSTRAINT cash_attested_flows_date_rule CHECK (
            date_rule IN ('checkpoint_cut', 'o1_receipt_plus_u')
            AND (direction = 'withdrawal') = (date_rule = 'checkpoint_cut')),
        CONSTRAINT cash_attested_flows_month CHECK (
            isfinite(economic_date)
            AND attributed_month = date_trunc('month', economic_date AT TIME ZONE 'UTC')::DATE)
    );

    CREATE TRIGGER cash_attested_flows_are_immutable BEFORE UPDATE OR DELETE ON public.cash_attested_flows
        FOR EACH ROW EXECUTE FUNCTION public.ledger_row_is_immutable();

    -- D (entry_dust) and R (write_off) of a closed leg, pooled as W = D + R. A write-off (R > 0) is covered:
    -- its loss is within the leg's charge and it is unsellable inside the envelope, with the minimum notional and
    -- the price it was judged at. close_at is Robson's close time with its clock evidence (NULL evidence is
    -- invalid evidence); last_fill_at is the venue time of the leg's last fill.
    CREATE TABLE public.dust_ledger (
        position_id UUID NOT NULL,
        tenant_id UUID NOT NULL,
        venue_id TEXT NOT NULL,
        account_scope TEXT NOT NULL,
        symbol TEXT NOT NULL,
        entry_dust NUMERIC NOT NULL,
        write_off NUMERIC NOT NULL,
        pooled NUMERIC NOT NULL,
        close_at TIMESTAMPTZ NOT NULL,
        close_clock_sample_id UUID,
        close_clock_wall_delta_ms BIGINT,
        close_clock_mono_delta_ms BIGINT,
        close_clock_verdict TEXT,
        last_fill_at TIMESTAMPTZ NOT NULL,
        loss_within_charge BOOLEAN,
        unsellable_in_envelope BOOLEAN,
        min_notional NUMERIC,
        write_off_price NUMERIC,
        CONSTRAINT dust_ledger_key PRIMARY KEY (position_id),
        CONSTRAINT dust_ledger_leg FOREIGN KEY (position_id, tenant_id, venue_id, account_scope, symbol)
            REFERENCES public.balance_legs (position_id, tenant_id, venue_id, account_scope, symbol),
        CONSTRAINT dust_ledger_version_row FOREIGN KEY (tenant_id, venue_id)
            REFERENCES public.accounting_state (tenant_id, venue_id),
        CONSTRAINT dust_ledger_close_sample FOREIGN KEY (close_clock_sample_id, tenant_id, venue_id)
            REFERENCES public.clock_samples (sample_id, tenant_id, venue_id),
        CONSTRAINT dust_ledger_amounts CHECK (
            ledger_amount_fits(entry_dust) AND ledger_amount_fits(write_off) AND ledger_amount_fits(pooled)
            AND entry_dust >= 0 AND write_off >= 0),
        CONSTRAINT dust_ledger_pool CHECK (pooled = entry_dust + write_off),
        CONSTRAINT dust_ledger_close_verdict CHECK (close_clock_verdict IN ('valid', 'invalid')),
        CONSTRAINT dust_ledger_close_evidence_whole CHECK (
            close_clock_verdict IS DISTINCT FROM 'valid'
            OR (close_clock_sample_id IS NOT NULL AND close_clock_wall_delta_ms IS NOT NULL
                AND close_clock_mono_delta_ms IS NOT NULL)),
        CONSTRAINT dust_ledger_evidence_amounts CHECK (
            (min_notional IS NULL OR (ledger_amount_fits(min_notional) AND min_notional > 0))
            AND (write_off_price IS NULL OR (ledger_amount_fits(write_off_price) AND write_off_price > 0))),
        CONSTRAINT dust_ledger_covered_write_off CHECK (
            write_off = 0
            OR (loss_within_charge IS TRUE AND unsellable_in_envelope IS TRUE
                AND min_notional IS NOT NULL AND write_off_price IS NOT NULL))
    );

    CREATE TRIGGER dust_ledger_is_immutable BEFORE UPDATE OR DELETE ON public.dust_ledger
        FOR EACH ROW EXECUTE FUNCTION public.ledger_row_is_immutable();
END;
$migration49$;
