-- Part 4: every check precedes every write. Each touched key is either one
-- compare-and-set UPDATE of its read prior or one INSERT of an absent key,
-- never both, with final nonnegative values; nothing is deleted, so a known
-- zero stays distinct from absence. The version advances only when every
-- planned row was written exactly once.
, verdict AS MATERIALIZED (
    SELECT (SELECT count(*) FROM links) = 1
       AND NOT EXISTS (SELECT 1 FROM sell_order_plan WHERE NOT ok)
       AND NOT EXISTS (SELECT 1 FROM sell_intent_plan WHERE NOT ok)
       AND NOT EXISTS (SELECT 1 FROM cancel_member_plan WHERE NOT ok)
       AND NOT EXISTS (SELECT 1 FROM cancel_group_plan WHERE NOT ok)
       AND NOT EXISTS (SELECT 1 FROM sell_aggregates WHERE potential_next < 0 OR conflict_next < 0)
       AND NOT EXISTS (SELECT 1 FROM cancel_aggregates WHERE active_next < 0 OR conflict_next < 0)
       AS ok
), planned AS MATERIALIZED (
    SELECT (SELECT count(*) FROM sell_order_plan WHERE touch)
         + (SELECT count(*) FROM sell_intent_plan WHERE touch)
         + (SELECT count(*) FROM cancel_member_plan WHERE touch)
         + (SELECT count(*) FROM cancel_group_plan WHERE touch)
         + (SELECT count(*) FROM sell_aggregates)
         + (SELECT count(*) FROM cancel_aggregates) AS planned_rows
), sell_order_update AS (
    UPDATE public.spot_order_set_sell_orders m
    SET scope_kind = p.n_kind, scope_symbol = p.n_symbol, scope_position = p.n_position,
        potential = p.n_potential, conflict = p.n_conflict
    FROM sell_order_plan p, verdict v
    WHERE v.ok AND p.touch AND p.stored
      AND (m.tenant_id, m.venue_id, m.account_scope, m.generation_id, m.symbol, m.exchange_order_id)
        = (p.tenant_id, p.venue_id, p.account_scope, p.generation_id, p.symbol, p.exchange_order_id)
      AND (m.scope_kind, m.scope_symbol, m.scope_position, m.potential, m.conflict)
          IS NOT DISTINCT FROM (p.s_kind, p.s_symbol, p.s_position, p.s_potential, p.s_conflict)
    RETURNING 1
), sell_order_insert AS (
    INSERT INTO public.spot_order_set_sell_orders (
        tenant_id, venue_id, account_scope, generation_id, symbol, exchange_order_id,
        scope_kind, potential, conflict, scope_symbol, scope_position
    )
    SELECT p.tenant_id, p.venue_id, p.account_scope, p.generation_id, p.symbol, p.exchange_order_id,
           p.n_kind, p.n_potential, p.n_conflict, p.n_symbol, p.n_position
    FROM sell_order_plan p, verdict v
    WHERE v.ok AND p.touch AND NOT p.stored
    RETURNING 1
), sell_intent_update AS (
    UPDATE public.spot_order_set_sell_intents m
    SET scope_kind = p.n_kind, scope_symbol = p.n_symbol, scope_position = p.n_position,
        potential = p.n_active, conflict = p.n_active
    FROM sell_intent_plan p, verdict v
    WHERE v.ok AND p.touch AND p.stored
      AND (m.tenant_id, m.venue_id, m.account_scope, m.generation_id, m.intent_id)
        = (p.tenant_id, p.venue_id, p.account_scope, p.generation_id, p.intent_id)
      AND (m.scope_kind, m.scope_symbol, m.scope_position, m.source_position, m.potential, m.conflict)
          IS NOT DISTINCT FROM (p.s_kind, p.s_symbol, p.s_position, p.s_source, p.s_potential, p.s_conflict)
    RETURNING 1
), sell_intent_insert AS (
    INSERT INTO public.spot_order_set_sell_intents (
        tenant_id, venue_id, account_scope, generation_id, intent_id, source_position,
        scope_kind, potential, conflict, scope_symbol, scope_position
    )
    SELECT p.tenant_id, p.venue_id, p.account_scope, p.generation_id, p.intent_id, p.j_position,
           p.n_kind, p.n_active, p.n_active, p.n_symbol, p.n_position
    FROM sell_intent_plan p, verdict v
    WHERE v.ok AND p.touch AND NOT p.stored
    RETURNING 1
), cancel_member_update AS (
    UPDATE public.spot_order_set_cancel_members m
    SET pending = true, conflict = true
    FROM cancel_member_plan p, verdict v
    WHERE v.ok AND p.touch AND p.stored
      AND (m.tenant_id, m.venue_id, m.account_scope, m.generation_id, m.intent_id)
        = (p.tenant_id, p.venue_id, p.account_scope, p.generation_id, p.intent_id)
      AND (m.symbol, m.position_id, m.target_id, m.pending, m.conflict)
          IS NOT DISTINCT FROM (p.s_symbol, p.s_position, p.s_target, p.s_pending, p.s_conflict)
    RETURNING 1
), cancel_member_insert AS (
    INSERT INTO public.spot_order_set_cancel_members (
        tenant_id, venue_id, account_scope, generation_id, intent_id, symbol, position_id,
        target_id, pending, conflict
    )
    SELECT p.tenant_id, p.venue_id, p.account_scope, p.generation_id, p.intent_id, p.symbol,
           p.position_id, p.target_id, true, true
    FROM cancel_member_plan p, verdict v
    WHERE v.ok AND p.touch AND NOT p.stored
    RETURNING 1
), cancel_group_update AS (
    UPDATE public.spot_order_set_cancel_groups t
    SET pending_count = p.pending_next, conflict_count = p.conflict_next, terminal_mask = p.final_mask
    FROM cancel_group_plan p, verdict v
    WHERE v.ok AND p.touch AND p.stored
      AND (t.tenant_id, t.venue_id, t.account_scope, t.generation_id, t.symbol, t.position_id, t.target_id)
        = (p.tenant_id, p.venue_id, p.account_scope, p.generation_id, p.symbol, p.position_id, p.target_id)
      AND (t.pending_count, t.conflict_count, t.terminal_mask)
        = (p.prior_pending, p.prior_conflict, p.prior_mask)
    RETURNING 1
), cancel_group_insert AS (
    INSERT INTO public.spot_order_set_cancel_groups (
        tenant_id, venue_id, account_scope, generation_id, symbol, position_id, target_id,
        pending_count, conflict_count, terminal_mask
    )
    SELECT p.tenant_id, p.venue_id, p.account_scope, p.generation_id, p.symbol, p.position_id,
           p.target_id, p.pending_next, p.conflict_next, p.final_mask
    FROM cancel_group_plan p, verdict v
    WHERE v.ok AND p.touch AND NOT p.stored
    RETURNING 1
), sell_leg_update AS (
    UPDATE public.spot_order_set_sell_legs t
    SET potential_count = p.potential_next, conflict_count = p.conflict_next
    FROM sell_aggregates p, verdict v
    WHERE v.ok AND p.kind = 'leg' AND p.stored
      AND (t.tenant_id, t.venue_id, t.account_scope, t.generation_id, t.symbol, t.position_id)
        = (p.tenant_id, p.venue_id, p.account_scope, p.generation_id, p.symbol, p.position_id)
      AND (t.potential_count, t.conflict_count) = (p.prior_potential, p.prior_conflict)
    RETURNING 1
), sell_leg_insert AS (
    INSERT INTO public.spot_order_set_sell_legs (
        tenant_id, venue_id, account_scope, generation_id, symbol, position_id,
        potential_count, conflict_count
    )
    SELECT p.tenant_id, p.venue_id, p.account_scope, p.generation_id, p.symbol, p.position_id,
           p.potential_next, p.conflict_next
    FROM sell_aggregates p, verdict v
    WHERE v.ok AND p.kind = 'leg' AND NOT p.stored
    RETURNING 1
), sell_symbol_update AS (
    UPDATE public.spot_order_set_sell_symbols t
    SET potential_count = p.potential_next, conflict_count = p.conflict_next
    FROM sell_aggregates p, verdict v
    WHERE v.ok AND p.kind = 'symbol' AND p.stored
      AND (t.tenant_id, t.venue_id, t.account_scope, t.generation_id, t.symbol)
        = (p.tenant_id, p.venue_id, p.account_scope, p.generation_id, p.symbol)
      AND (t.potential_count, t.conflict_count) = (p.prior_potential, p.prior_conflict)
    RETURNING 1
), sell_symbol_insert AS (
    INSERT INTO public.spot_order_set_sell_symbols (
        tenant_id, venue_id, account_scope, generation_id, symbol, potential_count, conflict_count
    )
    SELECT p.tenant_id, p.venue_id, p.account_scope, p.generation_id, p.symbol,
           p.potential_next, p.conflict_next
    FROM sell_aggregates p, verdict v
    WHERE v.ok AND p.kind = 'symbol' AND NOT p.stored
    RETURNING 1
), sell_unbound_update AS (
    UPDATE public.spot_order_set_sell_unbound t
    SET potential_count = p.potential_next, conflict_count = p.conflict_next
    FROM sell_aggregates p, verdict v
    WHERE v.ok AND p.kind = 'unbound' AND p.stored
      AND (t.tenant_id, t.venue_id, t.account_scope, t.generation_id)
        = (p.tenant_id, p.venue_id, p.account_scope, p.generation_id)
      AND (t.potential_count, t.conflict_count) = (p.prior_potential, p.prior_conflict)
    RETURNING 1
), sell_unbound_insert AS (
    INSERT INTO public.spot_order_set_sell_unbound (
        tenant_id, venue_id, account_scope, generation_id, potential_count, conflict_count
    )
    SELECT p.tenant_id, p.venue_id, p.account_scope, p.generation_id,
           p.potential_next, p.conflict_next
    FROM sell_aggregates p, verdict v
    WHERE v.ok AND p.kind = 'unbound' AND NOT p.stored
    RETURNING 1
), cancel_leg_update AS (
    UPDATE public.spot_order_set_cancel_legs t
    SET active_count = p.active_next, conflict_count = p.conflict_next
    FROM cancel_aggregates p, verdict v
    WHERE v.ok AND p.kind = 'leg' AND p.stored
      AND (t.tenant_id, t.venue_id, t.account_scope, t.generation_id, t.symbol, t.position_id)
        = (p.tenant_id, p.venue_id, p.account_scope, p.generation_id, p.symbol, p.position_id)
      AND (t.active_count, t.conflict_count) = (p.prior_active, p.prior_conflict)
    RETURNING 1
), cancel_leg_insert AS (
    INSERT INTO public.spot_order_set_cancel_legs (
        tenant_id, venue_id, account_scope, generation_id, symbol, position_id,
        active_count, conflict_count
    )
    SELECT p.tenant_id, p.venue_id, p.account_scope, p.generation_id, p.symbol, p.position_id,
           p.active_next, p.conflict_next
    FROM cancel_aggregates p, verdict v
    WHERE v.ok AND p.kind = 'leg' AND NOT p.stored
    RETURNING 1
), cancel_ungroupable_update AS (
    UPDATE public.spot_order_set_cancel_ungroupable t
    SET active_count = p.active_next, conflict_count = p.conflict_next
    FROM cancel_aggregates p, verdict v
    WHERE v.ok AND p.kind = 'ungroupable' AND p.stored
      AND (t.tenant_id, t.venue_id, t.account_scope, t.generation_id, t.symbol, t.position_id)
        = (p.tenant_id, p.venue_id, p.account_scope, p.generation_id, p.symbol, p.position_id)
      AND (t.active_count, t.conflict_count) = (p.prior_active, p.prior_conflict)
    RETURNING 1
), cancel_ungroupable_insert AS (
    INSERT INTO public.spot_order_set_cancel_ungroupable (
        tenant_id, venue_id, account_scope, generation_id, symbol, position_id,
        active_count, conflict_count
    )
    SELECT p.tenant_id, p.venue_id, p.account_scope, p.generation_id, p.symbol, p.position_id,
           p.active_next, p.conflict_next
    FROM cancel_aggregates p, verdict v
    WHERE v.ok AND p.kind = 'ungroupable' AND NOT p.stored
    RETURNING 1
), cancel_unbound_update AS (
    UPDATE public.spot_order_set_cancel_unbound t
    SET active_count = p.active_next, conflict_count = p.conflict_next
    FROM cancel_aggregates p, verdict v
    WHERE v.ok AND p.kind = 'unbound' AND p.stored
      AND (t.tenant_id, t.venue_id, t.account_scope, t.generation_id)
        = (p.tenant_id, p.venue_id, p.account_scope, p.generation_id)
      AND (t.active_count, t.conflict_count) = (p.prior_active, p.prior_conflict)
    RETURNING 1
), cancel_unbound_insert AS (
    INSERT INTO public.spot_order_set_cancel_unbound (
        tenant_id, venue_id, account_scope, generation_id, active_count, conflict_count
    )
    SELECT p.tenant_id, p.venue_id, p.account_scope, p.generation_id,
           p.active_next, p.conflict_next
    FROM cancel_aggregates p, verdict v
    WHERE v.ok AND p.kind = 'unbound' AND NOT p.stored
    RETURNING 1
), written AS (
    SELECT 1 FROM sell_order_update UNION ALL SELECT 1 FROM sell_order_insert
    UNION ALL SELECT 1 FROM sell_intent_update UNION ALL SELECT 1 FROM sell_intent_insert
    UNION ALL SELECT 1 FROM cancel_member_update UNION ALL SELECT 1 FROM cancel_member_insert
    UNION ALL SELECT 1 FROM cancel_group_update UNION ALL SELECT 1 FROM cancel_group_insert
    UNION ALL SELECT 1 FROM sell_leg_update UNION ALL SELECT 1 FROM sell_leg_insert
    UNION ALL SELECT 1 FROM sell_symbol_update UNION ALL SELECT 1 FROM sell_symbol_insert
    UNION ALL SELECT 1 FROM sell_unbound_update UNION ALL SELECT 1 FROM sell_unbound_insert
    UNION ALL SELECT 1 FROM cancel_leg_update UNION ALL SELECT 1 FROM cancel_leg_insert
    UNION ALL SELECT 1 FROM cancel_ungroupable_update UNION ALL SELECT 1 FROM cancel_ungroupable_insert
    UNION ALL SELECT 1 FROM cancel_unbound_update UNION ALL SELECT 1 FROM cancel_unbound_insert
)
-- Each keyed write affects at most its one planned row, so equal totals
-- prove every planned row; a suppressed or CAS-missed row refuses.
UPDATE public.spot_order_set_generation g
SET accounting_version = $6
FROM gen c, verdict v
WHERE (g.tenant_id, g.venue_id, g.account_scope, g.generation_id)
    = (c.tenant_id, c.venue_id, c.account_scope, c.generation_id)
  AND g.accounting_version = $6::bigint - 1
  AND v.ok
  AND (SELECT count(*) FROM written) = (SELECT planned_rows FROM planned)
RETURNING g.accounting_version
