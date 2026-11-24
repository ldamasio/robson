-- ADR-0056 step 8l-2: the settlement barrier and the validity of one UTC month, current or historical, for one
-- balance-account venue (v16 §3.7.3; PLAN-r4 §6.3). One read-only statement that takes no row lock; the caller
-- owns its transaction and bounds. $1 tenant, $2 venue, $3 the month as its first day, $4 now (Robson's wall
-- clock), $5..$7 per symbol the commission asset a buy ($6, base) and a sell ($7, quote) must carry.
--
-- A leg is unsettled when an order of it is unknown, or the leg is entry_unknown; an intent of it other than a
-- cancel (which executes nothing) has no venue_orders row and is neither not_sent nor rejected; an order's fills
-- are incomplete; X is set and B is not final; a fill's commission asset is not the expected one (a symbol with
-- no expected assets expects none); its stream suspension is on record; or it is closed and X differs from
-- consumed_cum + R (with X and no dust row there is no R). An intent naming no leg of the venue counts as one.
-- Only the intent's own order, of its tenant, venue, scope and leg, is its evidence.
--
-- Lives, all closed intervals widened by v16's 2 s margin for Robson's times: an unsettled order's
-- [min(intent created_at, live_seen_at), terminal_seen_at or now]; an unsettled intent's [created_at, now]; for
-- a leg-wide condition (entry_unknown, provisional B, suspension, broken X), every order of the leg and the
-- leg's own [created_at, closed_at or now]. No life ends before it starts. A life meets the half-open month
-- [first, next) when it starts before next and ends at or after first, so a month is valid again only once
-- every such life ends strictly before its first instant. A month is also invalid with an
-- accounting_invalidations row for it, or inside a recovery re-anchor's span (operator decision Q2 = R, 8l-5b): from
-- the UTC month of the anchor it rests on (the latest clean-hull one) through the month of its own anchor. A venue
-- without its accounting row has no answer.
WITH legs AS MATERIALIZED (
    SELECT l.position_id, l.created_at, l.closed_at,
           l.recovery_state IS NOT DISTINCT FROM 'entry_unknown' AS entry_unknown,
           l.executable_qty IS NOT NULL AND NOT l.base_commission_final AS provisional_fee,
           EXISTS (SELECT 1 FROM public.stream_suspensions s
                   WHERE (s.tenant_id, s.stream_key) = (l.tenant_id, 'position:' || l.position_id::text)
                     AND s.cleared_at IS NULL) AS suspended,
           l.closed_at IS NOT NULL
               AND ((l.executable_qty IS NOT NULL AND d.position_id IS NULL)
                    OR coalesce(l.executable_qty, 0) <> l.consumed_cum + coalesce(d.write_off, 0)) AS broken_identity
    FROM public.balance_legs l
    LEFT JOIN public.dust_ledger d ON d.position_id = l.position_id
    WHERE (l.tenant_id, l.venue_id) = ($1, $2)
), orders AS MATERIALIZED (
    SELECT o.position_id, least(i.created_at, o.live_seen_at) AS born, o.terminal_seen_at AS ended,
           o.phase = 'unknown' AS unknown, NOT o.fills_complete AS incomplete,
           EXISTS (SELECT 1 FROM public.order_fills f
                   WHERE (f.venue_id, f.account_scope, f.symbol, f.exchange_order_id)
                       = (o.venue_id, o.account_scope, o.symbol, o.exchange_order_id)
                     AND f.commission_asset IS DISTINCT FROM
                         CASE o.side WHEN 'BUY' THEN a.base ELSE a.quote END) AS odd_asset
    FROM public.venue_orders o
    LEFT JOIN public.order_intents i ON i.intent_id = o.intent_id
         AND (i.tenant_id, i.venue_id, i.account_scope, i.position_id)
             = (o.tenant_id, o.venue_id, o.account_scope, o.position_id)
    LEFT JOIN unnest($5::text[], $6::text[], $7::text[]) AS a(symbol, base, quote) ON a.symbol = o.symbol
    WHERE (o.tenant_id, o.venue_id, o.owner) = ($1, $2, 'leg')
), intents AS MATERIALIZED (
    SELECT coalesce(i.position_id, '00000000-0000-0000-0000-000000000000'::uuid) AS position_id, i.created_at
    FROM public.order_intents i
    WHERE (i.tenant_id, i.venue_id) = ($1, $2) AND i.kind <> 'cancel'
      AND NOT i.not_sent AND i.venue_outcome IS DISTINCT FROM 'rejected'
      AND NOT EXISTS (SELECT 1 FROM public.venue_orders o
                      WHERE o.intent_id = i.intent_id AND o.owner = 'leg'
                        AND (o.tenant_id, o.venue_id, o.account_scope, o.position_id)
                            = (i.tenant_id, i.venue_id, i.account_scope, i.position_id))
), verdict AS MATERIALIZED (
    SELECT k.position_id,
           coalesce(g.entry_unknown, false) OR coalesce(o.unknown, false) AS unknown_order,
           k.position_id IN (SELECT position_id FROM intents) AS unrowed_intent,
           coalesce(o.incomplete, false) AS incomplete_fills,
           coalesce(g.provisional_fee, false) AS provisional_fee,
           coalesce(o.odd_asset, false) AS unexpected_asset,
           coalesce(g.suspended, false) AS suspended,
           coalesce(g.broken_identity, false) AS broken_identity,
           coalesce(g.entry_unknown OR g.provisional_fee OR g.suspended OR g.broken_identity, false) AS leg_wide
    FROM (SELECT position_id FROM legs UNION SELECT position_id FROM intents) k
    LEFT JOIN legs g ON g.position_id = k.position_id
    LEFT JOIN (SELECT position_id, bool_or(unknown) AS unknown, bool_or(incomplete) AS incomplete,
                      bool_or(odd_asset) AS odd_asset
               FROM orders GROUP BY position_id) o ON o.position_id = k.position_id
), lives AS (
    SELECT o.born, o.ended FROM orders o JOIN verdict v ON v.position_id = o.position_id
    WHERE o.unknown OR o.incomplete OR o.odd_asset OR v.leg_wide
    UNION ALL
    SELECT g.created_at, g.closed_at FROM legs g JOIN verdict v ON v.position_id = g.position_id
    WHERE v.leg_wide
    UNION ALL
    SELECT created_at, NULL FROM intents
)
SELECT count(*) FILTER (WHERE unknown_order OR unrowed_intent OR incomplete_fills OR provisional_fee
                              OR unexpected_asset OR suspended OR broken_identity) AS unsettled,
       count(*) FILTER (WHERE unknown_order) AS unknown_order,
       count(*) FILTER (WHERE unrowed_intent) AS unrowed_intent,
       count(*) FILTER (WHERE incomplete_fills) AS incomplete_fills,
       count(*) FILTER (WHERE provisional_fee) AS provisional_fee,
       count(*) FILTER (WHERE unexpected_asset) AS unexpected_asset,
       count(*) FILTER (WHERE suspended) AS suspended,
       count(*) FILTER (WHERE broken_identity) AS broken_identity,
       EXISTS (SELECT 1 FROM lives
               WHERE lives.born - interval '2 seconds' < ($3::date + interval '1 month') AT TIME ZONE 'UTC'
                 AND greatest(coalesce(lives.ended, $4), lives.born) + interval '2 seconds'
                     >= $3::date::timestamp AT TIME ZONE 'UTC') AS life_meets_month,
       (SELECT count(*) FROM public.accounting_invalidations x
        WHERE (x.tenant_id, x.venue_id, x.month) = ($1, $2, $3::date))
       + (SELECT count(*) FROM public.cash_reanchors r
          JOIN public.cash_anchors o ON o.anchor_id = r.old_anchor_id
          JOIN public.cash_anchors n ON n.anchor_id = r.new_anchor_id
          WHERE (r.tenant_id, r.venue_id, r.mode) = ($1, $2, 'recovery')
            AND $3::date BETWEEN date_trunc('month', o.as_of AT TIME ZONE 'UTC')::date
                             AND date_trunc('month', n.as_of AT TIME ZONE 'UTC')::date) AS invalidations,
       EXISTS (SELECT 1 FROM public.accounting_state s WHERE (s.tenant_id, s.venue_id) = ($1, $2)) AS venue_known
FROM verdict
