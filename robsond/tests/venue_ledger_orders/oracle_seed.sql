-- Test-only. Install and populate the accepted SELL/CANCEL shadow fixtures
-- first, then run this one statement on the same disposable PG16 snapshot.
-- Bind $1 tenant UUID, $2 exact account scope, $3 generation UUID. The caller
-- already seeded the private generation/owner and its exact accounting version.
-- Require exactly one receipt row; no row means the binding, emptiness or
-- source-scope guard refused and nothing was seeded. No generation is issued.
WITH binding AS MATERIALIZED (
    SELECT g.tenant_id, g.venue_id, g.account_scope, g.generation_id
    FROM public.spot_order_set_generation g
    JOIN public.accounting_state a USING (tenant_id, venue_id)
    WHERE (g.tenant_id, g.venue_id, g.account_scope, g.generation_id)
        = ($1::uuid, 'binance:spot', $2::text, $3::uuid)
      AND g.process_epoch IS NOT NULL AND g.source_audit_id IS NOT NULL
      AND g.accounting_version = a.version
      AND NOT EXISTS (
          SELECT 1 FROM public.spot_order_set_sell_intents p
          WHERE (p.tenant_id,p.venue_id,p.account_scope,p.generation_id)
              = (g.tenant_id,g.venue_id,g.account_scope,g.generation_id)
          UNION ALL SELECT 1 FROM public.spot_order_set_sell_orders p
          WHERE (p.tenant_id,p.venue_id,p.account_scope,p.generation_id)
              = (g.tenant_id,g.venue_id,g.account_scope,g.generation_id)
          UNION ALL SELECT 1 FROM public.spot_order_set_sell_legs p
          WHERE (p.tenant_id,p.venue_id,p.account_scope,p.generation_id)
              = (g.tenant_id,g.venue_id,g.account_scope,g.generation_id)
          UNION ALL SELECT 1 FROM public.spot_order_set_sell_symbols p
          WHERE (p.tenant_id,p.venue_id,p.account_scope,p.generation_id)
              = (g.tenant_id,g.venue_id,g.account_scope,g.generation_id)
          UNION ALL SELECT 1 FROM public.spot_order_set_sell_unbound p
          WHERE (p.tenant_id,p.venue_id,p.account_scope,p.generation_id)
              = (g.tenant_id,g.venue_id,g.account_scope,g.generation_id)
          UNION ALL SELECT 1 FROM public.spot_order_set_cancel_members p
          WHERE (p.tenant_id,p.venue_id,p.account_scope,p.generation_id)
              = (g.tenant_id,g.venue_id,g.account_scope,g.generation_id)
          UNION ALL SELECT 1 FROM public.spot_order_set_cancel_groups p
          WHERE (p.tenant_id,p.venue_id,p.account_scope,p.generation_id)
              = (g.tenant_id,g.venue_id,g.account_scope,g.generation_id)
          UNION ALL SELECT 1 FROM public.spot_order_set_cancel_legs p
          WHERE (p.tenant_id,p.venue_id,p.account_scope,p.generation_id)
              = (g.tenant_id,g.venue_id,g.account_scope,g.generation_id)
          UNION ALL SELECT 1 FROM public.spot_order_set_cancel_unbound p
          WHERE (p.tenant_id,p.venue_id,p.account_scope,p.generation_id)
              = (g.tenant_id,g.venue_id,g.account_scope,g.generation_id)
          UNION ALL SELECT 1 FROM public.spot_order_set_cancel_ungroupable p
          WHERE (p.tenant_id,p.venue_id,p.account_scope,p.generation_id)
              = (g.tenant_id,g.venue_id,g.account_scope,g.generation_id)
      )
      AND NOT EXISTS (
          SELECT 1 FROM public.sell_shadow_intents s
          LEFT JOIN public.order_intents j ON j.intent_id = s.intent_id
          WHERE s.scope_kind <> 'none'
            AND ((s.scope_tenant,s.scope_venue,s.scope_account)
                     = (g.tenant_id,g.venue_id,g.account_scope)
                 OR (j.tenant_id,j.venue_id,j.account_scope)
                     = (g.tenant_id,g.venue_id,g.account_scope))
            AND (s.scope_tenant,s.scope_venue,s.scope_account)
                IS DISTINCT FROM (j.tenant_id,j.venue_id,j.account_scope)
      )
      AND NOT EXISTS (
          SELECT 1 FROM public.sell_shadow_orders s
          WHERE (s.tenant_id,s.venue_id,s.account_scope) = (g.tenant_id,g.venue_id,g.account_scope)
            AND s.scope_kind <> 'none'
            AND (s.scope_tenant,s.scope_venue,s.scope_account)
                IS DISTINCT FROM (s.tenant_id,s.venue_id,s.account_scope)
      )
), sell_intents AS (
    -- A discharged J shadow row has NULL classification scope. Its key and
    -- original position hint still come from J, never from the linked order.
    INSERT INTO public.spot_order_set_sell_intents (
        tenant_id, venue_id, account_scope, generation_id, intent_id,
        scope_kind, potential, conflict, scope_symbol, scope_position, source_position
    )
    SELECT b.tenant_id, b.venue_id, b.account_scope, b.generation_id, s.intent_id,
           s.scope_kind, s.potential, s.conflict, s.scope_symbol, s.scope_position, j.position_id
    FROM binding b
    JOIN public.order_intents j USING (tenant_id, venue_id, account_scope)
    JOIN public.sell_shadow_intents s ON s.intent_id = j.intent_id
    RETURNING 1
), sell_orders AS (
    -- Exact SELL pairs live under the native key; mismatched J/native rows
    -- retain their distinct keys and classifications.
    INSERT INTO public.spot_order_set_sell_orders (
        tenant_id, venue_id, account_scope, generation_id, symbol, exchange_order_id,
        scope_kind, potential, conflict, scope_symbol, scope_position
    )
    SELECT b.tenant_id, b.venue_id, b.account_scope, b.generation_id, s.symbol, s.exchange_order_id,
           s.scope_kind, s.potential, s.conflict, s.scope_symbol, s.scope_position
    FROM binding b JOIN public.sell_shadow_orders s USING (tenant_id, venue_id, account_scope)
    RETURNING 1
), sell_legs AS (
    INSERT INTO public.spot_order_set_sell_legs (
        tenant_id, venue_id, account_scope, generation_id, symbol, position_id,
        potential_count, conflict_count
    )
    SELECT b.tenant_id, b.venue_id, b.account_scope, b.generation_id, s.symbol, s.position_id,
           s.potential_sell, s.conflict_count
    FROM binding b JOIN public.sell_shadow_legs s USING (tenant_id, venue_id, account_scope)
    RETURNING 1
), sell_symbols AS (
    INSERT INTO public.spot_order_set_sell_symbols (
        tenant_id, venue_id, account_scope, generation_id, symbol, potential_count, conflict_count
    )
    SELECT b.tenant_id, b.venue_id, b.account_scope, b.generation_id, s.symbol,
           s.potential_sell, s.conflict_count
    FROM binding b JOIN public.sell_shadow_symbols s USING (tenant_id, venue_id, account_scope)
    RETURNING 1
), sell_unbound AS (
    INSERT INTO public.spot_order_set_sell_unbound (
        tenant_id, venue_id, account_scope, generation_id, potential_count, conflict_count
    )
    SELECT b.tenant_id, b.venue_id, b.account_scope, b.generation_id, s.potential_sell, s.conflict_count
    FROM binding b JOIN public.sell_shadow_unbound s USING (tenant_id, venue_id, account_scope)
    RETURNING 1
), cancel_members AS (
    -- An unresolved shadow hint may have position_id without symbol. M43
    -- requires both leg coordinates or neither. The target and flags survive.
    INSERT INTO public.spot_order_set_cancel_members (
        tenant_id, venue_id, account_scope, generation_id, intent_id,
        symbol, position_id, target_id, pending, conflict
    )
    SELECT b.tenant_id, b.venue_id, b.account_scope, b.generation_id, s.intent_id,
           CASE WHEN s.position_id IS NOT NULL THEN s.symbol END,
           CASE WHEN s.symbol IS NOT NULL THEN s.position_id END,
           s.target_id, s.pending, s.conflict
    FROM binding b JOIN public.cancel_shadow_members s USING (tenant_id, venue_id, account_scope)
    RETURNING 1
), cancel_groups AS (
    INSERT INTO public.spot_order_set_cancel_groups (
        tenant_id, venue_id, account_scope, generation_id, symbol, position_id, target_id,
        pending_count, conflict_count, terminal_mask
    )
    SELECT b.tenant_id, b.venue_id, b.account_scope, b.generation_id, s.symbol, s.position_id,
           s.target_id, s.pending_count, s.conflict_count, s.terminal_mask
    FROM binding b JOIN public.cancel_shadow_groups s USING (tenant_id, venue_id, account_scope)
    RETURNING 1
), cancel_legs AS (
    INSERT INTO public.spot_order_set_cancel_legs (
        tenant_id, venue_id, account_scope, generation_id, symbol, position_id, active_count, conflict_count
    )
    SELECT b.tenant_id, b.venue_id, b.account_scope, b.generation_id, s.symbol, s.position_id,
           s.active_count, s.conflict_count
    FROM binding b JOIN public.cancel_shadow_legs s USING (tenant_id, venue_id, account_scope)
    RETURNING 1
), cancel_unbound AS (
    INSERT INTO public.spot_order_set_cancel_unbound (
        tenant_id, venue_id, account_scope, generation_id, active_count, conflict_count
    )
    SELECT b.tenant_id, b.venue_id, b.account_scope, b.generation_id, s.active_count, s.conflict_count
    FROM binding b JOIN public.cancel_shadow_unbound s USING (tenant_id, venue_id, account_scope)
    RETURNING 1
), cancel_ungroupable AS (
    INSERT INTO public.spot_order_set_cancel_ungroupable (
        tenant_id, venue_id, account_scope, generation_id, symbol, position_id, active_count, conflict_count
    )
    SELECT b.tenant_id, b.venue_id, b.account_scope, b.generation_id, s.symbol, s.position_id,
           s.active_count, s.conflict_count
    FROM binding b JOIN public.cancel_shadow_ungroupable s USING (tenant_id, venue_id, account_scope)
    RETURNING 1
)
SELECT b.generation_id,
       (SELECT count(*) FROM sell_intents) AS sell_intents,
       (SELECT count(*) FROM sell_orders) AS sell_orders,
       (SELECT count(*) FROM sell_legs) AS sell_legs,
       (SELECT count(*) FROM sell_symbols) AS sell_symbols,
       (SELECT count(*) FROM sell_unbound) AS sell_unbound,
       (SELECT count(*) FROM cancel_members) AS cancel_members,
       (SELECT count(*) FROM cancel_groups) AS cancel_groups,
       (SELECT count(*) FROM cancel_legs) AS cancel_legs,
       (SELECT count(*) FROM cancel_unbound) AS cancel_unbound,
       (SELECT count(*) FROM cancel_ungroupable) AS cancel_ungroupable
FROM binding b;
