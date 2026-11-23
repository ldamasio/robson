DO $migration46$
BEGIN
    -- ADR-0056 step 8l-1, S8-4 (1 of 5): observed cash anchors and the clean checkpoint pointer. Additive and
    -- inert: no writer exists in this slice. An observation is its send and receive times, the clock bound u,
    -- the clock sample and process epoch, and its interval [cut, end]; cuts and interval starts are floored to
    -- the microsecond and interval ends ceiled.
    CREATE FUNCTION public.cash_anchor_moves_forward() RETURNS TRIGGER
        LANGUAGE plpgsql SET search_path = pg_catalog, public AS $anchor_guard$
    BEGIN
        IF TG_OP = 'DELETE' THEN
            RAISE EXCEPTION 'ledger invariant: cash anchors are never deleted' USING ERRCODE = '23514';
        END IF;
        -- Identity, kind and quote never change. The observation, its as_of and the hull are set once, from
        -- NULL, and only a cutover anchor is ever written without them (cash_anchors_pending).
        IF (NEW.anchor_id, NEW.tenant_id, NEW.venue_id, NEW.kind, NEW.cutover_id, NEW.quote)
               IS DISTINCT FROM
           (OLD.anchor_id, OLD.tenant_id, OLD.venue_id, OLD.kind, OLD.cutover_id, OLD.quote)
           OR (OLD.as_of IS NOT NULL AND
               (NEW.as_of, NEW.send_at, NEW.receive_at, NEW.u_ns, NEW.sample_id, NEW.epoch_id,
                NEW.interval_start, NEW.interval_end, NEW.depends_start, NEW.depends_end)
                   IS DISTINCT FROM
               (OLD.as_of, OLD.send_at, OLD.receive_at, OLD.u_ns, OLD.sample_id, OLD.epoch_id,
                OLD.interval_start, OLD.interval_end, OLD.depends_start, OLD.depends_end))
        THEN
            RAISE EXCEPTION 'ledger invariant: a cash anchor only sets its cutover observation once'
                USING ERRCODE = '23514';
        END IF;
        RETURN NEW;
    END;
    $anchor_guard$;

    CREATE TABLE public.cash_anchors (
        anchor_id UUID NOT NULL,
        tenant_id UUID NOT NULL,
        venue_id TEXT NOT NULL,
        kind TEXT NOT NULL,
        cutover_id UUID,
        quote NUMERIC NOT NULL,
        as_of TIMESTAMPTZ,
        send_at TIMESTAMPTZ,
        receive_at TIMESTAMPTZ,
        u_ns BIGINT,
        sample_id UUID,
        epoch_id UUID,
        interval_start TIMESTAMPTZ,
        interval_end TIMESTAMPTZ,
        depends_start TIMESTAMPTZ,
        depends_end TIMESTAMPTZ,
        CONSTRAINT cash_anchors_key PRIMARY KEY (anchor_id),
        CONSTRAINT cash_anchors_scope UNIQUE (anchor_id, tenant_id, venue_id),
        CONSTRAINT cash_anchors_checkpoint_key UNIQUE (anchor_id, tenant_id, venue_id, kind, as_of),
        CONSTRAINT cash_anchors_as_of_once UNIQUE (tenant_id, venue_id, as_of),
        CONSTRAINT cash_anchors_cutover_once UNIQUE (tenant_id, venue_id, cutover_id),
        -- cutover_id's foreign key to venue_cutovers comes with S8-6 (8m), as venue_accounts.cutover_id's.
        CONSTRAINT cash_anchors_version_row FOREIGN KEY (tenant_id, venue_id)
            REFERENCES public.accounting_state (tenant_id, venue_id),
        CONSTRAINT cash_anchors_sample FOREIGN KEY (sample_id, tenant_id, venue_id)
            REFERENCES public.clock_samples (sample_id, tenant_id, venue_id),
        CONSTRAINT cash_anchors_ids CHECK (
            anchor_id <> '00000000-0000-0000-0000-000000000000'::UUID
            AND tenant_id <> '00000000-0000-0000-0000-000000000000'::UUID
            AND (cutover_id IS NULL OR cutover_id <> '00000000-0000-0000-0000-000000000000'::UUID)
            AND (sample_id IS NULL OR sample_id <> '00000000-0000-0000-0000-000000000000'::UUID)
            AND (epoch_id IS NULL OR epoch_id <> '00000000-0000-0000-0000-000000000000'::UUID)),
        CONSTRAINT cash_anchors_kind CHECK (kind IN ('cutover', 'reanchor')),
        CONSTRAINT cash_anchors_cutover CHECK ((kind = 'cutover') = (cutover_id IS NOT NULL)),
        CONSTRAINT cash_anchors_quote CHECK (ledger_amount_fits(quote) AND quote >= 0),
        CONSTRAINT cash_anchors_pending CHECK (as_of IS NOT NULL OR kind = 'cutover'),
        CONSTRAINT cash_anchors_observation_whole CHECK (
            num_nulls(as_of, send_at, receive_at, u_ns, sample_id, epoch_id, interval_start, interval_end,
                      depends_start, depends_end) IN (0, 10)),
        CONSTRAINT cash_anchors_observation CHECK (
            as_of = interval_start AND u_ns >= 0 AND isfinite(interval_start) AND isfinite(interval_end)
            AND interval_start <= send_at AND send_at <= receive_at AND receive_at <= interval_end),
        CONSTRAINT cash_anchors_hull CHECK (
            isfinite(depends_start) AND depends_start <= interval_start AND depends_end = interval_end)
    );

    CREATE TRIGGER cash_anchors_move_forward BEFORE UPDATE OR DELETE ON public.cash_anchors
        FOR EACH ROW EXECUTE FUNCTION public.cash_anchor_moves_forward();

    -- A pointer to the latest clean cut, upserted in place: every consumer copies the cut and interval it uses.
    -- It bumps no version and has no trigger; its writers keep it monotone. A cutover or re-anchor checkpoint
    -- is its anchor's own observation: same venue, same kind, cut equal to the anchor's as_of.
    CREATE TABLE public.cash_checkpoints (
        tenant_id UUID NOT NULL,
        venue_id TEXT NOT NULL,
        cut TIMESTAMPTZ NOT NULL,
        send_at TIMESTAMPTZ NOT NULL,
        receive_at TIMESTAMPTZ NOT NULL,
        u_ns BIGINT NOT NULL,
        sample_id UUID NOT NULL,
        epoch_id UUID NOT NULL,
        interval_start TIMESTAMPTZ NOT NULL,
        interval_end TIMESTAMPTZ NOT NULL,
        source TEXT NOT NULL,
        anchor_id UUID,
        CONSTRAINT cash_checkpoints_key PRIMARY KEY (tenant_id, venue_id),
        CONSTRAINT cash_checkpoints_version_row FOREIGN KEY (tenant_id, venue_id)
            REFERENCES public.accounting_state (tenant_id, venue_id),
        CONSTRAINT cash_checkpoints_sample FOREIGN KEY (sample_id, tenant_id, venue_id)
            REFERENCES public.clock_samples (sample_id, tenant_id, venue_id),
        CONSTRAINT cash_checkpoints_anchor FOREIGN KEY (anchor_id, tenant_id, venue_id, source, cut)
            REFERENCES public.cash_anchors (anchor_id, tenant_id, venue_id, kind, as_of),
        CONSTRAINT cash_checkpoints_ids CHECK (
            tenant_id <> '00000000-0000-0000-0000-000000000000'::UUID
            AND sample_id <> '00000000-0000-0000-0000-000000000000'::UUID
            AND epoch_id <> '00000000-0000-0000-0000-000000000000'::UUID),
        CONSTRAINT cash_checkpoints_source CHECK (source IN ('scan', 'cutover', 'reanchor')),
        CONSTRAINT cash_checkpoints_anchor_pair CHECK ((source = 'scan') = (anchor_id IS NULL)),
        CONSTRAINT cash_checkpoints_observation CHECK (
            cut = interval_start AND u_ns >= 0 AND isfinite(interval_start) AND isfinite(interval_end)
            AND interval_start <= send_at AND send_at <= receive_at AND receive_at <= interval_end)
    );
END;
$migration46$;
