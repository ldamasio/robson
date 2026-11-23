-- ADR-0056 step 8p-4: a balance-account venue's leg facts as of a month boundary B (8p PLAN-r3 §6.4, §6.3), as
-- 8p-3's `CandidateLeg` JSON (amounts as exact decimal text). One read-only statement on the caller's connection;
-- it takes no row lock. $1 tenant, $2 venue, $3 B (the first instant of month K, UTC).
--
-- Candidates: the venue's legs of a cohort before K, unless proven finished before B (a dust row whose close has
-- a valid verdict at or before B - 2 s and whose last fill is before B). One row each, by position: X, the
-- current admission's terms, the dust row, every order of the leg with every booked trade, and every cancel
-- intent of the leg (any outcome), in a fixed order. Each Robson time comes with its stored verdict. An
-- intent's first attempt is its earliest; its verdict is valid only when every attempt's is (otherwise which
-- came first is unknown), invalid when any attempt's is invalid, else none.
-- A stop or sell that §6.3's exclusion rule shows not open at B because Robson saw it terminal by B - 2 s (valid
-- verdict) with every trade before B is summarized, not listed: its trades count in summarized_sold_before_b, a
-- sum given only when a ledger amount can hold it exactly (otherwise null, which refuses the read).
-- The venue's own order times are never selected (source guard `leg_facts_never_select_venue_times`).
WITH legs AS (
    SELECT l.position_id, jsonb_build_object('position_id', l.position_id, 'cohort_month', l.cohort_month,
        'executable', l.executable_qty::TEXT, 'terms', jsonb_build_object('gap_bps', a.gap_bps::TEXT,
            'taker_bps', a.taker_bps::TEXT, 'remainder_reserve', a.remainder_reserve::TEXT,
            'admission_charge', a.admission_charge::TEXT),
        'dust', CASE WHEN d.position_id IS NOT NULL THEN jsonb_build_object('write_off', d.write_off::TEXT,
            'close_at', jsonb_build_object('at', d.close_at, 'verdict', d.close_clock_verdict),
            'last_fill_at', d.last_fill_at) END) AS leg
    FROM public.balance_legs l
    JOIN public.balance_leg_admissions a ON a.entry_intent_id = l.admission_intent_id
    LEFT JOIN public.dust_ledger d ON d.position_id = l.position_id
    WHERE (l.tenant_id, l.venue_id) = ($1, $2) AND l.cohort_month < ($3 AT TIME ZONE 'UTC')::DATE
        AND (d.close_clock_verdict = 'valid' AND d.close_at <= $3 - INTERVAL '2 s' AND d.last_fill_at < $3)
            IS NOT TRUE
), cancels AS (
    SELECT c.position_id, c.intent_id, c.cancels_order_id, c.venue_outcome, c.cancel_class
    FROM legs JOIN public.order_intents c USING (position_id)
    WHERE (c.tenant_id, c.venue_id, c.kind) = ($1, $2, 'cancel')
), firsts AS (
    SELECT s.intent_id, jsonb_build_object('at', min(s.attempt_created_at), 'verdict',
        CASE WHEN bool_and(s.clock_verdict IS NOT DISTINCT FROM 'valid') THEN 'valid'
            WHEN bool_or(s.clock_verdict = 'invalid') THEN 'invalid' END) AS first
    FROM public.spot_intent_clock s
    WHERE (s.tenant_id, s.venue_id) = ($1, $2) AND s.intent_id IN (SELECT intent_id FROM cancels
        UNION ALL SELECT intent_id FROM legs JOIN public.venue_orders USING (position_id))
    GROUP BY s.intent_id
), marked AS (
    SELECT o.position_id, o.exchange_order_id, x.sold,
        jsonb_build_object('exchange_order_id', o.exchange_order_id, 'kind', o.kind, 'status', o.status,
            'phase', initcap(o.phase), 'orig_qty', o.orig_qty::TEXT, 'stop_price', o.stop_price::TEXT,
            'live_seen', jsonb_build_object('at', o.live_seen_at, 'verdict', o.live_clock_verdict),
            'terminal_seen', CASE WHEN o.terminal_seen_at IS NOT NULL THEN
                jsonb_build_object('at', o.terminal_seen_at, 'verdict', o.terminal_clock_verdict) END,
            'first_attempt', f.first, 'fills', x.fills) AS body,
        (o.kind <> 'entry' AND o.terminal_clock_verdict = 'valid' AND o.terminal_seen_at <= $3 - INTERVAL '2 s'
            AND x.after_b IS NOT TRUE) IS TRUE AS summarized
    FROM legs JOIN public.venue_orders o USING (position_id) LEFT JOIN firsts f USING (intent_id)
    CROSS JOIN LATERAL (
        SELECT coalesce(jsonb_agg(jsonb_build_object('trade_time', t.trade_time, 'qty', t.qty::TEXT,
                'price', t.price::TEXT, 'commission', t.commission::TEXT) ORDER BY t.trade_time, t.trade_id), '[]')
                AS fills,
            coalesce(sum(t.qty) FILTER (WHERE t.trade_time < $3), 0) AS sold, bool_or(t.trade_time >= $3) AS after_b
        FROM public.order_fills t
        WHERE (t.venue_id, t.account_scope, t.symbol, t.exchange_order_id)
            = (o.venue_id, o.account_scope, o.symbol, o.exchange_order_id)) x
)
SELECT l.leg || jsonb_build_object('orders', m.orders, 'cancels', k.cancels, 'summarized_sold_before_b',
    CASE WHEN public.ledger_amount_fits(m.sold) THEN m.sold::TEXT END)
FROM legs l
CROSS JOIN LATERAL (
    SELECT coalesce(sum(o.sold) FILTER (WHERE o.summarized), 0) AS sold,
        coalesce(jsonb_agg(o.body ORDER BY o.exchange_order_id::NUMERIC) FILTER (WHERE NOT o.summarized), '[]')
            AS orders
    FROM marked o WHERE o.position_id = l.position_id) m
CROSS JOIN LATERAL (
    SELECT coalesce(jsonb_agg(jsonb_build_object('position_id', c.position_id, 'cancels_order_id',
            c.cancels_order_id, 'venue_outcome', c.venue_outcome, 'cancel_class', c.cancel_class,
            'first_attempt', f.first) ORDER BY c.intent_id), '[]') AS cancels
    FROM cancels c LEFT JOIN firsts f USING (intent_id) WHERE c.position_id = l.position_id) k
ORDER BY l.position_id
