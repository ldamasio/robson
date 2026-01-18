//! Executor: Orchestrates engine decisions to exchange actions.
//!
//! The Executor is the bridge between the pure Engine (decisions) and
//! the impure Exchange (I/O). It ensures idempotent execution via the
//! intent journal.
//!
//! # Flow
//!
//! ```text
//! Engine Decision → Executor → Intent Journal → Exchange → Result
//! ```

use std::sync::Arc;
use tracing::{debug, error, info};
use uuid::Uuid;

use robson_domain::{Event, ExitReason, Position, PositionId};
use robson_engine::EngineAction;
use robson_store::Store;

use crate::error::{ExecError, ExecResult};
use crate::intent::{Intent, IntentAction, IntentJournal, IntentResult};
use crate::ports::{ExchangePort, OrderResult};

// =============================================================================
// Execution Result
// =============================================================================

/// Result of executing an engine action.
#[derive(Debug)]
pub enum ActionResult {
    /// Order placed successfully
    OrderPlaced(OrderResult),
    /// Action was already processed (idempotent skip)
    AlreadyProcessed(Uuid),
    /// Event emitted (no exchange interaction)
    EventEmitted(Event),
    /// Position state updated (no exchange interaction)
    StateUpdated,
    /// Action skipped (e.g., no-op)
    Skipped(String),
}

// =============================================================================
// Executor
// =============================================================================

/// Fixed leverage for isolated margin trading.
pub const FIXED_LEVERAGE: u8 = 10;

/// Executes engine actions with idempotency guarantees.
///
/// The Executor:
/// 1. Receives actions from the Engine
/// 2. Records intents before execution
/// 3. **Validates margin settings before orders** (isolated + 10x)
/// 4. Executes via Exchange port
/// 5. Records results for audit trail
pub struct Executor<E: ExchangePort, S: Store> {
    /// Exchange port for placing orders
    exchange: Arc<E>,
    /// Intent journal for idempotency
    journal: Arc<IntentJournal>,
    /// Store for persisting positions, orders, events
    store: Arc<S>,
}

impl<E: ExchangePort, S: Store> Executor<E, S> {
    /// Create a new executor.
    pub fn new(exchange: Arc<E>, journal: Arc<IntentJournal>, store: Arc<S>) -> Self {
        Self { exchange, journal, store }
    }

    /// Execute a list of engine actions.
    ///
    /// Actions are executed in order. If one fails, subsequent actions
    /// are not executed.
    pub async fn execute(&self, actions: Vec<EngineAction>) -> ExecResult<Vec<ActionResult>> {
        let mut results = Vec::with_capacity(actions.len());

        for action in actions {
            let result = self.execute_action(action).await?;
            results.push(result);
        }

        Ok(results)
    }

    /// Execute a single engine action.
    async fn execute_action(&self, action: EngineAction) -> ExecResult<ActionResult> {
        match action {
            EngineAction::PlaceEntryOrder {
                position_id,
                symbol,
                side,
                quantity,
                signal_id,
            } => self.execute_entry_order(position_id, symbol, side, quantity, signal_id).await,

            EngineAction::PlaceExitOrder {
                position_id,
                symbol,
                side,
                quantity,
                reason,
            } => self.execute_exit_order(position_id, symbol, side, quantity, reason).await,

            EngineAction::UpdateTrailingStop {
                position_id,
                previous_stop,
                new_stop,
                trigger_price,
            } => {
                debug!(
                    %position_id,
                    previous = %previous_stop,
                    new = %new_stop,
                    trigger = %trigger_price,
                    "Trailing stop updated"
                );
                // No exchange interaction needed
                Ok(ActionResult::StateUpdated)
            },

            EngineAction::TriggerExit {
                position_id,
                reason,
                trigger_price,
                stop_price,
            } => {
                info!(
                    %position_id,
                    ?reason,
                    trigger = %trigger_price,
                    stop = %stop_price,
                    "Exit triggered"
                );
                // The actual exit order is placed via PlaceExitOrder action
                Ok(ActionResult::StateUpdated)
            },

            EngineAction::EmitEvent(event) => {
                debug!(
                    position_id = %event.position_id(),
                    event_type = event.event_type(),
                    "Emitting event"
                );

                // Persist event
                self.store.events().append(&event).await?;

                Ok(ActionResult::EventEmitted(event))
            },
        }
    }

    /// Execute entry order with idempotency.
    async fn execute_entry_order(
        &self,
        position_id: PositionId,
        symbol: robson_domain::Symbol,
        side: robson_domain::OrderSide,
        quantity: robson_domain::Quantity,
        signal_id: Uuid,
    ) -> ExecResult<ActionResult> {
        // 1. Check idempotency (signal_id is the intent ID)
        if let Some(existing) = self.journal.get(signal_id)? {
            if !existing.is_pending() {
                info!(
                    %signal_id,
                    %position_id,
                    "Entry order already processed, skipping"
                );
                return Ok(ActionResult::AlreadyProcessed(signal_id));
            }
        }

        // 2. SAFETY CHECK: Validate margin settings (isolated + 10x)
        // This MUST succeed before any order placement
        info!(
            %position_id,
            symbol = %symbol.as_pair(),
            "Validating margin settings (isolated + {}x)", FIXED_LEVERAGE
        );
        self.exchange.validate_margin_settings(&symbol, FIXED_LEVERAGE).await?;

        // 3. Record intent
        let intent = Intent::new(
            signal_id,
            position_id,
            IntentAction::PlaceEntryOrder { symbol: symbol.clone(), side, quantity },
        );

        if let Err(ExecError::AlreadyProcessed(id)) = self.journal.record(intent) {
            info!(%id, "Intent already recorded, checking status");
            if self.journal.is_processed(id)? {
                return Ok(ActionResult::AlreadyProcessed(id));
            }
        }

        // 4. Mark as executing
        self.journal.mark_executing(signal_id)?;

        info!(
            %position_id,
            %signal_id,
            symbol = %symbol.as_pair(),
            ?side,
            quantity = %quantity.as_decimal(),
            "Placing entry order"
        );

        // 5. Execute on exchange
        let result = self
            .exchange
            .place_market_order(&symbol, side, quantity, &signal_id.to_string())
            .await;

        // 6. Record result
        match &result {
            Ok(order_result) => {
                info!(
                    %position_id,
                    exchange_order_id = %order_result.exchange_order_id,
                    fill_price = %order_result.fill_price.as_decimal(),
                    "Entry order filled"
                );
                self.journal.complete(signal_id, IntentResult::Success(order_result.clone()))?;
            },
            Err(e) => {
                error!(%position_id, error = %e, "Entry order failed");
                self.journal.complete(signal_id, IntentResult::Failed(e.to_string()))?;
            },
        }

        result.map(ActionResult::OrderPlaced)
    }

    /// Execute exit order with idempotency.
    async fn execute_exit_order(
        &self,
        position_id: PositionId,
        symbol: robson_domain::Symbol,
        side: robson_domain::OrderSide,
        quantity: robson_domain::Quantity,
        reason: ExitReason,
    ) -> ExecResult<ActionResult> {
        // Generate unique intent ID for exit
        let intent_id = Uuid::now_v7();

        // 1. SAFETY CHECK: Validate margin settings (isolated + 10x)
        // Even for exits, we verify account state hasn't changed
        info!(
            %position_id,
            symbol = %symbol.as_pair(),
            "Validating margin settings for exit (isolated + {}x)", FIXED_LEVERAGE
        );
        self.exchange.validate_margin_settings(&symbol, FIXED_LEVERAGE).await?;

        // 2. Record intent
        let intent = Intent::new(
            intent_id,
            position_id,
            IntentAction::PlaceExitOrder {
                symbol: symbol.clone(),
                side,
                quantity,
                reason: reason.clone(),
            },
        );

        self.journal.record(intent)?;
        self.journal.mark_executing(intent_id)?;

        info!(
            %position_id,
            %intent_id,
            symbol = %symbol.as_pair(),
            ?side,
            quantity = %quantity.as_decimal(),
            ?reason,
            "Placing exit order"
        );

        // 3. Execute on exchange
        let result = self
            .exchange
            .place_market_order(&symbol, side, quantity, &intent_id.to_string())
            .await;

        // 4. Record result
        match &result {
            Ok(order_result) => {
                info!(
                    %position_id,
                    exchange_order_id = %order_result.exchange_order_id,
                    fill_price = %order_result.fill_price.as_decimal(),
                    ?reason,
                    "Exit order filled"
                );
                self.journal.complete(intent_id, IntentResult::Success(order_result.clone()))?;
            },
            Err(e) => {
                error!(%position_id, error = %e, "Exit order failed");
                self.journal.complete(intent_id, IntentResult::Failed(e.to_string()))?;
            },
        }

        result.map(ActionResult::OrderPlaced)
    }

    /// Get the intent journal (for inspection/recovery).
    pub fn journal(&self) -> &IntentJournal {
        &self.journal
    }

    /// Get the store (for state updates).
    pub fn store(&self) -> &S {
        &self.store
    }

    /// Update position in store after engine decision.
    pub async fn update_position(&self, position: &Position) -> ExecResult<()> {
        self.store.positions().save(position).await?;
        Ok(())
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stub::StubExchange;
    use robson_domain::{Quantity, Side, Symbol};
    use robson_store::MemoryStore;
    use rust_decimal_macros::dec;

    async fn create_test_executor() -> Executor<StubExchange, MemoryStore> {
        let exchange = Arc::new(StubExchange::new(dec!(95000)));
        let journal = Arc::new(IntentJournal::new());
        let store = Arc::new(MemoryStore::new());

        Executor::new(exchange, journal, store)
    }

    #[tokio::test]
    async fn test_execute_entry_order() {
        let executor = create_test_executor().await;

        let signal_id = Uuid::now_v7();
        let position_id = Uuid::now_v7();

        let action = EngineAction::PlaceEntryOrder {
            position_id,
            symbol: Symbol::from_pair("BTCUSDT").unwrap(),
            side: robson_domain::OrderSide::Buy,
            quantity: Quantity::new(dec!(0.1)).unwrap(),
            signal_id,
        };

        let results = executor.execute(vec![action]).await.unwrap();

        assert_eq!(results.len(), 1);
        match &results[0] {
            ActionResult::OrderPlaced(order) => {
                assert_eq!(order.fill_price.as_decimal(), dec!(95000));
                assert_eq!(order.filled_quantity.as_decimal(), dec!(0.1));
            },
            _ => panic!("Expected OrderPlaced"),
        }

        // Intent should be recorded and completed
        let intent = executor.journal.get(signal_id).unwrap().unwrap();
        assert!(intent.is_success());
    }

    #[tokio::test]
    async fn test_idempotent_entry_order() {
        let executor = create_test_executor().await;

        let signal_id = Uuid::now_v7();
        let position_id = Uuid::now_v7();

        let action = EngineAction::PlaceEntryOrder {
            position_id,
            symbol: Symbol::from_pair("BTCUSDT").unwrap(),
            side: robson_domain::OrderSide::Buy,
            quantity: Quantity::new(dec!(0.1)).unwrap(),
            signal_id,
        };

        // First execution
        let results1 = executor.execute(vec![action.clone()]).await.unwrap();
        assert!(matches!(results1[0], ActionResult::OrderPlaced(_)));

        // Second execution should be idempotent
        let results2 = executor.execute(vec![action]).await.unwrap();
        assert!(matches!(
            results2[0],
            ActionResult::AlreadyProcessed(id) if id == signal_id
        ));
    }

    #[tokio::test]
    async fn test_execute_emit_event() {
        let executor = create_test_executor().await;

        let event = Event::PositionArmed {
            position_id: Uuid::now_v7(),
            account_id: Uuid::now_v7(),
            symbol: Symbol::from_pair("BTCUSDT").unwrap(),
            side: Side::Long,
            timestamp: chrono::Utc::now(),
        };

        let action = EngineAction::EmitEvent(event.clone());

        let results = executor.execute(vec![action]).await.unwrap();

        assert_eq!(results.len(), 1);
        match &results[0] {
            ActionResult::EventEmitted(e) => {
                assert_eq!(e.position_id(), event.position_id());
            },
            _ => panic!("Expected EventEmitted"),
        }

        // Event should be persisted
        let events = executor.store.events().find_by_position(event.position_id()).await.unwrap();
        assert_eq!(events.len(), 1);
    }

    #[tokio::test]
    async fn test_execute_multiple_actions() {
        let executor = create_test_executor().await;

        let position_id = Uuid::now_v7();
        let signal_id = Uuid::now_v7();

        let actions = vec![
            EngineAction::EmitEvent(Event::PositionArmed {
                position_id,
                account_id: Uuid::now_v7(),
                symbol: Symbol::from_pair("BTCUSDT").unwrap(),
                side: Side::Long,
                timestamp: chrono::Utc::now(),
            }),
            EngineAction::PlaceEntryOrder {
                position_id,
                symbol: Symbol::from_pair("BTCUSDT").unwrap(),
                side: robson_domain::OrderSide::Buy,
                quantity: Quantity::new(dec!(0.1)).unwrap(),
                signal_id,
            },
        ];

        let results = executor.execute(actions).await.unwrap();

        assert_eq!(results.len(), 2);
        assert!(matches!(results[0], ActionResult::EventEmitted(_)));
        assert!(matches!(results[1], ActionResult::OrderPlaced(_)));
    }
}
