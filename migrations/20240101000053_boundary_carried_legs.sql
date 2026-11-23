DO $migration53$
BEGIN
    -- ADR-0056 step 8p-1, S8-5 (3 of 4): the legs a revision carries across B, immutable. G_B is the leg's
    -- quantity at B; an entry in flight carries nothing and floors at minus its admission charge; an inventory
    -- leg floors at F_B = max(0, G_B - reserve) * p_B * (1 - f) only when protected at B, otherwise at 0. A
    -- protected leg stores the evidence its proof used, each Robson time with a valid verdict on the sound side
    -- of B: its stop was acknowledged by B - 2 s, and it was open after B by an owner lookup sent at or after
    -- B + 2 s that answered NEW, by a trade after B, or by Robson's own cancel attempted at or after B + 2 s.
    CREATE TABLE public.boundary_carried_legs (
        tenant_id UUID NOT NULL,
        venue_id TEXT NOT NULL,
        month DATE NOT NULL,
        revision INTEGER NOT NULL,
        position_id UUID NOT NULL,
        account_scope TEXT NOT NULL,
        symbol TEXT NOT NULL,
        cohort_month DATE NOT NULL,
        kind TEXT NOT NULL,
        g_b NUMERIC NOT NULL,
        vwap NUMERIC,
        protected_at_b BOOLEAN NOT NULL,
        stop_order_id TEXT,
        trigger_b NUMERIC,
        open_after_b TEXT,
        stop_live_seen_at TIMESTAMPTZ,
        stop_live_clock_verdict TEXT,
        lookup_sent_at TIMESTAMPTZ,
        lookup_clock_sample_id UUID,
        lookup_clock_verdict TEXT,
        lookup_status TEXT,
        stop_trade_id TEXT,
        cancel_intent_id UUID,
        cancel_attempt_at TIMESTAMPTZ,
        cancel_attempt_clock_verdict TEXT,
        exclusions JSONB NOT NULL,
        gap_bps NUMERIC NOT NULL,
        fee_bps NUMERIC NOT NULL,
        reserve_qty NUMERIC NOT NULL,
        admission_charge NUMERIC NOT NULL,
        adverse_fill_b NUMERIC,
        floor_price_b NUMERIC,
        floor_quote NUMERIC NOT NULL,
        mark_value NUMERIC NOT NULL,
        downside NUMERIC NOT NULL,
        prior_month_excess NUMERIC,
        CONSTRAINT boundary_carried_legs_key PRIMARY KEY (tenant_id, venue_id, month, revision, position_id),
        CONSTRAINT boundary_carried_legs_revision FOREIGN KEY (tenant_id, venue_id, month, revision)
            REFERENCES public.boundary_snapshot_revisions (tenant_id, venue_id, month, revision),
        CONSTRAINT boundary_carried_legs_leg FOREIGN KEY (position_id, tenant_id, venue_id, account_scope, symbol)
            REFERENCES public.balance_legs (position_id, tenant_id, venue_id, account_scope, symbol),
        CONSTRAINT boundary_carried_legs_version_row FOREIGN KEY (tenant_id, venue_id)
            REFERENCES public.accounting_state (tenant_id, venue_id),
        CONSTRAINT boundary_carried_legs_lookup_sample FOREIGN KEY (lookup_clock_sample_id, tenant_id, venue_id)
            REFERENCES public.clock_samples (sample_id, tenant_id, venue_id),
        CONSTRAINT boundary_carried_legs_ids CHECK (
            tenant_id <> '00000000-0000-0000-0000-000000000000'::UUID
            AND position_id <> '00000000-0000-0000-0000-000000000000'::UUID
            AND (lookup_clock_sample_id IS NULL
                 OR lookup_clock_sample_id <> '00000000-0000-0000-0000-000000000000'::UUID)
            AND (cancel_intent_id IS NULL OR cancel_intent_id <> '00000000-0000-0000-0000-000000000000'::UUID)
            AND ledger_canonical_id(stop_order_id) AND ledger_canonical_id(stop_trade_id)),
        CONSTRAINT boundary_carried_legs_months CHECK (
            (EXTRACT(DAY FROM cohort_month) = 1 AND cohort_month < month) IS TRUE),
        CONSTRAINT boundary_carried_legs_kind CHECK (kind IN ('inventory', 'entry_in_flight')),
        CONSTRAINT boundary_carried_legs_amounts CHECK (
            ledger_amount_fits(g_b) AND ledger_amount_fits(vwap) AND ledger_amount_fits(trigger_b)
            AND ledger_amount_fits(gap_bps) AND ledger_amount_fits(fee_bps) AND ledger_amount_fits(reserve_qty)
            AND ledger_amount_fits(admission_charge) AND ledger_amount_fits(adverse_fill_b)
            AND ledger_amount_fits(floor_price_b) AND ledger_amount_fits(floor_quote)
            AND ledger_amount_fits(mark_value) AND ledger_amount_fits(downside)
            AND ledger_amount_fits(prior_month_excess)),
        CONSTRAINT boundary_carried_legs_terms CHECK (
            g_b >= 0 AND vwap > 0 AND trigger_b > 0 AND gap_bps >= 0 AND gap_bps < 10000 AND fee_bps >= 0
            AND fee_bps < 10000 AND reserve_qty >= 0 AND admission_charge > 0 AND adverse_fill_b > 0
            AND floor_price_b > 0 AND mark_value >= 0 AND prior_month_excess >= 0),
        CONSTRAINT boundary_carried_legs_in_flight CHECK (
            kind <> 'entry_in_flight' OR (g_b = 0 AND NOT protected_at_b AND floor_quote = -admission_charge)),
        CONSTRAINT boundary_carried_legs_inventory CHECK (
            kind <> 'inventory' OR (g_b > 0 AND floor_quote >= 0 AND (protected_at_b OR floor_quote = 0))),
        CONSTRAINT boundary_carried_legs_downside CHECK (downside = mark_value - floor_quote),
        CONSTRAINT boundary_carried_legs_protection CHECK (
            num_nonnulls(stop_order_id, trigger_b, open_after_b, adverse_fill_b, floor_price_b)
            = CASE WHEN protected_at_b THEN 5 ELSE 0 END),
        CONSTRAINT boundary_carried_legs_open_after_b CHECK (
            open_after_b IN ('new_at_completion', 'trade_after_b', 'robson_cancel_after_b')),
        CONSTRAINT boundary_carried_legs_acknowledged CHECK (
            num_nonnulls(stop_live_seen_at, stop_live_clock_verdict) = CASE WHEN protected_at_b THEN 2 ELSE 0 END
            AND (NOT protected_at_b OR (stop_live_clock_verdict = 'valid'
                 AND stop_live_seen_at <= (month::TIMESTAMP AT TIME ZONE 'UTC') - INTERVAL '2 seconds'))),
        CONSTRAINT boundary_carried_legs_lookup CHECK (
            num_nonnulls(lookup_sent_at, lookup_clock_sample_id, lookup_clock_verdict, lookup_status)
            = CASE WHEN open_after_b = 'new_at_completion' THEN 4 ELSE 0 END
            AND (open_after_b IS DISTINCT FROM 'new_at_completion' OR (lookup_clock_verdict = 'valid'
                 AND lookup_status = 'NEW'
                 AND lookup_sent_at >= (month::TIMESTAMP AT TIME ZONE 'UTC') + INTERVAL '2 seconds'))),
        CONSTRAINT boundary_carried_legs_trade CHECK (
            (stop_trade_id IS NOT NULL) = (open_after_b IS NOT DISTINCT FROM 'trade_after_b')),
        CONSTRAINT boundary_carried_legs_cancel CHECK (
            num_nonnulls(cancel_intent_id, cancel_attempt_at, cancel_attempt_clock_verdict)
            = CASE WHEN open_after_b = 'robson_cancel_after_b' THEN 3 ELSE 0 END
            AND (open_after_b IS DISTINCT FROM 'robson_cancel_after_b' OR (cancel_attempt_clock_verdict = 'valid'
                 AND cancel_attempt_at >= (month::TIMESTAMP AT TIME ZONE 'UTC') + INTERVAL '2 seconds'))),
        CONSTRAINT boundary_carried_legs_exclusions CHECK (
            CASE WHEN jsonb_typeof(exclusions) = 'array' THEN jsonb_array_length(exclusions) <= 64 ELSE FALSE END)
    );

    CREATE TRIGGER boundary_carried_legs_are_immutable BEFORE UPDATE OR DELETE ON public.boundary_carried_legs
        FOR EACH ROW EXECUTE FUNCTION public.ledger_row_is_immutable();

    -- activation_bases' guard (8m writes the rows, migration 54 creates the table): a row starts pending with
    -- no as_of; everything but as_of, state and certified_at is fixed; as_of is set once, from NULL; pending
    -- moves to certified only, and a certified row never changes.
    CREATE FUNCTION public.activation_basis_moves_forward() RETURNS TRIGGER
        LANGUAGE plpgsql SET search_path = pg_catalog, public AS $activation_guard$
    BEGIN
        IF TG_OP = 'DELETE' THEN
            RAISE EXCEPTION 'ledger invariant: activation bases are never deleted' USING ERRCODE = '23514';
        END IF;
        IF TG_OP = 'INSERT' THEN
            IF NEW.state IS DISTINCT FROM 'pending' OR NEW.as_of IS NOT NULL THEN
                RAISE EXCEPTION 'ledger invariant: an activation basis starts pending' USING ERRCODE = '23514';
            END IF;
            RETURN NEW;
        END IF;
        IF (NEW.tenant_id, NEW.venue_id, NEW.cutover_id, NEW.month, NEW.previous_basis, NEW.quote_total,
            NEW.basis, NEW.created_at)
               IS DISTINCT FROM
           (OLD.tenant_id, OLD.venue_id, OLD.cutover_id, OLD.month, OLD.previous_basis, OLD.quote_total,
            OLD.basis, OLD.created_at)
           OR (OLD.as_of IS NOT NULL AND NEW.as_of IS DISTINCT FROM OLD.as_of)
           OR (OLD.state = 'certified' AND (NEW.state, NEW.certified_at) IS DISTINCT FROM (OLD.state, OLD.certified_at))
        THEN
            RAISE EXCEPTION 'ledger invariant: activation basis % only moves forward', OLD.cutover_id
                USING ERRCODE = '23514';
        END IF;
        RETURN NEW;
    END;
    $activation_guard$;
END;
$migration53$;
