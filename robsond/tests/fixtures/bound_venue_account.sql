-- Test fixture only (ADR-0056 step 8m-1): a venue account bound to a cutover run of its tenant and venue, as
-- migration 56's foreign key requires. One statement, like the INSERT each test used before: it creates a
-- pending run of ($3, $1) when that venue has none and reuses it otherwise (a second scope of the same venue),
-- then inserts the account. $1 venue, $2 account scope, $3 tenant, $4 uid.
WITH created AS (
    INSERT INTO public.venue_cutovers (cutover_id, tenant_id, venue_id, account_scope, state, month, uid, report,
        report_hash, generation, generation_hash, v1, budget_model, previous_basis, quote_total, basis, command_id,
        epoch_id, created_at)
    VALUES (gen_random_uuid(), $3, $1, $2, 'pending', DATE '2026-12-01', $4, '{}', repeat('0', 64),
        '{"schema_version": 1}', repeat('0', 64), 0, 'net_from_start_non_expanding_v1', 0, 0, 0, gen_random_uuid(),
        gen_random_uuid(), now())
    ON CONFLICT (tenant_id, venue_id) WHERE state = 'pending' DO NOTHING
    RETURNING cutover_id),
run AS (
    SELECT cutover_id FROM created
    UNION ALL
    SELECT cutover_id FROM public.venue_cutovers WHERE tenant_id = $3 AND venue_id = $1 AND state = 'pending')
INSERT INTO public.venue_accounts (venue_id, account_scope, tenant_id, uid, bound_at, cutover_id)
SELECT $1, $2, $3, $4, now(), cutover_id FROM run
