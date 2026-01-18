//! Robson v2 Daemon
//!
//! Runtime orchestrator for engine, execution, and API server.
//!
//! # Usage
//!
//! ```bash
//! # Start daemon with default configuration
//! robsond
//!
//! # Database migrations
//! robsond db migrate
//! robsond db status
//! robsond db init [--tenant-id UUID] [--account-id UUID]
//!
//! # Start with custom environment
//! ROBSON_ENV=test ROBSON_API_PORT=8081 robsond
//! ```
//!
//! # Environment Variables
//!
//! - `DATABASE_URL`: PostgreSQL connection string (required for db commands)
//! - `ROBSON_ENV`: Environment (test, development, production)
//! - `ROBSON_API_HOST`: API host (default: 0.0.0.0)
//! - `ROBSON_API_PORT`: API port (default: 8080)
//! - `ROBSON_DEFAULT_RISK_PERCENT`: Default risk (default: 0.01)
//! - `ROBSON_MIN_TECH_STOP_PERCENT`: Min tech stop (default: 0.001)
//! - `ROBSON_MAX_TECH_STOP_PERCENT`: Max tech stop (default: 0.10)

mod db;

use db::run_db_command;
use robsond::{Config, Daemon};
use std::sync::Arc;
use tracing::info;
use tracing_subscriber::{EnvFilter, fmt, prelude::*};

// Optional PostgreSQL dependencies (only available with postgres feature)
#[cfg(feature = "postgres")]
use robson_store::PgProjectionReader;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing
    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(EnvFilter::from_default_env().add_directive("robsond=info".parse()?))
        .init();

    // Parse CLI arguments
    let args: Vec<String> = std::env::args().collect();

    // Check for db subcommand
    if args.len() > 1 && args[1] == "db" {
        return run_db_command(args).await;
    }

    // Default: run daemon
    let config = Config::from_env()?;

    info!(
        version = env!("CARGO_PKG_VERSION"),
        environment = %config.environment,
        api_host = %config.api.host,
        api_port = config.api.port,
        "Robson v2 Daemon"
    );

    // Create daemon with or without PostgreSQL projection recovery
    #[cfg(feature = "postgres")]
    {
        // If DATABASE_URL is set and tenant_id is configured, create PgPool and projection recovery
        if let (Some(database_url), Some(tenant_id)) = (
            &config.projection.database_url,
            config.projection.tenant_id,
        ) {
            info!(
                %tenant_id,
                "PostgreSQL configured, enabling projection recovery"
            );

            // Create PostgreSQL connection pool
            let pool = sqlx::PgPool::connect(database_url).await?;

            // Create projection recovery adapter
            let projection_recovery = Some(Arc::new(PgProjectionReader::new(Arc::new(pool))) as Arc<dyn robson_store::ProjectionRecovery>);

            // Create daemon with projection recovery
            let daemon = Daemon::new_stub_with_projection(config, projection_recovery);
            daemon.run().await?;
        } else {
            info!("No DATABASE_URL or PROJECTION_TENANT_ID configured, using stub daemon");
            let daemon = Daemon::new_stub(config);
            daemon.run().await?;
        }
    }

    #[cfg(not(feature = "postgres"))]
    {
        // Without postgres feature, always use stub daemon
        let daemon = Daemon::new_stub(config);
        daemon.run().await?;
    }

    Ok(())
}
