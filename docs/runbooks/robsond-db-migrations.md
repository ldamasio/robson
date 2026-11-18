# Robsond DB Migrations

Robsond owns the schema definition and migration files in `migrations`.
Production applies them through the `rbx-infra` ArgoCD application.

## Ownership

- `robson`: migration SQL files, `robsond db migrate`, known migration-state
  repairs, and CI validation against a clean PostgreSQL database.
- `rbx-infra`: Kubernetes Job, ArgoCD hook ordering, image tag wiring, secrets,
  and environment-specific execution.

## Rules

- Already-applied migration files are immutable.
- Schema corrections after a migration has shipped must be new migrations.
- `robsond db migrate` is the only deploy-time migration entrypoint.
- Migration repair logic must be narrow, explicit, schema-validated, and logged.
- A failed migration job must block rollout of a new `robsond` image.

## Current Historical Repairs

`robsond db migrate` first runs a constrained repair for known historical drift:

- `20240101000001_event_log_phase9`: normalizes legacy index names and updates
  the SQLx checksum from the pre-rename file to the current file checksum.
- `20240101000008_create_monthly_state`: replaces the historical zero checksum
  with the current file checksum after validating `monthly_state`.
- `20240101000010_add_realized_loss_trades_opened`: records the migration as
  applied when both columns already exist from the production hotfix.

Any other checksum mismatch remains a hard failure.

## Deployment Flow

1. The Robson CI builds and publishes the `robsond` image.
2. The CI updates both `robsond-deploy.yml` and `robsond-db-migrate-job.yml` in
   `rbx-infra` to the same image tag.
3. ArgoCD runs `robsond-db-migrate` as a Sync hook at wave `-1`.
4. Only after the migration hook succeeds does ArgoCD apply the `robsond`
   Deployment in the default wave.

## Operator Checks

Check ArgoCD:

```bash
kubectl get application robson-prod -n argocd -o wide
```

Check the migration hook logs during a deploy:

```bash
kubectl logs -n robson -l job-name=robsond-db-migrate
```

Check migration status from the runtime image:

```bash
robsond db status
```

## CI Guard

The `Robsond CI/CD` workflow starts a clean PostgreSQL service and runs:

```bash
DATABASE_URL=postgresql://postgres:postgres@localhost:5432/robson_migration_ci \
  cargo run -p robsond --features postgres -- db migrate
```

This catches non-idempotent DDL, duplicate index names, and broken migration
ordering before an image can be published.

## ADR-0056 step 8e: additive ledger and spot one-attempt index

Migration 36 creates the balance-account ledger; 37 creates the spot-only unique
index concurrently. They do not select spot. The factory continues to refuse
`binance_spot` until the separately authorized step 8o activation. At step 8e-1,
there are no production ledger callers. The balance-journal readiness checks
belong to the later step 8e-3 and must not be claimed as implemented by 8e-1.

The schema requires PostgreSQL 12 or newer (generated stored columns). Verify
the target's `server_version_num` through existing authenticated access without
printing credentials. The later flag-on readiness check also needs TEMP
privilege; ordinary USD-M readiness must not require it.

Before 8e-1 merges, the entire five-part 8e stack must pass its schema/writer,
upgrade, old-pod compatibility and independent review acceptance. Deploy 8e-1
alone first. The migration hook runs while the previous daemon is still alive:
36 adds objects and references no table that daemon writes. Migration 37 takes
`SHARE UPDATE EXCLUSIVE`, allowing ordinary INSERT/UPDATE/DELETE but potentially
waiting for maintenance or DDL. A failed hook blocks rollout.

After deployment, the operator inspects migration records, index health and the
eight new tables through the approved database inspection route. No agent reads
`robsond-secret` or guesses credentials. Preserve the output as release evidence:

```sql
SELECT version, success FROM _sqlx_migrations
WHERE version IN (20240101000036, 20240101000037) ORDER BY version;

SELECT n.nspname, c.relname, t.relname AS indexed_table, a.amname,
       i.indisunique, i.indisvalid, i.indisready, i.indimmediate,
       i.indnkeyatts, i.indnatts, pg_get_indexdef(i.indexrelid),
       pg_get_expr(i.indpred, i.indrelid, false) AS predicate
FROM pg_index i
JOIN pg_class c ON c.oid = i.indexrelid
JOIN pg_class t ON t.oid = i.indrelid
JOIN pg_namespace n ON n.oid = c.relnamespace
JOIN pg_am a ON a.oid = c.relam
WHERE c.oid = to_regclass('public.idx_submit_attempts_one_per_spot_client_id');

SELECT 'accounting_state' AS table_name, count(*) FROM accounting_state
UNION ALL SELECT 'venue_accounts', count(*) FROM venue_accounts
UNION ALL SELECT 'leg_routes', count(*) FROM leg_routes
UNION ALL SELECT 'balance_leg_admissions', count(*) FROM balance_leg_admissions
UNION ALL SELECT 'balance_legs', count(*) FROM balance_legs
UNION ALL SELECT 'venue_orders', count(*) FROM venue_orders
UNION ALL SELECT 'order_fills', count(*) FROM order_fills
UNION ALL SELECT 'spot_intent_clock', count(*) FROM spot_intent_clock;
```

The index must be unique, valid, ready and immediate, use btree on the actual
`submit_attempts` relation, have exactly three ordinary keys in the order
`venue_id, account_scope, client_order_id`, and restrict rows to
`venue_id = 'binance:spot'`. An index name alone establishes none of these.
For automated predicate comparison, use the same server's rendering of an
expected index on a disposable temporary LIKE relation; never strip casts or
hard-code another server version's deparsed text. That temporary-object probe
is DDL inside a rolled-back transaction, not read-only SQL. Step 8e-3 owns its
bounded implementation and cleanup acceptance.

All eight table counts must be zero after 8e-1 deployment and again after 24 h.
Record the deployed image digest and a compatible rollback image before the
next merge. **Once 36/37 are applied, a pre-8e image is not a rollback target:**
SQLx rejects applied migrations missing from its embedded set. The earliest
compatible rollback artifact is the recorded 8e-1 image embedding both files.
Compatibility of the already-running old pod with additive DDL does not make
its old migration runner a compatible rollback artifact.

### Failed or incorrect spot index

A failed concurrent unique-index build can leave an invalid index. Re-running
37 with `IF NOT EXISTS` does not repair it or validate its definition. Stop the
rollout investigation and obtain the operator's direction; do not weaken an
index/readiness check. After diagnosis, the operator's repair procedure is:

1. Inspect the catalog query above, including the full definition.
2. Check for conflicting spot identities:

   ```sql
   SELECT venue_id, account_scope, client_order_id, count(*)
   FROM submit_attempts WHERE venue_id = 'binance:spot'
   GROUP BY 1, 2, 3 HAVING count(*) > 1;
   ```

   Any result is an incident. Stop: do not delete attempts, rewrite identities
   or continue with the build until the operator resolves the evidence.
3. With no duplicates and an authorized repair window, run each statement
   separately, outside a transaction:

   ```sql
   DROP INDEX CONCURRENTLY IF EXISTS public.idx_submit_attempts_one_per_spot_client_id;
   ```

   ```sql
   CREATE UNIQUE INDEX CONCURRENTLY IF NOT EXISTS idx_submit_attempts_one_per_spot_client_id
       ON public.submit_attempts (venue_id, account_scope, client_order_id)
       WHERE venue_id = 'binance:spot';
   ```

4. Reinspect every property. If the failed migration has no recorded success,
   rerun the canonical `robsond db migrate` entrypoint only after the index is
   correct; SQLx records the successful migration. Never edit
   `_sqlx_migrations` to hide the failure.

Applied migration 36 is immutable, including the `VOLATILE` admission function.
Any later body change needs a new numbered migration and compatibility review.
`CREATE OR REPLACE FUNCTION` must preserve the callable signature and return
type; changing argument types creates a different function. Do not overwrite
36 or deploy an image carrying a modified checksum.

## Migration 38: request budget schema through the bounded adapter

`robson_db::migrate()` runs every pending migration through
`bounded_migration::BoundedMigration`, a dedicated raw connection that is never
returned to a pool. Migrations other than 38 keep SQLx's own apply path.
Migration 38 (`20240101000038_request_budget.sql`) is applied as 58 operations
on that connection: settings install, BEGIN, the 52 fixed DDL slices of the
original file, the original `_sqlx_migrations` INSERT, COMMIT, the
`execution_time` UPDATE after the acknowledged COMMIT, and the settings restore.
Each statement runs with `statement_timeout=40ms`, `lock_timeout=20ms` and
`idle_in_transaction_session_timeout=20ms`; the whole apply is bounded at
200 ms; the connection and the migration advisory lock are each bounded at 5 s. Any bound exceeded fails
`robsond db migrate`, so the `robsond-db-migrate` Sync hook job fails and the
sync stops before the new deployment (see Deployment Flow).

### What migration 38 changes at deploy

- New request-budget tables, functions and triggers; a second reason check
  `recovery_incidents_reason_v38` (NOT VALID) beside the original one.
- Every insert into `spot_intent_clock` is projected into the daily budget and
  is **refused (23514) until `budget_venue_state` for that venue is
  `complete`**. While Spot stays refused by the production factory no clock row
  is written, so this is inert for USD-M; enabling Spot needs the venue
  initialization first.

### Before the merge that carries 38

Record the deployed image digest. **Once 38 is applied, an image without 38 is
not a rollback target**: SQLx rejects applied migrations missing from its
embedded set. The earliest compatible rollback artifact is the first image
that embeds 38; record it before any later merge.

### Failure outcomes

The adapter logs `migration connection retired` (warn) with the phase and one
outcome (`kubectl logs -n robson -l job-name=robsond-db-migrate`), then drops
the raw connection without a graceful close, so a pending ROLLBACK is discarded
with the session. Dropping the socket does not prove the server backend has
finished: it may still be completing the transaction.

| Outcome | Meaning | Operator check |
|---|---|---|
| `ApplyNotStarted` | failed before the transaction began | 38 absent; rerun `robsond db migrate` after the cause is fixed |
| `MayHaveCommitted` | interrupted anywhere from BEGIN through the post-commit `execution_time` UPDATE; the COMMIT may or may not have landed | unresolved until the checks below; never infer absence |
| `AppliedButSessionUnrestored` | COMMIT and the `execution_time` UPDATE returned; only the settings restore failed | 38 is applied; the session settings died with the connection |

For `MayHaveCommitted`, an empty history row read from another connection is
not proof of absence while the original backend can still commit. In order:

1. Wait until no backend of the failed migrate job remains: check
   `pg_stat_activity` for the job's connection (and any lock it held in
   `pg_locks`) until it is gone. If it does not go away, stop and get the
   operator's direction; the outcome stays unresolved.
2. Rerun the canonical `robsond db migrate`. It takes SQLx's migration lock
   before it reads the applied history, so its decision is serialized behind
   any surviving migrator: it skips 38 if the COMMIT landed and applies it if
   not.
3. Afterwards confirm with:

   ```sql
   SELECT version, success, execution_time
   FROM _sqlx_migrations WHERE version = 20240101000038;
   ```

   A row with `success = true` means 38 is applied; an `execution_time` of
   `-1` only means the post-commit UPDATE did not land, and SQLx treats the
   migration as applied.

Do not edit `_sqlx_migrations` or rerun 38's SQL by hand. A database lost
mid-apply follows the same steps once it is reachable again. Repeated deadline
failures on a busy database need the operator's direction, not wider bounds.
