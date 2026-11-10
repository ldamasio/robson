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
//! robsond db repair
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
//! - `ROBSON_VENUE`: Venue (`binance_usdm`, `stub`). Unset selects from Binance
//!   credentials and environment markers
//! - `ROBSON_API_HOST`: API host (default: 0.0.0.0)
//! - `ROBSON_API_PORT`: API port (default: 8080)
//! - `ROBSON_DEFAULT_RISK_PERCENT`: Default risk (default: 0.01)
//! - `ROBSON_MIN_TECH_STOP_PERCENT`: Min tech stop (default: 0.001)
//! - `ROBSON_MAX_TECH_STOP_PERCENT`: Max tech stop (default: 0.10)

#[cfg(feature = "postgres")]
mod db;

#[cfg(feature = "postgres")]
use std::sync::Arc;

use robsond::{
    exit_code_for_daemon_error, stop::StopRequest, Config, Daemon, DaemonResult, VenueFactory,
};
use tracing::info;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

/// Convert a `DaemonResult` into an `anyhow::Result`, mapping known error
/// variants to their documented exit codes via `std::process::exit`.
///
/// The exit code policy lives entirely in `exit_code_for_daemon_error`.
/// Errors that map to exit code 1 (the default) propagate via `anyhow` so
/// that the caller can print a formatted error before exiting. Errors with
/// a non-1 code are already logged by the daemon, so we exit directly.
fn map_daemon_result(result: DaemonResult<()>) -> anyhow::Result<()> {
    match result {
        Ok(()) => Ok(()),
        Err(e) => {
            let code = exit_code_for_daemon_error(&e);
            if code != 1 {
                std::process::exit(code);
            }
            Err(e.into())
        },
    }
}

fn main() -> anyhow::Result<()> {
    #[cfg(feature = "crash-points")]
    arm_crash_harness();

    // SIGTERM and SIGINT ask for the stop from here on, before configuration
    // and the database connection (ADR-0056 step 7): as PID 1 the process sees
    // SIGTERM only once a handler is installed. The `db` commands keep the
    // default handling.
    let stop = StopRequest::default();
    let _signals = if runs_the_daemon() {
        Some(robsond::stop::listen_for_signals(&stop).map_err(anyhow::Error::msg)?)
    } else {
        None
    };

    // 8 MiB worker stacks (default 2 MiB): robsond aborted twice in
    // production (2026-06-30, 2026-07-02) with tokio-runtime-worker stack
    // overflows that leave no backtrace. Larger stacks close that crash
    // class while the offending path is still being hunted.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_stack_size(8 * 1024 * 1024)
        .build()?;
    let result = runtime.block_on(async_main(stop));
    // Teardown never outlasts the stop's deadline (ADR-0056 step 7): a task or
    // a worker thread still running then is abandoned, not waited for.
    runtime.shutdown_timeout(robsond::stop::teardown_bound());
    result
}

/// Whether this run is the daemon rather than a `db` command.
fn runs_the_daemon() -> bool {
    !(cfg!(feature = "postgres") && std::env::args().nth(1).as_deref() == Some("db"))
}

async fn async_main(stop: StopRequest) -> anyhow::Result<()> {
    // Initialize tracing
    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("robsond=info")))
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

    // Build the venue selected by ROBSON_VENUE; a selection that cannot be
    // honoured is a startup error (ADR-0056 §3).
    let parts = VenueFactory::build(&config)?;

    // Create daemon with optional projection recovery (wiring layer)
    #[cfg(feature = "postgres")]
    {
        // If DATABASE_URL is set, create shared PgPool (used by both recovery and
        // worker)
        let (projection_recovery, pg_pool) = if let (Some(database_url), Some(tenant_id)) =
            (&config.projection.database_url, config.projection.tenant_id)
        {
            info!(%tenant_id, "PostgreSQL configured, creating shared connection pool");

            // A stop asked for while connecting ends the start here, with
            // nothing sent, whether or not the database comes back.
            let pool = tokio::select! {
                biased;
                _ = stop.asked() => {
                    tracing::warn!(
                        cause = stop.cause(),
                        "Stop requested before the start completed: nothing started, no marker \
                         written"
                    );
                    return Ok(());
                },
                connected = connect_database(database_url) => connected?,
            };
            let pool = Arc::new(pool);

            // Create projection recovery adapter (uses same pool)
            let recovery = Some(Arc::new(robson_store::PgProjectionReader::new(pool.clone()))
                as Arc<dyn robson_store::ProjectionRecovery>);

            (recovery, Some(pool))
        } else {
            (None, None)
        };

        let daemon = Daemon::with_venue_and_recovery(
            config,
            parts.venue,
            parts.market_data,
            parts.ohlcv,
            projection_recovery,
            pg_pool,
        )
        .with_funding_recovery_route(parts.funding_recovery);
        map_daemon_result(daemon.run_until(stop).await)?;
    }

    #[cfg(not(feature = "postgres"))]
    {
        let daemon = Daemon::with_venue(config, parts.venue, parts.market_data, parts.ohlcv)
            .with_funding_recovery_route(parts.funding_recovery);
        map_daemon_result(daemon.run_until(stop).await)?;
    }

    Ok(())
}

/// Connect to PostgreSQL and check the schema is ready. The first connection
/// is retried with exponential backoff so that transient DNS failures during
/// pod startup do not cause an immediate exitCode=1; Kubernetes restarts are a
/// last resort, not the normal retry mechanism. Cap: about 2 minutes in all
/// (1+2+4+8+16+32+60 = 123 s across 7 attempts).
#[cfg(feature = "postgres")]
async fn connect_database(database_url: &str) -> anyhow::Result<sqlx::PgPool> {
    const MAX_ATTEMPTS: u32 = 7;
    const MAX_BACKOFF_SECS: u64 = 60;
    let mut backoff_secs: u64 = 1;
    let mut attempt: u32 = 0;
    let pool = loop {
        attempt += 1;
        match sqlx::PgPool::connect(database_url).await {
            Ok(p) => {
                if attempt > 1 {
                    info!(attempt, "PostgreSQL connection established after retry");
                }
                break p;
            },
            Err(e) if attempt >= MAX_ATTEMPTS => {
                tracing::error!(attempt, error = %e, "PostgreSQL connection failed after retries");
                return Err(e.into());
            },
            Err(e) => {
                tracing::warn!(
                    attempt,
                    retry_in_secs = backoff_secs,
                    error = %e,
                    "PostgreSQL connection failed, retrying"
                );
                tokio::time::sleep(tokio::time::Duration::from_secs(backoff_secs)).await;
                backoff_secs = (backoff_secs * 2).min(MAX_BACKOFF_SECS);
            },
        }
    };
    db::verify_adr0052_schema_readiness(&pool).await?;
    Ok(pool)
}

/// `.env` first, as `Config::from_env` loads it: a production marker that only
/// exists there must be refused before any connection is built.
#[cfg(feature = "crash-points")]
fn arm_crash_harness() {
    let _ = dotenvy::dotenv();
    robson_exec::crash_points::arm_from_env();
}
