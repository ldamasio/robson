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

#[cfg(feature = "postgres")]
use robson_exec::venue::{AccountKind, AccountPort as _};
#[cfg(feature = "postgres")]
use robsond::db_session::{self, SessionBounds};
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
        .with(robsond::operation_observation::layer())
        .with(fmt::layer().with_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("robsond=info")),
        ))
        .init();

    robsond::operation_observation::initialize_metrics();

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
                connected = connect_database(database_url, parts.venue.capabilities().account_kind) => connected?,
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
///
/// A balance account's pool starts its sessions with server-side bounds and
/// the daemon checks they are in effect before anything else runs on it
/// (ADR-0056 step 8); a position account's pool is built as it always was.
#[cfg(feature = "postgres")]
async fn connect_database(
    database_url: &str,
    account_kind: AccountKind,
) -> anyhow::Result<sqlx::PgPool> {
    connect_database_with(
        database_url,
        account_kind,
        SessionBounds::BALANCE,
        SessionBounds::BALANCE,
    )
    .await
}

/// [`connect_database`] with the bounds applied to the pool kept apart from the
/// bounds its self-check expects, so a test can make them differ; production
/// passes the same values to both.
#[cfg(feature = "postgres")]
async fn connect_database_with(
    database_url: &str,
    account_kind: AccountKind,
    applied: SessionBounds,
    expected: SessionBounds,
) -> anyhow::Result<sqlx::PgPool> {
    const MAX_ATTEMPTS: u32 = 7;
    const MAX_BACKOFF_SECS: u64 = 60;
    let mut backoff_secs: u64 = 1;
    let mut attempt: u32 = 0;
    let pool = loop {
        attempt += 1;
        match db_session::connect_daemon_pool_with(database_url, account_kind, applied).await {
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
    if account_kind == AccountKind::BalanceAccount {
        if let Err(error) = db_session::verify_session_bounds(&pool, expected).await {
            tracing::error!(%error, "Refusing to start: database session bounds not in effect");
            return Err(error.into());
        }
    }
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

#[cfg(all(test, feature = "postgres"))]
mod session_bounds_tests {
    use super::*;

    /// The `sqlx::test` database, addressed through `DATABASE_URL`.
    fn url(pool: &sqlx::PgPool) -> String {
        let database = pool.connect_options().get_database().unwrap().to_string();
        let base = std::env::var("DATABASE_URL").unwrap();
        format!("{}/{database}", base.rsplit_once('/').unwrap().0)
    }

    /// `work_mem` and the three bounds, as the pool's sessions see them.
    async fn settings(pool: &sqlx::PgPool) -> (String, String, String, String) {
        sqlx::query_as(
            "SELECT current_setting('work_mem'), current_setting('lock_timeout'), \
             current_setting('statement_timeout'), \
             current_setting('idle_in_transaction_session_timeout')",
        )
        .fetch_one(pool)
        .await
        .unwrap()
    }

    /// T1 and T1-USD-M through the daemon's own wiring on the database at
    /// `plain`, as a plain URL and as one that already carries Startup options
    /// (one of them a bound), with whatever `PGOPTIONS` this process started
    /// with. A balance account's root, journal and alarm pools keep the
    /// existing options and carry the bounds, the alarm with its own statement
    /// bound; a position account's root pool reports what a direct
    /// `PgPool::connect` reports. `work_mem` is what `PGOPTIONS` set, if any.
    async fn every_pool_through_the_wiring(plain: String, work_mem: Option<&str>) {
        let with_options = format!("{plain}?options=-c%20work_mem%3D8MB%20-c%20lock_timeout%3D3s");
        for (case, url) in [("plain URL", plain), ("URL options", with_options)] {
            let direct_pool = sqlx::PgPool::connect(&url).await.unwrap();
            let direct = settings(&direct_pool).await;
            let position = connect_database(&url, AccountKind::PositionAccount).await.unwrap();
            assert_eq!(settings(&position).await, direct, "{case}: position account");
            match (case, work_mem) {
                ("URL options", _) => {
                    assert_eq!((direct.0.as_str(), direct.1.as_str()), ("8MB", "3s"), "{case}")
                },
                (_, Some(work_mem)) => assert_eq!(direct.0, work_mem, "PGOPTIONS took effect"),
                _ => {},
            }
            let bounded = |statement: &str| {
                (direct.0.clone(), "2s".to_string(), statement.to_string(), "5s".to_string())
            };
            let root = connect_database(&url, AccountKind::BalanceAccount).await.unwrap();
            assert_eq!(settings(&root).await, bounded("5s"), "{case}: root pool");
            let journal =
                robsond::order_journal_store::PgJournalBackend::new(&root, uuid::Uuid::now_v7());
            assert_eq!(settings(journal.session_pool()).await, bounded("5s"), "{case}: journal");
            let alarm = robsond::budget_basis_alarm::BudgetBasisAlarm::new(&root);
            assert_eq!(settings(alarm.session_pool()).await, bounded("10s"), "{case}: alarm");
            for opened in [direct_pool, position, root] {
                opened.close().await;
            }
        }
    }

    #[sqlx::test(migrations = "../migrations")]
    #[ignore = "requires DATABASE_URL"]
    async fn every_daemon_pool_carries_the_bounds_through_the_wiring(pool: sqlx::PgPool) {
        every_pool_through_the_wiring(url(&pool), None).await;
    }

    /// The parent's database, given only to the child process of
    /// [`pgoptions_are_kept_through_the_wiring_in_a_process_of_their_own`].
    const CHILD_DATABASE_URL: &str = "ROBSON_SESSION_BOUNDS_CHILD_DATABASE_URL";
    const CHILD_PROVED: &str = "session-bounds child: PGOPTIONS proof ran";

    /// The child half of the `PGOPTIONS` proof. It proves something only in the
    /// process that test starts, with `PGOPTIONS` and the parent's database in
    /// its environment from birth; anywhere else it returns at once, and the
    /// parent accepts only the marker it prints after the proof.
    #[tokio::test]
    #[ignore = "child of pgoptions_are_kept_through_the_wiring_in_a_process_of_their_own"]
    async fn pgoptions_child() {
        let Ok(url) = std::env::var(CHILD_DATABASE_URL) else {
            return;
        };
        every_pool_through_the_wiring(url, Some("16MB")).await;
        println!("{CHILD_PROVED}");
    }

    /// The same proof with `PGOPTIONS`, in a child process of this test binary
    /// started with it: this process's environment is never changed, so no
    /// test running beside it sees the setting.
    #[sqlx::test(migrations = "../migrations")]
    #[ignore = "requires DATABASE_URL"]
    async fn pgoptions_are_kept_through_the_wiring_in_a_process_of_their_own(pool: sqlx::PgPool) {
        let output = tokio::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--ignored",
                "--exact",
                "session_bounds_tests::pgoptions_child",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("PGOPTIONS", "-c work_mem=16MB")
            .env(CHILD_DATABASE_URL, url(&pool))
            .output()
            .await
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success()
                && stdout.contains(CHILD_PROVED)
                && stdout.contains("test result: ok. 1 passed"),
            "child: {}\n{stdout}\n{}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    /// T7: with bounds that differ from the expected ones, the start fails on
    /// the self-check. The database has no ADR-0052 schema, so had schema
    /// readiness run first, the error would be that one instead.
    #[sqlx::test(migrations = false)]
    #[ignore = "requires DATABASE_URL"]
    async fn the_self_check_refuses_the_start_before_schema_readiness(pool: sqlx::PgPool) {
        let applied = SessionBounds {
            lock_timeout_ms: 1_000,
            ..SessionBounds::BALANCE
        };
        let error = connect_database_with(
            &url(&pool),
            AccountKind::BalanceAccount,
            applied,
            SessionBounds::BALANCE,
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(error.contains("session bounds not in effect"), "{error}");
        assert!(!error.contains("ADR-0052"), "{error}");
    }

    /// T7, the other side: matching bounds pass the self-check and the start.
    #[sqlx::test(migrations = "../migrations")]
    #[ignore = "requires DATABASE_URL"]
    async fn matching_bounds_pass_the_self_check(pool: sqlx::PgPool) {
        connect_database_with(
            &url(&pool),
            AccountKind::BalanceAccount,
            SessionBounds::BALANCE,
            SessionBounds::BALANCE,
        )
        .await
        .unwrap();
    }
}
