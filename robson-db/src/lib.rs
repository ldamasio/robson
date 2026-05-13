//! Database lifecycle management for Robson v2.
//!
//! Provides migration running, status checking, and minimal data seeding.

mod init;

pub use init::init_minimal_data;
use sqlx::{PgPool, Row};
use tracing::{info, warn};

/// Result type for DB operations.
pub type Result<T> = std::result::Result<T, anyhow::Error>;

/// Run all pending migrations.
///
/// Uses sqlx migrations from the `v2/migrations` directory.
/// Idempotent: safe to run multiple times.
pub async fn migrate(pool: &PgPool) -> Result<()> {
    info!("Running database migrations...");

    repair_known_migration_state(pool).await?;
    sqlx::migrate!("../migrations").run(pool).await?;

    info!("Migrations completed successfully");
    Ok(())
}

/// Normalize known historical migration metadata drift before SQLx validates
/// checksums.
///
/// This is intentionally narrow. It only repairs states that are known to be
/// equivalent to the current migration files and validates the live schema
/// before changing `_sqlx_migrations`.
pub async fn repair_known_migration_state(pool: &PgPool) -> Result<()> {
    if !table_exists(pool, "_sqlx_migrations").await? {
        return Ok(());
    }

    repair_event_log_phase9_index_drift(pool).await?;
    repair_monthly_state_zero_checksum(pool).await?;
    record_realized_loss_migration_if_already_applied(pool).await?;
    repair_safety_net_table_permissions(pool).await?;

    Ok(())
}

async fn repair_event_log_phase9_index_drift(pool: &PgPool) -> Result<()> {
    const VERSION: i64 = 20240101000001;
    const LEGACY_CHECKSUM: &str = "3f67f5a624146c14c3112b584a5d56263f7772feaee4e16601d96819a64d9e44758187260146aae0d79199189b15aae5";
    const CURRENT_CHECKSUM: &str = "1fb5f7ce99f9d40c0f66da184d716770776b4a561c494164fa77b27167518bb16a249c807fe8575a48901dd98b55990c";

    let Some(checksum) = migration_checksum(pool, VERSION).await? else {
        return Ok(());
    };

    if checksum == CURRENT_CHECKSUM {
        return Ok(());
    }

    if checksum != LEGACY_CHECKSUM {
        anyhow::bail!(
            "unexpected checksum for migration {VERSION}: {checksum}; refusing automatic repair"
        );
    }

    require_table(pool, "orders_current").await?;
    require_table(pool, "positions_current").await?;

    sqlx::query(
        r#"
        DO $$
        BEGIN
            IF to_regclass('public.idx_orders_status') IS NOT NULL
               AND to_regclass('public.idx_orders_current_status') IS NULL THEN
                ALTER INDEX public.idx_orders_status RENAME TO idx_orders_current_status;
            END IF;

            IF to_regclass('public.idx_positions_state') IS NOT NULL
               AND to_regclass('public.idx_positions_current_state') IS NULL THEN
                ALTER INDEX public.idx_positions_state RENAME TO idx_positions_current_state;
            END IF;

            IF to_regclass('public.idx_positions_symbol') IS NOT NULL
               AND to_regclass('public.idx_positions_current_symbol') IS NULL THEN
                ALTER INDEX public.idx_positions_symbol RENAME TO idx_positions_current_symbol;
            END IF;
        END $$;
        "#,
    )
    .execute(pool)
    .await?;

    update_migration_checksum(pool, VERSION, CURRENT_CHECKSUM).await?;
    info!("Repaired migration metadata for v{VERSION} event log phase9 index names");

    Ok(())
}

async fn repair_monthly_state_zero_checksum(pool: &PgPool) -> Result<()> {
    const VERSION: i64 = 20240101000008;
    const ZERO_CHECKSUM: &str = "0000000000000000000000000000000000000000000000000000000000000000";
    const CURRENT_CHECKSUM: &str = "695cb2fd4d7d434c772b43b77e237146c112c1964378f474dc44dfb7ed6127309148f8848b12e892627e6d5ac365c14a";

    let Some(checksum) = migration_checksum(pool, VERSION).await? else {
        return Ok(());
    };

    if checksum == CURRENT_CHECKSUM {
        return Ok(());
    }

    if checksum != ZERO_CHECKSUM {
        anyhow::bail!(
            "unexpected checksum for migration {VERSION}: {checksum}; refusing automatic repair"
        );
    }

    require_table(pool, "monthly_state").await?;
    require_column(pool, "monthly_state", "capital_base").await?;
    require_column(pool, "monthly_state", "carried_risk").await?;

    update_migration_checksum(pool, VERSION, CURRENT_CHECKSUM).await?;
    info!("Repaired zero checksum for v{VERSION} monthly_state migration");

    Ok(())
}

async fn record_realized_loss_migration_if_already_applied(pool: &PgPool) -> Result<()> {
    const VERSION: i64 = 20240101000010;
    const DESCRIPTION: &str = "add realized loss trades opened";
    const CHECKSUM: &str = "9a3d5992c92a0071a2f499d7eff35a1aa2491f9fc118f2947d001ba55b778662fe95e442e6ec50cc24911a52440bcfa9";

    if migration_checksum(pool, VERSION).await?.is_some() {
        return Ok(());
    }

    if !(column_exists(pool, "monthly_state", "realized_loss").await?
        && column_exists(pool, "monthly_state", "trades_opened").await?)
    {
        return Ok(());
    }

    sqlx::query(
        r#"
        INSERT INTO _sqlx_migrations (version, description, success, checksum, execution_time)
        VALUES ($1, $2, TRUE, decode($3, 'hex'), 0)
        ON CONFLICT (version) DO NOTHING
        "#,
    )
    .bind(VERSION)
    .bind(DESCRIPTION)
    .bind(CHECKSUM)
    .execute(pool)
    .await?;

    info!("Recorded v{VERSION} migration metadata for already-applied monthly_state columns");

    Ok(())
}

/// Try to grant DML on safety net tables to the current role.
///
/// Tables created by a superuser during initial provisioning are owned by that
/// superuser, not the runtime role. If the current user lacks GRANT OPTION the
/// attempt is silently skipped (migration 012 also attempts this at the SQL
/// level). The function never fails the migration run.
async fn repair_safety_net_table_permissions(pool: &PgPool) -> Result<()> {
    for table in ["detected_positions", "safety_net_executions"] {
        // Best-effort GRANT; swallow errors so migrations proceed regardless.
        let result = sqlx::query(&format!("GRANT ALL PRIVILEGES ON TABLE {table} TO CURRENT_USER"))
            .execute(pool)
            .await;

        match result {
            Ok(_) => info!("Granted DML on {table} to current user"),
            Err(e) if e.to_string().contains("permission denied") => {
                // Table exists but is owned by another role.
                if let Ok(Some(owner)) = sqlx::query_scalar::<_, Option<String>>(
                    "SELECT tableowner FROM pg_tables WHERE tablename = $1 AND schemaname = 'public'"
                )
                .bind(table)
                .fetch_optional(pool)
                .await
                .map(|o| o.flatten())
                {
                    warn!(
                        owner = %owner,
                        table,
                        "{table} is owned by '{owner}', not the migration user. \
                         Run as superuser: GRANT ALL ON TABLE {table} TO robson"
                    );
                }
            },
            Err(e) if e.to_string().contains("does not exist") => {
                // Table not created yet — migration 003 will handle it.
            },
            Err(e) => {
                warn!(error = %e, table, "Unexpected error granting on {table}");
            },
        }
    }

    Ok(())
}

async fn migration_checksum(pool: &PgPool, version: i64) -> Result<Option<String>> {
    let checksum = sqlx::query_scalar::<_, Option<String>>(
        "SELECT encode(checksum, 'hex') FROM _sqlx_migrations WHERE version = $1",
    )
    .bind(version)
    .fetch_optional(pool)
    .await?;

    Ok(checksum.flatten())
}

async fn update_migration_checksum(pool: &PgPool, version: i64, checksum: &str) -> Result<()> {
    sqlx::query("UPDATE _sqlx_migrations SET checksum = decode($1, 'hex') WHERE version = $2")
        .bind(checksum)
        .bind(version)
        .execute(pool)
        .await?;

    Ok(())
}

async fn require_table(pool: &PgPool, table_name: &str) -> Result<()> {
    if !table_exists(pool, table_name).await? {
        anyhow::bail!("required table {table_name} not found; refusing migration repair");
    }
    Ok(())
}

async fn require_column(pool: &PgPool, table_name: &str, column_name: &str) -> Result<()> {
    if !column_exists(pool, table_name, column_name).await? {
        anyhow::bail!(
            "required column {table_name}.{column_name} not found; refusing migration repair"
        );
    }
    Ok(())
}

async fn table_exists(pool: &PgPool, table_name: &str) -> Result<bool> {
    let exists = sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS (
            SELECT 1
            FROM information_schema.tables
            WHERE table_schema = 'public'
              AND table_name = $1
        )
        "#,
    )
    .bind(table_name)
    .fetch_one(pool)
    .await?;

    Ok(exists)
}

async fn column_exists(pool: &PgPool, table_name: &str, column_name: &str) -> Result<bool> {
    let exists = sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS (
            SELECT 1
            FROM information_schema.columns
            WHERE table_schema = 'public'
              AND table_name = $1
              AND column_name = $2
        )
        "#,
    )
    .bind(table_name)
    .bind(column_name)
    .fetch_one(pool)
    .await?;

    Ok(exists)
}

/// Check database connectivity and migration status.
///
/// Prints current migration version and any pending migrations.
pub async fn status(pool: &PgPool) -> Result<()> {
    // Check connectivity
    let result: i64 = sqlx::query_scalar("SELECT 1::BIGINT").fetch_one(pool).await?;

    if result != 1 {
        return Err(anyhow::anyhow!("Database connectivity check failed"));
    }

    info!("Database connectivity: OK");

    // Check migration status using runtime query (sqlx::query! requires DB at
    // compile time)
    let rows = sqlx::query(
        r#"
        SELECT version, description, installed_on::TEXT AS installed_on, success
        FROM _sqlx_migrations
        ORDER BY version DESC
        LIMIT 10
        "#,
    )
    .fetch_all(pool)
    .await;

    match rows {
        Ok(migs) if !migs.is_empty() => {
            info!("Latest migrations:");
            for mig in migs {
                let version: i64 = mig.get("version");
                let description: String = mig.get("description");
                let installed_on: Option<String> = mig.get("installed_on");
                let success: Option<bool> = mig.get("success");

                let status = if success.unwrap_or(true) {
                    "✓"
                } else {
                    "✗"
                };
                info!(
                    "  {} v{}: {} ({})",
                    status,
                    version,
                    description,
                    installed_on.unwrap_or_else(|| "N/A".to_string())
                );
            }
        },
        Ok(_) => {
            warn!("No migrations found in database (run `robsond db migrate` first)");
        },
        Err(e) => {
            // Table might not exist yet
            if e.to_string().contains("_sqlx_migrations") {
                warn!("Migration table not found (run `robsond db migrate` first)");
            } else {
                return Err(e.into());
            }
        },
    }

    Ok(())
}
