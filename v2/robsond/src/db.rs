//! Database CLI subcommands for robsond.
//!
//! Provides `db migrate`, `db status`, and `db init` commands.

use anyhow::{Result, anyhow};
use std::env;
use tracing::info;

use robson_db::{init_minimal_data, migrate, status};

/// Run database CLI subcommands.
///
/// Supported commands:
/// - `robsond db migrate` - Run pending migrations
/// - `robsond db status` - Check migration status
/// - `robsond db init [--tenant-id UUID] [--account-id UUID]` - Seed minimal data
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
        },
        _ => {
            return Err(anyhow!("Unknown db command: {}. Use migrate, status, or init", args[2]));
        },
    }

    Ok(())
}
