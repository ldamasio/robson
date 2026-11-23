-- Test fixture only (5a composition, migration 56): a pending cutover run $3 of tenant $2 and venue $1, for a test
-- that writes a cutover anchor or an activation basis naming $3 (cash_anchors_venue_cutover,
-- activation_bases_venue_cutover). Single flight, as 8m's runs: the venue's pending run, if any, is abandoned first
-- and keeps the rows that name it. One statement: the scalar subquery reads the UPDATE out before the insert.
WITH ended AS (
    UPDATE public.venue_cutovers SET state = 'abandoned', abandoned_at = now(), abandon_reason = 'operator'
    WHERE tenant_id = $2::UUID AND venue_id = $1::TEXT AND state = 'pending'
    RETURNING cutover_id)
INSERT INTO public.venue_cutovers (cutover_id, tenant_id, venue_id, account_scope, state, month, uid, report,
    report_hash, generation, generation_hash, v1, budget_model, previous_basis, quote_total, basis, command_id,
    epoch_id, created_at)
SELECT $3::UUID, $2::UUID, $1::TEXT, 'fixture', 'pending', DATE '2026-12-01', 'fixture', '{}', repeat('0', 64),
    '{"schema_version": 1}', repeat('0', 64), 0, 'net_from_start_non_expanding_v1', 0, 0, 0, gen_random_uuid(),
    gen_random_uuid(), now()
WHERE (SELECT count(*) FROM ended) >= 0
