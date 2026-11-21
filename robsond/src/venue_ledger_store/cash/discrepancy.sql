-- ADR-0056 step 8l-3: the open discrepancy of venue $2 of tenant $1 and its surviving evidence (PLAN-r4 §6.4, §6.10),
-- embedded as a sub-select. A sighting whose interval contains the venue time of a booked fill of the venue is dirty:
-- a voided observation, excluded from the evidence while the clean ones, the checkpoint copy and the fence stand. The
-- effective minimum and maximum are over the clean sightings only, NULL when none survives.
SELECT d.discrepancy_id, d.direction, d.checkpoint_cut, max(s.seq) AS last_seq,
       min(s.delta) FILTER (WHERE NOT s.dirty) AS clean_min,
       max(s.delta) FILTER (WHERE NOT s.dirty) AS clean_max
FROM public.cash_discrepancies d
LEFT JOIN LATERAL (
    SELECT s.seq, s.delta,
           EXISTS (SELECT 1 FROM public.order_fills f
                   WHERE f.venue_id = $2 AND f.tenant_id = $1
                     AND f.trade_time BETWEEN s.interval_start AND s.interval_end) AS dirty
    FROM public.cash_sightings s
    WHERE s.discrepancy_id = d.discrepancy_id
) s ON true
WHERE (d.tenant_id, d.venue_id) = ($1, $2) AND d.state = 'open'
GROUP BY d.discrepancy_id, d.direction, d.checkpoint_cut
