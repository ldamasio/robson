-- The caller already owns the tenant/venue accounting row and has inserted the
-- fresh cancel and its transition in this same transaction. Exact keys only.
WITH cancel AS MATERIALIZED (
    SELECT j.intent_id, j.tenant_id, j.venue_id, j.account_scope, j.position_id,
           j.cancels_order_id AS target_id, l.symbol, g.generation_id
    FROM public.order_intents j
    JOIN public.balance_legs l
      ON (l.position_id, l.tenant_id, l.venue_id, l.account_scope)
       = (j.position_id, j.tenant_id, j.venue_id, j.account_scope)
    JOIN public.spot_order_set_generation g
      ON (g.tenant_id, g.venue_id, g.account_scope) = (j.tenant_id, j.venue_id, j.account_scope)
    JOIN LATERAL (
        SELECT epoch_id, clean_stop_at FROM public.process_epochs
        WHERE tenant_id = g.tenant_id ORDER BY epoch_seq DESC LIMIT 1
    ) epoch ON epoch.epoch_id = g.process_epoch AND epoch.clean_stop_at IS NULL
    WHERE j.intent_id = $7 AND (j.tenant_id, j.venue_id, j.account_scope) = ($1, 'binance:spot', $2)
      AND (g.generation_id, g.process_epoch, g.source_audit_id) = ($3, $4, $5)
      AND g.accounting_version = $6::bigint - 1
      AND j.position_id = $8 AND j.cancels_order_id = $9 AND l.symbol = $10
      AND j.kind = 'cancel' AND ledger_canonical_id(j.cancels_order_id)
      -- A fresh first insertion: one pending, zero conflicts.
      AND NOT j.not_sent AND j.venue_outcome IS NULL AND j.venue_evidence IS NULL
      AND NOT EXISTS (SELECT 1 FROM public.venue_orders o WHERE o.intent_id = j.intent_id)
), evidence AS MATERIALIZED (
    -- cancel_shadow_sync_target's qualified terminal proof, from the exact
    -- target PK and its original journal row. Unknown evidence is no mask.
    SELECT c.*,
           coalesce((
               SELECT o.owner = 'leg'
                  AND (o.tenant_id, o.position_id) = (c.tenant_id, c.position_id)
                  AND o.phase = 'terminal'
                  AND o.status IN ('FILLED', 'CANCELED', 'EXPIRED', 'EXPIRED_IN_MATCH', 'REJECTED')
                  AND (t.tenant_id, t.venue_id, t.account_scope, t.position_id, t.client_order_id)
                    = (o.tenant_id, o.venue_id, o.account_scope, o.position_id, o.client_order_id)
                  AND NOT t.not_sent
                  AND o.kind = CASE t.kind WHEN 'entry' THEN 'entry'
                                           WHEN 'protection' THEN 'stop' WHEN 'exit' THEN 'sell' END
               FROM public.venue_orders o
               JOIN public.order_intents t ON t.intent_id = o.intent_id
               WHERE (o.venue_id, o.account_scope, o.symbol, o.exchange_order_id)
                   = (c.venue_id, c.account_scope, c.symbol, c.target_id)
           ), false) AS terminal_proof
    FROM cancel c
), prior AS MATERIALIZED (
    SELECT e.*,
           coalesce(p.pending_count, 0) AS pending_count,
           coalesce(p.conflict_count, 0) AS conflict_count
    FROM evidence e
    LEFT JOIN public.spot_order_set_cancel_groups p
      ON (p.tenant_id, p.venue_id, p.account_scope, p.generation_id, p.symbol, p.position_id, p.target_id)
       = (e.tenant_id, e.venue_id, e.account_scope, e.generation_id, e.symbol, e.position_id, e.target_id)
    -- This command never changes a mask: refuse a discrepancy.
    WHERE coalesce(p.terminal_mask, false) = e.terminal_proof
), member_write AS (
    INSERT INTO public.spot_order_set_cancel_members (
        tenant_id, venue_id, account_scope, generation_id, intent_id, symbol, position_id,
        source_position, target_id, pending, conflict
    )
    SELECT tenant_id, venue_id, account_scope, generation_id, intent_id, symbol, position_id,
           position_id, target_id, true, false
    FROM prior
    ON CONFLICT (tenant_id, venue_id, account_scope, generation_id, intent_id) DO NOTHING
    RETURNING intent_id
), group_write AS (
    INSERT INTO public.spot_order_set_cancel_groups (
        tenant_id, venue_id, account_scope, generation_id, symbol, position_id, target_id,
        pending_count, conflict_count, terminal_mask
    )
    SELECT tenant_id, venue_id, account_scope, generation_id, symbol, position_id, target_id,
           pending_count + 1, conflict_count, terminal_proof
    FROM prior
    ON CONFLICT (tenant_id, venue_id, account_scope, generation_id, symbol, position_id, target_id)
    DO UPDATE SET pending_count = excluded.pending_count
    WHERE spot_order_set_cancel_groups.pending_count = excluded.pending_count - 1
      AND spot_order_set_cancel_groups.conflict_count = excluded.conflict_count
      AND spot_order_set_cancel_groups.terminal_mask = excluded.terminal_mask
    RETURNING target_id
), leg_write AS (
    -- A terminal group keeps its pending increment without an active leg.
    INSERT INTO public.spot_order_set_cancel_legs (
        tenant_id, venue_id, account_scope, generation_id, symbol, position_id,
        active_count, conflict_count
    )
    SELECT tenant_id, venue_id, account_scope, generation_id, symbol, position_id, 1, 0
    FROM prior WHERE NOT terminal_proof
    ON CONFLICT (tenant_id, venue_id, account_scope, generation_id, symbol, position_id)
    DO UPDATE SET active_count = spot_order_set_cancel_legs.active_count + 1
    RETURNING position_id
)
UPDATE public.spot_order_set_generation g
SET accounting_version = $6
FROM prior c
WHERE (g.tenant_id, g.venue_id, g.account_scope, g.generation_id)
    = (c.tenant_id, c.venue_id, c.account_scope, c.generation_id)
  AND g.accounting_version = $6::bigint - 1
  AND (SELECT count(*) FROM member_write) = 1
  AND (SELECT count(*) FROM group_write) = 1
  AND (SELECT count(*) FROM leg_write) = (NOT c.terminal_proof)::integer
RETURNING g.accounting_version
