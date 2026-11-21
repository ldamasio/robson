-- Part 2: SELL membership (sell_shadow.sql classify/sync). One native-key
-- member and, when the row names a J in scope, one independent J member.
, sell_class AS MATERIALIZED (
    SELECT k.*,
           (k.side = 'SELL' OR k.suspected)
               AND NOT (k.recognized AND k.exact AND NOT k.contradicted) AS n_active,
           $9::boolean AND (k.side = 'SELL' OR (k.prior_linked AND k.suspected))
               AND NOT (k.prior_recognized AND k.prior_exact AND NOT k.prior_contradicted)
               AS p_active
    FROM links k
), sell_order AS MATERIALIZED (
    SELECT c.tenant_id, c.venue_id, c.account_scope, c.generation_id, c.symbol, c.exchange_order_id,
           CASE WHEN NOT c.p_active THEN 'none' WHEN $12::uuid IS NULL THEN 'symbol' ELSE 'leg' END
               AS p_kind,
           CASE WHEN c.p_active THEN c.symbol END AS p_symbol,
           CASE WHEN c.p_active THEN $12::uuid END AS p_position,
           c.p_active AS p_potential,
           c.p_active AND ($14::text = 'terminal' OR c.prior_contradicted
               OR (($12::uuid IS NOT NULL OR c.prior_linked) AND NOT c.prior_exact)) AS p_conflict,
           CASE WHEN NOT c.n_active THEN 'none' WHEN c.position_id IS NULL THEN 'symbol' ELSE 'leg' END
               AS n_kind,
           CASE WHEN c.n_active THEN c.symbol END AS n_symbol,
           CASE WHEN c.n_active THEN c.position_id END AS n_position,
           c.n_active AS n_potential,
           c.n_active AND (c.phase = 'terminal' OR c.contradicted
               OR ((c.position_id IS NOT NULL OR c.has_j) AND NOT c.exact)) AS n_conflict,
           m.exchange_order_id IS NOT NULL AS stored,
           coalesce(m.scope_kind, 'none') AS s_kind, m.scope_symbol AS s_symbol,
           m.scope_position AS s_position,
           coalesce(m.potential, false) AS s_potential, coalesce(m.conflict, false) AS s_conflict
    FROM sell_class c
    LEFT JOIN LATERAL (
        SELECT scope_kind, scope_symbol, scope_position, potential, conflict, exchange_order_id FROM public.spot_order_set_sell_orders
        WHERE (tenant_id, venue_id, account_scope, generation_id, symbol, exchange_order_id)
            = (c.tenant_id, c.venue_id, c.account_scope, c.generation_id, c.symbol, c.exchange_order_id)
        LIMIT 1
    ) m ON true
), sell_order_plan AS MATERIALIZED (
    -- The native member's prior is exactly the locked fingerprint's class.
    SELECT o.*,
           (o.s_kind, o.s_symbol, o.s_position, o.s_potential, o.s_conflict)
               IS NOT DISTINCT FROM (o.p_kind, o.p_symbol, o.p_position, o.p_potential, o.p_conflict)
               AS ok,
           (o.stored OR o.n_kind <> 'none')
               AND (o.s_kind, o.s_symbol, o.s_position, o.s_potential, o.s_conflict)
                   IS DISTINCT FROM (o.n_kind, o.n_symbol, o.n_position, o.n_potential, o.n_conflict)
               AS touch
    FROM sell_order o
), sell_intent AS MATERIALIZED (
    SELECT c.tenant_id, c.venue_id, c.account_scope, c.generation_id, c.intent_id, c.j_position,
           c.j_leg_symbol,
           CASE WHEN NOT c.suspected THEN false
                WHEN c.prior_linked THEN NOT c.prior_exact
                ELSE NOT ((c.j_not_sent AND c.j_no_answer) OR c.j_definite_rejection) END
               AS p_potential,
           CASE WHEN NOT c.suspected THEN false
                WHEN c.prior_linked THEN NOT c.prior_exact
                ELSE NOT ((c.j_not_sent AND c.j_no_answer) OR c.j_definite_rejection)
                     AND (c.j_not_sent OR c.j_rejected) END AS p_conflict,
           -- A named but mismatched native row keeps J's own conservative member.
           c.suspected AND NOT c.exact AS n_active,
           m.intent_id IS NOT NULL AS stored,
           coalesce(m.scope_kind, 'none') AS s_kind, m.scope_symbol AS s_symbol,
           m.scope_position AS s_position, m.source_position AS s_source,
           coalesce(m.potential, false) AS s_potential, coalesce(m.conflict, false) AS s_conflict
    FROM sell_class c
    LEFT JOIN LATERAL (
        SELECT scope_kind, scope_symbol, scope_position, source_position, potential, conflict, intent_id FROM public.spot_order_set_sell_intents
        WHERE (tenant_id, venue_id, account_scope, generation_id, intent_id)
            = (c.tenant_id, c.venue_id, c.account_scope, c.generation_id, c.intent_id)
        LIMIT 1
    ) m ON true
    WHERE c.has_j
), sell_intent_class AS MATERIALIZED (
    SELECT i.*,
           CASE WHEN NOT (i.p_potential OR i.p_conflict) THEN 'none'
                WHEN i.j_leg_symbol IS NULL THEN 'unbound' ELSE 'leg' END AS p_kind,
           CASE WHEN NOT i.n_active THEN 'none'
                WHEN i.j_leg_symbol IS NULL THEN 'unbound' ELSE 'leg' END AS n_kind,
           CASE WHEN i.n_active AND i.j_leg_symbol IS NOT NULL THEN i.j_leg_symbol END AS n_symbol,
           CASE WHEN i.n_active AND i.j_leg_symbol IS NOT NULL THEN i.j_position END AS n_position
    FROM sell_intent i
), sell_intent_plan AS MATERIALIZED (
    -- A member written before its leg keeps its unbound hint until this event.
    SELECT i.*,
           (i.stored OR i.p_kind = 'none')
               AND (i.s_potential, i.s_conflict) = (i.p_potential, i.p_conflict)
               AND (NOT i.stored OR i.s_source IS NOT DISTINCT FROM i.j_position)
               AND ((i.s_kind, i.s_symbol, i.s_position) IS NOT DISTINCT FROM (
                        i.p_kind,
                        CASE WHEN i.p_kind = 'leg' THEN i.j_leg_symbol END,
                        CASE WHEN i.p_kind = 'leg' THEN i.j_position END)
                    OR (i.p_kind = 'leg' AND i.s_kind = 'unbound'
                        AND i.s_symbol IS NULL AND i.s_position IS NULL)) AS ok,
           (i.stored OR i.n_kind <> 'none')
               AND (i.s_kind, i.s_symbol, i.s_position, i.s_potential, i.s_conflict)
                   IS DISTINCT FROM (i.n_kind, i.n_symbol, i.n_position, i.n_active, i.n_active)
               AS touch
    FROM sell_intent_class i
), sell_counts AS MATERIALIZED (
    -- Old members establish a minimum for each exact prior bucket even
    -- when another delta cancels the net movement or this write is numeric.
    SELECT d.kind, d.symbol, d.position_id,
           sum(d.potential)::bigint AS potential_delta, sum(d.conflict)::bigint AS conflict_delta,
           sum(d.required_potential)::bigint AS required_potential,
           sum(d.required_conflict)::bigint AS required_conflict
    FROM (
        SELECT s_kind, s_symbol, s_position,
               CASE WHEN touch THEN -s_potential::integer ELSE 0 END,
               CASE WHEN touch THEN -s_conflict::integer ELSE 0 END,
               s_potential::integer, s_conflict::integer
        FROM sell_order_plan
        UNION ALL
        SELECT n_kind, n_symbol, n_position, n_potential::integer, n_conflict::integer, 0, 0
        FROM sell_order_plan WHERE touch
        UNION ALL
        SELECT s_kind, s_symbol, s_position,
               CASE WHEN touch THEN -s_potential::integer ELSE 0 END,
               CASE WHEN touch THEN -s_conflict::integer ELSE 0 END,
               s_potential::integer, s_conflict::integer
        FROM sell_intent_plan
        UNION ALL
        SELECT n_kind, n_symbol, n_position, n_active::integer, n_active::integer, 0, 0
        FROM sell_intent_plan WHERE touch
    ) d(kind, symbol, position_id, potential, conflict, required_potential, required_conflict)
    WHERE d.kind <> 'none'
    GROUP BY d.kind, d.symbol, d.position_id
    HAVING sum(d.potential) <> 0 OR sum(d.conflict) <> 0
        OR sum(d.required_potential) <> 0 OR sum(d.required_conflict) <> 0
), sell_aggregates AS MATERIALIZED (
    SELECT g.tenant_id, g.venue_id, g.account_scope, g.generation_id, d.*,
           d.potential_delta <> 0 OR d.conflict_delta <> 0 AS touch,
           coalesce(l.tenant_id, y.tenant_id, u.tenant_id) IS NOT NULL AS stored,
           coalesce(l.potential_count, y.potential_count, u.potential_count, 0) AS prior_potential,
           coalesce(l.conflict_count, y.conflict_count, u.conflict_count, 0) AS prior_conflict,
           coalesce(l.potential_count, y.potential_count, u.potential_count, 0)
               + d.potential_delta AS potential_next,
           coalesce(l.conflict_count, y.conflict_count, u.conflict_count, 0)
               + d.conflict_delta AS conflict_next
    FROM sell_counts d
    CROSS JOIN gen g
    LEFT JOIN LATERAL (
        SELECT tenant_id, potential_count, conflict_count
        FROM public.spot_order_set_sell_legs
        WHERE d.kind = 'leg'
          AND (tenant_id, venue_id, account_scope, generation_id, symbol, position_id)
            = (g.tenant_id, g.venue_id, g.account_scope, g.generation_id, d.symbol, d.position_id)
        LIMIT 1) l ON true
    LEFT JOIN LATERAL (
        SELECT tenant_id, potential_count, conflict_count
        FROM public.spot_order_set_sell_symbols
        WHERE d.kind = 'symbol'
          AND (tenant_id, venue_id, account_scope, generation_id, symbol)
            = (g.tenant_id, g.venue_id, g.account_scope, g.generation_id, d.symbol)
        LIMIT 1) y ON true
    LEFT JOIN LATERAL (
        SELECT tenant_id, potential_count, conflict_count
        FROM public.spot_order_set_sell_unbound
        WHERE d.kind = 'unbound'
          AND (tenant_id, venue_id, account_scope, generation_id)
            = (g.tenant_id, g.venue_id, g.account_scope, g.generation_id)
        LIMIT 1) u ON true
)
