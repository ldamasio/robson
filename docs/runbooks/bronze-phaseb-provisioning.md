# Bronze Phase B provisioning runbook (gated)

Operator-executed, in an approved window, after migrations 000027/000028
are applied by the migrate job. Companion to `docs/data-contracts/
bronze-v1.md` §3.2/§4 and the bronze provisioning study (role + pg_hba via
reload; never `site.yml`, never a Postgres restart). Nothing here runs
automatically.

Every psql below runs as `postgres` on the database host, database
`robson`, one statement per invocation (autocommit), with
`-v ON_ERROR_STOP=1`.

## 1. Child cursor indexes (CONCURRENTLY) + attach

Migration 000027 created `idx_event_log_bronze_cursor` ON ONLY: the parent
index exists but is `indisvalid = false` until every existing partition has
an attached child index. Partitions created AFTER 000027 by
`create_event_log_partitions` get their child index automatically; only
partitions that existed before need this procedure.

```sql
-- 1a. Enumerate partitions with NO child index attached to the parent
-- cursor index (linkage via pg_inherits on indexes, never by name:
-- auto-created child indexes carry PostgreSQL-generated names).
SELECT t.oid::regclass AS partition
  FROM pg_inherits ti
  JOIN pg_class t ON t.oid = ti.inhrelid
 WHERE ti.inhparent = 'public.event_log'::regclass
   AND NOT EXISTS (
     SELECT 1
       FROM pg_inherits ii
       JOIN pg_index x ON x.indexrelid = ii.inhrelid
      WHERE ii.inhparent = 'public.idx_event_log_bronze_cursor'::regclass
        AND x.indrelid = t.oid);

-- 1a'. Valid child indexes on those partitions that were built but never
-- attached (interruption between build and attach): attach them directly
-- in 1c instead of rebuilding. Selected by COLUMN SIGNATURE (the cursor
-- key), never by name.
SELECT format('%I.%I', n.nspname, ic.relname) AS orphan_index,
       x.indrelid::regclass AS partition
  FROM pg_index x
  JOIN pg_class ic ON ic.oid = x.indexrelid
  JOIN pg_namespace n ON n.oid = ic.relnamespace
 WHERE x.indisvalid
   AND x.indrelid IN (SELECT ti.inhrelid FROM pg_inherits ti
                       WHERE ti.inhparent = 'public.event_log'::regclass)
   AND NOT EXISTS (SELECT 1 FROM pg_inherits ii
                    WHERE ii.inhrelid = x.indexrelid)
   AND NOT x.indisunique
   AND NOT x.indisexclusion
   AND pg_get_indexdef(x.indexrelid) ~ 'USING btree \(ingested_at, event_id\)$';
-- The rendered-definition tail proves full equivalence: btree method,
-- exactly these two key columns, default ASC ordering, default
-- opclass/collation, no expression columns and no partial predicate
-- (any deviation renders extra text inside or after the parentheses).
```

For EACH partition `P` from 1a, one at a time:

```sql
-- 1b. Build (autocommit; CONCURRENTLY never inside a transaction)
CREATE INDEX CONCURRENTLY P_bronze_cursor_idx
    ON public.P (ingested_at ASC, event_id ASC);
```

If 1b fails or is interrupted it leaves an INVALID index; drop and retry:

```sql
-- Scoped to the CURSOR-equivalent index only (same column signature) on
-- partitions of public.event_log; schema-qualified names. Unrelated
-- invalid indexes are never touched by this runbook.
SELECT format('%I.%I', n.nspname, ic.relname) AS invalid_index
  FROM pg_index x
  JOIN pg_class ic ON ic.oid = x.indexrelid
  JOIN pg_namespace n ON n.oid = ic.relnamespace
 WHERE NOT x.indisvalid
   AND x.indrelid IN (SELECT ti.inhrelid FROM pg_inherits ti
                       WHERE ti.inhparent = 'public.event_log'::regclass)
   AND NOT x.indisunique
   AND NOT x.indisexclusion
   AND pg_get_indexdef(x.indexrelid) ~ 'USING btree \(ingested_at, event_id\)$';
-- The rendered-definition tail proves full equivalence: btree method,
-- exactly these two key columns, default ASC ordering, default
-- opclass/collation, no expression columns and no partial predicate
-- (any deviation renders extra text inside or after the parentheses).
-- for each: DROP INDEX CONCURRENTLY <schema-qualified name>; then repeat 1b
```

```sql
-- 1c. Attach each valid child to the parent
ALTER INDEX public.idx_event_log_bronze_cursor
    ATTACH PARTITION public.P_bronze_cursor_idx;
```

```sql
-- 1d. Re-enumerate against races (a month boundary during the window can
-- create a new partition; it self-indexes, but confirm) and verify the
-- parent went valid:
--   re-run 1a: expect zero rows;
SELECT x.indisvalid
  FROM pg_index x
 WHERE x.indexrelid = 'public.idx_event_log_bronze_cursor'::regclass;
--   expect: t
```

Abort criteria: any 1b build that repeatedly fails; unexpected lock waits
visible in `pg_stat_activity`; robsond error-rate movement. Stop, drop the
invalid index, close the window; the runtime is unaffected (the parent
index being invalid has no effect on writers or existing plans).

## 2. Fence ownership and grants

Migration 000028 created `public.bronze_seal_fence` owned by the migration
role. Transfer to a dedicated stats definer and grant the exporter:

Prerequisite: the roles `bronze_fence_definer` (NOLOGIN, **INHERIT**,
member of `pg_read_all_stats`) and `robson_bronze_reader` are provisioned
by the rbx-infra isolated playbook (role provisioning is rbx-infra
territory; this runbook only performs the schema-side operations below).
INHERIT is load-bearing: with NOINHERIT the granted `pg_read_all_stats`
privileges would not be active during SECURITY DEFINER execution and the
fence would see masked sessions.

```sql
-- 2a. Verify the definer role and its effective stats membership
SELECT rolname, rolinherit, rolcanlogin FROM pg_roles
 WHERE rolname = 'bronze_fence_definer';
--   expect: rolinherit = t, rolcanlogin = f
SELECT pg_has_role('bronze_fence_definer', 'pg_read_all_stats', 'USAGE');
--   expect: t (membership usable via inheritance)

-- 2b. Ownership transfer (removes the unreadable_sessions availability
-- false positives from autovacuum/background workers)
ALTER FUNCTION public.bronze_seal_fence(timestamptz)
    OWNER TO bronze_fence_definer;

-- 2c. Exporter grant (role created by the bronze role-provisioning step)
GRANT EXECUTE ON FUNCTION public.bronze_seal_fence(timestamptz)
    TO robson_bronze_reader;
```

After 2b, this migration must never be re-run manually: `CREATE OR
REPLACE` by the application role would fail on ownership, by design.

```sql
-- 2d. Verify function, ACL and both cursor indexes
SELECT p.proname, r.rolname AS owner, p.prosecdef, p.proisstrict, p.proacl
  FROM pg_proc p JOIN pg_roles r ON r.oid = p.proowner
 WHERE p.oid = 'public.bronze_seal_fence(timestamptz)'::regprocedure;
--   expect owner=bronze_fence_definer, prosecdef=t, proisstrict=t,
--   proacl WITHOUT any =X/ (PUBLIC) entry
SELECT has_function_privilege('robson_bronze_reader',
       'public.bronze_seal_fence(timestamptz)', 'EXECUTE');
--   expect: t
SELECT has_function_privilege('robson',
       'public.bronze_seal_fence(timestamptz)', 'EXECUTE');
--   informational: expect f (the app role does not need EXECUTE)
SELECT * FROM public.bronze_seal_fence(now());
--   as postgres: expect one row; with the definer owning the function,
--   autovacuum/background sessions no longer count as unreadable
SELECT indexrelid::regclass, indisvalid FROM pg_index
 WHERE indexrelid IN ('public.idx_event_log_bronze_cursor'::regclass,
                      'public.idx_income_ledger_bronze_cursor'::regclass);
--   expect: both t (event_log parent only after §1 completes)
```

## 3. Ordering with the rest of Phase B

1. Contract artifacts merged: bronze-v1 at the CURRENT version (1.1.3:
   registry with STRATEGY_CREATED audited, PR #172; originally 1.1.1 via
   PR #168) and bronze-c1 profile + golden vectors (PR #170). The canary
   is contractually blocked without them, and the exporter binary must
   embed the matching registry (rbx-data re-vendor + rebuild).
2. Merge + sync applies 000027/000028 (this repo).
3. Role + pg_hba provisioning (rbx-infra isolated playbook, reload-only)
   creates `robson_bronze_reader` and `bronze_fence_definer`.
4. This runbook: §1 child indexes, §2 ownership + grants + verification.
5. Contabo conformance gate (rbx-data `bronze-conformance`, T1-T7 PASS).
6. Live `SELECT DISTINCT event_type` inventory vs the event registry.
7. Only then: exporter canary, suspended-first, per the plan's quantitative
   gate.

The exporter must independently verify `indisvalid = true` on both cursor
indexes before its first run (contract §4).
