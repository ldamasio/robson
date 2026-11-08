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
//! ```text
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

mod compat;
#[cfg(feature = "crash-points")]
pub mod crash_points;
pub mod error;
pub mod executor;
pub mod intent;
pub mod order_journal;
pub mod ports;
pub mod quantize;
pub mod stub;
pub mod venue;

// Re-exports for convenience
pub use error::{ExecError, ExecResult};
pub use executor::{ActionResult, Executor};
pub use intent::{Intent, IntentAction, IntentJournal, IntentResult, IntentStatus};
pub use ports::{
    CandleInterval, ExchangePort, ExchangePosition, FuturesBalance, FuturesSettings,
    MarketDataPort, OhlcvPort, OpenOrderRecord, OrderResult, PriceUpdate, SpotBalance, SpotOrder,
    SpotOrderRequest, StopCancelOutcome, TradeFeedMessage, TradeFeedPort, TradeStream, Transfer,
    TransferId, UniversalTransferType, UserTradeRecord,
};
pub use stub::{StubExchange, StubMarketData, StubOhlcv, StubTradeFeed};

/// A named place where the crash harness can SIGKILL the real process.
/// Expands to nothing unless the `crash-points` feature is on; see
/// [`crash_points`].
#[cfg(feature = "crash-points")]
#[macro_export]
macro_rules! crash_point {
    ($point:ident) => {
        $crate::crash_points::hit($crate::crash_points::$point)
    };
}

/// A named place where the crash harness can SIGKILL the real process.
/// Expands to nothing unless the `crash-points` feature is on.
#[cfg(not(feature = "crash-points"))]
#[macro_export]
macro_rules! crash_point {
    ($point:ident) => {};
}
