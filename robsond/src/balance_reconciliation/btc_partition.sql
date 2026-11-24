-- 8k-2 pre statement (v16 §3.10, §C8): one autocommit statement on a fresh
-- read-only connection; nothing is held across the inventory requests.
-- $1..$6 are the exact ExpectedGeneration, $7 the bound venue UID and $8 the
-- exact operated pair. No row means the scope, UID, generation or epoch is
-- not current. Amounts stay NUMERIC per row: every sum is the caller's exact
-- Rust arithmetic, never a decoded SQL aggregate.
WITH g AS MATERIALIZED (
    SELECT g.accounting_version
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
      AND g.position_hints_complete
), legs AS MATERIALIZED (
    SELECT l.*
    FROM public.balance_legs l
    WHERE (l.tenant_id,l.venue_id,l.account_scope,l.symbol)=($1,$2,$3,$8)
      AND EXISTS (SELECT 1 FROM g)
), open_legs AS MATERIALIZED (
    SELECT * FROM legs WHERE closed_at IS NULL
), intents AS MATERIALIZED (
    -- Candidates: open-ended entry, protection, exit and sweep intents; cancels
    -- with no definite outcome; and every open leg's current admission intent.
    SELECT j.*, ol.position_id AS admission_of
    FROM public.order_intents j
    LEFT JOIN open_legs ol
      ON (ol.admission_intent_id,ol.position_id)=(j.intent_id,j.position_id)
    WHERE (j.tenant_id,j.venue_id,j.account_scope)=($1,$2,$3)
      AND EXISTS (SELECT 1 FROM g)
      AND (ol.position_id IS NOT NULL
        OR (j.kind IN ('entry','protection','exit','sweep_close')
            AND NOT j.not_sent AND j.disposition_kind IS NULL
            AND (j.venue_outcome IS NULL OR j.venue_outcome='success'))
        OR (j.kind='cancel'
            AND NOT j.not_sent AND j.disposition_kind IS NULL AND j.venue_outcome IS NULL))
)
SELECT 0 AS rank, 'header'::text AS kind, g.accounting_version AS version,
       (SELECT count(*) FROM legs WHERE closed_at IS NOT NULL) AS closed_legs,
       NULL::uuid AS position_id, NULL::uuid AS admission_intent_id,
       NULL::numeric AS gross, NULL::numeric AS base_fee, NULL::boolean AS base_final,
       NULL::numeric AS executable, NULL::numeric AS consumed, NULL::text AS recovery_state,
       NULL::bigint AS entry_orders, NULL::boolean AS entry_linked, NULL::numeric AS requested,
       NULL::text AS order_id, NULL::text AS order_kind, NULL::text AS order_phase,
       NULL::numeric AS orig_qty, NULL::numeric AS executed,
       NULL::uuid AS intent_id, NULL::text AS intent_kind, NULL::boolean AS not_sent,
       NULL::text AS venue_outcome, NULL::text AS disposition_kind,
       NULL::timestamptz AS uncertain_at, NULL::text AS client_order_id,
       NULL::uuid AS admission_of, NULL::boolean AS target_terminal_proof,
       NULL::text AS o_owner, NULL::text AS o_kind, NULL::uuid AS o_position,
       NULL::uuid AS o_tenant, NULL::text AS o_venue, NULL::text AS o_scope,
       NULL::text AS o_client, NULL::text AS o_phase, NULL::numeric AS o_executed,
       NULL::text AS o_order_id
FROM g
UNION ALL
-- One row per open leg of the operated pair, with its entry-order count and
-- the current admission's requested quantity.
SELECT 1, 'leg', g.accounting_version, NULL,
       l.position_id, l.admission_intent_id,
       l.gross_qty, l.base_commission, l.base_commission_final,
       l.executable_qty, l.consumed_cum, l.recovery_state,
       e.entry_orders, e.entry_linked, ad.requested_qty,
       NULL, NULL, NULL, NULL, NULL,
       NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL,
       NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL
FROM g
CROSS JOIN open_legs l
CROSS JOIN LATERAL (
    SELECT count(*) AS entry_orders,
           coalesce(bool_and(o.intent_id = l.admission_intent_id), true) AS entry_linked
    FROM public.venue_orders o
    WHERE (o.tenant_id,o.venue_id,o.account_scope,o.position_id)
        = (l.tenant_id,l.venue_id,l.account_scope,l.position_id)
      AND o.owner='leg' AND o.kind='entry'
) e
LEFT JOIN public.balance_leg_admissions ad
  ON (ad.entry_intent_id,ad.position_id)=(l.admission_intent_id,l.position_id)
 AND ad.superseded_at IS NULL
UNION ALL
-- Robson's sell-capable orders of those legs that are not terminal.
SELECT 2, 'order', g.accounting_version, NULL,
       l.position_id, NULL,
       NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL,
       o.exchange_order_id, o.kind, o.phase, o.orig_qty, o.executed_cum,
       o.intent_id, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL,
       NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL
FROM g
CROSS JOIN open_legs l
JOIN public.venue_orders o
  ON (o.tenant_id,o.venue_id,o.account_scope,o.position_id)
   = (l.tenant_id,l.venue_id,l.account_scope,l.position_id)
 AND o.owner='leg' AND o.kind IN ('stop','sell') AND o.phase IN ('live','unknown')
UNION ALL
-- Candidate intents with the ledger-backing columns of their linked order
-- (at most one, idx_venue_orders_intent) and, for an uncertain cancel, the
-- accepted exact terminal-target proof (journal_cancel_outcome.sql).
SELECT 3, 'intent', g.accounting_version, NULL,
       j.position_id, NULL,
       NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL,
       NULL, NULL, NULL, NULL, NULL,
       j.intent_id, j.kind, j.not_sent, j.venue_outcome, j.disposition_kind,
       j.uncertain_at, j.client_order_id, j.admission_of,
       CASE WHEN j.kind='cancel' AND j.venue_outcome IS NULL THEN coalesce((
           SELECT t_o.owner = 'leg'
              AND (t_o.tenant_id, t_o.position_id) = (j.tenant_id, j.position_id)
              AND t_o.phase = 'terminal'
              AND t_o.status IN ('FILLED','CANCELED','EXPIRED','EXPIRED_IN_MATCH','REJECTED')
              AND (t.tenant_id, t.venue_id, t.account_scope, t.position_id, t.client_order_id)
                = (t_o.tenant_id, t_o.venue_id, t_o.account_scope, t_o.position_id, t_o.client_order_id)
              AND NOT t.not_sent
              AND t_o.kind = CASE t.kind WHEN 'entry' THEN 'entry'
                                         WHEN 'protection' THEN 'stop' WHEN 'exit' THEN 'sell' END
           FROM public.balance_legs bl
           JOIN public.venue_orders t_o
             ON (t_o.venue_id, t_o.account_scope, t_o.symbol, t_o.exchange_order_id)
              = (j.venue_id, j.account_scope, bl.symbol, j.cancels_order_id)
           JOIN public.order_intents t ON t.intent_id = t_o.intent_id
           WHERE (bl.position_id, bl.tenant_id, bl.venue_id, bl.account_scope)
               = (j.position_id, j.tenant_id, j.venue_id, j.account_scope)
       ), false) END,
       o.owner, o.kind, o.position_id, o.tenant_id, o.venue_id, o.account_scope,
       o.client_order_id, o.phase, o.executed_cum, o.exchange_order_id
FROM g
CROSS JOIN intents j
LEFT JOIN public.venue_orders o ON o.intent_id=j.intent_id
ORDER BY 1, 5, 16, 21
-- PRE_ROW_CAP + 1 (8k-3a): at most 65 rows are returned; the header ranks
-- first, so the limit never drops it, and a 65th row is refused as Cap
-- before any row is decoded.
LIMIT 65
