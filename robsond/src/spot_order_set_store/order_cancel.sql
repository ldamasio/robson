-- Part 3: CANCEL membership (cancel_shadow_sync_target). Leg-owned rows only:
-- at most one named cancel J member and this order's own target group. The
-- target's members are never searched or rewritten.
, cancel_member AS MATERIALIZED (
    SELECT c.tenant_id, c.venue_id, c.account_scope, c.generation_id, c.intent_id,
           c.j_leg_symbol AS symbol,
           CASE WHEN c.j_leg_symbol IS NOT NULL THEN c.j_position END AS position_id,
           c.j_target AS target_id, c.j_position AS source_position, c.position_hints_complete,
           -- A linked prior row already disputed J; otherwise the journal facts.
           c.prior_linked OR NOT c.j_not_sent OR NOT c.j_no_answer AS p_pending,
           c.prior_linked OR (c.j_not_sent AND NOT c.j_no_answer) AS p_conflict,
           m.intent_id IS NOT NULL AS stored,
           m.symbol AS s_symbol, m.position_id AS s_position, m.target_id AS s_target,
           m.source_position AS s_source,
           coalesce(m.pending, false) AS s_pending, coalesce(m.conflict, false) AS s_conflict
    FROM links c
    LEFT JOIN LATERAL (
        SELECT intent_id, symbol, position_id, source_position, target_id, pending, conflict FROM public.spot_order_set_cancel_members
        WHERE (tenant_id, venue_id, account_scope, generation_id, intent_id)
            = (c.tenant_id, c.venue_id, c.account_scope, c.generation_id, c.intent_id)
        LIMIT 1
    ) m ON true
    WHERE c.owner = 'leg' AND c.has_j AND c.j_kind = 'cancel'
), cancel_member_plan AS MATERIALIZED (
    -- This native row names the cancel: pending and conflict for that J only.
    SELECT m.*,
           m.stored
               AND (m.s_symbol, m.s_position, m.s_target)
                   IS NOT DISTINCT FROM (m.symbol, m.position_id, m.target_id)
               AND (m.s_source IS NOT DISTINCT FROM m.source_position
                    OR (NOT m.position_hints_complete AND m.s_source IS NULL))
               AND (m.s_pending, m.s_conflict) = (m.p_pending, m.p_conflict) AS ok,
           (NOT (m.s_pending AND m.s_conflict)
               OR m.s_source IS DISTINCT FROM m.source_position) AS touch,
           (NOT m.s_pending)::integer AS pending_delta,
           (NOT m.s_conflict)::integer AS conflict_delta
    FROM cancel_member m
), cancel_group_moves AS MATERIALIZED (
    -- Merge both paths when the member group is this order's target group.
    SELECT d.symbol, d.position_id, d.target_id,
           sum(d.pending_delta)::bigint AS pending_delta,
           sum(d.conflict_delta)::bigint AS conflict_delta,
           sum(d.required_pending)::bigint AS required_pending,
           sum(d.required_conflict)::bigint AS required_conflict,
           bool_or(d.mask_prior) AS mask_prior, bool_or(d.mask_next) AS mask_next
    FROM (
        SELECT symbol, position_id, target_id, pending_delta, conflict_delta,
               s_pending::integer, s_conflict::integer, NULL::boolean, NULL::boolean
        FROM cancel_member_plan
        WHERE position_id IS NOT NULL AND target_id IS NOT NULL
        UNION ALL
        SELECT symbol, position_id, exchange_order_id, 0, 0, 0, 0, mask_prior, mask_next
        FROM links WHERE owner = 'leg' AND position_id IS NOT NULL
    ) d(symbol, position_id, target_id, pending_delta, conflict_delta,
        required_pending, required_conflict, mask_prior, mask_next)
    GROUP BY d.symbol, d.position_id, d.target_id
), cancel_groups AS MATERIALIZED (
    SELECT g.tenant_id, g.venue_id, g.account_scope, g.generation_id, d.*,
           p.target_id IS NOT NULL AS stored,
           coalesce(p.pending_count, 0) AS prior_pending,
           coalesce(p.conflict_count, 0) AS prior_conflict,
           coalesce(p.terminal_mask, false) AS prior_mask,
           coalesce(p.pending_count, 0) + d.pending_delta AS pending_next,
           coalesce(p.conflict_count, 0) + d.conflict_delta AS conflict_next,
           coalesce(d.mask_next, p.terminal_mask, false) AS final_mask
    FROM cancel_group_moves d
    CROSS JOIN gen g
    LEFT JOIN LATERAL (
        SELECT target_id, pending_count, conflict_count, terminal_mask FROM public.spot_order_set_cancel_groups
        WHERE (tenant_id, venue_id, account_scope, generation_id, symbol, position_id, target_id)
            = (g.tenant_id, g.venue_id, g.account_scope, g.generation_id, d.symbol, d.position_id, d.target_id)
        LIMIT 1
    ) p ON true
), cancel_group_plan AS MATERIALIZED (
    -- The target's prior mask follows the locked native row, not the stored one.
    SELECT p.*,
           (p.mask_prior IS NULL OR p.prior_mask = p.mask_prior)
               AND p.prior_pending >= p.required_pending AND p.prior_conflict >= p.required_conflict
               AND p.pending_next >= 0 AND p.conflict_next >= 0 AS ok,
           p.pending_delta <> 0 OR p.conflict_delta <> 0 OR p.final_mask <> p.prior_mask AS touch,
           (CASE WHEN p.final_mask THEN 0 ELSE p.pending_next END)
               - (CASE WHEN p.prior_mask THEN 0 ELSE p.prior_pending END) AS active_delta
    FROM cancel_groups p
), cancel_counts AS MATERIALIZED (
    -- Leg aggregates from at most two groups; one ungroupable or unbound bucket.
    SELECT d.kind, d.symbol, d.position_id,
           sum(d.active)::bigint AS active_delta, sum(d.conflict)::bigint AS conflict_delta,
           sum(d.required_active)::bigint AS required_active,
           sum(d.required_conflict)::bigint AS required_conflict
    FROM (
        SELECT 'leg', symbol, position_id, active_delta, conflict_delta,
               CASE WHEN prior_mask THEN 0 ELSE prior_pending END, prior_conflict
        FROM cancel_group_plan
        UNION ALL
        SELECT CASE WHEN position_id IS NULL THEN 'unbound' ELSE 'ungroupable' END,
               symbol, position_id, pending_delta::bigint, conflict_delta::bigint,
               s_pending::integer::bigint, s_conflict::integer::bigint
        FROM cancel_member_plan WHERE position_id IS NULL OR target_id IS NULL
    ) d(kind, symbol, position_id, active, conflict, required_active, required_conflict)
    GROUP BY d.kind, d.symbol, d.position_id
    HAVING sum(d.active) <> 0 OR sum(d.conflict) <> 0
        OR sum(d.required_active) <> 0 OR sum(d.required_conflict) <> 0
), cancel_aggregates AS MATERIALIZED (
    SELECT g.tenant_id, g.venue_id, g.account_scope, g.generation_id, d.*,
           d.active_delta <> 0 OR d.conflict_delta <> 0 AS touch,
           coalesce(l.tenant_id, x.tenant_id, u.tenant_id) IS NOT NULL AS stored,
           coalesce(l.active_count, x.active_count, u.active_count, 0) AS prior_active,
           coalesce(l.conflict_count, x.conflict_count, u.conflict_count, 0) AS prior_conflict,
           coalesce(l.active_count, x.active_count, u.active_count, 0) + d.active_delta AS active_next,
           coalesce(l.conflict_count, x.conflict_count, u.conflict_count, 0)
               + d.conflict_delta AS conflict_next
    FROM cancel_counts d
    CROSS JOIN gen g
    LEFT JOIN LATERAL (
        SELECT tenant_id, active_count, conflict_count
        FROM public.spot_order_set_cancel_legs
        WHERE d.kind = 'leg'
          AND (tenant_id, venue_id, account_scope, generation_id, symbol, position_id)
            = (g.tenant_id, g.venue_id, g.account_scope, g.generation_id, d.symbol, d.position_id)
        LIMIT 1) l ON true
    LEFT JOIN LATERAL (
        SELECT tenant_id, active_count, conflict_count
        FROM public.spot_order_set_cancel_ungroupable
        WHERE d.kind = 'ungroupable'
          AND (tenant_id, venue_id, account_scope, generation_id, symbol, position_id)
            = (g.tenant_id, g.venue_id, g.account_scope, g.generation_id, d.symbol, d.position_id)
        LIMIT 1) x ON true
    LEFT JOIN LATERAL (
        SELECT tenant_id, active_count, conflict_count
        FROM public.spot_order_set_cancel_unbound
        WHERE d.kind = 'unbound'
          AND (tenant_id, venue_id, account_scope, generation_id)
            = (g.tenant_id, g.venue_id, g.account_scope, g.generation_id)
        LIMIT 1) u ON true
)
