-- One already-admitted leg and at most the one supplied exact candidate key.
-- Test fixtures are initialized before enabled writes. An empty summary here
-- does not supply evidence of initialization or any runtime authority.
SELECT l.position_id, a.version AS accounting_version,
       l.gross_qty, l.acquisition_cost, l.base_commission, l.base_commission_final, l.executable_qty,
       l.consumed_cum, l.target_trigger,
       coalesce(sl.potential_sell, 0::bigint) AS leg_sell,
       coalesce(sl.conflict_count, 0::bigint) AS leg_sell_conflicts,
       coalesce(sy.potential_sell, 0::bigint) AS symbol_sell,
       coalesce(sy.conflict_count, 0::bigint) AS symbol_sell_conflicts,
       coalesce(cl.active_count, 0::bigint) AS grouped_cancel,
       coalesce(cl.conflict_count, 0::bigint) AS grouped_cancel_conflicts,
       coalesce(cg.active_count, 0::bigint) AS ungroupable_cancel,
       coalesce(cg.conflict_count, 0::bigint) AS ungroupable_cancel_conflicts,
       coalesce(cu.active_count, 0::bigint) AS unbound_cancel,
       coalesce(cu.conflict_count, 0::bigint) AS unbound_cancel_conflicts,
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
FROM balance_legs AS l
JOIN accounting_state AS a
    ON (a.tenant_id, a.venue_id) = (l.tenant_id, l.venue_id)
LEFT JOIN sell_shadow_legs AS sl
    ON (sl.tenant_id, sl.venue_id, sl.account_scope, sl.symbol, sl.position_id)
        = (l.tenant_id, l.venue_id, l.account_scope, l.symbol, l.position_id)
LEFT JOIN sell_shadow_symbols AS sy
    ON (sy.tenant_id, sy.venue_id, sy.account_scope, sy.symbol)
        = (l.tenant_id, l.venue_id, l.account_scope, l.symbol)
LEFT JOIN cancel_shadow_legs AS cl
    ON (cl.tenant_id, cl.venue_id, cl.account_scope, cl.symbol, cl.position_id)
        = (l.tenant_id, l.venue_id, l.account_scope, l.symbol, l.position_id)
LEFT JOIN cancel_shadow_ungroupable AS cg
    ON (cg.tenant_id, cg.venue_id, cg.account_scope, cg.symbol, cg.position_id)
        = (l.tenant_id, l.venue_id, l.account_scope, l.symbol, l.position_id)
LEFT JOIN cancel_shadow_unbound AS cu
    ON (cu.tenant_id, cu.venue_id, cu.account_scope) = (l.tenant_id, l.venue_id, l.account_scope)
LEFT JOIN venue_orders AS stop
    ON (stop.venue_id, stop.account_scope, stop.symbol, stop.exchange_order_id)
        = (l.venue_id, l.account_scope, l.symbol, $6::text)
LEFT JOIN order_intents AS j ON j.intent_id = stop.intent_id
WHERE l.tenant_id = $1 AND l.venue_id = $2 AND l.account_scope = $3
  AND l.position_id = $4 AND l.symbol = $5
