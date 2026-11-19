-- The caller already owns the tenant/venue accounting row and has applied
-- the original intent and attempt effects in this same transaction.
WITH journal AS MATERIALIZED (
    SELECT j.intent_id, j.tenant_id, j.venue_id, j.account_scope, j.position_id,
           g.generation_id,
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
    WHERE j.intent_id = $7 AND (j.tenant_id, j.venue_id, j.account_scope) = ($1, 'binance:spot', $2)
      AND (g.generation_id, g.process_epoch, g.source_audit_id) = ($3, $4, $5)
      AND g.accounting_version = $6::bigint - 1
      AND j.client_order_id = $8 AND j.position_id IS NOT DISTINCT FROM $9::uuid
      AND j.kind = $10 AND j.kind IN ('protection', 'exit')
      AND NOT EXISTS (
          SELECT 1 FROM public.balance_legs l
          WHERE l.position_id = j.position_id
            AND (l.tenant_id, l.venue_id, l.account_scope) = (j.tenant_id, j.venue_id, j.account_scope)
      )
      AND NOT EXISTS (
          SELECT 1 FROM public.venue_orders o WHERE o.intent_id = j.intent_id
      )
), classified AS (
    SELECT j.*,
           NOT ((j.not_sent AND j.no_answer)
                 OR (j.rejected AND NOT j.not_sent AND j.definite_rejection)) AS potential,
           (j.not_sent AND NOT j.no_answer)
                 OR (j.rejected AND NOT (NOT j.not_sent AND j.definite_rejection)) AS conflict
    FROM journal j
), prior AS MATERIALIZED (
    SELECT c.*,
           coalesce(p.potential, false) AS prior_potential,
           coalesce(p.conflict, false) AS prior_conflict,
           coalesce(t.potential_count, 0) AS potential_count,
           coalesce(t.conflict_count, 0) AS conflict_count
    FROM classified c
    LEFT JOIN public.spot_order_set_sell_intents p
      ON (p.tenant_id, p.venue_id, p.account_scope, p.generation_id, p.intent_id)
       = (c.tenant_id, c.venue_id, c.account_scope, c.generation_id, c.intent_id)
    LEFT JOIN public.spot_order_set_sell_unbound t
      ON (t.tenant_id, t.venue_id, t.account_scope, t.generation_id)
       = (c.tenant_id, c.venue_id, c.account_scope, c.generation_id)
    WHERE coalesce((p.potential OR p.conflict), false) = $11::boolean
      AND (p.intent_id IS NULL OR (
        p.scope_kind IN ('unbound', 'none')
        AND p.source_position IS NOT DISTINCT FROM c.position_id
        AND (p.scope_kind = 'unbound') = (p.potential OR p.conflict)
    ))
      AND coalesce(t.potential_count, 0) >= coalesce(p.potential, false)::integer
      AND coalesce(t.conflict_count, 0) >= coalesce(p.conflict, false)::integer
), changes AS MATERIALIZED (
    SELECT p.*,
           p.potential::integer - p.prior_potential::integer AS potential_delta,
           p.conflict::integer - p.prior_conflict::integer AS conflict_delta
    FROM prior p
), member_write AS (
    INSERT INTO public.spot_order_set_sell_intents (
        tenant_id, venue_id, account_scope, generation_id, intent_id, source_position,
        scope_kind, potential, conflict
    )
    SELECT tenant_id, venue_id, account_scope, generation_id, intent_id, position_id,
           CASE WHEN potential OR conflict THEN 'unbound' ELSE 'none' END, potential, conflict
    FROM changes WHERE potential_delta <> 0 OR conflict_delta <> 0
    ON CONFLICT (tenant_id, venue_id, account_scope, generation_id, intent_id) DO UPDATE
    SET scope_kind = excluded.scope_kind, potential = excluded.potential, conflict = excluded.conflict
    RETURNING intent_id
), aggregate_write AS (
    INSERT INTO public.spot_order_set_sell_unbound (
        tenant_id, venue_id, account_scope, generation_id, potential_count, conflict_count
    )
    SELECT tenant_id, venue_id, account_scope, generation_id,
           potential_count + potential_delta, conflict_count + conflict_delta
    FROM changes WHERE potential_delta <> 0 OR conflict_delta <> 0
    ON CONFLICT (tenant_id, venue_id, account_scope, generation_id) DO UPDATE
    SET potential_count = excluded.potential_count, conflict_count = excluded.conflict_count
    RETURNING generation_id
)
UPDATE public.spot_order_set_generation g
SET accounting_version = $6
FROM changes c
WHERE (g.tenant_id, g.venue_id, g.account_scope, g.generation_id)
    = (c.tenant_id, c.venue_id, c.account_scope, c.generation_id)
  AND g.accounting_version = $6::bigint - 1
  AND ((c.potential_delta <> 0 OR c.conflict_delta <> 0)::integer)
    = (SELECT count(*) FROM member_write)
  AND ((c.potential_delta <> 0 OR c.conflict_delta <> 0)::integer)
    = (SELECT count(*) FROM aggregate_write)
RETURNING g.accounting_version
