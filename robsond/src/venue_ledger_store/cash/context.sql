-- ADR-0056 step 8l-3: the cash writer's one context read (PLAN-r4 §6.4 step 3), under the locked accounting row and
-- with no lock of its own: 8l-2's settlement barrier ($1..$7: tenant, venue, the cut's month, now, assets), E at the
-- observation's cut $8, the open discrepancy with its surviving evidence, and the writer's own reads ($9 the
-- observation's interval end, $10 its process epoch):
-- - at_rest: no open transition;
-- - observation_dirty: a booked fill of the venue lies in [cut, end], the write-time guard;
-- - epoch_current: the observation's epoch is the tenant's live one;
-- - the copy a new discrepancy takes: the current checkpoint when its interval is clean and its cut is not after
--   this one, otherwise the latest observed anchor whose hull is clean, its as_of and hull (earlier: conservative).
SELECT b.unsettled, b.venue_known, e.expected,
       d.discrepancy_id, d.direction, d.last_seq, d.clean_min, d.clean_max,
       NOT EXISTS (SELECT 1 FROM public.balance_transitions x
                   WHERE (x.tenant_id, x.venue_id) = ($1, $2) AND x.completed_at IS NULL) AS at_rest,
       EXISTS (SELECT 1 FROM public.order_fills f
               WHERE f.venue_id = $2 AND f.tenant_id = $1 AND f.trade_time BETWEEN $8 AND $9) AS observation_dirty,
       EXISTS (SELECT 1 FROM (SELECT p.epoch_id, p.clean_stop_at FROM public.process_epochs p
                              WHERE p.tenant_id = $1 ORDER BY p.epoch_seq DESC LIMIT 1) p
               WHERE p.epoch_id = $10 AND p.clean_stop_at IS NULL) AS epoch_current,
       coalesce(k.cut, h.as_of) AS copy_cut, coalesce(k.lo, h.lo) AS copy_start, coalesce(k.hi, h.hi) AS copy_end
FROM ({barrier}) b
CROSS JOIN ({expected}) e
LEFT JOIN ({discrepancy}) d ON true
LEFT JOIN LATERAL (
    SELECT c.cut, c.interval_start AS lo, c.interval_end AS hi
    FROM public.cash_checkpoints c
    WHERE (c.tenant_id, c.venue_id) = ($1, $2) AND c.cut <= $8
      AND NOT EXISTS (SELECT 1 FROM public.order_fills f
                      WHERE f.venue_id = $2 AND f.tenant_id = $1
                        AND f.trade_time BETWEEN c.interval_start AND c.interval_end)
) k ON true
LEFT JOIN LATERAL (
    SELECT a.as_of, a.depends_start AS lo, a.depends_end AS hi
    FROM public.cash_anchors a
    WHERE (a.tenant_id, a.venue_id) = ($1, $2) AND a.as_of <= $8
      AND NOT EXISTS (SELECT 1 FROM public.order_fills f
                      WHERE f.venue_id = $2 AND f.tenant_id = $1
                        AND f.trade_time BETWEEN a.depends_start AND a.depends_end)
    ORDER BY a.as_of DESC LIMIT 1
) h ON true
