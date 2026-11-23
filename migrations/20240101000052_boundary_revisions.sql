DO $migration52$
BEGIN
    -- ADR-0056 step 8p-1, S8-5 (2 of 4): reopenings become immutable, and each completed revision n of a
    -- month's snapshot is an immutable record of what it was computed from: the observed anchor A before B
    -- (the derived anchor is (as_of, quote_b), recorded, never used for cash), quote_B = E(B), the validated
    -- one-minute kline ending at B, the floors, basis_n = quote_B + sum of F_B, the budget model, the input
    -- digest, and Robson's completion time with valid clock evidence at or after B + 60 s.
    CREATE TRIGGER boundary_reopenings_are_immutable BEFORE UPDATE OR DELETE ON public.boundary_reopenings
        FOR EACH ROW EXECUTE FUNCTION public.ledger_row_is_immutable();

    CREATE TABLE public.boundary_snapshot_revisions (
        tenant_id UUID NOT NULL,
        venue_id TEXT NOT NULL,
        month DATE NOT NULL,
        revision INTEGER NOT NULL,
        as_of TIMESTAMPTZ NOT NULL,
        anchor_id UUID NOT NULL,
        anchor_as_of TIMESTAMPTZ NOT NULL,
        quote_b NUMERIC NOT NULL,
        mark_price NUMERIC NOT NULL,
        mark_kline_open_time TIMESTAMPTZ NOT NULL,
        mark_kline_close_time TIMESTAMPTZ NOT NULL,
        floor_total NUMERIC NOT NULL,
        downside_total NUMERIC NOT NULL,
        basis NUMERIC NOT NULL,
        carried_count INTEGER NOT NULL,
        budget_model TEXT NOT NULL,
        completed_version BIGINT NOT NULL,
        evidence_hash TEXT NOT NULL,
        d_fills BIGINT NOT NULL,
        d_flows BIGINT NOT NULL,
        d_legs BIGINT NOT NULL,
        reason TEXT NOT NULL,
        reopen_seq INTEGER,
        computed_at TIMESTAMPTZ NOT NULL,
        computed_clock_sample_id UUID NOT NULL,
        computed_clock_wall_delta_ms BIGINT NOT NULL,
        computed_clock_mono_delta_ms BIGINT NOT NULL,
        computed_clock_verdict TEXT NOT NULL,
        CONSTRAINT boundary_snapshot_revisions_key PRIMARY KEY (tenant_id, venue_id, month, revision),
        CONSTRAINT boundary_snapshot_revisions_snapshot FOREIGN KEY (tenant_id, venue_id, month)
            REFERENCES public.boundary_snapshots (tenant_id, venue_id, month),
        -- A recomputation names the reopening it follows.
        CONSTRAINT boundary_snapshot_revisions_reopening FOREIGN KEY (tenant_id, venue_id, month, reopen_seq)
            REFERENCES public.boundary_reopenings (tenant_id, venue_id, month, reopen_seq),
        CONSTRAINT boundary_snapshot_revisions_version_row FOREIGN KEY (tenant_id, venue_id)
            REFERENCES public.accounting_state (tenant_id, venue_id),
        CONSTRAINT boundary_snapshot_revisions_anchor FOREIGN KEY (anchor_id, tenant_id, venue_id)
            REFERENCES public.cash_anchors (anchor_id, tenant_id, venue_id),
        CONSTRAINT boundary_snapshot_revisions_computed_sample
            FOREIGN KEY (computed_clock_sample_id, tenant_id, venue_id)
            REFERENCES public.clock_samples (sample_id, tenant_id, venue_id),
        CONSTRAINT boundary_snapshot_revisions_ids CHECK (
            tenant_id <> '00000000-0000-0000-0000-000000000000'::UUID
            AND anchor_id <> '00000000-0000-0000-0000-000000000000'::UUID
            AND computed_clock_sample_id <> '00000000-0000-0000-0000-000000000000'::UUID),
        CONSTRAINT boundary_snapshot_revisions_as_of CHECK (
            as_of = (month::TIMESTAMP AT TIME ZONE 'UTC') AND anchor_as_of < as_of),
        CONSTRAINT boundary_snapshot_revisions_mark CHECK (
            mark_price > 0 AND mark_kline_open_time = as_of - INTERVAL '60 seconds'
            AND mark_kline_close_time = as_of - INTERVAL '1 millisecond'),
        CONSTRAINT boundary_snapshot_revisions_amounts CHECK (
            ledger_amount_fits(quote_b) AND ledger_amount_fits(mark_price) AND ledger_amount_fits(floor_total)
            AND ledger_amount_fits(downside_total) AND ledger_amount_fits(basis)),
        CONSTRAINT boundary_snapshot_revisions_basis CHECK (basis = quote_b + floor_total),
        CONSTRAINT boundary_snapshot_revisions_counts CHECK (
            revision >= 1 AND carried_count >= 0 AND completed_version >= 0
            AND d_fills >= 0 AND d_flows >= 0 AND d_legs >= 0),
        CONSTRAINT boundary_snapshot_revisions_budget_model CHECK (budget_model = 'net_from_start_non_expanding_v1'),
        CONSTRAINT boundary_snapshot_revisions_evidence_hash CHECK (evidence_hash ~ '^[0-9a-f]{64}$'),
        CONSTRAINT boundary_snapshot_revisions_reason CHECK (
            reason IN ('initial', 'reopened') AND (revision = 1) = (reason = 'initial')
            AND (reason = 'reopened') = (reopen_seq IS NOT NULL)),
        CONSTRAINT boundary_snapshot_revisions_computed CHECK (
            computed_clock_verdict = 'valid' AND computed_at >= as_of + INTERVAL '60 seconds')
    );

    CREATE TRIGGER boundary_snapshot_revisions_are_immutable BEFORE UPDATE OR DELETE
        ON public.boundary_snapshot_revisions
        FOR EACH ROW EXECUTE FUNCTION public.ledger_row_is_immutable();
END;
$migration52$;
