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
//! 3. Restore active positions from store/projection
//! 4. Start API server
//! 5. Spawn WebSocket clients (market data)
//! 6. Spawn projection worker (if database configured)
//! 7. Main event loop (process events, market data)
//! 8. Graceful shutdown on SIGINT/SIGTERM

use std::{net::SocketAddr, sync::Arc, time::Duration};

// Macro for creating Decimal literals
use async_trait::async_trait;
use chrono::{DateTime, Datelike, Utc};
use robson_connectors::BinanceRestClient;
use robson_domain::{Position, PositionId, Symbol, TradingPolicy};
use robson_engine::Engine;
use robson_exec::{ExchangePort, Executor, IntentJournal, StubExchange};
#[cfg(feature = "postgres")]
use robson_store::PgDetectedPositionRepository;
// Optional projection recovery for crash recovery
#[cfg(feature = "postgres")]
use robson_store::ProjectionRecovery;
use robson_store::{
    DetectedPositionRepository, MemoryDetectedPositionRepository, MemoryStore, PositionRepository,
    Store, StoreError,
};
use rust_decimal_macros::dec;
#[cfg(feature = "postgres")]
use sqlx::Row;
use tokio::{net::TcpListener, sync::RwLock};
use tracing::{error, info, warn};

#[cfg(feature = "postgres")]
use crate::projection_worker::ProjectionWorker;
#[cfg(feature = "postgres")]
use crate::query::ExecutionQuery;
#[cfg(feature = "postgres")]
use crate::query_engine::{append_query_state_changed_event, EventLogQueryRecorder};
use crate::{
    api::{create_router, ApiState},
    binance_exchange::BinanceExchangeAdapter,
    binance_ohlcv::BinanceOhlcvAdapter,
    config::Config,
    error::{DaemonError, DaemonResult},
    event_bus::{DaemonEvent, EventBus},
    market_data::MarketDataManager,
    position_manager::PositionManager,
    position_monitor::{PositionMonitor, PositionMonitorConfig as RuntimePositionMonitorConfig},
    query_engine::{QueryRecorder, TracingQueryRecorder},
    reconciliation_worker::ReconciliationWorker,
};

// =============================================================================
// Daemon
// =============================================================================

/// The main Robson daemon.
pub struct Daemon<E: ExchangePort + 'static, S: Store + 'static> {
    /// Configuration
    config: Config,
    /// Exchange adapter shared with execution and reconciliation flows.
    exchange: Arc<E>,
    /// Position manager
    position_manager: Arc<RwLock<PositionManager<E, S>>>,
    /// Event bus
    event_bus: Arc<EventBus>,
    /// Store
    store: Arc<S>,
    /// Optional projection recovery for crash recovery (injected trait object)
    #[cfg(feature = "postgres")]
    projection_recovery: Option<Arc<dyn ProjectionRecovery>>,
    /// Shared PostgreSQL connection pool for projection worker (injected)
    #[cfg(feature = "postgres")]
    pg_pool: Option<Arc<sqlx::PgPool>>,
    /// Last month processed by the month-boundary poller.
    last_month_check: Arc<RwLock<(i32, u32)>>,
}

/// Default query recorder (tracing only, no persistence).
fn default_query_recorder() -> Arc<dyn QueryRecorder> {
    Arc::new(TracingQueryRecorder)
}

fn initial_month_check() -> Arc<RwLock<(i32, u32)>> {
    let now = Utc::now();
    Arc::new(RwLock::new((now.year(), now.month())))
}

/// Adapts a generic `Store` into a concrete `PositionRepository` trait object.
struct StorePositionRepositoryAdapter<S: Store + 'static> {
    store: Arc<S>,
}

#[async_trait]
impl<S: Store + 'static> PositionRepository for StorePositionRepositoryAdapter<S> {
    async fn save(&self, position: &Position) -> Result<(), StoreError> {
        self.store.positions().save(position).await
    }

    async fn find_by_id(&self, id: PositionId) -> Result<Option<Position>, StoreError> {
        self.store.positions().find_by_id(id).await
    }

    async fn find_by_account(&self, account_id: uuid::Uuid) -> Result<Vec<Position>, StoreError> {
        self.store.positions().find_by_account(account_id).await
    }

    async fn find_active(&self) -> Result<Vec<Position>, StoreError> {
        self.store.positions().find_active().await
    }

    async fn find_by_state(&self, state: &str) -> Result<Vec<Position>, StoreError> {
        self.store.positions().find_by_state(state).await
    }

    async fn find_active_by_symbol_and_side(
        &self,
        symbol: &robson_domain::Symbol,
        side: robson_domain::Side,
    ) -> Result<Option<Position>, StoreError> {
        self.store.positions().find_active_by_symbol_and_side(symbol, side).await
    }

    async fn delete(&self, id: PositionId) -> Result<(), StoreError> {
        self.store.positions().delete(id).await
    }

    async fn find_closed_in_month(
        &self,
        year: i32,
        month: u32,
    ) -> Result<Vec<Position>, StoreError> {
        self.store.positions().find_closed_in_month(year, month).await
    }

    async fn find_all_closed(&self) -> Result<Vec<Position>, StoreError> {
        self.store.positions().find_all_closed().await
    }
}

impl Daemon<StubExchange, MemoryStore> {
    /// Create a new daemon with stub components (for testing/development).
    pub fn new_stub(config: Config) -> Self {
        use robson_domain::RiskConfig;

        let exchange = Arc::new(StubExchange::new(dec!(95000)));
        let journal = Arc::new(IntentJournal::new());
        let store = Arc::new(MemoryStore::new());
        let executor = Arc::new(Executor::new(Arc::clone(&exchange), journal, store.clone()));
        let event_bus = Arc::new(EventBus::new(1000));
        let query_recorder = default_query_recorder();
        let risk_config = RiskConfig::new(config.engine.capital_base).unwrap();
        let engine = Engine::new(risk_config);
        let trading_policy = TradingPolicy::default();

        let position_manager = Arc::new(RwLock::new(PositionManager::new(
            engine,
            executor,
            store.clone(),
            event_bus.clone(),
            query_recorder,
            trading_policy,
        )));

        Self {
            config,
            exchange,
            position_manager,
            event_bus,
            store,
            #[cfg(feature = "postgres")]
            projection_recovery: None,
            #[cfg(feature = "postgres")]
            pg_pool: None,
            last_month_check: initial_month_check(),
        }
    }

    /// Create a new stub daemon with optional projection recovery and shared
    /// pool.
    #[cfg(feature = "postgres")]
    pub fn new_stub_with_recovery(
        config: Config,
        projection_recovery: Option<Arc<dyn ProjectionRecovery>>,
        pg_pool: Option<Arc<sqlx::PgPool>>,
    ) -> Self {
        use robson_domain::RiskConfig;

        let exchange = Arc::new(StubExchange::new(dec!(95000)));
        let journal = Arc::new(IntentJournal::new());
        let store = Arc::new(MemoryStore::new());
        let executor = Arc::new(Executor::new(Arc::clone(&exchange), journal, store.clone()));
        let event_bus = Arc::new(EventBus::new(1000));
        let query_recorder: Arc<dyn QueryRecorder> =
            if let (Some(pool), Some(tenant_id)) = (&pg_pool, config.projection.tenant_id) {
                Arc::new(EventLogQueryRecorder::new(
                    (**pool).clone(),
                    tenant_id,
                    config.projection.stream_key.clone(),
                ))
            } else {
                default_query_recorder()
            };
        let risk_config = RiskConfig::new(config.engine.capital_base).unwrap();
        let engine = Engine::new(risk_config);
        let trading_policy = TradingPolicy::default();

        let mut pm = PositionManager::new(
            engine,
            executor,
            store.clone(),
            event_bus.clone(),
            query_recorder,
            trading_policy,
        );
        if let (Some(pool), Some(tenant_id)) = (&pg_pool, config.projection.tenant_id) {
            pm = pm.with_event_log((**pool).clone(), tenant_id);
        }
        let position_manager = Arc::new(RwLock::new(pm));

        Self {
            config,
            exchange,
            position_manager,
            event_bus,
            store,
            projection_recovery,
            pg_pool,
            last_month_check: initial_month_check(),
        }
    }
}

impl Daemon<BinanceExchangeAdapter, MemoryStore> {
    /// Create a daemon with Binance exchange adapter (for production).
    pub fn new_binance(config: Config, client: Arc<BinanceRestClient>) -> Self {
        use robson_domain::RiskConfig;

        let exchange = Arc::new(BinanceExchangeAdapter::new(Arc::clone(&client)));
        let ohlcv_port: Arc<dyn robson_exec::OhlcvPort> =
            Arc::new(BinanceOhlcvAdapter::new(client));
        let journal = Arc::new(IntentJournal::new());
        let store = Arc::new(MemoryStore::new());
        let executor = Arc::new(Executor::new(Arc::clone(&exchange), journal, store.clone()));
        let event_bus = Arc::new(EventBus::new(1000));
        let query_recorder = default_query_recorder();
        let risk_config = RiskConfig::new(config.engine.capital_base).unwrap();
        let engine = Engine::new(risk_config);
        let trading_policy = TradingPolicy::default();

        let position_manager = Arc::new(RwLock::new(
            PositionManager::new(
                engine,
                executor,
                store.clone(),
                event_bus.clone(),
                query_recorder,
                trading_policy,
            )
            .with_ohlcv_port(ohlcv_port),
        ));

        Self {
            config,
            exchange,
            position_manager,
            event_bus,
            store,
            #[cfg(feature = "postgres")]
            projection_recovery: None,
            #[cfg(feature = "postgres")]
            pg_pool: None,
            last_month_check: initial_month_check(),
        }
    }

    /// Create a Binance daemon with optional projection recovery and shared
    /// pool.
    #[cfg(feature = "postgres")]
    pub fn new_binance_with_recovery(
        config: Config,
        client: Arc<BinanceRestClient>,
        projection_recovery: Option<Arc<dyn ProjectionRecovery>>,
        pg_pool: Option<Arc<sqlx::PgPool>>,
    ) -> Self {
        use robson_domain::RiskConfig;

        let exchange = Arc::new(BinanceExchangeAdapter::new(Arc::clone(&client)));
        let ohlcv_port: Arc<dyn robson_exec::OhlcvPort> =
            Arc::new(BinanceOhlcvAdapter::new(client));
        let journal = Arc::new(IntentJournal::new());
        let store = Arc::new(MemoryStore::new());
        let executor = Arc::new(Executor::new(Arc::clone(&exchange), journal, store.clone()));
        let event_bus = Arc::new(EventBus::new(1000));

        let query_recorder: Arc<dyn QueryRecorder> =
            if let (Some(pool), Some(tenant_id)) = (&pg_pool, config.projection.tenant_id) {
                Arc::new(EventLogQueryRecorder::new(
                    (**pool).clone(),
                    tenant_id,
                    config.projection.stream_key.clone(),
                ))
            } else {
                default_query_recorder()
            };
        let risk_config = RiskConfig::new(config.engine.capital_base).unwrap();
        let engine = Engine::new(risk_config);
        let trading_policy = TradingPolicy::default();

        let mut pm = PositionManager::new(
            engine,
            executor,
            store.clone(),
            event_bus.clone(),
            query_recorder,
            trading_policy,
        )
        .with_ohlcv_port(ohlcv_port);
        if let (Some(pool), Some(tenant_id)) = (&pg_pool, config.projection.tenant_id) {
            pm = pm.with_event_log((**pool).clone(), tenant_id);
        }
        let position_manager = Arc::new(RwLock::new(pm));

        Self {
            config,
            exchange,
            position_manager,
            event_bus,
            store,
            projection_recovery,
            pg_pool,
            last_month_check: initial_month_check(),
        }
    }
}

impl<E: ExchangePort + 'static, S: Store + 'static> Daemon<E, S> {
    /// Create a new daemon with provided components.
    pub fn new(
        config: Config,
        exchange: Arc<E>,
        position_manager: Arc<RwLock<PositionManager<E, S>>>,
        event_bus: Arc<EventBus>,
        store: Arc<S>,
        #[cfg(feature = "postgres")] projection_recovery: Option<Arc<dyn ProjectionRecovery>>,
        #[cfg(feature = "postgres")] pg_pool: Option<Arc<sqlx::PgPool>>,
    ) -> Self {
        Self {
            config,
            exchange,
            position_manager,
            event_bus,
            store,
            #[cfg(feature = "postgres")]
            projection_recovery,
            #[cfg(feature = "postgres")]
            pg_pool,
            last_month_check: initial_month_check(),
        }
    }

    /// Get a reference to the store (for testing).
    pub fn store(&self) -> &Arc<S> {
        &self.store
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

        // 0. Rebuild store from event log (crash recovery)
        self.rebuild_store().await?;

        // 1. Invalidate durable query approvals that cannot be rehydrated safely.
        #[cfg(feature = "postgres")]
        self.invalidate_restart_pending_queries().await?;

        // 2. Restore active positions
        self.restore_positions().await?;

        let reconciliation_interval = Duration::from_secs(self.config.reconciliation.interval_secs);
        let startup_reconciliation = ReconciliationWorker::new(
            Arc::clone(&self.exchange),
            Arc::clone(&self.store),
            Arc::clone(&self.event_bus),
            reconciliation_interval,
            shutdown.clone(),
        );
        info!("Running startup reconciliation scan");
        let untracked_count = startup_reconciliation.scan_and_reconcile_blocking().await?;
        if untracked_count > 0 {
            return Err(DaemonError::Config(format!(
                "Startup aborted: {} UNTRACKED positions detected and closed. Review exchange account before restarting.",
                untracked_count
            )));
        }
        info!("Startup reconciliation clean (0 UNTRACKED)");

        // 3. Initialize safety net monitor (when configured with Binance credentials)
        let position_monitor = self.initialize_position_monitor().await?;
        let position_monitor_handle =
            position_monitor.as_ref().map(|monitor| Arc::clone(monitor).start());

        // 4. Start API server
        let api_addr = self.start_api_server(position_monitor.clone()).await?;
        info!(%api_addr, "API server started");

        // 5. Spawn reconciliation worker
        let reconciliation_worker = ReconciliationWorker::new(
            Arc::clone(&self.exchange),
            Arc::clone(&self.store),
            Arc::clone(&self.event_bus),
            reconciliation_interval,
            shutdown.clone(),
        );
        let reconciliation_handle = tokio::spawn(async move {
            if let Err(e) = reconciliation_worker.run().await {
                error!(error = %e, "Reconciliation worker failed");
            }
        });

        // 6. Spawn WebSocket clients (Phase 6: Market Data)
        let ws_use_testnet =
            std::env::var("ROBSON_BINANCE_USE_TESTNET").unwrap_or_default() == "true";
        let market_data_manager =
            MarketDataManager::new(self.event_bus.clone(), shutdown.clone(), ws_use_testnet);
        let mut ws_handles = Vec::with_capacity(self.config.market_data.symbols.len());
        for symbol_str in &self.config.market_data.symbols {
            let symbol = Symbol::from_pair(symbol_str).map_err(|e| {
                DaemonError::Config(format!("Invalid symbol {}: {}", symbol_str, e))
            })?;
            let handle = market_data_manager.spawn_ws_client(symbol)?;
            ws_handles.push(handle);
            info!(symbol = %symbol_str, "WebSocket client spawned");
        }

        // 7. Spawn projection worker (if pg_pool configured)
        #[cfg(feature = "postgres")]
        let projection_handle = if let (Some(pool), Some(tenant_id)) =
            (&self.pg_pool, self.config.projection.tenant_id)
        {
            info!(
                stream_key = %self.config.projection.stream_key,
                %tenant_id,
                "Starting projection worker with shared pool"
            );

            let worker =
                ProjectionWorker::new((**pool).clone(), self.config.projection.clone(), tenant_id);

            let worker_shutdown = shutdown.clone();
            Some(tokio::spawn(async move {
                if let Err(e) = worker.run(worker_shutdown).await {
                    error!(error = %e, "Projection worker failed");
                }
            }))
        } else {
            info!("No projection worker configured");
            None
        };

        #[cfg(not(feature = "postgres"))]
        let projection_handle: Option<tokio::task::JoinHandle<()>> = None;

        // 8. Subscribe to event bus
        let mut event_receiver = self.event_bus.subscribe();

        // 9. Spawn ctrl+c handler
        let ctrl_c_shutdown = shutdown.clone();
        tokio::spawn(async move {
            if let Err(_) = tokio::signal::ctrl_c().await {
                error!("Failed to install ctrl+c handler");
            }
            info!("Received ctrl+c, initiating shutdown");
            ctrl_c_shutdown.cancel();
        });

        let mut month_boundary_interval =
            tokio::time::interval(tokio::time::Duration::from_secs(60));
        month_boundary_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        // 10. Main event loop
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

                _ = month_boundary_interval.tick() => {
                    let now = Utc::now();
                    if let Err(e) = self.poll_month_boundary(now).await {
                        error!(error = %e, "Month boundary check failed");
                    }
                }
            }
        }

        // 11. Graceful shutdown
        shutdown_sig.cancel(); // Ensure any remaining tasks are cancelled

        info!("Waiting for reconciliation worker to finish...");
        let _ = tokio::time::timeout(Duration::from_secs(10), reconciliation_handle).await;

        info!(count = ws_handles.len(), "Waiting for WebSocket clients to finish...");
        for handle in ws_handles {
            let _ = tokio::time::timeout(tokio::time::Duration::from_secs(5), handle).await;
        }

        if let Some(handle) = projection_handle {
            info!("Waiting for projection worker to finish...");
            let _ = tokio::time::timeout(tokio::time::Duration::from_secs(30), handle).await;
        }

        if let Some(monitor) = position_monitor {
            if let Ok(m) = Arc::try_unwrap(monitor) {
                m.shutdown().await;
            }
        }
        if let Some(handle) = position_monitor_handle {
            info!("Waiting for position monitor to finish...");
            let _ = tokio::time::timeout(tokio::time::Duration::from_secs(10), handle).await;
        }

        self.shutdown().await?;

        Ok(())
    }

    async fn poll_month_boundary(&self, now: DateTime<Utc>) -> DaemonResult<bool> {
        let current_month = (now.year(), now.month());
        let previous_month = { *self.last_month_check.read().await };

        if previous_month == current_month {
            return Ok(false);
        }

        info!(
            previous = ?previous_month,
            current = ?current_month,
            "Month boundary detected, processing reset"
        );

        self.handle_month_boundary(now).await?;

        let mut last_month_check = self.last_month_check.write().await;
        *last_month_check = current_month;

        Ok(true)
    }

    async fn handle_month_boundary(&self, now: DateTime<Utc>) -> DaemonResult<()> {
        #[cfg(feature = "postgres")]
        if let Some(pool) = &self.pg_pool {
            let exists = sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS(SELECT 1 FROM monthly_state WHERE year = $1 AND month = $2)",
            )
            .bind(now.year())
            .bind(now.month() as i16)
            .fetch_one(&**pool)
            .await?;

            if exists {
                tracing::debug!(
                    year = now.year(),
                    month = now.month(),
                    "MonthBoundaryReset already projected for current month, skipping"
                );
                return Ok(());
            }
        }

        let open_positions = self.store.positions().find_risk_open().await?;
        let carried_risk = Self::calculate_carried_risk(&open_positions);

        // current_equity per ADR-0024 §6:
        //   current_equity = initial_capital + all_time_realized_pnl + unrealized_pnl
        //
        // The capital_base is pessimistic: it subtracts carried_risk (worst case
        // loss from inherited positions) from current_equity. This guarantees
        // every month starts with 4 available slots.
        let current_equity = {
            let manager = self.position_manager.read().await;
            manager.compute_current_equity().await?
        };
        let capital_base = (current_equity - carried_risk).max(rust_decimal::Decimal::ZERO);

        let event = robson_domain::Event::MonthBoundaryReset {
            capital_base,
            carried_positions_risk: carried_risk,
            month: now.month(),
            year: now.year(),
            timestamp: now,
        };

        {
            let manager = self.position_manager.read().await;
            manager.emit_domain_event(event).await?;
        }

        let circuit_breaker = {
            let manager = self.position_manager.read().await;
            manager.circuit_breaker()
        };
        let _ = circuit_breaker.reset().await;

        info!(
            year = now.year(),
            month = now.month(),
            %capital_base,
            %carried_risk,
            "Month boundary processed"
        );

        Ok(())
    }

    fn calculate_carried_risk(positions: &[Position]) -> rust_decimal::Decimal {
        positions
            .iter()
            .filter_map(|position| {
                let qty = position.quantity.as_decimal();
                if qty.is_zero() {
                    return None;
                }

                let (entry, stop) = match &position.state {
                    robson_domain::PositionState::Active { trailing_stop, .. } => {
                        (position.entry_price?.as_decimal(), trailing_stop.as_decimal())
                    },
                    robson_domain::PositionState::Entering { expected_entry, .. } => {
                        let entry = expected_entry.as_decimal();
                        let stop = position
                            .tech_stop_distance
                            .as_ref()
                            .map(|tech_stop| tech_stop.initial_stop.as_decimal())
                            .unwrap_or(entry);
                        (entry, stop)
                    },
                    _ => return None,
                };

                let risk = match position.side {
                    robson_domain::Side::Long => (entry - stop) * qty,
                    robson_domain::Side::Short => (stop - entry) * qty,
                };

                Some(risk.max(rust_decimal::Decimal::ZERO))
            })
            .sum()
    }

    /// Rebuild store from EventLog on startup (crash recovery).
    ///
    /// Reads all stored events and re-applies them to rebuild the in-memory
    /// projection. This is idempotent and safe to run on every boot.
    async fn rebuild_store(&self) -> DaemonResult<()> {
        let events = self.store.events().get_all_events().await?;
        let count = events.len();

        if count == 0 {
            info!("No events to replay, starting with empty store");
            return Ok(());
        }

        info!(count, "Replaying events to rebuild store projection");

        // Apply each event in order to rebuild the projection
        for event in &events {
            self.store.apply_event(event)?;
        }

        info!(count, "Successfully rebuilt store from event log");
        Ok(())
    }

    #[cfg(feature = "postgres")]
    async fn invalidate_restart_pending_queries(&self) -> DaemonResult<()> {
        let (Some(pool), Some(tenant_id)) = (&self.pg_pool, self.config.projection.tenant_id)
        else {
            return Ok(());
        };

        let rows = sqlx::query(
            r#"
            SELECT snapshot
            FROM queries_current
            WHERE tenant_id = $1
              AND stream_key = $2
              AND state = 'AwaitingApproval'
            ORDER BY last_seq ASC
            "#,
        )
        .bind(tenant_id)
        .bind(&self.config.projection.stream_key)
        .fetch_all(&**pool)
        .await?;

        if rows.is_empty() {
            return Ok(());
        }

        let mut invalidated = 0usize;
        for row in rows {
            let snapshot: serde_json::Value = row.try_get("snapshot")?;
            let mut query: ExecutionQuery = serde_json::from_value(snapshot).map_err(|error| {
                DaemonError::Config(format!(
                    "Failed to deserialize AwaitingApproval query snapshot during restart: {}",
                    error
                ))
            })?;

            if query.state != crate::query::QueryState::AwaitingApproval {
                continue;
            }

            query.expire_approval().map_err(|error| {
                DaemonError::Config(format!(
                    "Failed to invalidate AwaitingApproval query {} on restart: {}",
                    query.id, error
                ))
            })?;

            append_query_state_changed_event(
                &**pool,
                tenant_id,
                &self.config.projection.stream_key,
                &query,
                "restart_invalidated",
            )
            .await?;
            invalidated += 1;
        }

        if invalidated > 0 {
            info!(invalidated, "Invalidated persisted AwaitingApproval queries on restart");
        }

        Ok(())
    }

    /// Restore active positions from store or projection.
    ///
    /// First tries to restore from the in-memory store.
    /// If projection_recovery is configured and store is empty,
    /// falls back to reading from the PostgreSQL projection.
    async fn restore_positions(&self) -> DaemonResult<()> {
        // First, try to restore from the store
        let positions = self.store.positions().find_active().await?;
        let store_count = positions.len();

        if store_count > 0 {
            info!(count = store_count, "Restored active positions from store");
            return Ok(());
        }

        // Store is empty, try projection recovery if available
        #[cfg(feature = "postgres")]
        {
            if let (Some(recovery), Some(tenant_id)) =
                (&self.projection_recovery, self.config.projection.tenant_id)
            {
                info!("Store empty, attempting projection recovery");

                match recovery.find_active_from_projection(tenant_id).await {
                    Ok(restored_positions) => {
                        let count = restored_positions.len();
                        if count > 0 {
                            // Save restored positions to store
                            for position in restored_positions {
                                if let Err(e) = self.store.positions().save(&position).await {
                                    error!(
                                        position_id = %position.id,
                                        error = %e,
                                        "Failed to save restored position to store"
                                    );
                                }
                            }

                            info!(count, "Restored active positions from projection");
                        } else {
                            info!("Projection recovery: no active positions found");
                        }
                    },
                    Err(e) => {
                        warn!(error = %e, "Projection recovery failed, continuing with empty store");
                    },
                }
            } else {
                info!("No projection recovery configured, starting with empty store");
            }
        }

        #[cfg(not(feature = "postgres"))]
        {
            info!("No active positions to restore");
        }

        Ok(())
    }

    /// Start the API server.
    ///
    /// Public to allow integration tests to start the server and get the
    /// address.
    pub async fn start_api_server(
        &self,
        position_monitor: Option<Arc<PositionMonitor>>,
    ) -> DaemonResult<SocketAddr> {
        let circuit_breaker = {
            let pm = self.position_manager.read().await;
            pm.circuit_breaker()
        };
        let state = Arc::new(ApiState {
            position_manager: self.position_manager.clone(),
            event_bus: self.event_bus.clone(),
            circuit_breaker,
            position_monitor,
            #[cfg(feature = "postgres")]
            pg_pool: self.pg_pool.clone(),
            api_token: self.config.api.api_token.clone(),
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

    async fn initialize_position_monitor(&self) -> DaemonResult<Option<Arc<PositionMonitor>>> {
        if !self.config.position_monitor.enabled {
            info!("Position monitor disabled by configuration");
            return Ok(None);
        }

        let Some(api_key) = self.config.position_monitor.binance_api_key.clone() else {
            info!("Position monitor enabled but Binance API key not configured; skipping");
            return Ok(None);
        };
        let Some(api_secret) = self.config.position_monitor.binance_api_secret.clone() else {
            info!("Position monitor enabled but Binance API secret not configured; skipping");
            return Ok(None);
        };

        let monitor_config = RuntimePositionMonitorConfig {
            poll_interval_secs: self.config.position_monitor.poll_interval_secs,
            symbols: self.config.position_monitor.symbols.clone(),
            enabled: self.config.position_monitor.enabled,
            ..RuntimePositionMonitorConfig::default()
        };

        let detected_repo: Arc<dyn DetectedPositionRepository> = {
            #[cfg(feature = "postgres")]
            {
                if let Some(pool) = &self.pg_pool {
                    Arc::new(PgDetectedPositionRepository::new(pool.clone()))
                } else {
                    Arc::new(MemoryDetectedPositionRepository::new())
                }
            }
            #[cfg(not(feature = "postgres"))]
            {
                Arc::new(MemoryDetectedPositionRepository::new())
            }
        };

        let core_repo: Arc<dyn PositionRepository> =
            Arc::new(StorePositionRepositoryAdapter { store: self.store.clone() });
        let binance_client = Arc::new(BinanceRestClient::new(api_key, api_secret));

        let monitor = Arc::new(PositionMonitor::with_core_exclusion(
            binance_client,
            self.event_bus.clone(),
            monitor_config,
            detected_repo,
            core_repo,
        ));

        monitor.load_persisted_positions().await?;
        info!(
            symbols = ?self.config.position_monitor.symbols,
            "Position monitor initialized"
        );
        Ok(Some(monitor))
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

            DaemonEvent::QueryAwaitingApproval { query_id, position_id, expires_at, .. } => {
                info!(
                    %query_id,
                    ?position_id,
                    %expires_at,
                    "Query awaiting approval"
                );
            },

            DaemonEvent::QueryAuthorized { query_id, position_id, approved_at } => {
                info!(
                    %query_id,
                    ?position_id,
                    %approved_at,
                    "Query authorized"
                );
            },

            DaemonEvent::QueryExpired { query_id, position_id, expired_at } => {
                warn!(
                    %query_id,
                    ?position_id,
                    %expired_at,
                    "Query approval expired"
                );
            },

            DaemonEvent::Shutdown => {
                info!("Shutdown event received");
                return Err(DaemonError::Shutdown);
            },

            DaemonEvent::RoguePositionDetected { symbol, side, entry_price, stop_price } => {
                info!(
                    %symbol,
                    ?side,
                    %entry_price,
                    %stop_price,
                    "Rogue position detected"
                );
            },

            DaemonEvent::SafetyExitExecuted { symbol, order_id, executed_quantity } => {
                info!(
                    %symbol,
                    %order_id,
                    %executed_quantity,
                    "Safety exit executed"
                );
            },

            DaemonEvent::SafetyExitFailed { symbol, error } => {
                error!(
                    %symbol,
                    %error,
                    "Safety exit failed"
                );
            },

            DaemonEvent::SafetyPanic {
                position_id,
                symbol,
                side,
                error,
                consecutive_failures,
            } => {
                error!(
                    %position_id,
                    %symbol,
                    ?side,
                    %error,
                    %consecutive_failures,
                    "PANIC: Safety exit failed repeatedly, position in panic mode"
                );
            },

            DaemonEvent::CorePositionOpened { position_id, symbol, side, .. } => {
                info!(
                    %position_id,
                    %symbol,
                    ?side,
                    "Core position opened"
                );
            },

            DaemonEvent::CorePositionClosed { position_id, symbol, side } => {
                info!(
                    %position_id,
                    %symbol,
                    ?side,
                    "Core position closed"
                );
            },

            DaemonEvent::MonthlyHaltTriggered { reason, .. } => {
                warn!(
                    %reason,
                    "MonthlyHalt triggered"
                );
            },

            DaemonEvent::MonthlyHaltReset {} => {
                info!("MonthlyHalt reset to Active");
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
    use chrono::{TimeZone, Utc};
    use robson_domain::{PositionState, Price, Quantity, Side, TechnicalStopDistance};
    #[cfg(feature = "postgres")]
    use robson_eventlog::{
        append_event, query_events, ActorType, Event, QueryOptions, QUERY_STATE_CHANGED_EVENT_TYPE,
    };
    #[cfg(feature = "postgres")]
    use robson_projector::apply_event_to_projections;
    #[cfg(feature = "postgres")]
    use rust_decimal_macros::dec;

    use super::*;
    #[cfg(feature = "postgres")]
    use crate::query::{ActorKind, ExecutionQuery, QueryKind};
    #[cfg(feature = "postgres")]
    use crate::query_engine::QueryStateChangedEvent;

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

        let addr = daemon.start_api_server(None).await.unwrap();

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

    #[tokio::test]
    async fn test_poll_month_boundary_emits_reset_once_per_month() {
        let daemon = Daemon::new_stub(Config::test());
        {
            let mut last_month_check = daemon.last_month_check.write().await;
            *last_month_check = (2026, 4);
        }

        let first = Utc.with_ymd_and_hms(2026, 5, 1, 0, 0, 1).single().unwrap();
        assert!(daemon.poll_month_boundary(first).await.unwrap());

        let second = Utc.with_ymd_and_hms(2026, 5, 2, 12, 0, 0).single().unwrap();
        assert!(!daemon.poll_month_boundary(second).await.unwrap());

        let events = daemon.store.events().get_all_events().await.unwrap();
        let month_events: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                robson_domain::Event::MonthBoundaryReset {
                    capital_base,
                    carried_positions_risk,
                    month,
                    year,
                    ..
                } => Some((*capital_base, *carried_positions_risk, *month, *year)),
                _ => None,
            })
            .collect();

        assert_eq!(month_events.len(), 1, "month boundary must emit once per in-process month");
        assert_eq!(month_events[0], (dec!(10000), dec!(0), 5, 2026));
    }

    #[test]
    fn test_calculate_carried_risk_uses_effective_stop_by_state() {
        let account_id = uuid::Uuid::now_v7();
        let now = Utc::now();

        let mut active =
            Position::new(account_id, Symbol::from_pair("BTCUSDT").unwrap(), Side::Long);
        active.entry_price = Some(Price::new(dec!(100)).unwrap());
        active.quantity = Quantity::new(dec!(2)).unwrap();
        active.state = PositionState::Active {
            current_price: Price::new(dec!(110)).unwrap(),
            trailing_stop: Price::new(dec!(95)).unwrap(),
            favorable_extreme: Price::new(dec!(110)).unwrap(),
            extreme_at: now,
            insurance_stop_id: None,
            last_emitted_stop: None,
        };

        let mut entering =
            Position::new(account_id, Symbol::from_pair("ETHUSDT").unwrap(), Side::Long);
        entering.quantity = Quantity::new(dec!(3)).unwrap();
        entering.tech_stop_distance = Some(TechnicalStopDistance::from_entry_and_stop(
            Price::new(dec!(100)).unwrap(),
            Price::new(dec!(90)).unwrap(),
        ));
        entering.state = PositionState::Entering {
            entry_order_id: uuid::Uuid::now_v7(),
            expected_entry: Price::new(dec!(100)).unwrap(),
            signal_id: uuid::Uuid::now_v7(),
        };

        let carried_risk =
            Daemon::<StubExchange, MemoryStore>::calculate_carried_risk(&[active, entering]);

        assert_eq!(carried_risk, dec!(40));
    }

    #[cfg(feature = "postgres")]
    fn ts(hour: u32, minute: u32, second: u32) -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 4, 5, hour, minute, second).single().unwrap()
    }

    #[cfg(feature = "postgres")]
    fn awaiting_approval_query(
        query_id: uuid::Uuid,
        position_id: uuid::Uuid,
        started_at: chrono::DateTime<Utc>,
    ) -> ExecutionQuery {
        use robson_domain::{Price, Side, Symbol};

        let mut query = ExecutionQuery::new(
            QueryKind::ProcessSignal {
                signal_id: uuid::Uuid::from_u128(0xB1),
                symbol: Symbol::from_pair("BTCUSDT").unwrap(),
                side: Side::Long,
                entry_price: Price::new(dec!(95000)).unwrap(),
                stop_loss: Price::new(dec!(85500)).unwrap(),
            },
            ActorKind::Detector,
        );
        query.id = query_id;
        query.position_id = Some(position_id);
        query.started_at = started_at;
        query.transition(crate::query::QueryState::Processing).unwrap();
        query.transition(crate::query::QueryState::RiskChecked).unwrap();
        query.await_approval("manual approval".to_string(), 300).unwrap();
        query.approval.as_mut().unwrap().expires_at = ts(10, 5, 0);
        query
    }

    #[cfg(feature = "postgres")]
    async fn append_query_snapshot(
        pool: &sqlx::PgPool,
        tenant_id: uuid::Uuid,
        stream_key: &str,
        query: &ExecutionQuery,
        transition_cause: &str,
        occurred_at: chrono::DateTime<Utc>,
    ) {
        let payload = QueryStateChangedEvent::from_query(query, transition_cause);
        let mut event = Event::new(
            tenant_id,
            stream_key,
            QUERY_STATE_CHANGED_EVENT_TYPE,
            serde_json::to_value(payload).unwrap(),
        )
        .with_actor(ActorType::Daemon, Some("daemon-restart-test".to_string()));
        event.occurred_at = occurred_at;

        let event_id = append_event(pool, stream_key, None, event).await.unwrap();
        let envelope: robson_eventlog::EventEnvelope =
            sqlx::query_as("SELECT * FROM event_log WHERE event_id = $1")
                .bind(event_id)
                .fetch_one(pool)
                .await
                .unwrap();
        apply_event_to_projections(pool, &envelope).await.unwrap();
    }

    #[cfg(feature = "postgres")]
    #[sqlx::test(migrations = "../migrations")]
    #[ignore = "Requires DATABASE_URL to be set"]
    async fn test_restart_invalidates_awaiting_approval_queries(pool: sqlx::PgPool) {
        let tenant_id = uuid::Uuid::from_u128(0x200);
        let stream_key = "robson:daemon:phase4:restart";
        let query_id = uuid::Uuid::from_u128(0x201);
        let position_id = uuid::Uuid::from_u128(0x202);

        let awaiting_query = awaiting_approval_query(query_id, position_id, ts(10, 0, 0));
        append_query_snapshot(
            &pool,
            tenant_id,
            stream_key,
            &awaiting_query,
            "awaiting_approval",
            ts(10, 0, 1),
        )
        .await;

        let mut config = Config::test();
        config.projection.tenant_id = Some(tenant_id);
        config.projection.stream_key = stream_key.to_string();

        let daemon = Daemon::new_stub_with_recovery(config, None, Some(Arc::new(pool.clone())));
        daemon.invalidate_restart_pending_queries().await.unwrap();
        daemon.invalidate_restart_pending_queries().await.unwrap();

        let events = query_events(
            &pool,
            QueryOptions::new(tenant_id)
                .stream(stream_key)
                .event_type(QUERY_STATE_CHANGED_EVENT_TYPE),
        )
        .await
        .unwrap();

        assert_eq!(events.len(), 2, "restart invalidation must be idempotent");

        let last_payload: QueryStateChangedEvent =
            serde_json::from_value(events.last().unwrap().payload.clone()).unwrap();
        assert_eq!(last_payload.query_id, query_id);
        assert_eq!(last_payload.state, "Expired");
        assert_eq!(last_payload.transition_cause, "restart_invalidated");

        apply_event_to_projections(&pool, events.last().unwrap()).await.unwrap();

        let projected_state: String = sqlx::query_scalar(
            r#"
            SELECT state
            FROM queries_current
            WHERE query_id = $1
            "#,
        )
        .bind(query_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(projected_state, "Expired");
    }
}
