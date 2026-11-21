-- Both this read and the ordinary upsert use the same command snapshot.
-- Match the member rule in journal_cancel_outcome.sql before changing the source.
WITH prior_cancel AS MATERIALIZED (
    SELECT j.intent_id, c.conflict, NOT j.not_sent OR c.conflict AS pending
    FROM public.order_intents j
    CROSS JOIN LATERAL (
        SELECT EXISTS (SELECT 1 FROM public.venue_orders o WHERE o.intent_id = j.intent_id)
            OR (j.not_sent AND (j.venue_outcome IS NOT NULL OR j.venue_evidence IS NOT NULL))
            AS conflict
    ) c
    WHERE (j.intent_id, j.tenant_id, j.venue_id, j.account_scope, j.client_order_id)
        = ($1, $2, $3, $4, $5)
      AND j.kind = 'cancel' AND j.position_id = $6 AND j.cancels_order_id = $8
)
