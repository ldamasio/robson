-- Installed only in a disposable SQLx test database. The real Spot writers
-- already serialize relevant changes on accounting_state before these hooks.
-- Starting from a verified complete backfill and granting authority are
-- outside this proof.

CREATE TYPE cancel_shadow_key AS (
    tenant_id uuid, venue_id text, account_scope text, symbol text,
    position_id uuid, target_id text
);

CREATE TYPE cancel_shadow_work AS (group_rows bigint, leg_rows bigint);

CREATE TABLE cancel_shadow_members (
    intent_id uuid PRIMARY KEY,
    tenant_id uuid NOT NULL, venue_id text NOT NULL, account_scope text NOT NULL,
    symbol text, position_id uuid, target_id text,
    pending boolean NOT NULL, conflict boolean NOT NULL
);

CREATE TABLE cancel_shadow_groups (
    tenant_id uuid NOT NULL, venue_id text NOT NULL, account_scope text NOT NULL,
    symbol text NOT NULL, position_id uuid NOT NULL, target_id text NOT NULL,
    pending_count bigint NOT NULL CHECK (pending_count >= 0),
    conflict_count bigint NOT NULL CHECK (conflict_count >= 0),
    terminal_mask boolean NOT NULL,
    PRIMARY KEY (tenant_id, venue_id, account_scope, symbol, position_id, target_id)
);

CREATE TABLE cancel_shadow_legs (
    tenant_id uuid NOT NULL, venue_id text NOT NULL, account_scope text NOT NULL,
    symbol text NOT NULL, position_id uuid NOT NULL,
    active_count bigint NOT NULL CHECK (active_count >= 0),
    conflict_count bigint NOT NULL CHECK (conflict_count >= 0),
    PRIMARY KEY (tenant_id, venue_id, account_scope, symbol, position_id)
);

CREATE TABLE cancel_shadow_terminal_work (
    work_id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    tenant_id uuid NOT NULL, venue_id text NOT NULL, account_scope text NOT NULL,
    symbol text NOT NULL, position_id uuid NOT NULL, target_id text NOT NULL,
    group_rows bigint NOT NULL, leg_rows bigint NOT NULL, member_rows bigint NOT NULL
);

CREATE TABLE cancel_shadow_fail_terminal (target_id text PRIMARY KEY);

-- Observe any physical member mutation, including an UPDATE that assigns the
-- old value. The transaction-local counter resets with the fixture write.
CREATE FUNCTION cancel_shadow_member_writes() RETURNS bigint LANGUAGE sql AS $$
    SELECT coalesce(nullif(current_setting('robson_test.cancel_shadow_member_writes', true), ''),
                    '0')::bigint;
$$;

CREATE FUNCTION cancel_shadow_observe_member() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    PERFORM set_config('robson_test.cancel_shadow_member_writes',
                       (cancel_shadow_member_writes() + 1)::text, true);
    RETURN NULL;
END;
$$;

CREATE TRIGGER cancel_shadow_observe_member AFTER INSERT OR UPDATE OR DELETE
    ON cancel_shadow_members FOR EACH ROW EXECUTE FUNCTION cancel_shadow_observe_member();

CREATE FUNCTION cancel_shadow_adjust(
    k cancel_shadow_key, pending_delta bigint, conflict_delta bigint, next_mask boolean
) RETURNS cancel_shadow_work LANGUAGE plpgsql AS $$
DECLARE
    prior cancel_shadow_groups%ROWTYPE;
    has_group boolean;
    group_rows bigint := 0;
    leg_rows bigint := 0;
    pending_next bigint;
    mask_next boolean;
    previous_active bigint;
    active_delta bigint;
BEGIN
    SELECT g.* INTO prior FROM cancel_shadow_groups g
    WHERE (g.tenant_id, g.venue_id, g.account_scope, g.symbol, g.position_id, g.target_id)
        = (k.tenant_id, k.venue_id, k.account_scope, k.symbol, k.position_id, k.target_id);
    has_group := FOUND;
    mask_next := coalesce(next_mask, prior.terminal_mask, false);
    IF pending_delta = 0 AND conflict_delta = 0
       AND mask_next = coalesce(prior.terminal_mask, false) THEN
        RETURN ROW(0, 0)::cancel_shadow_work;
    END IF;

    pending_next := coalesce(prior.pending_count, 0) + pending_delta;
    IF has_group THEN
        UPDATE cancel_shadow_groups g
        SET pending_count = pending_next,
            conflict_count = prior.conflict_count + conflict_delta, terminal_mask = mask_next
        WHERE (g.tenant_id, g.venue_id, g.account_scope, g.symbol, g.position_id, g.target_id)
            = (k.tenant_id, k.venue_id, k.account_scope, k.symbol, k.position_id, k.target_id);
    ELSE
        INSERT INTO cancel_shadow_groups
        VALUES (k.tenant_id, k.venue_id, k.account_scope, k.symbol, k.position_id,
                k.target_id, pending_next, conflict_delta, mask_next);
    END IF;
    GET DIAGNOSTICS group_rows = ROW_COUNT;

    previous_active := CASE WHEN prior.terminal_mask THEN 0 ELSE coalesce(prior.pending_count, 0) END;
    active_delta := (CASE WHEN mask_next THEN 0 ELSE pending_next END) - previous_active;
    IF active_delta <> 0 OR conflict_delta <> 0 THEN
        UPDATE cancel_shadow_legs l
        SET active_count = l.active_count + active_delta,
            conflict_count = l.conflict_count + conflict_delta
        WHERE (l.tenant_id, l.venue_id, l.account_scope, l.symbol, l.position_id)
            = (k.tenant_id, k.venue_id, k.account_scope, k.symbol, k.position_id);
        IF NOT FOUND THEN
            INSERT INTO cancel_shadow_legs
            VALUES (k.tenant_id, k.venue_id, k.account_scope, k.symbol, k.position_id,
                    active_delta, conflict_delta);
        END IF;
        GET DIAGNOSTICS leg_rows = ROW_COUNT;
    END IF;
    RETURN ROW(group_rows, leg_rows)::cancel_shadow_work;
END;
$$;

CREATE FUNCTION cancel_shadow_sync_member(i order_intents) RETURNS void LANGUAGE plpgsql AS $$
DECLARE
    prior cancel_shadow_members%ROWTYPE;
    k cancel_shadow_key;
    has_named_row boolean;
    is_pending boolean;
    is_conflict boolean;
BEGIN
    SELECT l.symbol INTO k.symbol FROM balance_legs l
    WHERE (l.position_id, l.tenant_id, l.venue_id, l.account_scope)
        = (i.position_id, i.tenant_id, i.venue_id, i.account_scope);
    k.tenant_id := i.tenant_id;
    k.venue_id := i.venue_id;
    k.account_scope := i.account_scope;
    k.position_id := i.position_id;
    IF ledger_canonical_id(i.cancels_order_id) THEN
        k.target_id := i.cancels_order_id;
    END IF;

    SELECT EXISTS (SELECT 1 FROM venue_orders o WHERE o.intent_id = i.intent_id)
        INTO has_named_row;
    is_conflict := has_named_row
        OR (i.not_sent AND (i.venue_outcome IS NOT NULL OR i.venue_evidence IS NOT NULL));
    is_pending := NOT i.not_sent OR is_conflict;
    SELECT m.* INTO prior FROM cancel_shadow_members m WHERE m.intent_id = i.intent_id;
    IF FOUND THEN
        IF (prior.tenant_id, prior.venue_id, prior.account_scope, prior.symbol,
            prior.position_id, prior.target_id) IS DISTINCT FROM
           (k.tenant_id, k.venue_id, k.account_scope, k.symbol, k.position_id, k.target_id) THEN
            RAISE EXCEPTION 'test shadow requires a stable cancel identity';
        END IF;
        IF (prior.pending, prior.conflict) = (is_pending, is_conflict) THEN
            RETURN;
        END IF;
    END IF;
    INSERT INTO cancel_shadow_members
    VALUES (i.intent_id, k.tenant_id, k.venue_id, k.account_scope, k.symbol,
            k.position_id, k.target_id, is_pending, is_conflict)
    ON CONFLICT (intent_id) DO UPDATE
    SET pending = excluded.pending, conflict = excluded.conflict;

    -- Undefined scope remains an explicit uncertainty in the member row.
    IF k.position_id IS NOT NULL AND k.symbol IS NOT NULL AND k.target_id IS NOT NULL THEN
        PERFORM cancel_shadow_adjust(k,
            is_pending::integer - coalesce(prior.pending::integer, 0),
            is_conflict::integer - coalesce(prior.conflict::integer, 0), NULL);
    END IF;
END;
$$;

CREATE FUNCTION cancel_shadow_sync_target(
    o venue_orders, observe_native boolean
) RETURNS void LANGUAGE plpgsql AS $$
DECLARE
    j order_intents%ROWTYPE;
    k cancel_shadow_key;
    terminal_proof boolean;
    touched cancel_shadow_work;
    previous_member_writes bigint;
BEGIN
    IF o.venue_id <> 'binance:spot' OR o.owner <> 'leg' THEN
        RETURN;
    END IF;
    previous_member_writes := cancel_shadow_member_writes();
    SELECT i.* INTO j FROM order_intents i WHERE i.intent_id = o.intent_id;
    IF FOUND AND j.kind = 'cancel' THEN
        PERFORM cancel_shadow_sync_member(j);
    END IF;

    k := ROW(o.tenant_id, o.venue_id, o.account_scope, o.symbol, o.position_id,
             o.exchange_order_id)::cancel_shadow_key;
    terminal_proof := coalesce(
        o.phase = 'terminal'
        AND o.status IN ('FILLED', 'CANCELED', 'EXPIRED', 'EXPIRED_IN_MATCH', 'REJECTED')
        AND (j.tenant_id, j.venue_id, j.account_scope, j.position_id, j.client_order_id)
            = (o.tenant_id, o.venue_id, o.account_scope, o.position_id, o.client_order_id)
        AND NOT j.not_sent
        AND o.kind = CASE j.kind WHEN 'entry' THEN 'entry'
                                 WHEN 'protection' THEN 'stop' WHEN 'exit' THEN 'sell' END,
        false);
    touched := cancel_shadow_adjust(k, 0, 0, terminal_proof);

    IF observe_native AND terminal_proof AND touched.group_rows <> 0 THEN
        -- A qualified order's J kind is not cancel. This branch performs no
        -- member mutation and never enumerates siblings, including for a mask.
        INSERT INTO cancel_shadow_terminal_work
            (tenant_id, venue_id, account_scope, symbol, position_id, target_id,
             group_rows, leg_rows, member_rows)
        VALUES (k.tenant_id, k.venue_id, k.account_scope, k.symbol, k.position_id,
                k.target_id, touched.group_rows, touched.leg_rows,
                cancel_shadow_member_writes() - previous_member_writes);
        IF EXISTS (SELECT 1 FROM cancel_shadow_fail_terminal f WHERE f.target_id = k.target_id) THEN
            RAISE EXCEPTION 'cancel shadow terminal rollback witness';
        END IF;
    END IF;
END;
$$;

CREATE FUNCTION cancel_shadow_intent_changed() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE
    o venue_orders%ROWTYPE;
BEGIN
    IF NEW.venue_id <> 'binance:spot' THEN
        RETURN NULL;
    END IF;
    IF TG_OP = 'UPDATE' AND (NEW.not_sent, NEW.venue_outcome, NEW.venue_evidence)
                        IS NOT DISTINCT FROM (OLD.not_sent, OLD.venue_outcome, OLD.venue_evidence)
    THEN
        RETURN NULL;
    END IF;

    IF NEW.kind = 'cancel' THEN
        PERFORM cancel_shadow_sync_member(NEW);
    ELSE
        SELECT v.* INTO o FROM venue_orders v WHERE v.intent_id = NEW.intent_id;
        IF FOUND THEN
            PERFORM cancel_shadow_sync_target(o, false);
        END IF;
    END IF;
    RETURN NULL;
END;
$$;

CREATE TRIGGER cancel_shadow_intent AFTER INSERT OR UPDATE OF not_sent, venue_outcome, venue_evidence
    ON order_intents FOR EACH ROW EXECUTE FUNCTION cancel_shadow_intent_changed();

CREATE FUNCTION cancel_shadow_venue_changed() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'UPDATE' AND (NEW.phase, NEW.status, NEW.owner, NEW.intent_id, NEW.position_id, NEW.kind)
                        IS NOT DISTINCT FROM (OLD.phase, OLD.status, OLD.owner, OLD.intent_id,
                                              OLD.position_id, OLD.kind)
    THEN
        RETURN NULL;
    END IF;
    PERFORM cancel_shadow_sync_target(NEW, true);
    RETURN NULL;
END;
$$;

CREATE TRIGGER cancel_shadow_venue AFTER INSERT OR UPDATE OF phase, status, owner, intent_id,
    position_id, kind ON venue_orders FOR EACH ROW EXECUTE FUNCTION cancel_shadow_venue_changed();
