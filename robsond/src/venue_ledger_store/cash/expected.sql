-- ADR-0056 step 8l-3: E(t), the expected quote of one balance-account venue (v16 §C1; PLAN-r4 §6.4, §6.10), the
-- anchor it rests on and whether t is impaired. One read-only SELECT, embedded as a sub-select; no row lock. $1
-- tenant, $2 venue, $5..$7 per symbol the commission asset a buy ($6, base) and a sell ($7, quote) must carry; the
-- caller substitutes its own parameter for the time asked about, written T below and {t} in the text.
--
-- A is the latest observed anchor with as_of < T; an anchor whose as_of is not set never counts, and no derived
-- boundary anchor is read. E(T) = A.quote + the quote flow of every leg-owned fill with A.as_of <= trade_time < T (a
-- sell q*p - fee, a buy -q*p) + every attested flow dated in [A.as_of, T) (a withdrawal negative). A counted fill
-- whose commission asset is not the expected one gives no value, and the sum is decoded only when it fits.
--
-- Late evidence, from immutable rows only: an interval is dirty when it contains the venue time of a booked fill of
-- the venue (any owner). T is impaired, and E(T) has no value, when
-- (A) A's own hull [O1 start, O2 end] is dirty. This is exactly T in (D.as_of, D'.as_of] for some dirty-hull anchor D,
--     D' the next anchor whose hull is clean (none: unbounded): the anchors between D and D' are dirty too; or
-- (R) a re-anchor R has O.as_of < T <= A_R.as_of, O the anchor it replaced and A_R its own, and its dating copy, its
--     anchor's hull or a sighting it used (its resolved discrepancy's, less excluded_sighting_seqs) is dirty.
-- Both spans are right-closed, as E selects: the anchor at as_of = T is not yet used, and R's flows (dated at or
-- after O.as_of) still count at A_R.as_of.
WITH anchor AS (
    SELECT a.anchor_id, a.as_of, a.quote,
           EXISTS (SELECT 1 FROM public.order_fills f
                   WHERE f.venue_id = $2 AND f.tenant_id = $1
                     AND f.trade_time BETWEEN a.depends_start AND a.depends_end) AS impaired
    FROM public.cash_anchors a
    WHERE (a.tenant_id, a.venue_id) = ($1, $2) AND a.as_of < {t}
    ORDER BY a.as_of DESC LIMIT 1
), relied AS (
    SELECT EXISTS (
        SELECT 1
        FROM public.cash_reanchors r
        JOIN public.cash_anchors o ON o.anchor_id = r.old_anchor_id
        JOIN public.cash_anchors n ON n.anchor_id = r.new_anchor_id
        CROSS JOIN LATERAL (
            SELECT r.dating_interval_start AS lo, r.dating_interval_end AS hi
            UNION ALL
            SELECT n.depends_start, n.depends_end
            UNION ALL
            SELECT s.interval_start, s.interval_end FROM public.cash_sightings s
            WHERE s.discrepancy_id = r.resolved_discrepancy_id AND s.seq <> ALL (r.excluded_sighting_seqs)
        ) dependency
        WHERE (r.tenant_id, r.venue_id) = ($1, $2)
          AND coalesce(o.as_of, '-infinity') < {t} AND {t} <= n.as_of
          AND EXISTS (SELECT 1 FROM public.order_fills f
                      WHERE f.venue_id = $2 AND f.tenant_id = $1
                        AND f.trade_time BETWEEN dependency.lo AND dependency.hi)
    ) AS impaired
), flows AS (
    SELECT count(*) FILTER (WHERE x.amount IS NULL) AS odd, coalesce(sum(x.amount), 0) AS total
    FROM anchor a
    CROSS JOIN LATERAL (
        SELECT CASE WHEN o.side = 'SELL' AND f.commission_asset = e.quote THEN f.qty * f.price - f.commission
                    WHEN o.side = 'BUY' AND f.commission_asset = e.base THEN -(f.qty * f.price) END AS amount
        FROM public.order_fills f
        JOIN public.venue_orders o
          ON (o.venue_id, o.account_scope, o.symbol, o.exchange_order_id, o.tenant_id)
             = (f.venue_id, f.account_scope, f.symbol, f.exchange_order_id, f.tenant_id)
        LEFT JOIN unnest($5::text[], $6::text[], $7::text[]) AS e(symbol, base, quote) ON e.symbol = f.symbol
        WHERE f.venue_id = $2 AND f.tenant_id = $1 AND o.owner = 'leg'
          AND f.trade_time >= a.as_of AND f.trade_time < {t}
        UNION ALL
        SELECT CASE w.direction WHEN 'withdrawal' THEN -w.amount ELSE w.amount END
        FROM public.cash_attested_flows w
        WHERE (w.tenant_id, w.venue_id) = ($1, $2) AND w.economic_date >= a.as_of AND w.economic_date < {t}
    ) x
)
SELECT a.anchor_id, a.as_of AS anchor_as_of, coalesce(a.impaired, false) OR r.impaired AS impaired,
       CASE WHEN NOT (coalesce(a.impaired, false) OR r.impaired) AND f.odd = 0
                 AND ledger_amount_fits(a.quote + f.total)
            THEN a.quote + f.total END AS expected
FROM relied r CROSS JOIN flows f LEFT JOIN anchor a ON true
