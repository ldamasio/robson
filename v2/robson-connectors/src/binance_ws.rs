//! Binance WebSocket Market Data Client
//!
//! Connects to Binance WebSocket API for real-time market data.
//! Normalizes Binance-specific messages to canonical domain types.

use futures_util::stream::StreamExt;
use robson_domain::{MarketDataEvent, Symbol, Tick};
use rust_decimal::Decimal;
use serde_json::Value;
use std::time::Duration;
use thiserror::Error;
use tokio::sync::broadcast;
use tokio::time::timeout;
use tokio_tungstenite::{WebSocketStream, connect_async, tungstenite::Message as WebSocketMessage};
use tracing::{debug, error, info, warn};

/// Type alias for the WebSocket stream (with auto TLS).
type WsStream = WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// Binance WebSocket base URL.
const BINANCE_WS_URL: &str = "wss://stream.binance.com:9443/ws";

/// WebSocket read timeout (in seconds).
const READ_TIMEOUT_SECS: u64 = 30;

/// Reconnect delay (in seconds).
const RECONNECT_DELAY_SECS: u64 = 5;

/// Errors that can occur in the Binance WebSocket client.
#[derive(Debug, Error)]
pub enum BinanceWsError {
    /// Failed to connect to WebSocket.
    #[error("Failed to connect to WebSocket: {0}")]
    ConnectionFailed(String),

    /// Failed to send message.
    #[error("Failed to send message: {0}")]
    SendFailed(String),

    /// Failed to receive message.
    #[error("Failed to receive message: {0}")]
    ReceiveError(String),

    /// Invalid message format.
    #[error("Invalid message format: {0}")]
    InvalidMessage(String),

    /// Subscription failed.
    #[error("Subscription failed: {0}")]
    SubscriptionFailed(String),

    /// Channel closed unexpectedly.
    #[error("Channel closed unexpectedly")]
    ChannelClosed,

    /// Timed out waiting for message.
    #[error("Timed out waiting for message")]
    Timeout,
}

/// Binance WebSocket client for market data.
pub struct BinanceMarketDataClient {
    /// Symbol to subscribe to.
    symbol: Symbol,
    /// WebSocket stream (with TLS wrapper).
    ws_stream: WsStream,
    /// Sender for market data events (fan-out).
    event_sender: broadcast::Sender<MarketDataEvent>,
    /// Whether the client is connected.
    connected: bool,
}

impl BinanceMarketDataClient {
    /// Create a new Binance WebSocket client.
    ///
    /// # Arguments
    ///
    /// * `symbol` - Trading pair symbol (e.g., "BTCUSDT")
    /// * `event_sender` - Channel for sending market data events to subscribers
    pub async fn new(
        symbol: Symbol,
        event_sender: broadcast::Sender<MarketDataEvent>,
    ) -> Result<Self, BinanceWsError> {
        // Build WebSocket URL for the symbol
        let stream_name = format!("{}@trade", symbol.as_pair().to_lowercase());
        let url = format!("{}/{}", BINANCE_WS_URL, stream_name);

        info!(%url, "Connecting to Binance WebSocket");

        // Connect to WebSocket
        let (ws_stream, _) = connect_async(&url)
            .await
            .map_err(|e| BinanceWsError::ConnectionFailed(e.to_string()))?;

        info!(symbol = %symbol.as_pair(), "Connected to Binance WebSocket");

        Ok(Self {
            symbol,
            ws_stream,
            event_sender,
            connected: true,
        })
    }

    /// Run the client message loop.
    ///
    /// This method runs indefinitely, processing incoming messages
    /// and publishing market data events. Returns when the connection
    /// is closed or an error occurs.
    pub async fn run(&mut self) -> Result<(), BinanceWsError> {
        while self.connected {
            match timeout(Duration::from_secs(READ_TIMEOUT_SECS), self.next_message()).await {
                Ok(Ok(Some(msg))) => {
                    if let Err(e) = self.handle_message(msg).await {
                        error!(error = %e, "Error handling message");
                        // Continue processing other messages
                    }
                },
                Ok(Ok(None)) => {
                    warn!("WebSocket stream closed");
                    self.connected = false;
                    return Err(BinanceWsError::ChannelClosed);
                },
                Ok(Err(e)) => {
                    error!(error = %e, "Error reading from WebSocket");
                    self.connected = false;
                    return Err(e);
                },
                Err(_) => {
                    error!("Timeout waiting for message");
                    self.connected = false;
                    return Err(BinanceWsError::Timeout);
                },
            }
        }

        Ok(())
    }

    /// Read the next message from the WebSocket stream.
    async fn next_message(&mut self) -> Result<Option<WebSocketMessage>, BinanceWsError> {
        match self.ws_stream.next().await {
            Some(Ok(msg)) => Ok(Some(msg)),
            Some(Err(e)) => {
                error!(error = %e, "WebSocket error");
                Err(BinanceWsError::ReceiveError(format!("{:?}", e)))
            },
            None => {
                warn!("WebSocket stream ended");
                self.connected = false;
                Ok(None)
            },
        }
    }

    /// Reconnect to WebSocket with exponential backoff.
    async fn reconnect(&mut self) -> Result<(), BinanceWsError> {
        let stream_name = format!("{}@trade", self.symbol.as_pair().to_lowercase());
        let url = format!("{}/{}", BINANCE_WS_URL, stream_name);

        // Exponential backoff: wait before reconnect
        tokio::time::sleep(Duration::from_secs(RECONNECT_DELAY_SECS)).await;

        info!(%url, "Reconnecting to Binance WebSocket");

        // Connect to WebSocket
        let (ws_stream, _) = connect_async(&url)
            .await
            .map_err(|e| BinanceWsError::ConnectionFailed(e.to_string()))?;

        // Unwrap the TLS wrapper
        self.ws_stream = ws_stream;

        self.connected = true;
        info!(symbol = %self.symbol.as_pair(), "Reconnected to Binance WebSocket");

        Ok(())
    }

    /// Handle a single WebSocket message.
    async fn handle_message(&mut self, msg: WebSocketMessage) -> Result<(), BinanceWsError> {
        match msg {
            WebSocketMessage::Text(text) => {
                self.handle_text_message(&text).await?;
            },
            WebSocketMessage::Ping(_) => {
                // Respond to ping with pong
                // Note: For Binance, ping/pong is handled automatically by the connection
                debug!("Received ping from Binance");
            },
            WebSocketMessage::Pong(_) => {
                // Ignore pong
                debug!("Received pong from Binance");
            },
            WebSocketMessage::Close(_) => {
                self.connected = false;
                warn!("WebSocket connection closed");
                return Err(BinanceWsError::ChannelClosed);
            },
            _ => {
                // Ignore other message types
            },
        }

        Ok(())
    }

    /// Handle a text message (JSON).
    async fn handle_text_message(&self, text: &str) -> Result<(), BinanceWsError> {
        // Parse JSON
        let json: Value = serde_json::from_str(text)
            .map_err(|e| BinanceWsError::InvalidMessage(e.to_string()))?;

        // Check if it's a trade event
        if let Some(event_type) = json.get("e").and_then(|v| v.as_str()) {
            if event_type == "trade" {
                self.handle_trade_event(&json).await?;
            }
        }

        Ok(())
    }

    /// Handle a Binance trade event.
    async fn handle_trade_event(&self, json: &Value) -> Result<(), BinanceWsError> {
        // Parse Binance trade event
        let event: BinanceTradeEvent = serde_json::from_value(json.clone())
            .map_err(|e| BinanceWsError::InvalidMessage(format!("Invalid trade event: {}", e)))?;

        // Convert Unix timestamp (milliseconds) to DateTime<Utc>
        let timestamp =
            chrono::DateTime::from_timestamp(event.T / 1000, ((event.T % 1000) * 1_000_000) as u32)
                .unwrap_or_else(|| chrono::Utc::now());

        // Convert to domain Tick
        let tick = Tick::new(self.symbol.clone(), event.p, event.q, timestamp, event.t.to_string());

        debug!(
            symbol = %self.symbol.as_pair(),
            price = %tick.price,
            quantity = %tick.quantity,
            "Received trade tick"
        );

        // Publish to event channel
        let market_event = MarketDataEvent::Tick(tick);
        self.event_sender
            .send(market_event)
            .map_err(|_| BinanceWsError::SubscriptionFailed("No receivers".to_string()))?;

        Ok(())
    }

    /// Check if the client is still connected.
    pub fn is_connected(&self) -> bool {
        self.connected
    }
}

// =============================================================================
// Binance-Specific Types (internal to connector)
// =============================================================================

/// Binance trade event (from WebSocket stream).
#[derive(Debug, Clone, serde::Deserialize)]
struct BinanceTradeEvent {
    /// Event type
    #[serde(rename = "e")]
    pub e: String,
    /// Event time
    #[serde(rename = "E")]
    pub E: i64,
    /// Symbol
    #[serde(rename = "s")]
    pub s: String,
    /// Trade ID
    #[serde(rename = "t")]
    pub t: i64,
    /// Price
    #[serde(rename = "p")]
    pub p: Decimal,
    /// Quantity
    #[serde(rename = "q")]
    pub q: Decimal,
    /// Trade time
    #[serde(rename = "T")]
    pub T: i64,
    /// Buyer is market maker
    #[serde(rename = "m")]
    pub m: bool,
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_binance_trade_event_deserialize() {
        let json = r#"
        {
            "e": "trade",
            "E": 123456789,
            "s": "BTCUSDT",
            "t": 12345,
            "p": "95000.00",
            "q": "0.100",
            "T": 1234567890,
            "m": false
        }
        "#;

        let event: BinanceTradeEvent = serde_json::from_str(json).unwrap();

        assert_eq!(event.e, "trade");
        assert_eq!(event.s, "BTCUSDT");
        assert_eq!(event.p, Decimal::from(95000));
    }
}
