//! Binance USDⓈ-M futures adapters implementing the `robson-exec` ports.
//!
//! Relocated from `robsond` unchanged (ADR-0056 step 1).

pub mod exchange;
pub mod ohlcv;

pub use exchange::{normalize_market_quantity, BinanceExchangeAdapter};
pub use ohlcv::BinanceOhlcvAdapter;
