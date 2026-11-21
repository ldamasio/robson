DO $migration41$
BEGIN
    CREATE TABLE public.spot_order_set_generation (
        tenant_id UUID NOT NULL,
        venue_id TEXT NOT NULL,
        account_scope TEXT NOT NULL,
        generation_id UUID NOT NULL,
        process_epoch UUID,
        source_audit_id UUID,
        accounting_version BIGINT,
        CONSTRAINT spot_order_set_generation_key PRIMARY KEY (tenant_id, venue_id, account_scope),
        CONSTRAINT spot_order_set_generation_venue CHECK (venue_id = 'binance:spot'),
        CONSTRAINT spot_order_set_generation_account
            CHECK (btrim(account_scope) <> ''),
        CONSTRAINT spot_order_set_generation_tenant
            CHECK (tenant_id <> '00000000-0000-0000-0000-000000000000'::UUID),
        CONSTRAINT spot_order_set_generation_id
            CHECK (generation_id <> '00000000-0000-0000-0000-000000000000'::UUID),
        CONSTRAINT spot_order_set_generation_epoch
            CHECK (process_epoch IS NULL OR process_epoch <> '00000000-0000-0000-0000-000000000000'::UUID),
        CONSTRAINT spot_order_set_generation_audit
            CHECK (source_audit_id IS NULL OR source_audit_id <> '00000000-0000-0000-0000-000000000000'::UUID),
        CONSTRAINT spot_order_set_generation_version
            CHECK (accounting_version IS NULL OR accounting_version >= 0),
        CONSTRAINT spot_order_set_generation_shape
            CHECK ((process_epoch IS NULL) = (source_audit_id IS NULL)
                AND (source_audit_id IS NULL) = (accounting_version IS NULL))
    );
    CREATE TABLE public.spot_order_set_sell_intents (
        tenant_id UUID NOT NULL,
        venue_id TEXT NOT NULL,
        account_scope TEXT NOT NULL,
        generation_id UUID NOT NULL,
        intent_id UUID NOT NULL,
        scope_kind TEXT NOT NULL,
        potential BOOLEAN NOT NULL,
        conflict BOOLEAN NOT NULL,
        scope_symbol TEXT,
        scope_position UUID,
        source_position UUID,
        CONSTRAINT spot_order_set_sell_intents_key
            PRIMARY KEY (tenant_id, venue_id, account_scope, generation_id, intent_id),
        CONSTRAINT spot_order_set_sell_intents_venue CHECK (venue_id = 'binance:spot'),
        CONSTRAINT spot_order_set_sell_intents_account
            CHECK (btrim(account_scope) <> ''),
        CONSTRAINT spot_order_set_sell_intents_tenant
            CHECK (tenant_id <> '00000000-0000-0000-0000-000000000000'::UUID),
        CONSTRAINT spot_order_set_sell_intents_generation
            CHECK (generation_id <> '00000000-0000-0000-0000-000000000000'::UUID),
        CONSTRAINT spot_order_set_sell_intents_kind
            CHECK (scope_kind IN ('leg', 'symbol', 'unbound', 'none')),
        CONSTRAINT spot_order_set_sell_intents_shape
            CHECK ((scope_kind = 'leg' AND scope_symbol IS NOT NULL AND scope_position IS NOT NULL)
                OR (scope_kind = 'symbol' AND scope_symbol IS NOT NULL AND scope_position IS NULL)
                OR (scope_kind IN ('unbound', 'none') AND scope_symbol IS NULL AND scope_position IS NULL))
    );
    CREATE TABLE public.spot_order_set_sell_orders (
        tenant_id UUID NOT NULL,
        venue_id TEXT NOT NULL,
        account_scope TEXT NOT NULL,
        generation_id UUID NOT NULL,
        symbol TEXT NOT NULL,
        exchange_order_id TEXT NOT NULL,
        scope_kind TEXT NOT NULL,
        potential BOOLEAN NOT NULL,
        conflict BOOLEAN NOT NULL,
        scope_symbol TEXT,
        scope_position UUID,
        CONSTRAINT spot_order_set_sell_orders_key
            PRIMARY KEY (tenant_id, venue_id, account_scope, generation_id, symbol, exchange_order_id),
        CONSTRAINT spot_order_set_sell_orders_venue CHECK (venue_id = 'binance:spot'),
        CONSTRAINT spot_order_set_sell_orders_account
            CHECK (btrim(account_scope) <> ''),
        CONSTRAINT spot_order_set_sell_orders_tenant
            CHECK (tenant_id <> '00000000-0000-0000-0000-000000000000'::UUID),
        CONSTRAINT spot_order_set_sell_orders_generation
            CHECK (generation_id <> '00000000-0000-0000-0000-000000000000'::UUID),
        CONSTRAINT spot_order_set_sell_orders_kind
            CHECK (scope_kind IN ('leg', 'symbol', 'unbound', 'none')),
        CONSTRAINT spot_order_set_sell_orders_shape
            CHECK ((scope_kind = 'leg' AND scope_symbol IS NOT NULL AND scope_position IS NOT NULL)
                OR (scope_kind = 'symbol' AND scope_symbol IS NOT NULL AND scope_position IS NULL)
                OR (scope_kind IN ('unbound', 'none') AND scope_symbol IS NULL AND scope_position IS NULL))
    );
    CREATE INDEX spot_order_set_sell_intents_unbound_hint
        ON public.spot_order_set_sell_intents
            (tenant_id, venue_id, account_scope, generation_id, source_position, intent_id)
        WHERE scope_kind = 'unbound' AND source_position IS NOT NULL AND (potential OR conflict);
END;
$migration41$;
