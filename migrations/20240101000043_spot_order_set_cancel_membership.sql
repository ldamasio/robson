DO $migration43$
BEGIN
    CREATE TABLE public.spot_order_set_cancel_members (
        tenant_id UUID NOT NULL,
        venue_id TEXT NOT NULL,
        account_scope TEXT NOT NULL,
        generation_id UUID NOT NULL,
        intent_id UUID NOT NULL,
        symbol TEXT,
        position_id UUID,
        target_id TEXT,
        pending BOOLEAN NOT NULL,
        conflict BOOLEAN NOT NULL,
        CONSTRAINT spot_order_set_cancel_members_key
            PRIMARY KEY (tenant_id, venue_id, account_scope, generation_id, intent_id),
        CONSTRAINT spot_order_set_cancel_members_venue CHECK (venue_id = 'binance:spot'),
        CONSTRAINT spot_order_set_cancel_members_account
            CHECK (btrim(account_scope) <> ''),
        CONSTRAINT spot_order_set_cancel_members_tenant
            CHECK (tenant_id <> '00000000-0000-0000-0000-000000000000'::UUID),
        CONSTRAINT spot_order_set_cancel_members_generation
            CHECK (generation_id <> '00000000-0000-0000-0000-000000000000'::UUID),
        CONSTRAINT spot_order_set_cancel_members_leg
            CHECK ((symbol IS NULL) = (position_id IS NULL))
    );
    CREATE TABLE public.spot_order_set_cancel_groups (
        tenant_id UUID NOT NULL,
        venue_id TEXT NOT NULL,
        account_scope TEXT NOT NULL,
        generation_id UUID NOT NULL,
        symbol TEXT NOT NULL,
        position_id UUID NOT NULL,
        target_id TEXT NOT NULL,
        pending_count BIGINT NOT NULL,
        conflict_count BIGINT NOT NULL,
        terminal_mask BOOLEAN NOT NULL,
        CONSTRAINT spot_order_set_cancel_groups_key
            PRIMARY KEY (tenant_id, venue_id, account_scope, generation_id, symbol, position_id, target_id),
        CONSTRAINT spot_order_set_cancel_groups_venue CHECK (venue_id = 'binance:spot'),
        CONSTRAINT spot_order_set_cancel_groups_account
            CHECK (btrim(account_scope) <> ''),
        CONSTRAINT spot_order_set_cancel_groups_tenant
            CHECK (tenant_id <> '00000000-0000-0000-0000-000000000000'::UUID),
        CONSTRAINT spot_order_set_cancel_groups_generation
            CHECK (generation_id <> '00000000-0000-0000-0000-000000000000'::UUID),
        CONSTRAINT spot_order_set_cancel_groups_pending_count CHECK (pending_count >= 0),
        CONSTRAINT spot_order_set_cancel_groups_conflict_count CHECK (conflict_count >= 0)
    );
    CREATE TABLE public.spot_order_set_cancel_legs (
        tenant_id UUID NOT NULL,
        venue_id TEXT NOT NULL,
        account_scope TEXT NOT NULL,
        generation_id UUID NOT NULL,
        symbol TEXT NOT NULL,
        position_id UUID NOT NULL,
        active_count BIGINT NOT NULL,
        conflict_count BIGINT NOT NULL,
        CONSTRAINT spot_order_set_cancel_legs_key
            PRIMARY KEY (tenant_id, venue_id, account_scope, generation_id, symbol, position_id),
        CONSTRAINT spot_order_set_cancel_legs_venue CHECK (venue_id = 'binance:spot'),
        CONSTRAINT spot_order_set_cancel_legs_account
            CHECK (btrim(account_scope) <> ''),
        CONSTRAINT spot_order_set_cancel_legs_tenant
            CHECK (tenant_id <> '00000000-0000-0000-0000-000000000000'::UUID),
        CONSTRAINT spot_order_set_cancel_legs_generation
            CHECK (generation_id <> '00000000-0000-0000-0000-000000000000'::UUID),
        CONSTRAINT spot_order_set_cancel_legs_active_count CHECK (active_count >= 0),
        CONSTRAINT spot_order_set_cancel_legs_conflict_count CHECK (conflict_count >= 0)
    );
END;
$migration43$;
