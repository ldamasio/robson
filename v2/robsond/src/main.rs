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

#[cfg(feature = "postgres")]
mod db;

use std::sync::Arc;

use robson_connectors::BinanceRestClient;
use robsond::{Config, Daemon};
use tracing::info;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing
    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(EnvFilter::from_default_env().add_directive("robsond=info".parse()?))
        .init();

    // Check for db subcommand
    #[cfg(feature = "postgres")]
    {
        let args: Vec<String> = std::env::args().collect();
        if args.len() > 1 && args[1] == "db" {
            use db::run_db_command;
            return run_db_command(args).await;
        }
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

    // Select exchange adapter based on Binance credentials and environment marker.
    // Reuses position_monitor.binance_api_key/secret from config (same env vars).
    // ROBSON_BINANCE_USE_TESTNET is a birth-time environment marker set in the
    // ConfigMap — never a runtime toggle. See ADR-0003.
    let has_binance_creds = config.position_monitor.binance_api_key.is_some()
        && config.position_monitor.binance_api_secret.is_some();
    let use_testnet = std::env::var("ROBSON_BINANCE_USE_TESTNET").unwrap_or_default() == "true";

    // Create daemon with optional projection recovery (wiring layer)
    #[cfg(feature = "postgres")]
    {
        // If DATABASE_URL is set, create shared PgPool (used by both recovery and
        // worker)
        let (projection_recovery, pg_pool) = if let (Some(database_url), Some(tenant_id)) =
            (&config.projection.database_url, config.projection.tenant_id)
        {
            info!(%tenant_id, "PostgreSQL configured, creating shared connection pool");

            // Retry initial DB connect with exponential backoff so that transient DNS
            // failures during pod startup do not cause an immediate exitCode=1.
            // Kubernetes restarts are a last resort, not the normal retry mechanism.
            // Cap: ~2 minutes total (1+2+4+8+16+32+60 = 123 s across 7 attempts).
            let pool = {
                const MAX_ATTEMPTS: u32 = 7;
                const MAX_BACKOFF_SECS: u64 = 60;
                let mut backoff_secs: u64 = 1;
                let mut attempt: u32 = 0;
                loop {
                    attempt += 1;
                    match sqlx::PgPool::connect(database_url).await {
                        Ok(p) => {
                            if attempt > 1 {
                                info!(attempt, "PostgreSQL connection established after retry");
                            }
                            break p;
                        },
                        Err(e) if attempt >= MAX_ATTEMPTS => {
                            tracing::error!(
                                attempt,
                                error = %e,
                                "PostgreSQL connection failed after retries"
                            );
                            return Err(e.into());
                        },
                        Err(e) => {
                            tracing::warn!(
                                attempt,
                                retry_in_secs = backoff_secs,
                                error = %e,
                                "PostgreSQL connection failed, retrying"
                            );
                            tokio::time::sleep(tokio::time::Duration::from_secs(backoff_secs))
                                .await;
                            backoff_secs = (backoff_secs * 2).min(MAX_BACKOFF_SECS);
                        },
                    }
                }
            };
            let pool = Arc::new(pool);

            // Create projection recovery adapter (uses same pool)
            let recovery = Some(Arc::new(robson_store::PgProjectionReader::new(pool.clone()))
                as Arc<dyn robson_store::ProjectionRecovery>);

            (recovery, Some(pool))
        } else {
            (None, None)
        };

        if has_binance_creds && use_testnet {
            info!("Exchange: Binance (testnet)");
            let (api_key, api_secret) = (
                config.position_monitor.binance_api_key.clone().unwrap(),
                config.position_monitor.binance_api_secret.clone().unwrap(),
            );
            let client = Arc::new(BinanceRestClient::testnet(api_key, api_secret));
            let daemon =
                Daemon::new_binance_with_recovery(config, client, projection_recovery, pg_pool);
            daemon.run().await?;
        } else {
            if has_binance_creds && !use_testnet {
                tracing::error!(
                    "Binance credentials present but ROBSON_BINANCE_USE_TESTNET is not set. \
                     Refusing to connect to Binance production. Falling back to StubExchange. \
                     Set ROBSON_BINANCE_USE_TESTNET=true to enable testnet, or remove credentials."
                );
            } else {
                info!("Exchange: Stub (no Binance credentials)");
            }
            let daemon = Daemon::new_stub_with_recovery(config, projection_recovery, pg_pool);
            daemon.run().await?;
        }
    }

    #[cfg(not(feature = "postgres"))]
    {
        if has_binance_creds && use_testnet {
            info!("Exchange: Binance (testnet)");
            let (api_key, api_secret) = (
                config.position_monitor.binance_api_key.clone().unwrap(),
                config.position_monitor.binance_api_secret.clone().unwrap(),
            );
            let client = Arc::new(BinanceRestClient::testnet(api_key, api_secret));
            let daemon = Daemon::new_binance(config, client);
            daemon.run().await?;
        } else {
            if has_binance_creds && !use_testnet {
                tracing::error!(
                    "Binance credentials present but ROBSON_BINANCE_USE_TESTNET is not set. \
                     Refusing to connect to Binance production. Falling back to StubExchange. \
                     Set ROBSON_BINANCE_USE_TESTNET=true to enable testnet, or remove credentials."
                );
            } else {
                info!("Exchange: Stub (no Binance credentials)");
            }
            let daemon = Daemon::new_stub(config);
            daemon.run().await?;
        }
    }

    Ok(())
}
