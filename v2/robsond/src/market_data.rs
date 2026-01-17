//! Market data manager for WebSocket integration.
//!
//! Spawns WebSocket client tasks and bridges market data events
//! from connectors to the daemon event bus.

use robson_connectors::BinanceMarketDataClient;
use robson_domain::{MarketDataEvent, Price, Symbol};
use std::sync::Arc;
use tokio::sync::broadcast;
use tokio::task::JoinHandle;
use tracing::{error, info};

use crate::event_bus::{DaemonEvent, EventBus};
use crate::error::DaemonResult;

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
        // Create internal broadcast channel for MarketDataEvent
        let (event_sender, mut event_receiver) = broadcast::channel::<MarketDataEvent>(100);

        // Clone event_bus and symbol for the bridge task
        let event_bus = self.event_bus.clone();
        let symbol_for_bridge = symbol.clone();

        // Spawn the WebSocket client task
        let client_task = tokio::spawn(async move {
            // TODO: In production, handle reconnect logic here
            let mut client = match BinanceMarketDataClient::new(symbol.clone(), event_sender).await {
                Ok(c) => {
                    info!(symbol = %symbol.as_pair(), "WebSocket client connected");
                    c
                }
                Err(e) => {
                    error!(error = %e, symbol = %symbol.as_pair(), "Failed to create WebSocket client");
                    return;
                }
            };

            info!(symbol = %symbol.as_pair(), "WebSocket client task started");

            // Run the client message loop
            match client.run().await {
                Ok(_) => {
                    info!(symbol = %symbol.as_pair(), "WebSocket client disconnected gracefully");
                }
                Err(e) => {
                    error!(error = %e, symbol = %symbol.as_pair(), "WebSocket client error");
                }
            }
        });

        // Spawn a bridge task to forward MarketDataEvent to DaemonEvent
        let _bridge_task = tokio::spawn(async move {
            let mut first_tick_logged = false;

            while let Ok(result) = event_receiver.recv().await {
                match result {
                    MarketDataEvent::Tick(tick) => {
                        if !first_tick_logged {
                            info!(
                                symbol = %tick.symbol.as_pair(),
                                price = %tick.price,
                                quantity = %tick.quantity,
                                "First tick received"
                            );
                            first_tick_logged = true;
                        }

                        // Convert Decimal to Price (unwrap is safe: tick prices are always positive)
                        let price = Price::new(tick.price).unwrap();

                        // Convert to DaemonEvent and publish
                        let daemon_event = DaemonEvent::MarketData(crate::event_bus::MarketData {
                            symbol: tick.symbol.clone(),
                            price,
                            timestamp: tick.timestamp,
                        });

                        event_bus.send(daemon_event);
                    }
                    _ => {
                        // Other event types (Candle, OrderBookSnapshot) not implemented yet
                    }
                }
            }

            info!(symbol = %symbol_for_bridge.as_pair(), "Market data bridge task ended");
        });

        Ok(client_task)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_market_data_manager_creation() {
        let event_bus = Arc::new(EventBus::new(100));
        let manager = MarketDataManager::new(event_bus);

        // Manager is created successfully
        // Actual WebSocket connection requires async runtime
    }
}
