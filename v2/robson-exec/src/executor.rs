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

use robson_domain::{Event, ExitReason, Position, PositionId, RiskConfig};
use robson_engine::EngineAction;
use robson_store::Store;
use tracing::{debug, error, info};
use uuid::Uuid;

use crate::{
    error::{ExecError, ExecResult},
    intent::{Intent, IntentAction, IntentJournal, IntentResult},
    ports::{ExchangePort, OrderResult},
};

// =============================================================================
// Execution Result
// =============================================================================

/// Result of executing an engine action.
#[derive(Debug)]
pub enum ActionResult {
    /// Order placed successfully, optionally with a domain event emitted.
    /// For exit orders, the event is ExitOrderPlaced which transitions position
    /// to Exiting.
    /// For entry orders, the event is EntryOrderAccepted (post-exchange ack).
    OrderPlaced {
        order: OrderResult,
        event: Option<Event>,
    },
    /// Entry order failed on exchange. Event (EntryOrderFailed) was applied to
    /// the in-memory store. The caller is responsible for persisting the event
    /// to the PostgreSQL eventlog via execute_and_persist().
    OrderFailed { event: Event, error: String },
    /// Entry execution was rejected by an internal safety or policy check
    /// before any exchange placement was attempted. The event
    /// (EntryExecutionRejected) was applied to the in-memory store. The caller
    /// is responsible for persisting it to the PostgreSQL eventlog.
    EntryExecutionRejected { event: Event, error: String },
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

/// Executes engine actions with idempotency guarantees.
///
/// The Executor:
/// 1. Receives actions from the Engine
/// 2. Records intents before execution
/// 3. **Validates futures settings before orders** (One-way mode + leverage)
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
            let should_stop = matches!(
                result,
                ActionResult::OrderFailed { .. } | ActionResult::EntryExecutionRejected { .. }
            );
            results.push(result);
            if should_stop {
                break;
            }
        }

        Ok(results)
    }

    /// Execute a single engine action.
    async fn execute_action(&self, action: EngineAction) -> ExecResult<ActionResult> {
        match action {
            EngineAction::PlaceEntryOrder {
                position_id,
                cycle_id,
                symbol,
                side,
                quantity,
                order_id,
                client_order_id,
                expected_price,
                signal_id,
            } => {
                self.execute_entry_order(
                    position_id,
                    cycle_id,
                    symbol,
                    side,
                    quantity,
                    order_id,
                    client_order_id,
                    expected_price,
                    signal_id,
                )
                .await
            },

            EngineAction::PlaceExitOrder {
                position_id,
                cycle_id,
                symbol,
                side,
                quantity,
                reason,
            } => {
                self.execute_exit_order(position_id, cycle_id, symbol, side, quantity, reason)
                    .await
            },

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

                // Persist event FIRST (EventLog is source of truth)
                self.store.events().append(&event).await?;

                // THEN apply to in-memory projection (updates positions)
                self.store.apply_event(&event)?;

                Ok(ActionResult::EventEmitted(event))
            },
        }
    }

    /// Execute entry order with idempotency.
    async fn execute_entry_order(
        &self,
        position_id: PositionId,
        cycle_id: Option<Uuid>,
        symbol: robson_domain::Symbol,
        side: robson_domain::OrderSide,
        quantity: robson_domain::Quantity,
        order_id: robson_domain::OrderId,
        client_order_id: String,
        expected_price: robson_domain::Price,
        signal_id: Uuid,
    ) -> ExecResult<ActionResult> {
        // 0. Validate governance proof — cycle_id is mandatory for entry events
        let cycle_id = cycle_id.ok_or_else(|| {
            ExecError::InvalidState(
                "governance proof (cycle_id) required for entry order".to_string(),
            )
        })?;

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

        // 2. SAFETY CHECK: Validate futures settings (One-way + leverage)
        info!(
            %position_id,
            symbol = %symbol.as_pair(),
            "Validating futures settings (One-way + {}x)", RiskConfig::LEVERAGE
        );
        if let Err(e) = self.exchange.validate_futures_settings(&symbol, RiskConfig::LEVERAGE).await
        {
            let event = Event::EntryExecutionRejected {
                position_id,
                cycle_id,
                order_id,
                client_order_id,
                signal_id,
                reason: e.to_string(),
                recoverable: true,
                timestamp: chrono::Utc::now(),
            };
            self.store.events().append(&event).await?;
            self.store.apply_event(&event)?;

            return Ok(ActionResult::EntryExecutionRejected { event, error: e.to_string() });
        }

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
            .place_market_order(&symbol, side, quantity, &client_order_id, false)
            .await;

        // 6. Record result and emit appropriate domain event
        match &result {
            Ok(order_result) => {
                info!(
                    %position_id,
                    exchange_order_id = %order_result.exchange_order_id,
                    fill_price = %order_result.fill_price.as_decimal(),
                    "Entry order filled"
                );
                self.journal.complete(signal_id, IntentResult::Success(order_result.clone()))?;

                // Emit EntryOrderAccepted (post-exchange ack, no fill fields)
                let event = Event::EntryOrderAccepted {
                    position_id,
                    cycle_id,
                    order_id,
                    client_order_id,
                    exchange_order_id: order_result.exchange_order_id.clone(),
                    expected_price,
                    quantity,
                    signal_id,
                    timestamp: chrono::Utc::now(),
                };
                self.store.events().append(&event).await?;
                self.store.apply_event(&event)?;

                Ok(ActionResult::OrderPlaced {
                    order: order_result.clone(),
                    event: Some(event),
                })
            },
            Err(e) => {
                error!(%position_id, error = %e, "Entry order failed");

                // Emit EntryOrderFailed before journal completion
                let event = Event::EntryOrderFailed {
                    position_id,
                    cycle_id,
                    order_id,
                    client_order_id,
                    signal_id,
                    reason: e.to_string(),
                    timestamp: chrono::Utc::now(),
                };
                self.store.events().append(&event).await?;
                self.store.apply_event(&event)?;

                self.journal.complete(signal_id, IntentResult::Failed(e.to_string()))?;

                // Return Ok(OrderFailed) — event already applied to store.
                // Caller persists to PostgreSQL eventlog via execute_and_persist().
                Ok(ActionResult::OrderFailed { event, error: e.to_string() })
            },
        }
    }

    /// Execute exit order with idempotency.
    async fn execute_exit_order(
        &self,
        position_id: PositionId,
        cycle_id: Option<Uuid>,
        symbol: robson_domain::Symbol,
        side: robson_domain::OrderSide,
        quantity: robson_domain::Quantity,
        reason: ExitReason,
    ) -> ExecResult<ActionResult> {
        // Generate unique intent ID for exit
        let intent_id = Uuid::now_v7();

        // 1. SAFETY CHECK: Validate futures settings (One-way + leverage)
        // Even for exits, we verify account state hasn't changed
        info!(
            %position_id,
            symbol = %symbol.as_pair(),
            "Validating futures settings for exit (One-way + {}x)", RiskConfig::LEVERAGE
        );
        self.exchange.validate_futures_settings(&symbol, RiskConfig::LEVERAGE).await?;

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
            .place_market_order(&symbol, side, quantity, &intent_id.to_string(), true)
            .await;

        // 4. Record result and emit ExitOrderPlaced event on success
        let mut exit_event_opt = None;
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

                // Create ExitOrderPlaced event (will be returned and persisted by caller)
                let exit_event = Event::ExitOrderPlaced {
                    position_id,
                    cycle_id,
                    order_id: intent_id,
                    expected_price: order_result.fill_price,
                    quantity,
                    exit_reason: reason,
                    timestamp: chrono::Utc::now(),
                };
                // Apply to in-memory store (caller persists to eventlog)
                self.store.events().append(&exit_event).await?;
                self.store.apply_event(&exit_event)?;
                exit_event_opt = Some(exit_event);
            },
            Err(e) => {
                error!(%position_id, error = %e, "Exit order failed");
                self.journal.complete(intent_id, IntentResult::Failed(e.to_string()))?;
            },
        }

        result.map(|order| ActionResult::OrderPlaced { order, event: exit_event_opt })
    }

    /// Get the intent journal (for inspection/recovery).
    pub fn journal(&self) -> &IntentJournal {
        &self.journal
    }

    /// Get the store (for state updates).
    pub fn store(&self) -> &S {
        &self.store
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use robson_domain::{Quantity, Side, Symbol};
    use robson_store::MemoryStore;
    use rust_decimal_macros::dec;

    use super::*;
    use crate::stub::StubExchange;

    async fn create_test_executor() -> Executor<StubExchange, MemoryStore> {
        let exchange = Arc::new(StubExchange::new(dec!(95000)));
        let journal = Arc::new(IntentJournal::new());
        let store = Arc::new(MemoryStore::new());

        Executor::new(exchange, journal, store)
    }

    #[tokio::test]
    async fn test_execute_entry_order_emits_accepted() {
        let executor = create_test_executor().await;

        let signal_id = Uuid::now_v7();
        let position_id = Uuid::now_v7();
        let cycle_id = Uuid::now_v7();
        let order_id = Uuid::now_v7();
        let client_order_id = signal_id.to_string();

        let action = EngineAction::PlaceEntryOrder {
            position_id,
            cycle_id: Some(cycle_id),
            symbol: Symbol::from_pair("BTCUSDT").unwrap(),
            side: robson_domain::OrderSide::Buy,
            quantity: Quantity::new(dec!(0.1)).unwrap(),
            order_id,
            client_order_id: client_order_id.clone(),
            expected_price: robson_domain::Price::new(dec!(95000)).unwrap(),
            signal_id,
        };

        let results = executor.execute(vec![action]).await.unwrap();

        assert_eq!(results.len(), 1);
        match &results[0] {
            ActionResult::OrderPlaced { order, event: Some(event) } => {
                assert_eq!(order.fill_price.as_decimal(), dec!(95000));
                assert_eq!(order.filled_quantity.as_decimal(), dec!(0.1));
                match event {
                    Event::EntryOrderAccepted {
                        cycle_id: cid,
                        exchange_order_id,
                        client_order_id: coid,
                        ..
                    } => {
                        assert_eq!(*cid, cycle_id);
                        assert!(!exchange_order_id.is_empty());
                        assert_eq!(coid, &client_order_id);
                    },
                    other => panic!("Expected EntryOrderAccepted, got {:?}", other.event_type()),
                }
            },
            other => panic!("Expected OrderPlaced with event, got {:?}", other),
        }

        // Intent should be recorded and completed
        let intent = executor.journal.get(signal_id).unwrap().unwrap();
        assert!(intent.is_success());
    }

    #[tokio::test]
    async fn test_execute_exit_order_emits_cycle_id() {
        let executor = create_test_executor().await;

        let position_id = Uuid::now_v7();
        let cycle_id = Uuid::now_v7();

        let action = EngineAction::PlaceExitOrder {
            position_id,
            cycle_id: Some(cycle_id),
            symbol: Symbol::from_pair("BTCUSDT").unwrap(),
            side: robson_domain::OrderSide::Sell,
            quantity: Quantity::new(dec!(0.1)).unwrap(),
            reason: ExitReason::TrailingStop,
        };

        let results = executor.execute(vec![action]).await.unwrap();

        assert_eq!(results.len(), 1);
        match &results[0] {
            ActionResult::OrderPlaced {
                event: Some(Event::ExitOrderPlaced { cycle_id: actual, .. }),
                ..
            } => {
                assert_eq!(*actual, Some(cycle_id));
            },
            other => panic!("Expected exit OrderPlaced event, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_idempotent_entry_order() {
        let executor = create_test_executor().await;

        let signal_id = Uuid::now_v7();
        let position_id = Uuid::now_v7();
        let cycle_id = Uuid::now_v7();

        let action = EngineAction::PlaceEntryOrder {
            position_id,
            cycle_id: Some(cycle_id),
            symbol: Symbol::from_pair("BTCUSDT").unwrap(),
            side: robson_domain::OrderSide::Buy,
            quantity: Quantity::new(dec!(0.1)).unwrap(),
            order_id: Uuid::now_v7(),
            client_order_id: signal_id.to_string(),
            expected_price: robson_domain::Price::new(dec!(95000)).unwrap(),
            signal_id,
        };

        // First execution
        let results1 = executor.execute(vec![action.clone()]).await.unwrap();
        assert!(matches!(results1[0], ActionResult::OrderPlaced { .. }));

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
            tech_stop_distance: None,
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
        let cycle_id = Uuid::now_v7();

        let actions = vec![
            EngineAction::EmitEvent(Event::PositionArmed {
                position_id,
                account_id: Uuid::now_v7(),
                symbol: Symbol::from_pair("BTCUSDT").unwrap(),
                side: Side::Long,
                timestamp: chrono::Utc::now(),
                tech_stop_distance: None,
            }),
            EngineAction::EmitEvent(Event::EntrySignalReceived {
                position_id,
                signal_id,
                entry_price: robson_domain::Price::new(dec!(95000)).unwrap(),
                stop_loss: robson_domain::Price::new(dec!(93500)).unwrap(),
                quantity: Quantity::new(dec!(0.1)).unwrap(),
                timestamp: chrono::Utc::now(),
            }),
            EngineAction::PlaceEntryOrder {
                position_id,
                cycle_id: Some(cycle_id),
                symbol: Symbol::from_pair("BTCUSDT").unwrap(),
                side: robson_domain::OrderSide::Buy,
                quantity: Quantity::new(dec!(0.1)).unwrap(),
                order_id: Uuid::now_v7(),
                client_order_id: signal_id.to_string(),
                expected_price: robson_domain::Price::new(dec!(95000)).unwrap(),
                signal_id,
            },
        ];

        let results = executor.execute(actions).await.unwrap();

        assert_eq!(results.len(), 3);
        assert!(matches!(results[0], ActionResult::EventEmitted(_)));
        assert!(matches!(results[1], ActionResult::EventEmitted(_)));
        assert!(matches!(results[2], ActionResult::OrderPlaced { .. }));
    }

    #[tokio::test]
    async fn test_execute_entry_missing_cycle_id_fails() {
        let executor = create_test_executor().await;

        let signal_id = Uuid::now_v7();
        let position_id = Uuid::now_v7();

        let action = EngineAction::PlaceEntryOrder {
            position_id,
            cycle_id: None,
            symbol: Symbol::from_pair("BTCUSDT").unwrap(),
            side: robson_domain::OrderSide::Buy,
            quantity: Quantity::new(dec!(0.1)).unwrap(),
            order_id: Uuid::now_v7(),
            client_order_id: signal_id.to_string(),
            expected_price: robson_domain::Price::new(dec!(95000)).unwrap(),
            signal_id,
        };

        let result = executor.execute(vec![action]).await;
        assert!(result.is_err(), "Missing cycle_id must fail");
    }

    #[tokio::test]
    async fn test_execute_entry_margin_rejection_returns_entry_execution_rejected() {
        let exchange = Arc::new(StubExchange::new(dec!(95000)));
        exchange.set_futures_settings("Hedge", RiskConfig::LEVERAGE);
        let journal = Arc::new(IntentJournal::new());
        let store = Arc::new(MemoryStore::new());
        let executor = Executor::new(exchange, journal, store);

        let signal_id = Uuid::now_v7();
        let position_id = Uuid::now_v7();
        let cycle_id = Uuid::now_v7();

        let action = EngineAction::PlaceEntryOrder {
            position_id,
            cycle_id: Some(cycle_id),
            symbol: Symbol::from_pair("BTCUSDT").unwrap(),
            side: robson_domain::OrderSide::Buy,
            quantity: Quantity::new(dec!(0.1)).unwrap(),
            order_id: Uuid::now_v7(),
            client_order_id: signal_id.to_string(),
            expected_price: robson_domain::Price::new(dec!(95000)).unwrap(),
            signal_id,
        };

        let results = executor.execute(vec![action]).await.unwrap();
        assert_eq!(results.len(), 1);
        match &results[0] {
            ActionResult::EntryExecutionRejected {
                event:
                    Event::EntryExecutionRejected {
                        cycle_id: actual_cycle_id, recoverable, ..
                    },
                error,
            } => {
                assert_eq!(*actual_cycle_id, cycle_id);
                assert!(*recoverable);
                assert!(error.contains("FUTURES SAFETY VIOLATION"));
            },
            other => panic!("Expected EntryExecutionRejected, got {:?}", other),
        }
    }
}
