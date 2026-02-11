//! Robson v2 Daemon Library
//!
//! Runtime orchestrator for the Robson trading engine.
//!
//! # Architecture
//!
//! ```text
//! CLI → API Server → Position Manager → Engine → Executor → Exchange
//!                         ↑
//!                    Event Bus (signals, market data)
//!                         ↑
//!                    Detector Tasks
//!                         ↑
//!                 Position Monitor (Safety Net)
//! ```
//!
//! # Components
//!
//! - **Daemon**: Main runtime orchestrator
//! - **Position Manager**: Manages position lifecycle and detector tasks
//! - **Position Monitor**: Safety net for rogue positions (opened outside Robson v2)
//! - **Event Bus**: Internal communication (detector → engine, market data)
//! - **API**: HTTP endpoints for CLI interaction
//! - **Config**: Environment-based configuration
//!
//! # Example
//!
//! ```rust,ignore
//! use robsond::{Config, Daemon};
//!
//! #[tokio::main]
//! async fn main() {
//!     let config = Config::from_env().expect("Failed to load config");
//!     let daemon = Daemon::new_stub(config);
//!     daemon.run().await.expect("Daemon error");
//! }
//! ```

#![warn(clippy::all)]

pub mod api;
pub mod config;
pub mod daemon;
pub mod detector;
pub mod error;
pub mod event_bus;
pub mod market_data;
pub mod position_manager;
pub mod position_monitor;

#[cfg(feature = "postgres")]
pub mod projection_worker;

// Re-exports for convenience
pub use config::{ApiConfig, Config, EngineConfig, Environment, PositionMonitorConfig, ProjectionConfig};
pub use daemon::Daemon;
pub use detector::{DetectorConfig, DetectorTask};
pub use error::{DaemonError, DaemonResult};
pub use event_bus::{DaemonEvent, EventBus, EventReceiver, MarketData, OrderFill};
pub use position_manager::PositionManager;
pub use position_monitor::{MonitorError, PositionMonitor, PositionMonitorConfig as MonitorConfig};
