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

    sqlx::migrate!("../migrations").run(pool).await?;

    info!("Migrations completed successfully");
    Ok(())
}

/// Check database connectivity and migration status.
///
/// Prints current migration version and any pending migrations.
pub async fn status(pool: &PgPool) -> Result<()> {
    // Check connectivity
    let result: i64 = sqlx::query_scalar("SELECT 1").fetch_one(pool).await?;

    if result != 1 {
        return Err(anyhow::anyhow!("Database connectivity check failed"));
    }

    info!("Database connectivity: OK");

    // Check migration status using runtime query (sqlx::query! requires DB at compile time)
    let rows = sqlx::query(
        r#"
        SELECT version, description, installed_on, success
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
                let installed_at: Option<String> = mig.get("installed_at");
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
                    installed_at.unwrap_or_else(|| "N/A".to_string())
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
