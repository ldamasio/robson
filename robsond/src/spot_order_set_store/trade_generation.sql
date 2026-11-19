-- The caller already owns the tenant/venue accounting row and has applied
-- the original trade-page fills, order totals, cursor and leg consumption in
-- this same transaction. Trade pages touch no SELL/CANCEL membership input,
-- so only the exact generation version advances. No row is a refusal.
UPDATE public.spot_order_set_generation g
SET accounting_version = $6
WHERE (g.tenant_id, g.venue_id, g.account_scope) = ($1, 'binance:spot', $2)
  AND (g.generation_id, g.process_epoch, g.source_audit_id) = ($3, $4, $5)
  AND g.accounting_version = $6::bigint - 1
  AND EXISTS (
      SELECT 1 FROM (
          SELECT epoch_id, clean_stop_at FROM public.process_epochs
          WHERE tenant_id = g.tenant_id ORDER BY epoch_seq DESC LIMIT 1
      ) epoch
      WHERE epoch.epoch_id = g.process_epoch AND epoch.clean_stop_at IS NULL
  )
RETURNING g.accounting_version
