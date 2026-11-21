-- ADR-0056 step 8l-3: E(t) with its anchor, and the cash validity predicates of one balance-account venue (PLAN-r4
-- §6.4), read by 8l-8, 8p and 8n inside their own transactions. One read-only statement; no row lock. $1 tenant, $2
-- venue, $3 t (B for 8p), $4 now, $5..$7 the expected commission assets.
-- - impaired_now: E(now) is undefined, so the current month is invalid;
-- - open_missing (V5's clause): an open missing discrepancy with a clean sighting below -0.01;
-- - missing_before (V6): such a discrepancy whose stored checkpoint cut is before t;
-- - withdrawal_fence: an attested withdrawal attributed to now's UTC month.
SELECT e.anchor_id, e.anchor_as_of, e.impaired, e.expected, n.impaired AS impaired_now,
       coalesce(d.direction = 'missing' AND d.clean_min < -0.01, false) AS open_missing,
       coalesce(d.direction = 'missing' AND d.clean_min < -0.01 AND d.checkpoint_cut < $3, false) AS missing_before,
       d.clean_min AS effective_minimum,
       EXISTS (SELECT 1 FROM public.cash_attested_flows w
               WHERE (w.tenant_id, w.venue_id) = ($1, $2) AND w.direction = 'withdrawal'
                 AND w.attributed_month = date_trunc('month', $4 AT TIME ZONE 'UTC')::date) AS withdrawal_fence
FROM ({expected_t}) e
CROSS JOIN ({expected_now}) n
LEFT JOIN ({discrepancy}) d ON true
