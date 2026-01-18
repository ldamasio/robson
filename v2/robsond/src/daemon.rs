//! Daemon: Main runtime orchestrator.
//!
//! The Daemon ties together all components:
//! - Position Manager (position lifecycle)
//! - Event Bus (internal communication)
//! - API Server (HTTP endpoints)
//! - Market Data (price updates)
//! - Projection Worker (event log → projections)
//!
//! # Lifecycle
//!
//! 1. Load configuration
//! 2. Initialize components
//! 3. Restore active positions from store
//! 4. Start API server
//! 5. Spawn WebSocket clients (market data)
//! 6. Spawn projection worker (if database configured)
//! 7. Main event loop (process events, market data)
//! 8. Graceful shutdown on SIGINT/SIGTERM

use std::net::SocketAddr;
use std::sync::Arc;

// Macro for creating Decimal literals
use rust_decimal_macros::dec;

use tokio::net::TcpListener;
use tokio::sync::RwLock;
use tracing::{error, info, warn};

use robson_domain::Symbol;
use robson_engine::Engine;
use robson_exec::{ExchangePort, Executor, IntentJournal, StubExchange};
use robson_store::{MemoryStore, Store};

use crate::api::{ApiState, create_router};
use crate::config::Config;
use crate::error::{DaemonError, DaemonResult};
use crate::event_bus::{DaemonEvent, EventBus};
use crate::market_data::MarketDataManager;
use crate::position_manager::PositionManager;
use crate::projection_worker::ProjectionWorker;

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
        use robson_domain::RiskConfig;

        let exchange = Arc::new(StubExchange::new(dec!(95000)));
        let journal = Arc::new(IntentJournal::new());
        let store = Arc::new(MemoryStore::new());
        let executor = Arc::new(Executor::new(exchange, journal, store.clone()));
        let event_bus = Arc::new(EventBus::new(1000));
        let risk_config = RiskConfig::new(dec!(10000), dec!(1)).unwrap();
        let engine = Engine::new(risk_config);

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

        // Create shutdown token for coordinating graceful shutdown
        let shutdown = tokio_util::sync::CancellationToken::new();
        let shutdown_sig = shutdown.clone();

        // 1. Restore active positions
        self.restore_positions().await?;

        // 2. Start API server
        let api_addr = self.start_api_server().await?;
        info!(%api_addr, "API server started");

        // 3. Spawn WebSocket client (Phase 6: Market Data)
        // TODO: Make this configurable (symbols list from config)
        let market_data_manager = MarketDataManager::new(self.event_bus.clone());
        let btcusdt = Symbol::from_pair("BTCUSDT").unwrap();
        let _ws_handle = market_data_manager.spawn_ws_client(btcusdt)?;
        info!("WebSocket client spawned for BTCUSDT");

        // 4. Spawn projection worker (if database configured)
        let projection_handle = match &self.config.projection.database_url {
            Some(database_url) => match self.config.projection.tenant_id {
                Some(id) => {
                    info!(
                        stream_key = %self.config.projection.stream_key,
                        %id,
                        "Starting projection worker"
                    );

                    let pool = sqlx::PgPool::connect(database_url).await?;
                    let worker = ProjectionWorker::new(pool, self.config.projection.clone(), id);

                    let worker_shutdown = shutdown.clone();
                    Some(tokio::spawn(async move {
                        if let Err(e) = worker.run(worker_shutdown).await {
                            error!(error = %e, "Projection worker failed");
                        }
                    }))
                }
                None => {
                    warn!(
                        "DATABASE_URL set but PROJECTION_TENANT_ID missing, projection worker disabled"
                    );
                    None
                }
            },
            None => {
                info!("No DATABASE_URL configured, projection worker disabled");
                None
            }
        };

        // 5. Subscribe to event bus
        let mut event_receiver = self.event_bus.subscribe();

        // 6. Spawn ctrl+c handler
        let ctrl_c_shutdown = shutdown.clone();
        tokio::spawn(async move {
            if let Err(_) = tokio::signal::ctrl_c().await {
                error!("Failed to install ctrl+c handler");
            }
            info!("Received ctrl+c, initiating shutdown");
            ctrl_c_shutdown.cancel();
        });

        // 7. Main event loop
        info!("Entering main event loop");
        loop {
            tokio::select! {
                // Shutdown requested
                _ = shutdown.cancelled() => {
                    info!("Shutdown requested");
                    break;
                }

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
            }
        }

        // 8. Graceful shutdown
        shutdown_sig.cancel(); // Ensure any remaining tasks are cancelled

        if let Some(handle) = projection_handle {
            info!("Waiting for projection worker to finish...");
            let _ = tokio::time::timeout(tokio::time::Duration::from_secs(30), handle).await;
        }

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

        let listener = TcpListener::bind(&addr)
            .await
            .map_err(|e| DaemonError::Config(format!("Failed to bind to {}: {}", addr, e)))?;

        let local_addr = listener
            .local_addr()
            .map_err(|e| DaemonError::Config(format!("Failed to get local address: {}", e)))?;

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
            },

            DaemonEvent::MarketData(data) => {
                let manager = self.position_manager.read().await;
                manager.process_market_data(data).await?;
            },

            DaemonEvent::OrderFill(fill) => {
                info!(
                    position_id = %fill.position_id,
                    order_id = %fill.order_id,
                    fill_price = %fill.fill_price.as_decimal(),
                    "Received order fill"
                );
                // Order fills are handled internally by executor
                // This is just for logging/monitoring
            },

            DaemonEvent::PositionStateChanged {
                position_id, previous_state, new_state, ..
            } => {
                info!(
                    %position_id,
                    %previous_state,
                    %new_state,
                    "Position state changed"
                );
            },

            DaemonEvent::Shutdown => {
                info!("Shutdown event received");
                return Err(DaemonError::Shutdown);
            },
        }

        Ok(())
    }

    /// Graceful shutdown.
    async fn shutdown(&self) -> DaemonResult<()> {
        info!("Initiating graceful shutdown");

        // Shutdown position manager (cancels all detector tasks)
        // Clone the Arc to drop the read lock before calling shutdown
        let position_manager = Arc::clone(&self.position_manager);
        position_manager.read().await.shutdown().await;

        info!("Shutdown complete");
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
        let response = client.get(format!("http://{}/health", addr)).send().await.unwrap();

        assert!(response.status().is_success());
    }

    #[tokio::test]
    async fn test_daemon_restore_empty() {
        let config = Config::test();
        let daemon = Daemon::new_stub(config);

        // Should not fail with empty store
        daemon.restore_positions().await.unwrap();
    }

    #[tokio::test]
    async fn test_daemon_shutdown() {
        // Test that daemon shutdown properly calls position manager shutdown
        let config = Config::test();
        let daemon = Daemon::new_stub(config);

        // Shutdown should complete without errors
        daemon.shutdown().await.unwrap();
    }
}
