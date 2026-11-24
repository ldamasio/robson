DO $migration55$
BEGIN
    -- ADR-0056 step 8m-1, S8-6 (1 of 2): the runs of the cutover certificate. tx1 inserts a run pending with the
    -- report, its hash, the generation and every value tx2 and completion rebuild from; a run ends once, complete
    -- (completion, with the audited observations) or abandoned (with its reason), and never changes again.
    -- Writing it bumps no version: tx1 and abandonment write only this table, and completion's write is covered
    -- by completion's own conditional bump. At most one run per venue is complete. Migration 56 adds the
    -- one-pending index and the cutover_id foreign keys of venue_accounts, cash_anchors and activation_bases.
    CREATE FUNCTION public.venue_cutover_moves_forward() RETURNS TRIGGER
        LANGUAGE plpgsql SET search_path = pg_catalog, public AS $cutover_guard$
    BEGIN
        IF TG_OP = 'DELETE' THEN
            RAISE EXCEPTION 'ledger invariant: venue cutovers are never deleted' USING ERRCODE = '23514';
        END IF;
        IF TG_OP = 'INSERT' THEN
            IF NEW.state IS DISTINCT FROM 'pending' THEN
                RAISE EXCEPTION 'ledger invariant: a venue cutover starts pending' USING ERRCODE = '23514';
            END IF;
            RETURN NEW;
        END IF;
        -- What tx1 wrote never changes; amounts and documents compare as text, so a rescaled amount or a
        -- renumbered document field is a change. Only a pending run ends (the CHECKs give its two ends), and an
        -- ended run never changes.
        IF (NEW.cutover_id, NEW.tenant_id, NEW.venue_id, NEW.account_scope, NEW.month, NEW.uid, NEW.report::TEXT,
            NEW.report_hash, NEW.generation::TEXT, NEW.generation_hash, NEW.v1, NEW.budget_model,
            NEW.previous_basis::TEXT, NEW.quote_total::TEXT, NEW.basis::TEXT, NEW.command_id, NEW.epoch_id,
            NEW.created_at)
               IS DISTINCT FROM
           (OLD.cutover_id, OLD.tenant_id, OLD.venue_id, OLD.account_scope, OLD.month, OLD.uid, OLD.report::TEXT,
            OLD.report_hash, OLD.generation::TEXT, OLD.generation_hash, OLD.v1, OLD.budget_model,
            OLD.previous_basis::TEXT, OLD.quote_total::TEXT, OLD.basis::TEXT, OLD.command_id, OLD.epoch_id,
            OLD.created_at)
           OR (OLD.state <> 'pending' AND
               (NEW.state, NEW.completed_at, NEW.observations::TEXT, NEW.abandoned_at, NEW.abandon_reason)
                   IS DISTINCT FROM
               (OLD.state, OLD.completed_at, OLD.observations::TEXT, OLD.abandoned_at, OLD.abandon_reason))
        THEN
            RAISE EXCEPTION 'ledger invariant: venue cutover % only moves forward', OLD.cutover_id
                USING ERRCODE = '23514';
        END IF;
        RETURN NEW;
    END;
    $cutover_guard$;

    CREATE TABLE public.venue_cutovers (
        cutover_id UUID NOT NULL,
        tenant_id UUID NOT NULL,
        venue_id TEXT NOT NULL,
        account_scope TEXT NOT NULL,
        state TEXT NOT NULL,
        month DATE NOT NULL,
        uid TEXT NOT NULL,
        report JSONB NOT NULL,
        report_hash TEXT NOT NULL,
        generation JSONB NOT NULL,
        generation_hash TEXT NOT NULL,
        v1 BIGINT NOT NULL,
        budget_model TEXT NOT NULL,
        previous_basis NUMERIC NOT NULL,
        quote_total NUMERIC NOT NULL,
        basis NUMERIC NOT NULL,
        command_id UUID NOT NULL,
        epoch_id UUID NOT NULL,
        created_at TIMESTAMPTZ NOT NULL,
        completed_at TIMESTAMPTZ,
        observations JSONB,
        abandoned_at TIMESTAMPTZ,
        abandon_reason TEXT,
        CONSTRAINT venue_cutovers_key PRIMARY KEY (cutover_id),
        -- The target of the cutover_id foreign keys (migration 56).
        CONSTRAINT venue_cutovers_scope UNIQUE (tenant_id, venue_id, cutover_id),
        CONSTRAINT venue_cutovers_version_row FOREIGN KEY (tenant_id, venue_id)
            REFERENCES public.accounting_state (tenant_id, venue_id),
        CONSTRAINT venue_cutovers_ids CHECK (
            cutover_id <> '00000000-0000-0000-0000-000000000000'::UUID
            AND tenant_id <> '00000000-0000-0000-0000-000000000000'::UUID
            AND command_id <> '00000000-0000-0000-0000-000000000000'::UUID
            AND epoch_id <> '00000000-0000-0000-0000-000000000000'::UUID),
        CONSTRAINT venue_cutovers_account CHECK (account_scope <> '' AND uid <> ''),
        -- IS TRUE: an infinite date has no day, and a NULL CHECK would pass.
        CONSTRAINT venue_cutovers_month CHECK ((EXTRACT(DAY FROM month) = 1) IS TRUE),
        CONSTRAINT venue_cutovers_report CHECK (jsonb_typeof(report) = 'object' AND report_hash ~ '^[0-9a-f]{64}$'),
        -- The generation of tx1 (8m PLAN §6.3), with its schema version.
        CONSTRAINT venue_cutovers_generation CHECK (
            COALESCE(jsonb_typeof(generation -> 'schema_version') = 'number'
                     AND (generation ->> 'schema_version') ~ '^[1-9][0-9]{0,8}$', FALSE)
            AND generation_hash ~ '^[0-9a-f]{64}$'),
        CONSTRAINT venue_cutovers_v1 CHECK (v1 >= 0),
        CONSTRAINT venue_cutovers_budget_model CHECK (budget_model = 'net_from_start_non_expanding_v1'),
        CONSTRAINT venue_cutovers_amounts CHECK (
            ledger_amount_fits(previous_basis) AND ledger_amount_fits(quote_total) AND ledger_amount_fits(basis)
            AND previous_basis >= 0 AND quote_total >= 0),
        CONSTRAINT venue_cutovers_basis CHECK (basis = LEAST(previous_basis, quote_total)),
        CONSTRAINT venue_cutovers_state CHECK (state IN ('pending', 'complete', 'abandoned')),
        CONSTRAINT venue_cutovers_complete CHECK (
            (state = 'complete') = (completed_at IS NOT NULL) AND (completed_at IS NULL) = (observations IS NULL)),
        CONSTRAINT venue_cutovers_abandoned CHECK (
            (state = 'abandoned') = (abandoned_at IS NOT NULL)
            AND (abandoned_at IS NULL) = (abandon_reason IS NULL)),
        CONSTRAINT venue_cutovers_abandon_reason CHECK (abandon_reason IN (
            'check_failed', 'report_changed', 'generation_changed', 'observation_failed', 'observations_disagree',
            'clock_invalid', 'discrepancy_open', 'month_changed', 'version_mismatch', 'timeout', 'operator'))
    );

    CREATE TRIGGER venue_cutovers_move_forward BEFORE INSERT OR UPDATE OR DELETE ON public.venue_cutovers
        FOR EACH ROW EXECUTE FUNCTION public.venue_cutover_moves_forward();

    -- One certificate per venue (v16:1666-1667).
    CREATE UNIQUE INDEX idx_venue_cutovers_complete ON public.venue_cutovers (tenant_id, venue_id)
        WHERE state = 'complete';
END;
$migration55$;
