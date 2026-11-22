-- ADR-0056 step 8l-5b: the re-anchor's one context read (PLAN-r4 §6.12), after the bump that took the accounting rows,
-- with no lock of its own. $1..$7 as the settlement barrier (tenant, venue, O2's month, now, assets); $8 O1's cut, $9 O2's
-- cut, $10 O2's interval end, $11 the observations' process epoch.
-- - at rest: no unsettled leg (the barrier), no open transition, no unknown order and no live order but a stop, no
--   uncertain intent (stamped, no outcome, not proven unsent, no disposition);
-- - E_old at O1's and O2's cuts, the anchor it rests on and whether that anchor's own hull is dirty (kind (A));
-- - the open discrepancy with its clean evidence, its dirty sightings and its checkpoint copy (NULL when dirty);
-- - the current checkpoint when its interval is clean and its cut is not after O1's; the latest observed anchor before
--   O1's cut whose hull is clean (a recovery's old anchor, else the earliest observed anchor);
-- - hull_dirty: a booked fill of the venue lies in [O1's cut, O2's end], the write-time guard;
-- - epoch_current: the observations' epoch is the tenant's live one.
SELECT b.unsettled, b.venue_known,
       NOT EXISTS (SELECT 1 FROM public.balance_transitions x
                   WHERE (x.tenant_id, x.venue_id) = ($1, $2) AND x.completed_at IS NULL)
       AND NOT EXISTS (SELECT 1 FROM public.venue_orders o
                       WHERE (o.venue_id, o.tenant_id) = ($2, $1)
                         AND (o.phase = 'unknown' OR (o.phase = 'live' AND o.kind IS DISTINCT FROM 'stop')))
       AND NOT EXISTS (SELECT 1 FROM public.order_intents i
                       WHERE (i.tenant_id, i.venue_id) = ($1, $2) AND i.uncertain_generation IS NOT NULL
                         AND i.venue_outcome IS NULL AND NOT i.not_sent AND i.disposition_kind IS NULL) AS at_rest,
       e1.anchor_id, e1.anchor_as_of, e1.anchor_dirty, e2.anchor_id AS anchor_o2,
       e1.impaired OR e2.impaired AS impaired, e1.expected AS expected_o1, e2.expected AS expected_o2,
       d.discrepancy_id, d.direction = 'missing' AS missing, d.clean_min, d.clean_max, d.dirty_seqs,
       c.cut AS copy_cut, c.lo AS copy_start, c.hi AS copy_end,
       k.cut AS checkpoint_cut, k.lo AS checkpoint_start, k.hi AS checkpoint_end,
       h.as_of AS clean_as_of, h.lo AS clean_start, h.hi AS clean_end,
       coalesce(h.anchor_id, (SELECT a.anchor_id FROM public.cash_anchors a
                              WHERE (a.tenant_id, a.venue_id) = ($1, $2) AND a.as_of IS NOT NULL
                              ORDER BY a.as_of LIMIT 1)) AS recovery_anchor,
       EXISTS (SELECT 1 FROM public.order_fills f
               WHERE f.venue_id = $2 AND f.tenant_id = $1 AND f.trade_time BETWEEN $8 AND $10) AS hull_dirty,
       EXISTS (SELECT 1 FROM (SELECT p.epoch_id, p.clean_stop_at FROM public.process_epochs p
                              WHERE p.tenant_id = $1 ORDER BY p.epoch_seq DESC LIMIT 1) p
               WHERE p.epoch_id = $11 AND p.clean_stop_at IS NULL) AS epoch_current
FROM ({barrier}) b
CROSS JOIN ({expected_o1}) e1
CROSS JOIN ({expected_o2}) e2
LEFT JOIN ({discrepancy}) d ON true
LEFT JOIN LATERAL (
    SELECT d.checkpoint_cut AS cut, d.checkpoint_interval_start AS lo, d.checkpoint_interval_end AS hi
    WHERE NOT EXISTS (SELECT 1 FROM public.order_fills f
                      WHERE f.venue_id = $2 AND f.tenant_id = $1
                        AND f.trade_time BETWEEN d.checkpoint_interval_start AND d.checkpoint_interval_end)
) c ON true
LEFT JOIN LATERAL (
    SELECT c.cut, c.interval_start AS lo, c.interval_end AS hi
    FROM public.cash_checkpoints c
    WHERE (c.tenant_id, c.venue_id) = ($1, $2) AND c.cut <= $8
      AND NOT EXISTS (SELECT 1 FROM public.order_fills f
                      WHERE f.venue_id = $2 AND f.tenant_id = $1
                        AND f.trade_time BETWEEN c.interval_start AND c.interval_end)
) k ON true
LEFT JOIN LATERAL (
    SELECT a.anchor_id, a.as_of, a.depends_start AS lo, a.depends_end AS hi
    FROM public.cash_anchors a
    WHERE (a.tenant_id, a.venue_id) = ($1, $2) AND a.as_of < $8
      AND NOT EXISTS (SELECT 1 FROM public.order_fills f
                      WHERE f.venue_id = $2 AND f.tenant_id = $1
                        AND f.trade_time BETWEEN a.depends_start AND a.depends_end)
    ORDER BY a.as_of DESC LIMIT 1
) h ON true
