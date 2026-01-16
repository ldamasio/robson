//! Daemon: Main runtime orchestrator.
//!
//! The Daemon ties together all components:
//! - Position Manager (position lifecycle)
//! - Event Bus (internal communication)
//! - API Server (HTTP endpoints)
//! - Market Data (price updates)
//!
//! # Lifecycle
//!
//! 1. Load configuration
//! 2. Initialize components
//! 3. Restore active positions from store
//! 4. Start API server
//! 5. Main event loop (process events, market data)
//! 6. Graceful shutdown on SIGINT/SIGTERM

use std::net::SocketAddr;
use std::sync::Arc;

use tokio::net::TcpListener;
use tokio::sync::RwLock;
use tracing::{error, info, warn};

use robson_engine::Engine;
use robson_exec::{ExchangePort, Executor, IntentJournal, MarketDataPort, StubExchange};
use robson_store::{MemoryStore, Store};

use crate::api::{create_router, ApiState};
use crate::config::{Config, Environment};
use crate::error::{DaemonError, DaemonResult};
use crate::event_bus::{DaemonEvent, EventBus};
use crate::position_manager::PositionManager;

// =============================================================================
// Daemon
// =============================================================================

/// The main Robson daemon.
pub struct Daemon<E: ExchangePort + 'static, S: Store + 'static> {
    /// Configuration
    config: Config,
    /// Position manager
    position_manager: Arc<RwLock<PositionManager<E, S>>>,
    /// Event bus
    event_bus: Arc<EventBus>,
    /// Store
    store: Arc<S>,
}

impl Daemon<StubExchange, MemoryStore> {
    /// Create a new daemon with stub components (for testing/development).
    pub fn new_stub(config: Config) -> Self {
        let exchange = Arc::new(StubExchange::new(rust_decimal_macros::dec!(95000)));
        let journal = Arc::new(IntentJournal::new());
        let store = Arc::new(MemoryStore::new());
        let executor = Arc::new(Executor::new(exchange, journal, store.clone()));
        let event_bus = Arc::new(EventBus::new(1000));
        let engine = Engine::new();

        let position_manager = Arc::new(RwLock::new(PositionManager::new(
            engine,
            executor,
            store.clone(),
            event_bus.clone(),
        )));

        Self {
            config,
            position_manager,
            event_bus,
            store,
        }
    }
}

impl<E: ExchangePort + 'static, S: Store + 'static> Daemon<E, S> {
    /// Create a new daemon with provided components.
    pub fn new(
        config: Config,
        position_manager: Arc<RwLock<PositionManager<E, S>>>,
        event_bus: Arc<EventBus>,
        store: Arc<S>,
    ) -> Self {
        Self {
            config,
            position_manager,
            event_bus,
            store,
        }
    }

    /// Run the daemon.
    ///
    /// This method blocks until shutdown is requested (SIGINT/SIGTERM).
    pub async fn run(self) -> DaemonResult<()> {
        info!(
            version = env!("CARGO_PKG_VERSION"),
            environment = %self.config.environment,
            "Starting Robson daemon"
        );

        // 1. Restore active positions
        self.restore_positions().await?;

        // 2. Start API server
        let api_addr = self.start_api_server().await?;
        info!(%api_addr, "API server started");

        // 3. Subscribe to event bus
        let mut event_receiver = self.event_bus.subscribe();

        // 4. Main event loop
        info!("Entering main event loop");
        loop {
            tokio::select! {
                // Process events from event bus
                Some(event_result) = event_receiver.recv() => {
                    match event_result {
                        Ok(event) => {
                            if let Err(e) = self.handle_event(event).await {
                                error!(error = %e, "Error handling event");
                            }
                        }
                        Err(lag_msg) => {
                            warn!(%lag_msg, "Event receiver lagged");
                        }
                    }
                }

                // Handle shutdown signals
                _ = tokio::signal::ctrl_c() => {
                    info!("Received shutdown signal");
                    break;
                }
            }
        }

        // 5. Graceful shutdown
        self.shutdown().await?;

        Ok(())
    }

    /// Restore active positions from store.
    async fn restore_positions(&self) -> DaemonResult<()> {
        let positions = self.store.positions().find_active().await?;
        let count = positions.len();

        if count > 0 {
            info!(count, "Restored active positions from store");

            // For each Armed position, we'd spawn a detector
            // For each Active position, we resume monitoring
            // This will be implemented in Phase 7 (Detector Interface)
        } else {
            info!("No active positions to restore");
        }

        Ok(())
    }

    /// Start the API server.
    async fn start_api_server(&self) -> DaemonResult<SocketAddr> {
        let state = Arc::new(ApiState {
            position_manager: self.position_manager.clone(),
        });

        let router = create_router(state);
        let addr = format!("{}:{}", self.config.api.host, self.config.api.port);

        let listener = TcpListener::bind(&addr).await.map_err(|e| {
            DaemonError::Config(format!("Failed to bind to {}: {}", addr, e))
        })?;

        let local_addr = listener.local_addr().map_err(|e| {
            DaemonError::Config(format!("Failed to get local address: {}", e))
        })?;

        // Spawn the server task
        tokio::spawn(async move {
            if let Err(e) = axum::serve(listener, router).await {
                error!(error = %e, "API server error");
            }
        });

        Ok(local_addr)
    }

    /// Handle an event from the event bus.
    async fn handle_event(&self, event: DaemonEvent) -> DaemonResult<()> {
        match event {
            DaemonEvent::DetectorSignal(signal) => {
                info!(
                    position_id = %signal.position_id,
                    signal_id = %signal.signal_id,
                    "Received detector signal"
                );
                let manager = self.position_manager.write().await;
                manager.handle_signal(signal).await?;
            }

            DaemonEvent::MarketData(data) => {
                let manager = self.position_manager.read().await;
                manager.process_market_data(data).await?;
            }

            DaemonEvent::OrderFill(fill) => {
                info!(
                    position_id = %fill.position_id,
                    order_id = %fill.order_id,
                    fill_price = %fill.fill_price.as_decimal(),
                    "Received order fill"
                );
                // Order fills are handled internally by executor
                // This is just for logging/monitoring
            }

            DaemonEvent::PositionStateChanged {
                position_id,
                previous_state,
                new_state,
                ..
            } => {
                info!(
                    %position_id,
                    %previous_state,
                    %new_state,
                    "Position state changed"
                );
            }

            DaemonEvent::Shutdown => {
                info!("Shutdown event received");
                return Err(DaemonError::Shutdown);
            }
        }

        Ok(())
    }

    /// Graceful shutdown.
    async fn shutdown(&self) -> DaemonResult<()> {
        info!("Initiating graceful shutdown");

        // In production, we might:
        // 1. Stop accepting new positions
        // 2. Wait for pending orders to complete
        // 3. Persist any in-memory state
        // 4. Close connections

        // For now, just log
        let manager = self.position_manager.read().await;
        let count = manager.position_count().await?;
        info!(active_positions = count, "Shutdown complete");

        Ok(())
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_daemon_stub_creation() {
        let config = Config::test();
        let daemon = Daemon::new_stub(config);

        // Should be able to access position manager
        let manager = daemon.position_manager.read().await;
        let count = manager.position_count().await.unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn test_daemon_api_server_start() {
        let config = Config::test();
        let daemon = Daemon::new_stub(config);

        let addr = daemon.start_api_server().await.unwrap();

        // Server should be running on a port
        assert!(addr.port() > 0);

        // Can make a health check request
        let client = reqwest::Client::new();
        let response = client
            .get(format!("http://{}/health", addr))
            .send()
            .await
            .unwrap();

        assert!(response.status().is_success());
    }

    #[tokio::test]
    async fn test_daemon_restore_empty() {
        let config = Config::test();
        let daemon = Daemon::new_stub(config);

        // Should not fail with empty store
        daemon.restore_positions().await.unwrap();
    }
}
