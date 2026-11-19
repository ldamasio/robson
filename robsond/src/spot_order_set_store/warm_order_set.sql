-- One structural warm observation. $1..$6 are the exact ExpectedGeneration
-- binding, $7/$8 the original leg, $9 the optional candidate order ID. A stale
-- or unknown owner, or an absent leg, yields no row. Missing aggregates and
-- targets stay NULL; nothing here infers zeros, readiness or authority.
SELECT g.generation_id, g.process_epoch, g.source_audit_id, a.version AS accounting_version,
       l.position_id, l.symbol,
       l.gross_qty, l.acquisition_cost, l.base_commission, l.base_commission_final, l.executable_qty,
       l.consumed_cum, l.target_trigger,
       sl.potential_count AS leg_sell, sl.conflict_count AS leg_sell_conflicts,
       sy.potential_count AS symbol_sell, sy.conflict_count AS symbol_sell_conflicts,
       su.potential_count AS unbound_sell, su.conflict_count AS unbound_sell_conflicts,
       cl.active_count AS grouped_cancel, cl.conflict_count AS grouped_cancel_conflicts,
       cg.active_count AS ungroupable_cancel, cg.conflict_count AS ungroupable_cancel_conflicts,
       cu.active_count AS unbound_cancel, cu.conflict_count AS unbound_cancel_conflicts,
       m.scope_kind AS candidate_scope, m.scope_position AS candidate_position,
       m.potential AS candidate_potential, m.conflict AS candidate_conflict,
       tg.pending_count AS target_cancel_pending, tg.conflict_count AS target_cancel_conflicts,
       tg.terminal_mask AS target_cancel_terminal,
       stop.exchange_order_id AS stop_id, stop.phase AS stop_phase, stop.status AS stop_status,
       stop.side AS stop_side, stop.order_type AS stop_type, stop.orig_qty AS stop_qty,
       stop.executed_cum AS stop_executed, stop.stop_price AS stop_trigger,
       coalesce(stop.tenant_id = l.tenant_id AND stop.position_id = l.position_id
           AND stop.owner = 'leg' AND stop.kind = 'stop' AND stop.side = 'SELL'
           AND (j.tenant_id, j.venue_id, j.account_scope, j.position_id, j.client_order_id)
               = (stop.tenant_id, stop.venue_id, stop.account_scope, stop.position_id,
                  stop.client_order_id)
           AND j.kind = 'protection' AND NOT j.not_sent
           AND j.venue_outcome IS DISTINCT FROM 'rejected', false) AS exact_stop_identity
FROM public.spot_order_set_generation AS g
JOIN public.accounting_state AS a
    ON (a.tenant_id, a.venue_id) = (g.tenant_id, g.venue_id)
JOIN public.balance_legs AS l
    ON (l.tenant_id, l.venue_id, l.account_scope, l.position_id, l.symbol)
        = (g.tenant_id, g.venue_id, g.account_scope, $7, $8)
LEFT JOIN public.spot_order_set_sell_legs AS sl
    ON (sl.tenant_id, sl.venue_id, sl.account_scope, sl.generation_id, sl.symbol, sl.position_id)
        = (g.tenant_id, g.venue_id, g.account_scope, g.generation_id, l.symbol, l.position_id)
LEFT JOIN public.spot_order_set_sell_symbols AS sy
    ON (sy.tenant_id, sy.venue_id, sy.account_scope, sy.generation_id, sy.symbol)
        = (g.tenant_id, g.venue_id, g.account_scope, g.generation_id, l.symbol)
LEFT JOIN public.spot_order_set_sell_unbound AS su
    ON (su.tenant_id, su.venue_id, su.account_scope, su.generation_id)
        = (g.tenant_id, g.venue_id, g.account_scope, g.generation_id)
LEFT JOIN public.spot_order_set_cancel_legs AS cl
    ON (cl.tenant_id, cl.venue_id, cl.account_scope, cl.generation_id, cl.symbol, cl.position_id)
        = (g.tenant_id, g.venue_id, g.account_scope, g.generation_id, l.symbol, l.position_id)
LEFT JOIN public.spot_order_set_cancel_ungroupable AS cg
    ON (cg.tenant_id, cg.venue_id, cg.account_scope, cg.generation_id, cg.symbol, cg.position_id)
        = (g.tenant_id, g.venue_id, g.account_scope, g.generation_id, l.symbol, l.position_id)
LEFT JOIN public.spot_order_set_cancel_unbound AS cu
    ON (cu.tenant_id, cu.venue_id, cu.account_scope, cu.generation_id)
        = (g.tenant_id, g.venue_id, g.account_scope, g.generation_id)
LEFT JOIN public.spot_order_set_sell_orders AS m
    ON (m.tenant_id, m.venue_id, m.account_scope, m.generation_id, m.symbol, m.exchange_order_id)
        = (g.tenant_id, g.venue_id, g.account_scope, g.generation_id, l.symbol, $9::text)
LEFT JOIN public.spot_order_set_cancel_groups AS tg
    ON (tg.tenant_id, tg.venue_id, tg.account_scope, tg.generation_id, tg.symbol, tg.position_id,
        tg.target_id)
        = (g.tenant_id, g.venue_id, g.account_scope, g.generation_id, l.symbol, l.position_id,
           $9::text)
LEFT JOIN public.venue_orders AS stop
    ON (stop.tenant_id, stop.venue_id, stop.account_scope, stop.symbol, stop.exchange_order_id)
        = (l.tenant_id, l.venue_id, l.account_scope, l.symbol, $9::text)
LEFT JOIN public.order_intents AS j
    ON (j.intent_id, j.tenant_id, j.venue_id, j.account_scope)
        = (stop.intent_id, l.tenant_id, l.venue_id, l.account_scope)
WHERE (g.tenant_id, g.venue_id, g.account_scope) = ($1, $2, $3)
  AND (g.generation_id, g.process_epoch, g.source_audit_id) = ($4, $5, $6)
  AND g.accounting_version = a.version
