-- ADR-0056 step 8, PR 8e (S8-1): the venue ledger of balance-account venues (Binance spot).
-- Nothing writes these tables until step 8 selects spot (8o): on a USD-M database they stay empty.
-- Additive: no existing table is altered, and no constraint here references a table the running daemon writes,
-- so the migration hook takes no lock the old process waits on. Integrity with the journal comes from writing
-- in the same transaction. Every write of balance_leg_admissions, balance_legs, venue_orders (except a
-- last_checked_at-only touch), order_fills or spot_intent_clock bumps accounting_state.version of its venue in
-- that transaction, as its first write (design §3.7.3, §3.16). leg_routes and venue_accounts bump nothing.

-- Finite, and decodable as rust_decimal::Decimal (scale <= 28, mantissa within 96 bits). NaN and ±Infinity
-- give NaN for value - value, never 0.
CREATE FUNCTION ledger_amount_fits(value NUMERIC) RETURNS BOOLEAN
    LANGUAGE sql IMMUTABLE STRICT PARALLEL SAFE AS $$
    SELECT CASE WHEN value - value = 0
                THEN scale(value) <= 28
                     AND abs(value) * power(10::numeric, scale(value)) <= 79228162514264337593543950335
                ELSE false END
$$;

-- A venue integer id as stored: digits, no sign, no leading zero, within u64.
CREATE FUNCTION ledger_canonical_id(value TEXT) RETURNS BOOLEAN
    LANGUAGE sql IMMUTABLE STRICT PARALLEL SAFE AS $$
    SELECT CASE WHEN value ~ '^(0|[1-9][0-9]{0,19})$' THEN value::numeric <= 18446744073709551615
                ELSE false END
$$;

CREATE FUNCTION ledger_row_is_immutable() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'ledger invariant: % rows are never updated or deleted', TG_TABLE_NAME
        USING ERRCODE = 'check_violation';
END;
$$;

-- 1. One version row per balance-account venue (§3.7.3). Created by ensure_accounting_row, called by the cutover
--    before tx1 (8m). It certifies nothing by existing: certification is 8m's. J never creates it.
CREATE TABLE accounting_state (
    tenant_id    UUID NOT NULL,
    venue_id     TEXT NOT NULL,
    version      BIGINT NOT NULL DEFAULT 0 CHECK (version >= 0),
    budget_model TEXT NOT NULL CHECK (budget_model IN ('hwm_v1', 'net_from_start_non_expanding_v1')),
    created_at   TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (tenant_id, venue_id)
);
CREATE FUNCTION accounting_state_version_rises() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'DELETE'
       OR (NEW.tenant_id, NEW.venue_id, NEW.created_at) IS DISTINCT FROM (OLD.tenant_id, OLD.venue_id, OLD.created_at)
       OR NEW.version <= OLD.version THEN
        RAISE EXCEPTION 'ledger invariant: accounting_state % must raise its version and keep its identity', TG_OP
            USING ERRCODE = 'check_violation';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER accounting_state_version_rises BEFORE UPDATE OR DELETE ON accounting_state
    FOR EACH ROW EXECUTE FUNCTION accounting_state_version_rises();

-- 2. The venue account a scope is bound to (§3.11): immutable, inserted once by the cutover's completion (8m).
CREATE TABLE venue_accounts (
    venue_id      TEXT NOT NULL,
    account_scope TEXT NOT NULL,
    tenant_id     UUID NOT NULL,
    uid           TEXT NOT NULL CHECK (uid <> ''),
    bound_at      TIMESTAMPTZ NOT NULL,
    cutover_id    UUID NOT NULL,                  -- its foreign key to venue_cutovers comes with S8-6 (8m)
    PRIMARY KEY (venue_id, account_scope),
    FOREIGN KEY (tenant_id, venue_id) REFERENCES accounting_state (tenant_id, venue_id)
);
CREATE TRIGGER venue_accounts_are_immutable BEFORE UPDATE OR DELETE ON venue_accounts
    FOR EACH ROW EXECUTE FUNCTION ledger_row_is_immutable();

-- 3. Routes (§3.11): written at arm for balance-account legs only; no row means a legacy binance:usdm position.
CREATE TABLE leg_routes (
    position_id   UUID PRIMARY KEY,
    tenant_id     UUID NOT NULL,
    venue_id      TEXT NOT NULL,
    account_scope TEXT NOT NULL,
    routed_at     TIMESTAMPTZ NOT NULL,
    UNIQUE (position_id, tenant_id, venue_id, account_scope),
    FOREIGN KEY (tenant_id, venue_id) REFERENCES accounting_state (tenant_id, venue_id)
);
CREATE TRIGGER leg_routes_are_immutable BEFORE UPDATE OR DELETE ON leg_routes
    FOR EACH ROW EXECUTE FUNCTION ledger_row_is_immutable();

-- 4. Every admission of a leg (§3.8, §C7): one per entry intent's strict commit, immutable. A later admission may
--    supersede one only on positive proof that it was never sent (balance_leg_admit); the superseded row stays.
CREATE TABLE balance_leg_admissions (
    entry_intent_id     UUID PRIMARY KEY,
    position_id         UUID NOT NULL,
    tenant_id           UUID NOT NULL,
    venue_id            TEXT NOT NULL,
    account_scope       TEXT NOT NULL,
    symbol              TEXT NOT NULL CHECK (symbol <> ''),
    decided_month       DATE NOT NULL CHECK (EXTRACT(DAY FROM decided_month) = 1),
    -- cost terms from the admitted runtime rules and envelope snapshot (§3.8.1-§3.8.2; 8g)
    taker_bps           NUMERIC NOT NULL CHECK (ledger_amount_fits(taker_bps) AND taker_bps >= 0 AND taker_bps < 10000),
    gap_bps             NUMERIC NOT NULL CHECK (ledger_amount_fits(gap_bps) AND gap_bps >= 0 AND gap_bps < 10000),
    notional_margin_bps NUMERIC NOT NULL
        CHECK (ledger_amount_fits(notional_margin_bps) AND notional_margin_bps >= 0 AND notional_margin_bps < 10000),
    min_notional        NUMERIC NOT NULL CHECK (ledger_amount_fits(min_notional) AND min_notional > 0),
    lot_step            NUMERIC NOT NULL CHECK (ledger_amount_fits(lot_step) AND lot_step > 0),
    fee_asset_policy    TEXT NOT NULL CHECK (fee_asset_policy = 'base'),                       -- SD2
    envelope_source     TEXT NOT NULL CHECK (envelope_source <> ''),
    envelope_as_of      TIMESTAMPTZ NOT NULL,
    envelope_hash       TEXT NOT NULL CHECK (envelope_hash <> ''),
    -- sizing and reserves (§3.8.3)
    planned_qty         NUMERIC NOT NULL CHECK (ledger_amount_fits(planned_qty) AND planned_qty > 0),        -- S
    requested_qty       NUMERIC NOT NULL
        CHECK (ledger_amount_fits(requested_qty) AND requested_qty >= planned_qty),                          -- Q_gross
    reference_price     NUMERIC NOT NULL CHECK (ledger_amount_fits(reference_price) AND reference_price > 0),
    remainder_reserve   NUMERIC NOT NULL CHECK (ledger_amount_fits(remainder_reserve)
                            AND remainder_reserve >= 0 AND remainder_reserve <= planned_qty),               -- R̂
    admission_charge    NUMERIC NOT NULL CHECK (ledger_amount_fits(admission_charge) AND admission_charge > 0),
    initial_trigger     NUMERIC NOT NULL CHECK (ledger_amount_fits(initial_trigger) AND initial_trigger > 0),
    created_at          TIMESTAMPTZ NOT NULL,
    superseded_at       TIMESTAMPTZ,
    superseded_by       UUID,
    CONSTRAINT balance_leg_admissions_route FOREIGN KEY (position_id, tenant_id, venue_id, account_scope)
        REFERENCES leg_routes (position_id, tenant_id, venue_id, account_scope),
    CONSTRAINT balance_leg_admissions_supersession_whole CHECK ((superseded_at IS NULL) = (superseded_by IS NULL)),
    CONSTRAINT balance_leg_admissions_not_by_itself CHECK (superseded_by IS DISTINCT FROM entry_intent_id),
    UNIQUE (entry_intent_id, position_id),
    UNIQUE (entry_intent_id, position_id, tenant_id, venue_id, account_scope, symbol, decided_month),
    -- the successor is an admission of the same leg; deferred: the predecessor is marked before its successor lands
    CONSTRAINT balance_leg_admissions_successor FOREIGN KEY (superseded_by, position_id)
        REFERENCES balance_leg_admissions (entry_intent_id, position_id) DEFERRABLE INITIALLY DEFERRED
);
CREATE UNIQUE INDEX idx_balance_leg_admissions_current ON balance_leg_admissions (position_id)
    WHERE superseded_at IS NULL;
CREATE FUNCTION balance_leg_admissions_once() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    -- The only update ever allowed: the one-time supersession mark; every other column is frozen.
    IF TG_OP = 'DELETE' OR OLD.superseded_at IS NOT NULL OR NEW.superseded_at IS NULL
       OR (NEW.entry_intent_id, NEW.position_id, NEW.tenant_id, NEW.venue_id, NEW.account_scope, NEW.symbol,
           NEW.decided_month, NEW.taker_bps, NEW.gap_bps, NEW.notional_margin_bps, NEW.min_notional, NEW.lot_step,
           NEW.fee_asset_policy, NEW.envelope_source, NEW.envelope_as_of, NEW.envelope_hash, NEW.planned_qty,
           NEW.requested_qty, NEW.reference_price, NEW.remainder_reserve, NEW.admission_charge, NEW.initial_trigger,
           NEW.created_at)
          IS DISTINCT FROM
          (OLD.entry_intent_id, OLD.position_id, OLD.tenant_id, OLD.venue_id, OLD.account_scope, OLD.symbol,
           OLD.decided_month, OLD.taker_bps, OLD.gap_bps, OLD.notional_margin_bps, OLD.min_notional, OLD.lot_step,
           OLD.fee_asset_policy, OLD.envelope_source, OLD.envelope_as_of, OLD.envelope_hash, OLD.planned_qty,
           OLD.requested_qty, OLD.reference_price, OLD.remainder_reserve, OLD.admission_charge, OLD.initial_trigger,
           OLD.created_at)
    THEN
        RAISE EXCEPTION 'ledger invariant: an admission is only ever marked superseded, once'
            USING ERRCODE = 'check_violation';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER balance_leg_admissions_once BEFORE UPDATE OR DELETE ON balance_leg_admissions
    FOR EACH ROW EXECUTE FUNCTION balance_leg_admissions_once();

-- 5. Balance-account legs (§3.3.2, §3.3.4, §3.5, §C7): the current admission, the cohort, and the quantities.
CREATE TABLE balance_legs (
    position_id           UUID PRIMARY KEY,
    tenant_id             UUID NOT NULL,
    venue_id              TEXT NOT NULL,
    account_scope         TEXT NOT NULL,
    symbol                TEXT NOT NULL,
    admission_intent_id   UUID NOT NULL UNIQUE,      -- the current admission (balance_leg_admit)
    decided_month         DATE NOT NULL,             -- the current admission's (checked by the key below)
    cohort_month          DATE CHECK (cohort_month IS NULL OR cohort_month = decided_month),
    target_trigger        NUMERIC NOT NULL CHECK (ledger_amount_fits(target_trigger) AND target_trigger > 0),
    gross_qty             NUMERIC CHECK (ledger_amount_fits(gross_qty) AND gross_qty >= 0),               -- Q
    acquisition_cost      NUMERIC CHECK (ledger_amount_fits(acquisition_cost) AND acquisition_cost >= 0), -- C
    base_commission       NUMERIC CHECK (ledger_amount_fits(base_commission) AND base_commission >= 0),   -- B or B̂
    base_commission_final BOOLEAN NOT NULL DEFAULT FALSE,
    executable_qty        NUMERIC CHECK (ledger_amount_fits(executable_qty) AND executable_qty >= 0),     -- X
    consumed_cum          NUMERIC NOT NULL DEFAULT 0
        CHECK (ledger_amount_fits(consumed_cum) AND consumed_cum >= 0),       -- Σ leg-attributed sells; may exceed X
    recovery_state        TEXT CHECK (recovery_state IN ('entry_unknown', 'orders_outstanding', 'unprotected',
                              'unresolved', 'unresolved_locked', 'cancel_uncertain')),
    created_at            TIMESTAMPTZ NOT NULL,
    closed_at             TIMESTAMPTZ,
    CONSTRAINT balance_legs_admission
        FOREIGN KEY (admission_intent_id, position_id, tenant_id, venue_id, account_scope, symbol, decided_month)
        REFERENCES balance_leg_admissions (entry_intent_id, position_id, tenant_id, venue_id, account_scope, symbol,
                                           decided_month),
    CONSTRAINT balance_legs_version_row FOREIGN KEY (tenant_id, venue_id) REFERENCES accounting_state (tenant_id, venue_id),
    UNIQUE (position_id, tenant_id, venue_id, account_scope, symbol),   -- venue_orders' leg key
    CONSTRAINT balance_legs_quantities_together CHECK (
        (gross_qty IS NULL) = (acquisition_cost IS NULL) AND (gross_qty IS NULL) = (base_commission IS NULL)
        AND (gross_qty IS NULL) = (executable_qty IS NULL)),
    CONSTRAINT balance_legs_dust_not_negative CHECK (gross_qty IS NULL OR base_commission + executable_qty <= gross_qty),
    CONSTRAINT balance_legs_final_has_quantities CHECK (NOT base_commission_final OR gross_qty IS NOT NULL)
);
CREATE INDEX idx_balance_legs_open ON balance_legs (venue_id) WHERE closed_at IS NULL;
CREATE FUNCTION balance_legs_move_forward() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'DELETE'
       OR (NEW.position_id, NEW.tenant_id, NEW.venue_id, NEW.account_scope, NEW.symbol, NEW.created_at)
          IS DISTINCT FROM (OLD.position_id, OLD.tenant_id, OLD.venue_id, OLD.account_scope, OLD.symbol, OLD.created_at)
       -- the admission changes only by supersession of an admission that never reached the venue
       OR (NEW.admission_intent_id <> OLD.admission_intent_id AND NOT (
              OLD.cohort_month IS NULL AND NEW.cohort_month IS NULL AND OLD.gross_qty IS NULL
              AND OLD.consumed_cum = 0 AND NEW.consumed_cum = 0
              AND EXISTS (SELECT 1 FROM balance_leg_admissions a
                          WHERE a.entry_intent_id = OLD.admission_intent_id
                            AND a.superseded_by = NEW.admission_intent_id)))
       OR (NEW.admission_intent_id = OLD.admission_intent_id AND NEW.decided_month <> OLD.decided_month)
       OR (OLD.cohort_month IS NOT NULL AND NEW.cohort_month IS DISTINCT FROM OLD.cohort_month)
       OR NEW.consumed_cum < OLD.consumed_cum
       OR (OLD.executable_qty IS NOT NULL AND (NEW.executable_qty IS NULL OR NEW.executable_qty > OLD.executable_qty))
       OR (OLD.gross_qty IS NOT NULL
           AND (NEW.gross_qty, NEW.acquisition_cost) IS DISTINCT FROM (OLD.gross_qty, OLD.acquisition_cost))
       OR (OLD.base_commission_final
           AND (NOT NEW.base_commission_final OR NEW.base_commission IS DISTINCT FROM OLD.base_commission))
       OR (OLD.closed_at IS NOT NULL AND NEW.closed_at IS DISTINCT FROM OLD.closed_at)
    THEN
        RAISE EXCEPTION 'ledger invariant: balance_legs % on % only moves forward', TG_OP, OLD.position_id
            USING ERRCODE = 'check_violation';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER balance_legs_move_forward BEFORE UPDATE OR DELETE ON balance_legs
    FOR EACH ROW EXECUTE FUNCTION balance_legs_move_forward();

-- 6. Every venue order Robson tracks on a balance-account venue (§3.7.1; ExpiredUnfilled's durable home, 8c C4).
CREATE TABLE venue_orders (
    venue_id              TEXT NOT NULL,
    account_scope         TEXT NOT NULL,
    symbol                TEXT NOT NULL,
    exchange_order_id     TEXT NOT NULL CHECK (ledger_canonical_id(exchange_order_id)),
    tenant_id             UUID NOT NULL,
    client_order_id       TEXT NOT NULL,          -- placed under; immutable
    last_client_order_id  TEXT NOT NULL,          -- latest the venue reported (a cancel renames it, F12); audit
    owner                 TEXT NOT NULL CHECK (owner IN ('leg', 'unauthored', 'discovered')),
    intent_id             UUID,
    position_id           UUID,
    kind                  TEXT CHECK (kind IN ('entry', 'stop', 'sell')),
    side                  TEXT NOT NULL CHECK (side IN ('BUY', 'SELL')),
    order_type            TEXT NOT NULL,
    orig_qty              NUMERIC NOT NULL CHECK (ledger_amount_fits(orig_qty) AND orig_qty > 0),
    -- Non-trigger zero placeholders are normalized to NULL before insert AND identity comparison (§3.3).
    stop_price            NUMERIC CHECK (ledger_amount_fits(stop_price) AND stop_price > 0),
    status                TEXT NOT NULL,
    phase                 TEXT NOT NULL CHECK (phase IN ('live', 'terminal', 'unknown')),
    terminal              BOOLEAN GENERATED ALWAYS AS (phase = 'terminal') STORED,
    expiry_reason         TEXT,
    executed_cum          NUMERIC NOT NULL DEFAULT 0 CHECK (ledger_amount_fits(executed_cum)),
    cumulative_quote      NUMERIC CHECK (ledger_amount_fits(cumulative_quote) AND cumulative_quote >= 0),
    fills_qty             NUMERIC NOT NULL DEFAULT 0 CHECK (ledger_amount_fits(fills_qty)),
    fills_complete        BOOLEAN GENERATED ALWAYS AS (fills_qty = executed_cum) STORED,
    trades_cursor         BIGINT NOT NULL DEFAULT 0 CHECK (trades_cursor >= 0),
    last_checked_at       TIMESTAMPTZ NOT NULL,   -- rotation only: a touch of it alone bumps nothing (Q8)
    venue_time_ms           BIGINT,               -- the venue's times as given: audit only (R9-4)
    venue_update_time_ms    BIGINT,
    venue_transact_time_ms  BIGINT,
    venue_working_time_ms   BIGINT,
    live_seen_at                  TIMESTAMPTZ NOT NULL,   -- first durable record: the order existed by then
    live_clock_sample_id          UUID,                   -- NULL evidence is invalid evidence; FKs with S8-2
    live_clock_wall_delta_ms      BIGINT,
    live_clock_mono_delta_ms      BIGINT,
    live_clock_verdict            TEXT CHECK (live_clock_verdict IN ('valid', 'invalid')),
    terminal_seen_at              TIMESTAMPTZ,            -- first durable terminal record: terminal by then
    terminal_clock_sample_id      UUID,
    terminal_clock_wall_delta_ms  BIGINT,
    terminal_clock_mono_delta_ms  BIGINT,
    terminal_clock_verdict        TEXT CHECK (terminal_clock_verdict IN ('valid', 'invalid')),
    PRIMARY KEY (venue_id, account_scope, symbol, exchange_order_id),
    UNIQUE (venue_id, account_scope, symbol, exchange_order_id, tenant_id),          -- order_fills' key
    CONSTRAINT venue_orders_leg FOREIGN KEY (position_id, tenant_id, venue_id, account_scope, symbol)
        REFERENCES balance_legs (position_id, tenant_id, venue_id, account_scope, symbol),
    CONSTRAINT venue_orders_version_row FOREIGN KEY (tenant_id, venue_id) REFERENCES accounting_state (tenant_id, venue_id),
    CONSTRAINT venue_orders_executed_within_order CHECK (executed_cum >= 0 AND executed_cum <= orig_qty),
    CONSTRAINT venue_orders_fills_within_executed CHECK (fills_qty >= 0 AND fills_qty <= executed_cum),
    -- ownership: all of intent, position and kind, or none of them
    CONSTRAINT venue_orders_ownership_whole CHECK (
        (owner = 'leg') = (position_id IS NOT NULL)
        AND (position_id IS NULL) = (intent_id IS NULL) AND (position_id IS NULL) = (kind IS NULL)),
    CONSTRAINT venue_orders_kind_matches_side CHECK (kind IS NULL OR (kind = 'entry') = (side = 'BUY')),
    CONSTRAINT venue_orders_stop_has_trigger CHECK (kind IS DISTINCT FROM 'stop' OR stop_price IS NOT NULL),
    CONSTRAINT venue_orders_terminal_was_seen CHECK ((phase = 'terminal') = (terminal_seen_at IS NOT NULL)),
    CONSTRAINT venue_orders_live_evidence_whole CHECK (live_clock_verdict IS DISTINCT FROM 'valid' OR
        (live_clock_sample_id IS NOT NULL AND live_clock_wall_delta_ms IS NOT NULL AND live_clock_mono_delta_ms IS NOT NULL)),
    CONSTRAINT venue_orders_terminal_evidence_whole CHECK (terminal_clock_verdict IS DISTINCT FROM 'valid' OR
        (terminal_clock_sample_id IS NOT NULL AND terminal_clock_wall_delta_ms IS NOT NULL
         AND terminal_clock_mono_delta_ms IS NOT NULL)),
    CONSTRAINT venue_orders_terminal_evidence_needs_its_time CHECK (terminal_seen_at IS NOT NULL OR
        (terminal_clock_sample_id IS NULL AND terminal_clock_wall_delta_ms IS NULL
         AND terminal_clock_mono_delta_ms IS NULL AND terminal_clock_verdict IS NULL))
);
CREATE UNIQUE INDEX idx_venue_orders_intent ON venue_orders (intent_id) WHERE intent_id IS NOT NULL;
CREATE INDEX idx_venue_orders_settlement ON venue_orders (venue_id, last_checked_at) WHERE terminal AND NOT fills_complete;
CREATE INDEX idx_venue_orders_open_by_leg ON venue_orders (position_id) WHERE NOT terminal;
CREATE INDEX idx_venue_orders_leg ON venue_orders (position_id) WHERE position_id IS NOT NULL;
CREATE FUNCTION venue_orders_move_forward() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'DELETE'
       -- identity and first-seen evidence, primary key included
       OR (NEW.venue_id, NEW.account_scope, NEW.symbol, NEW.exchange_order_id, NEW.tenant_id, NEW.client_order_id,
           NEW.side, NEW.order_type, NEW.orig_qty, NEW.stop_price, NEW.live_seen_at, NEW.live_clock_sample_id,
           NEW.live_clock_wall_delta_ms, NEW.live_clock_mono_delta_ms, NEW.live_clock_verdict)
          IS DISTINCT FROM
          (OLD.venue_id, OLD.account_scope, OLD.symbol, OLD.exchange_order_id, OLD.tenant_id, OLD.client_order_id,
           OLD.side, OLD.order_type, OLD.orig_qty, OLD.stop_price, OLD.live_seen_at, OLD.live_clock_sample_id,
           OLD.live_clock_wall_delta_ms, OLD.live_clock_mono_delta_ms, OLD.live_clock_verdict)
       -- ownership: a leg link is permanent; a non-leg owner changes only by adoption into a leg
       OR (OLD.owner = 'leg' AND (NEW.owner, NEW.intent_id, NEW.position_id, NEW.kind)
                                 IS DISTINCT FROM (OLD.owner, OLD.intent_id, OLD.position_id, OLD.kind))
       OR (OLD.owner <> 'leg' AND NEW.owner <> 'leg' AND NEW.owner <> OLD.owner)
       OR NEW.executed_cum < OLD.executed_cum OR NEW.fills_qty < OLD.fills_qty
       OR NEW.trades_cursor < OLD.trades_cursor OR NEW.last_checked_at < OLD.last_checked_at
       -- a terminal order's state and its terminal evidence are final
       OR (OLD.phase = 'terminal' AND
           (NEW.phase, NEW.status, NEW.executed_cum, NEW.cumulative_quote, NEW.expiry_reason, NEW.terminal_seen_at,
            NEW.terminal_clock_sample_id, NEW.terminal_clock_wall_delta_ms, NEW.terminal_clock_mono_delta_ms,
            NEW.terminal_clock_verdict)
           IS DISTINCT FROM
           (OLD.phase, OLD.status, OLD.executed_cum, OLD.cumulative_quote, OLD.expiry_reason, OLD.terminal_seen_at,
            OLD.terminal_clock_sample_id, OLD.terminal_clock_wall_delta_ms, OLD.terminal_clock_mono_delta_ms,
            OLD.terminal_clock_verdict))
    THEN
        RAISE EXCEPTION 'ledger invariant: venue_orders % on % only moves forward', TG_OP, OLD.exchange_order_id
            USING ERRCODE = 'check_violation';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER venue_orders_move_forward BEFORE UPDATE OR DELETE ON venue_orders
    FOR EACH ROW EXECUTE FUNCTION venue_orders_move_forward();

-- 7. One row per venue trade, from trade pages only (§3.7.1). A trade id is unique within the symbol (ports.rs:524),
--    so one trade can never be booked against two orders. FULL placement fills carry no time and stay in J's evidence.
CREATE TABLE order_fills (
    venue_id          TEXT NOT NULL,
    account_scope     TEXT NOT NULL,
    symbol            TEXT NOT NULL,
    trade_id          TEXT NOT NULL CHECK (ledger_canonical_id(trade_id)),
    exchange_order_id TEXT NOT NULL,
    tenant_id         UUID NOT NULL,
    price             NUMERIC NOT NULL CHECK (ledger_amount_fits(price) AND price > 0),
    qty               NUMERIC NOT NULL CHECK (ledger_amount_fits(qty) AND qty > 0),
    quote_qty         NUMERIC NOT NULL CHECK (ledger_amount_fits(quote_qty) AND quote_qty >= 0),
    commission        NUMERIC NOT NULL CHECK (ledger_amount_fits(commission) AND commission >= 0),
    commission_asset  TEXT NOT NULL CHECK (commission_asset <> ''),
    trade_time        TIMESTAMPTZ NOT NULL,               -- the venue's time: the economic cut (§C2)
    recorded_at       TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (venue_id, account_scope, symbol, trade_id),
    FOREIGN KEY (venue_id, account_scope, symbol, exchange_order_id, tenant_id)
        REFERENCES venue_orders (venue_id, account_scope, symbol, exchange_order_id, tenant_id)
);
CREATE INDEX idx_order_fills_order ON order_fills (venue_id, account_scope, symbol, exchange_order_id);
CREATE INDEX idx_order_fills_trade_time ON order_fills (venue_id, trade_time);
CREATE TRIGGER order_fills_are_immutable BEFORE UPDATE OR DELETE ON order_fills
    FOR EACH ROW EXECUTE FUNCTION ledger_row_is_immutable();

-- 8. Each attempt's creation time on a balance-account venue, with its clock evidence (§C4, §C8, v13): taken before
--    the attempt's commit and before any send, it is a conservative lower bound on the send, not the commit time.
--    Cancels included (§C4(b)); a proof must also bind a cancel's answer to its target (8p).
CREATE TABLE spot_intent_clock (
    attempt_id          UUID PRIMARY KEY,
    intent_id           UUID NOT NULL,
    tenant_id           UUID NOT NULL,
    venue_id            TEXT NOT NULL,
    attempt_created_at  TIMESTAMPTZ NOT NULL,
    clock_sample_id     UUID,
    clock_wall_delta_ms BIGINT,
    clock_mono_delta_ms BIGINT,
    clock_verdict       TEXT CHECK (clock_verdict IN ('valid', 'invalid')),
    FOREIGN KEY (tenant_id, venue_id) REFERENCES accounting_state (tenant_id, venue_id),
    CONSTRAINT spot_intent_clock_evidence_whole CHECK (clock_verdict IS DISTINCT FROM 'valid' OR
        (clock_sample_id IS NOT NULL AND clock_wall_delta_ms IS NOT NULL AND clock_mono_delta_ms IS NOT NULL))
);
CREATE INDEX idx_spot_intent_clock_intent ON spot_intent_clock (intent_id);
CREATE TRIGGER spot_intent_clock_is_immutable BEFORE UPDATE OR DELETE ON spot_intent_clock
    FOR EACH ROW EXECUTE FUNCTION ledger_row_is_immutable();

-- 9. An entry's admission, inside J's commit (§3.2 of the plan). Precondition, by protocol: the calling transaction
--    already holds this venue's accounting row (its first write). Inserts a new leg and admission; replays the same
--    admission as a no-op; supersedes the current admission only on positive proof it never reached the venue.
--    Sets cohort_month when p_handed_off. Refusals raise check_violation, and nothing in the transaction lands.
CREATE FUNCTION balance_leg_admit(
    p_tenant UUID, p_venue TEXT, p_scope TEXT, p_symbol TEXT, p_position UUID, p_intent UUID, p_decided_month DATE,
    p_taker_bps NUMERIC, p_gap_bps NUMERIC, p_margin_bps NUMERIC, p_min_notional NUMERIC, p_lot_step NUMERIC,
    p_fee_policy TEXT, p_envelope_source TEXT, p_envelope_as_of TIMESTAMPTZ, p_envelope_hash TEXT,
    p_planned NUMERIC, p_requested NUMERIC, p_reference NUMERIC, p_reserve NUMERIC, p_charge NUMERIC,
    p_trigger NUMERIC, p_created_at TIMESTAMPTZ, p_handed_off BOOLEAN) RETURNS VOID LANGUAGE plpgsql VOLATILE AS $$
DECLARE
    current_leg balance_legs%ROWTYPE;
    has_leg     BOOLEAN;
BEGIN
    SELECT * INTO current_leg FROM balance_legs WHERE position_id = p_position FOR UPDATE;
    has_leg := FOUND;
    IF EXISTS (SELECT 1 FROM balance_leg_admissions WHERE entry_intent_id = p_intent) THEN
        -- A replay: the stored admission must be this very one, on this leg, unchanged.
        IF NOT EXISTS (SELECT 1 FROM balance_leg_admissions a WHERE a.entry_intent_id = p_intent
                         AND (a.position_id, a.tenant_id, a.venue_id, a.account_scope, a.symbol, a.decided_month,
                              a.taker_bps, a.gap_bps, a.notional_margin_bps, a.min_notional, a.lot_step,
                              a.fee_asset_policy, a.envelope_source, a.envelope_as_of, a.envelope_hash, a.planned_qty,
                              a.requested_qty, a.reference_price, a.remainder_reserve, a.admission_charge,
                              a.initial_trigger, a.created_at)
                             IS NOT DISTINCT FROM
                             (p_position, p_tenant, p_venue, p_scope, p_symbol, p_decided_month, p_taker_bps,
                              p_gap_bps, p_margin_bps, p_min_notional, p_lot_step, p_fee_policy, p_envelope_source,
                              p_envelope_as_of, p_envelope_hash, p_planned, p_requested, p_reference, p_reserve,
                              p_charge, p_trigger, p_created_at)) THEN
            RAISE EXCEPTION 'ledger invariant: admission % replayed with other values', p_intent
                USING ERRCODE = 'check_violation';
        END IF;
    ELSE
        IF has_leg THEN
            -- Supersession: the current admission must be proven never to have reached the venue.
            IF current_leg.cohort_month IS NOT NULL OR current_leg.gross_qty IS NOT NULL
               OR current_leg.consumed_cum <> 0
               OR EXISTS (SELECT 1 FROM venue_orders WHERE position_id = p_position)
               OR NOT EXISTS (SELECT 1 FROM order_intents i
                              WHERE i.intent_id = current_leg.admission_intent_id AND i.kind = 'entry'
                                AND i.position_id = p_position AND i.not_sent AND i.venue_outcome IS NULL
                                AND i.uncertain_at IS NULL AND i.disposition_kind IS NULL) THEN
                RAISE EXCEPTION 'ledger invariant: leg % is held by admission %, not proven unsent',
                    p_position, current_leg.admission_intent_id USING ERRCODE = 'check_violation';
            END IF;
            UPDATE balance_leg_admissions SET superseded_at = p_created_at, superseded_by = p_intent
                WHERE entry_intent_id = current_leg.admission_intent_id;
        END IF;
        INSERT INTO balance_leg_admissions (entry_intent_id, position_id, tenant_id, venue_id, account_scope, symbol,
            decided_month, taker_bps, gap_bps, notional_margin_bps, min_notional, lot_step, fee_asset_policy,
            envelope_source, envelope_as_of, envelope_hash, planned_qty, requested_qty, reference_price,
            remainder_reserve, admission_charge, initial_trigger, created_at)
        VALUES (p_intent, p_position, p_tenant, p_venue, p_scope, p_symbol, p_decided_month, p_taker_bps, p_gap_bps,
            p_margin_bps, p_min_notional, p_lot_step, p_fee_policy, p_envelope_source, p_envelope_as_of,
            p_envelope_hash, p_planned, p_requested, p_reference, p_reserve, p_charge, p_trigger, p_created_at);
        IF has_leg THEN
            UPDATE balance_legs SET admission_intent_id = p_intent, decided_month = p_decided_month,
                                    target_trigger = p_trigger
                WHERE position_id = p_position;
        ELSE
            INSERT INTO balance_legs (position_id, tenant_id, venue_id, account_scope, symbol, admission_intent_id,
                decided_month, target_trigger, created_at)
            VALUES (p_position, p_tenant, p_venue, p_scope, p_symbol, p_intent, p_decided_month, p_trigger, p_created_at);
        END IF;
    END IF;
    IF p_handed_off THEN
        UPDATE balance_legs SET cohort_month = decided_month
            WHERE position_id = p_position AND admission_intent_id = p_intent AND cohort_month IS NULL;
    END IF;
END;
$$;
