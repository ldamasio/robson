-- The caller already owns the tenant/venue accounting row and has applied the
-- original entry intent, attempt, clock and admission effects in this same
-- transaction. The before-write branch was checked by the caller; this proves
-- the resulting current pointer and that no prior membership can gain
-- attribution from it. Two exact hint probes, no journal scan, and only the
-- generation row is written.
WITH entry AS MATERIALIZED (
    SELECT j.tenant_id, j.venue_id, j.account_scope, g.generation_id
    FROM public.order_intents j
    JOIN public.leg_routes r
      ON (r.position_id, r.tenant_id, r.venue_id, r.account_scope)
       = (j.position_id, j.tenant_id, j.venue_id, j.account_scope)
    -- The expected exact new current pointer and its unsuperseded admission.
    JOIN public.balance_legs l
      ON (l.position_id, l.tenant_id, l.venue_id, l.account_scope, l.admission_intent_id)
       = (j.position_id, j.tenant_id, j.venue_id, j.account_scope, j.intent_id)
    JOIN public.balance_leg_admissions a
      ON (a.entry_intent_id, a.position_id, a.tenant_id, a.venue_id, a.account_scope, a.symbol)
       = (j.intent_id, j.position_id, j.tenant_id, j.venue_id, j.account_scope, l.symbol)
     AND a.superseded_at IS NULL AND a.decided_month = l.decided_month
    JOIN public.spot_order_set_generation g
      ON (g.tenant_id, g.venue_id, g.account_scope) = (j.tenant_id, j.venue_id, j.account_scope)
    JOIN LATERAL (
        SELECT epoch_id, clean_stop_at FROM public.process_epochs
        WHERE tenant_id = g.tenant_id ORDER BY epoch_seq DESC LIMIT 1
    ) epoch ON epoch.epoch_id = g.process_epoch AND epoch.clean_stop_at IS NULL
    WHERE j.intent_id = $7 AND (j.tenant_id, j.venue_id, j.account_scope) = ($1, 'binance:spot', $2)
      AND (g.generation_id, g.process_epoch, g.source_audit_id) = ($3, $4, $5)
      AND g.accounting_version = $6::bigint - 1
      AND j.client_order_id = $8 AND j.position_id = $9 AND l.symbol = $10
      AND j.kind = 'entry'
      -- No linked outcome: an Entry outcome can change a CANCEL mask.
      AND NOT j.not_sent AND j.venue_outcome IS NULL AND j.venue_evidence IS NULL
      AND j.uncertain_at IS NULL
      -- No native reverse edge on the entry after the source effects.
      AND NOT EXISTS (SELECT 1 FROM public.venue_orders o WHERE o.intent_id = j.intent_id)
      -- Admission replay preserves the exact current leg. Only birth can
      -- newly attribute its historical unbound members.
      AND (NOT $11::boolean OR (
        -- M41's positive unbound SELL hint: an active member would gain
        -- attribution through this leg.
        NOT EXISTS (
          SELECT 1 FROM public.spot_order_set_sell_intents s
          WHERE (s.tenant_id, s.venue_id, s.account_scope, s.generation_id, s.source_position)
              = (j.tenant_id, j.venue_id, j.account_scope, g.generation_id, j.position_id)
            AND s.scope_kind = 'unbound' AND s.source_position IS NOT NULL
            AND (s.potential OR s.conflict)
      )
        -- The CANCEL member's preserved original position, active or inactive,
        -- where its scoped pair is NULL.
      AND NOT EXISTS (
          SELECT 1 FROM public.spot_order_set_cancel_members m
          WHERE (m.tenant_id, m.venue_id, m.account_scope, m.generation_id, m.source_position)
              = (j.tenant_id, j.venue_id, j.account_scope, g.generation_id, j.position_id)
            AND m.position_id IS NULL AND m.source_position IS NOT NULL
      )
      ))
)
UPDATE public.spot_order_set_generation g
SET accounting_version = $6
FROM entry e
WHERE (g.tenant_id, g.venue_id, g.account_scope, g.generation_id)
    = (e.tenant_id, e.venue_id, e.account_scope, e.generation_id)
  AND (g.process_epoch, g.source_audit_id) = ($4, $5)
  AND g.accounting_version = $6::bigint - 1
RETURNING g.accounting_version
