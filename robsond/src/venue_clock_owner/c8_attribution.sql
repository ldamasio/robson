-- One bounded scoped snapshot for the C8 attribution witness (fixture only).
-- $1..$6 are the exact ExpectedGeneration; $7 the bound venue UID; $8 the
-- expected current accounting version. No row means the scope, UID,
-- generation or version moved: the caller refuses. $9/$10 are the exact
-- operated (symbol, exchange_order_id) keys, already capped by the caller.
-- Probes use the unique key only; tenant, client, side and J scope are
-- returned and judged by the caller so disagreement never reads as absence.
WITH g AS (
    SELECT g.generation_id, g.accounting_version, g.position_hints_complete
    FROM public.spot_order_set_generation g
    JOIN public.accounting_state a
      ON (a.tenant_id,a.venue_id)=(g.tenant_id,g.venue_id)
    JOIN public.venue_accounts va
      ON (va.venue_id,va.account_scope,va.tenant_id)=(g.venue_id,g.account_scope,g.tenant_id)
     AND va.uid=$7
    JOIN LATERAL (
        SELECT epoch_id,clean_stop_at FROM public.process_epochs
        WHERE tenant_id=g.tenant_id ORDER BY epoch_seq DESC LIMIT 1
    ) epoch ON epoch.epoch_id=g.process_epoch AND epoch.clean_stop_at IS NULL
    WHERE (g.tenant_id,g.venue_id,g.account_scope)=($1,$2,$3)
      AND (g.generation_id,g.process_epoch,g.source_audit_id)=($4,$5,$6)
      AND g.accounting_version=a.version
      AND g.accounting_version=$8
      AND g.position_hints_complete
), k AS (
    SELECT symbol, order_id, ord
    FROM unnest($9::text[], $10::text[]) WITH ORDINALITY AS k(symbol, order_id, ord)
)
SELECT g.accounting_version,
       k.ord, k.symbol, k.order_id,
       o.tenant_id AS o_tenant, o.client_order_id AS o_client, o.side AS o_side,
       o.phase AS o_phase, o.owner AS o_owner, o.intent_id AS o_intent,
       o.position_id AS o_position, o.kind AS o_kind, o.order_type AS o_type,
       j.intent_id AS j_intent, j.tenant_id AS j_tenant, j.venue_id AS j_venue,
       j.account_scope AS j_scope, j.client_order_id AS j_client,
       j.position_id AS j_position, j.kind AS j_kind,
       j.not_sent AS j_not_sent, j.venue_outcome AS j_outcome
FROM g
LEFT JOIN k ON true
LEFT JOIN public.venue_orders o
  ON (o.venue_id,o.account_scope,o.symbol,o.exchange_order_id)=($2,$3,k.symbol,k.order_id)
LEFT JOIN public.order_intents j ON j.intent_id=o.intent_id
ORDER BY k.ord
