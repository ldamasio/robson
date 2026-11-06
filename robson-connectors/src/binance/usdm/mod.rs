//! Binance USDⓈ-M futures adapters implementing the `robson-exec` ports.
//!
//! Relocated from `robsond` unchanged (ADR-0056 steps 1 and 3).

pub mod exchange;
pub mod market_data;
pub mod ohlcv;

pub use exchange::{normalize_market_quantity, BinanceExchangeAdapter};
pub use market_data::{
    BinanceUsdmTradeFeed, MAINNET_MARKET_WS_ENDPOINT, TESTNET_MARKET_WS_ENDPOINT,
};
pub use ohlcv::BinanceOhlcvAdapter;
