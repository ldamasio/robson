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
#[cfg(feature = "postgres")]
use robson_domain::{ApprovalPolicy, EntryPolicy, EntryPolicyConfig};
use robson_domain::{Position, PositionId, PositionState, Symbol, TradingPolicy};
use robson_engine::Engine;
#[cfg(feature = "postgres")]
use robson_eventlog::{query_events, EventEnvelope, QueryOptions};
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
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
#[cfg(feature = "postgres")]
use sqlx::Row;
use tokio::{net::TcpListener, sync::RwLock};
use tracing::{error, info, warn};

#[cfg(feature = "postgres")]
use crate::funding::{worker::FundingWorker, FundingService};
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
    config::{Config, StartupStaleActivePolicy},
    error::{DaemonError, DaemonResult, StartupStaleActiveInfo},
    event_bus::{DaemonEvent, EventBus},
    market_data::MarketDataManager,
    position_manager::{PositionManager, ReconcileCloseOutcome, ReconciledCloseInput},
    position_monitor::{PositionMonitor, PositionMonitorConfig as RuntimePositionMonitorConfig},
    query_engine::{QueryRecorder, TracingQueryRecorder},
    reconciliation_worker::{gather_real_evidence, ReconciliationWorker},
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

/// Parse entry_mode and approval_mode strings from DB into EntryPolicyConfig.
/// Returns None for unknown variants so callers can log and skip.
#[cfg(feature = "postgres")]
fn parse_entry_policy_config(entry_mode: &str, approval_mode: &str) -> Option<EntryPolicyConfig> {
    let mode = match entry_mode {
        "immediate" => EntryPolicy::Immediate,
        "confirmed_trend" => EntryPolicy::ConfirmedTrend,
        "confirmed_reversal" => EntryPolicy::ConfirmedReversal,
        "confirmed_key_level" => EntryPolicy::ConfirmedKeyLevel,
        _ => return None,
    };
    let approval = match approval_mode {
        "automatic" => ApprovalPolicy::Automatic,
        "human_confirmation" => ApprovalPolicy::HumanConfirmation,
        _ => return None,
    };
    Some(EntryPolicyConfig::new(mode, approval))
}

/// Default query recorder (tracing only, no persistence).
fn default_query_recorder() -> Arc<dyn QueryRecorder> {
    Arc::new(TracingQueryRecorder)
}

fn initial_month_check() -> Arc<RwLock<(i32, u32)>> {
    // Sentinel, never the wall-clock month: seeding with the current month
    // made a restart landing after a month boundary skip that month's
    // MonthBoundaryReset forever (2026-07 incident). With the sentinel, the
    // first poll always runs handle_month_boundary, which consults the
    // persisted monthly_state row and no-ops when the reset already exists.
    Arc::new(RwLock::new((0, 0)))
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
        let risk_config = RiskConfig::new(dec!(10000)).unwrap();
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

    /// Create a stub daemon with a specific capital for position sizing tests.
    pub fn new_stub_with_capital(config: Config, capital: Decimal) -> Self {
        use robson_domain::RiskConfig;

        let exchange = Arc::new(StubExchange::with_balance(dec!(95000), capital));
        let journal = Arc::new(IntentJournal::new());
        let store = Arc::new(MemoryStore::new());
        let executor = Arc::new(Executor::new(Arc::clone(&exchange), journal, store.clone()));
        let event_bus = Arc::new(EventBus::new(1000));
        let query_recorder = default_query_recorder();
        let risk_config = RiskConfig::new(capital).unwrap();
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
        let risk_config = RiskConfig::new(dec!(10000)).unwrap();
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
        // Placeholder capital; will be updated from exchange in run().
        // Execution-cost buffer parameters are operator-configured and are
        // preserved across capital rebuilds (ADR-0039). Invalid values are a
        // config error: fail fast at boot.
        let risk_config = RiskConfig::new(dec!(1))
            .unwrap()
            .with_execution_costs(config.engine.taker_fee_rate, config.engine.stop_gap_bps)
            .expect("invalid ROBSON_TAKER_FEE_RATE / ROBSON_STOP_GAP_BPS configuration")
            .with_stop_buffer(config.engine.stop_buffer_bps)
            .expect("invalid ROBSON_STOP_BUFFER_BPS configuration");
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
        // Placeholder capital; will be updated from exchange in run().
        // Execution-cost buffer parameters are operator-configured and are
        // preserved across capital rebuilds (ADR-0039). Invalid values are a
        // config error: fail fast at boot.
        let risk_config = RiskConfig::new(dec!(1))
            .unwrap()
            .with_execution_costs(config.engine.taker_fee_rate, config.engine.stop_gap_bps)
            .expect("invalid ROBSON_TAKER_FEE_RATE / ROBSON_STOP_GAP_BPS configuration")
            .with_stop_buffer(config.engine.stop_buffer_bps)
            .expect("invalid ROBSON_STOP_BUFFER_BPS configuration");
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

    /// Initialize engine capital from the exchange balance.
    ///
    /// Queries the exchange for the USDT-M futures wallet balance and updates
    /// the engine's risk config. This ensures the engine uses real capital for
    /// position sizing rather than the constructor placeholder.
    ///
    /// Best-effort: logs a warning on failure and continues with the
    /// placeholder so the daemon can still start during exchange outages.
    async fn initialize_capital(&self) {
        match crate::api::refresh_capital_from_exchange(&self.exchange, &self.position_manager)
            .await
        {
            Ok(capital) => {
                info!(%capital, "Engine capital initialized from exchange balance");
            },
            Err(e) => {
                warn!(
                    error = %e,
                    "Failed to query exchange balance — keeping placeholder capital. \
                     Position sizing may be incorrect until the first arm request."
                );
            },
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

        // 0. Initialize capital from exchange (best-effort)
        self.initialize_capital().await;

        // 1. Rebuild store from event log (crash recovery)
        self.rebuild_store().await?;

        // 2. Invalidate durable query approvals that cannot be rehydrated safely.
        #[cfg(feature = "postgres")]
        self.invalidate_restart_pending_queries().await?;

        // 3. Restore active positions
        self.restore_positions().await?;

        // 3b. Re-spawn detector tasks for Armed positions (entry policy recovery).
        // entry_policies is in-memory only; restore it from the projection so
        // Armed positions don't silently stall after a restart.
        #[cfg(feature = "postgres")]
        self.restore_armed_detectors().await;

        // 3a. Startup stale-active gate (fail-closed; exit code 78 on abort).
        // Runs immediately — no grace period. Must precede the UNTRACKED scan.
        info!("Running startup stale-active gate");
        self.run_startup_stale_active_gate().await?;
        info!("Startup stale-active gate: passed");

        let reconciliation_interval = Duration::from_secs(self.config.reconciliation.interval_secs);
        let missing_grace = Duration::from_secs(self.config.reconciliation.missing_grace_secs);
        let startup_reconciliation = ReconciliationWorker::new_with_missing_grace(
            Arc::clone(&self.exchange),
            Arc::clone(&self.position_manager),
            Arc::clone(&self.store),
            Arc::clone(&self.event_bus),
            reconciliation_interval,
            missing_grace,
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

        // 3b. Startup recovery: catch-up missed stop hits during downtime.
        {
            let pm = self.position_manager.read().await;
            let ohlcv_port = pm.ohlcv_port();
            match crate::startup_recovery::run_startup_recovery(&pm, &ohlcv_port).await {
                Ok(report) => {
                    if report.positions_closed > 0 || report.stops_updated > 0 {
                        info!(%report, "Startup recovery applied catch-up actions");
                    } else {
                        info!("Startup recovery: no catch-up needed");
                    }
                },
                Err(e) => {
                    warn!(
                        error = %e,
                        "Startup recovery failed — continuing with live ticks only. \
                         Positions that crossed their stop during downtime will NOT be \
                         retroactively closed."
                    );
                },
            }
        }

        // 4. Initialize safety net monitor (when configured with Binance credentials)
        let position_monitor = self.initialize_position_monitor().await?;
        let position_monitor_handle =
            position_monitor.as_ref().map(|monitor| Arc::clone(monitor).start());

        // 5. Start API server
        let api_addr = self.start_api_server(position_monitor.clone()).await?;
        info!(%api_addr, "API server started");

        // 6. Spawn reconciliation worker (uses explicit missing_grace from config)
        let reconciliation_worker = ReconciliationWorker::new_with_missing_grace(
            Arc::clone(&self.exchange),
            Arc::clone(&self.position_manager),
            Arc::clone(&self.store),
            Arc::clone(&self.event_bus),
            reconciliation_interval,
            missing_grace,
            shutdown.clone(),
        );
        let reconciliation_handle = tokio::spawn(async move {
            if let Err(e) = reconciliation_worker.run().await {
                error!(error = %e, "Reconciliation worker failed");
            }
        });

        // 7. Spawn WebSocket clients (Phase 6: Market Data)
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

        // 8. Spawn projection worker (if pg_pool configured)
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

        #[cfg(feature = "postgres")]
        let funding_handle = if let (Some(pool), Some(tenant_id)) =
            (&self.pg_pool, self.config.projection.tenant_id)
        {
            let service = FundingService::new(
                pool.clone(),
                tenant_id,
                Arc::clone(&self.exchange),
                Arc::clone(&self.position_manager),
                self.config.funding.clone(),
            );
            let worker = FundingWorker::new(service);
            let worker_shutdown = shutdown.clone();
            Some(tokio::spawn(async move {
                if let Err(e) = worker.run(worker_shutdown).await {
                    error!(error = %e, "Funding worker failed");
                }
            }))
        } else {
            None
        };

        #[cfg(not(feature = "postgres"))]
        let funding_handle: Option<tokio::task::JoinHandle<()>> = None;

        // 9. Subscribe to event bus
        let mut event_receiver = self.event_bus.subscribe();

        // 10. Spawn ctrl+c handler
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

        // 11. Main event loop
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

        // 12. Graceful shutdown
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

        if let Some(handle) = funding_handle {
            info!("Waiting for funding worker to finish...");
            let _ = tokio::time::timeout(tokio::time::Duration::from_secs(10), handle).await;
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

            // Keep event_log/snapshots partitions ahead before appending the
            // reset event — the 2026-07 boundary failed because event_log had
            // no partition for the new month.
            sqlx::query("SELECT create_event_log_partitions(3)").execute(&**pool).await?;
            sqlx::query("SELECT create_snapshot_partitions(3)").execute(&**pool).await?;
        }

        let open_positions = self.store.positions().find_risk_open().await?;
        let carried_risk_committed = Self::calculate_carried_risk(&open_positions);

        // Armed positions have no measurable committed risk yet, but each will
        // be sized so the stop loss stays at or below 1% of capital when triggered.
        // Use the previous month's capital_base to avoid a circular month-boundary
        // calculation.
        let (prev_year, prev_month_num) = if now.month() == 1 {
            (now.year() - 1, 12u32)
        } else {
            (now.year(), now.month() - 1)
        };
        let prev_month = chrono::DateTime::<chrono::Utc>::from_naive_utc_and_offset(
            chrono::NaiveDate::from_ymd_opt(prev_year, prev_month_num, 1)
                .expect("previous month date must be valid")
                .and_hms_opt(0, 0, 0)
                .expect("midnight must be valid"),
            chrono::Utc,
        );
        let prev_capital = {
            let manager = self.position_manager.read().await;
            manager.load_capital_base_for_month(prev_month).await.unwrap_or(Decimal::ZERO)
        };
        let all_open_positions = self.store.positions().find_active().await?;
        let armed_count = all_open_positions
            .iter()
            .filter(|p| matches!(p.state, PositionState::Armed))
            .count() as u32;
        let armed_risk = prev_capital * Decimal::new(1, 2) * Decimal::from(armed_count);
        let carried_risk = carried_risk_committed + armed_risk;

        // current_equity per ADR-0024 §6: wallet balance from the exchange.
        // The capital_base is pessimistic: it subtracts carried_risk (worst case
        // loss from inherited positions) from current_equity. This guarantees
        // every month starts with 4 available slots.
        let current_equity = match self.exchange.get_futures_balance().await {
            Ok(balance) => balance.wallet_balance,
            Err(e) => {
                warn!(
                    error = %e,
                    "Failed to query exchange balance for month boundary — \
                     falling back to local equity estimate"
                );
                let manager = self.position_manager.read().await;
                manager.compute_current_equity().await?
            },
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
            %carried_risk_committed,
            armed_count,
            %armed_risk,
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

    /// Check for stale-Active positions at startup and abort if any are found.
    ///
    /// Runs immediately after `restore_positions()` and before the UNTRACKED
    /// scan. The startup gate is unconditional (no grace period). If any local
    /// `Active` position is absent from the exchange the daemon refuses to
    /// start (exit code 78). `Entering` and `Exiting` positions are ignored.
    async fn run_startup_stale_active_gate(&self) -> DaemonResult<()> {
        match self.config.reconciliation.on_startup_stale_active {
            StartupStaleActivePolicy::Abort => self.abort_if_stale_active().await,
            StartupStaleActivePolicy::AutoReconcile => self.run_startup_auto_reconcile().await,
        }
    }

    /// Two-phase startup auto-reconcile for stale-active positions.
    ///
    /// Phase 1 (read-only): gather real evidence for every stale-active
    /// position. Phase 2 (write): reconcile-close each position that passed
    /// Phase 1.
    ///
    /// Fail-closed: if Phase 1 fails for any position, no writes occur and the
    /// daemon exits with code 78.
    async fn run_startup_auto_reconcile(&self) -> DaemonResult<()> {
        // ------------------------------------------------------------------
        // Phase 0 — detect stale-active
        // ------------------------------------------------------------------
        let exchange_positions = self.exchange.get_all_open_positions().await?;
        let local_positions = self.store.positions().find_active().await?;

        let mut stale_actives: Vec<&robson_domain::Position> = Vec::new();
        for position in &local_positions {
            if !matches!(position.state, PositionState::Active { .. }) {
                continue;
            }
            let present_on_exchange = exchange_positions
                .iter()
                .any(|ep| ep.symbol == position.symbol && ep.side == position.side);
            if !present_on_exchange {
                stale_actives.push(position);
            }
        }

        if stale_actives.is_empty() {
            info!(
                count = 0,
                "Startup stale-active auto-reconcile: clean, no stale-active positions"
            );
            return Ok(());
        }

        info!(
            count = stale_actives.len(),
            "Startup stale-active auto-reconcile: detected stale-active positions"
        );

        // ------------------------------------------------------------------
        // Phase 1 — read-only evidence gathering (all-or-nothing)
        // ------------------------------------------------------------------
        let mut inputs: Vec<ReconciledCloseInput> = Vec::with_capacity(stale_actives.len());
        for position in &stale_actives {
            let observed_at_floor = position.entry_filled_at.unwrap_or(position.created_at);

            match gather_real_evidence(
                &self.exchange,
                &self.store,
                position,
                position.quantity,
                observed_at_floor,
            )
            .await?
            {
                Some(input) => {
                    let evidence_source = match &input.evidence {
                        robson_domain::ReconciliationEvidence::OrderFillRecord(_) => "order_fill",
                        robson_domain::ReconciliationEvidence::UserTradeRecord(_) => "user_trade",
                        other => {
                            let variant = match other {
                                robson_domain::ReconciliationEvidence::AccountSnapshot(_) => {
                                    "account_snapshot"
                                },
                                robson_domain::ReconciliationEvidence::Estimated(_) => "estimated",
                                _ => "unknown",
                            };
                            error!(
                                position_id = %position.id,
                                symbol = %position.symbol.as_pair(),
                                side = ?position.side,
                                evidence_source = %variant,
                                "Startup auto-reconcile: unsupported evidence type gathered in Phase 1"
                            );
                            let infos: Vec<StartupStaleActiveInfo> = stale_actives
                                .iter()
                                .map(|p| StartupStaleActiveInfo {
                                    position_id: p.id,
                                    symbol: p.symbol.as_pair(),
                                    side: format!("{:?}", p.side),
                                    quantity: p.quantity.as_decimal(),
                                    entry_price: p.entry_price.map(|pr| pr.as_decimal()),
                                })
                                .collect();
                            return Err(DaemonError::StartupStaleActiveDetected {
                                count: stale_actives.len(),
                                positions: infos,
                            });
                        },
                    };
                    info!(
                        position_id = %position.id,
                        symbol = %position.symbol.as_pair(),
                        side = ?position.side,
                        %evidence_source,
                        "Startup auto-reconcile: real evidence gathered"
                    );
                    inputs.push(input);
                },
                None => {
                    warn!(
                        position_id = %position.id,
                        symbol = %position.symbol.as_pair(),
                        side = ?position.side,
                        "Startup auto-reconcile: no unambiguous real evidence for stale-active position; continuing so the periodic reconciliation worker can resolve it"
                    );
                    return Ok(());
                },
            }
        }

        // ------------------------------------------------------------------
        // Phase 2 — apply reconcile-close for each validated input (fail-fast)
        // ------------------------------------------------------------------
        let stale_infos: Vec<StartupStaleActiveInfo> = stale_actives
            .iter()
            .map(|p| StartupStaleActiveInfo {
                position_id: p.id,
                symbol: p.symbol.as_pair(),
                side: format!("{:?}", p.side),
                quantity: p.quantity.as_decimal(),
                entry_price: p.entry_price.map(|pr| pr.as_decimal()),
            })
            .collect();
        self.apply_startup_auto_reconcile_batch(inputs, stale_infos).await?;

        info!(
            count = stale_actives.len(),
            "Startup stale-active auto-reconcile: all positions reconciled and closed"
        );
        Ok(())
    }

    /// Apply reconcile-close for a batch of validated inputs.
    ///
    /// Fail-fast: stops at the first rejection and does not process subsequent
    /// inputs. This reduces blast radius when an unexpected inconsistency
    /// occurs during startup reconciliation.
    async fn apply_startup_auto_reconcile_batch(
        &self,
        inputs: Vec<ReconciledCloseInput>,
        stale_infos: Vec<StartupStaleActiveInfo>,
    ) -> DaemonResult<()> {
        for input in inputs {
            let position_id = input.position_id;
            let outcome = {
                let manager = self.position_manager.read().await;
                manager.reconcile_close(input).await?
            };

            match outcome {
                ReconcileCloseOutcome::Closed | ReconcileCloseOutcome::AlreadyTerminal => {
                    info!(
                        position_id = %position_id,
                        outcome = ?outcome,
                        "Startup auto-reconcile: position closed"
                    );
                },
                ReconcileCloseOutcome::RejectedUnsupportedEvidence { source } => {
                    error!(
                        position_id = %position_id,
                        %source,
                        "Startup auto-reconcile CRITICAL: reconcile_close rejected unsupported evidence"
                    );
                    return Err(DaemonError::StartupStaleActiveDetected {
                        count: stale_infos.len(),
                        positions: stale_infos,
                    });
                },
                ReconcileCloseOutcome::RejectedInconsistentEvidence { field } => {
                    error!(
                        position_id = %position_id,
                        %field,
                        "Startup auto-reconcile CRITICAL: reconcile_close rejected inconsistent evidence"
                    );
                    return Err(DaemonError::StartupStaleActiveDetected {
                        count: stale_infos.len(),
                        positions: stale_infos,
                    });
                },
                ReconcileCloseOutcome::SkippedNonActive { state } => {
                    error!(
                        position_id = %position_id,
                        %state,
                        "Startup auto-reconcile CRITICAL: reconcile_close skipped non-active position"
                    );
                    return Err(DaemonError::StartupStaleActiveDetected {
                        count: stale_infos.len(),
                        positions: stale_infos,
                    });
                },
            }
        }
        Ok(())
    }

    async fn abort_if_stale_active(&self) -> DaemonResult<()> {
        let exchange_positions = self.exchange.get_all_open_positions().await?;
        let local_positions = self.store.positions().find_active().await?;

        let mut stale: Vec<StartupStaleActiveInfo> = Vec::new();

        for position in &local_positions {
            let present_on_exchange = exchange_positions
                .iter()
                .any(|ep| ep.symbol == position.symbol && ep.side == position.side);

            match &position.state {
                PositionState::Active { .. } if !present_on_exchange => {
                    error!(
                        position_id = %position.id,
                        symbol = %position.symbol.as_pair(),
                        side = ?position.side,
                        quantity = %position.quantity,
                        "CRITICAL: Startup gate: Robson-Active position absent from exchange"
                    );
                    stale.push(StartupStaleActiveInfo {
                        position_id: position.id,
                        symbol: position.symbol.as_pair(),
                        side: format!("{:?}", position.side),
                        quantity: position.quantity.as_decimal(),
                        entry_price: position.entry_price.map(|p| p.as_decimal()),
                    });
                },
                PositionState::Armed if present_on_exchange => {
                    error!(
                        position_id = %position.id,
                        symbol = %position.symbol.as_pair(),
                        side = ?position.side,
                        "CRITICAL: Startup gate: Armed position already appears on exchange"
                    );
                    stale.push(StartupStaleActiveInfo {
                        position_id: position.id,
                        symbol: position.symbol.as_pair(),
                        side: format!("{:?}", position.side),
                        quantity: position.quantity.as_decimal(),
                        entry_price: position.entry_price.map(|p| p.as_decimal()),
                    });
                },
                _ => {},
            }
        }

        if stale.is_empty() {
            info!("Startup stale-active gate: clean");
            return Ok(());
        }

        Err(DaemonError::StartupStaleActiveDetected { count: stale.len(), positions: stale })
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

    /// Re-spawn detector tasks for Armed positions after a daemon restart.
    ///
    /// entry_policies is an in-memory HashMap lost on every restart. This
    /// method reads entry_policy_resolved events directly from the eventlog
    /// (the source of truth, always consistent) rather than the projection
    /// (which may lag behind by one or more events when the daemon was
    /// killed mid-flight).
    #[cfg(feature = "postgres")]
    async fn restore_armed_detectors(&self) {
        let (Some(pool), Some(tenant_id)) = (&self.pg_pool, self.config.projection.tenant_id)
        else {
            return;
        };

        // Collect all Armed positions from the in-memory store (already rebuilt
        // by restore_positions()).
        let armed_positions: Vec<Position> = match self.store.positions().find_active().await {
            Ok(positions) => positions
                .into_iter()
                .filter(|p| matches!(p.state, PositionState::Armed))
                .collect(),
            Err(e) => {
                warn!(error = %e, "Could not read active positions; skipping detector restore");
                return;
            },
        };

        if armed_positions.is_empty() {
            info!("No Armed positions to restore detectors for");
            return;
        }

        for position in armed_positions {
            let position_id = position.id;
            let stream_key = format!("position:{}", position_id);

            // Read the most recent entry_policy_resolved event for this position
            // from the eventlog. Descending order so we get the latest one first.
            let events = match query_events(
                pool,
                QueryOptions::new(tenant_id)
                    .stream(stream_key)
                    .event_type("entry_policy_resolved")
                    .descending()
                    .limit(1),
            )
            .await
            {
                Ok(e) => e,
                Err(e) => {
                    warn!(%position_id, error = %e, "Eventlog query failed; skipping detector restore");
                    continue;
                },
            };

            let envelope: &EventEnvelope = match events.first() {
                Some(e) => e,
                None => {
                    warn!(%position_id, "No entry_policy_resolved event in eventlog; skipping detector restore");
                    continue;
                },
            };

            let entry_mode = envelope.payload.get("entry_policy").and_then(|v| v.as_str());
            let approval_mode = envelope.payload.get("approval_policy").and_then(|v| v.as_str());

            let entry_policy = match (entry_mode, approval_mode) {
                (Some(e), Some(a)) => match parse_entry_policy_config(e, a) {
                    Some(p) => p,
                    None => {
                        warn!(
                            %position_id,
                            entry_mode = %e,
                            approval_mode = %a,
                            "Unknown entry/approval mode in eventlog; skipping"
                        );
                        continue;
                    },
                },
                _ => {
                    warn!(%position_id, "Malformed entry_policy_resolved payload; skipping");
                    continue;
                },
            };

            let pm = self.position_manager.read().await;
            if let Err(e) = pm.restore_armed_position(&position, entry_policy).await {
                warn!(%position_id, error = %e, "Failed to restore detector for Armed position");
            }
        }
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
            exchange: self.exchange.clone(),
            position_manager: self.position_manager.clone(),
            event_bus: self.event_bus.clone(),
            circuit_breaker,
            position_monitor,
            wallet_balance_cache: tokio::sync::Mutex::new(None),
            #[cfg(feature = "postgres")]
            pg_pool: self.pg_pool.clone(),
            #[cfg(feature = "postgres")]
            tenant_id: self.config.projection.tenant_id,
            api_token: self.config.api.api_token.clone(),
            funding: self.config.funding.clone(),
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

            DaemonEvent::DomainEvent(event) => {
                let manager = self.position_manager.read().await;
                manager.emit_domain_event(event).await?;
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

            DaemonEvent::ReconciliationStaleNonActiveDetected {
                position_id,
                state,
                symbol,
                side,
                observed_at,
            } => {
                warn!(
                    %position_id,
                    %state,
                    %symbol,
                    ?side,
                    %observed_at,
                    "Reverse reconciliation detected stale non-Active position, skipped"
                );
            },

            DaemonEvent::ReconciliationStaleActiveUnresolved {
                position_id,
                symbol,
                side,
                first_observed_missing_at,
                confirmed_missing_at,
                reason,
            } => {
                error!(
                    %position_id,
                    %symbol,
                    ?side,
                    %first_observed_missing_at,
                    %confirmed_missing_at,
                    %reason,
                    "Reverse reconciliation stale Active unresolved"
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

            DaemonEvent::InsuranceStopOrphanCancelled {
                symbol,
                exchange_order_id,
                client_order_id,
            } => {
                warn!(
                    %symbol,
                    %exchange_order_id,
                    %client_order_id,
                    "Reconciliation cancelled an orphan insurance-stop order (ADR-0039)"
                );
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
    use robson_domain::{PositionState, Price, Quantity, RiskConfig, Side, TechnicalStopDistance};
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

    #[tokio::test]
    async fn test_first_poll_after_startup_always_verifies_month_boundary() {
        // Regression (2026-07 incident): seeding last_month_check with the
        // wall-clock month made a restart landing after a month boundary
        // skip that month's MonthBoundaryReset forever. The sentinel seed
        // must force the first poll to run the boundary check.
        let daemon = Daemon::new_stub(Config::test());

        let now = Utc.with_ymd_and_hms(2026, 7, 2, 8, 29, 4).single().unwrap();
        assert!(daemon.poll_month_boundary(now).await.unwrap());
        assert!(!daemon.poll_month_boundary(now).await.unwrap());
    }

    #[tokio::test]
    async fn test_month_boundary_includes_armed_risk_in_carried_risk() {
        let daemon = Daemon::new_stub(Config::test());
        {
            let manager = daemon.position_manager.read().await;
            manager
                .arm_position(
                    Symbol::from_pair("BTCUSDT").unwrap(),
                    Side::Long,
                    RiskConfig::new(dec!(10000)).unwrap(),
                    None,
                    uuid::Uuid::now_v7(),
                )
                .await
                .unwrap();
        }

        let now = Utc.with_ymd_and_hms(2026, 5, 1, 0, 0, 1).single().unwrap();
        daemon.handle_month_boundary(now).await.unwrap();

        let events = daemon.store.events().get_all_events().await.unwrap();
        let month_event = events
            .iter()
            .find_map(|event| match event {
                robson_domain::Event::MonthBoundaryReset {
                    capital_base,
                    carried_positions_risk,
                    month,
                    year,
                    ..
                } => Some((*capital_base, *carried_positions_risk, *month, *year)),
                _ => None,
            })
            .expect("month boundary event must be emitted");

        assert_eq!(month_event, (dec!(9900), dec!(100), 5, 2026));
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

    // -------------------------------------------------------------------------
    // TD-2026-05-05-001 Slice 5A — Startup stale-active gate tests
    // -------------------------------------------------------------------------

    fn active_position(symbol: robson_domain::Symbol, side: Side) -> robson_domain::Position {
        use rust_decimal_macros::dec;
        let mut pos = robson_domain::Position::new(uuid::Uuid::now_v7(), symbol, side);
        pos.entry_price = Some(Price::new(dec!(100)).unwrap());
        pos.quantity = Quantity::new(dec!(0.010)).unwrap();
        pos.state = PositionState::Active {
            current_price: Price::new(dec!(101)).unwrap(),
            trailing_stop: Price::new(dec!(99)).unwrap(),
            favorable_extreme: Price::new(dec!(101)).unwrap(),
            extreme_at: chrono::Utc::now(),
            insurance_stop_id: None,
            last_emitted_stop: None,
        };
        pos
    }

    #[tokio::test]
    async fn test_startup_gate_clean_no_positions() {
        let daemon = Daemon::new_stub(Config::test());
        // Store empty, exchange empty → gate passes.
        daemon.abort_if_stale_active().await.unwrap();
    }

    #[tokio::test]
    async fn test_startup_gate_stale_active_returns_typed_error() {
        use robson_domain::Symbol;
        let daemon = Daemon::new_stub(Config::test());

        let position = active_position(Symbol::from_pair("BTCUSDT").unwrap(), Side::Long);
        daemon.store.positions().save(&position).await.unwrap();
        // Exchange returns empty — position is stale-active.

        let err = daemon.abort_if_stale_active().await.unwrap_err();
        assert!(
            matches!(err, DaemonError::StartupStaleActiveDetected { count: 1, .. }),
            "expected StartupStaleActiveDetected with count 1, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn test_startup_gate_stale_active_does_not_mutate_store() {
        use robson_domain::Symbol;
        let daemon = Daemon::new_stub(Config::test());

        let position = active_position(Symbol::from_pair("BTCUSDT").unwrap(), Side::Long);
        let pid = position.id;
        daemon.store.positions().save(&position).await.unwrap();

        let _ = daemon.abort_if_stale_active().await;

        let stored = daemon.store.positions().find_by_id(pid).await.unwrap().unwrap();
        assert!(
            matches!(stored.state, PositionState::Active { .. }),
            "gate must not close or mutate the position"
        );
    }

    #[tokio::test]
    async fn test_startup_gate_armed_on_exchange_returns_typed_error() {
        use robson_domain::Symbol;
        let daemon = Daemon::new_stub(Config::test());
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();

        let mut position = active_position(symbol.clone(), Side::Long);
        let pid = position.id;
        position.state = PositionState::Armed;
        daemon.store.positions().save(&position).await.unwrap();
        daemon.exchange.set_open_position(
            symbol,
            Side::Long,
            position.quantity,
            position.entry_price.unwrap(),
        );

        let err = daemon.abort_if_stale_active().await.unwrap_err();
        assert!(
            matches!(err, DaemonError::StartupStaleActiveDetected { count: 1, .. }),
            "expected StartupStaleActiveDetected with count 1, got: {err:?}"
        );

        let stored = daemon.store.positions().find_by_id(pid).await.unwrap().unwrap();
        assert!(matches!(stored.state, PositionState::Armed));
    }

    #[tokio::test]
    async fn test_startup_gate_entering_does_not_abort() {
        use robson_domain::Symbol;
        let daemon = Daemon::new_stub(Config::test());

        let mut pos = active_position(Symbol::from_pair("BTCUSDT").unwrap(), Side::Long);
        pos.state = PositionState::Entering {
            entry_order_id: uuid::Uuid::now_v7(),
            expected_entry: Price::new(rust_decimal_macros::dec!(100)).unwrap(),
            signal_id: uuid::Uuid::now_v7(),
        };
        daemon.store.positions().save(&pos).await.unwrap();

        // Entering missing on exchange must NOT cause abort.
        daemon.abort_if_stale_active().await.unwrap();
    }

    #[tokio::test]
    async fn test_startup_gate_exiting_does_not_abort() {
        use robson_domain::{ExitReason, Symbol};
        let daemon = Daemon::new_stub(Config::test());

        let mut pos = active_position(Symbol::from_pair("BTCUSDT").unwrap(), Side::Long);
        pos.state = PositionState::Exiting {
            exit_order_id: uuid::Uuid::now_v7(),
            exit_reason: ExitReason::TrailingStop,
        };
        daemon.store.positions().save(&pos).await.unwrap();

        daemon.abort_if_stale_active().await.unwrap();
    }

    // -------------------------------------------------------------------------
    // TD-2026-05-05-001 Slice 5B2B — Startup auto-reconcile tests
    // -------------------------------------------------------------------------

    fn config_with_auto_reconcile() -> Config {
        let mut config = Config::test();
        config.reconciliation.on_startup_stale_active = StartupStaleActivePolicy::AutoReconcile;
        config
    }

    fn order_result(
        exchange_order_id: &str,
        price: rust_decimal::Decimal,
        quantity: rust_decimal::Decimal,
        filled_at: chrono::DateTime<chrono::Utc>,
    ) -> robson_exec::OrderResult {
        use robson_domain::{Price, Quantity};
        robson_exec::OrderResult {
            exchange_order_id: exchange_order_id.to_string(),
            client_order_id: format!("client-{exchange_order_id}"),
            fill_price: Price::new(price).unwrap(),
            filled_quantity: Quantity::new(quantity).unwrap(),
            fee: rust_decimal_macros::dec!(0.01),
            fee_asset: "USDT".to_string(),
            filled_at,
        }
    }

    fn user_trade(
        exchange_trade_id: &str,
        exchange_order_id: &str,
        price: rust_decimal::Decimal,
        quantity: rust_decimal::Decimal,
        filled_at: chrono::DateTime<chrono::Utc>,
    ) -> robson_exec::UserTradeRecord {
        use robson_domain::{Price, Quantity};
        robson_exec::UserTradeRecord {
            exchange_order_id: exchange_order_id.to_string(),
            exchange_trade_id: exchange_trade_id.to_string(),
            fill_price: Price::new(price).unwrap(),
            filled_quantity: Quantity::new(quantity).unwrap(),
            fee: rust_decimal_macros::dec!(0.01),
            fee_asset: "USDT".to_string(),
            filled_at,
        }
    }

    async fn attach_insurance_order(
        store: &std::sync::Arc<robson_store::MemoryStore>,
        position: &mut robson_domain::Position,
        exchange_order_id: &str,
    ) {
        use robson_domain::{Order, OrderSide, Price};
        let order = {
            let mut order = Order::new_stop_loss_limit(
                position.id,
                position.symbol.clone(),
                OrderSide::Sell,
                position.quantity,
                Price::new(rust_decimal_macros::dec!(99)).unwrap(),
                Price::new(rust_decimal_macros::dec!(99)).unwrap(),
            );
            order.exchange_order_id = Some(exchange_order_id.to_string());
            order
        };

        if let PositionState::Active { insurance_stop_id, .. } = &mut position.state {
            *insurance_stop_id = Some(exchange_order_id.to_string());
        }
        position.insurance_stop_id = Some(exchange_order_id.to_string());
        store.orders().save(&order).await.unwrap();
        store.positions().save(position).await.unwrap();
    }

    #[tokio::test]
    async fn test_startup_dispatch_abort_vs_auto_reconcile_is_observable() {
        use robson_domain::Symbol;
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();

        // --- Abort path ---
        let mut config_abort = Config::test();
        config_abort.reconciliation.on_startup_stale_active = StartupStaleActivePolicy::Abort;
        let daemon_abort = Daemon::new_stub(config_abort);

        let pos_abort = active_position(symbol.clone(), Side::Long);
        let pid_abort = pos_abort.id;
        daemon_abort.store.positions().save(&pos_abort).await.unwrap();

        let err = daemon_abort.run_startup_stale_active_gate().await.unwrap_err();
        assert!(
            matches!(err, DaemonError::StartupStaleActiveDetected { count: 1, .. }),
            "Abort policy must fail with stale-active, got: {err:?}"
        );
        let stored_abort =
            daemon_abort.store.positions().find_by_id(pid_abort).await.unwrap().unwrap();
        assert!(
            matches!(stored_abort.state, PositionState::Active { .. }),
            "Abort must leave position Active"
        );

        // --- AutoReconcile path ---
        let mut config_auto = Config::test();
        config_auto.reconciliation.on_startup_stale_active =
            StartupStaleActivePolicy::AutoReconcile;
        let daemon_auto = Daemon::new_stub(config_auto);

        let mut pos_auto = active_position(symbol.clone(), Side::Long);
        let pid_auto = pos_auto.id;
        attach_insurance_order(&daemon_auto.store, &mut pos_auto, "EX-ORDER-1").await;
        daemon_auto.store.positions().save(&pos_auto).await.unwrap();

        let now = chrono::Utc::now();
        daemon_auto
            .exchange
            .set_order_result("EX-ORDER-1", order_result("EX-ORDER-1", dec!(90), dec!(0.010), now));

        daemon_auto.run_startup_stale_active_gate().await.unwrap();
        let stored_auto =
            daemon_auto.store.positions().find_by_id(pid_auto).await.unwrap().unwrap();
        assert!(
            matches!(stored_auto.state, PositionState::Closed {
                exit_reason: robson_domain::ExitReason::ReconciledMissingOnExchange,
                ..
            }),
            "AutoReconcile policy must close stale-active with real evidence"
        );
    }

    #[tokio::test]
    async fn test_startup_auto_reconcile_zero_stale_active_returns_ok() {
        let daemon = Daemon::new_stub(config_with_auto_reconcile());
        daemon.run_startup_auto_reconcile().await.unwrap();
    }

    #[tokio::test]
    async fn test_startup_auto_reconcile_active_on_exchange_not_closed() {
        use robson_domain::Symbol;
        let daemon = Daemon::new_stub(config_with_auto_reconcile());
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();

        let position = active_position(symbol.clone(), Side::Long);
        let pid = position.id;
        daemon.store.positions().save(&position).await.unwrap();
        daemon.exchange.set_open_position(
            symbol,
            Side::Long,
            position.quantity,
            position.entry_price.unwrap(),
        );

        daemon.run_startup_auto_reconcile().await.unwrap();

        let stored = daemon.store.positions().find_by_id(pid).await.unwrap().unwrap();
        assert!(matches!(stored.state, PositionState::Active { .. }));
    }

    #[tokio::test]
    async fn test_startup_auto_reconcile_order_fill_closes_position() {
        use robson_domain::Symbol;
        let daemon = Daemon::new_stub(config_with_auto_reconcile());
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();

        let mut position = active_position(symbol.clone(), Side::Long);
        let pid = position.id;
        attach_insurance_order(&daemon.store, &mut position, "EX-ORDER-1").await;
        daemon.store.positions().save(&position).await.unwrap();

        let now = chrono::Utc::now();
        daemon
            .exchange
            .set_order_result("EX-ORDER-1", order_result("EX-ORDER-1", dec!(90), dec!(0.010), now));

        daemon.run_startup_auto_reconcile().await.unwrap();

        let stored = daemon.store.positions().find_by_id(pid).await.unwrap().unwrap();
        assert!(matches!(stored.state, PositionState::Closed {
            exit_reason: robson_domain::ExitReason::ReconciledMissingOnExchange,
            ..
        }));
    }

    #[tokio::test]
    async fn test_startup_auto_reconcile_user_trade_closes_position() {
        use robson_domain::Symbol;
        let daemon = Daemon::new_stub(config_with_auto_reconcile());
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();

        let position = active_position(symbol.clone(), Side::Long);
        let pid = position.id;
        daemon.store.positions().save(&position).await.unwrap();

        let now = chrono::Utc::now();
        daemon.exchange.set_user_trades(&symbol.as_pair(), vec![user_trade(
            "TRADE-1",
            "EX-ORDER-2",
            dec!(90),
            dec!(0.010),
            now,
        )]);

        daemon.run_startup_auto_reconcile().await.unwrap();

        let stored = daemon.store.positions().find_by_id(pid).await.unwrap().unwrap();
        assert!(matches!(stored.state, PositionState::Closed {
            exit_reason: robson_domain::ExitReason::ReconciledMissingOnExchange,
            ..
        }));
    }

    #[tokio::test]
    async fn test_startup_auto_reconcile_no_evidence_allows_startup() {
        use robson_domain::Symbol;
        let daemon = Daemon::new_stub(config_with_auto_reconcile());

        let position = active_position(Symbol::from_pair("BTCUSDT").unwrap(), Side::Long);
        let pid = position.id;
        daemon.store.positions().save(&position).await.unwrap();

        daemon.run_startup_auto_reconcile().await.unwrap();

        let stored = daemon.store.positions().find_by_id(pid).await.unwrap().unwrap();
        assert!(matches!(stored.state, PositionState::Active { .. }));
    }

    #[tokio::test]
    async fn test_startup_auto_reconcile_phase1_all_or_nothing() {
        use robson_domain::Symbol;
        let daemon = Daemon::new_stub(config_with_auto_reconcile());
        let symbol_a = Symbol::from_pair("BTCUSDT").unwrap();
        let symbol_b = Symbol::from_pair("ETHUSDT").unwrap();

        let mut pos_a = active_position(symbol_a.clone(), Side::Long);
        let pid_a = pos_a.id;
        attach_insurance_order(&daemon.store, &mut pos_a, "EX-ORDER-A").await;
        daemon.store.positions().save(&pos_a).await.unwrap();

        let pos_b = active_position(symbol_b.clone(), Side::Long);
        let pid_b = pos_b.id;
        daemon.store.positions().save(&pos_b).await.unwrap();

        let now = chrono::Utc::now();
        daemon
            .exchange
            .set_order_result("EX-ORDER-A", order_result("EX-ORDER-A", dec!(90), dec!(0.010), now));
        // pos_b has no evidence

        daemon.run_startup_auto_reconcile().await.unwrap();

        let stored_a = daemon.store.positions().find_by_id(pid_a).await.unwrap().unwrap();
        let stored_b = daemon.store.positions().find_by_id(pid_b).await.unwrap().unwrap();
        assert!(
            matches!(stored_a.state, PositionState::Active { .. }),
            "pos_a must remain Active while unresolved evidence is present"
        );
        assert!(
            matches!(stored_b.state, PositionState::Active { .. }),
            "pos_b must remain Active while unresolved evidence is present"
        );
    }

    #[tokio::test]
    async fn test_startup_auto_reconcile_entering_not_processed() {
        use robson_domain::Symbol;
        let daemon = Daemon::new_stub(config_with_auto_reconcile());

        let mut pos = active_position(Symbol::from_pair("BTCUSDT").unwrap(), Side::Long);
        pos.state = PositionState::Entering {
            entry_order_id: uuid::Uuid::now_v7(),
            expected_entry: Price::new(dec!(100)).unwrap(),
            signal_id: uuid::Uuid::now_v7(),
        };
        daemon.store.positions().save(&pos).await.unwrap();

        daemon.run_startup_auto_reconcile().await.unwrap();
    }

    #[tokio::test]
    async fn test_startup_auto_reconcile_exiting_not_processed() {
        use robson_domain::{ExitReason, Symbol};
        let daemon = Daemon::new_stub(config_with_auto_reconcile());

        let mut pos = active_position(Symbol::from_pair("BTCUSDT").unwrap(), Side::Long);
        pos.state = PositionState::Exiting {
            exit_order_id: uuid::Uuid::now_v7(),
            exit_reason: ExitReason::TrailingStop,
        };
        daemon.store.positions().save(&pos).await.unwrap();

        daemon.run_startup_auto_reconcile().await.unwrap();
    }

    // -------------------------------------------------------------------------
    // Phase 2 fail-fast tests via apply_startup_auto_reconcile_batch
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn test_startup_auto_reconcile_batch_rejected_unsupported_evidence() {
        use robson_domain::{AccountSnapshotEvidence, Price, Quantity, ReconciliationEvidence};
        let daemon = Daemon::new_stub(config_with_auto_reconcile());
        let symbol = robson_domain::Symbol::from_pair("BTCUSDT").unwrap();

        let position = active_position(symbol.clone(), Side::Long);
        let pid = position.id;
        daemon.store.positions().save(&position).await.unwrap();

        let now = chrono::Utc::now();
        let input = ReconciledCloseInput {
            position_id: pid,
            exit_price: Price::new(dec!(90)).unwrap(),
            filled_quantity: Quantity::new(dec!(0.010)).unwrap(),
            fee: dec!(0.01),
            fee_asset: "USDT".to_string(),
            closed_at: now,
            authored_client_order_id: None,
            evidence: ReconciliationEvidence::AccountSnapshot(AccountSnapshotEvidence {
                first_observed_missing_at: now,
                confirmed_missing_at: now,
                futures_balance_delta: None,
            }),
        };

        let err = daemon
            .apply_startup_auto_reconcile_batch(vec![input], vec![StartupStaleActiveInfo {
                position_id: pid,
                symbol: symbol.as_pair(),
                side: "Long".to_string(),
                quantity: dec!(0.010),
                entry_price: Some(dec!(100)),
            }])
            .await
            .unwrap_err();

        assert!(
            matches!(err, DaemonError::StartupStaleActiveDetected { count: 1, .. }),
            "expected fail-fast on unsupported evidence, got: {err:?}"
        );

        let stored = daemon.store.positions().find_by_id(pid).await.unwrap().unwrap();
        assert!(
            matches!(stored.state, PositionState::Active { .. }),
            "position must remain Active"
        );
    }

    #[tokio::test]
    async fn test_startup_auto_reconcile_batch_skipped_non_active() {
        use robson_domain::{
            ExitReason, OrderFillEvidence, Price, Quantity, ReconciliationEvidence,
        };
        let daemon = Daemon::new_stub(config_with_auto_reconcile());
        let symbol = robson_domain::Symbol::from_pair("BTCUSDT").unwrap();

        let mut position = active_position(symbol.clone(), Side::Long);
        let pid = position.id;
        daemon.store.positions().save(&position).await.unwrap();

        // Mutate to Exiting before calling the batch helper
        position.state = PositionState::Exiting {
            exit_order_id: uuid::Uuid::now_v7(),
            exit_reason: ExitReason::TrailingStop,
        };
        daemon.store.positions().save(&position).await.unwrap();

        let now = chrono::Utc::now();
        let input = ReconciledCloseInput {
            position_id: pid,
            exit_price: Price::new(dec!(90)).unwrap(),
            filled_quantity: Quantity::new(dec!(0.010)).unwrap(),
            fee: dec!(0.01),
            fee_asset: "USDT".to_string(),
            closed_at: now,
            authored_client_order_id: None,
            evidence: ReconciliationEvidence::OrderFillRecord(OrderFillEvidence {
                exchange_order_id: "EX-ORDER-1".to_string(),
                fill_price: Price::new(dec!(90)).unwrap(),
                filled_quantity: Quantity::new(dec!(0.010)).unwrap(),
                fee: dec!(0.01),
                fee_asset: "USDT".to_string(),
                filled_at: now,
            }),
        };

        let err = daemon
            .apply_startup_auto_reconcile_batch(vec![input], vec![StartupStaleActiveInfo {
                position_id: pid,
                symbol: symbol.as_pair(),
                side: "Long".to_string(),
                quantity: dec!(0.010),
                entry_price: Some(dec!(100)),
            }])
            .await
            .unwrap_err();

        assert!(
            matches!(err, DaemonError::StartupStaleActiveDetected { count: 1, .. }),
            "expected fail-fast on skipped non-active, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn test_startup_auto_reconcile_batch_rejected_inconsistent_evidence() {
        use robson_domain::{OrderFillEvidence, Price, Quantity, ReconciliationEvidence};
        let daemon = Daemon::new_stub(config_with_auto_reconcile());
        let symbol = robson_domain::Symbol::from_pair("BTCUSDT").unwrap();

        let position = active_position(symbol.clone(), Side::Long);
        let pid = position.id;
        daemon.store.positions().save(&position).await.unwrap();

        let now = chrono::Utc::now();
        // exit_price (95) != evidence.fill_price (90) → inconsistent
        let input = ReconciledCloseInput {
            position_id: pid,
            exit_price: Price::new(dec!(95)).unwrap(),
            filled_quantity: Quantity::new(dec!(0.010)).unwrap(),
            fee: dec!(0.01),
            fee_asset: "USDT".to_string(),
            closed_at: now,
            authored_client_order_id: None,
            evidence: ReconciliationEvidence::OrderFillRecord(OrderFillEvidence {
                exchange_order_id: "EX-ORDER-1".to_string(),
                fill_price: Price::new(dec!(90)).unwrap(),
                filled_quantity: Quantity::new(dec!(0.010)).unwrap(),
                fee: dec!(0.01),
                fee_asset: "USDT".to_string(),
                filled_at: now,
            }),
        };

        let err = daemon
            .apply_startup_auto_reconcile_batch(vec![input], vec![StartupStaleActiveInfo {
                position_id: pid,
                symbol: symbol.as_pair(),
                side: "Long".to_string(),
                quantity: dec!(0.010),
                entry_price: Some(dec!(100)),
            }])
            .await
            .unwrap_err();

        assert!(
            matches!(err, DaemonError::StartupStaleActiveDetected { count: 1, .. }),
            "expected fail-fast on inconsistent evidence, got: {err:?}"
        );

        let stored = daemon.store.positions().find_by_id(pid).await.unwrap().unwrap();
        assert!(
            matches!(stored.state, PositionState::Active { .. }),
            "position must remain Active"
        );
    }

    #[tokio::test]
    async fn test_startup_auto_reconcile_batch_fail_fast_stops_at_first_rejection() {
        use robson_domain::{
            AccountSnapshotEvidence, OrderFillEvidence, Price, Quantity, ReconciliationEvidence,
            Symbol,
        };
        let daemon = Daemon::new_stub(config_with_auto_reconcile());
        let symbol_a = Symbol::from_pair("BTCUSDT").unwrap();
        let symbol_b = Symbol::from_pair("ETHUSDT").unwrap();

        let pos_a = active_position(symbol_a.clone(), Side::Long);
        let pid_a = pos_a.id;
        daemon.store.positions().save(&pos_a).await.unwrap();

        let mut pos_b = active_position(symbol_b.clone(), Side::Long);
        let pid_b = pos_b.id;
        attach_insurance_order(&daemon.store, &mut pos_b, "EX-ORDER-B").await;
        daemon.store.positions().save(&pos_b).await.unwrap();

        let now = chrono::Utc::now();

        // First input: unsupported evidence (will be rejected)
        let input_a = ReconciledCloseInput {
            position_id: pid_a,
            exit_price: Price::new(dec!(90)).unwrap(),
            filled_quantity: Quantity::new(dec!(0.010)).unwrap(),
            fee: dec!(0.01),
            fee_asset: "USDT".to_string(),
            closed_at: now,
            authored_client_order_id: None,
            evidence: ReconciliationEvidence::AccountSnapshot(AccountSnapshotEvidence {
                first_observed_missing_at: now,
                confirmed_missing_at: now,
                futures_balance_delta: None,
            }),
        };

        // Second input: valid order-fill evidence (would close if executed)
        let input_b = ReconciledCloseInput {
            position_id: pid_b,
            exit_price: Price::new(dec!(90)).unwrap(),
            filled_quantity: Quantity::new(dec!(0.010)).unwrap(),
            fee: dec!(0.01),
            fee_asset: "USDT".to_string(),
            closed_at: now,
            authored_client_order_id: None,
            evidence: ReconciliationEvidence::OrderFillRecord(OrderFillEvidence {
                exchange_order_id: "EX-ORDER-B".to_string(),
                fill_price: Price::new(dec!(90)).unwrap(),
                filled_quantity: Quantity::new(dec!(0.010)).unwrap(),
                fee: dec!(0.01),
                fee_asset: "USDT".to_string(),
                filled_at: now,
            }),
        };

        let err = daemon
            .apply_startup_auto_reconcile_batch(vec![input_a, input_b], vec![
                StartupStaleActiveInfo {
                    position_id: pid_a,
                    symbol: symbol_a.as_pair(),
                    side: "Long".to_string(),
                    quantity: dec!(0.010),
                    entry_price: Some(dec!(100)),
                },
                StartupStaleActiveInfo {
                    position_id: pid_b,
                    symbol: symbol_b.as_pair(),
                    side: "Long".to_string(),
                    quantity: dec!(0.010),
                    entry_price: Some(dec!(100)),
                },
            ])
            .await
            .unwrap_err();

        assert!(
            matches!(err, DaemonError::StartupStaleActiveDetected { count: 2, .. }),
            "expected fail-fast on first rejection, got: {err:?}"
        );

        // pos_a was never going to be closed (rejected first)
        let stored_a = daemon.store.positions().find_by_id(pid_a).await.unwrap().unwrap();
        assert!(
            matches!(stored_a.state, PositionState::Active { .. }),
            "pos_a must remain Active"
        );

        // pos_b must also remain Active because the batch stopped at pos_a
        let stored_b = daemon.store.positions().find_by_id(pid_b).await.unwrap().unwrap();
        assert!(
            matches!(stored_b.state, PositionState::Active { .. }),
            "pos_b must remain Active (fail-fast)"
        );
    }
}
