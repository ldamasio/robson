//! Strictly test-only fresh sqlx-owned database preparation. This is not a
//! producer of production C2, source fencing or RuntimeReady evidence.
use chrono::{DateTime, Utc};
use robson_exec::venue_clock::NativeClock;
use sqlx::{ConnectOptions, PgPool, Row};
use tokio::time::{timeout_at, Instant};
use uuid::Uuid;

use super::{
    bootstrap::{issue, BootstrapInput},
    completion,
    snapshot::{check_state, get},
    BudgetError, Trace,
};

static MIGRATIONS: sqlx::migrate::Migrator = sqlx::migrate!("../migrations");

pub(crate) async fn prepare(
    pool: &PgPool,
    clock: &NativeClock,
    deadline: Instant,
    trace: &Trace,
) -> Result<BootstrapInput, BudgetError> {
    let audit = Uuid::now_v7();
    let initialization = Uuid::now_v7();
    let at = DateTime::from_timestamp_micros(Utc::now().timestamp_micros())
        .ok_or(BudgetError::Invalid)?;
    let start = clock.capture().map_err(|_| BudgetError::Invalid)?;
    let versions: Vec<i64> = MIGRATIONS.iter().map(|m| m.version).collect();
    let checksums: Vec<Vec<u8>> = MIGRATIONS.iter().map(|m| m.checksum.to_vec()).collect();
    let work = async {
        if Instant::now() >= deadline {
            return Err(BudgetError::Expired);
        }
        let mut connection =
            pool.connect_options().connect().await.map_err(completion::classify)?;
        issue(trace, deadline)?;
        completion::run(&mut connection, sqlx::query(include_str!("sql/fixture_begin.sql")), false)
            .await?;
        issue(trace, deadline)?;
        let row =
            completion::one(&mut connection, sqlx::query(include_str!("sql/fixture_settings.sql")))
                .await?;
        for (name, value) in [
            ("lock_timeout", "300ms"),
            ("statement_timeout", "1s"),
            ("idle_timeout", "1s"),
        ] {
            if get::<String>(&row, name)? != value {
                return Err(BudgetError::Invalid);
            }
        }
        issue(trace, deadline)?;
        let row =
            completion::one(&mut connection, sqlx::query(include_str!("sql/fixture_lock.sql")))
                .await?;
        if row.len() != 13
            || get::<String>(&row, "venue_id")? != "binance:spot"
            || get::<String>(&row, "initialization_state")? != "incomplete"
            || get::<Option<Uuid>>(&row, "initialization_id")?.is_some()
            || get::<Option<Uuid>>(&row, "audit_id")?.is_some()
            || get::<i64>(&row, "quiescence_generation")? != 0
            || get::<i16>(&row, "schema_version")? != 38
            || get::<i16>(&row, "proof_version")? != 1
            || get::<i64>(&row, "recovery_generation")? != 0
            || get::<String>(&row, "recovery_status")? != "unknown"
            || get::<Option<Uuid>>(&row, "cohort_id")?.is_some()
            || get::<Option<Uuid>>(&row, "source_high_water")?.is_some()
            || get::<i16>(&row, "epoch_count_60")? != 9
            || get::<i16>(&row, "epoch_count_10")? != 3
        {
            return Err(BudgetError::Invalid);
        }
        issue(trace, deadline)?;
        let row = completion::one(
            &mut connection,
            sqlx::query(include_str!("sql/fixture_absence.sql"))
                .bind(&versions)
                .bind(&checksums),
        )
        .await?;
        if row.len() != 11 {
            return Err(BudgetError::Invalid);
        }
        // Macro ownership and the reviewed canonical test driver are mandatory
        // external preconditions; the returned name alone supplies no authority.
        let _database: String = get(&row, "database_name")?;
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
            if !get::<bool>(&row, name)? {
                return Err(BudgetError::Invalid);
            }
        }
        issue(trace, deadline)?;
        let row = completion::one(
            &mut connection,
            sqlx::query(include_str!("sql/fixture_audit.sql")).bind(audit).bind(at),
        )
        .await?;
        if row.len() != 12
            || get::<Uuid>(&row, "audit_id")? != audit
            || get::<String>(&row, "venue_id")? != "binance:spot"
            || get::<i64>(&row, "quiescence_generation")? != 1
            || get::<Option<Uuid>>(&row, "high_water")?.is_some()
            || get::<Option<Uuid>>(&row, "cursor")?.is_some()
            || get::<i64>(&row, "examined_count")? != 0
            || get::<String>(&row, "status")? != "complete"
            || get::<Option<String>>(&row, "failure_reason")?.is_some()
            || get::<i64>(&row, "started_at_us")? != at.timestamp_micros()
            || get::<i64>(&row, "completed_at_us")? != at.timestamp_micros()
            || get::<i16>(&row, "schema_version")? != 38
            || get::<i64>(&row, "proof_generation")? != 1
        {
            return Err(BudgetError::Invalid);
        }
        issue(trace, deadline)?;
        let row = completion::one(
            &mut connection,
            sqlx::query(include_str!("sql/fixture_state.sql"))
                .bind(initialization)
                .bind(audit),
        )
        .await?;
        let input = BootstrapInput { audit, generation: 1, start };
        check_state(&row, &input)?;
        if row.len() != 14
            || get::<Option<Uuid>>(&row, "initialization_id")? != Some(initialization)
        {
            return Err(BudgetError::Invalid);
        }
        issue(trace, deadline)?;
        completion::run(
            &mut connection,
            sqlx::query(include_str!("sql/fixture_commit.sql")),
            false,
        )
        .await?;
        if Instant::now() >= deadline {
            return Err(BudgetError::Expired);
        }
        Ok(input)
    };
    timeout_at(deadline, work).await.map_err(|_| BudgetError::Expired)?
}
