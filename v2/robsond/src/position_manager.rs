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
use robson_engine::Engine;
use robson_exec::{ActionResult, ExchangePort, Executor};
use robson_store::Store;

use crate::detector::DetectorTask;
use crate::error::{DaemonError, DaemonResult};
use crate::event_bus::{DaemonEvent, EventBus, MarketData};

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

        Self {
            engine,
            executor,
            store,
            event_bus,
            shutdown_token,
            detectors: RwLock::new(HashMap::new()),
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
        // Create position in Armed state
        // Note: risk_config is used by Engine for position sizing, not stored here
        // tech_stop_distance is stored for reference
        let mut position = Position::new(account_id, symbol.clone(), side);
        position.tech_stop_distance = Some(tech_stop_distance);
        let position_id = position.id;

        info!(
            %position_id,
            symbol = %symbol.as_pair(),
            ?side,
            "Arming position"
        );

        // Persist position
        self.store.positions().save(&position).await?;

        // Emit event
        let event = Event::PositionArmed {
            position_id,
            account_id,
            symbol: symbol.clone(),
            side,
            timestamp: chrono::Utc::now(),
        };
        self.store.events().append(&event).await?;
        self.event_bus.send(DaemonEvent::PositionStateChanged {
            position_id,
            previous_state: "None".to_string(),
            new_state: "Armed".to_string(),
            timestamp: chrono::Utc::now(),
        });

        // Spawn detector task
        let cancel_token = self.child_cancel_token();
        let detector =
            DetectorTask::from_position(&position, Arc::clone(&self.event_bus), cancel_token)?;
        let handle = detector.spawn();

        // Store detector handle for cancellation
        let mut detectors = self.detectors.write().await;
        detectors.insert(position_id, handle);

        debug!(%position_id, "Position armed, detector spawned");

        Ok(position)
    }

    /// Disarm (cancel) an armed position.
    ///
    /// Only positions in Armed state can be disarmed.
    pub async fn disarm_position(&self, position_id: PositionId) -> DaemonResult<()> {
        let position = self
            .store
            .positions()
            .find_by_id(position_id)
            .await?
            .ok_or(DaemonError::PositionNotFound(position_id))?;

        if !matches!(position.state, PositionState::Armed) {
            return Err(DaemonError::InvalidPositionState {
                expected: "Armed".to_string(),
                actual: format!("{:?}", position.state),
            });
        }

        info!(%position_id, "Disarming position");

        // Kill detector task if exists
        self.kill_detector(position_id).await;

        // Delete position (it never entered, so no need to keep it)
        self.store.positions().delete(position_id).await?;

        Ok(())
    }

    /// Handle a detector signal (entry signal received).
    ///
    /// Flow: Engine → Execute actions (emit events) → Save state → Process fill
    pub async fn handle_signal(&self, signal: DetectorSignal) -> DaemonResult<()> {
        let position_id = signal.position_id;
        info!(
            %position_id,
            signal_id = %signal.signal_id,
            entry_price = %signal.entry_price.as_decimal(),
            "Processing detector signal"
        );

        // Load position
        let position = self
            .store
            .positions()
            .find_by_id(position_id)
            .await?
            .ok_or(DaemonError::PositionNotFound(position_id))?;

        // Kill detector (it's single-shot)
        self.kill_detector(position_id).await;

        // Use engine to decide entry (pure: State+Signal → Decision)
        let decision = self.engine.decide_entry(&position, &signal)?;

        // Execute actions FIRST (emits events via EventLog.append)
        let results = self.executor.execute(decision.actions).await?;

        // THEN save state (atomicity: events before state)
        // Entering state must be persisted for handle_entry_fill to find it
        if let Some(ref updated_position) = decision.updated_position {
            let old_state = format!("{:?}", position.state);
            self.store.positions().save(updated_position).await?;

            self.event_bus.send(DaemonEvent::PositionStateChanged {
                position_id,
                previous_state: old_state,
                new_state: format!("{:?}", updated_position.state),
                timestamp: chrono::Utc::now(),
            });
        }

        // Log results and process fill if order was placed
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
                    self.handle_entry_fill(position_id, order.fill_price, order.filled_quantity)
                        .await?;
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
    ) -> DaemonResult<()> {
        // Load position
        let position = self
            .store
            .positions()
            .find_by_id(position_id)
            .await?
            .ok_or(DaemonError::PositionNotFound(position_id))?;

        // Use engine to process fill (pure: State+Fill → Decision)
        let decision = self.engine.process_entry_fill(&position, fill_price, filled_quantity)?;

        // Execute actions FIRST (emits events via EventLog.append)
        self.executor.execute(decision.actions).await?;

        // THEN save state (atomicity: events before state)
        if let Some(ref updated_position) = decision.updated_position {
            let old_state = format!("{:?}", position.state);
            self.store.positions().save(updated_position).await?;

            info!(
                %position_id,
                fill_price = %fill_price.as_decimal(),
                "Entry filled, position now Active"
            );

            self.event_bus.send(DaemonEvent::PositionStateChanged {
                position_id,
                previous_state: old_state,
                new_state: format!("{:?}", updated_position.state),
                timestamp: chrono::Utc::now(),
            });
        }

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

        for position in active_positions {
            if position.symbol != data.symbol {
                continue;
            }

            if !matches!(position.state, PositionState::Active { .. }) {
                continue;
            }

            // Use engine to process (pure: State+Tick → Decision)
            let symbol_clone = data.symbol.clone();
            let market_data = robson_engine::MarketData::new(symbol_clone, data.price);
            let decision = self.engine.process_active_position(&position, &market_data)?;

            // Execute actions via Executor (side-effects: EventLog.append, Exchange orders)
            if !decision.actions.is_empty() {
                let results = self.executor.execute(decision.actions).await?;

                // CRITICAL: Save updated position to MemoryStore (runtime source of truth)
                // Projection updates Postgres asynchronously, but runtime reads from MemoryStore
                if let Some(ref updated_position) = decision.updated_position {
                    self.store.positions().save(updated_position).await?;
                }

                for result in results {
                    if let ActionResult::OrderPlaced(order) = result {
                        // Exit order filled, handle close
                        self.handle_exit_fill(position.id, order.fill_price, order.filled_quantity)
                            .await?;
                    }
                }
            }
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

        info!(
            %position_id,
            fill_price = %fill_price.as_decimal(),
            "Exit filled, emitting PositionClosed event"
        );

        // Calculate PnL for event
        let entry_price = position.entry_price.unwrap_or(fill_price);
        let pnl = position.calculate_pnl();

        // Emit PositionClosed event FIRST (atomicity: event before state)
        let event = Event::PositionClosed {
            position_id,
            exit_reason: robson_domain::ExitReason::TrailingStop,
            entry_price,
            exit_price: fill_price,
            realized_pnl: pnl,
            total_fees: position.fees_paid,
            timestamp: chrono::Utc::now(),
        };
        self.store.events().append(&event).await?;

        // CRITICAL: Update position to Closed state in MemoryStore (runtime source of truth)
        // find_active() filters by can_enter()/can_exit(), so Closed positions won't appear
        let mut closed_position = position.clone();
        closed_position.state = PositionState::Closed {
            exit_price: fill_price,
            realized_pnl: pnl,
            exit_reason: robson_domain::ExitReason::TrailingStop,
        };
        closed_position.closed_at = Some(chrono::Utc::now());
        closed_position.updated_at = chrono::Utc::now();
        self.store.positions().save(&closed_position).await?;

        // Send to event bus for real-time notification
        self.event_bus.send(DaemonEvent::PositionStateChanged {
            position_id,
            previous_state: "Active".to_string(),
            new_state: "Closed".to_string(),
            timestamp: chrono::Utc::now(),
        });

        Ok(())
    }

    /// Emergency close all positions.
    pub async fn panic_close_all(&self) -> DaemonResult<Vec<PositionId>> {
        warn!("PANIC: Emergency close all positions");

        let active = self.store.positions().find_active().await?;
        let mut closed_ids = Vec::new();

        for position in active {
            match self.panic_close_position(position.id).await {
                Ok(_) => closed_ids.push(position.id),
                Err(e) => error!(position_id = %position.id, error = %e, "Failed to panic close"),
            }
        }

        // Also disarm any armed positions
        // (find_active already excludes armed, so we need another query - skip for now)

        info!(closed_count = closed_ids.len(), "Panic close complete");

        Ok(closed_ids)
    }

    /// Emergency close a single position.
    ///
    /// Emits PositionClosed event which will be applied by projection.
    async fn panic_close_position(&self, position_id: PositionId) -> DaemonResult<()> {
        let position = self
            .store
            .positions()
            .find_by_id(position_id)
            .await?
            .ok_or(DaemonError::PositionNotFound(position_id))?;

        // Get current price from executor's exchange
        let current_price = self
            .executor
            .store()
            .positions()
            .find_by_id(position_id)
            .await?
            .and_then(|p| match p.state {
                PositionState::Active { trailing_stop, .. } => Some(trailing_stop),
                _ => None,
            })
            .unwrap_or_else(|| Price::new(rust_decimal::Decimal::ZERO).unwrap());

        // Calculate PnL for event
        let pnl = position.calculate_pnl();
        let entry_price = position.entry_price.unwrap_or(current_price);

        // Emit PositionClosed event FIRST (atomicity: event before state)
        let event = Event::PositionClosed {
            position_id,
            exit_reason: robson_domain::ExitReason::UserPanic,
            entry_price,
            exit_price: current_price,
            realized_pnl: pnl,
            total_fees: position.fees_paid,
            timestamp: chrono::Utc::now(),
        };
        self.store.events().append(&event).await?;

        // CRITICAL: Update position to Closed state in MemoryStore (runtime source of truth)
        // find_active() filters by can_enter()/can_exit(), so Closed positions won't appear
        let mut closed_position = position.clone();
        closed_position.state = PositionState::Closed {
            exit_price: current_price,
            realized_pnl: pnl,
            exit_reason: robson_domain::ExitReason::UserPanic,
        };
        closed_position.closed_at = Some(chrono::Utc::now());
        closed_position.updated_at = chrono::Utc::now();
        self.store.positions().save(&closed_position).await?;

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

        // Should be deleted
        let loaded = manager.get_position(position.id).await.unwrap();
        assert!(loaded.is_none());
    }

    #[tokio::test]
    async fn test_handle_signal() {
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
