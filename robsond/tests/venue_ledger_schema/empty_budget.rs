//! Test-only empty-budget preparation for fresh sqlx::test-owned databases.
//! Not a production initializer, source fence, native clock or send authority.
use chrono::{DateTime, Utc};
use sqlx::{postgres::PgRow, PgPool, Row};
use uuid::Uuid;

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../migrations");

const F03: &str = r#"
INSERT INTO budget_venue_state(venue_id) VALUES ('binance:spot')
ON CONFLICT(venue_id) DO UPDATE SET venue_id=EXCLUDED.venue_id
RETURNING venue_id,initialization_state,initialization_id,audit_id,
          quiescence_generation,schema_version,proof_version,
          recovery_generation,recovery_status,cohort_id,source_high_water,
          epoch_count_60,epoch_count_10;
"#;

const F04: &str = r#"
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
"#;

const F05: &str = r#"
INSERT INTO budget_source_audits(audit_id,venue_id,quiescence_generation,
    high_water,cursor,examined_count,status,failure_reason,started_at,
    completed_at,schema_version,proof_generation)
SELECT $1::uuid,'binance:spot',1,NULL,NULL,0,'complete',NULL,
       $2::timestamptz,$2::timestamptz,38,1
WHERE NOT EXISTS(SELECT 1 FROM submit_attempts)
  AND NOT EXISTS(SELECT 1 FROM order_intents)
  AND NOT EXISTS(SELECT 1 FROM spot_intent_clock)
  AND NOT EXISTS(SELECT 1 FROM budget_daily_charges)
  AND NOT EXISTS(SELECT 1 FROM budget_epochs)
  AND NOT EXISTS(SELECT 1 FROM budget_source_audits)
  AND NOT EXISTS(SELECT 1 FROM venue_pauses)
  AND NOT EXISTS(SELECT 1 FROM budget_recovery_cohorts)
  AND NOT EXISTS(SELECT 1 FROM budget_expiry_proofs)
RETURNING audit_id,venue_id,quiescence_generation,high_water,cursor,
          examined_count,status,failure_reason,
          (extract(epoch FROM started_at)*1000000)::bigint AS started_at_us,
          (extract(epoch FROM completed_at)*1000000)::bigint AS completed_at_us,
          schema_version,proof_generation;
"#;

const F06: &str = r#"
UPDATE budget_venue_state s
SET initialization_state='complete',initialization_id=$1::uuid,
    audit_id=a.audit_id,quiescence_generation=1,
    recovery_status='comparable',epoch_count_60=1,epoch_count_10=1
FROM budget_source_audits a
WHERE s.venue_id='binance:spot' AND s.initialization_state='incomplete'
  AND s.initialization_id IS NULL AND s.audit_id IS NULL
  AND s.quiescence_generation=0 AND s.schema_version=38 AND s.proof_version=1
  AND s.recovery_generation=0 AND s.cohort_id IS NULL AND s.source_high_water IS NULL
  AND s.recovery_status='unknown' AND s.epoch_count_60=9 AND s.epoch_count_10=3
  AND a.audit_id=$2::uuid AND a.venue_id=s.venue_id
  AND a.quiescence_generation=1 AND a.status='complete'
  AND a.high_water IS NULL AND a.cursor IS NULL AND a.examined_count=0
  AND a.completed_at IS NOT NULL AND a.failure_reason IS NULL
  AND a.schema_version=38 AND a.proof_generation=1
  AND NOT EXISTS(SELECT 1 FROM submit_attempts)
  AND NOT EXISTS(SELECT 1 FROM order_intents)
  AND NOT EXISTS(SELECT 1 FROM spot_intent_clock)
  AND NOT EXISTS(SELECT 1 FROM budget_daily_charges)
  AND NOT EXISTS(SELECT 1 FROM budget_epochs)
  AND NOT EXISTS(SELECT 1 FROM venue_pauses)
  AND NOT EXISTS(SELECT 1 FROM budget_recovery_cohorts)
  AND NOT EXISTS(SELECT 1 FROM budget_expiry_proofs)
  AND NOT EXISTS(SELECT 1 FROM budget_source_audits
                 WHERE audit_id<>a.audit_id)
RETURNING s.venue_id,s.initialization_state,s.initialization_id,s.audit_id,
          s.quiescence_generation,s.schema_version,s.proof_version,
          s.recovery_generation,s.recovery_status,s.cohort_id,s.source_high_water,
          s.epoch_count_60,s.epoch_count_10;
"#;

fn require(condition: bool) -> Result<(), sqlx::Error> {
    if condition {
        Ok(())
    } else {
        Err(sqlx::Error::Protocol("isolated empty fixture refused".into()))
    }
}
fn state(
    row: &PgRow,
    initialization: Option<Uuid>,
    audit: Option<Uuid>,
) -> Result<(), sqlx::Error> {
    let complete = initialization.is_some();
    require(
        row.try_get::<String, _>("venue_id")? == super::VENUE
            && row.try_get::<String, _>("initialization_state")?
                == if complete { "complete" } else { "incomplete" }
            && row.try_get::<Option<Uuid>, _>("initialization_id")? == initialization
            && row.try_get::<Option<Uuid>, _>("audit_id")? == audit
            && row.try_get::<i64, _>("quiescence_generation")? == i64::from(complete)
            && row.try_get::<i16, _>("schema_version")? == 38
            && row.try_get::<i16, _>("proof_version")? == 1
            && row.try_get::<i64, _>("recovery_generation")? == 0
            && row.try_get::<String, _>("recovery_status")?
                == if complete { "comparable" } else { "unknown" }
            && row.try_get::<Option<Uuid>, _>("cohort_id")?.is_none()
            && row.try_get::<Option<Uuid>, _>("source_high_water")?.is_none()
            && row.try_get::<i16, _>("epoch_count_60")? == if complete { 1 } else { 9 }
            && row.try_get::<i16, _>("epoch_count_10")? == if complete { 1 } else { 3 },
    )
}

// Only the explicit fresh-macro fixture path calls this, before creating
// intents. No arbitrary database name or caller-supplied ready/checksum flag is
// trusted.
pub(crate) async fn initialize(pool: &PgPool, at: DateTime<Utc>) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin_with("BEGIN ISOLATION LEVEL READ COMMITTED").await?;
    let checked = async {
    let settings = sqlx::query("SELECT set_config('lock_timeout','300ms',true), set_config('statement_timeout','1s',true), set_config('idle_in_transaction_session_timeout','1s',true)")
        .fetch_one(&mut *tx).await?;
    for (n, expected) in ["300ms", "1s", "1s"].iter().enumerate() {
        require(settings.try_get::<String, _>(n)? == *expected)?;
    }
    state(&sqlx::query(F03).fetch_one(&mut *tx).await?, None, None)?;
    let versions: Vec<i64> = MIGRATOR.iter().map(|m| m.version).collect();
    let checksums: Vec<Vec<u8>> = MIGRATOR.iter().map(|m| m.checksum.to_vec()).collect();
    let absence = sqlx::query(F04).bind(versions).bind(checksums).fetch_one(&mut *tx).await?;
    require(!absence.try_get::<String, _>("database_name")?.is_empty())?;
    for name in [
        "migration_lineage_ok",
        "source_empty",
        "intents_empty",
        "clocks_empty",
        "charges_empty",
        "epochs_empty",
        "audits_empty",
        "pauses_empty",
        "cohorts_empty",
        "proofs_empty",
    ] {
        require(absence.try_get::<bool, _>(name)?)?;
    }
    let audit = Uuid::now_v7();
    let initialization = Uuid::now_v7();
    let row = sqlx::query(F05).bind(audit).bind(at).fetch_one(&mut *tx).await?;
    require(
        row.try_get::<Uuid, _>("audit_id")? == audit
            && row.try_get::<String, _>("venue_id")? == super::VENUE
            && row.try_get::<i64, _>("quiescence_generation")? == 1
            && row.try_get::<Option<Uuid>, _>("high_water")?.is_none()
            && row.try_get::<Option<Uuid>, _>("cursor")?.is_none()
            && row.try_get::<i64, _>("examined_count")? == 0
            && row.try_get::<String, _>("status")? == "complete"
            && row.try_get::<Option<String>, _>("failure_reason")?.is_none()
            && row.try_get::<i64, _>("started_at_us")? == at.timestamp_micros()
            && row.try_get::<i64, _>("completed_at_us")? == at.timestamp_micros()
            && row.try_get::<i16, _>("schema_version")? == 38
            && row.try_get::<i64, _>("proof_generation")? == 1,
    )?;
    state(
        &sqlx::query(F06).bind(initialization).bind(audit).fetch_one(&mut *tx).await?,
        Some(initialization),
        Some(audit),
    )?;
    Ok::<(), sqlx::Error>(())
    }.await;
    match checked {
        Ok(()) => tx.commit().await,
        Err(error) => {
            tx.rollback().await?;
            Err(error)
        },
    }
}
