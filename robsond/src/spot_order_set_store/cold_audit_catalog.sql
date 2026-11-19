-- Flag-only catalog facts the four cold source pages and their route coverage
-- rely on, read inside the audit snapshot. Read-only: the expected predicates
-- are PostgreSQL's rendering of the M36/M37 definitions and the unmigrated
-- route index, not a temp-table rendering. One row per label; any false or
-- missing label refuses. It cannot attest rows restored with checks bypassed.
WITH expected(label, rel, cols, uniq, pred) AS (VALUES
    ('intent_key', 'order_intents', ARRAY['venue_id','account_scope','client_order_id'], true, NULL::text),
    ('attempt_key', 'submit_attempts', ARRAY['venue_id','account_scope','client_order_id'], true,
        '(venue_id = ''binance:spot''::text)'),
    ('order_key', 'venue_orders', ARRAY['venue_id','account_scope','symbol','exchange_order_id'], true, NULL),
    ('route_key', 'leg_routes', ARRAY['account_scope','position_id'], false,
        '(venue_id = ''binance:spot''::text)'),
    ('leg_key', 'balance_legs', ARRAY['position_id'], true, NULL),
    ('admission_current', 'balance_leg_admissions', ARRAY['position_id'], true, '(superseded_at IS NULL)'),
    ('account_key', 'venue_accounts', ARRAY['venue_id','account_scope'], true, NULL))
SELECT e.label, EXISTS (
        SELECT 1 FROM pg_index i
          JOIN pg_class ix ON ix.oid = i.indexrelid
          JOIN pg_am am ON am.oid = ix.relam
         WHERE i.indrelid = to_regclass('public.' || e.rel) AND am.amname = 'btree'
           AND i.indisvalid AND i.indisready AND i.indislive AND (i.indimmediate OR NOT e.uniq)
           AND i.indisunique = e.uniq AND i.indexprs IS NULL
           AND i.indnatts = cardinality(e.cols) AND i.indnkeyatts = cardinality(e.cols)
           AND ARRAY(SELECT a.attname::text FROM unnest(i.indkey) WITH ORDINALITY AS k(attnum, n)
                     JOIN pg_attribute a ON a.attrelid = i.indrelid AND a.attnum = k.attnum ORDER BY k.n)
               = e.cols
           AND pg_get_expr(i.indpred, i.indrelid, false) IS NOT DISTINCT FROM e.pred
           AND (e.label <> 'route_key' OR ix.relname = 'idx_leg_routes_spot_scope_position')) AS ok
  FROM expected e
UNION ALL
SELECT 'uid_nonblank', EXISTS (
        SELECT 1 FROM pg_constraint c
         WHERE c.conrelid = to_regclass('public.venue_accounts') AND c.contype = 'c' AND c.convalidated
           AND pg_get_constraintdef(c.oid) = 'CHECK ((uid <> ''''::text))')
UNION ALL
SELECT r.rel || '_accounting_fk', EXISTS (
        SELECT 1 FROM pg_constraint c
         WHERE c.conrelid = to_regclass('public.' || r.rel) AND c.contype = 'f' AND c.convalidated
           AND c.confrelid = to_regclass('public.accounting_state'))
  FROM (VALUES ('venue_accounts'), ('leg_routes')) r(rel)
UNION ALL
SELECT t.name, EXISTS (
        SELECT 1 FROM pg_trigger g
         WHERE g.tgrelid = to_regclass('public.' || t.rel) AND g.tgname = t.name
           AND NOT g.tgisinternal AND g.tgenabled IN ('O', 'A'))
  FROM (VALUES ('venue_accounts', 'venue_accounts_are_immutable'),
               ('leg_routes', 'leg_routes_are_immutable')) t(rel, name)
UNION ALL
-- Origin-enabled triggers do not fire under replica replication role.
SELECT 'origin_replication_role', current_setting('session_replication_role') = 'origin'
UNION ALL
SELECT f.name, EXISTS (
       SELECT 1 FROM pg_constraint c
       WHERE c.conname = f.name AND c.contype = 'f' AND c.convalidated
         AND c.conrelid = to_regclass('public.' || f.source)
         AND c.confrelid = to_regclass('public.' || f.target)
         AND ARRAY(SELECT a.attname::text FROM unnest(c.conkey) WITH ORDINALITY AS k(attnum,n)
                   JOIN pg_attribute a ON a.attrelid=c.conrelid AND a.attnum=k.attnum ORDER BY k.n)
             = f.source_columns
         AND ARRAY(SELECT a.attname::text FROM unnest(c.confkey) WITH ORDINALITY AS k(attnum,n)
                   JOIN pg_attribute a ON a.attrelid=c.confrelid AND a.attnum=k.attnum ORDER BY k.n)
             = f.target_columns
         AND (SELECT count(*) = 4 AND bool_and(g.tgisinternal AND g.tgenabled IN ('O','A'))
              FROM pg_trigger g WHERE g.tgconstraint=c.oid))
FROM (VALUES
   ('balance_leg_admissions_route','balance_leg_admissions','leg_routes',
    ARRAY['position_id','tenant_id','venue_id','account_scope'],
    ARRAY['position_id','tenant_id','venue_id','account_scope']),
   ('balance_legs_admission','balance_legs','balance_leg_admissions',
    ARRAY['admission_intent_id','position_id','tenant_id','venue_id','account_scope','symbol','decided_month'],
    ARRAY['entry_intent_id','position_id','tenant_id','venue_id','account_scope','symbol','decided_month'])
) f(name,source,target,source_columns,target_columns)

