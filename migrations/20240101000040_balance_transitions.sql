DO $migration40$
BEGIN
    CREATE TABLE balance_transitions (
        transition_id UUID PRIMARY KEY,
        position_id UUID NOT NULL,
        tenant_id UUID NOT NULL,
        venue_id TEXT NOT NULL,
        account_scope TEXT NOT NULL,
        symbol TEXT NOT NULL,
        kind TEXT NOT NULL,
        target_trigger NUMERIC NOT NULL,
        reason TEXT NOT NULL,
        started_at TIMESTAMPTZ NOT NULL,
        completed_at TIMESTAMPTZ,
        outcome TEXT,
        first_cancel_intent_id UUID,
        original_target_order_id TEXT,
        CONSTRAINT balance_transitions_kind CHECK (kind IN ('replace', 'reprotect', 'flatten', 'exit')),
        CONSTRAINT balance_transitions_target CHECK (ledger_amount_fits(target_trigger) AND target_trigger > 0),
        CONSTRAINT balance_transitions_reason CHECK (btrim(reason) <> ''),
        CONSTRAINT balance_transitions_completion_pair CHECK ((completed_at IS NULL) = (outcome IS NULL)),
        CONSTRAINT balance_transitions_first_cancel_pair
            CHECK ((first_cancel_intent_id IS NULL) = (original_target_order_id IS NULL)),
        CONSTRAINT balance_transitions_leg FOREIGN KEY (position_id, tenant_id, venue_id, account_scope, symbol)
            REFERENCES balance_legs (position_id, tenant_id, venue_id, account_scope, symbol),
        CONSTRAINT balance_transitions_first_cancel FOREIGN KEY (first_cancel_intent_id)
            REFERENCES order_intents (intent_id),
        CONSTRAINT balance_transitions_original_order FOREIGN KEY (venue_id, account_scope, symbol, original_target_order_id)
            REFERENCES venue_orders (venue_id, account_scope, symbol, exchange_order_id)
    );

    CREATE UNIQUE INDEX idx_balance_transitions_open ON balance_transitions (position_id)
        WHERE completed_at IS NULL;

    CREATE FUNCTION balance_transitions_move_forward() RETURNS TRIGGER
        LANGUAGE plpgsql SET search_path = pg_catalog, public AS $transition_guard$
    BEGIN
        IF TG_OP = 'DELETE' THEN
            RAISE EXCEPTION 'ledger invariant: transition rows cannot be deleted' USING ERRCODE = '23514';
        END IF;
        IF TG_OP = 'UPDATE' THEN
            IF (NEW.transition_id, NEW.position_id, NEW.tenant_id, NEW.venue_id, NEW.account_scope,
                NEW.symbol, NEW.started_at) IS DISTINCT FROM
               (OLD.transition_id, OLD.position_id, OLD.tenant_id, OLD.venue_id, OLD.account_scope,
                OLD.symbol, OLD.started_at)
               OR OLD.completed_at IS NOT NULL
               OR NEW.target_trigger < OLD.target_trigger
               OR (NEW.kind = OLD.kind AND NEW.reason IS DISTINCT FROM OLD.reason)
               OR (NEW.kind <> OLD.kind AND NOT (
                   NEW.kind = 'exit' OR (NEW.kind = 'flatten' AND OLD.kind IN ('replace', 'reprotect'))
               ))
               OR (OLD.first_cancel_intent_id IS NOT NULL AND
                   (NEW.first_cancel_intent_id, NEW.original_target_order_id) IS DISTINCT FROM
                   (OLD.first_cancel_intent_id, OLD.original_target_order_id))
            THEN
                RAISE EXCEPTION 'ledger invariant: transition may only move forward' USING ERRCODE = '23514';
            END IF;
        END IF;

        IF NEW.first_cancel_intent_id IS NOT NULL AND NEW.original_target_order_id IS NOT NULL THEN
            PERFORM 1
              FROM public.order_intents AS i
              JOIN public.venue_orders AS o
                ON o.venue_id = i.venue_id
               AND o.account_scope = i.account_scope
               AND o.exchange_order_id = i.cancels_order_id
             WHERE i.intent_id = NEW.first_cancel_intent_id
               AND i.kind = 'cancel'
               AND i.tenant_id = NEW.tenant_id
               AND i.venue_id = NEW.venue_id
               AND i.account_scope = NEW.account_scope
               AND i.position_id = NEW.position_id
               AND i.cancels_order_id = NEW.original_target_order_id
               AND o.symbol = NEW.symbol
               AND o.tenant_id = NEW.tenant_id
               AND o.position_id = NEW.position_id
               AND o.owner = 'leg'
               AND o.kind = 'stop'
               FOR SHARE OF i, o;
            IF NOT FOUND THEN
                RAISE EXCEPTION 'ledger invariant: first cancel must target the transition leg''s original stop'
                    USING ERRCODE = '23514';
            END IF;
        END IF;
        RETURN NEW;
    END;
    $transition_guard$;

    CREATE TRIGGER balance_transitions_move_forward BEFORE INSERT OR UPDATE OR DELETE ON balance_transitions
        FOR EACH ROW EXECUTE FUNCTION balance_transitions_move_forward();
END;
$migration40$;
