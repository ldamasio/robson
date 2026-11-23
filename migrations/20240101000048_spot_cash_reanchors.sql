DO $migration48$
BEGIN
    -- ADR-0056 step 8l-1, S8-4 (3 of 5): every sighting a discrepancy rests on, and the re-anchor, the only
    -- way an external movement enters the ledger (operator decision A). Both are immutable. A sighting bumps
    -- no version; a re-anchor's dependencies (O1, O2, its dating copy, the resolved discrepancy's sightings and
    -- those already excluded at its commit) stay readable for the late-evidence rule. An attested re-anchor
    -- computes delta against the old expected quote and attests its flows; a recovery re-anchor (operator
    -- decision Q2 = R, 2026-11-21) recovers an anchor whose own O1..O2 hull late evidence made dirty: it runs
    -- O1 and O2 as usual, has no delta, dating copy or amounts, and admits no attested flow (S8-4 file 4).
    CREATE TABLE public.cash_sightings (
        discrepancy_id UUID NOT NULL,
        seq INTEGER NOT NULL,
        tenant_id UUID NOT NULL,
        venue_id TEXT NOT NULL,
        send_at TIMESTAMPTZ NOT NULL,
        receive_at TIMESTAMPTZ NOT NULL,
        u_ns BIGINT NOT NULL,
        sample_id UUID NOT NULL,
        epoch_id UUID NOT NULL,
        interval_start TIMESTAMPTZ NOT NULL,
        interval_end TIMESTAMPTZ NOT NULL,
        delta NUMERIC NOT NULL,
        CONSTRAINT cash_sightings_key PRIMARY KEY (discrepancy_id, seq),
        CONSTRAINT cash_sightings_discrepancy FOREIGN KEY (discrepancy_id, tenant_id, venue_id)
            REFERENCES public.cash_discrepancies (discrepancy_id, tenant_id, venue_id),
        CONSTRAINT cash_sightings_version_row FOREIGN KEY (tenant_id, venue_id)
            REFERENCES public.accounting_state (tenant_id, venue_id),
        CONSTRAINT cash_sightings_sample FOREIGN KEY (sample_id, tenant_id, venue_id)
            REFERENCES public.clock_samples (sample_id, tenant_id, venue_id),
        CONSTRAINT cash_sightings_ids CHECK (
            tenant_id <> '00000000-0000-0000-0000-000000000000'::UUID
            AND sample_id <> '00000000-0000-0000-0000-000000000000'::UUID
            AND epoch_id <> '00000000-0000-0000-0000-000000000000'::UUID),
        CONSTRAINT cash_sightings_seq CHECK (seq >= 1),
        CONSTRAINT cash_sightings_delta CHECK (ledger_amount_fits(delta)),
        CONSTRAINT cash_sightings_observation CHECK (
            u_ns >= 0 AND interval_start <= send_at AND send_at <= receive_at AND receive_at <= interval_end)
    );

    CREATE TRIGGER cash_sightings_are_immutable BEFORE UPDATE OR DELETE ON public.cash_sightings
        FOR EACH ROW EXECUTE FUNCTION public.ledger_row_is_immutable();

    CREATE TABLE public.cash_reanchors (
        reanchor_id UUID NOT NULL,
        tenant_id UUID NOT NULL,
        venue_id TEXT NOT NULL,
        operator_subject TEXT NOT NULL,
        reason TEXT NOT NULL,
        o1_send_at TIMESTAMPTZ NOT NULL,
        o1_receive_at TIMESTAMPTZ NOT NULL,
        o1_u_ns BIGINT NOT NULL,
        o1_sample_id UUID NOT NULL,
        o1_epoch_id UUID NOT NULL,
        o1_interval_start TIMESTAMPTZ NOT NULL,
        o1_interval_end TIMESTAMPTZ NOT NULL,
        o1_total NUMERIC NOT NULL,
        o2_send_at TIMESTAMPTZ NOT NULL,
        o2_receive_at TIMESTAMPTZ NOT NULL,
        o2_u_ns BIGINT NOT NULL,
        o2_sample_id UUID NOT NULL,
        o2_epoch_id UUID NOT NULL,
        o2_interval_start TIMESTAMPTZ NOT NULL,
        o2_interval_end TIMESTAMPTZ NOT NULL,
        o2_total NUMERIC NOT NULL,
        mode TEXT NOT NULL,
        checked_version BIGINT NOT NULL,
        old_anchor_id UUID NOT NULL,
        new_anchor_id UUID NOT NULL,
        dating_cut TIMESTAMPTZ,
        dating_interval_start TIMESTAMPTZ,
        dating_interval_end TIMESTAMPTZ,
        delta NUMERIC,
        min_delta NUMERIC,
        withdrawal NUMERIC,
        contribution NUMERIC,
        resolved_discrepancy_id UUID,
        excluded_sighting_seqs INTEGER[] NOT NULL,
        voided_discrepancy_id UUID,
        CONSTRAINT cash_reanchors_key PRIMARY KEY (reanchor_id),
        CONSTRAINT cash_reanchors_flow_key UNIQUE (reanchor_id, tenant_id, venue_id, mode),
        CONSTRAINT cash_reanchors_new_anchor_once UNIQUE (new_anchor_id),
        CONSTRAINT cash_reanchors_resolves_once UNIQUE (resolved_discrepancy_id),
        CONSTRAINT cash_reanchors_version_row FOREIGN KEY (tenant_id, venue_id)
            REFERENCES public.accounting_state (tenant_id, venue_id),
        CONSTRAINT cash_reanchors_o1_sample FOREIGN KEY (o1_sample_id, tenant_id, venue_id)
            REFERENCES public.clock_samples (sample_id, tenant_id, venue_id),
        CONSTRAINT cash_reanchors_o2_sample FOREIGN KEY (o2_sample_id, tenant_id, venue_id)
            REFERENCES public.clock_samples (sample_id, tenant_id, venue_id),
        CONSTRAINT cash_reanchors_old_anchor FOREIGN KEY (old_anchor_id, tenant_id, venue_id)
            REFERENCES public.cash_anchors (anchor_id, tenant_id, venue_id),
        CONSTRAINT cash_reanchors_new_anchor FOREIGN KEY (new_anchor_id, tenant_id, venue_id)
            REFERENCES public.cash_anchors (anchor_id, tenant_id, venue_id),
        CONSTRAINT cash_reanchors_resolved FOREIGN KEY (resolved_discrepancy_id, tenant_id, venue_id)
            REFERENCES public.cash_discrepancies (discrepancy_id, tenant_id, venue_id),
        CONSTRAINT cash_reanchors_voided FOREIGN KEY (voided_discrepancy_id, tenant_id, venue_id)
            REFERENCES public.cash_discrepancies (discrepancy_id, tenant_id, venue_id),
        CONSTRAINT cash_reanchors_ids CHECK (
            reanchor_id <> '00000000-0000-0000-0000-000000000000'::UUID
            AND tenant_id <> '00000000-0000-0000-0000-000000000000'::UUID
            AND o1_sample_id <> '00000000-0000-0000-0000-000000000000'::UUID
            AND o1_epoch_id <> '00000000-0000-0000-0000-000000000000'::UUID
            AND o2_sample_id <> '00000000-0000-0000-0000-000000000000'::UUID
            AND o2_epoch_id <> '00000000-0000-0000-0000-000000000000'::UUID),
        CONSTRAINT cash_reanchors_identity CHECK (btrim(operator_subject) <> '' AND btrim(reason) <> ''),
        CONSTRAINT cash_reanchors_o1 CHECK (
            o1_u_ns >= 0 AND o1_interval_start <= o1_send_at AND o1_send_at <= o1_receive_at
            AND o1_receive_at <= o1_interval_end AND ledger_amount_fits(o1_total) AND o1_total >= 0),
        CONSTRAINT cash_reanchors_o2 CHECK (
            o2_u_ns >= 0 AND o2_interval_start <= o2_send_at AND o2_send_at <= o2_receive_at
            AND o2_receive_at <= o2_interval_end AND ledger_amount_fits(o2_total) AND o2_total >= 0),
        -- O1 ends before O2's cut, so the contribution dated at O1's receipt plus u precedes the new anchor;
        -- the two totals agree within 0.01
        CONSTRAINT cash_reanchors_observations CHECK (
            o1_interval_end < o2_interval_start AND abs(o1_total - o2_total) <= 0.01),
        CONSTRAINT cash_reanchors_mode CHECK (mode IN ('attested', 'recovery')),
        -- an attested re-anchor carries its dating copy and amounts; a recovery re-anchor carries none
        CONSTRAINT cash_reanchors_mode_shape CHECK (
            num_nulls(dating_cut, dating_interval_start, dating_interval_end, delta, min_delta, withdrawal,
                      contribution) = CASE mode WHEN 'attested' THEN 0 ELSE 7 END),
        CONSTRAINT cash_reanchors_checked_version CHECK (checked_version >= 0),
        CONSTRAINT cash_reanchors_anchors CHECK (old_anchor_id <> new_anchor_id),
        -- the effective dating copy; the withdrawal it dates precedes the new anchor
        CONSTRAINT cash_reanchors_dating CHECK (
            dating_interval_start <= dating_cut AND dating_cut <= dating_interval_end
            AND dating_cut < o2_interval_start),
        -- min_delta = min(effective minimum, delta at O1, delta at O2, 0), delta being O2's; W = -min_delta
        CONSTRAINT cash_reanchors_amounts CHECK (
            ledger_amount_fits(delta) AND ledger_amount_fits(min_delta) AND ledger_amount_fits(withdrawal)
            AND ledger_amount_fits(contribution) AND min_delta <= 0 AND min_delta <= delta
            AND withdrawal = -min_delta AND contribution >= 0),
        CONSTRAINT cash_reanchors_flow_balance CHECK (contribution - withdrawal = delta),
        -- an open discrepancy is either voided by late evidence (then none is open) or resolved
        CONSTRAINT cash_reanchors_discrepancy CHECK (
            voided_discrepancy_id IS NULL OR resolved_discrepancy_id IS NULL),
        CONSTRAINT cash_reanchors_excluded CHECK (
            cardinality(excluded_sighting_seqs) = 0
            OR (resolved_discrepancy_id IS NOT NULL AND array_ndims(excluded_sighting_seqs) = 1
                AND array_position(excluded_sighting_seqs, NULL) IS NULL AND 1 <= ALL (excluded_sighting_seqs)))
    );

    CREATE TRIGGER cash_reanchors_are_immutable BEFORE UPDATE OR DELETE ON public.cash_reanchors
        FOR EACH ROW EXECUTE FUNCTION public.ledger_row_is_immutable();
END;
$migration48$;
