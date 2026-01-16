//! Robson v2 Execution Layer
//!
//! Idempotent order execution with intent journal.
//!
//! # Architecture
//!
//! ```text
//! Engine Decision → Executor → Intent Journal → Exchange → Result
//! ```
//!
//! # Components
//!
//! - **Ports**: Traits defining interfaces for exchange and market data
//! - **Intent Journal**: Ensures idempotent execution (at-most-once semantics)
//! - **Executor**: Orchestrates engine actions to exchange operations
//! - **Stub**: Test implementations for development
//!
//! # Example
//!
//! ```rust,ignore
//! use robson_exec::{Executor, IntentJournal, StubExchange};
//! use robson_store::MemoryStore;
//! use std::sync::Arc;
//!
//! // Create components
//! let exchange = Arc::new(StubExchange::new(dec!(95000)));
//! let journal = Arc::new(IntentJournal::new());
//! let store = Arc::new(MemoryStore::new());
//!
//! // Create executor
//! let executor = Executor::new(exchange, journal, store);
//!
//! // Execute engine actions
//! let results = executor.execute(actions).await?;
//! ```

#![warn(clippy::all)]

pub mod error;
pub mod executor;
pub mod intent;
pub mod ports;
pub mod stub;

// Re-exports for convenience
pub use error::{ExecError, ExecResult};
pub use executor::{ActionResult, Executor};
pub use intent::{Intent, IntentAction, IntentJournal, IntentResult, IntentStatus};
pub use ports::{ExchangePort, MarketDataPort, OrderResult, PriceUpdate};
pub use stub::{StubExchange, StubMarketData};
