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
//! ```
//!
//! # Components
//!
//! - **Daemon**: Main runtime orchestrator
//! - **Position Manager**: Manages position lifecycle and detector tasks
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
pub mod error;
pub mod event_bus;
pub mod market_data;
pub mod position_manager;

// Re-exports for convenience
pub use config::{ApiConfig, Config, EngineConfig, Environment};
pub use daemon::Daemon;
pub use error::{DaemonError, DaemonResult};
pub use event_bus::{DaemonEvent, EventBus, EventReceiver, MarketData, OrderFill};
pub use position_manager::PositionManager;
