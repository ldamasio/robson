//! Robson v2 Exchange Connectors
//!
//! Adapters for exchange APIs (REST + WebSocket).
//! Normalizes exchange-specific types to domain types.

#![warn(clippy::all)]

// Public modules
pub mod binance_rest;
pub mod binance_ws;

// Re-exports
pub use binance_rest::{
    BinanceOrderResponse, BinanceRestClient, BinanceRestError, IsolatedMarginAccount,
    IsolatedMarginAsset, IsolatedMarginPosition,
};
pub use binance_ws::{
    AggTradeEvent, BalanceUpdateEvent, BinanceWebSocketClient, BinanceWsError, BinanceWsStream,
    ExecutionReportEvent, KlineEvent, StreamType, TickerEvent, WsMessage,
};

#[cfg(test)]
mod tests {
    #[test]
    fn it_works() {
        assert_eq!(2 + 2, 4);
    }
}
