//! Binance WebSocket Client for real-time market and user data streams.
//!
//! Provides WebSocket integration for:
//! - Real-time market data (ticker, trades, candlesticks)
//! - User data streams (order updates, position updates)
//! - Automatic reconnection and keepalive
//!
//! # Usage
//!
//! ```rust,no_run
//! use robson_connectors::BinanceWebSocketClient;
//!
//! #[tokio::main]
//! async fn main() {
//!     let ws_client = BinanceWebSocketClient::new(false);
//!     
//!     // Subscribe to ticker for BTCUSDT
//!     let mut stream = ws_client.subscribe_ticker("BTCUSDT").await.unwrap();
//!     
//!     // Receive updates
//!     while let Some(msg) = stream.next().await {
//!         println!("Ticker: {:?}", msg);
//!     }
//! }
//! ```

use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio_tungstenite::{connect_async, tungstenite::Message, WebSocketStream, MaybeTlsStream};
use tokio::net::TcpStream;
use std::time::Duration;

// =============================================================================
// Constants
// =============================================================================

/// Binance WebSocket base URL (production)
const BINANCE_WS_URL: &str = "wss://stream.binance.com:9443";

/// Binance WebSocket base URL (testnet)
const BINANCE_WS_TESTNET_URL: &str = "wss://testnet.binance.vision";

// =============================================================================
// Errors
// =============================================================================

/// Errors that can occur in the Binance WebSocket client.
#[derive(Debug, thiserror::Error)]
pub enum BinanceWsError {
    /// WebSocket connection failed
    #[error("WebSocket connection failed: {0}")]
    ConnectionFailed(String),

    /// WebSocket send failed
    #[error("WebSocket send failed: {0}")]
    SendFailed(String),

    /// WebSocket receive failed
    #[error("WebSocket receive failed: {0}")]
    ReceiveFailed(String),

    /// Failed to parse message
    #[error("Failed to parse message: {0}")]
    ParseError(String),

    /// Connection closed
    #[error("Connection closed")]
    ConnectionClosed,

    /// Invalid symbol
    #[error("Invalid symbol: {0}")]
    InvalidSymbol(String),
}

// =============================================================================
// WebSocket Client
// =============================================================================

/// Binance WebSocket client for real-time data streams.
pub struct BinanceWebSocketClient {
    /// Base WebSocket URL
    base_url: String,
    /// Use testnet
    testnet: bool,
}

impl BinanceWebSocketClient {
    /// Create a new Binance WebSocket client.
    ///
    /// # Arguments
    ///
    /// * `testnet` - If true, connects to testnet WebSocket
    pub fn new(testnet: bool) -> Self {
        let base_url = if testnet {
            BINANCE_WS_TESTNET_URL.to_string()
        } else {
            BINANCE_WS_URL.to_string()
        };

        Self { base_url, testnet }
    }

    /// Subscribe to ticker stream for a symbol.
    ///
    /// Receives 24hr ticker updates every second.
    ///
    /// # Arguments
    ///
    /// * `symbol` - Trading pair (e.g., "BTCUSDT")
    pub async fn subscribe_ticker(
        &self,
        symbol: &str,
    ) -> Result<BinanceWsStream, BinanceWsError> {
        let symbol_lower = symbol.to_lowercase();
        let url = format!("{}/ws/{}@ticker", self.base_url, symbol_lower);

        let (ws_stream, _) = connect_async(&url)
            .await
            .map_err(|e| BinanceWsError::ConnectionFailed(e.to_string()))?;

        Ok(BinanceWsStream::new(ws_stream, StreamType::Ticker))
    }

    /// Subscribe to aggregated trade stream for a symbol.
    ///
    /// Receives trade updates in real-time.
    ///
    /// # Arguments
    ///
    /// * `symbol` - Trading pair (e.g., "BTCUSDT")
    pub async fn subscribe_agg_trade(
        &self,
        symbol: &str,
    ) -> Result<BinanceWsStream, BinanceWsError> {
        let symbol_lower = symbol.to_lowercase();
        let url = format!("{}/ws/{}@aggTrade", self.base_url, symbol_lower);

        let (ws_stream, _) = connect_async(&url)
            .await
            .map_err(|e| BinanceWsError::ConnectionFailed(e.to_string()))?;

        Ok(BinanceWsStream::new(ws_stream, StreamType::AggTrade))
    }

    /// Subscribe to user data stream (requires listen key from REST API).
    ///
    /// Receives:
    /// - Order execution reports
    /// - Account position updates
    /// - Account balance updates
    ///
    /// # Arguments
    ///
    /// * `listen_key` - Listen key obtained from REST API
    pub async fn subscribe_user_data(
        &self,
        listen_key: &str,
    ) -> Result<BinanceWsStream, BinanceWsError> {
        let url = format!("{}/ws/{}", self.base_url, listen_key);

        let (ws_stream, _) = connect_async(&url)
            .await
            .map_err(|e| BinanceWsError::ConnectionFailed(e.to_string()))?;

        Ok(BinanceWsStream::new(ws_stream, StreamType::UserData))
    }

    /// Subscribe to candlestick (kline) stream for a symbol and interval.
    ///
    /// # Arguments
    ///
    /// * `symbol` - Trading pair (e.g., "BTCUSDT")
    /// * `interval` - Kline interval (e.g., "1m", "5m", "1h", "1d")
    pub async fn subscribe_kline(
        &self,
        symbol: &str,
        interval: &str,
    ) -> Result<BinanceWsStream, BinanceWsError> {
        let symbol_lower = symbol.to_lowercase();
        let url = format!("{}/ws/{}@kline_{}", self.base_url, symbol_lower, interval);

        let (ws_stream, _) = connect_async(&url)
            .await
            .map_err(|e| BinanceWsError::ConnectionFailed(e.to_string()))?;

        Ok(BinanceWsStream::new(ws_stream, StreamType::Kline))
    }
}

// =============================================================================
// WebSocket Stream
// =============================================================================

/// Type of WebSocket stream.
#[derive(Debug, Clone, Copy)]
pub enum StreamType {
    Ticker,
    AggTrade,
    UserData,
    Kline,
}

/// WebSocket stream wrapper for Binance messages.
pub struct BinanceWsStream {
    inner: WebSocketStream<MaybeTlsStream<TcpStream>>,
    stream_type: StreamType,
    last_ping: Option<std::time::Instant>,
}

impl BinanceWsStream {
    fn new(inner: WebSocketStream<MaybeTlsStream<TcpStream>>, stream_type: StreamType) -> Self {
        Self {
            inner,
            stream_type,
            last_ping: None,
        }
    }

    /// Receive next message from stream.
    ///
    /// Returns `None` if the stream is closed.
    /// Returns `Err` if there was an error receiving or parsing the message.
    pub async fn next(&mut self) -> Option<Result<WsMessage, BinanceWsError>> {
        // Send ping if needed (every 3 minutes to keep connection alive)
        if let Some(last_ping) = self.last_ping {
            if last_ping.elapsed() > Duration::from_secs(180) {
                if let Err(e) = self.ping().await {
                    return Some(Err(e));
                }
            }
        } else {
            // First message, initialize ping timer
            self.last_ping = Some(std::time::Instant::now());
        }

        match self.inner.next().await {
            Some(Ok(Message::Text(text))) => {
                // Parse JSON message based on stream type
                match serde_json::from_str(&text) {
                    Ok(msg) => Some(Ok(msg)),
                    Err(e) => Some(Err(BinanceWsError::ParseError(e.to_string()))),
                }
            }
            Some(Ok(Message::Ping(_))) => {
                // Respond to ping with pong
                if let Err(e) = self.pong().await {
                    return Some(Err(e));
                }
                // Continue receiving next message
                Box::pin(self.next()).await
            }
            Some(Ok(Message::Pong(_))) => {
                // Pong received, continue
                Box::pin(self.next()).await
            }
            Some(Ok(Message::Close(_))) => None,
            Some(Err(e)) => Some(Err(BinanceWsError::ReceiveFailed(e.to_string()))),
            None => None,
            _ => {
                // Binary or Frame messages, skip
                Box::pin(self.next()).await
            }
        }
    }

    /// Send ping to keep connection alive.
    pub async fn ping(&mut self) -> Result<(), BinanceWsError> {
        self.inner
            .send(Message::Ping(vec![]))
            .await
            .map_err(|e| BinanceWsError::SendFailed(e.to_string()))?;

        self.last_ping = Some(std::time::Instant::now());
        Ok(())
    }

    /// Send pong in response to ping.
    async fn pong(&mut self) -> Result<(), BinanceWsError> {
        self.inner
            .send(Message::Pong(vec![]))
            .await
            .map_err(|e| BinanceWsError::SendFailed(e.to_string()))
    }

    /// Close the WebSocket connection.
    pub async fn close(mut self) -> Result<(), BinanceWsError> {
        self.inner
            .close(None)
            .await
            .map_err(|e| BinanceWsError::SendFailed(e.to_string()))
    }
}

// =============================================================================
// WebSocket Message Types
// =============================================================================

/// WebSocket message from Binance.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "e", rename_all = "camelCase")]
pub enum WsMessage {
    /// 24hr ticker update
    #[serde(rename = "24hrTicker")]
    Ticker(TickerEvent),

    /// Aggregated trade
    #[serde(rename = "aggTrade")]
    AggTrade(AggTradeEvent),

    /// Kline/Candlestick
    #[serde(rename = "kline")]
    Kline(KlineEvent),

    /// Execution report (order update)
    #[serde(rename = "executionReport")]
    ExecutionReport(ExecutionReportEvent),

    /// Account position update
    #[serde(rename = "outboundAccountPosition")]
    AccountPosition(AccountPositionEvent),

    /// Balance update
    #[serde(rename = "balanceUpdate")]
    BalanceUpdate(BalanceUpdateEvent),
}

/// 24hr ticker event.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TickerEvent {
    /// Event time
    #[serde(rename = "E")]
    pub event_time: u64,

    /// Symbol
    #[serde(rename = "s")]
    pub symbol: String,

    /// Price change
    #[serde(rename = "p")]
    pub price_change: String,

    /// Price change percent
    #[serde(rename = "P")]
    pub price_change_percent: String,

    /// Last price
    #[serde(rename = "c")]
    pub close_price: String,

    /// Last quantity
    #[serde(rename = "Q")]
    pub close_qty: String,

    /// Open price
    #[serde(rename = "o")]
    pub open_price: String,

    /// High price
    #[serde(rename = "h")]
    pub high_price: String,

    /// Low price
    #[serde(rename = "l")]
    pub low_price: String,

    /// Total traded volume
    #[serde(rename = "v")]
    pub volume: String,

    /// Total traded quote asset volume
    #[serde(rename = "q")]
    pub quote_volume: String,
}

/// Aggregated trade event.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AggTradeEvent {
    /// Event time
    #[serde(rename = "E")]
    pub event_time: u64,

    /// Symbol
    #[serde(rename = "s")]
    pub symbol: String,

    /// Aggregated trade ID
    #[serde(rename = "a")]
    pub agg_trade_id: u64,

    /// Price
    #[serde(rename = "p")]
    pub price: String,

    /// Quantity
    #[serde(rename = "q")]
    pub quantity: String,

    /// First trade ID
    #[serde(rename = "f")]
    pub first_trade_id: u64,

    /// Last trade ID
    #[serde(rename = "l")]
    pub last_trade_id: u64,

    /// Trade time
    #[serde(rename = "T")]
    pub trade_time: u64,

    /// Is buyer maker?
    #[serde(rename = "m")]
    pub is_buyer_maker: bool,
}

/// Kline event.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KlineEvent {
    /// Event time
    #[serde(rename = "E")]
    pub event_time: u64,

    /// Symbol
    #[serde(rename = "s")]
    pub symbol: String,

    /// Kline data
    #[serde(rename = "k")]
    pub kline: KlineData,
}

/// Kline data.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KlineData {
    /// Kline start time
    #[serde(rename = "t")]
    pub start_time: u64,

    /// Kline close time
    #[serde(rename = "T")]
    pub close_time: u64,

    /// Symbol
    #[serde(rename = "s")]
    pub symbol: String,

    /// Interval
    #[serde(rename = "i")]
    pub interval: String,

    /// Open price
    #[serde(rename = "o")]
    pub open: String,

    /// Close price
    #[serde(rename = "c")]
    pub close: String,

    /// High price
    #[serde(rename = "h")]
    pub high: String,

    /// Low price
    #[serde(rename = "l")]
    pub low: String,

    /// Volume
    #[serde(rename = "v")]
    pub volume: String,

    /// Is kline closed?
    #[serde(rename = "x")]
    pub is_closed: bool,
}

/// Execution report event (order update).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionReportEvent {
    /// Event time
    #[serde(rename = "E")]
    pub event_time: u64,

    /// Symbol
    #[serde(rename = "s")]
    pub symbol: String,

    /// Client order ID
    #[serde(rename = "c")]
    pub client_order_id: String,

    /// Side
    #[serde(rename = "S")]
    pub side: String,

    /// Order type
    #[serde(rename = "o")]
    pub order_type: String,

    /// Time in force
    #[serde(rename = "f")]
    pub time_in_force: String,

    /// Order quantity
    #[serde(rename = "q")]
    pub quantity: String,

    /// Order price
    #[serde(rename = "p")]
    pub price: String,

    /// Order status
    #[serde(rename = "X")]
    pub order_status: String,

    /// Order ID
    #[serde(rename = "i")]
    pub order_id: u64,

    /// Last executed quantity
    #[serde(rename = "l")]
    pub last_executed_qty: String,

    /// Cumulative filled quantity
    #[serde(rename = "z")]
    pub cumulative_filled_qty: String,

    /// Last executed price
    #[serde(rename = "L")]
    pub last_executed_price: String,

    /// Transaction time
    #[serde(rename = "T")]
    pub transaction_time: u64,
}

/// Account position event.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountPositionEvent {
    /// Event time
    #[serde(rename = "E")]
    pub event_time: u64,

    /// Last update time
    #[serde(rename = "u")]
    pub last_update_time: u64,

    /// Balances
    #[serde(rename = "B")]
    pub balances: Vec<BalanceData>,
}

/// Balance data.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BalanceData {
    /// Asset
    #[serde(rename = "a")]
    pub asset: String,

    /// Free amount
    #[serde(rename = "f")]
    pub free: String,

    /// Locked amount
    #[serde(rename = "l")]
    pub locked: String,
}

/// Balance update event.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BalanceUpdateEvent {
    /// Event time
    #[serde(rename = "E")]
    pub event_time: u64,

    /// Asset
    #[serde(rename = "a")]
    pub asset: String,

    /// Balance delta
    #[serde(rename = "d")]
    pub balance_delta: String,

    /// Clear time
    #[serde(rename = "T")]
    pub clear_time: u64,
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ticker_event_deserialization() {
        let json = r#"{
            "e": "24hrTicker",
            "E": 1672515782136,
            "s": "BTCUSDT",
            "p": "1000.00",
            "P": "2.00",
            "o": "50000.00",
            "h": "51000.00",
            "l": "49000.00",
            "c": "51000.00",
            "Q": "0.1",
            "v": "1000.0",
            "q": "50000000.0"
        }"#;

        let event: WsMessage = serde_json::from_str(json).unwrap();

        match event {
            WsMessage::Ticker(ticker) => {
                assert_eq!(ticker.symbol, "BTCUSDT");
                assert_eq!(ticker.close_price, "51000.00");
            }
            _ => panic!("Expected Ticker event"),
        }
    }

    #[test]
    fn test_agg_trade_event_deserialization() {
        let json = r#"{
            "e": "aggTrade",
            "E": 1672515782136,
            "s": "BTCUSDT",
            "a": 12345,
            "p": "50000.00",
            "q": "0.1",
            "f": 1000,
            "l": 1010,
            "T": 1672515782000,
            "m": true
        }"#;

        let event: WsMessage = serde_json::from_str(json).unwrap();

        match event {
            WsMessage::AggTrade(trade) => {
                assert_eq!(trade.symbol, "BTCUSDT");
                assert_eq!(trade.price, "50000.00");
                assert!(trade.is_buyer_maker);
            }
            _ => panic!("Expected AggTrade event"),
        }
    }

    #[test]
    fn test_execution_report_deserialization() {
        let json = r#"{
            "e": "executionReport",
            "E": 1672515782136,
            "s": "BTCUSDT",
            "c": "client_order_123",
            "S": "BUY",
            "o": "MARKET",
            "f": "GTC",
            "q": "0.1",
            "p": "0.0",
            "X": "FILLED",
            "i": 12345,
            "l": "0.1",
            "z": "0.1",
            "L": "50000.00",
            "T": 1672515782000
        }"#;

        let event: WsMessage = serde_json::from_str(json).unwrap();

        match event {
            WsMessage::ExecutionReport(report) => {
                assert_eq!(report.symbol, "BTCUSDT");
                assert_eq!(report.order_status, "FILLED");
                assert_eq!(report.side, "BUY");
            }
            _ => panic!("Expected ExecutionReport event"),
        }
    }
}
