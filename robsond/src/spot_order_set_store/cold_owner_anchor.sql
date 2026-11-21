-- The owner's first data read inside the cold audit snapshot: the scoped
-- Spot account with its accounting version, its tenant's latest process
-- epoch and the account's persisted generation tuple, or none of them.
-- $1 account_scope. At most one row; every join is a native key or the
-- latest-epoch index. The UID is cut before decode; `uid_wide` refuses.
SELECT va.tenant_id,
       octet_length(va.uid)>256 AS uid_wide,
       left(va.uid,256) AS uid,
       s.version AS accounting_version,
       e.epoch_id AS latest_epoch,
       e.clean_stop_at IS NULL AS latest_epoch_live,
       g.generation_id AS prior_generation_id,
       g.process_epoch AS prior_process_epoch,
       g.source_audit_id AS prior_source_audit_id,
       g.accounting_version AS prior_accounting_version,
       g.position_hints_complete AS prior_position_hints_complete
  FROM public.venue_accounts va
  LEFT JOIN public.accounting_state s
    ON (s.tenant_id,s.venue_id)=(va.tenant_id,va.venue_id)
  LEFT JOIN LATERAL (
       SELECT epoch_id,clean_stop_at FROM public.process_epochs
        WHERE tenant_id=va.tenant_id ORDER BY epoch_seq DESC LIMIT 1) e ON true
  LEFT JOIN public.spot_order_set_generation g
    ON (g.tenant_id,g.venue_id,g.account_scope)=(va.tenant_id,va.venue_id,va.account_scope)
 WHERE (va.venue_id,va.account_scope)=('binance:spot',$1)
