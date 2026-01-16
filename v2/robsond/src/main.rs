//! Robson v2 Daemon
//!
//! Runtime orchestrator for engine, execution, and API server.
//!
//! # Usage
//!
//! ```bash
//! # Start with default configuration
//! cargo run -p robsond
//!
//! # Start with custom environment
//! ROBSON_ENV=test ROBSON_API_PORT=8081 cargo run -p robsond
//! ```
//!
//! # Environment Variables
//!
//! - `ROBSON_ENV`: Environment (test, development, production)
//! - `ROBSON_API_HOST`: API host (default: 0.0.0.0)
//! - `ROBSON_API_PORT`: API port (default: 8080)
//! - `ROBSON_DEFAULT_RISK_PERCENT`: Default risk (default: 0.01)
//! - `ROBSON_MIN_TECH_STOP_PERCENT`: Min tech stop (default: 0.001)
//! - `ROBSON_MAX_TECH_STOP_PERCENT`: Max tech stop (default: 0.10)

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

    // Load configuration
    let config = Config::from_env()?;

    info!(
        version = env!("CARGO_PKG_VERSION"),
        environment = %config.environment,
        api_host = %config.api.host,
        api_port = config.api.port,
        "Robson v2 Daemon"
    );

    // Create and run daemon
    let daemon = Daemon::new_stub(config);
    daemon.run().await?;

    Ok(())
}
