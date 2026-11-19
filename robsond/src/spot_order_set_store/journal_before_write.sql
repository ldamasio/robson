-- Both this read and the ordinary upsert use the same command snapshot.
-- Match the activity rule in journal_unbound.sql before changing the source.
WITH prior_member AS MATERIALIZED (
    SELECT j.intent_id
    FROM public.order_intents j
    WHERE (j.intent_id, j.tenant_id, j.venue_id, j.account_scope, j.client_order_id)
        = ($1, $2, $3, $4, $5)
      AND j.kind IN ('protection', 'exit')
      AND NOT (
          (j.not_sent AND j.venue_outcome IS NULL AND j.venue_evidence IS NULL)
          OR (
              j.venue_outcome IS NOT DISTINCT FROM 'rejected'
              AND NOT j.not_sent AND j.venue_evidence IS NULL
              AND EXISTS (
                  SELECT 1 FROM public.submit_attempts s
                  WHERE s.venue_id = 'binance:spot'
                    AND (s.account_scope, s.client_order_id) = (j.account_scope, j.client_order_id)
                    AND s.state = 'rejected' AND s.rejected_at IS NOT NULL
              )
          )
      )
)
