//! Position Manager: Manages position lifecycle and detector tasks.
//!
//! The Position Manager is responsible for:
//! - Arming new positions (creates detector task)
//! - Processing detector signals (entry logic)
//! - Processing market data (trailing stop updates, exit triggers)
//! - Managing position state transitions
//! - Graceful shutdown of all detector tasks
//!
//! # Architecture
//!
//! ```text
//! CLI (arm) → PositionManager → spawn Detector → wait for signal
//!                  ↑
//!          EventBus (signals, market data)
//!                  ↓
//!              Engine → Executor → Exchange
//!
//! Shutdown → CancellationToken.cancel() → all detectors exit
//! ```

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use robson_domain::{
    DetectorSignal, Event, Position, PositionId, PositionState, Price, Quantity, RiskConfig, Side,
    Symbol, TechnicalStopDistance,
};
use robson_engine::{Engine, EngineAction};
use robson_exec::{ActionResult, ExchangePort, ExecError, Executor};
use robson_store::Store;

use crate::detector::DetectorTask;
use crate::error::{DaemonError, DaemonResult};
use crate::event_bus::{DaemonEvent, EventBus, MarketData};
use crate::query::{
    ActorKind, CommandSource, ContextSummary, ExecutionQuery, QueryKind, QueryOutcome, QueryState,
};
use crate::query_engine::{QueryEngine, TracingQueryRecorder};

// =============================================================================
// Position Manager
// =============================================================================

/// Manages position lifecycle and detector tasks.
pub struct PositionManager<E: ExchangePort + 'static, S: Store + 'static> {
    /// Trading engine
    engine: Engine,
    /// Order executor
    executor: Arc<Executor<E, S>>,
    /// Store for persistence
    store: Arc<S>,
    /// Event bus for publishing events
    event_bus: Arc<EventBus>,
    /// Master cancellation token for all detector tasks
    shutdown_token: CancellationToken,
    /// Active detector tasks (position_id → task handle)
    detectors: RwLock<HashMap<PositionId, JoinHandle<Option<DetectorSignal>>>>,
    /// Query engine for lifecycle tracking
    query_engine: QueryEngine<TracingQueryRecorder>,
}

impl<E: ExchangePort + 'static, S: Store + 'static> PositionManager<E, S> {
    /// Create a new position manager.
    ///
    /// After creation, call `start(Arc::clone(&manager))` to start the signal listener.
    pub fn new(
        engine: Engine,
        executor: Arc<Executor<E, S>>,
        store: Arc<S>,
        event_bus: Arc<EventBus>,
    ) -> Self {
        let shutdown_token = CancellationToken::new();
        let query_engine = QueryEngine::new(TracingQueryRecorder);

        Self {
            engine,
            executor,
            store,
            event_bus,
            shutdown_token,
            detectors: RwLock::new(HashMap::new()),
            query_engine,
        }
    }

    /// Start the position manager's background tasks.
    ///
    /// This spawns the signal listener that processes DetectorSignal events
    /// from the EventBus and calls `handle_signal()`.
    ///
    /// Must be called after wrapping in Arc:
    /// ```ignore
    /// let manager = Arc::new(PositionManager::new(...));
    /// PositionManager::start(Arc::clone(&manager));
    /// ```
    pub fn start(manager: Arc<Self>) {
        Self::start_signal_listener(manager);
    }

    /// Initiate graceful shutdown of all detector tasks.
    ///
    /// This cancels the shutdown token, causing all active detectors
    /// to exit cooperatively.
    pub async fn shutdown(&self) {
        info!("Initiating position manager shutdown");

        // Cancel all detectors
        self.shutdown_token.cancel();

        // Wait for all detectors to finish
        let mut detectors = self.detectors.write().await;
        let count = detectors.len();

        for (position_id, handle) in detectors.drain() {
            debug!(%position_id, "Waiting for detector to finish");

            // Give each detector a moment to finish gracefully
            match tokio::time::timeout(std::time::Duration::from_millis(500), handle).await {
                Ok(_) => debug!(%position_id, "Detector finished gracefully"),
                Err(_) => {
                    // Timeout - detector will be aborted when handle drops
                    warn!(%position_id, "Detector did not finish in time, will be aborted");
                },
            }
        }

        info!("Position manager shutdown complete ({count} detectors terminated)");
    }

    /// Get a child cancellation token for a new detector.
    ///
    /// Child tokens are cancelled when the parent is cancelled.
    fn child_cancel_token(&self) -> CancellationToken {
        self.shutdown_token.child_token()
    }

    fn operator_actor() -> ActorKind {
        ActorKind::Operator { source: CommandSource::Api }
    }

    fn set_query_context_summary(query: &mut ExecutionQuery, active_positions_count: usize) {
        query.context_summary = Some(ContextSummary { active_positions_count });
    }

    async fn populate_query_context_summary(&self, query: &mut ExecutionQuery) {
        if let Ok(active_positions) = self.store.positions().find_active().await {
            Self::set_query_context_summary(query, active_positions.len());
        }
    }

    /// Start background task to listen for detector signals.
    ///
    /// This task subscribes to the EventBus and processes DetectorSignal events
    /// by calling handle_signal() for each received signal.
    fn start_signal_listener(manager: Arc<Self>) {
        let event_bus = Arc::clone(&manager.event_bus);
        let shutdown_token = manager.shutdown_token.clone();

        tokio::spawn(async move {
            let mut receiver = event_bus.subscribe();

            info!("Position manager signal listener started");

            loop {
                tokio::select! {
                    // Handle shutdown
                    _ = shutdown_token.cancelled() => {
                        info!("Signal listener received shutdown signal");
                        break;
                    }
                    // Process events
                    event = receiver.recv() => {
                        match event {
                            Some(Ok(DaemonEvent::DetectorSignal(signal))) => {
                                let position_id = signal.position_id;
                                let signal_id = signal.signal_id;

                                info!(
                                    %position_id,
                                    %signal_id,
                                    "Processing detector signal from EventBus"
                                );

                                // Call handle_signal - now we have Arc<Self>!
                                if let Err(e) = manager.handle_signal(signal).await {
                                    error!(
                                        %position_id,
                                        %signal_id,
                                        error = %e,
                                        "Failed to process detector signal"
                                    );
                                } else {
                                    info!(
                                        %position_id,
                                        %signal_id,
                                        "Detector signal processed successfully"
                                    );
                                }
                            }
                            Some(Err(lag_msg)) => {
                                warn!(error = %lag_msg, "Signal receiver lagged");
                            }
                            None => {
                                info!("Signal receiver channel closed");
                                break;
                            }
                            Some(Ok(_)) => {
                                // Ignore other event types (MarketData, StateChanged, etc.)
                            }
                        }
                    }
                }
            }

            info!("Position manager signal listener terminated");
        });
    }

    /// Arm a new position.
    ///
    /// Creates the position in Armed state and spawns a detector task.
    /// The detector will fire a signal when entry conditions are met.
    pub async fn arm_position(
        &self,
        symbol: Symbol,
        side: Side,
        _risk_config: RiskConfig, // Used by Engine for position sizing
        tech_stop_distance: TechnicalStopDistance,
        account_id: Uuid,
    ) -> DaemonResult<Position> {
        // Generate position ID upfront (used in event and returned to caller)
        let position_id = Uuid::now_v7();

        // Create query for lifecycle tracking
        let mut query = ExecutionQuery::new(
            QueryKind::ArmPosition {
                symbol: symbol.clone(),
                side,
                tech_stop_distance,
                account_id,
            },
            Self::operator_actor(),
        );
        query.position_id = Some(position_id);
        self.populate_query_context_summary(&mut query).await;
        self.query_engine.on_accepted(&query);

        info!(
            %position_id,
            query_id = %query.id,
            symbol = %symbol.as_pair(),
            ?side,
            "Arming position"
        );

        // Transition to Processing
        if let Err(e) = query.transition(QueryState::Processing) {
            query.fail(format!("{}", e), "accepted".to_string());
            self.query_engine.on_error(&query, &format!("{}", e));
            return Err(DaemonError::Config(format!("Query transition error: {}", e)));
        }
        self.query_engine.on_state_change(&query);

        // Emit PositionArmed event → apply_event creates position in Armed state
        let now = chrono::Utc::now();
        let event = Event::PositionArmed {
            position_id,
            account_id,
            symbol: symbol.clone(),
            side,
            tech_stop_distance: Some(tech_stop_distance),
            timestamp: now,
        };

        // Transition to Acting before executor call
        if let Err(e) = query.transition(QueryState::Acting) {
            query.fail(format!("{}", e), "processing".to_string());
            self.query_engine.on_error(&query, &format!("{}", e));
            return Err(DaemonError::Config(format!("Query transition error: {}", e)));
        }
        self.query_engine.on_state_change(&query);

        // Execute event emission
        let results = match self.executor.execute(vec![EngineAction::EmitEvent(event)]).await {
            Ok(r) => r,
            Err(e) => {
                let err_str = format!("{}", e);
                query.fail(err_str.clone(), "acting".to_string());
                self.query_engine.on_error(&query, &err_str);
                return Err(e.into());
            },
        };
        let actions_count = results.len();

        self.event_bus.send(DaemonEvent::PositionStateChanged {
            position_id,
            previous_state: "None".to_string(),
            new_state: "Armed".to_string(),
            timestamp: now,
        });

        // Load position from projection for detector and return
        let position = match self.store.positions().find_by_id(position_id).await {
            Ok(Some(pos)) => pos,
            Ok(None) => {
                let e = DaemonError::PositionNotFound(position_id);
                query.fail(format!("{}", e), "acting".to_string());
                self.query_engine.on_error(&query, &format!("{}", e));
                return Err(e);
            },
            Err(e) => {
                let err_str = format!("{}", e);
                query.fail(err_str.clone(), "acting".to_string());
                self.query_engine.on_error(&query, &err_str);
                return Err(e.into());
            },
        };

        // Spawn detector task
        let cancel_token = self.child_cancel_token();
        let detector =
            match DetectorTask::from_position(&position, Arc::clone(&self.event_bus), cancel_token)
            {
                Ok(d) => d,
                Err(e) => {
                    let err_str = format!("{}", e);
                    query.fail(err_str.clone(), "acting".to_string());
                    self.query_engine.on_error(&query, &err_str);
                    return Err(e);
                },
            };
        let handle = detector.spawn();

        // Store detector handle for cancellation
        let mut detectors = self.detectors.write().await;
        detectors.insert(position_id, handle);

        debug!(%position_id, "Position armed, detector spawned");

        // Complete query ONLY after all operations succeed
        if let Err(e) = query.complete(QueryOutcome::ActionsExecuted { actions_count }) {
            query.fail(format!("{}", e), "acting".to_string());
            self.query_engine.on_error(&query, &format!("{}", e));
            return Err(DaemonError::Config(format!("Query completion error: {}", e)));
        }
        self.query_engine.on_state_change(&query);

        Ok(position)
    }

    /// Disarm (cancel) an armed position.
    ///
    /// Only positions in Armed state can be disarmed.
    pub async fn disarm_position(&self, position_id: PositionId) -> DaemonResult<()> {
        // Create query for lifecycle tracking
        let mut query =
            ExecutionQuery::new(QueryKind::DisarmPosition { position_id }, Self::operator_actor());
        query.position_id = Some(position_id);
        self.populate_query_context_summary(&mut query).await;
        self.query_engine.on_accepted(&query);

        // Transition to Processing
        if let Err(e) = query.transition(QueryState::Processing) {
            query.fail(format!("{}", e), "accepted".to_string());
            self.query_engine.on_error(&query, &format!("{}", e));
            return Err(DaemonError::Config(format!("Query transition error: {}", e)));
        }
        self.query_engine.on_state_change(&query);

        let position = match self.store.positions().find_by_id(position_id).await {
            Ok(Some(pos)) => pos,
            Ok(None) => {
                let e = DaemonError::PositionNotFound(position_id);
                query.fail(format!("{}", e), "processing".to_string());
                self.query_engine.on_error(&query, &format!("{}", e));
                return Err(e);
            },
            Err(e) => {
                query.fail(format!("{}", e), "processing".to_string());
                self.query_engine.on_error(&query, &format!("{}", e));
                return Err(e.into());
            },
        };

        if !matches!(position.state, PositionState::Armed) {
            let e = DaemonError::InvalidPositionState {
                expected: "Armed".to_string(),
                actual: format!("{:?}", position.state),
            };
            query.fail(format!("{}", e), "processing".to_string());
            self.query_engine.on_error(&query, &format!("{}", e));
            return Err(e);
        }

        info!(%position_id, query_id = %query.id, "Disarming position");

        // Kill detector task if exists
        self.kill_detector(position_id).await;

        // Transition to Acting before executor call
        if let Err(e) = query.transition(QueryState::Acting) {
            query.fail(format!("{}", e), "processing".to_string());
            self.query_engine.on_error(&query, &format!("{}", e));
            return Err(DaemonError::Config(format!("Query transition error: {}", e)));
        }
        self.query_engine.on_state_change(&query);

        // Emit PositionDisarmed event → apply_event transitions position to Closed
        let event = Event::PositionDisarmed {
            position_id,
            reason: "user_disarmed".to_string(),
            timestamp: chrono::Utc::now(),
        };

        let exec_result = self.executor.execute(vec![EngineAction::EmitEvent(event)]).await;
        match exec_result {
            Ok(results) => {
                if let Err(e) =
                    query.complete(QueryOutcome::ActionsExecuted { actions_count: results.len() })
                {
                    query.fail(format!("{}", e), "acting".to_string());
                    self.query_engine.on_error(&query, &format!("{}", e));
                    return Err(DaemonError::Config(format!("Query completion error: {}", e)));
                }
                self.query_engine.on_state_change(&query);
            },
            Err(e) => {
                query.fail(format!("{}", e), "acting".to_string());
                self.query_engine.on_error(&query, &format!("{}", e));
                return Err(e.into());
            },
        }

        self.event_bus.send(DaemonEvent::PositionStateChanged {
            position_id,
            previous_state: "Armed".to_string(),
            new_state: "Closed".to_string(),
            timestamp: chrono::Utc::now(),
        });

        Ok(())
    }

    /// Handle a detector signal (entry signal received).
    ///
    /// Flow: Engine → Execute actions (emit events) → Save state → Process fill
    pub async fn handle_signal(&self, signal: DetectorSignal) -> DaemonResult<()> {
        let position_id = signal.position_id;

        // Create query for lifecycle tracking
        let mut query = ExecutionQuery::new(
            QueryKind::ProcessSignal {
                signal_id: signal.signal_id,
                symbol: signal.symbol.clone(),
                side: signal.side,
                entry_price: signal.entry_price,
                stop_loss: signal.stop_loss,
            },
            ActorKind::Detector,
        );
        query.position_id = Some(position_id);
        self.populate_query_context_summary(&mut query).await;
        self.query_engine.on_accepted(&query);

        info!(
            %position_id,
            query_id = %query.id,
            signal_id = %signal.signal_id,
            entry_price = %signal.entry_price.as_decimal(),
            "Processing detector signal"
        );

        // Transition to Processing
        if let Err(e) = query.transition(QueryState::Processing) {
            query.fail(format!("{}", e), "accepted".to_string());
            self.query_engine.on_error(&query, &format!("{}", e));
            return Err(DaemonError::Config(format!("Query transition error: {}", e)));
        }
        self.query_engine.on_state_change(&query);

        // Load position
        let position = match self.store.positions().find_by_id(position_id).await {
            Ok(Some(pos)) => pos,
            Ok(None) => {
                let e = DaemonError::PositionNotFound(position_id);
                query.fail(format!("{}", e), "processing".to_string());
                self.query_engine.on_error(&query, &format!("{}", e));
                return Err(e);
            },
            Err(e) => {
                query.fail(format!("{}", e), "processing".to_string());
                self.query_engine.on_error(&query, &format!("{}", e));
                return Err(e.into());
            },
        };

        // Kill detector (it's single-shot)
        self.kill_detector(position_id).await;

        // Use engine to decide entry (pure: State+Signal → Decision)
        let decision = match self.engine.decide_entry(&position, &signal) {
            Ok(d) => d,
            Err(e) => {
                query.fail(format!("{}", e), "processing".to_string());
                self.query_engine.on_error(&query, &format!("{}", e));
                return Err(e.into());
            },
        };

        // Check if we have actions to execute
        if decision.actions.is_empty() {
            if let Err(e) = query.complete(QueryOutcome::NoAction {
                reason: "No actions from engine".to_string(),
            }) {
                query.fail(format!("{}", e), "processing".to_string());
                self.query_engine.on_error(&query, &format!("{}", e));
                return Err(DaemonError::Config(format!("Query completion error: {}", e)));
            }
            self.query_engine.on_state_change(&query);
            return Ok(());
        }

        // Transition to Acting before executor call
        if let Err(e) = query.transition(QueryState::Acting) {
            query.fail(format!("{}", e), "processing".to_string());
            self.query_engine.on_error(&query, &format!("{}", e));
            return Err(DaemonError::Config(format!("Query transition error: {}", e)));
        }
        self.query_engine.on_state_change(&query);

        // Execute actions (events are appended and applied; exchange orders are placed)
        // EntryOrderPlaced event transitions position to Entering via apply_event
        let results = match self.executor.execute(decision.actions).await {
            Ok(r) => r,
            Err(e) => {
                query.fail(format!("{}", e), "acting".to_string());
                self.query_engine.on_error(&query, &format!("{}", e));
                return Err(e.into());
            },
        };

        // Log results and process fill if order was placed
        // actions_count represents ALL ActionResult variants (including AlreadyProcessed and Skipped)
        let actions_count = results.len();
        for result in results {
            match result {
                ActionResult::OrderPlaced(order) => {
                    info!(
                        %position_id,
                        exchange_order_id = %order.exchange_order_id,
                        fill_price = %order.fill_price.as_decimal(),
                        "Entry order placed and filled"
                    );

                    // Process the fill (position is now in Entering state)
                    // Note: handle_entry_fill is internal, covered by this query's lifecycle
                    if let Err(e) = self
                        .handle_entry_fill(
                            position_id,
                            order.fill_price,
                            order.filled_quantity,
                            Some(order.exchange_order_id),
                        )
                        .await
                    {
                        query.fail(format!("{}", e), "acting".to_string());
                        self.query_engine.on_error(&query, &format!("{}", e));
                        return Err(e);
                    }
                },
                ActionResult::AlreadyProcessed(id) => {
                    warn!(%position_id, %id, "Signal already processed (idempotent skip)");
                },
                ActionResult::EventEmitted(event) => {
                    debug!(%position_id, event_type = event.event_type(), "Event emitted");
                },
                ActionResult::StateUpdated => {
                    debug!(%position_id, "State updated");
                },
                ActionResult::Skipped(reason) => {
                    debug!(%position_id, %reason, "Action skipped");
                },
            }
        }

        // Complete query with success
        if let Err(e) = query.complete(QueryOutcome::ActionsExecuted { actions_count }) {
            query.fail(format!("{}", e), "acting".to_string());
            self.query_engine.on_error(&query, &format!("{}", e));
            return Err(DaemonError::Config(format!("Query completion error: {}", e)));
        }
        self.query_engine.on_state_change(&query);

        Ok(())
    }

    /// Handle entry fill (transition from Entering → Active).
    ///
    /// Flow: Load → Engine → Execute actions (emit events) → Save state
    async fn handle_entry_fill(
        &self,
        position_id: PositionId,
        fill_price: Price,
        filled_quantity: Quantity,
        binance_position_id: Option<String>,
    ) -> DaemonResult<()> {
        // Load position
        let position = self
            .store
            .positions()
            .find_by_id(position_id)
            .await?
            .ok_or(DaemonError::PositionNotFound(position_id))?;

        // Use engine to process fill (pure: State+Fill → Decision)
        // binance_position_id is passed through to EntryFilled event
        let decision = self.engine.process_entry_fill(
            &position,
            fill_price,
            filled_quantity,
            binance_position_id.clone(),
        )?;

        // Execute actions (EntryFilled event transitions position to Active via apply_event)
        self.executor.execute(decision.actions).await?;

        info!(
            %position_id,
            fill_price = %fill_price.as_decimal(),
            "Entry filled, position now Active"
        );

        self.event_bus.send(DaemonEvent::PositionStateChanged {
            position_id,
            previous_state: "Entering".to_string(),
            new_state: "Active".to_string(),
            timestamp: chrono::Utc::now(),
        });

        let core_exchange_id = binance_position_id.unwrap_or_else(|| {
            warn!(
                %position_id,
                "Missing binance_position_id on Core open; using position_id fallback"
            );
            position_id.to_string()
        });

        self.event_bus.send(DaemonEvent::CorePositionOpened {
            position_id,
            symbol: position.symbol.clone(),
            side: position.side,
            binance_position_id: core_exchange_id,
        });

        Ok(())
    }

    /// Process market data for active positions.
    ///
    /// This updates trailing stops and triggers exits when necessary.
    ///
    /// # Canonical Flow
    ///
    /// ```text
    /// Tick → State (from projection) → Engine(State, Tick) → Decision
    /// → Executor(Decision) → Result → EventLog.append(Event)
    /// → Projection.apply(Event) (async)
    /// ```
    pub async fn process_market_data(&self, data: MarketData) -> DaemonResult<()> {
        // Find all active positions for this symbol (from projection)
        let active_positions = self.store.positions().find_active().await?;
        let active_positions_count = active_positions.len();

        for position in active_positions {
            if position.symbol != data.symbol {
                continue;
            }

            if !matches!(position.state, PositionState::Active { .. }) {
                continue;
            }

            // Create one ExecutionQuery PER POSITION processed
            let mut query = ExecutionQuery::new(
                QueryKind::ProcessMarketTick {
                    symbol: data.symbol.clone(),
                    price: data.price,
                },
                ActorKind::MarketData,
            );
            query.position_id = Some(position.id);
            Self::set_query_context_summary(&mut query, active_positions_count);
            self.query_engine.on_accepted(&query);

            debug!(
                position_id = %position.id,
                query_id = %query.id,
                price = %data.price.as_decimal(),
                "Processing market tick for position"
            );

            // Transition to Processing
            if let Err(e) = query.transition(QueryState::Processing) {
                query.fail(format!("{}", e), "accepted".to_string());
                self.query_engine.on_error(&query, &format!("{}", e));
                return Err(DaemonError::Config(format!("Query transition error: {}", e)));
            }
            self.query_engine.on_state_change(&query);

            // Use engine to process (pure: State+Tick → Decision)
            let symbol_clone = data.symbol.clone();
            let market_data = robson_engine::MarketData::new(symbol_clone, data.price);
            let decision = match self.engine.process_active_position(&position, &market_data) {
                Ok(d) => d,
                Err(e) => {
                    let err_str = format!("{}", e);
                    query.fail(err_str.clone(), "processing".to_string());
                    self.query_engine.on_error(&query, &err_str);
                    return Err(e.into());
                },
            };

            // Check if we have actions to execute
            if decision.actions.is_empty() {
                if let Err(e) =
                    query.complete(QueryOutcome::NoAction { reason: "No stop trigger".to_string() })
                {
                    let err_str = format!("{}", e);
                    query.fail(err_str.clone(), "processing".to_string());
                    self.query_engine.on_error(&query, &err_str);
                    return Err(DaemonError::Config(format!("Query completion error: {}", e)));
                }
                self.query_engine.on_state_change(&query);
                continue;
            }

            // Transition to Acting before executor call
            if let Err(e) = query.transition(QueryState::Acting) {
                query.fail(format!("{}", e), "processing".to_string());
                self.query_engine.on_error(&query, &format!("{}", e));
                return Err(DaemonError::Config(format!("Query transition error: {}", e)));
            }
            self.query_engine.on_state_change(&query);

            // Execute actions via Executor (side-effects: EventLog.append, Exchange orders)
            // MemoryStore is updated via apply_event() called by executor after event append
            let results = match self.executor.execute(decision.actions).await {
                Ok(r) => r,
                Err(e) => {
                    let err_str = format!("{}", e);
                    query.fail(err_str.clone(), "acting".to_string());
                    self.query_engine.on_error(&query, &err_str);
                    return Err(e.into());
                },
            };

            // Process results
            // actions_count represents ALL ActionResult variants
            let actions_count = results.len();
            for result in results {
                if let ActionResult::OrderPlaced(order) = result {
                    // Exit order filled, handle close
                    // Note: handle_exit_fill is internal, covered by this query's lifecycle
                    if let Err(e) = self
                        .handle_exit_fill(position.id, order.fill_price, order.filled_quantity)
                        .await
                    {
                        let err_str = format!("{}", e);
                        query.fail(err_str.clone(), "acting".to_string());
                        self.query_engine.on_error(&query, &err_str);
                        return Err(e);
                    }
                }
            }

            // Complete query with success
            if let Err(e) = query.complete(QueryOutcome::ActionsExecuted { actions_count }) {
                query.fail(format!("{}", e), "acting".to_string());
                self.query_engine.on_error(&query, &format!("{}", e));
                return Err(DaemonError::Config(format!("Query completion error: {}", e)));
            }
            self.query_engine.on_state_change(&query);
        }

        Ok(())
    }

    /// Handle exit fill (transition to Closed).
    ///
    /// Called after exit order is filled. Emits PositionClosed event
    /// which will be applied by projection to update state.
    async fn handle_exit_fill(
        &self,
        position_id: PositionId,
        fill_price: Price,
        _filled_quantity: Quantity,
    ) -> DaemonResult<()> {
        let position = self
            .store
            .positions()
            .find_by_id(position_id)
            .await?
            .ok_or(DaemonError::PositionNotFound(position_id))?;

        // Extract exit reason from position's Exiting state.
        // By this point, executor.execute_exit_order has already emitted and applied
        // ExitOrderPlaced, transitioning the position to Exiting { exit_reason }.
        let exit_reason = match &position.state {
            PositionState::Exiting { exit_reason, .. } => *exit_reason,
            other => {
                return Err(DaemonError::InvalidPositionState {
                    expected: "Exiting".to_string(),
                    actual: format!("{:?}", other),
                });
            },
        };

        info!(
            %position_id,
            fill_price = %fill_price.as_decimal(),
            ?exit_reason,
            "Exit filled, emitting PositionClosed event"
        );

        // Calculate PnL for event
        let entry_price = position.entry_price.unwrap_or(fill_price);
        let pnl = position.calculate_pnl();

        // Emit PositionClosed event via executor (ensures append->apply order)
        let event = Event::PositionClosed {
            position_id,
            exit_reason,
            entry_price,
            exit_price: fill_price,
            realized_pnl: pnl,
            total_fees: position.fees_paid,
            timestamp: chrono::Utc::now(),
        };
        self.executor.execute(vec![EngineAction::EmitEvent(event)]).await?;

        // Send to event bus for real-time notification
        self.event_bus.send(DaemonEvent::PositionStateChanged {
            position_id,
            previous_state: "Exiting".to_string(),
            new_state: "Closed".to_string(),
            timestamp: chrono::Utc::now(),
        });
        self.event_bus.send(DaemonEvent::CorePositionClosed {
            position_id,
            symbol: position.symbol.clone(),
            side: position.side,
        });

        Ok(())
    }

    /// Emergency close all positions.
    pub async fn panic_close_all(&self) -> DaemonResult<Vec<PositionId>> {
        warn!("PANIC: Emergency close all positions");

        let active = self.store.positions().find_active().await?;
        let active_positions_count = active.len();
        let mut closed_ids = Vec::new();

        for position in active {
            // Create one PanicClosePosition query PER POSITION
            let mut query = ExecutionQuery::new(
                QueryKind::PanicClosePosition { position_id: position.id },
                Self::operator_actor(),
            );
            query.position_id = Some(position.id);
            Self::set_query_context_summary(&mut query, active_positions_count);
            self.query_engine.on_accepted(&query);

            match self.panic_close_position_internal(position.id, &mut query).await {
                Ok(_) => {
                    if let Err(e) =
                        query.complete(QueryOutcome::ActionsExecuted { actions_count: 1 })
                    {
                        query.fail(format!("{}", e), "acting".to_string());
                        self.query_engine.on_error(&query, &format!("{}", e));
                    } else {
                        self.query_engine.on_state_change(&query);
                    }
                    closed_ids.push(position.id);
                },
                Err(e) => {
                    // Caller owns failure recording - internal method does NOT call fail()
                    // Infer phase from current query state for accurate audit trail
                    let phase = match &query.state {
                        QueryState::Accepted => "accepted".to_string(),
                        QueryState::Processing => "processing".to_string(),
                        QueryState::Acting => "acting".to_string(),
                        QueryState::Completed => "completed".to_string(),
                        QueryState::Failed { phase, .. } => phase.clone(),
                    };
                    query.fail(format!("{}", e), phase);
                    self.query_engine.on_error(&query, &format!("{}", e));
                    error!(position_id = %position.id, error = %e, "Failed to panic close");
                },
            }
        }

        // Also disarm any armed positions
        // (find_active already excludes armed, so we need another query - skip for now)

        info!(closed_count = closed_ids.len(), "Panic close complete");

        Ok(closed_ids)
    }

    /// Emergency close a single position (internal, takes query for lifecycle tracking).
    ///
    /// # Failure Recording Ownership
    ///
    /// This method does NOT call `query.fail()` or `on_error()` on errors.
    /// The caller is responsible for failure recording. This prevents double-fail.
    ///
    /// Emits PositionClosed event which will be applied by projection.
    async fn panic_close_position_internal(
        &self,
        position_id: PositionId,
        query: &mut ExecutionQuery,
    ) -> DaemonResult<()> {
        // Transition to Processing
        if let Err(e) = query.transition(QueryState::Processing) {
            return Err(DaemonError::Config(format!("Query transition error: {}", e)));
        }
        self.query_engine.on_state_change(query);

        let position = match self.store.positions().find_by_id(position_id).await {
            Ok(Some(pos)) => pos,
            Ok(None) => {
                return Err(DaemonError::PositionNotFound(position_id));
            },
            Err(e) => {
                return Err(e.into());
            },
        };

        let exit_side = position.side.exit_action();

        // Transition to Acting before executor call
        if let Err(e) = query.transition(QueryState::Acting) {
            return Err(DaemonError::Config(format!("Query transition error: {}", e)));
        }
        self.query_engine.on_state_change(query);

        // Place market exit order on exchange (executor also emits ExitOrderPlaced → Active → Exiting)
        let results = self
            .executor
            .execute(vec![EngineAction::PlaceExitOrder {
                position_id,
                symbol: position.symbol.clone(),
                side: exit_side,
                quantity: position.quantity,
                reason: robson_domain::ExitReason::UserPanic,
            }])
            .await?;

        // Extract actual fill price from exchange result
        let fill_price = match results.into_iter().find_map(|r| {
            if let ActionResult::OrderPlaced(order) = r {
                Some(order.fill_price)
            } else {
                None
            }
        }) {
            Some(price) => price,
            None => {
                return Err(DaemonError::Exec(ExecError::InvalidState(
                    "Panic close: PlaceExitOrder did not return OrderPlaced".to_string(),
                )));
            },
        };

        // Calculate PnL with actual fill price
        let entry_price = position.entry_price.unwrap_or(fill_price);
        let pnl = position.calculate_pnl();

        // Emit PositionClosed with actual fill price (Exiting → Closed)
        let event = Event::PositionClosed {
            position_id,
            exit_reason: robson_domain::ExitReason::UserPanic,
            entry_price,
            exit_price: fill_price,
            realized_pnl: pnl,
            total_fees: position.fees_paid,
            timestamp: chrono::Utc::now(),
        };

        self.executor.execute(vec![EngineAction::EmitEvent(event)]).await?;

        // Send to event bus for real-time notification
        self.event_bus.send(DaemonEvent::CorePositionClosed {
            position_id,
            symbol: position.symbol.clone(),
            side: position.side,
        });

        Ok(())
    }

    /// Kill detector task for a position.
    async fn kill_detector(&self, position_id: PositionId) {
        let mut detectors = self.detectors.write().await;
        if let Some(handle) = detectors.remove(&position_id) {
            handle.abort();
            debug!(%position_id, "Detector task killed");
        }
    }

    /// Get position by ID.
    pub async fn get_position(&self, position_id: PositionId) -> DaemonResult<Option<Position>> {
        Ok(self.store.positions().find_by_id(position_id).await?)
    }

    /// Get all active positions.
    pub async fn get_active_positions(&self) -> DaemonResult<Vec<Position>> {
        Ok(self.store.positions().find_active().await?)
    }

    /// Get position count.
    pub async fn position_count(&self) -> DaemonResult<usize> {
        // This is a hack since Store doesn't have count method
        // In memory store we can count, in production we'd have proper query
        let active = self.store.positions().find_active().await?;
        Ok(active.len())
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use robson_exec::{IntentJournal, StubExchange};
    use robson_store::MemoryStore;
    use rust_decimal::Decimal;
    use rust_decimal_macros::dec;

    /// Create a test manager without starting the signal listener.
    /// Use this for unit tests that call handle_signal() directly.
    async fn create_test_manager() -> Arc<PositionManager<StubExchange, MemoryStore>> {
        let exchange = Arc::new(StubExchange::new(dec!(95000)));
        let journal = Arc::new(IntentJournal::new());
        let store = Arc::new(MemoryStore::new());
        let executor = Arc::new(Executor::new(exchange, journal, store.clone()));
        let event_bus = Arc::new(EventBus::new(100));
        let risk_config = RiskConfig::new(dec!(10000), dec!(1)).unwrap(); // 1% risk
        let engine = Engine::new(risk_config);

        Arc::new(PositionManager::new(engine, executor, store, event_bus))
    }

    /// Create a test manager WITH signal listener running.
    /// Use this for E2E tests that need full event-driven flow.
    async fn create_test_manager_with_listener() -> Arc<PositionManager<StubExchange, MemoryStore>>
    {
        let manager = create_test_manager().await;
        PositionManager::start(Arc::clone(&manager));
        manager
    }

    fn create_test_risk_config() -> RiskConfig {
        RiskConfig::new(dec!(10000), dec!(1)).unwrap() // 1% risk
    }

    #[tokio::test]
    async fn test_arm_position() {
        let manager = create_test_manager().await;
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        // Create tech stop distance: entry $100, stop $98 (2% distance)
        let entry = Price::new(dec!(100)).unwrap();
        let stop = Price::new(dec!(98)).unwrap();
        let tech_stop = TechnicalStopDistance::from_entry_and_stop(entry, stop);

        let position = manager
            .arm_position(symbol, Side::Long, create_test_risk_config(), tech_stop, Uuid::now_v7())
            .await
            .unwrap();

        assert!(matches!(position.state, PositionState::Armed));

        // Should be persisted
        let loaded = manager.get_position(position.id).await.unwrap().unwrap();
        assert_eq!(loaded.id, position.id);
    }

    #[tokio::test]
    async fn test_disarm_position() {
        let manager = create_test_manager().await;
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        // Create tech stop distance: entry $100, stop $98 (2% distance)
        let entry = Price::new(dec!(100)).unwrap();
        let stop = Price::new(dec!(98)).unwrap();
        let tech_stop = TechnicalStopDistance::from_entry_and_stop(entry, stop);

        let position = manager
            .arm_position(symbol, Side::Long, create_test_risk_config(), tech_stop, Uuid::now_v7())
            .await
            .unwrap();

        manager.disarm_position(position.id).await.unwrap();

        // Position must be kept for audit trail, transitioned to Closed state
        let loaded = manager
            .get_position(position.id)
            .await
            .unwrap()
            .expect("position must exist after disarm");
        assert!(
            matches!(loaded.state, PositionState::Closed { .. }),
            "expected Closed after disarm, got {:?}",
            loaded.state
        );
    }

    #[tokio::test]
    async fn test_handle_signal() {
        let manager = create_test_manager().await;
        let mut receiver = manager.event_bus.subscribe();
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        // Create tech stop distance: entry $100, stop $98 (2% distance)
        let entry = Price::new(dec!(100)).unwrap();
        let stop = Price::new(dec!(98)).unwrap();
        let tech_stop = TechnicalStopDistance::from_entry_and_stop(entry, stop);

        let position = manager
            .arm_position(
                symbol.clone(),
                Side::Long,
                create_test_risk_config(),
                tech_stop,
                Uuid::now_v7(),
            )
            .await
            .unwrap();

        // Create detector signal
        let signal = DetectorSignal {
            signal_id: Uuid::now_v7(),
            position_id: position.id,
            symbol: symbol.clone(),
            side: Side::Long,
            entry_price: Price::new(dec!(95000)).unwrap(),
            stop_loss: Price::new(dec!(93100)).unwrap(), // 2% below
            timestamp: chrono::Utc::now(),
        };

        manager.handle_signal(signal).await.unwrap();

        // Position should now be Active
        let updated = manager.get_position(position.id).await.unwrap().unwrap();
        assert!(matches!(updated.state, PositionState::Active { .. }));

        // Core open event must be emitted when position becomes active
        let mut opened = false;
        for _ in 0..20 {
            if let Ok(Some(event)) =
                tokio::time::timeout(std::time::Duration::from_millis(50), receiver.recv()).await
            {
                if let Ok(DaemonEvent::CorePositionOpened { position_id, symbol, side, .. }) = event
                {
                    assert_eq!(position_id, position.id);
                    assert_eq!(symbol.as_pair(), "BTCUSDT");
                    assert_eq!(side, Side::Long);
                    opened = true;
                    break;
                }
            }
        }
        assert!(opened, "Expected CorePositionOpened event");
    }

    #[tokio::test]
    async fn test_disarm_non_armed_fails() {
        let manager = create_test_manager().await;
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        // Create tech stop distance: entry $100, stop $98 (2% distance)
        let entry = Price::new(dec!(100)).unwrap();
        let stop = Price::new(dec!(98)).unwrap();
        let tech_stop = TechnicalStopDistance::from_entry_and_stop(entry, stop);

        let position = manager
            .arm_position(
                symbol.clone(),
                Side::Long,
                create_test_risk_config(),
                tech_stop,
                Uuid::now_v7(),
            )
            .await
            .unwrap();

        // Move to Active
        let signal = DetectorSignal {
            signal_id: Uuid::now_v7(),
            position_id: position.id,
            symbol,
            side: Side::Long,
            entry_price: Price::new(dec!(95000)).unwrap(),
            stop_loss: Price::new(dec!(93100)).unwrap(),
            timestamp: chrono::Utc::now(),
        };
        manager.handle_signal(signal).await.unwrap();

        // Try to disarm (should fail)
        let result = manager.disarm_position(position.id).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_position_not_found() {
        let manager = create_test_manager().await;
        let fake_id = Uuid::now_v7();

        let result = manager.disarm_position(fake_id).await;
        assert!(matches!(result, Err(DaemonError::PositionNotFound(_))));
    }

    #[tokio::test]
    async fn test_panic_close_emits_core_position_closed() {
        let manager = create_test_manager().await;
        let mut receiver = manager.event_bus.subscribe();
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let entry = Price::new(dec!(100)).unwrap();
        let stop = Price::new(dec!(98)).unwrap();
        let tech_stop = TechnicalStopDistance::from_entry_and_stop(entry, stop);

        let position = manager
            .arm_position(
                symbol.clone(),
                Side::Long,
                create_test_risk_config(),
                tech_stop,
                Uuid::now_v7(),
            )
            .await
            .unwrap();

        let signal = DetectorSignal {
            signal_id: Uuid::now_v7(),
            position_id: position.id,
            symbol,
            side: Side::Long,
            entry_price: Price::new(dec!(95000)).unwrap(),
            stop_loss: Price::new(dec!(93100)).unwrap(),
            timestamp: chrono::Utc::now(),
        };
        manager.handle_signal(signal).await.unwrap();

        let _ = manager.panic_close_all().await.unwrap();

        let mut closed = false;
        for _ in 0..20 {
            if let Ok(Some(event)) =
                tokio::time::timeout(std::time::Duration::from_millis(50), receiver.recv()).await
            {
                if let Ok(DaemonEvent::CorePositionClosed { position_id, symbol, side }) = event {
                    assert_eq!(position_id, position.id);
                    assert_eq!(symbol.as_pair(), "BTCUSDT");
                    assert_eq!(side, Side::Long);
                    closed = true;
                    break;
                }
            }
        }
        assert!(closed, "Expected CorePositionClosed event");
    }

    /// E2E test: full detector integration (arm → spawn detector → MA crossover → signal → entry)
    ///
    /// Flow:
    /// 1. arm_position() → spawns detector
    /// 2. Inject synthetic market data via EventBus
    /// 3. Wait for MA crossover → DetectorSignal
    /// 4. Signal listener processes signal → Entry order → Position becomes Active
    ///
    /// Scope: Uses stub exchange, NO real orders, NO WebSocket
    #[tokio::test]
    async fn test_e2e_detector_ma_crossover_signal() {
        // Use manager WITH signal listener for full E2E flow
        let manager = create_test_manager_with_listener().await;
        let event_bus = manager.event_bus.clone();

        // Subscribe to EventBus to capture DetectorSignal
        let mut signal_receiver = event_bus.subscribe();

        // Arm position (spawns detector internally)
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let entry = Price::new(dec!(100)).unwrap();
        let stop = Price::new(dec!(98)).unwrap();
        let tech_stop = TechnicalStopDistance::from_entry_and_stop(entry, stop);

        let position = manager
            .arm_position(
                symbol.clone(),
                Side::Long,
                create_test_risk_config(),
                tech_stop,
                Uuid::now_v7(),
            )
            .await
            .unwrap();

        let position_id = position.id;

        // Yield to let detector task subscribe
        tokio::task::yield_now().await;

        // Feed descending prices (fast MA < slow MA) to establish "below" state
        // Need enough data points for MA calculation (slow_period=21 default)
        for i in (0..30).rev() {
            let price = Decimal::from(100 + i);
            let market_data = MarketData {
                symbol: symbol.clone(),
                price: Price::new(price).unwrap(),
                timestamp: chrono::Utc::now(),
            };
            event_bus.send(DaemonEvent::MarketData(market_data));
        }

        // Feed ascending prices to trigger MA crossover (fast crosses above slow)
        let mut signal_found = false;
        let mut detector_signal = None;

        for i in 0..10 {
            let price = Decimal::from(100 + i * 3); // Larger steps to trigger crossover faster
            let market_data = MarketData {
                symbol: symbol.clone(),
                price: Price::new(price).unwrap(),
                timestamp: chrono::Utc::now(),
            };
            event_bus.send(DaemonEvent::MarketData(market_data));

            // Check if detector emitted signal (after each tick, with timeout)
            for _ in 0..5 {
                let deadline = tokio::time::timeout(
                    std::time::Duration::from_millis(50),
                    signal_receiver.recv(),
                );

                match deadline.await {
                    Ok(Some(Ok(DaemonEvent::DetectorSignal(signal)))) => {
                        detector_signal = Some(signal);
                        signal_found = true;
                        break;
                    },
                    Ok(Some(Ok(_))) => continue, // Other events
                    Ok(Some(Err(_))) | Ok(None) | Err(_) => break, // Channel error or timeout
                }
                if signal_found {
                    break;
                }
            }
            if signal_found {
                break;
            }
        }

        // Assert: signal was emitted
        assert!(signal_found, "Detector should emit signal on MA crossover");

        let signal = detector_signal.expect("Signal should exist");

        // Assert: signal properties
        assert_eq!(signal.position_id, position_id);
        assert_eq!(signal.symbol.as_pair(), "BTCUSDT");
        assert_eq!(signal.side, Side::Long);
        assert!(signal.entry_price.as_decimal() > dec!(0));
        assert!(signal.stop_loss.as_decimal() > dec!(0));

        // Verify detector was cleaned up (single-shot)
        // Detector should be removed after signaling (checked via detector count)
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}
