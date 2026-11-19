-- Installed only in a disposable SQLx test database. The real Spot writers
-- already serialize relevant changes on accounting_state before these hooks.
-- Starting from a verified complete backfill and granting authority are
-- outside this proof. Hooks maintain membership only: quantities stay on the
-- current balance_legs row and are read there in the same snapshot.
--
-- One logical member per exact linked J/native pair. While a native row
-- exists for an exact pair, the member lives on its venue key. A mismatched
-- pair keeps the unresolved journal intent and the native conflict in their
-- respective scopes. Every probe is keyed: at most one J (PK or client key),
-- one native row (PK or idx_venue_orders_intent), one leg (PK), one attempt
-- (the M37 partial unique key) and the touched member rows.
--
-- Uncovered here: a J written before its leg row stays in sell_shadow_unbound
-- until its next journal, attempt or native event; sweep_close is not a
-- suspected SELL kind; J identity columns are assumed immutable.

CREATE TYPE sell_shadow_class AS (
    scope_kind text, tenant_id uuid, venue_id text, account_scope text,
    symbol text, position_id uuid, potential boolean, conflict boolean,
    branch text, probes integer
);

CREATE TABLE sell_shadow_intents (
    intent_id uuid PRIMARY KEY,
    scope_kind text NOT NULL CHECK (scope_kind IN ('leg', 'symbol', 'unbound', 'none')),
    scope_tenant uuid, scope_venue text, scope_account text, scope_symbol text, scope_position uuid,
    potential boolean NOT NULL, conflict boolean NOT NULL
);

CREATE TABLE sell_shadow_orders (
    tenant_id uuid NOT NULL, venue_id text NOT NULL, account_scope text NOT NULL,
    symbol text NOT NULL, exchange_order_id text NOT NULL,
    scope_kind text NOT NULL CHECK (scope_kind IN ('leg', 'symbol', 'unbound', 'none')),
    scope_tenant uuid, scope_venue text, scope_account text, scope_symbol text, scope_position uuid,
    potential boolean NOT NULL, conflict boolean NOT NULL,
    PRIMARY KEY (tenant_id, venue_id, account_scope, symbol, exchange_order_id)
);

CREATE TABLE sell_shadow_legs (
    tenant_id uuid NOT NULL, venue_id text NOT NULL, account_scope text NOT NULL,
    symbol text NOT NULL, position_id uuid NOT NULL,
    potential_sell bigint NOT NULL CHECK (potential_sell >= 0),
    conflict_count bigint NOT NULL CHECK (conflict_count >= 0),
    PRIMARY KEY (tenant_id, venue_id, account_scope, symbol, position_id)
);

-- Native SELL rows without an owning leg (kind NULL included). No owning
-- position is manufactured for them.
CREATE TABLE sell_shadow_symbols (
    tenant_id uuid NOT NULL, venue_id text NOT NULL, account_scope text NOT NULL,
    symbol text NOT NULL,
    potential_sell bigint NOT NULL CHECK (potential_sell >= 0),
    conflict_count bigint NOT NULL CHECK (conflict_count >= 0),
    PRIMARY KEY (tenant_id, venue_id, account_scope, symbol)
);

-- A journal SELL without a resolvable leg. bounded_order_set.sql does not
-- read this namespace; a nonzero row is a named uncovered route.
CREATE TABLE sell_shadow_unbound (
    tenant_id uuid NOT NULL, venue_id text NOT NULL, account_scope text NOT NULL,
    potential_sell bigint NOT NULL CHECK (potential_sell >= 0),
    conflict_count bigint NOT NULL CHECK (conflict_count >= 0),
    PRIMARY KEY (tenant_id, venue_id, account_scope)
);

-- Written only when robson_test.sell_shadow_trace = 'on' (set per database or
-- role in the disposable fixture). robson_test.sell_shadow_label names direct
-- fixture-history inserts.
CREATE TABLE sell_shadow_work (
    work_id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    source text NOT NULL, label text, branch text NOT NULL,
    intent_id uuid, exchange_order_id text,
    probes integer NOT NULL, member_rows integer NOT NULL, aggregate_rows integer NOT NULL
);

CREATE TABLE sell_shadow_fail_member (member_key text PRIMARY KEY);

CREATE FUNCTION sell_shadow_exact_link(j order_intents, o venue_orders)
RETURNS boolean LANGUAGE sql IMMUTABLE AS $$
    SELECT coalesce(o.intent_id = j.intent_id AND o.owner = 'leg' AND o.side = 'SELL'
        AND (j.tenant_id, j.venue_id, j.account_scope, j.position_id, j.client_order_id)
            = (o.tenant_id, o.venue_id, o.account_scope, o.position_id, o.client_order_id)
        AND o.kind = CASE j.kind WHEN 'protection' THEN 'stop' WHEN 'exit' THEN 'sell' END,
        false);
$$;

CREATE FUNCTION sell_shadow_bump(
    b_kind text, b_tenant uuid, b_venue text, b_account text, b_symbol text, b_position uuid,
    d_potential integer, d_conflict integer
) RETURNS integer LANGUAGE plpgsql AS $$
DECLARE
    n integer := 0;
BEGIN
    IF b_kind = 'none' OR (d_potential = 0 AND d_conflict = 0) THEN
        RETURN 0;
    END IF;
    IF b_kind = 'leg' THEN
        UPDATE sell_shadow_legs AS g
        SET potential_sell = g.potential_sell + d_potential,
            conflict_count = g.conflict_count + d_conflict
        WHERE (g.tenant_id, g.venue_id, g.account_scope, g.symbol, g.position_id)
            = (b_tenant, b_venue, b_account, b_symbol, b_position);
        IF NOT FOUND THEN
            INSERT INTO sell_shadow_legs
            VALUES (b_tenant, b_venue, b_account, b_symbol, b_position, d_potential, d_conflict);
        END IF;
    ELSIF b_kind = 'symbol' THEN
        UPDATE sell_shadow_symbols AS g
        SET potential_sell = g.potential_sell + d_potential,
            conflict_count = g.conflict_count + d_conflict
        WHERE (g.tenant_id, g.venue_id, g.account_scope, g.symbol)
            = (b_tenant, b_venue, b_account, b_symbol);
        IF NOT FOUND THEN
            INSERT INTO sell_shadow_symbols
            VALUES (b_tenant, b_venue, b_account, b_symbol, d_potential, d_conflict);
        END IF;
    ELSE
        UPDATE sell_shadow_unbound AS g
        SET potential_sell = g.potential_sell + d_potential,
            conflict_count = g.conflict_count + d_conflict
        WHERE (g.tenant_id, g.venue_id, g.account_scope) = (b_tenant, b_venue, b_account);
        IF NOT FOUND THEN
            INSERT INTO sell_shadow_unbound
            VALUES (b_tenant, b_venue, b_account, d_potential, d_conflict);
        END IF;
    END IF;
    GET DIAGNOSTICS n = ROW_COUNT;
    RETURN n;
END;
$$;

-- Subtract the old contribution and add the new one in at most two exact keys.
CREATE FUNCTION sell_shadow_move(
    o_kind text, o_tenant uuid, o_venue text, o_account text, o_symbol text, o_position uuid,
    o_potential boolean, o_conflict boolean, c sell_shadow_class
) RETURNS integer LANGUAGE plpgsql AS $$
BEGIN
    IF (o_kind, o_tenant, o_venue, o_account, o_symbol, o_position) IS NOT DISTINCT FROM
       (c.scope_kind, c.tenant_id, c.venue_id, c.account_scope, c.symbol, c.position_id) THEN
        RETURN sell_shadow_bump(c.scope_kind, c.tenant_id, c.venue_id, c.account_scope, c.symbol,
                                c.position_id, c.potential::integer - o_potential::integer,
                                c.conflict::integer - o_conflict::integer);
    END IF;
    RETURN sell_shadow_bump(o_kind, o_tenant, o_venue, o_account, o_symbol, o_position,
                            -o_potential::integer, -o_conflict::integer)
         + sell_shadow_bump(c.scope_kind, c.tenant_id, c.venue_id, c.account_scope, c.symbol,
                            c.position_id, c.potential::integer, c.conflict::integer);
END;
$$;

CREATE FUNCTION sell_shadow_set_intent(
    key_id uuid, c sell_shadow_class, OUT n_members integer, OUT n_aggregates integer
) LANGUAGE plpgsql AS $$
DECLARE
    prior sell_shadow_intents%ROWTYPE;
BEGIN
    n_members := 0;
    n_aggregates := 0;
    SELECT m.* INTO prior FROM sell_shadow_intents m WHERE m.intent_id = key_id;
    IF NOT FOUND THEN
        IF NOT (c.potential OR c.conflict) THEN
            RETURN;
        END IF;
    ELSIF (prior.scope_kind, prior.scope_tenant, prior.scope_venue, prior.scope_account,
           prior.scope_symbol, prior.scope_position, prior.potential, prior.conflict)
          IS NOT DISTINCT FROM
          (c.scope_kind, c.tenant_id, c.venue_id, c.account_scope, c.symbol, c.position_id,
           c.potential, c.conflict) THEN
        RETURN;
    END IF;
    INSERT INTO sell_shadow_intents
    VALUES (key_id, c.scope_kind, c.tenant_id, c.venue_id, c.account_scope, c.symbol,
            c.position_id, c.potential, c.conflict)
    ON CONFLICT (intent_id) DO UPDATE
    SET scope_kind = excluded.scope_kind, scope_tenant = excluded.scope_tenant,
        scope_venue = excluded.scope_venue, scope_account = excluded.scope_account,
        scope_symbol = excluded.scope_symbol, scope_position = excluded.scope_position,
        potential = excluded.potential, conflict = excluded.conflict;
    GET DIAGNOSTICS n_members = ROW_COUNT;
    n_aggregates := sell_shadow_move(
        coalesce(prior.scope_kind, 'none'), prior.scope_tenant, prior.scope_venue,
        prior.scope_account, prior.scope_symbol, prior.scope_position,
        coalesce(prior.potential, false), coalesce(prior.conflict, false), c);
END;
$$;

CREATE FUNCTION sell_shadow_set_order(
    o venue_orders, c sell_shadow_class, OUT n_members integer, OUT n_aggregates integer
) LANGUAGE plpgsql AS $$
DECLARE
    prior sell_shadow_orders%ROWTYPE;
BEGIN
    n_members := 0;
    n_aggregates := 0;
    SELECT m.* INTO prior FROM sell_shadow_orders m
    WHERE (m.tenant_id, m.venue_id, m.account_scope, m.symbol, m.exchange_order_id)
        = (o.tenant_id, o.venue_id, o.account_scope, o.symbol, o.exchange_order_id);
    IF NOT FOUND THEN
        IF NOT (c.potential OR c.conflict) THEN
            RETURN;
        END IF;
    ELSIF (prior.scope_kind, prior.scope_tenant, prior.scope_venue, prior.scope_account,
           prior.scope_symbol, prior.scope_position, prior.potential, prior.conflict)
          IS NOT DISTINCT FROM
          (c.scope_kind, c.tenant_id, c.venue_id, c.account_scope, c.symbol, c.position_id,
           c.potential, c.conflict) THEN
        RETURN;
    END IF;
    INSERT INTO sell_shadow_orders
    VALUES (o.tenant_id, o.venue_id, o.account_scope, o.symbol, o.exchange_order_id,
            c.scope_kind, c.tenant_id, c.venue_id, c.account_scope, c.symbol,
            c.position_id, c.potential, c.conflict)
    ON CONFLICT (tenant_id, venue_id, account_scope, symbol, exchange_order_id) DO UPDATE
    SET scope_kind = excluded.scope_kind, scope_tenant = excluded.scope_tenant,
        scope_venue = excluded.scope_venue, scope_account = excluded.scope_account,
        scope_symbol = excluded.scope_symbol, scope_position = excluded.scope_position,
        potential = excluded.potential, conflict = excluded.conflict;
    GET DIAGNOSTICS n_members = ROW_COUNT;
    n_aggregates := sell_shadow_move(
        coalesce(prior.scope_kind, 'none'), prior.scope_tenant, prior.scope_venue,
        prior.scope_account, prior.scope_symbol, prior.scope_position,
        coalesce(prior.potential, false), coalesce(prior.conflict, false), c);
END;
$$;

-- Raw facts only: a disposition, an attempt state's name or an empty native
-- read never discharges a member.
CREATE FUNCTION sell_shadow_classify(
    j order_intents, has_j boolean, o venue_orders, has_o boolean, mismatched_native boolean
) RETURNS sell_shadow_class LANGUAGE plpgsql AS $$
DECLARE
    r sell_shadow_class;
    suspected boolean := has_j AND j.kind IN ('protection', 'exit');
    exact boolean;
    contradicted boolean;
    recognized boolean;
    leg_symbol text;
    a_state text;
    a_rejected_at timestamptz;
BEGIN
    r := ROW('none', NULL, NULL, NULL, NULL, NULL, false, false, NULL, 0)::sell_shadow_class;
    IF has_o THEN
        IF o.side <> 'SELL' AND NOT suspected THEN
            r.branch := 'native_not_sell';
            RETURN r;
        END IF;
        -- Native scope only: a J claim never casts a foreign or null scope into a leg.
        r.scope_kind := CASE WHEN o.position_id IS NULL THEN 'symbol' ELSE 'leg' END;
        r.tenant_id := o.tenant_id;
        r.venue_id := o.venue_id;
        r.account_scope := o.account_scope;
        r.symbol := o.symbol;
        r.position_id := o.position_id;
        exact := has_j AND sell_shadow_exact_link(j, o);
        -- A named native row disputes a no-send or a rejection.
        contradicted := has_j AND (j.not_sent OR j.venue_outcome IS NOT DISTINCT FROM 'rejected');
        recognized := o.phase = 'terminal'
            AND o.status IN ('FILLED', 'CANCELED', 'EXPIRED', 'EXPIRED_IN_MATCH', 'REJECTED');
        IF recognized AND exact AND NOT contradicted THEN
            r.branch := 'native_discharged';
            RETURN r;
        END IF;
        r.potential := true;
        r.conflict := o.phase = 'terminal' OR contradicted
            OR ((o.position_id IS NOT NULL OR has_j) AND NOT exact);
        r.branch := CASE WHEN o.position_id IS NULL THEN 'native_unowned'
                         WHEN r.conflict THEN 'native_conflict' ELSE 'native_open' END;
        RETURN r;
    END IF;

    IF NOT suspected THEN
        r.branch := 'journal_not_sell';
        RETURN r;
    END IF;
    r.tenant_id := j.tenant_id;
    r.venue_id := j.venue_id;
    r.account_scope := j.account_scope;
    SELECT l.symbol INTO leg_symbol FROM balance_legs l
    WHERE (l.position_id, l.tenant_id, l.venue_id, l.account_scope)
        = (j.position_id, j.tenant_id, j.venue_id, j.account_scope);
    r.probes := r.probes + 1;
    IF FOUND THEN
        r.scope_kind := 'leg';
        r.symbol := leg_symbol;
        r.position_id := j.position_id;
    ELSE
        r.scope_kind := 'unbound';
    END IF;

    IF mismatched_native THEN
        r.potential := true;
        r.conflict := true;
        r.branch := 'journal_mismatched_native';
        RETURN r;
    END IF;
    IF j.not_sent AND j.venue_outcome IS NULL AND j.venue_evidence IS NULL THEN
        r.branch := 'journal_no_send';
        RETURN r;
    END IF;
    IF j.venue_outcome = 'rejected' AND NOT j.not_sent AND j.venue_evidence IS NULL THEN
        -- order_journal failed_outcome: the one Spot attempt is rejected with a time.
        SELECT s.state, s.rejected_at INTO a_state, a_rejected_at FROM submit_attempts s
        WHERE s.venue_id = 'binance:spot'
          AND (s.venue_id, s.account_scope, s.client_order_id)
              = (j.venue_id, j.account_scope, j.client_order_id);
        r.probes := r.probes + 1;
        IF FOUND AND a_state = 'rejected' AND a_rejected_at IS NOT NULL THEN
            r.branch := 'journal_rejected';
            RETURN r;
        END IF;
    END IF;
    r.potential := true;
    r.conflict := j.not_sent OR j.venue_outcome IS NOT DISTINCT FROM 'rejected';
    r.branch := CASE WHEN r.conflict THEN 'journal_conflict' ELSE 'journal_unresolved' END;
    RETURN r;
END;
$$;

CREATE FUNCTION sell_shadow_sync(
    j order_intents, has_j boolean, o venue_orders, has_o boolean, source text, n_probes integer
) RETURNS void LANGUAGE plpgsql AS $$
DECLARE
    c sell_shadow_class;
    journal_class sell_shadow_class;
    m integer;
    g integer;
    n_members integer := 0;
    n_aggregates integer := 0;
BEGIN
    IF has_j AND has_o AND (j.tenant_id, j.venue_id) <> (o.tenant_id, o.venue_id) THEN
        RAISE EXCEPTION 'sell shadow cannot reconcile across accounting owners';
    END IF;
    c := sell_shadow_classify(j, has_j, o, has_o, false);
    n_probes := n_probes + c.probes;
    IF NOT (c.potential OR c.conflict) THEN
        c := ROW('none', NULL, NULL, NULL, NULL, NULL, false, false, c.branch, 0)::sell_shadow_class;
    END IF;
    -- A non-SELL kind never held a member, so these branches touch no member row.
    IF c.branch NOT IN ('native_not_sell', 'journal_not_sell') THEN
        IF has_o THEN
            SELECT * INTO m, g FROM sell_shadow_set_order(o, c);
            n_members := n_members + m;
            n_aggregates := n_aggregates + g;
            n_probes := n_probes + 1;
            IF has_j THEN
                journal_class := ROW('none', NULL, NULL, NULL, NULL, NULL,
                                     false, false, NULL, 0)::sell_shadow_class;
                IF NOT sell_shadow_exact_link(j, o) THEN
                    journal_class := sell_shadow_classify(j, true, o, false, true);
                END IF;
                SELECT * INTO m, g FROM sell_shadow_set_intent(j.intent_id, journal_class);
                n_members := n_members + m;
                n_aggregates := n_aggregates + g;
                n_probes := n_probes + 1 + journal_class.probes;
            END IF;
        ELSE
            SELECT * INTO m, g FROM sell_shadow_set_intent(j.intent_id, c);
            n_members := n_members + m;
            n_aggregates := n_aggregates + g;
            n_probes := n_probes + 1;
        END IF;
    END IF;

    IF current_setting('robson_test.sell_shadow_trace', true) = 'on' THEN
        INSERT INTO sell_shadow_work
            (source, label, branch, intent_id, exchange_order_id, probes, member_rows, aggregate_rows)
        VALUES (source, nullif(current_setting('robson_test.sell_shadow_label', true), ''),
                c.branch, CASE WHEN has_j THEN j.intent_id END,
                CASE WHEN has_o THEN o.exchange_order_id END, n_probes, n_members, n_aggregates);
    END IF;
    IF n_members <> 0 AND EXISTS (
        SELECT 1 FROM sell_shadow_fail_member f
        WHERE f.member_key IN (CASE WHEN has_o THEN o.exchange_order_id END,
                               CASE WHEN has_j THEN j.intent_id::text END)) THEN
        RAISE EXCEPTION 'sell shadow member rollback witness';
    END IF;
END;
$$;

CREATE FUNCTION sell_shadow_intent_changed() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE
    o venue_orders%ROWTYPE;
    has_o boolean;
BEGIN
    IF NEW.venue_id <> 'binance:spot' THEN
        RETURN NULL;
    END IF;
    IF TG_OP = 'UPDATE' AND (NEW.not_sent, NEW.venue_outcome, NEW.venue_evidence)
                        IS NOT DISTINCT FROM (OLD.not_sent, OLD.venue_outcome, OLD.venue_evidence)
    THEN
        RETURN NULL;
    END IF;
    SELECT v.* INTO o FROM venue_orders v WHERE v.intent_id = NEW.intent_id;
    has_o := FOUND;
    PERFORM sell_shadow_sync(NEW, true, o, has_o, 'journal', 1);
    RETURN NULL;
END;
$$;

CREATE TRIGGER sell_shadow_intent AFTER INSERT OR UPDATE OF not_sent, venue_outcome, venue_evidence
    ON order_intents FOR EACH ROW EXECUTE FUNCTION sell_shadow_intent_changed();

CREATE FUNCTION sell_shadow_attempt_changed() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE
    j order_intents%ROWTYPE;
    o venue_orders%ROWTYPE;
BEGIN
    IF NEW.venue_id <> 'binance:spot' THEN
        RETURN NULL;
    END IF;
    IF TG_OP = 'UPDATE' AND (NEW.state, NEW.rejected_at)
                        IS NOT DISTINCT FROM (OLD.state, OLD.rejected_at) THEN
        RETURN NULL;
    END IF;
    SELECT i.* INTO j FROM order_intents i
    WHERE (i.venue_id, i.account_scope, i.client_order_id)
        = (NEW.venue_id, NEW.account_scope, NEW.client_order_id);
    IF NOT FOUND OR j.kind NOT IN ('protection', 'exit') THEN
        RETURN NULL;
    END IF;
    -- A paired member is classified from J and native facts, not the attempt.
    SELECT v.* INTO o FROM venue_orders v WHERE v.intent_id = j.intent_id;
    IF FOUND THEN
        RETURN NULL;
    END IF;
    PERFORM sell_shadow_sync(j, true, o, false, 'attempt', 2);
    RETURN NULL;
END;
$$;

CREATE TRIGGER sell_shadow_attempt AFTER INSERT OR UPDATE OF state, rejected_at
    ON submit_attempts FOR EACH ROW EXECUTE FUNCTION sell_shadow_attempt_changed();

CREATE FUNCTION sell_shadow_venue_changed() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE
    j order_intents%ROWTYPE;
    has_j boolean := false;
    n_probes integer := 0;
BEGIN
    IF NEW.venue_id <> 'binance:spot' THEN
        RETURN NULL;
    END IF;
    IF TG_OP = 'UPDATE' AND (NEW.phase, NEW.status, NEW.owner, NEW.intent_id, NEW.position_id, NEW.kind)
                        IS NOT DISTINCT FROM (OLD.phase, OLD.status, OLD.owner, OLD.intent_id,
                                              OLD.position_id, OLD.kind)
    THEN
        RETURN NULL;
    END IF;
    IF NEW.intent_id IS NOT NULL THEN
        SELECT i.* INTO j FROM order_intents i WHERE i.intent_id = NEW.intent_id;
        has_j := FOUND;
        n_probes := 1;
    END IF;
    PERFORM sell_shadow_sync(j, has_j, NEW, true, 'venue', n_probes);
    RETURN NULL;
END;
$$;

CREATE TRIGGER sell_shadow_venue AFTER INSERT OR UPDATE OF phase, status, owner, intent_id,
    position_id, kind ON venue_orders FOR EACH ROW EXECUTE FUNCTION sell_shadow_venue_changed();
