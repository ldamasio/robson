-- Both this read and the ordinary upsert use the same command snapshot.
-- Match the admission rule in journal_entry.sql before changing the source.
-- Exact keys only: the intent PK, the leg PK, the admission key and one
-- reverse probe on idx_venue_orders_intent.
WITH prior_entry AS MATERIALIZED (
    SELECT EXISTS (SELECT 1 FROM public.order_intents j WHERE j.intent_id = $1) AS prior_found,
           EXISTS (SELECT 1 FROM public.balance_legs l WHERE l.position_id = $6) AS prior_leg,
           (SELECT l.admission_intent_id FROM public.balance_legs l
            WHERE l.position_id = $6
              AND (l.tenant_id, l.venue_id, l.account_scope) = ($2, $3, $4)) AS prior_admission,
           EXISTS (
               SELECT 1 FROM public.balance_leg_admissions a WHERE a.entry_intent_id = $1
           ) AS prior_admitted,
           EXISTS (SELECT 1 FROM public.venue_orders o WHERE o.intent_id = $1) AS prior_native
)
