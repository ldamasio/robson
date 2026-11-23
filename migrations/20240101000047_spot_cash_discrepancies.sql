DO $migration47$
BEGIN
    -- ADR-0056 step 8l-1, S8-4 (2 of 5): unresolved cash discrepancies. Evidence, never an accounting write:
    -- the first sighting and the checkpoint copied at that moment are immutable; later sightings move only
    -- last_seen_at forward and min_delta down, without a version bump. Missing cash is never voided by a clean
    -- observation, only by late evidence or resolved by a re-anchor.
    CREATE FUNCTION public.cash_discrepancy_moves_forward() RETURNS TRIGGER
        LANGUAGE plpgsql SET search_path = pg_catalog, public AS $discrepancy_guard$
    BEGIN
        IF TG_OP = 'DELETE' THEN
            RAISE EXCEPTION 'ledger invariant: cash discrepancies are never deleted' USING ERRCODE = '23514';
        END IF;
        IF (NEW.discrepancy_id, NEW.tenant_id, NEW.venue_id, NEW.direction, NEW.first_send_at,
            NEW.first_receive_at, NEW.first_u_ns, NEW.first_sample_id, NEW.first_epoch_id,
            NEW.first_interval_start, NEW.first_interval_end, NEW.first_delta, NEW.checkpoint_cut,
            NEW.checkpoint_interval_start, NEW.checkpoint_interval_end, NEW.opened_version)
               IS DISTINCT FROM
           (OLD.discrepancy_id, OLD.tenant_id, OLD.venue_id, OLD.direction, OLD.first_send_at,
            OLD.first_receive_at, OLD.first_u_ns, OLD.first_sample_id, OLD.first_epoch_id,
            OLD.first_interval_start, OLD.first_interval_end, OLD.first_delta, OLD.checkpoint_cut,
            OLD.checkpoint_interval_start, OLD.checkpoint_interval_end, OLD.opened_version)
           -- open -> void | resolved; a closed row never changes again
           OR OLD.state <> 'open'
           OR NEW.last_seen_at < OLD.last_seen_at
           OR NEW.min_delta > OLD.min_delta
        THEN
            RAISE EXCEPTION 'ledger invariant: cash discrepancy % only moves forward', OLD.discrepancy_id
                USING ERRCODE = '23514';
        END IF;
        RETURN NEW;
    END;
    $discrepancy_guard$;

    CREATE TABLE public.cash_discrepancies (
        discrepancy_id UUID NOT NULL,
        tenant_id UUID NOT NULL,
        venue_id TEXT NOT NULL,
        direction TEXT NOT NULL,
        state TEXT NOT NULL,
        first_send_at TIMESTAMPTZ NOT NULL,
        first_receive_at TIMESTAMPTZ NOT NULL,
        first_u_ns BIGINT NOT NULL,
        first_sample_id UUID NOT NULL,
        first_epoch_id UUID NOT NULL,
        first_interval_start TIMESTAMPTZ NOT NULL,
        first_interval_end TIMESTAMPTZ NOT NULL,
        first_delta NUMERIC NOT NULL,
        checkpoint_cut TIMESTAMPTZ NOT NULL,
        checkpoint_interval_start TIMESTAMPTZ NOT NULL,
        checkpoint_interval_end TIMESTAMPTZ NOT NULL,
        opened_version BIGINT NOT NULL,
        last_seen_at TIMESTAMPTZ NOT NULL,
        min_delta NUMERIC NOT NULL,
        void_reason TEXT,
        closed_at TIMESTAMPTZ,
        CONSTRAINT cash_discrepancies_key PRIMARY KEY (discrepancy_id),
        CONSTRAINT cash_discrepancies_scope UNIQUE (discrepancy_id, tenant_id, venue_id),
        CONSTRAINT cash_discrepancies_version_row FOREIGN KEY (tenant_id, venue_id)
            REFERENCES public.accounting_state (tenant_id, venue_id),
        CONSTRAINT cash_discrepancies_first_sample FOREIGN KEY (first_sample_id, tenant_id, venue_id)
            REFERENCES public.clock_samples (sample_id, tenant_id, venue_id),
        CONSTRAINT cash_discrepancies_ids CHECK (
            discrepancy_id <> '00000000-0000-0000-0000-000000000000'::UUID
            AND tenant_id <> '00000000-0000-0000-0000-000000000000'::UUID
            AND first_sample_id <> '00000000-0000-0000-0000-000000000000'::UUID
            AND first_epoch_id <> '00000000-0000-0000-0000-000000000000'::UUID),
        CONSTRAINT cash_discrepancies_direction CHECK (direction IN ('missing', 'extra')),
        CONSTRAINT cash_discrepancies_state CHECK (state IN ('open', 'void', 'resolved')),
        CONSTRAINT cash_discrepancies_first_observation CHECK (
            first_u_ns >= 0 AND first_interval_start <= first_send_at AND first_send_at <= first_receive_at
            AND first_receive_at <= first_interval_end),
        -- the sign matches the direction and the first sighting is beyond the 0.01 tolerance
        CONSTRAINT cash_discrepancies_first_delta CHECK (
            ledger_amount_fits(first_delta)
            AND ((direction = 'missing' AND first_delta < -0.01) OR (direction = 'extra' AND first_delta > 0.01))),
        CONSTRAINT cash_discrepancies_checkpoint CHECK (
            checkpoint_interval_start <= checkpoint_cut AND checkpoint_cut <= checkpoint_interval_end
            AND checkpoint_cut <= first_interval_start),
        CONSTRAINT cash_discrepancies_opened_version CHECK (opened_version >= 0),
        CONSTRAINT cash_discrepancies_min_delta CHECK (ledger_amount_fits(min_delta) AND min_delta <= first_delta),
        CONSTRAINT cash_discrepancies_void_reason CHECK (
            void_reason IN ('clean_observation', 'superseded_by_missing', 'late_evidence')),
        CONSTRAINT cash_discrepancies_void_pair CHECK ((state = 'void') = (void_reason IS NOT NULL)),
        CONSTRAINT cash_discrepancies_closed CHECK ((state = 'open') = (closed_at IS NULL)),
        -- Missing cash is voided only by late evidence (a re-anchor resolves it); as a CHECK this holds for an
        -- insert as well as for the trigger-guarded update.
        CONSTRAINT cash_discrepancies_missing_void CHECK (
            direction <> 'missing' OR void_reason IS NULL OR void_reason = 'late_evidence')
    );

    CREATE TRIGGER cash_discrepancies_move_forward BEFORE UPDATE OR DELETE ON public.cash_discrepancies
        FOR EACH ROW EXECUTE FUNCTION public.cash_discrepancy_moves_forward();

    CREATE UNIQUE INDEX idx_cash_discrepancies_open ON public.cash_discrepancies (tenant_id, venue_id)
        WHERE state = 'open';
END;
$migration47$;
