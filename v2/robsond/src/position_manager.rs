//! Position Manager: Manages position lifecycle and detector tasks.
//!
//! The Position Manager is responsible for:
//! - Arming new positions (creates detector task)
//! - Processing detector signals (entry logic)
//! - Processing market data (trailing stop updates, exit triggers)
//! - Managing position state transitions
//!
//! # Architecture
//!
//! ```text
//! CLI (arm) → PositionManager → spawn Detector → wait for signal
//!                  ↑
//!          EventBus (signals, market data)
//!                  ↓
//!              Engine → Executor → Exchange
//! ```

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use robson_domain::{
    DetectorSignal, Event, Position, PositionId, PositionState, Price, Quantity, RiskConfig, Side,
    Symbol, TechnicalStopDistance,
};
use robson_engine::Engine;
use robson_exec::{ActionResult, ExchangePort, Executor};
use robson_store::Store;

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
    /// Active detector tasks (position_id → task handle)
    detectors: RwLock<HashMap<PositionId, JoinHandle<()>>>,
}

impl<E: ExchangePort + 'static, S: Store + 'static> PositionManager<E, S> {
    /// Create a new position manager.
    pub fn new(
        engine: Engine,
        executor: Arc<Executor<E, S>>,
        store: Arc<S>,
        event_bus: Arc<EventBus>,
    ) -> Self {
        Self {
            engine,
            executor,
            store,
            event_bus,
            detectors: RwLock::new(HashMap::new()),
        }
    }

    /// Arm a new position.
    ///
    /// Creates the position in Armed state and spawns a detector task.
    /// The detector will fire a signal when entry conditions are met.
    pub async fn arm_position(
        &self,
        symbol: Symbol,
        side: Side,
        risk_config: RiskConfig,
        tech_stop_distance: TechnicalStopDistance,
        account_id: Uuid,
    ) -> DaemonResult<Position> {
        // Create position
        let position = Position::arm(symbol.clone(), side, risk_config, tech_stop_distance, account_id);
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

        // Note: In a real implementation, we would spawn a detector task here.
        // For now, detectors will be triggered externally (e.g., by tests or CLI).
        // The detector interface will be implemented in Phase 7.

        debug!(%position_id, "Position armed, waiting for detector signal");

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

        // Emit event
        let event = Event::PositionDisarmed {
            position_id,
            timestamp: chrono::Utc::now(),
        };
        self.store.events().append(&event).await?;

        Ok(())
    }

    /// Handle a detector signal (entry signal received).
    pub async fn handle_signal(&self, signal: DetectorSignal) -> DaemonResult<()> {
        let position_id = signal.position_id;
        info!(
            %position_id,
            signal_id = %signal.signal_id,
            entry_price = %signal.entry_price.as_decimal(),
            "Processing detector signal"
        );

        // Load position
        let mut position = self
            .store
            .positions()
            .find_by_id(position_id)
            .await?
            .ok_or(DaemonError::PositionNotFound(position_id))?;

        // Kill detector (it's single-shot)
        self.kill_detector(position_id).await;

        // Use engine to decide entry
        let decision = self.engine.decide_entry(&position, &signal)?;

        // Apply state transition
        if let Some(new_state) = decision.new_state {
            let old_state = format!("{:?}", position.state);
            position.state = new_state;
            self.store.positions().save(&position).await?;

            self.event_bus.send(DaemonEvent::PositionStateChanged {
                position_id,
                previous_state: old_state,
                new_state: format!("{:?}", position.state),
                timestamp: chrono::Utc::now(),
            });
        }

        // Execute actions
        let results = self.executor.execute(decision.actions).await?;

        // Log results
        for result in results {
            match result {
                ActionResult::OrderPlaced(order) => {
                    info!(
                        %position_id,
                        exchange_order_id = %order.exchange_order_id,
                        fill_price = %order.fill_price.as_decimal(),
                        "Entry order placed and filled"
                    );

                    // Process the fill
                    self.handle_entry_fill(position_id, order.fill_price, order.filled_quantity)
                        .await?;
                }
                ActionResult::AlreadyProcessed(id) => {
                    warn!(%position_id, %id, "Signal already processed (idempotent skip)");
                }
                ActionResult::EventEmitted(event) => {
                    debug!(%position_id, event_type = event.event_type(), "Event emitted");
                }
                ActionResult::StateUpdated => {
                    debug!(%position_id, "State updated");
                }
                ActionResult::Skipped(reason) => {
                    debug!(%position_id, %reason, "Action skipped");
                }
            }
        }

        Ok(())
    }

    /// Handle entry fill (transition from Entering → Active).
    async fn handle_entry_fill(
        &self,
        position_id: PositionId,
        fill_price: Price,
        filled_quantity: Quantity,
    ) -> DaemonResult<()> {
        // Load position
        let mut position = self
            .store
            .positions()
            .find_by_id(position_id)
            .await?
            .ok_or(DaemonError::PositionNotFound(position_id))?;

        // Use engine to process fill
        let decision = self.engine.process_entry_fill(&position, fill_price, filled_quantity)?;

        // Apply state transition
        if let Some(new_state) = decision.new_state {
            let old_state = format!("{:?}", position.state);
            position.state = new_state;
            self.store.positions().save(&position).await?;

            info!(
                %position_id,
                fill_price = %fill_price.as_decimal(),
                "Entry filled, position now Active"
            );

            self.event_bus.send(DaemonEvent::PositionStateChanged {
                position_id,
                previous_state: old_state,
                new_state: format!("{:?}", position.state),
                timestamp: chrono::Utc::now(),
            });
        }

        // Execute actions (events, etc.)
        self.executor.execute(decision.actions).await?;

        Ok(())
    }

    /// Process market data for active positions.
    ///
    /// This updates trailing stops and triggers exits when necessary.
    pub async fn process_market_data(&self, data: MarketData) -> DaemonResult<()> {
        // Find all active positions for this symbol
        let active_positions = self.store.positions().find_active().await?;

        for position in active_positions {
            if position.symbol != data.symbol {
                continue;
            }

            if !matches!(position.state, PositionState::Active { .. }) {
                continue;
            }

            // Use engine to process
            let decision = self
                .engine
                .process_active_position(&position, data.price)?;

            // Apply state transition if any
            if let Some(new_state) = decision.new_state {
                let old_state = format!("{:?}", position.state);
                let mut updated_position = position.clone();
                updated_position.state = new_state;
                self.store.positions().save(&updated_position).await?;

                self.event_bus.send(DaemonEvent::PositionStateChanged {
                    position_id: position.id,
                    previous_state: old_state,
                    new_state: format!("{:?}", updated_position.state),
                    timestamp: chrono::Utc::now(),
                });
            }

            // Execute any actions
            if !decision.actions.is_empty() {
                let results = self.executor.execute(decision.actions).await?;

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
    async fn handle_exit_fill(
        &self,
        position_id: PositionId,
        fill_price: Price,
        _filled_quantity: Quantity,
    ) -> DaemonResult<()> {
        let mut position = self
            .store
            .positions()
            .find_by_id(position_id)
            .await?
            .ok_or(DaemonError::PositionNotFound(position_id))?;

        info!(
            %position_id,
            fill_price = %fill_price.as_decimal(),
            "Exit filled, position closed"
        );

        // Transition to Closed
        position.state = PositionState::Closed {
            exit_price: fill_price,
            exit_reason: robson_domain::ExitReason::TrailingStopHit,
            closed_at: chrono::Utc::now(),
        };
        position.closed_at = Some(chrono::Utc::now());

        self.store.positions().save(&position).await?;

        // Emit closed event
        let pnl = position.calculate_pnl();
        let event = Event::PositionClosed {
            position_id,
            exit_price: fill_price,
            exit_reason: robson_domain::ExitReason::TrailingStopHit,
            pnl,
            timestamp: chrono::Utc::now(),
        };
        self.store.events().append(&event).await?;

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

        // Mark as closed with panic reason
        let mut closed_position = position.clone();
        closed_position.state = PositionState::Closed {
            exit_price: current_price,
            exit_reason: robson_domain::ExitReason::PanicClose,
            closed_at: chrono::Utc::now(),
        };
        closed_position.closed_at = Some(chrono::Utc::now());

        self.store.positions().save(&closed_position).await?;

        // Emit panic event
        let event = Event::PanicClose {
            position_id,
            timestamp: chrono::Utc::now(),
        };
        self.store.events().append(&event).await?;

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
    use rust_decimal_macros::dec;

    async fn create_test_manager(
    ) -> PositionManager<StubExchange, MemoryStore> {
        let exchange = Arc::new(StubExchange::new(dec!(95000)));
        let journal = Arc::new(IntentJournal::new());
        let store = Arc::new(MemoryStore::new());
        let executor = Arc::new(Executor::new(exchange, journal, store.clone()));
        let event_bus = Arc::new(EventBus::new(100));
        let engine = Engine::new();

        PositionManager::new(engine, executor, store, event_bus)
    }

    fn create_test_risk_config() -> RiskConfig {
        RiskConfig::new(dec!(10000), dec!(0.01), dec!(0.05)).unwrap()
    }

    #[tokio::test]
    async fn test_arm_position() {
        let manager = create_test_manager().await;
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let tech_stop = TechnicalStopDistance::new(dec!(0.02)).unwrap();

        let position = manager
            .arm_position(
                symbol,
                Side::Long,
                create_test_risk_config(),
                tech_stop,
                Uuid::now_v7(),
            )
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
        let tech_stop = TechnicalStopDistance::new(dec!(0.02)).unwrap();

        let position = manager
            .arm_position(
                symbol,
                Side::Long,
                create_test_risk_config(),
                tech_stop,
                Uuid::now_v7(),
            )
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
        let tech_stop = TechnicalStopDistance::new(dec!(0.02)).unwrap();

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
        let tech_stop = TechnicalStopDistance::new(dec!(0.02)).unwrap();

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
}
