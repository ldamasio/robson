-- record_order projection, part 1 of 4 (order_source, order_sell, order_cancel,
-- order_write; concatenated into ONE statement). The caller owns the
-- tenant/venue accounting row and has applied the native order and leg
-- effects in this same transaction. $1..$6 are the exact generation binding
-- and the version its bump returned; $7/$8 the native key's symbol and
-- exchange ID. $9..$15 fingerprint the locked pre-state: existence, owner,
-- intent, position, kind, phase and status. Keyed probes only: the native
-- row by its full key, one J by PK, that J's leg by PK, one attempt by its
-- partial unique key and the touched member/aggregate rows. No row is a
-- refusal; the caller then rolls back every native write.
WITH gen AS MATERIALIZED (
    SELECT g.tenant_id, g.venue_id, g.account_scope, g.generation_id
    FROM public.spot_order_set_generation g
    JOIN LATERAL (
        SELECT epoch_id, clean_stop_at FROM public.process_epochs
        WHERE tenant_id = g.tenant_id ORDER BY epoch_seq DESC LIMIT 1
    ) epoch ON epoch.epoch_id = g.process_epoch AND epoch.clean_stop_at IS NULL
    WHERE (g.tenant_id, g.venue_id, g.account_scope) = ($1, 'binance:spot', $2)
      AND (g.generation_id, g.process_epoch, g.source_audit_id) = ($3, $4, $5)
      AND g.accounting_version = $6::bigint - 1
), native AS MATERIALIZED (
    SELECT g.generation_id, o.tenant_id, o.venue_id, o.account_scope, o.symbol,
           o.exchange_order_id, o.owner, o.intent_id, o.position_id, o.kind, o.side,
           o.phase, o.status, o.client_order_id,
           j.intent_id IS NOT NULL AS has_j, j.kind AS j_kind, j.position_id AS j_position,
           j.client_order_id AS j_client, j.not_sent AS j_not_sent,
           j.venue_outcome IS NOT DISTINCT FROM 'rejected' AS j_rejected,
           j.venue_outcome IS NULL AND j.venue_evidence IS NULL AS j_no_answer,
           CASE WHEN j.venue_outcome = 'rejected' AND NOT j.not_sent
                     AND j.venue_evidence IS NULL THEN EXISTS (
               SELECT 1 FROM public.submit_attempts s
               WHERE s.venue_id = 'binance:spot'
                 AND (s.account_scope, s.client_order_id) = (j.account_scope, j.client_order_id)
                 AND s.state = 'rejected' AND s.rejected_at IS NOT NULL
           ) ELSE false END AS j_definite_rejection,
           CASE WHEN ledger_canonical_id(j.cancels_order_id) THEN j.cancels_order_id END AS j_target,
           l.symbol AS j_leg_symbol
    FROM gen g
    JOIN public.venue_orders o
      ON (o.venue_id, o.account_scope, o.symbol, o.exchange_order_id)
       = (g.venue_id, g.account_scope, $7, $8)
     AND o.tenant_id = g.tenant_id
    LEFT JOIN LATERAL (
        SELECT intent_id, tenant_id, venue_id, account_scope, position_id, client_order_id,
               kind, not_sent, venue_outcome, venue_evidence, cancels_order_id
        FROM public.order_intents WHERE intent_id = o.intent_id LIMIT 1
    ) j ON true
    LEFT JOIN LATERAL (
        SELECT symbol FROM public.balance_legs
        WHERE (position_id, tenant_id, venue_id, account_scope)
            = (j.position_id, j.tenant_id, j.venue_id, j.account_scope)
        LIMIT 1
    ) l ON true
    -- A named J in another tenant, venue or account never casts across scope.
    WHERE (j.intent_id IS NULL
           OR (j.tenant_id, j.venue_id, j.account_scope) = (o.tenant_id, o.venue_id, o.account_scope))
      -- Absence or an unlinked prior may gain a link; a persisted link never changes.
      AND (NOT $9::boolean OR $11::uuid IS NULL
           OR ((o.intent_id, o.owner, o.position_id) = ($11::uuid, $10::text, $12::uuid)
               AND o.kind IS NOT DISTINCT FROM $13::text))
), facts AS MATERIALIZED (
    SELECT n.*,
           n.has_j AND n.j_kind IN ('protection', 'exit') AS suspected,
           n.has_j AND $9::boolean AND $11::uuid IS NOT NULL AS prior_linked,
           CASE n.j_kind WHEN 'protection' THEN 'stop' WHEN 'exit' THEN 'sell' END AS sell_kind,
           CASE n.j_kind WHEN 'entry' THEN 'entry'
                         WHEN 'protection' THEN 'stop' WHEN 'exit' THEN 'sell' END AS target_kind,
           n.phase = 'terminal'
               AND n.status IN ('FILLED', 'CANCELED', 'EXPIRED', 'EXPIRED_IN_MATCH', 'REJECTED')
               AS recognized,
           coalesce($14::text = 'terminal'
               AND $15::text IN ('FILLED', 'CANCELED', 'EXPIRED', 'EXPIRED_IN_MATCH', 'REJECTED'),
               false) AS prior_recognized
    FROM native n
), links AS MATERIALIZED (
    -- sell_shadow_exact_link and cancel_shadow_sync_target's terminal proof,
    -- once for the persisted row and once for the locked prior fingerprint.
    SELECT f.*,
           f.has_j AND (f.j_not_sent OR f.j_rejected) AS contradicted,
           f.prior_linked AND (f.j_not_sent OR f.j_rejected) AS prior_contradicted,
           coalesce(f.has_j AND f.owner = 'leg' AND f.side = 'SELL'
               AND (f.j_position, f.j_client) = (f.position_id, f.client_order_id)
               AND f.kind = f.sell_kind, false) AS exact,
           coalesce(f.prior_linked AND $10::text = 'leg' AND f.side = 'SELL'
               AND (f.j_position, f.j_client) = ($12::uuid, f.client_order_id)
               AND $13::text = f.sell_kind, false) AS prior_exact,
           coalesce(f.owner = 'leg' AND f.recognized AND f.has_j
               AND (f.j_position, f.j_client) = (f.position_id, f.client_order_id)
               AND NOT f.j_not_sent AND f.kind = f.target_kind, false) AS mask_next,
           coalesce($10::text = 'leg' AND f.prior_recognized AND f.prior_linked
               AND (f.j_position, f.j_client) = ($12::uuid, f.client_order_id)
               AND NOT f.j_not_sent AND $13::text = f.target_kind, false) AS mask_prior
    FROM facts f
)
