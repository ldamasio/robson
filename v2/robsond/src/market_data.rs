//! Market data manager for WebSocket integration.
//!
//! Spawns WebSocket client tasks and bridges market data events
//! from connectors to the daemon event bus.

use robson_connectors::{BinanceWebSocketClient, WsMessage};
use robson_domain::{Price, Symbol};
use std::str::FromStr;
use std::sync::Arc;
use tokio::task::JoinHandle;
use tracing::{error, info};

use crate::error::DaemonResult;
use crate::event_bus::{DaemonEvent, EventBus};

/// Market data manager - spawns and manages WebSocket tasks.
pub struct MarketDataManager {
    /// Event bus for publishing market data
    event_bus: Arc<EventBus>,
}

impl MarketDataManager {
    /// Create a new market data manager.
    pub fn new(event_bus: Arc<EventBus>) -> Self {
        Self { event_bus }
    }

    /// Spawn a WebSocket client task for a single symbol.
    ///
    /// Returns a join handle that can be used to monitor the task.
    pub fn spawn_ws_client(&self, symbol: Symbol) -> DaemonResult<JoinHandle<()>> {
        let event_bus = self.event_bus.clone();
        let symbol_str = symbol.as_pair();

        let handle = tokio::spawn(async move {
            let ws_client = BinanceWebSocketClient::new(false);

            let mut stream = match ws_client.subscribe_agg_trade(&symbol_str).await {
                Ok(s) => {
                    info!(symbol = %symbol_str, "WebSocket client connected");
                    s
                }
                Err(e) => {
                    error!(error = %e, symbol = %symbol_str, "Failed to connect WebSocket");
                    return;
                }
            };

            info!(symbol = %symbol_str, "WebSocket client task started");

            let mut first_tick_logged = false;

            loop {
                match stream.next().await {
                    None => {
                        info!(symbol = %symbol_str, "WebSocket stream closed");
                        break;
                    }
                    Some(Err(e)) => {
                        error!(error = %e, symbol = %symbol_str, "WebSocket stream error");
                        break;
                    }
                    Some(Ok(WsMessage::AggTrade(trade))) => {
                        let price_decimal = match rust_decimal::Decimal::from_str(&trade.price) {
                            Ok(d) => d,
                            Err(e) => {
                                error!(error = %e, "Failed to parse price from agg trade");
                                continue;
                            }
                        };

                        let price = match Price::new(price_decimal) {
                            Ok(p) => p,
                            Err(e) => {
                                error!(error = %e, price = %trade.price, "Invalid price value");
                                continue;
                            }
                        };

                        if !first_tick_logged {
                            info!(
                                symbol = %trade.symbol,
                                price = %price_decimal,
                                "First tick received"
                            );
                            first_tick_logged = true;
                        }

                        let trade_symbol = match Symbol::from_pair(&trade.symbol) {
                            Ok(s) => s,
                            Err(e) => {
                                error!(error = %e, symbol = %trade.symbol, "Failed to parse symbol");
                                continue;
                            }
                        };

                        let timestamp = chrono::Utc::now();
                        let daemon_event = DaemonEvent::MarketData(crate::event_bus::MarketData {
                            symbol: trade_symbol,
                            price,
                            timestamp,
                        });

                        event_bus.send(daemon_event);
                    }
                    Some(Ok(_)) => {
                        // Other message types not needed here
                    }
                }
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
        let _manager = MarketDataManager::new(event_bus);
        // Manager is created successfully
    }
}
