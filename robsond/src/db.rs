//! Database CLI subcommands for robsond.
//!
//! Provides `db migrate`, `db status`, and `db init` commands.

use std::env;

use anyhow::{anyhow, Result};
use robson_db::{init_minimal_data, migrate, repair_known_migration_state, status};
use tracing::info;

const ADR0052_MIGRATION: &str = "20240101000026_executable_span_stop_policy";
const STOP_PLAN_ENTRY_REFERENCE_MIGRATION: &str = "20240101000029_add_stop_plan_entry_reference";

/// Verify that the projection schema required by the ADR-0052 binary is
/// present before the daemon can recover positions, serve traffic, or accept
/// arms.
pub(crate) async fn verify_adr0052_schema_readiness(pool: &sqlx::PgPool) -> Result<()> {
    let present: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(DISTINCT column_name)::BIGINT
        FROM information_schema.columns
        WHERE table_schema = current_schema()
          AND table_name = 'positions_current'
          AND column_name IN (
              'initial_executable_stop',
              'executable_span',
              'cap_basis_distance',
              'tick_size_at_admission',
              'stop_plan_entry_reference'
          )
        "#,
    )
    .fetch_one(pool)
    .await?;

    if present != 5 {
        return Err(anyhow!(
            "ADR-0052 schema readiness failed: migrations {ADR0052_MIGRATION} and {STOP_PLAN_ENTRY_REFERENCE_MIGRATION} are required before this binary starts (found {present}/5 admission columns on positions_current)"
        ));
    }

    Ok(())
}

/// Run database CLI subcommands.
///
/// Supported commands:
/// - `robsond db migrate` - Run pending migrations
/// - `robsond db repair` - Normalize known historical migration metadata drift
/// - `robsond db status` - Check migration status
/// - `robsond db init [--tenant-id UUID] [--account-id UUID]` - Seed minimal
///   data
pub async fn run_db_command(args: Vec<String>) -> Result<()> {
    if args.len() < 3 {
        return Err(anyhow!("Usage: robsond db <migrate|status|init> [options]"));
    }

    let database_url = env::var("DATABASE_URL")
        .map_err(|_| anyhow!("DATABASE_URL environment variable is required for db commands"))?;

    let pool = sqlx::PgPool::connect(&database_url).await?;

    match args[2].as_str() {
        "migrate" => {
            migrate(&pool).await?;
        },
        "repair" => {
            repair_known_migration_state(&pool).await?;
        },
        "status" => {
            status(&pool).await?;
        },
        "init" => {
            let mut tenant_id = None;
            let mut account_id = None;

            // Parse optional arguments
            let mut i = 3;
            while i < args.len() {
                match args[i].as_str() {
                    "--tenant-id" => {
                        if i + 1 < args.len() {
                            tenant_id = Some(args[i + 1].parse()?);
                            i += 2;
                        } else {
                            return Err(anyhow!("--tenant-id requires a value"));
                        }
                    },
                    "--account-id" => {
                        if i + 1 < args.len() {
                            account_id = Some(args[i + 1].parse()?);
                            i += 2;
                        } else {
                            return Err(anyhow!("--account-id requires a value"));
                        }
                    },
                    _ => {
                        return Err(anyhow!("Unknown option: {}", args[i]));
                    },
                }
            }

            let (tid, aid) = init_minimal_data(&pool, tenant_id, account_id).await?;
            info!("Initialized: tenant_id={}, account_id={}", tid, aid);
            // Print to stdout so operators can capture the value for secret provisioning:
            //   PROJECTION_TENANT_ID=$(robsond db init | grep TENANT_ID | awk '{print $2}')
            println!("TENANT_ID={}", tid);
            println!("ACCOUNT_ID={}", aid);
        },
        _ => {
            return Err(anyhow!(
                "Unknown db command: {}. Use migrate, repair, status, or init",
                args[2]
            ));
        },
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[sqlx::test(migrations = "../migrations")]
    #[ignore = "requires DATABASE_URL"]
    async fn adr0052_schema_readiness_rejects_a_missing_admission_column(pool: sqlx::PgPool) {
        verify_adr0052_schema_readiness(&pool).await.unwrap();

        sqlx::query("ALTER TABLE positions_current DROP COLUMN stop_plan_entry_reference CASCADE")
            .execute(&pool)
            .await
            .unwrap();

        let error = verify_adr0052_schema_readiness(&pool).await.unwrap_err();
        let detail = error.to_string();
        assert!(detail.contains(ADR0052_MIGRATION), "unexpected error: {detail}");
        assert!(
            detail.contains(STOP_PLAN_ENTRY_REFERENCE_MIGRATION),
            "unexpected error: {detail}"
        );
        assert!(detail.contains("found 4/5"), "unexpected error: {detail}");
    }

    #[sqlx::test(migrations = "../migrations")]
    #[ignore = "requires DATABASE_URL"]
    async fn adr0052_admission_evidence_is_write_once(pool: sqlx::PgPool) {
        let position_id = uuid::Uuid::now_v7();
        let now = chrono::Utc::now();
        sqlx::query(
            r#"
            INSERT INTO positions_current (
                position_id, tenant_id, account_id, strategy_id,
                symbol, side, state, entry_price, entry_quantity,
                current_quantity, stop_policy,
                stop_plan_entry_reference,
                initial_executable_stop, executable_span,
                cap_basis_distance, tick_size_at_admission,
                last_event_id, last_seq, created_at, updated_at
            ) VALUES (
                $1, $2, $3, $4,
                'BTCUSDT', 'long', 'armed', 100, 1,
                1, 'executable_span',
                100,
                90, 10, 10, 0.1,
                $5, 1, $6, $6
            )
            "#,
        )
        .bind(position_id)
        .bind(uuid::Uuid::now_v7())
        .bind(uuid::Uuid::now_v7())
        .bind(uuid::Uuid::now_v7())
        .bind(uuid::Uuid::now_v7())
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();

        // Idempotent projection replay is allowed.
        sqlx::query(
            r#"
            UPDATE positions_current
            SET stop_plan_entry_reference = 100,
                initial_executable_stop = 90,
                executable_span = 10,
                cap_basis_distance = 10,
                tick_size_at_admission = 0.1
            WHERE position_id = $1
            "#,
        )
        .bind(position_id)
        .execute(&pool)
        .await
        .unwrap();

        let rewrites = [
            (
                "UPDATE positions_current SET stop_plan_entry_reference = 101 WHERE position_id = $1",
                "stop_plan_entry_reference",
            ),
            (
                "UPDATE positions_current SET initial_executable_stop = 89 WHERE position_id = $1",
                "initial_executable_stop",
            ),
            (
                "UPDATE positions_current SET executable_span = 11 WHERE position_id = $1",
                "executable_span",
            ),
            (
                "UPDATE positions_current SET cap_basis_distance = 11 WHERE position_id = $1",
                "cap_basis_distance",
            ),
            (
                "UPDATE positions_current SET tick_size_at_admission = 0.2 WHERE position_id = $1",
                "tick_size_at_admission",
            ),
        ];

        for (statement, column) in rewrites {
            let error = sqlx::query(statement).bind(position_id).execute(&pool).await.unwrap_err();
            assert!(
                error.to_string().contains(&format!("{column} is immutable once set")),
                "unexpected rewrite error for {column}: {error}"
            );
        }
    }
}
