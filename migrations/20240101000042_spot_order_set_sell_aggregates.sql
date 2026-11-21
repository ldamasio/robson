DO $migration42$
BEGIN
    CREATE TABLE public.spot_order_set_sell_legs (
        tenant_id UUID NOT NULL,
        venue_id TEXT NOT NULL,
        account_scope TEXT NOT NULL,
        generation_id UUID NOT NULL,
        symbol TEXT NOT NULL,
        position_id UUID NOT NULL,
        potential_count BIGINT NOT NULL,
        conflict_count BIGINT NOT NULL,
        CONSTRAINT spot_order_set_sell_legs_key
            PRIMARY KEY (tenant_id, venue_id, account_scope, generation_id, symbol, position_id),
        CONSTRAINT spot_order_set_sell_legs_venue CHECK (venue_id = 'binance:spot'),
        CONSTRAINT spot_order_set_sell_legs_account
            CHECK (btrim(account_scope) <> ''),
        CONSTRAINT spot_order_set_sell_legs_tenant
            CHECK (tenant_id <> '00000000-0000-0000-0000-000000000000'::UUID),
        CONSTRAINT spot_order_set_sell_legs_generation
            CHECK (generation_id <> '00000000-0000-0000-0000-000000000000'::UUID),
        CONSTRAINT spot_order_set_sell_legs_potential_count CHECK (potential_count >= 0),
        CONSTRAINT spot_order_set_sell_legs_conflict_count CHECK (conflict_count >= 0)
    );
    CREATE TABLE public.spot_order_set_sell_symbols (
        tenant_id UUID NOT NULL,
        venue_id TEXT NOT NULL,
        account_scope TEXT NOT NULL,
        generation_id UUID NOT NULL,
        symbol TEXT NOT NULL,
        potential_count BIGINT NOT NULL,
        conflict_count BIGINT NOT NULL,
        CONSTRAINT spot_order_set_sell_symbols_key
            PRIMARY KEY (tenant_id, venue_id, account_scope, generation_id, symbol),
        CONSTRAINT spot_order_set_sell_symbols_venue CHECK (venue_id = 'binance:spot'),
        CONSTRAINT spot_order_set_sell_symbols_account
            CHECK (btrim(account_scope) <> ''),
        CONSTRAINT spot_order_set_sell_symbols_tenant
            CHECK (tenant_id <> '00000000-0000-0000-0000-000000000000'::UUID),
        CONSTRAINT spot_order_set_sell_symbols_generation
            CHECK (generation_id <> '00000000-0000-0000-0000-000000000000'::UUID),
        CONSTRAINT spot_order_set_sell_symbols_potential_count CHECK (potential_count >= 0),
        CONSTRAINT spot_order_set_sell_symbols_conflict_count CHECK (conflict_count >= 0)
    );
    CREATE TABLE public.spot_order_set_sell_unbound (
        tenant_id UUID NOT NULL,
        venue_id TEXT NOT NULL,
        account_scope TEXT NOT NULL,
        generation_id UUID NOT NULL,
        potential_count BIGINT NOT NULL,
        conflict_count BIGINT NOT NULL,
        CONSTRAINT spot_order_set_sell_unbound_key
            PRIMARY KEY (tenant_id, venue_id, account_scope, generation_id),
        CONSTRAINT spot_order_set_sell_unbound_venue CHECK (venue_id = 'binance:spot'),
        CONSTRAINT spot_order_set_sell_unbound_account
            CHECK (btrim(account_scope) <> ''),
        CONSTRAINT spot_order_set_sell_unbound_tenant
            CHECK (tenant_id <> '00000000-0000-0000-0000-000000000000'::UUID),
        CONSTRAINT spot_order_set_sell_unbound_generation
            CHECK (generation_id <> '00000000-0000-0000-0000-000000000000'::UUID),
        CONSTRAINT spot_order_set_sell_unbound_potential_count CHECK (potential_count >= 0),
        CONSTRAINT spot_order_set_sell_unbound_conflict_count CHECK (conflict_count >= 0)
    );
END;
$migration42$;
