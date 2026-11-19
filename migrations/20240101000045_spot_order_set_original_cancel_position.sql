DO $migration45$
BEGIN
    ALTER TABLE public.spot_order_set_cancel_members ADD COLUMN source_position UUID;
    CREATE INDEX idx_spot_cancel_unbound_original_position
        ON public.spot_order_set_cancel_members
            (tenant_id, venue_id, account_scope, generation_id, source_position, intent_id)
        WHERE position_id IS NULL AND source_position IS NOT NULL;
    ALTER TABLE public.spot_order_set_generation
        ADD COLUMN position_hints_complete BOOLEAN NOT NULL DEFAULT false;
END;
$migration45$;
