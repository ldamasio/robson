-- The caller already owns the tenant/venue accounting row and has applied the
-- outcome of one previously persisted grouped cancel in this same transaction.
-- Exact keys only: no sibling member is read or written.
WITH cancel AS MATERIALIZED (
    SELECT j.intent_id, j.tenant_id, j.venue_id, j.account_scope, j.position_id,
           j.cancels_order_id AS target_id, l.symbol, g.generation_id, g.position_hints_complete,
           named.named_rows > 0
               OR (j.not_sent AND (j.venue_outcome IS NOT NULL OR j.venue_evidence IS NOT NULL))
               AS conflict,
           j.not_sent
    FROM public.order_intents j
    JOIN public.balance_legs l
      ON (l.position_id, l.tenant_id, l.venue_id, l.account_scope)
       = (j.position_id, j.tenant_id, j.venue_id, j.account_scope)
    JOIN public.spot_order_set_generation g
      ON (g.tenant_id, g.venue_id, g.account_scope) = (j.tenant_id, j.venue_id, j.account_scope)
    JOIN LATERAL (
        SELECT epoch_id, clean_stop_at FROM public.process_epochs
        WHERE tenant_id = g.tenant_id ORDER BY epoch_seq DESC LIMIT 1
    ) epoch ON epoch.epoch_id = g.process_epoch AND epoch.clean_stop_at IS NULL
    -- One reverse probe on idx_venue_orders_intent. A named row outside this
    -- leg's owner, venue or account refuses.
    CROSS JOIN LATERAL (
        SELECT count(*) AS named_rows,
               coalesce(bool_and(o.owner = 'leg'
                   AND (o.tenant_id, o.venue_id, o.account_scope)
                     = (j.tenant_id, j.venue_id, j.account_scope)), true) AS owned
        FROM public.venue_orders o WHERE o.intent_id = j.intent_id
    ) named
    WHERE j.intent_id = $7 AND (j.tenant_id, j.venue_id, j.account_scope) = ($1, 'binance:spot', $2)
      AND (g.generation_id, g.process_epoch, g.source_audit_id) = ($3, $4, $5)
      AND g.accounting_version = $6::bigint - 1
      AND j.client_order_id = $8 AND j.position_id = $9 AND j.cancels_order_id = $10
      AND j.kind = 'cancel' AND ledger_canonical_id(j.cancels_order_id)
      AND named.owned
), classified AS (
    SELECT c.*, NOT c.not_sent OR c.conflict AS pending
    FROM cancel c
), evidence AS MATERIALIZED (
    -- first_cancel.sql's qualified terminal proof, from the exact target PK and
    -- its original journal row. Unknown evidence is no mask.
    SELECT c.*,
           coalesce((
               SELECT o.owner = 'leg'
                  AND (o.tenant_id, o.position_id) = (c.tenant_id, c.position_id)
                  AND o.phase = 'terminal'
                  AND o.status IN ('FILLED', 'CANCELED', 'EXPIRED', 'EXPIRED_IN_MATCH', 'REJECTED')
                  AND (t.tenant_id, t.venue_id, t.account_scope, t.position_id, t.client_order_id)
                    = (o.tenant_id, o.venue_id, o.account_scope, o.position_id, o.client_order_id)
                  AND NOT t.not_sent
                  AND o.kind = CASE t.kind WHEN 'entry' THEN 'entry'
                                           WHEN 'protection' THEN 'stop' WHEN 'exit' THEN 'sell' END
               FROM public.venue_orders o
               JOIN public.order_intents t ON t.intent_id = o.intent_id
               WHERE (o.venue_id, o.account_scope, o.symbol, o.exchange_order_id)
                   = (c.venue_id, c.account_scope, c.symbol, c.target_id)
           ), false) AS terminal_proof
    FROM classified c
), prior AS MATERIALIZED (
    -- The exact persisted member must equal the contribution captured before
    -- the source write, and its group mask must agree with the native proof.
    SELECT e.*,
           m.source_position AS prior_source_position,
           p.pending_count, p.conflict_count AS group_conflict_count,
           l.active_count AS leg_active_count, l.conflict_count AS leg_conflict_count,
           e.pending::integer - $11::boolean::integer AS pending_delta,
           e.conflict::integer - $12::boolean::integer AS conflict_delta
    FROM evidence e
    JOIN public.spot_order_set_cancel_members m
      ON (m.tenant_id, m.venue_id, m.account_scope, m.generation_id, m.intent_id)
       = (e.tenant_id, e.venue_id, e.account_scope, e.generation_id, e.intent_id)
    JOIN public.spot_order_set_cancel_groups p
      ON (p.tenant_id, p.venue_id, p.account_scope, p.generation_id, p.symbol, p.position_id, p.target_id)
       = (e.tenant_id, e.venue_id, e.account_scope, e.generation_id, e.symbol, e.position_id, e.target_id)
    LEFT JOIN public.spot_order_set_cancel_legs l
      ON (l.tenant_id, l.venue_id, l.account_scope, l.generation_id, l.symbol, l.position_id)
       = (e.tenant_id, e.venue_id, e.account_scope, e.generation_id, e.symbol, e.position_id)
    WHERE (m.symbol, m.position_id, m.target_id) = (e.symbol, e.position_id, e.target_id)
      AND (m.source_position IS NOT DISTINCT FROM e.position_id
           OR (NOT e.position_hints_complete AND m.source_position IS NULL))
      AND (m.pending, m.conflict) = ($11::boolean, $12::boolean)
      AND p.terminal_mask = e.terminal_proof
      AND p.pending_count >= m.pending::integer
      AND p.conflict_count >= m.conflict::integer
), changes AS MATERIALIZED (
    -- A terminal group keeps pending evidence with no active contribution;
    -- conflict counts on the leg independently of the mask.
    SELECT p.*,
           p.pending_delta <> 0 OR p.conflict_delta <> 0 AS group_changed,
           (p.pending_delta <> 0 OR p.conflict_delta <> 0
               OR p.prior_source_position IS DISTINCT FROM p.position_id) AS member_changed,
           CASE WHEN p.terminal_proof THEN 0 ELSE p.pending_delta END AS active_delta
    FROM prior p
), leg_next AS MATERIALIZED (
    SELECT c.*,
           c.active_delta <> 0 OR c.conflict_delta <> 0 AS leg_changed,
           coalesce(c.leg_active_count, 0) + c.active_delta AS leg_active_next,
           coalesce(c.leg_conflict_count, 0) + c.conflict_delta AS leg_conflict_next
    FROM changes c
    WHERE coalesce(c.leg_active_count, 0)
              >= CASE WHEN c.terminal_proof THEN 0 ELSE c.pending_count END
      AND coalesce(c.leg_conflict_count, 0) >= c.group_conflict_count
      AND c.pending_count + c.pending_delta >= 0
      AND c.group_conflict_count + c.conflict_delta >= 0
      AND coalesce(c.leg_active_count, 0) + c.active_delta >= 0
      AND coalesce(c.leg_conflict_count, 0) + c.conflict_delta >= 0
), member_write AS (
    UPDATE public.spot_order_set_cancel_members m
    SET pending = c.pending, conflict = c.conflict, source_position = c.position_id
    FROM leg_next c
    WHERE (m.tenant_id, m.venue_id, m.account_scope, m.generation_id, m.intent_id)
        = (c.tenant_id, c.venue_id, c.account_scope, c.generation_id, c.intent_id)
      AND (m.pending, m.conflict) = ($11::boolean, $12::boolean)
      AND m.source_position IS NOT DISTINCT FROM c.prior_source_position
      AND c.member_changed
    RETURNING m.intent_id
), group_write AS (
    UPDATE public.spot_order_set_cancel_groups p
    SET pending_count = c.pending_count + c.pending_delta,
        conflict_count = c.group_conflict_count + c.conflict_delta
    FROM leg_next c
    WHERE (p.tenant_id, p.venue_id, p.account_scope, p.generation_id, p.symbol, p.position_id, p.target_id)
        = (c.tenant_id, c.venue_id, c.account_scope, c.generation_id, c.symbol, c.position_id, c.target_id)
      AND (p.pending_count, p.conflict_count, p.terminal_mask)
        = (c.pending_count, c.group_conflict_count, c.terminal_proof)
      AND c.group_changed
    RETURNING p.target_id
), leg_write AS (
    -- An absent leg stays absent unless this member changes it; an explicit
    -- zero is retained.
    INSERT INTO public.spot_order_set_cancel_legs (
        tenant_id, venue_id, account_scope, generation_id, symbol, position_id,
        active_count, conflict_count
    )
    SELECT tenant_id, venue_id, account_scope, generation_id, symbol, position_id,
           leg_active_next, leg_conflict_next
    FROM leg_next WHERE leg_changed
    ON CONFLICT (tenant_id, venue_id, account_scope, generation_id, symbol, position_id)
    DO UPDATE SET active_count = excluded.active_count, conflict_count = excluded.conflict_count
    WHERE (spot_order_set_cancel_legs.active_count, spot_order_set_cancel_legs.conflict_count)
        = (SELECT coalesce(leg_active_count, 0), coalesce(leg_conflict_count, 0) FROM leg_next)
    RETURNING position_id
)
UPDATE public.spot_order_set_generation g
SET accounting_version = $6
FROM leg_next c
WHERE (g.tenant_id, g.venue_id, g.account_scope, g.generation_id)
    = (c.tenant_id, c.venue_id, c.account_scope, c.generation_id)
  AND g.accounting_version = $6::bigint - 1
  AND (SELECT count(*) FROM member_write) = c.member_changed::integer
  AND (SELECT count(*) FROM group_write) = c.group_changed::integer
  AND (SELECT count(*) FROM leg_write) = c.leg_changed::integer
RETURNING g.accounting_version
