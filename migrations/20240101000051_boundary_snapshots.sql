DO $migration51$
BEGIN
    -- ADR-0056 step 8p-1, S8-5 (1 of 4): the boundary snapshot of a balance venue's month K, taken as of
    -- B = the first instant of K in UTC, and its durable reopenings. Additive and inert: no writer exists in
    -- this slice. A snapshot starts pending (8p-2), completes revision n (8p-5), records its reset once and
    -- advances its publication pointer (8p-6), and is reopened by late evidence or an impaired E(B) (8p-7).
    CREATE FUNCTION public.boundary_snapshot_moves_forward() RETURNS TRIGGER
        LANGUAGE plpgsql SET search_path = pg_catalog, public AS $snapshot_guard$
    BEGIN
        IF TG_OP = 'DELETE' THEN
            RAISE EXCEPTION 'ledger invariant: boundary snapshots are never deleted' USING ERRCODE = '23514';
        END IF;
        IF TG_OP = 'INSERT' THEN
            IF NEW.state IS DISTINCT FROM 'pending' OR NEW.reopen_count IS DISTINCT FROM 0 THEN
                RAISE EXCEPTION 'ledger invariant: a boundary snapshot starts pending' USING ERRCODE = '23514';
            END IF;
            RETURN NEW;
        END IF;
        IF (NEW.tenant_id, NEW.venue_id, NEW.month, NEW.as_of, NEW.pending_since)
               IS DISTINCT FROM (OLD.tenant_id, OLD.venue_id, OLD.month, OLD.as_of, OLD.pending_since)
           OR NOT COALESCE(CASE
               -- A lifecycle step: revision 1 completes, a complete revision reopens (reopen_count + 1), or a
               -- reopened one completes revision n + 1, its effective basis never rising; nothing published or
               -- corrected moves with it.
               WHEN (NEW.state, NEW.revision, NEW.reopen_count)
                    IS DISTINCT FROM (OLD.state, OLD.revision, OLD.reopen_count)
               THEN ((OLD.state = 'pending' AND NEW.state = 'complete' AND NEW.revision = 1
                      AND NEW.reopen_count = OLD.reopen_count)
                     OR (OLD.state = 'complete' AND NEW.state = 'reopened' AND NEW.revision = OLD.revision
                         AND NEW.reopen_count = OLD.reopen_count + 1
                         AND (NEW.effective_basis, NEW.completed_at)
                             IS NOT DISTINCT FROM (OLD.effective_basis, OLD.completed_at))
                     OR (OLD.state = 'reopened' AND NEW.state = 'complete' AND NEW.revision = OLD.revision + 1
                         AND NEW.reopen_count = OLD.reopen_count
                         AND NEW.effective_basis <= OLD.effective_basis
                         AND NEW.completed_at >= OLD.completed_at))
                    AND (NEW.reset_revision, NEW.reset_basis, NEW.published_revision, NEW.published_basis,
                         NEW.corrected_at, NEW.closed_by_correction_at)
                        IS NOT DISTINCT FROM
                        (OLD.reset_revision, OLD.reset_basis, OLD.published_revision, OLD.published_basis,
                         OLD.corrected_at, OLD.closed_by_correction_at)
               -- Otherwise only a complete row changes, and only (i) its reset, recorded once at the current
               -- revision as max(0, effective) with the pointer equal to it; (ii) its pointer, advanced to the
               -- current revision at max(0, effective), never above the previous one; (iii) its first correction
               -- and its closure latch, each set once.
               ELSE NEW IS NOT DISTINCT FROM OLD
                    OR (OLD.state = 'complete'
                        AND (NEW.effective_basis, NEW.completed_at)
                            IS NOT DISTINCT FROM (OLD.effective_basis, OLD.completed_at)
                        AND ((NEW.reset_revision, NEW.reset_basis)
                                 IS NOT DISTINCT FROM (OLD.reset_revision, OLD.reset_basis)
                             OR (OLD.reset_revision IS NULL AND NEW.reset_revision = NEW.revision
                                 AND NEW.reset_basis = GREATEST(NEW.effective_basis, 0)
                                 AND (NEW.published_revision, NEW.published_basis)
                                     IS NOT DISTINCT FROM (NEW.reset_revision, NEW.reset_basis)))
                        AND (OLD.reset_revision IS NULL
                             OR (NEW.published_revision, NEW.published_basis)
                                 IS NOT DISTINCT FROM (OLD.published_revision, OLD.published_basis)
                             OR (NEW.published_revision = NEW.revision
                                 AND NEW.published_revision > OLD.published_revision
                                 AND NEW.published_basis = GREATEST(NEW.effective_basis, 0)
                                 AND NEW.published_basis <= OLD.published_basis))
                        AND (OLD.corrected_at IS NULL OR NEW.corrected_at IS NOT DISTINCT FROM OLD.corrected_at)
                        AND (OLD.closed_by_correction_at IS NULL
                             OR NEW.closed_by_correction_at IS NOT DISTINCT FROM OLD.closed_by_correction_at))
               END, FALSE)
        THEN
            RAISE EXCEPTION 'ledger invariant: boundary snapshot % % only moves forward', OLD.venue_id, OLD.month
                USING ERRCODE = '23514';
        END IF;
        RETURN NEW;
    END;
    $snapshot_guard$;

    -- effective_basis = the minimum basis over the revisions; reset_* = the halt basis, recorded once by the
    -- reset; published_* = the latest reset or recalibration appended, which monthly_state must reflect.
    CREATE TABLE public.boundary_snapshots (
        tenant_id UUID NOT NULL,
        venue_id TEXT NOT NULL,
        month DATE NOT NULL,
        as_of TIMESTAMPTZ NOT NULL,
        state TEXT NOT NULL,
        revision INTEGER NOT NULL,
        reopen_count INTEGER NOT NULL,
        effective_basis NUMERIC,
        reset_revision INTEGER,
        reset_basis NUMERIC,
        published_revision INTEGER,
        published_basis NUMERIC,
        pending_since TIMESTAMPTZ NOT NULL,
        completed_at TIMESTAMPTZ,
        corrected_at TIMESTAMPTZ,
        closed_by_correction_at TIMESTAMPTZ,
        CONSTRAINT boundary_snapshots_key PRIMARY KEY (tenant_id, venue_id, month),
        CONSTRAINT boundary_snapshots_version_row FOREIGN KEY (tenant_id, venue_id)
            REFERENCES public.accounting_state (tenant_id, venue_id),
        CONSTRAINT boundary_snapshots_ids CHECK (tenant_id <> '00000000-0000-0000-0000-000000000000'::UUID),
        CONSTRAINT boundary_snapshots_month CHECK (EXTRACT(DAY FROM month) = 1),
        CONSTRAINT boundary_snapshots_as_of CHECK (as_of = (month::TIMESTAMP AT TIME ZONE 'UTC')),
        CONSTRAINT boundary_snapshots_state CHECK (state IN ('pending', 'complete', 'reopened')),
        CONSTRAINT boundary_snapshots_revision CHECK (
            revision >= 0 AND reopen_count >= 0 AND (state = 'pending') = (revision = 0)
            AND (revision = 0) = (effective_basis IS NULL) AND (revision = 0) = (completed_at IS NULL)),
        CONSTRAINT boundary_snapshots_amounts CHECK (
            ledger_amount_fits(effective_basis) AND ledger_amount_fits(reset_basis)
            AND ledger_amount_fits(published_basis)),
        CONSTRAINT boundary_snapshots_publication CHECK (
            num_nulls(reset_revision, reset_basis, published_revision, published_basis) IN (0, 4)
            AND reset_revision BETWEEN 1 AND revision AND published_revision BETWEEN reset_revision AND revision
            AND reset_basis >= 0 AND published_basis >= 0 AND published_basis <= reset_basis),
        -- The first correction publishes below the halt basis; the latch comes only with a correction.
        CONSTRAINT boundary_snapshots_correction CHECK (
            (corrected_at IS NULL OR (reset_basis IS NOT NULL AND published_basis < reset_basis))
            AND (closed_by_correction_at IS NULL OR corrected_at IS NOT NULL))
    );

    CREATE TRIGGER boundary_snapshots_move_forward BEFORE INSERT OR UPDATE OR DELETE ON public.boundary_snapshots
        FOR EACH ROW EXECUTE FUNCTION public.boundary_snapshot_moves_forward();

    -- One row per reopening, with the reopen_count it produced (reopen_seq) and its cause: the input digest
    -- (d_fills, d_flows, d_legs) of the revision against the observed one, or an impaired E(B) (the impairing
    -- anchor and fill in evidence_ids, which is audit only). Immutable from migration 52 on.
    CREATE TABLE public.boundary_reopenings (
        tenant_id UUID NOT NULL,
        venue_id TEXT NOT NULL,
        month DATE NOT NULL,
        reopen_seq INTEGER NOT NULL,
        from_revision INTEGER NOT NULL,
        detected_at TIMESTAMPTZ NOT NULL,
        cause TEXT NOT NULL,
        revision_d_fills BIGINT NOT NULL,
        revision_d_flows BIGINT NOT NULL,
        revision_d_legs BIGINT NOT NULL,
        observed_d_fills BIGINT NOT NULL,
        observed_d_flows BIGINT NOT NULL,
        observed_d_legs BIGINT NOT NULL,
        evidence_ids JSONB NOT NULL,
        CONSTRAINT boundary_reopenings_key PRIMARY KEY (tenant_id, venue_id, month, reopen_seq),
        CONSTRAINT boundary_reopenings_snapshot FOREIGN KEY (tenant_id, venue_id, month)
            REFERENCES public.boundary_snapshots (tenant_id, venue_id, month),
        CONSTRAINT boundary_reopenings_version_row FOREIGN KEY (tenant_id, venue_id)
            REFERENCES public.accounting_state (tenant_id, venue_id),
        CONSTRAINT boundary_reopenings_ids CHECK (tenant_id <> '00000000-0000-0000-0000-000000000000'::UUID),
        CONSTRAINT boundary_reopenings_seq CHECK (reopen_seq >= 1 AND from_revision >= 1),
        CONSTRAINT boundary_reopenings_cause CHECK (cause IN ('inputs_changed', 'impaired')),
        CONSTRAINT boundary_reopenings_digests CHECK (
            revision_d_fills >= 0 AND revision_d_flows >= 0 AND revision_d_legs >= 0
            AND observed_d_fills >= 0 AND observed_d_flows >= 0 AND observed_d_legs >= 0),
        CONSTRAINT boundary_reopenings_changed CHECK (
            cause <> 'inputs_changed'
            OR (revision_d_fills, revision_d_flows, revision_d_legs)
               IS DISTINCT FROM (observed_d_fills, observed_d_flows, observed_d_legs)),
        CONSTRAINT boundary_reopenings_evidence CHECK (
            CASE WHEN jsonb_typeof(evidence_ids) = 'array' THEN jsonb_array_length(evidence_ids) <= 64 ELSE FALSE END)
    );
END;
$migration51$;
