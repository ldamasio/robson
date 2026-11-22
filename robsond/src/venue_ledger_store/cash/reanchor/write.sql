-- ADR-0056 step 8l-5b: the re-anchor's one write (PLAN-r4 §6.12), data-modifying CTEs after the row lock and the context
-- read. Every row it inserts is immutable; the discrepancy's state and the checkpoint pointer move forward only.
-- - the new anchor: O2's total at O2's cut, O2's observation, the hull [O1's cut, O2's end];
-- - the re-anchor row: identity, reason, O1 and O2 with their totals, the version checked, the old and new anchors and,
--   for an attested one, its effective dating copy and amounts (a recovery one has none);
-- - the flows, when $37 asks: a withdrawal W > 0 dated $36 and a contribution C > 0 dated at O1's end, each in the UTC
--   month of its date, its id derived from the re-anchor's and its direction;
-- - the checkpoint pointer, moved forward to O2's cut (source reanchor, the new anchor);
-- - the open discrepancy resolved ($33) or voided by late evidence ($35).
-- It returns how many discrepancies it closed and how many flows it wrote.
--
-- $1 tenant, $2 venue. The anchor: $3 id, $4 quote (O2's total), $5 O2's cut, $6 send, $7 receive, $8 u, $9 sample,
-- $10 epoch, $11 end, $12 O1's cut. The re-anchor: $13 id, $14 operator subject, $15 reason, O1's $16 send, $17
-- receive, $18 u, $19 sample, $20 epoch, $21 end, $22 total, $23 mode, $24 the version checked, $25 the old anchor,
-- $26..$28 the dating copy (cut, start, end), $29 delta, $30 min delta, $31 withdrawal, $32 contribution, $33 the
-- resolved discrepancy, $34 the excluded sighting seqs, $35 the voided discrepancy. $36 the withdrawal's date, $37
-- whether flows are written.
WITH anchor AS (
    INSERT INTO public.cash_anchors (anchor_id, tenant_id, venue_id, kind, quote, as_of, send_at, receive_at, u_ns,
                                     sample_id, epoch_id, interval_start, interval_end, depends_start, depends_end)
    VALUES ($3, $1, $2, 'reanchor', $4, $5, $6, $7, $8, $9, $10, $5, $11, $12, $11)
    RETURNING anchor_id
), reanchor AS (
    INSERT INTO public.cash_reanchors (reanchor_id, tenant_id, venue_id, operator_subject, reason,
        o1_send_at, o1_receive_at, o1_u_ns, o1_sample_id, o1_epoch_id, o1_interval_start, o1_interval_end, o1_total,
        o2_send_at, o2_receive_at, o2_u_ns, o2_sample_id, o2_epoch_id, o2_interval_start, o2_interval_end, o2_total,
        mode, checked_version, old_anchor_id, new_anchor_id, dating_cut, dating_interval_start, dating_interval_end,
        delta, min_delta, withdrawal, contribution, resolved_discrepancy_id, excluded_sighting_seqs,
        voided_discrepancy_id)
    SELECT $13, $1, $2, $14, $15, $16, $17, $18, $19, $20, $12, $21, $22, $6, $7, $8, $9, $10, $5, $11, $4,
           $23, $24, $25, a.anchor_id, $26, $27, $28, $29, $30, $31, $32, $33, $34, $35
    FROM anchor a
    RETURNING reanchor_id, mode
), flows AS (
    INSERT INTO public.cash_attested_flows (tenant_id, venue_id, economic_date, flow_id, reanchor_id, direction,
                                            amount, date_rule, attributed_month, reanchor_mode)
    SELECT $1, $2, f.dated, md5(r.reanchor_id::text || f.direction)::uuid, r.reanchor_id, f.direction, f.amount,
           f.rule, date_trunc('month', f.dated AT TIME ZONE 'UTC')::date, r.mode
    FROM reanchor r
    CROSS JOIN (VALUES ('withdrawal', $31::numeric, $36::timestamptz, 'checkpoint_cut'),
                       ('contribution', $32::numeric, $21::timestamptz, 'o1_receipt_plus_u')) f (direction, amount, dated, rule)
    WHERE $37 AND f.amount > 0
    RETURNING direction
), pointer AS (
    INSERT INTO public.cash_checkpoints AS k (tenant_id, venue_id, cut, send_at, receive_at, u_ns, sample_id, epoch_id,
                                              interval_start, interval_end, source, anchor_id)
    SELECT $1, $2, $5, $6, $7, $8, $9, $10, $5, $11, 'reanchor', a.anchor_id FROM anchor a
    ON CONFLICT (tenant_id, venue_id) DO UPDATE
        SET (cut, send_at, receive_at, u_ns, sample_id, epoch_id, interval_start, interval_end, source, anchor_id)
            = (EXCLUDED.cut, EXCLUDED.send_at, EXCLUDED.receive_at, EXCLUDED.u_ns, EXCLUDED.sample_id,
               EXCLUDED.epoch_id, EXCLUDED.interval_start, EXCLUDED.interval_end, 'reanchor', EXCLUDED.anchor_id)
        WHERE k.cut < EXCLUDED.cut
), closed AS (
    UPDATE public.cash_discrepancies
    SET state = CASE WHEN discrepancy_id = $33 THEN 'resolved' ELSE 'void' END,
        void_reason = CASE WHEN discrepancy_id = $35 THEN 'late_evidence' END, closed_at = now()
    WHERE discrepancy_id IN ($33, $35) AND (tenant_id, venue_id) = ($1, $2) AND state = 'open'
    RETURNING discrepancy_id
)
SELECT (SELECT count(*) FROM closed) AS closed, (SELECT count(*) FROM flows) AS flows
