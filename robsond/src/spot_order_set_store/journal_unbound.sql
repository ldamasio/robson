-- The caller already owns the tenant/venue accounting row and has applied
-- the original intent and attempt effects in this same transaction. Every
-- read below is a point probe on a complete key.
WITH journal AS MATERIALIZED (
    SELECT j.intent_id, j.tenant_id, j.venue_id, j.account_scope, j.position_id,
           g.generation_id, leg.symbol AS leg_symbol, leg.position_id AS leg_position,
           j.not_sent,
           j.venue_outcome IS NOT DISTINCT FROM 'rejected' AS rejected,
           j.venue_outcome IS NULL AND j.venue_evidence IS NULL AS no_answer,
           CASE WHEN j.venue_outcome = 'rejected' AND NOT j.not_sent
                     AND j.venue_evidence IS NULL THEN EXISTS (
               SELECT 1 FROM public.submit_attempts s
               WHERE s.venue_id = 'binance:spot'
                 AND (s.account_scope, s.client_order_id) = (j.account_scope, j.client_order_id)
                 AND s.state = 'rejected' AND s.rejected_at IS NOT NULL
           ) ELSE false END AS definite_rejection
    FROM public.order_intents j
    JOIN public.spot_order_set_generation g
      ON (g.tenant_id, g.venue_id, g.account_scope) = (j.tenant_id, j.venue_id, j.account_scope)
    JOIN LATERAL (
        SELECT epoch_id, clean_stop_at FROM public.process_epochs
        WHERE tenant_id = g.tenant_id ORDER BY epoch_seq DESC LIMIT 1
    ) epoch ON epoch.epoch_id = g.process_epoch AND epoch.clean_stop_at IS NULL
    -- The actual leg named by J's raw position, in J's exact owner.
    LEFT JOIN LATERAL (
        SELECT l.symbol, l.position_id FROM public.balance_legs l
        WHERE l.position_id = j.position_id
          AND (l.tenant_id, l.venue_id, l.account_scope) = (j.tenant_id, j.venue_id, j.account_scope)
        LIMIT 1
    ) leg ON true
    WHERE j.intent_id = $7 AND (j.tenant_id, j.venue_id, j.account_scope) = ($1, 'binance:spot', $2)
      AND (g.generation_id, g.process_epoch, g.source_audit_id) = ($3, $4, $5)
      AND g.accounting_version = $6::bigint - 1
      AND j.client_order_id = $8 AND j.position_id IS NOT DISTINCT FROM $9::uuid
      AND j.kind = $10 AND j.kind IN ('protection', 'exit')
      -- A native order naming J is a separate edge.
      AND NOT EXISTS (
          SELECT 1 FROM public.venue_orders o WHERE o.intent_id = j.intent_id
      )
), classified AS MATERIALIZED (
    SELECT c.*,
           CASE WHEN NOT (c.potential OR c.conflict) THEN 'none'
                WHEN c.leg_position IS NOT NULL THEN 'leg'
                ELSE 'unbound' END AS scope_kind
    FROM (
        SELECT j.*,
               NOT ((j.not_sent AND j.no_answer)
                     OR (j.rejected AND NOT j.not_sent AND j.definite_rejection)) AS potential,
               (j.not_sent AND NOT j.no_answer)
                     OR (j.rejected AND NOT (NOT j.not_sent AND j.definite_rejection)) AS conflict
        FROM journal j
    ) c
), prior AS MATERIALIZED (
    SELECT c.*,
           p.intent_id IS NOT NULL AS member_found,
           coalesce(p.scope_kind, 'none') AS prior_kind,
           coalesce(p.potential, false) AS prior_potential,
           coalesce(p.conflict, false) AS prior_conflict,
           u.generation_id IS NOT NULL AS unbound_found,
           coalesce(u.potential_count, 0) AS unbound_potential,
           coalesce(u.conflict_count, 0) AS unbound_conflict,
           b.generation_id IS NOT NULL AS leg_found,
           coalesce(b.potential_count, 0) AS leg_potential,
           coalesce(b.conflict_count, 0) AS leg_conflict
    FROM classified c
    LEFT JOIN LATERAL (
        SELECT m.intent_id, m.scope_kind, m.potential, m.conflict, m.scope_symbol,
               m.scope_position, m.source_position
        FROM public.spot_order_set_sell_intents m
        WHERE (m.tenant_id, m.venue_id, m.account_scope, m.generation_id, m.intent_id)
            = (c.tenant_id, c.venue_id, c.account_scope, c.generation_id, c.intent_id)
        LIMIT 1
    ) p ON true
    LEFT JOIN LATERAL (
        SELECT t.generation_id, t.potential_count, t.conflict_count
        FROM public.spot_order_set_sell_unbound t
        WHERE (t.tenant_id, t.venue_id, t.account_scope, t.generation_id)
            = (c.tenant_id, c.venue_id, c.account_scope, c.generation_id)
        LIMIT 1
    ) u ON true
    LEFT JOIN LATERAL (
        SELECT t.generation_id, t.potential_count, t.conflict_count
        FROM public.spot_order_set_sell_legs t
        WHERE (t.tenant_id, t.venue_id, t.account_scope, t.generation_id, t.symbol, t.position_id)
            = (c.tenant_id, c.venue_id, c.account_scope, c.generation_id,
               c.leg_symbol, c.leg_position)
        LIMIT 1
    ) b ON true
    -- Exact prior flags from the original snapshot; absent and retained
    -- inactive members both carry none.
    WHERE coalesce(p.potential, false) = $11::boolean
      AND coalesce(p.conflict, false) = $12::boolean
      AND (p.intent_id IS NULL OR (
        p.source_position IS NOT DISTINCT FROM c.position_id
        AND (p.scope_kind <> 'none') = (p.potential OR p.conflict)
        AND (p.scope_kind IN ('unbound', 'none')
             OR (p.scope_kind = 'leg'
                 AND (p.scope_symbol, p.scope_position) = (c.leg_symbol, c.leg_position)))
      ))
), changes AS MATERIALIZED (
    SELECT p.*,
           p.prior_kind IS DISTINCT FROM p.scope_kind
               OR p.prior_potential <> p.potential OR p.prior_conflict <> p.conflict
               AS member_changed,
           (p.scope_kind = 'unbound' AND p.potential)::integer
               - (p.prior_kind = 'unbound' AND p.prior_potential)::integer AS unbound_potential_delta,
           (p.scope_kind = 'unbound' AND p.conflict)::integer
               - (p.prior_kind = 'unbound' AND p.prior_conflict)::integer AS unbound_conflict_delta,
           (p.scope_kind = 'leg' AND p.potential)::integer
               - (p.prior_kind = 'leg' AND p.prior_potential)::integer AS leg_potential_delta,
           (p.scope_kind = 'leg' AND p.conflict)::integer
               - (p.prior_kind = 'leg' AND p.prior_conflict)::integer AS leg_conflict_delta
    FROM prior p
    -- The actual older bucket must exist and cover the older contribution,
    -- even when its net delta is zero.
    WHERE (p.prior_kind <> 'unbound' OR (p.unbound_found
              AND p.unbound_potential >= p.prior_potential::integer
              AND p.unbound_conflict >= p.prior_conflict::integer))
      AND (p.prior_kind <> 'leg' OR (p.leg_found
              AND p.leg_potential >= p.prior_potential::integer
              AND p.leg_conflict >= p.prior_conflict::integer))
), member_insert AS (
    INSERT INTO public.spot_order_set_sell_intents (
        tenant_id, venue_id, account_scope, generation_id, intent_id, source_position,
        scope_kind, potential, conflict, scope_symbol, scope_position
    )
    SELECT tenant_id, venue_id, account_scope, generation_id, intent_id, position_id,
           scope_kind, potential, conflict,
           CASE WHEN scope_kind = 'leg' THEN leg_symbol END,
           CASE WHEN scope_kind = 'leg' THEN leg_position END
    FROM changes WHERE member_changed AND NOT member_found
    RETURNING intent_id
), member_update AS (
    UPDATE public.spot_order_set_sell_intents m
    SET scope_kind = c.scope_kind, potential = c.potential, conflict = c.conflict,
        scope_symbol = CASE WHEN c.scope_kind = 'leg' THEN c.leg_symbol END,
        scope_position = CASE WHEN c.scope_kind = 'leg' THEN c.leg_position END
    FROM changes c
    WHERE c.member_changed AND c.member_found
      AND (m.tenant_id, m.venue_id, m.account_scope, m.generation_id, m.intent_id)
        = (c.tenant_id, c.venue_id, c.account_scope, c.generation_id, c.intent_id)
      AND (m.scope_kind, m.potential, m.conflict) = (c.prior_kind, c.prior_potential, c.prior_conflict)
      AND m.source_position IS NOT DISTINCT FROM c.position_id
    RETURNING m.intent_id
), unbound_insert AS (
    INSERT INTO public.spot_order_set_sell_unbound (
        tenant_id, venue_id, account_scope, generation_id, potential_count, conflict_count
    )
    SELECT tenant_id, venue_id, account_scope, generation_id,
           unbound_potential_delta, unbound_conflict_delta
    FROM changes
    WHERE NOT unbound_found AND (unbound_potential_delta <> 0 OR unbound_conflict_delta <> 0)
    RETURNING generation_id
), unbound_update AS (
    UPDATE public.spot_order_set_sell_unbound t
    SET potential_count = t.potential_count + c.unbound_potential_delta,
        conflict_count = t.conflict_count + c.unbound_conflict_delta
    FROM changes c
    WHERE c.unbound_found AND (c.unbound_potential_delta <> 0 OR c.unbound_conflict_delta <> 0)
      AND (t.tenant_id, t.venue_id, t.account_scope, t.generation_id)
        = (c.tenant_id, c.venue_id, c.account_scope, c.generation_id)
      AND (t.potential_count, t.conflict_count) = (c.unbound_potential, c.unbound_conflict)
    RETURNING t.generation_id
), leg_insert AS (
    INSERT INTO public.spot_order_set_sell_legs (
        tenant_id, venue_id, account_scope, generation_id, symbol, position_id,
        potential_count, conflict_count
    )
    SELECT tenant_id, venue_id, account_scope, generation_id, leg_symbol, leg_position,
           leg_potential_delta, leg_conflict_delta
    FROM changes
    WHERE NOT leg_found AND (leg_potential_delta <> 0 OR leg_conflict_delta <> 0)
    RETURNING generation_id
), leg_update AS (
    UPDATE public.spot_order_set_sell_legs t
    SET potential_count = t.potential_count + c.leg_potential_delta,
        conflict_count = t.conflict_count + c.leg_conflict_delta
    FROM changes c
    WHERE c.leg_found AND (c.leg_potential_delta <> 0 OR c.leg_conflict_delta <> 0)
      AND (t.tenant_id, t.venue_id, t.account_scope, t.generation_id, t.symbol, t.position_id)
        = (c.tenant_id, c.venue_id, c.account_scope, c.generation_id,
           c.leg_symbol, c.leg_position)
      AND (t.potential_count, t.conflict_count) = (c.leg_potential, c.leg_conflict)
    RETURNING t.generation_id
)
UPDATE public.spot_order_set_generation g
SET accounting_version = $6
FROM changes c
WHERE (g.tenant_id, g.venue_id, g.account_scope, g.generation_id)
    = (c.tenant_id, c.venue_id, c.account_scope, c.generation_id)
  AND g.accounting_version = $6::bigint - 1
  AND c.member_changed::integer
    = (SELECT count(*) FROM member_insert) + (SELECT count(*) FROM member_update)
  AND (c.unbound_potential_delta <> 0 OR c.unbound_conflict_delta <> 0)::integer
    = (SELECT count(*) FROM unbound_insert) + (SELECT count(*) FROM unbound_update)
  AND (c.leg_potential_delta <> 0 OR c.leg_conflict_delta <> 0)::integer
    = (SELECT count(*) FROM leg_insert) + (SELECT count(*) FROM leg_update)
RETURNING g.accounting_version
