-- F04 $1 expected versions BIGINT[], $2 corresponding checksums BYTEA[].
-- Both derive from a test-private sqlx::migrate!("../migrations") static.
-- No caller-authored checksum/ready value. One row, all booleans required.
-- This is fresh-macro lineage/fixture checking, NOT full C1/C2 verification.
WITH expected AS MATERIALIZED (
    SELECT v,c FROM unnest($1::bigint[],$2::bytea[]) AS x(v,c)
), actual AS MATERIALIZED (
    SELECT version,checksum,success FROM _sqlx_migrations
)
SELECT current_database() AS database_name,
       cardinality($1::bigint[])=cardinality($2::bytea[])
       AND cardinality($1::bigint[])>0
       AND (SELECT count(*)=count(DISTINCT v) FROM expected)
       AND NOT EXISTS(SELECT 1 FROM expected e FULL JOIN actual a ON a.version=e.v
                      WHERE e.v IS NULL OR a.version IS NULL OR NOT a.success
                         OR a.checksum IS DISTINCT FROM e.c)
       AND EXISTS(SELECT 1 FROM expected WHERE v=20240101000038) AS migration_lineage_ok,
       NOT EXISTS(SELECT 1 FROM submit_attempts) AS source_empty,
       NOT EXISTS(SELECT 1 FROM order_intents) AS intents_empty,
       NOT EXISTS(SELECT 1 FROM spot_intent_clock) AS clocks_empty,
       NOT EXISTS(SELECT 1 FROM budget_daily_charges) AS charges_empty,
       NOT EXISTS(SELECT 1 FROM budget_epochs) AS epochs_empty,
       NOT EXISTS(SELECT 1 FROM budget_source_audits) AS audits_empty,
       NOT EXISTS(SELECT 1 FROM venue_pauses) AS pauses_empty,
       NOT EXISTS(SELECT 1 FROM budget_recovery_cohorts) AS cohorts_empty,
       NOT EXISTS(SELECT 1 FROM budget_expiry_proofs) AS proofs_empty;
