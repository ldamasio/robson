-- 8k-3a non-trading scope (PLAN-r2 §6.1): the pre ($6 NULL) and post
-- ($6 = v0) statement of a process with no order-set generation. One
-- autocommit statement on a fresh connection; nothing is held across the
-- inventory requests. $1 tenant, $2 venue, $3 account scope, $4 this
-- process's epoch, $5 the bound venue UID, $6 the expected version. No row:
-- no accounting row, or the version moved since pre. The flags: the scope is
-- bound to UID $5; $4 is the tenant's latest epoch and not clean-stopped;
-- the scope's Spot ledger holds no leg, venue order or intent. Every write to
-- those tables bumps the version, so an equal version at post proves the
-- ledger stayed empty across the inventory requests.
SELECT a.version,
       EXISTS (SELECT 1 FROM public.venue_accounts va
               WHERE (va.venue_id,va.account_scope,va.tenant_id)=($2,$3,$1) AND va.uid=$5) AS uid_bound,
       coalesce((SELECT e.epoch_id=$4 AND e.clean_stop_at IS NULL FROM public.process_epochs e
                 WHERE e.tenant_id=$1 ORDER BY e.epoch_seq DESC LIMIT 1), false) AS epoch_live,
       NOT EXISTS (SELECT 1 FROM public.balance_legs l WHERE (l.tenant_id,l.venue_id,l.account_scope)=($1,$2,$3))
       AND NOT EXISTS (SELECT 1 FROM public.venue_orders o WHERE (o.venue_id,o.account_scope,o.tenant_id)=($2,$3,$1))
       AND NOT EXISTS (SELECT 1 FROM public.order_intents j WHERE (j.venue_id,j.account_scope,j.tenant_id)=($2,$3,$1))
           AS ledger_empty
FROM public.accounting_state a
WHERE (a.tenant_id,a.venue_id)=($1,$2) AND ($6::bigint IS NULL OR a.version=$6)
