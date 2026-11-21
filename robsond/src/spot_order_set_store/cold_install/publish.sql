WITH owner AS MATERIALIZED (
    SELECT a.tenant_id, a.venue_id, v.account_scope, p.epoch_id, a.version
    FROM public.accounting_state a
    JOIN public.venue_accounts v USING (tenant_id, venue_id)
    JOIN LATERAL (
        SELECT epoch_id, clean_stop_at FROM public.process_epochs
        WHERE tenant_id = a.tenant_id
        ORDER BY epoch_seq DESC LIMIT 1
    ) p ON p.epoch_id = $7 AND p.clean_stop_at IS NULL
    WHERE (a.tenant_id, a.venue_id, v.account_scope, v.uid, a.version)
      = ($1, 'binance:spot', $2, $6, $8)
      AND @OBSERVED_LEAVES_MATCH@
), updated AS (
    UPDATE public.spot_order_set_generation g
    SET generation_id = $3, process_epoch = n.epoch_id, source_audit_id = $9,
        accounting_version = n.version, position_hints_complete = true
    FROM owner n
    WHERE (g.tenant_id, g.venue_id, g.account_scope)
            = (n.tenant_id, n.venue_id, n.account_scope)
      AND g.generation_id = $10::uuid AND g.generation_id <> $3
      AND g.source_audit_id IS DISTINCT FROM $9
      AND (g.process_epoch, g.source_audit_id, g.accounting_version, g.position_hints_complete)
            IS NOT DISTINCT FROM ($11::uuid, $12::uuid, $13::bigint, $14::boolean)
    RETURNING g.generation_id
), created AS (
    INSERT INTO public.spot_order_set_generation
        (tenant_id, venue_id, account_scope, generation_id, process_epoch,
         source_audit_id, accounting_version, position_hints_complete)
    SELECT n.tenant_id, n.venue_id, n.account_scope, $3, n.epoch_id, $9, n.version, true
    FROM owner n WHERE $10::uuid IS NULL
       AND NOT EXISTS (
           SELECT 1 FROM public.spot_order_set_generation g
           WHERE (g.tenant_id, g.venue_id, g.account_scope)
                 = (n.tenant_id, n.venue_id, n.account_scope)
       )
    ON CONFLICT (tenant_id, venue_id, account_scope) DO NOTHING
    RETURNING generation_id
)
SELECT generation_id FROM updated
UNION ALL SELECT generation_id FROM created
