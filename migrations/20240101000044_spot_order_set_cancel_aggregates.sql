DO $migration44$
BEGIN
    CREATE TABLE public.spot_order_set_cancel_unbound (
        tenant_id UUID NOT NULL,
        venue_id TEXT NOT NULL,
        account_scope TEXT NOT NULL,
        generation_id UUID NOT NULL,
        active_count BIGINT NOT NULL,
        conflict_count BIGINT NOT NULL,
        CONSTRAINT spot_order_set_cancel_unbound_key
            PRIMARY KEY (tenant_id, venue_id, account_scope, generation_id),
        CONSTRAINT spot_order_set_cancel_unbound_venue CHECK (venue_id = 'binance:spot'),
        CONSTRAINT spot_order_set_cancel_unbound_account
            CHECK (btrim(account_scope) <> ''),
        CONSTRAINT spot_order_set_cancel_unbound_tenant
            CHECK (tenant_id <> '00000000-0000-0000-0000-000000000000'::UUID),
        CONSTRAINT spot_order_set_cancel_unbound_generation
            CHECK (generation_id <> '00000000-0000-0000-0000-000000000000'::UUID),
        CONSTRAINT spot_order_set_cancel_unbound_active_count CHECK (active_count >= 0),
        CONSTRAINT spot_order_set_cancel_unbound_conflict_count CHECK (conflict_count >= 0)
    );
    CREATE TABLE public.spot_order_set_cancel_ungroupable (
        tenant_id UUID NOT NULL,
        venue_id TEXT NOT NULL,
        account_scope TEXT NOT NULL,
        generation_id UUID NOT NULL,
        symbol TEXT NOT NULL,
        position_id UUID NOT NULL,
        active_count BIGINT NOT NULL,
        conflict_count BIGINT NOT NULL,
        CONSTRAINT spot_order_set_cancel_ungroupable_key
            PRIMARY KEY (tenant_id, venue_id, account_scope, generation_id, symbol, position_id),
        CONSTRAINT spot_order_set_cancel_ungroupable_venue CHECK (venue_id = 'binance:spot'),
        CONSTRAINT spot_order_set_cancel_ungroupable_account
            CHECK (btrim(account_scope) <> ''),
        CONSTRAINT spot_order_set_cancel_ungroupable_tenant
            CHECK (tenant_id <> '00000000-0000-0000-0000-000000000000'::UUID),
        CONSTRAINT spot_order_set_cancel_ungroupable_generation
            CHECK (generation_id <> '00000000-0000-0000-0000-000000000000'::UUID),
        CONSTRAINT spot_order_set_cancel_ungroupable_active_count CHECK (active_count >= 0),
        CONSTRAINT spot_order_set_cancel_ungroupable_conflict_count CHECK (conflict_count >= 0)
    );
END;
$migration44$;
