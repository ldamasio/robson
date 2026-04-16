//! Market data manager for WebSocket integration.
//!
//! Spawns WebSocket client tasks and bridges market data events
//! from connectors to the daemon event bus.
//!
//! # Reconnection
//!
//! The spawned task runs indefinitely. When the WebSocket stream closes or
//! errors (Binance disconnects periodically — this is normal), the task waits
//! with exponential backoff (1 s → 2 s → 4 s … capped at 60 s) and reconnects.
//! The task only terminates on daemon shutdown (via `CancellationToken`).

use std::{str::FromStr, sync::Arc};

use robson_connectors::{BinanceWebSocketClient, WsMessage};
use robson_domain::{Price, Symbol};
use tokio::{
    task::JoinHandle,
    time::{sleep, Duration},
};
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use crate::{
    error::DaemonResult,
    event_bus::{DaemonEvent, EventBus},
};

/// Maximum reconnect backoff in seconds.
const MAX_BACKOFF_SECS: u64 = 60;

/// Market data manager - spawns and manages WebSocket tasks.
pub struct MarketDataManager {
    /// Event bus for publishing market data
    event_bus: Arc<EventBus>,
    /// Cancellation token for graceful shutdown
    cancel: CancellationToken,
    /// Whether to connect to Binance testnet streams (mirrors ROBSON_BINANCE_USE_TESTNET)
    use_testnet: bool,
}

impl MarketDataManager {
    /// Create a new market data manager.
    pub fn new(event_bus: Arc<EventBus>, cancel: CancellationToken, use_testnet: bool) -> Self {
        Self { event_bus, cancel, use_testnet }
    }

    /// Spawn a WebSocket client task for a single symbol.
    ///
    /// The task runs indefinitely, reconnecting with exponential backoff when
    /// the stream closes or errors. It exits cleanly when the cancellation
    /// token is cancelled.
    ///
    /// Returns a join handle that completes only on shutdown.
    pub fn spawn_ws_client(&self, symbol: Symbol) -> DaemonResult<JoinHandle<()>> {
        let event_bus = self.event_bus.clone();
        let cancel = self.cancel.clone();
        let symbol_str = symbol.as_pair();

        let use_testnet = self.use_testnet;
        let handle = tokio::spawn(async move {
            let ws_client = BinanceWebSocketClient::new(use_testnet);
            let mut backoff_secs: u64 = 1;

            'reconnect: loop {
                if cancel.is_cancelled() {
                    break;
                }

                let mut stream = match ws_client.subscribe_agg_trade(&symbol_str).await {
                    Ok(s) => {
                        info!(symbol = %symbol_str, "WebSocket client connected");
                        // Backoff resets only after first tick, not on connect, so that
                        // Binance accept-then-immediately-close loops still back off.
                        s
                    },
                    Err(e) => {
                        error!(
                            error = %e,
                            symbol = %symbol_str,
                            retry_in_secs = backoff_secs,
                            "WebSocket connect failed, retrying"
                        );
                        tokio::select! {
                            _ = sleep(Duration::from_secs(backoff_secs)) => {},
                            _ = cancel.cancelled() => break 'reconnect,
                        }
                        backoff_secs = (backoff_secs * 2).min(MAX_BACKOFF_SECS);
                        continue 'reconnect;
                    },
                };

                let mut first_tick_logged = false;

                loop {
                    tokio::select! {
                        _ = cancel.cancelled() => {
                            info!(symbol = %symbol_str, "WebSocket client shutting down");
                            break 'reconnect;
                        }
                        msg = stream.next() => {
                            match msg {
                                None => {
                                    warn!(
                                        symbol = %symbol_str,
                                        retry_in_secs = backoff_secs,
                                        "WebSocket stream closed, reconnecting"
                                    );
                                    break; // break inner loop → reconnect
                                },
                                Some(Err(e)) => {
                                    error!(
                                        error = %e,
                                        symbol = %symbol_str,
                                        retry_in_secs = backoff_secs,
                                        "WebSocket stream error, reconnecting"
                                    );
                                    break; // break inner loop → reconnect
                                },
                                Some(Ok(WsMessage::AggTrade(trade))) => {
                                    let price_decimal =
                                        match rust_decimal::Decimal::from_str(&trade.price) {
                                            Ok(d) => d,
                                            Err(e) => {
                                                error!(error = %e, "Failed to parse price");
                                                continue;
                                            },
                                        };

                                    let price = match Price::new(price_decimal) {
                                        Ok(p) => p,
                                        Err(e) => {
                                            error!(
                                                error = %e,
                                                price = %trade.price,
                                                "Invalid price value"
                                            );
                                            continue;
                                        },
                                    };

                                    if !first_tick_logged {
                                        info!(
                                            symbol = %trade.symbol,
                                            price = %price_decimal,
                                            "First tick received"
                                        );
                                        first_tick_logged = true;
                                        backoff_secs = 1; // stable connection confirmed
                                    }

                                    let trade_symbol = match Symbol::from_pair(&trade.symbol) {
                                        Ok(s) => s,
                                        Err(e) => {
                                            error!(
                                                error = %e,
                                                symbol = %trade.symbol,
                                                "Failed to parse symbol"
                                            );
                                            continue;
                                        },
                                    };

                                    let daemon_event =
                                        DaemonEvent::MarketData(crate::event_bus::MarketData {
                                            symbol: trade_symbol,
                                            price,
                                            timestamp: chrono::Utc::now(),
                                        });

                                    event_bus.send(daemon_event);
                                },
                                Some(Ok(_)) => {
                                    // Other message types not needed here
                                },
                            }
                        }
                    }
                }

                // Backoff before reconnect attempt
                tokio::select! {
                    _ = sleep(Duration::from_secs(backoff_secs)) => {},
                    _ = cancel.cancelled() => break 'reconnect,
                }
                backoff_secs = (backoff_secs * 2).min(MAX_BACKOFF_SECS);
            }

            info!(symbol = %symbol_str, "WebSocket client task ended");
        });

        Ok(handle)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_market_data_manager_creation() {
        let event_bus = Arc::new(EventBus::new(100));
        let cancel = CancellationToken::new();
        let _manager = MarketDataManager::new(event_bus, cancel, false);
        // Manager is created successfully
    }
}
