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

use robson_domain::{
    Event, ExitReason, OrderSide, PositionId, PositionState, Price, Quantity, ReductionKind,
    RiskConfig, StopPolicy, Symbol, SymbolTradingRules,
};
use robson_engine::EngineAction;
use robson_store::Store;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::{
    cancel::CancelClass,
    error::{ExecError, ExecResult},
    intent::{Intent, IntentAction, IntentJournal, IntentResult},
    order_journal::{
        self, IntentKind, IntentSpec, JournaledIntent, OrderJournal, OwnershipKey, SendRefusal,
        WriteClass,
    },
    ports::{ExchangePort, OrderResult},
    quantize::{self, ReductionOutcome},
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
    /// Proven no-send (ADR-0056 step 7): no request left for this entry, so the
    /// caller may give the detector back. Carries no event, so a database
    /// failure that caused it cannot also lose it.
    EntryNotSent { signal_id: Uuid, reason: String },
    /// The venue refused the exit and the newest rules agree the whole leg
    /// rounds below the lot minimum (ADR-0056 §5). Nothing executed, the
    /// rest of the batch is not run, and the leg keeps its state. `event` is
    /// `None` when the leg already stands at that value.
    ExitNotExecutable {
        event: Option<Event>,
        residual: rust_decimal::Decimal,
    },
    /// A protective stop with nothing executable (ADR-0056 §5): the leg rounds
    /// below the lot minimum, which no prompt retry will change. Returned on
    /// every attempt; the audit events come only when the residual is new.
    ProtectionNotExecutable { position_id: PositionId },
    /// The exit failed after earlier actions of its batch ran. Returned as a
    /// result so their events are persisted, the stop's cancel first of all;
    /// the caller surfaces `error` after that.
    ExitFailed { error: ExecError },
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
    /// Durable order journal (ADR-0056 step 7)
    order_journal: Arc<OrderJournal>,
}

impl<E: ExchangePort, S: Store> Executor<E, S> {
    /// Create a new executor. Its order journal keeps nothing until
    /// [`Self::with_order_journal`] gives it a durable one.
    pub fn new(exchange: Arc<E>, journal: Arc<IntentJournal>, store: Arc<S>) -> Self {
        Self {
            exchange,
            journal,
            store,
            order_journal: Arc::new(OrderJournal::unpersisted()),
        }
    }

    pub fn with_order_journal(mut self, order_journal: Arc<OrderJournal>) -> Self {
        self.order_journal = order_journal;
        self
    }

    /// The order journal this executor writes to.
    pub fn order_journal(&self) -> Arc<OrderJournal> {
        Arc::clone(&self.order_journal)
    }

    /// Return the latest exchange price for a symbol.
    pub async fn get_price(&self, symbol: &Symbol) -> ExecResult<Price> {
        self.exchange.get_price(symbol).await
    }

    /// Expose the exchange port for read-only inspection.
    pub fn exchange(&self) -> Arc<E> {
        Arc::clone(&self.exchange)
    }

    /// Execute a list of engine actions.
    ///
    /// Actions are executed in order. If one fails, subsequent actions
    /// are not executed.
    pub async fn execute(&self, actions: Vec<EngineAction>) -> ExecResult<Vec<ActionResult>> {
        if order_journal::in_operation() {
            return self.execute_batch(actions).await;
        }
        let operation = self.order_journal.begin_operation();
        operation.scope(self.execute_batch(actions)).await
    }

    async fn execute_batch(&self, actions: Vec<EngineAction>) -> ExecResult<Vec<ActionResult>> {
        let mut results = Vec::with_capacity(actions.len());

        for action in actions {
            let is_exit = matches!(action, EngineAction::PlaceExitOrder { .. });
            let action_results = match self.execute_action(action).await {
                Ok(action_results) => action_results,
                Err(error) if is_exit && !results.is_empty() => {
                    results.push(ActionResult::ExitFailed { error });
                    break;
                },
                Err(error) => return Err(error),
            };

            let mut should_stop = false;
            for result in action_results {
                should_stop |= matches!(
                    result,
                    ActionResult::OrderFailed { .. }
                        | ActionResult::EntryExecutionRejected { .. }
                        | ActionResult::EntryNotSent { .. }
                        | ActionResult::ExitNotExecutable { .. }
                );
                results.push(result);
            }
            if should_stop {
                break;
            }
        }

        Ok(results)
    }

    /// Execute a single engine action. Risk-reducing actions can produce an
    /// order result and a residual event, each persisted by the caller.
    async fn execute_action(&self, action: EngineAction) -> ExecResult<Vec<ActionResult>> {
        let result = match action {
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
                return self
                    .execute_exit_order(position_id, cycle_id, symbol, side, quantity, reason)
                    .await;
            },

            EngineAction::PlaceInsuranceStop {
                position_id,
                symbol,
                side,
                quantity,
                stop_price,
            } => {
                return self
                    .execute_place_insurance_stop(position_id, symbol, side, quantity, stop_price)
                    .await;
            },

            EngineAction::ReplaceInsuranceStop {
                position_id,
                symbol,
                side,
                quantity,
                previous_order_id,
                new_stop_price,
            } => {
                return self
                    .execute_replace_insurance_stop(
                        position_id,
                        symbol,
                        side,
                        quantity,
                        previous_order_id,
                        new_stop_price,
                    )
                    .await;
            },

            EngineAction::CancelInsuranceStop { position_id, symbol, order_id } => {
                self.execute_cancel_insurance_stop(position_id, symbol, order_id).await
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
        };
        result.map(|result| vec![result])
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
        let intent = Intent::new(signal_id, position_id, IntentAction::PlaceEntryOrder {
            symbol: symbol.clone(),
            side,
            quantity,
        });

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

        // 5. Commit the intent first (strict): no row, no request.
        let journaled = self.order_journal.open_intent(IntentSpec {
            position_id: Some(position_id),
            kind: IntentKind::Entry,
            client_order_id: client_order_id.clone(),
            cancels_order_id: None,
            params: serde_json::json!({
                "symbol": symbol.as_pair(),
                "side": side,
                "quantity": quantity.as_decimal(),
                "signal_id": signal_id,
            }),
        });
        // The inventory it opens: claimed at its send, with its ticket, and
        // held until its operation ends (ADR-0056 step 7, PR O).
        self.order_journal
            .own_entry_key(&journaled, OwnershipKey::of_entry(&symbol, side));
        if let Err(refusal) = self.order_journal.before_send(&journaled, WriteClass::Strict).await {
            warn!(%position_id, %signal_id, %refusal, "Entry not sent");
            self.order_journal.finish_failed(&journaled).await;
            self.journal.complete(signal_id, IntentResult::Failed(refusal.to_string()))?;
            return match refusal {
                SendRefusal::NotSent(reason) => {
                    Ok(ActionResult::EntryNotSent { signal_id, reason })
                },
                // The row may exist: this is not a proven no-send.
                SendRefusal::UncertainCommit(_) => {
                    self.entry_order_failed(
                        position_id,
                        cycle_id,
                        order_id,
                        client_order_id,
                        signal_id,
                        refusal.to_string(),
                    )
                    .await
                },
            };
        }

        // 6. Execute on exchange exactly once. A timeout/transport failure after
        // order acceptance is ambiguous for MARKET orders; automatic retry can
        // create duplicate real positions. Reconciliation must resolve drift.
        crate::crash_point!(ENTRY_BEFORE_SEND);
        let result = self
            .exchange
            .place_market_order(&symbol, side, quantity, &client_order_id, false)
            .await;
        crate::crash_point!(ENTRY_AFTER_ACK);

        // 7. Record result and emit appropriate domain event
        match &result {
            Ok(order_result) => {
                info!(
                    %position_id,
                    exchange_order_id = %order_result.exchange_order_id,
                    fill_price = %order_result.fill_price.as_decimal(),
                    "Entry order filled"
                );
                self.order_journal
                    .finish_success(&journaled, Some(order_evidence(order_result)))
                    .await;
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
                // No ticket: the adapter refused before the request left.
                let sent = self.order_journal.ticket_issued(&journaled);
                self.order_journal.finish_failed(&journaled).await;
                if !sent {
                    self.journal.complete(signal_id, IntentResult::Failed(e.to_string()))?;
                    return Ok(ActionResult::EntryNotSent { signal_id, reason: e.to_string() });
                }

                // Emit EntryOrderFailed before journal completion
                let error = e.to_string();
                let failed = self
                    .entry_order_failed(
                        position_id,
                        cycle_id,
                        order_id,
                        client_order_id,
                        signal_id,
                        error.clone(),
                    )
                    .await?;
                self.journal.complete(signal_id, IntentResult::Failed(error))?;

                // Return Ok(OrderFailed) — event already applied to store.
                // Caller persists to PostgreSQL eventlog via execute_and_persist().
                Ok(failed)
            },
        }
    }

    /// Emit `EntryOrderFailed`. Its projection changes nothing, and it never
    /// makes an unknown outcome a rejection.
    async fn entry_order_failed(
        &self,
        position_id: PositionId,
        cycle_id: Uuid,
        order_id: robson_domain::OrderId,
        client_order_id: String,
        signal_id: Uuid,
        reason: String,
    ) -> ExecResult<ActionResult> {
        let event = Event::EntryOrderFailed {
            position_id,
            cycle_id,
            order_id,
            client_order_id,
            signal_id,
            reason: reason.clone(),
            timestamp: chrono::Utc::now(),
        };
        self.store.events().append(&event).await?;
        self.store.apply_event(&event)?;
        Ok(ActionResult::OrderFailed { event, error: reason })
    }

    /// Execute a market exit under the risk-reduction rule (ADR-0056 §5).
    async fn execute_exit_order(
        &self,
        position_id: PositionId,
        cycle_id: Option<Uuid>,
        symbol: robson_domain::Symbol,
        side: robson_domain::OrderSide,
        quantity: robson_domain::Quantity,
        reason: ExitReason,
    ) -> ExecResult<Vec<ActionResult>> {
        // Generate unique intent ID for exit
        let intent_id = Uuid::now_v7();
        let client_order_id = intent_id.to_string();

        // 1. SAFETY CHECK: Validate futures settings (One-way + leverage)
        // Even for exits, we verify account state hasn't changed
        info!(
            %position_id,
            symbol = %symbol.as_pair(),
            "Validating futures settings before exit order"
        );
        self.exchange.validate_futures_settings(&symbol, RiskConfig::LEVERAGE).await?;

        // 2. Record intent
        let intent = Intent::new(intent_id, position_id, IntentAction::PlaceExitOrder {
            symbol: symbol.clone(),
            side,
            quantity,
            reason,
        });
        self.journal.record(intent)?;

        // Mark as executing
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

        // 3. Execute on exchange, each submission journaled ahead of it
        let journaled = self.order_journal.open_intent(IntentSpec {
            position_id: Some(position_id),
            kind: IntentKind::Exit,
            client_order_id: client_order_id.clone(),
            cancels_order_id: None,
            params: serde_json::json!({
                "symbol": symbol.as_pair(),
                "side": side,
                "quantity": quantity.as_decimal(),
                "reason": reason,
            }),
        });
        let leg = self.leg(position_id).await;
        let outcome = quantize::submit_reduction(
            &*self.exchange,
            &symbol,
            quantity,
            leg.as_ref().and_then(|leg| leg.rules_snapshot.as_ref()),
            quantize::METADATA_WAIT_BOUND,
            |submitted, _rules, submission| {
                let (journaled, symbol) = (&journaled, &symbol);
                async move {
                    let client_order_id = self
                        .order_journal
                        .authorize_reduction(journaled, submission, || Uuid::now_v7().to_string())
                        .await?;
                    self.exchange
                        .place_market_order(symbol, side, submitted, &client_order_id, true)
                        .await
                }
            },
        )
        .await;

        // 4. Record result
        match outcome {
            Ok(ReductionOutcome::Submitted { accepted: order_result, rules, .. }) => {
                info!(
                    %position_id,
                    exchange_order_id = %order_result.exchange_order_id,
                    fill_price = %order_result.fill_price.as_decimal(),
                    filled_quantity = %order_result.filled_quantity.as_decimal(),
                    ?reason,
                    "Exit order filled"
                );
                self.order_journal
                    .finish_success(&journaled, Some(order_evidence(&order_result)))
                    .await;
                self.journal.complete(intent_id, IntentResult::Success(order_result.clone()))?;

                // A fill closes the leg, whatever it left: what the venue still holds
                // is unauthored inventory for the reconciliation worker (ADR-0056 §5).
                // Nothing else durable says what this order filled, so this is
                // written every time, whatever residual the leg stands at.
                let mut results = Vec::with_capacity(2);
                let residual = quantity.as_decimal() - order_result.filled_quantity.as_decimal();
                if residual > rust_decimal::Decimal::ZERO {
                    warn!(
                        %position_id,
                        symbol = %symbol.as_pair(),
                        %residual,
                        "Exit executed less than the leg holds; closing the leg (ADR-0056 §5)"
                    );
                    let evidence = Self::residual_event(
                        position_id,
                        ReductionKind::Exit,
                        quantity,
                        residual,
                        ResidualEvidence::of_fill(&order_result),
                        rules.as_ref(),
                    );
                    results.extend(
                        self.append_residual(evidence).await.map(ActionResult::EventEmitted),
                    );
                }

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

                results.push(ActionResult::OrderPlaced {
                    order: order_result,
                    event: Some(exit_event),
                });
                Ok(results)
            },
            Ok(ReductionOutcome::NotExecutable { residual, rules }) => {
                error!(
                    %position_id,
                    symbol = %symbol.as_pair(),
                    %residual,
                    "Exit has nothing executable: the leg rounds below the lot minimum (ADR-0056 §5)"
                );
                self.order_journal.finish_failed(&journaled).await;
                self.journal.complete(
                    intent_id,
                    IntentResult::Failed(format!("{residual} below the lot minimum")),
                )?;
                let event = self
                    .record_residual(
                        position_id,
                        ReductionKind::Exit,
                        quantity,
                        residual,
                        ResidualEvidence::none(),
                        rules.as_ref(),
                    )
                    .await;
                Ok(vec![ActionResult::ExitNotExecutable { event, residual }])
            },
            Err(e) => {
                error!(%position_id, error = %e, "Exit order failed");
                self.order_journal.finish_failed(&journaled).await;
                self.journal.complete(intent_id, IntentResult::Failed(e.to_string()))?;
                Err(e)
            },
        }
    }

    /// The leg's stored state, for its rules snapshot and residual. A store
    /// failure never blocks a reduction.
    async fn leg(&self, position_id: PositionId) -> Option<robson_domain::Position> {
        match self.store.positions().find_by_id(position_id).await {
            Ok(position) => position,
            Err(error) => {
                warn!(%position_id, %error, "Cannot read the leg; continuing without it");
                None
            },
        }
    }

    /// Record what an order leaves standing on an open leg, unless the leg
    /// already stands at this value: an unexecutable stop and an
    /// unexecutable exit are both retried every 5 min, a rounded stop is
    /// replaced on every trailing advance, and none of them may grow the
    /// log. Best-effort like the other protection audit events.
    async fn record_residual(
        &self,
        position_id: PositionId,
        order_kind: ReductionKind,
        requested: Quantity,
        residual: rust_decimal::Decimal,
        evidence: ResidualEvidence,
        rules: Option<&SymbolTradingRules>,
    ) -> Option<Event> {
        if residual <= rust_decimal::Decimal::ZERO {
            return None;
        }
        let leg = self.leg(position_id).await;
        if leg.is_some_and(|leg| leg.residual_quantity == Some(residual)) {
            return None;
        }
        self.append_residual(Self::residual_event(
            position_id,
            order_kind,
            requested,
            residual,
            evidence,
            rules,
        ))
        .await
    }

    fn residual_event(
        position_id: PositionId,
        order_kind: ReductionKind,
        requested: Quantity,
        residual: rust_decimal::Decimal,
        evidence: ResidualEvidence,
        rules: Option<&SymbolTradingRules>,
    ) -> Event {
        let requested_quantity = requested.as_decimal();
        Event::ResidualInventoryRecorded {
            position_id,
            order_kind,
            requested_quantity,
            accepted_quantity: requested_quantity - residual,
            residual_quantity: residual,
            exchange_order_id: evidence.exchange_order_id,
            fill_price: evidence.fill_price,
            fee: evidence.fee,
            step_size: rules.map(SymbolTradingRules::step_size),
            min_qty: rules.map(SymbolTradingRules::min_qty),
            timestamp: evidence.timestamp,
        }
    }

    /// A store failure is logged, and the order path goes on.
    async fn append_residual(&self, event: Event) -> Option<Event> {
        let position_id = event.position_id();
        if let Err(error) = self.store.events().append(&event).await {
            warn!(%position_id, %error, "Failed to persist the residual inventory event");
            return None;
        }
        if let Err(error) = self.store.apply_event(&event) {
            warn!(%position_id, %error, "Failed to apply the residual inventory event");
        }
        Some(event)
    }

    /// Whether the leg left `Active` while a stop was on its way to the venue
    /// (ADR-0056 §5): closed by reconciliation, say. A leg not in the store has
    /// nothing to say about it.
    async fn leg_left_active(&self, position_id: PositionId) -> bool {
        self.leg(position_id).await.is_some_and(|leg| {
            matches!(
                leg.state,
                PositionState::Exiting { .. }
                    | PositionState::Closed { .. }
                    | PositionState::Error { .. }
                    | PositionState::Cancelled
            )
        })
    }

    /// Cancel a protective stop as its own journal intent.
    async fn journaled_cancel(
        &self,
        position_id: PositionId,
        symbol: &robson_domain::Symbol,
        order_id: &str,
    ) -> CancelClass {
        self.order_journal
            .journaled_cancel(
                Some(position_id),
                order_id,
                self.exchange.cancel_stop_market_order(symbol, order_id),
            )
            .await
    }

    /// The protection intent of one stop placement.
    fn protection_intent(
        &self,
        position_id: PositionId,
        symbol: &robson_domain::Symbol,
        side: OrderSide,
        quantity: Quantity,
        stop_price: Price,
        client_order_id: &str,
    ) -> JournaledIntent {
        self.order_journal.open_intent(IntentSpec {
            position_id: Some(position_id),
            kind: IntentKind::Protection,
            client_order_id: client_order_id.to_string(),
            cancels_order_id: None,
            params: serde_json::json!({
                "symbol": symbol.as_pair(),
                "side": side,
                "quantity": quantity.as_decimal(),
                "stop_price": stop_price.as_decimal(),
            }),
        })
    }

    /// Take back a stop the venue accepted for a leg that has left `Active`.
    /// Nothing is recorded on the leg, and no sweep visits a flat symbol with
    /// no open leg, so this is the only thing that keeps the stop off a
    /// later entry. Best-effort and logged.
    async fn withdraw_stop(
        &self,
        position_id: PositionId,
        intent_id: Uuid,
        symbol: &robson_domain::Symbol,
        order_ids: &[&str],
    ) -> ExecResult<Vec<ActionResult>> {
        warn!(%position_id, "Leg left Active while its stop was being placed; withdrawing the stop");
        for order_id in order_ids {
            let class = self.journaled_cancel(position_id, symbol, order_id).await;
            if !class.is_definite() {
                warn!(%position_id, %order_id, ?class, "Stop withdrawal is uncertain");
            }
        }
        self.journal.complete(
            intent_id,
            IntentResult::Failed("leg left Active during the placement".to_string()),
        )?;
        Ok(vec![ActionResult::StateUpdated])
    }

    /// Place a reduce-only protective insurance stop on the exchange
    /// (ADR-0039).
    ///
    /// Mirrors `execute_exit_order`: journal an intent, place the order, and on
    /// success emit `InsuranceStopPlaced` (applied to the store so the live
    /// order id is recorded). Any placement failure becomes an audit-only
    /// `InsuranceStopFailed` event — never `OrderFailed`, so the action batch
    /// is never aborted and the software stop remains the primary exit.
    /// Quantity and trigger follow the risk-reduction rule (ADR-0056 §5).
    async fn execute_place_insurance_stop(
        &self,
        position_id: PositionId,
        symbol: robson_domain::Symbol,
        side: OrderSide,
        quantity: Quantity,
        stop_price: Price,
    ) -> ExecResult<Vec<ActionResult>> {
        let intent_id = Uuid::now_v7();
        // Binance clientOrderId limit is 36 chars; `ins-` + 32-char uuid.simple() = 36.
        let client_order_id = format!("ins-{}", intent_id.simple());

        let intent = Intent::new(intent_id, position_id, IntentAction::PlaceInsuranceStopOrder {
            symbol: symbol.clone(),
            side,
            quantity,
            stop_price,
        });
        self.journal.record(intent)?;
        self.journal.mark_executing(intent_id)?;

        info!(
            %position_id,
            %intent_id,
            symbol = %symbol.as_pair(),
            ?side,
            stop_price = %stop_price.as_decimal(),
            "Placing insurance stop"
        );

        let journaled = self.protection_intent(
            position_id,
            &symbol,
            side,
            quantity,
            stop_price,
            &client_order_id,
        );
        let outcome = self
            .submit_protection(position_id, &symbol, side, quantity, stop_price, &journaled)
            .await;

        match outcome {
            Ok(ReductionOutcome::Submitted {
                accepted: order_result, residual, rules, ..
            }) => {
                self.order_journal
                    .finish_success(&journaled, Some(order_evidence(&order_result)))
                    .await;
                if self.leg_left_active(position_id).await {
                    return self
                        .withdraw_stop(position_id, intent_id, &symbol, &[order_result
                            .exchange_order_id
                            .as_str()])
                        .await;
                }
                info!(
                    %position_id,
                    exchange_order_id = %order_result.exchange_order_id,
                    stop_price = %stop_price.as_decimal(),
                    "Insurance stop placed"
                );
                self.journal.complete(intent_id, IntentResult::Success(order_result.clone()))?;
                let evidence = ResidualEvidence::of_stop(&order_result);

                let event = Event::InsuranceStopPlaced {
                    position_id,
                    order_id: order_result.exchange_order_id.clone(),
                    stop_price,
                    timestamp: chrono::Utc::now(),
                };
                self.store.events().append(&event).await?;
                self.store.apply_event(&event)?;

                let mut results =
                    vec![ActionResult::OrderPlaced { order: order_result, event: Some(event) }];
                results.extend(
                    self.record_residual(
                        position_id,
                        ReductionKind::Protection,
                        quantity,
                        residual,
                        evidence,
                        rules.as_ref(),
                    )
                    .await
                    .map(ActionResult::EventEmitted),
                );
                Ok(results)
            },
            Ok(ReductionOutcome::NotExecutable { residual, rules }) => {
                self.order_journal.finish_failed(&journaled).await;
                self.protection_not_executable(
                    position_id,
                    intent_id,
                    quantity,
                    stop_price,
                    residual,
                    rules.as_ref(),
                )
                .await
            },
            Err(e) => {
                warn!(%position_id, error = %e, "Insurance stop placement failed");
                self.order_journal.finish_failed(&journaled).await;
                self.journal.complete(intent_id, IntentResult::Failed(e.to_string()))?;
                Ok(vec![self.emit_insurance_stop_failed(position_id, stop_price, e).await])
            },
        }
    }

    /// Submit a protective stop under the risk-reduction rule (ADR-0056 §5).
    /// A resubmission after a rules rejection re-aligns the trigger on the
    /// refreshed grid, with the same client id.
    async fn submit_protection(
        &self,
        position_id: PositionId,
        symbol: &robson_domain::Symbol,
        side: OrderSide,
        quantity: Quantity,
        stop_price: Price,
        journaled: &JournaledIntent,
    ) -> ExecResult<ReductionOutcome<OrderResult>> {
        let leg = self.leg(position_id).await;
        quantize::submit_reduction(
            &*self.exchange,
            symbol,
            quantity,
            leg.as_ref().and_then(|leg| leg.rules_snapshot.as_ref()),
            quantize::METADATA_WAIT_BOUND,
            |submitted, rules, submission| {
                let trigger = quantize::protective_trigger(rules.as_ref(), side, stop_price);
                async move {
                    let client_order_id = self
                        .order_journal
                        .authorize_reduction(journaled, submission, || {
                            format!("ins-{}", Uuid::now_v7().simple())
                        })
                        .await?;
                    crate::crash_point!(PROTECTION_BEFORE_SEND);
                    self.exchange
                        .place_stop_market_order(symbol, side, submitted, trigger, &client_order_id)
                        .await
                }
            },
        )
        .await
    }

    /// A protective stop with nothing executable: audit-only like any other
    /// placement failure, with the residual on record, once. It stays this way
    /// until the leg or the rules change, so a repeat is only logged. A live
    /// stop stays live.
    async fn protection_not_executable(
        &self,
        position_id: PositionId,
        intent_id: Uuid,
        quantity: Quantity,
        stop_price: Price,
        residual: rust_decimal::Decimal,
        rules: Option<&SymbolTradingRules>,
    ) -> ExecResult<Vec<ActionResult>> {
        let reason = format!("{residual} rounds below the lot minimum");
        warn!(%position_id, %reason, "Insurance stop has nothing executable");
        self.journal.complete(intent_id, IntentResult::Failed(reason.clone()))?;
        let recorded = self
            .record_residual(
                position_id,
                ReductionKind::Protection,
                quantity,
                residual,
                ResidualEvidence::none(),
                rules,
            )
            .await;
        let mut results = Vec::with_capacity(3);
        if let Some(recorded) = recorded {
            results.push(
                self.emit_insurance_stop_failed(
                    position_id,
                    stop_price,
                    ExecError::OrderRejected(reason),
                )
                .await,
            );
            results.push(ActionResult::EventEmitted(recorded));
        }
        results.push(ActionResult::ProtectionNotExecutable { position_id });
        Ok(results)
    }

    /// Replace the protective insurance stop after a trailing advance
    /// (ADR-0039), PLACE-THEN-CANCEL (issue #154 deliverable 6): the
    /// existing stop is never cancelled before its replacement is accepted
    /// by the exchange, so the position is protected at every instant.
    ///
    /// Both orders are reduce-only STOP_MARKETs, so the transient window
    /// where the two coexist is safe: if both ever trigger, the second can
    /// only reduce an already-flat position and is rejected by the exchange.
    /// A cancel failure after a successful placement is tolerated (double
    /// protection over no protection); reconciliation resolves the leftover.
    async fn execute_replace_insurance_stop(
        &self,
        position_id: PositionId,
        symbol: robson_domain::Symbol,
        side: OrderSide,
        quantity: Quantity,
        previous_order_id: String,
        new_stop_price: Price,
    ) -> ExecResult<Vec<ActionResult>> {
        let intent_id = Uuid::now_v7();
        let client_order_id = format!("ins-{}", intent_id.simple());

        let intent = Intent::new(intent_id, position_id, IntentAction::PlaceInsuranceStopOrder {
            symbol: symbol.clone(),
            side,
            quantity,
            stop_price: new_stop_price,
        });
        self.journal.record(intent)?;
        self.journal.mark_executing(intent_id)?;

        info!(
            %position_id,
            %intent_id,
            symbol = %symbol.as_pair(),
            previous_order_id = %previous_order_id,
            new_stop_price = %new_stop_price.as_decimal(),
            "Replacing insurance stop (place-then-cancel)"
        );

        // 1. Place the replacement FIRST. If this fails, a rules rejection included
        //    after its one refresh and resubmission (ADR-0056 §5), the previous stop is
        //    still live on the exchange: protection is preserved and the failure is
        //    audit-only.
        let journaled = self.protection_intent(
            position_id,
            &symbol,
            side,
            quantity,
            new_stop_price,
            &client_order_id,
        );
        let (order_result, residual, rules) = match self
            .submit_protection(position_id, &symbol, side, quantity, new_stop_price, &journaled)
            .await
        {
            Ok(ReductionOutcome::Submitted { accepted, residual, rules, .. }) => {
                (accepted, residual, rules)
            },
            Ok(ReductionOutcome::NotExecutable { residual, rules }) => {
                self.order_journal.finish_failed(&journaled).await;
                return self
                    .protection_not_executable(
                        position_id,
                        intent_id,
                        quantity,
                        new_stop_price,
                        residual,
                        rules.as_ref(),
                    )
                    .await;
            },
            Err(e) => {
                warn!(
                    %position_id,
                    %previous_order_id,
                    error = %e,
                    "Insurance stop replacement placement failed; previous stop remains live"
                );
                self.order_journal.finish_failed(&journaled).await;
                self.journal.complete(intent_id, IntentResult::Failed(e.to_string()))?;
                return Ok(vec![
                    self.emit_insurance_stop_failed(position_id, new_stop_price, e).await,
                ]);
            },
        };
        // The placement's own result, whatever becomes of the predecessor.
        self.order_journal
            .finish_success(&journaled, Some(order_evidence(&order_result)))
            .await;

        if self.leg_left_active(position_id).await {
            return self
                .withdraw_stop(position_id, intent_id, &symbol, &[
                    order_result.exchange_order_id.as_str(),
                    previous_order_id.as_str(),
                ])
                .await;
        }

        // 2. Cancel the previous stop. Whatever becomes of it, it is its own intent and
        //    the placement above stays a success: an uncertain cancel is logged and
        //    tolerated, since the replacement is already protecting the position.
        let class = self.journaled_cancel(position_id, &symbol, &previous_order_id).await;
        if !class.is_definite() {
            warn!(
                %position_id,
                %previous_order_id,
                ?class,
                "Previous insurance stop cancel is uncertain after replacement placement; \
                 tolerating the transient double stop (reduce-only)"
            );
        }

        // The leg can close while that cancel is out, past the check above.
        if self.leg_left_active(position_id).await {
            return self
                .withdraw_stop(position_id, intent_id, &symbol, &[order_result
                    .exchange_order_id
                    .as_str()])
                .await;
        }

        info!(
            %position_id,
            previous_order_id = %previous_order_id,
            exchange_order_id = %order_result.exchange_order_id,
            new_stop_price = %new_stop_price.as_decimal(),
            "Insurance stop replaced"
        );
        self.journal.complete(intent_id, IntentResult::Success(order_result.clone()))?;

        let event = Event::InsuranceStopReplaced {
            position_id,
            previous_order_id,
            order_id: order_result.exchange_order_id.clone(),
            stop_price: new_stop_price,
            timestamp: chrono::Utc::now(),
        };
        self.store.events().append(&event).await?;
        self.store.apply_event(&event)?;

        let evidence = ResidualEvidence::of_stop(&order_result);
        let mut results =
            vec![ActionResult::OrderPlaced { order: order_result, event: Some(event) }];
        results.extend(
            self.record_residual(
                position_id,
                ReductionKind::Protection,
                quantity,
                residual,
                evidence,
                rules.as_ref(),
            )
            .await
            .map(ActionResult::EventEmitted),
        );
        Ok(results)
    }

    /// Cancel the protective insurance stop when the software exit takes over
    /// (ADR-0039). Only a typed `Cancelled` records a cancellation (ADR-0056
    /// step 7): an order found gone keeps its stop id, and an error of any kind
    /// becomes an audit-only `InsuranceStopFailed`.
    ///
    /// The cancel is not rate-limited: while the id stays on the leg it goes
    /// out again before each exit attempt, because a retried cancel can be what
    /// lets the next exit through. No class stops the exit behind it. A failed
    /// append or application of the `Cancelled` event does, as it always has,
    /// and the failure event's append is awaited.
    async fn execute_cancel_insurance_stop(
        &self,
        position_id: PositionId,
        symbol: robson_domain::Symbol,
        order_id: String,
    ) -> ExecResult<ActionResult> {
        debug!(
            %position_id,
            symbol = %symbol.as_pair(),
            order_id = %order_id,
            "Cancelling insurance stop"
        );

        let error = match self.journaled_cancel(position_id, &symbol, &order_id).await {
            CancelClass::Cancelled { .. } => {
                let event = Event::InsuranceStopCancelled {
                    position_id,
                    order_id,
                    timestamp: chrono::Utc::now(),
                };
                self.store.events().append(&event).await?;
                self.store.apply_event(&event)?;
                return Ok(ActionResult::EventEmitted(event));
            },
            // The exchange no longer knows the order. The dominant cause is
            // that the stop TRIGGERED and closed the position, so emitting
            // InsuranceStopCancelled here would both misreport the outcome and
            // clear `insurance_stop_id` — the pointer reverse reconciliation
            // needs to resolve the real fill (2026-07-31 incident). Terminal
            // evidence or not, the leg's close is left to the evidence paths.
            class @ (CancelClass::GoneTerminal { .. } | CancelClass::GoneUnresolved { .. }) => {
                warn!(
                    %position_id,
                    %order_id,
                    ?class,
                    "Insurance stop already gone on exchange; preserving stop id as \
                     reconciliation evidence instead of recording a cancellation"
                );
                return Ok(ActionResult::Skipped(format!(
                    "insurance stop {order_id} already gone on exchange"
                )));
            },
            CancelClass::Indeterminate => {
                ExecError::Timeout("protective stop cancel outcome is indeterminate".to_string())
            },
            CancelClass::Errored(error) => error,
        };
        warn!(%position_id, %error, "Insurance stop cancellation failed");
        // No new stop price for a pure cancel; carry zero as a sentinel.
        Ok(self.emit_insurance_stop_failed(position_id, Price::zero(), error).await)
    }

    /// Build, persist, and apply an `InsuranceStopFailed` audit event
    /// (ADR-0039). Best-effort: the failure must not abort the action batch, so
    /// a store error is logged rather than propagated — the software stop
    /// remains the primary exit regardless.
    async fn emit_insurance_stop_failed(
        &self,
        position_id: PositionId,
        stop_price: Price,
        error: ExecError,
    ) -> ActionResult {
        let event = Event::InsuranceStopFailed {
            position_id,
            stop_price,
            error: error.to_string(),
            timestamp: chrono::Utc::now(),
        };
        if let Err(append_err) = self.store.events().append(&event).await {
            warn!(%position_id, error = %append_err, "Failed to persist InsuranceStopFailed audit event");
        } else if let Err(apply_err) = self.store.apply_event(&event) {
            warn!(%position_id, error = %apply_err, "Failed to apply InsuranceStopFailed audit event");
        }
        ActionResult::EventEmitted(event)
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

/// What the venue answered, for the intent's `venue_evidence`.
fn order_evidence(order: &OrderResult) -> serde_json::Value {
    serde_json::json!({
        "exchange_order_id": order.exchange_order_id,
        "fill_price": order.fill_price.as_decimal(),
        "filled_quantity": order.filled_quantity.as_decimal(),
    })
}

/// What a residual event says about the order behind it (ADR-0056 §5).
struct ResidualEvidence {
    exchange_order_id: Option<String>,
    fill_price: Option<Price>,
    fee: Option<rust_decimal::Decimal>,
    timestamp: chrono::DateTime<chrono::Utc>,
}

impl ResidualEvidence {
    /// An exit that executed: its fill, stamped when the venue filled it.
    fn of_fill(order: &OrderResult) -> Self {
        Self {
            exchange_order_id: Some(order.exchange_order_id.clone()),
            fill_price: Some(order.fill_price),
            fee: Some(order.fee),
            timestamp: order.filled_at,
        }
    }

    /// A stop the venue accepted: nothing has executed.
    fn of_stop(order: &OrderResult) -> Self {
        Self {
            exchange_order_id: Some(order.exchange_order_id.clone()),
            ..Self::none()
        }
    }

    /// Nothing went out.
    fn none() -> Self {
        Self {
            exchange_order_id: None,
            fill_price: None,
            fee: None,
            timestamp: chrono::Utc::now(),
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use robson_domain::{Position, Quantity, Side, Symbol};
    use robson_store::MemoryStore;
    use rust_decimal::Decimal;
    use rust_decimal_macros::dec;

    use super::*;
    use crate::stub::StubExchange;

    async fn create_test_executor() -> Executor<StubExchange, MemoryStore> {
        let exchange = Arc::new(StubExchange::new(dec!(95000)));
        let journal = Arc::new(IntentJournal::new());
        let store = Arc::new(MemoryStore::new());

        Executor::new(exchange, journal, store)
    }

    // ---- ADR-0056 step 7, PR J: journal boundaries ------------------------------

    use crate::order_journal::{JournalFault, StubJournalBackend};

    struct JournalRig {
        exchange: Arc<StubExchange>,
        backend: Arc<StubJournalBackend>,
        journal: Arc<OrderJournal>,
        executor: Executor<StubExchange, MemoryStore>,
    }

    const SYMBOL: &str = "BTCUSDT";
    const FAULTS: [JournalFault; 4] = [
        JournalFault::Known,
        JournalFault::AckLost,
        JournalFault::NotLandedUncertain,
        JournalFault::Hang,
    ];

    fn journal_rig() -> JournalRig {
        journal_rig_with_bound(std::time::Duration::from_millis(20))
    }

    fn journal_rig_with_bound(bound: std::time::Duration) -> JournalRig {
        let exchange = Arc::new(StubExchange::new(dec!(95000)));
        let backend = Arc::new(StubJournalBackend::default());
        let journal = Arc::new(
            OrderJournal::new(backend.clone(), &crate::venue::VenueId::new("test:venue"))
                .with_commit_bound(bound),
        );
        let executor = Executor::new(
            Arc::clone(&exchange),
            Arc::new(IntentJournal::new()),
            Arc::new(MemoryStore::new()),
        )
        .with_order_journal(Arc::clone(&journal));
        JournalRig { exchange, backend, journal, executor }
    }

    impl JournalRig {
        fn sent_market_orders(&self) -> u64 {
            self.exchange.market_order_call_count()
        }

        fn kinds_of_rows(&self) -> Vec<IntentKind> {
            self.backend.rows().iter().map(|r| r.intent.as_ref().unwrap().kind).collect()
        }

        fn row_of_kind(&self, kind: IntentKind) -> crate::order_journal::StubJournalRow {
            self.backend
                .rows()
                .into_iter()
                .find(|r| r.intent.as_ref().unwrap().kind == kind)
                .unwrap_or_else(|| panic!("no {kind:?} row: {:?}", self.kinds_of_rows()))
        }

        async fn active_position(&self, stop_id: Option<&str>) -> Position {
            let symbol = Symbol::from_pair(SYMBOL).unwrap();
            let mut position = Position::new(Uuid::now_v7(), symbol, Side::Long);
            position.entry_price = Some(Price::new(dec!(95000)).unwrap());
            position.quantity = Quantity::new(dec!(0.1)).unwrap();
            position.state = PositionState::Active {
                current_price: Price::new(dec!(95000)).unwrap(),
                trailing_stop: Price::new(dec!(90000)).unwrap(),
                favorable_extreme: Price::new(dec!(95000)).unwrap(),
                extreme_at: chrono::Utc::now(),
                insurance_stop_id: stop_id.map(str::to_string),
                invalidation_guard_level: None,
                last_emitted_stop: None,
            };
            self.executor.store().positions().save(&position).await.unwrap();
            position
        }

        async fn placed_stop(&self, position_id: PositionId) -> String {
            let placed = self
                .executor
                .execute(vec![EngineAction::PlaceInsuranceStop {
                    position_id,
                    symbol: Symbol::from_pair(SYMBOL).unwrap(),
                    side: OrderSide::Sell,
                    quantity: Quantity::new(dec!(0.1)).unwrap(),
                    stop_price: Price::new(dec!(90000)).unwrap(),
                }])
                .await
                .unwrap();
            match &placed[0] {
                ActionResult::OrderPlaced { order, .. } => order.exchange_order_id.clone(),
                other => panic!("{other:?}"),
            }
        }
    }

    fn entry_action() -> EngineAction {
        EngineAction::PlaceEntryOrder {
            position_id: Uuid::now_v7(),
            cycle_id: Some(Uuid::now_v7()),
            symbol: Symbol::from_pair(SYMBOL).unwrap(),
            side: OrderSide::Buy,
            quantity: Quantity::new(dec!(0.1)).unwrap(),
            order_id: Uuid::now_v7(),
            client_order_id: Uuid::now_v7().to_string(),
            expected_price: Price::new(dec!(95000)).unwrap(),
            signal_id: Uuid::now_v7(),
        }
    }

    #[tokio::test]
    async fn entry_pre_send_commit_known_failure_is_typed_and_sends_nothing() {
        let rig = journal_rig();
        rig.backend.push_fault(JournalFault::Known);
        let results = rig.executor.execute(vec![entry_action()]).await.unwrap();
        assert!(matches!(results[0], ActionResult::EntryNotSent { .. }), "{results:?}");
        assert_eq!(rig.sent_market_orders(), 0);
        assert!(rig.backend.rows().is_empty(), "no row, so nothing to mark");
    }

    #[tokio::test]
    async fn entry_pre_send_commit_uncertain_sends_nothing_and_is_not_a_no_send() {
        for fault in [
            JournalFault::AckLost,
            JournalFault::NotLandedUncertain,
            JournalFault::Hang,
        ] {
            let rig = journal_rig();
            rig.backend.push_fault(fault);
            let results = rig.executor.execute(vec![entry_action()]).await.unwrap();
            assert!(matches!(results[0], ActionResult::OrderFailed { .. }), "{fault:?}");
            assert_eq!(rig.sent_market_orders(), 0, "{fault:?}");
            let outcome = rig.row_of_kind(IntentKind::Entry).outcome.unwrap();
            assert!(outcome.uncertain_at.is_some() && !outcome.not_sent, "{fault:?}");
        }
    }

    #[tokio::test]
    async fn entry_result_write_failure_keeps_the_venue_result() {
        for fault in FAULTS {
            let rig = journal_rig();
            rig.backend.fail_commit(1, Some(fault));
            let results = rig.executor.execute(vec![entry_action()]).await.unwrap();
            assert!(matches!(results[0], ActionResult::OrderPlaced { .. }), "{fault:?}");
            assert_eq!(rig.sent_market_orders(), 1, "{fault:?}");
            assert!(!rig.journal.latch_set(), "{fault:?}");
            rig.journal.flush_pending().await;
            let outcome = rig.row_of_kind(IntentKind::Entry).outcome.unwrap();
            assert!(outcome.venue_outcome.is_some(), "{fault:?}");
        }
    }

    #[tokio::test]
    async fn exit_is_sent_through_every_journal_fault() {
        for fault in FAULTS {
            for write in 0..2 {
                let rig = journal_rig();
                let position = rig.active_position(None).await;
                rig.backend.fail_commit(write, Some(fault));
                let results =
                    rig.executor.execute(vec![exit_action(position.id, dec!(0.1))]).await.unwrap();
                let case = format!("{fault:?} write {write}");
                assert!(
                    results.iter().any(|r| matches!(r, ActionResult::OrderPlaced { .. })),
                    "{case}: {results:?}"
                );
                assert_eq!(rig.sent_market_orders(), 1, "{case}");
                assert_eq!(rig.journal.latch_set(), write == 0, "{case}");
                rig.journal.flush_pending().await;
                let row = rig.row_of_kind(IntentKind::Exit);
                assert_eq!(row.attempts.len(), 1, "{case}");
                assert!(row.outcome.unwrap().venue_outcome.is_some(), "{case}");
            }
        }
    }

    #[tokio::test]
    async fn stop_placement_is_sent_through_every_journal_fault() {
        for fault in FAULTS {
            for write in 0..2 {
                let rig = journal_rig();
                let position = rig.active_position(None).await;
                rig.backend.fail_commit(write, Some(fault));
                rig.placed_stop(position.id).await;
                let case = format!("{fault:?} write {write}");
                assert_eq!(rig.exchange.protection_call_count(), 1, "{case}");
                assert_eq!(rig.journal.latch_set(), write == 0, "{case}");
                rig.journal.flush_pending().await;
                let outcome = rig.row_of_kind(IntentKind::Protection).outcome.unwrap();
                assert!(outcome.venue_outcome.is_some(), "{case}");
            }
        }
    }

    /// A replacement is two intents: writes 0 and 1 belong to the placement, 2
    /// and 3 to the predecessor's cancel. Nothing stops either request.
    #[tokio::test]
    async fn replacement_keeps_going_through_every_journal_fault() {
        for fault in FAULTS {
            for write in 0..4 {
                let rig = journal_rig();
                let position = rig.active_position(None).await;
                let old = rig.placed_stop(position.id).await;
                rig.backend.fail_commit(write, Some(fault));

                let results = rig
                    .executor
                    .execute(vec![EngineAction::ReplaceInsuranceStop {
                        position_id: position.id,
                        symbol: Symbol::from_pair(SYMBOL).unwrap(),
                        side: OrderSide::Sell,
                        quantity: Quantity::new(dec!(0.1)).unwrap(),
                        previous_order_id: old.clone(),
                        new_stop_price: Price::new(dec!(91000)).unwrap(),
                    }])
                    .await
                    .unwrap();
                let case = format!("{fault:?} write {write}");
                assert!(
                    results.iter().any(|r| matches!(r, ActionResult::OrderPlaced { .. })),
                    "{case}: {results:?}"
                );
                assert_eq!(rig.exchange.protection_call_count(), 2, "{case}");
                assert!(
                    !rig.exchange.has_stop_order(&old),
                    "{case}: the predecessor was cancelled"
                );
                assert_eq!(rig.journal.latch_set(), write == 0 || write == 2, "{case}");
                rig.journal.flush_pending().await;
                let cancel = rig.row_of_kind(IntentKind::Cancel);
                assert_eq!(cancel.intent.unwrap().cancels_order_id.as_deref(), Some(old.as_str()));
                assert_eq!(cancel.outcome.unwrap().cancel_class, Some("cancelled"));
            }
        }
    }

    /// The cancel of a replacement is its own intent: an unresolved one leaves
    /// the placement a success.
    #[tokio::test]
    async fn a_failed_predecessor_cancel_never_touches_the_placement() {
        let rig = journal_rig();
        let position = rig.active_position(None).await;
        let old = rig.placed_stop(position.id).await;
        rig.exchange.set_cancel_fails_next(true);
        rig.executor
            .execute(vec![EngineAction::ReplaceInsuranceStop {
                position_id: position.id,
                symbol: Symbol::from_pair(SYMBOL).unwrap(),
                side: OrderSide::Sell,
                quantity: Quantity::new(dec!(0.1)).unwrap(),
                previous_order_id: old,
                new_stop_price: Price::new(dec!(91000)).unwrap(),
            }])
            .await
            .unwrap();
        let cancel = rig.row_of_kind(IntentKind::Cancel).outcome.unwrap();
        assert!(cancel.uncertain_at.is_some());
        assert!(rig
            .backend
            .rows()
            .iter()
            .filter(|r| r.intent.as_ref().unwrap().kind == IntentKind::Protection)
            .all(|r| r.outcome.as_ref().unwrap().venue_outcome.is_some()));
    }

    /// A stop accepted for a leg that left `Active` is withdrawn: the
    /// placement stays a success and each withdrawal is a cancel intent.
    #[tokio::test]
    async fn withdrawal_keeps_going_through_every_journal_fault() {
        for fault in FAULTS {
            for write in 2..4 {
                let rig = journal_rig();
                let position = rig.active_position(None).await;
                let store = Arc::clone(&rig.executor.store);
                let position_id = position.id;
                rig.exchange.set_after_next_stop_accepted(move || {
                    let price = Price::new(dec!(90000)).unwrap();
                    store
                        .apply_event(&Event::PositionClosed {
                            position_id,
                            exit_reason: ExitReason::InsuranceStop,
                            entry_price: price,
                            exit_price: price,
                            realized_pnl: Decimal::ZERO,
                            total_fees: Decimal::ZERO,
                            closure_evidence: robson_domain::ClosureEvidence::real_exit_fill(None),
                            timestamp: chrono::Utc::now(),
                        })
                        .unwrap();
                });
                rig.backend.fail_commit(write, Some(fault));

                rig.executor
                    .execute(vec![EngineAction::PlaceInsuranceStop {
                        position_id: position.id,
                        symbol: Symbol::from_pair(SYMBOL).unwrap(),
                        side: OrderSide::Sell,
                        quantity: Quantity::new(dec!(0.1)).unwrap(),
                        stop_price: Price::new(dec!(90000)).unwrap(),
                    }])
                    .await
                    .unwrap();
                let case = format!("{fault:?} write {write}");
                assert_eq!(rig.exchange.stop_order_count(), 0, "{case}: withdrawn");
                assert_eq!(rig.journal.latch_set(), write == 2, "{case}");
                rig.journal.flush_pending().await;
                let placement = rig.row_of_kind(IntentKind::Protection);
                assert!(placement.outcome.unwrap().venue_outcome.is_some(), "{case}");
                assert!(rig.row_of_kind(IntentKind::Cancel).outcome.is_some(), "{case}");
            }
        }
    }

    /// The first submission's rules rejection does not resolve the attempt, so
    /// the resubmission's commit carries both.
    #[tokio::test]
    async fn the_quantisation_retry_needs_its_own_completeness_commit() {
        let rig = journal_rig();
        let position = rig.active_position(None).await;
        rig.exchange.set_trading_rules(lot_rules(dec!(0.001)));
        rig.exchange.set_rules_rejections_next(1);
        rig.executor.execute(vec![exit_action(position.id, dec!(0.1))]).await.unwrap();
        assert_eq!(rig.sent_market_orders(), 2);
        assert_eq!(rig.row_of_kind(IntentKind::Exit).attempts.len(), 2);
    }

    fn exit_rows(
        rig: &JournalRig,
    ) -> (
        crate::order_journal::StubJournalRow,
        Option<crate::order_journal::StubJournalRow>,
    ) {
        let rows: Vec<_> = rig
            .backend
            .rows()
            .into_iter()
            .filter(|row| row.intent.as_ref().unwrap().kind == IntentKind::Exit)
            .collect();
        let fresh = rows.iter().find(|row| {
            row.intent.as_ref().unwrap().params.get("replaces_client_order_id").is_some()
        });
        let sealed = rows
            .iter()
            .find(|row| {
                row.intent.as_ref().unwrap().params.get("replaces_client_order_id").is_none()
            })
            .expect("the first intent");
        (sealed.clone(), fresh.cloned())
    }

    /// Stop cancelled, exit rejected on rules, the same-id retry refused by the
    /// journal: the fresh-id exit reaches the venue in the same call.
    #[tokio::test]
    async fn a_refused_retry_after_the_stop_is_cancelled_exits_under_a_fresh_id() {
        for fault in FAULTS {
            let rig = journal_rig();
            let position = rig.active_position(None).await;
            let stop = rig.placed_stop(position.id).await;
            rig.exchange.set_trading_rules(lot_rules(dec!(0.001)));
            rig.exchange.set_rules_rejections_next(1);
            // Cancel: 0 and 1. Exit: 2 is the first commit, 3 the same-id retry's.
            rig.backend.fail_commit(3, Some(fault));

            let results = rig
                .executor
                .execute(vec![
                    cancel_action(position.id, &stop),
                    exit_action(position.id, dec!(0.1)),
                ])
                .await
                .unwrap();

            assert!(!rig.exchange.has_stop_order(&stop), "{fault:?}");
            assert!(
                results.iter().any(|r| matches!(r, ActionResult::OrderPlaced { .. })),
                "{fault:?}: {results:?}"
            );
            let sent = rig.exchange.market_order_client_ids();
            assert_eq!(sent.len(), 2, "{fault:?}: the rejected one and the fresh one: {sent:?}");
            assert_ne!(sent[0], sent[1], "{fault:?}: nothing more under the sealed id");

            rig.journal.flush_pending().await;
            let (sealed, fresh) = exit_rows(&rig);
            let fresh = fresh.expect("the fresh intent is on record");
            assert_eq!(sealed.intent.as_ref().unwrap().client_order_id, sent[0], "{fault:?}");
            assert_eq!(fresh.intent.as_ref().unwrap().client_order_id, sent[1], "{fault:?}");
            assert_eq!(
                fresh.intent.as_ref().unwrap().params["replaces_client_order_id"],
                sent[0].as_str(),
                "{fault:?}"
            );
            assert!(!sealed.attempts.is_empty(), "{fault:?}: the rejected attempt is on record");
            let sealed_outcome = sealed.outcome.expect("the sealed intent has an ending");
            assert!(sealed_outcome.uncertain_at.is_some(), "{fault:?}");
            assert_eq!(sealed_outcome.venue_outcome, None, "{fault:?}");
            assert_eq!(
                fresh.outcome.expect("the fresh intent has an ending").venue_outcome,
                Some(crate::order_journal::VenueOutcome::Success),
                "{fault:?}"
            );
        }
    }

    /// ADR-0056 step 7, PR F, review round 1: a refused retry's original is
    /// uncertain the moment its fresh-id replacement starts, so its incident
    /// closes the fence before the replacement's commit or venue answer is
    /// awaited. With the replacement's commit held mid-flight, an entry sent
    /// through the venue's real send start is refused, and only the rejected
    /// exit and its replacement reach the venue. For every way the same-id
    /// retry's commit can fail, the review's own sequence (an uncertain
    /// retry commit, which seals the original with no occurrence) included.
    #[tokio::test]
    async fn a_refused_retrys_original_fences_before_its_replacement_is_awaited() {
        use crate::{
            gate::{IncidentReason, OperationKind},
            venue::{EntryOrReduce, TradingPort},
        };

        for fault in FAULTS {
            let rig = journal_rig_with_bound(std::time::Duration::from_secs(5));
            let position = rig.active_position(None).await;
            rig.exchange.set_trading_rules(lot_rules(dec!(0.001)));
            rig.exchange.set_rules_rejections_next(1);
            // Exit: 0 is its first commit, 1 the same-id retry's, refused; 2
            // the replacement's, held.
            rig.backend.fail_commit(1, Some(fault));
            let held = rig.backend.hold_commit(2);

            let exiting = rig.executor.execute(vec![exit_action(position.id, dec!(0.1))]);
            let meanwhile = async {
                while rig.backend.commits().len() < 3 {
                    tokio::task::yield_now().await;
                }
                assert_eq!(rig.sent_market_orders(), 1, "{fault:?}: only the rejected one so far");
                assert!(
                    rig.journal.gate().fenced(),
                    "{fault:?}: fenced while the replacement is held"
                );
                let reasons: Vec<_> =
                    rig.journal.gate().open_incidents().iter().map(|i| i.reason).collect();
                assert_eq!(reasons, [IncidentReason::UncertainOutcome], "{fault:?}");
                let entry = rig
                    .journal
                    .acquire(OperationKind::Arm)
                    .unwrap()
                    .run(async {
                        let entry = rig.journal.open_intent(IntentSpec {
                            position_id: None,
                            kind: IntentKind::Entry,
                            client_order_id: "e-while-held".to_string(),
                            cancels_order_id: None,
                            params: serde_json::Value::Null,
                        });
                        rig.journal.before_send(&entry, WriteClass::Strict).await.unwrap();
                        TradingPort::place_market_order(
                            &*rig.exchange,
                            &Symbol::from_pair(SYMBOL).unwrap(),
                            robson_domain::OrderSide::Buy,
                            Quantity::new(dec!(0.1)).unwrap(),
                            "e-while-held",
                            EntryOrReduce::Entry,
                        )
                        .await
                    })
                    .await;
                assert!(
                    matches!(&entry, Err(ExecError::SendRefused(why)) if why.contains("fenced")),
                    "{fault:?}: {entry:?}"
                );
                held.notify_one();
            };
            let (results, ()) = tokio::join!(exiting, meanwhile);

            let results = results.unwrap();
            assert!(
                results.iter().any(|r| matches!(r, ActionResult::OrderPlaced { .. })),
                "{fault:?}: {results:?}"
            );
            let sent = rig.exchange.market_order_client_ids();
            assert_eq!(sent.len(), 2, "{fault:?}: the rejected exit and its replacement: {sent:?}");
            assert!(!sent.contains(&"e-while-held".to_string()), "{fault:?}");
            rig.journal.flush_pending().await;
            let (sealed, _) = exit_rows(&rig);
            let outcome = sealed.outcome.expect("the original's ending");
            assert!(
                outcome.uncertain_at.is_some() && outcome.uncertain_generation.is_some(),
                "{fault:?}"
            );
            assert_eq!(rig.journal.gate().open_incidents()[0].count, 1, "{fault:?}: recorded once");
        }
    }

    /// ADR-0056 step 7, PR F, review round 2: while the replacement waits at
    /// the venue, the retry flush lands the original's uncertain ending and its
    /// incident, and a release at that generation opens the fence. The
    /// original's link to its replacement survives the flush, so the
    /// replacement's own ending is recorded at its completion, inside the
    /// enclosing operation (a reconciliation scan's outlives the exit): an
    /// uncertain one fences at once, and an entry is then refused at its send;
    /// a known success is recorded with its evidence, and nothing is
    /// classified uncertain when the operation ends.
    #[tokio::test]
    async fn a_replacement_link_survives_the_retry_flush_and_a_release() {
        use crate::{
            gate::{IncidentReason, OperationKind},
            order_journal::ReleaseRecord,
            venue::{EntryOrReduce, TradingPort},
        };

        let faults = [
            JournalFault::Known,
            JournalFault::AckLost,
            JournalFault::NotLandedUncertain,
        ];
        for fault in faults {
            for uncertain in [true, false] {
                let ending = if uncertain { "uncertain" } else { "accepted" };
                let case = format!("{fault:?}, the replacement {ending}");
                let rig = journal_rig_with_bound(std::time::Duration::from_secs(5));
                let position = rig.active_position(None).await;
                rig.exchange.set_trading_rules(lot_rules(dec!(0.001)));
                rig.exchange.set_rules_rejections_next(1);
                // Exit: 0 is its first commit, 1 the same-id retry's, refused;
                // 2 the replacement's, held until the venue hold is set.
                rig.backend.fail_commit(1, Some(fault));
                let held = rig.backend.hold_commit(2);

                let scan = rig.journal.begin_operation();
                let exiting =
                    scan.scope(rig.executor.execute(vec![exit_action(position.id, dec!(0.1))]));
                let meanwhile = async {
                    while rig.backend.commits().len() < 3 {
                        tokio::task::yield_now().await;
                    }
                    // The replacement goes on to the venue and waits there.
                    let at_venue = rig.exchange.hold_next_mutation();
                    held.notify_one();
                    while rig.exchange.mutations().len() < 2 {
                        tokio::task::yield_now().await;
                    }
                    rig.journal.flush_pending().await;
                    assert_eq!(rig.journal.retry_set_depth(), 0, "{case}: the original landed");
                    let generation = rig.journal.gate().generation();
                    let receipt = rig
                        .journal
                        .release(ReleaseRecord {
                            decision_id: Uuid::now_v7(),
                            generation,
                            actor: "operator@example.com".to_string(),
                            auth_kind: "verified",
                            note: "reviewed".to_string(),
                            treatment: "unbooked_accepted",
                            decided_at: chrono::Utc::now(),
                        })
                        .await
                        .unwrap();
                    assert!(receipt.fence_open, "{case}");
                    if uncertain {
                        rig.exchange.set_order_fail_next(true);
                    }
                    at_venue.notify_one();
                    generation
                };
                let (results, released) = tokio::join!(exiting, meanwhile);
                assert_eq!(
                    rig.exchange.market_order_client_ids().len(),
                    2,
                    "{case}: the rejected exit and its replacement"
                );

                // The exit is over; the operation that ran it is not.
                if uncertain {
                    assert!(rig.journal.gate().fenced(), "{case}: fenced at once: {results:?}");
                    let incidents = rig.journal.gate().open_incidents();
                    assert_eq!(incidents.len(), 1, "{case}");
                    assert_eq!(incidents[0].reason, IncidentReason::UncertainOutcome, "{case}");
                    assert!(incidents[0].first_generation > released, "{case}");
                    let entry = rig
                        .journal
                        .acquire(OperationKind::Arm)
                        .unwrap()
                        .run(async {
                            let entry = rig.journal.open_intent(IntentSpec {
                                position_id: None,
                                kind: IntentKind::Entry,
                                client_order_id: "e-after".to_string(),
                                cancels_order_id: None,
                                params: serde_json::Value::Null,
                            });
                            rig.journal.before_send(&entry, WriteClass::Strict).await.unwrap();
                            TradingPort::place_market_order(
                                &*rig.exchange,
                                &Symbol::from_pair(SYMBOL).unwrap(),
                                robson_domain::OrderSide::Buy,
                                Quantity::new(dec!(0.1)).unwrap(),
                                "e-after",
                                EntryOrReduce::Entry,
                            )
                            .await
                        })
                        .await;
                    assert!(
                        matches!(&entry, Err(ExecError::SendRefused(why)) if why.contains("fenced")),
                        "{case}: {entry:?}"
                    );
                } else {
                    let results = results.unwrap();
                    assert!(
                        results.iter().any(|r| matches!(r, ActionResult::OrderPlaced { .. })),
                        "{case}: {results:?}"
                    );
                    assert!(!rig.journal.gate().fenced(), "{case}");
                    assert!(rig.journal.gate().open_incidents().is_empty(), "{case}");
                }
                rig.journal.flush_pending().await;
                let (_, fresh) = exit_rows(&rig);
                let fresh = fresh.expect("the replacement is on record").outcome;
                let fresh = fresh.expect("the replacement's ending is on record");
                if uncertain {
                    assert!(fresh.uncertain_generation.is_some_and(|g| g > released), "{case}");
                } else {
                    assert_eq!(
                        fresh.venue_outcome,
                        Some(crate::order_journal::VenueOutcome::Success),
                        "{case}"
                    );
                    assert!(fresh.venue_evidence.is_some(), "{case}: its evidence");
                    assert!(fresh.uncertain_at.is_none(), "{case}");
                }
                assert!(rig.journal.is_quiescent(), "{case}: the link consumed at completion");

                drop(scan);
                assert_eq!(rig.journal.gate().fenced(), uncertain, "{case}: after the operation");
                assert_eq!(
                    rig.journal.gate().open_incidents().len(),
                    usize::from(uncertain),
                    "{case}: nothing more at the operation's end"
                );
                rig.journal.flush_pending().await;
                assert!(rig.journal.is_quiescent(), "{case}: the link consumed, memory empty");
            }
        }
    }

    /// Retry set full, discarded set with room: the exit's first commit fails,
    /// so its id can only be kept as discarded, and the same-id retry is
    /// refused on that. The fresh-id exit still reaches the venue in the
    /// same call.
    #[tokio::test]
    async fn a_retry_refused_on_a_discarded_id_exits_under_a_fresh_id() {
        let rig = journal_rig();
        let position = rig.active_position(None).await;
        let stop = rig.placed_stop(position.id).await;
        rig.exchange.set_trading_rules(lot_rules(dec!(0.001)));

        rig.backend.set_fail_all(Some(JournalFault::Known));
        let filling = rig.journal.begin_operation();
        filling
            .scope(async {
                for index in 0..order_journal::RETRY_SET_CAPACITY {
                    let intent = rig.journal.open_intent(IntentSpec {
                        position_id: None,
                        kind: IntentKind::Exit,
                        client_order_id: format!("filling-{index}"),
                        cancels_order_id: None,
                        params: serde_json::Value::Null,
                    });
                    rig.journal.before_send(&intent, WriteClass::Bounded).await.unwrap();
                }
            })
            .await;
        rig.backend.set_fail_all(None);
        // Full, and the `unlanded_send` incident's upsert beside it (PR F).
        assert_eq!(rig.journal.retry_set_depth(), order_journal::RETRY_SET_CAPACITY + 1);

        rig.exchange.set_rules_rejections_next(1);
        // Cancel: 0 and 1. Exit: 2 is its first commit.
        rig.backend.fail_commit(2, Some(JournalFault::Known));

        let results = rig
            .executor
            .execute(vec![
                cancel_action(position.id, &stop),
                exit_action(position.id, dec!(0.1)),
            ])
            .await
            .unwrap();

        assert!(!rig.exchange.has_stop_order(&stop), "the stop was cancelled");
        assert!(
            results.iter().any(|r| matches!(r, ActionResult::OrderPlaced { .. })),
            "{results:?}"
        );
        let sent = rig.exchange.market_order_client_ids();
        assert_eq!(sent.len(), 2, "the rejected one and the fresh one: {sent:?}");
        assert_ne!(sent[0], sent[1], "nothing more under the discarded id");
        assert_eq!(rig.journal.non_reuse_ids_lost(), 0, "its id was kept");
        let fresh = rig.backend.row(&sent[1]).expect("the fresh intent is on record");
        assert_eq!(
            fresh.outcome.expect("it has an ending").venue_outcome,
            Some(crate::order_journal::VenueOutcome::Success)
        );
    }

    /// Both memories full: the first commit fails and the id cannot be kept, so
    /// it is only sealed. The retry is refused on that, and the fresh-id exit
    /// goes out all the same, with every journal write failing.
    #[tokio::test]
    async fn a_retry_refused_on_a_sealed_id_exits_under_a_fresh_id() {
        let rig = journal_rig();
        let position = rig.active_position(None).await;
        let stop = rig.placed_stop(position.id).await;
        rig.exchange.set_trading_rules(lot_rules(dec!(0.001)));
        rig.backend.saturate(&rig.journal).await;
        rig.exchange.set_rules_rejections_next(1);

        let results = rig
            .executor
            .execute(vec![
                cancel_action(position.id, &stop),
                exit_action(position.id, dec!(0.1)),
            ])
            .await
            .unwrap();

        assert!(!rig.exchange.has_stop_order(&stop), "the stop was cancelled");
        assert!(
            results.iter().any(|r| matches!(r, ActionResult::OrderPlaced { .. })),
            "{results:?}"
        );
        let sent = rig.exchange.market_order_client_ids();
        assert_eq!(sent.len(), 2, "the rejected one and the fresh one: {sent:?}");
        assert_ne!(sent[0], sent[1], "nothing more under the sealed id");
    }

    /// The fresh send is rejected too: exactly one fallback, the ordinary
    /// error.
    #[tokio::test]
    async fn a_rejected_fresh_send_is_the_one_fallback_and_returns_the_ordinary_error() {
        let rig = journal_rig();
        let position = rig.active_position(None).await;
        rig.exchange.set_trading_rules(lot_rules(dec!(0.001)));
        rig.exchange.set_rules_rejections_next(2);
        rig.backend.fail_commit(1, Some(JournalFault::Known));

        let error = rig
            .executor
            .execute(vec![exit_action(position.id, dec!(0.1))])
            .await
            .expect_err("the fresh send was refused too");

        assert!(matches!(error, ExecError::RulesRejected(_)), "{error:?}");
        assert_eq!(rig.sent_market_orders(), 2, "one first, one fresh, no loop");
        let (sealed, fresh) = exit_rows(&rig);
        let sent = rig.exchange.market_order_client_ids();
        assert_eq!(sealed.intent.as_ref().unwrap().client_order_id, sent[0]);
        assert!(fresh.expect("on record").outcome.expect("ended").uncertain_at.is_some());
    }

    /// An uncertain first attempt never reaches the fallback or a retry.
    #[tokio::test]
    async fn an_uncertain_first_attempt_does_not_fall_back() {
        let rig = journal_rig();
        let position = rig.active_position(None).await;
        rig.exchange.set_trading_rules(lot_rules(dec!(0.001)));
        rig.exchange.set_order_fail_next(true);
        // Every commit after the first fails, so a retry could only be refused.
        rig.backend.fail_commit(1, Some(JournalFault::Known));

        let error = rig
            .executor
            .execute(vec![exit_action(position.id, dec!(0.1))])
            .await
            .expect_err("the send failed");

        assert!(matches!(error, ExecError::Exchange(_)), "{error:?}");
        assert_eq!(rig.sent_market_orders(), 1);
        let (_, fresh) = exit_rows(&rig);
        assert!(fresh.is_none());
    }

    /// Stops have the same resubmission, and the fresh id keeps the `ins-`
    /// prefix the orphan sweep recognises.
    #[tokio::test]
    async fn a_refused_stop_retry_is_placed_under_a_fresh_ins_id() {
        let rig = journal_rig();
        let position = rig.active_position(None).await;
        rig.exchange.set_trading_rules(lot_rules(dec!(0.001)));
        rig.exchange.set_rules_rejections_next(1);
        rig.backend.fail_commit(1, Some(JournalFault::Known));

        let id = rig.placed_stop(position.id).await;

        assert!(rig.exchange.has_stop_order(&id));
        assert_eq!(rig.exchange.protection_call_count(), 2);
        let stored = rig.exchange.stop_order(&id).expect("the stop");
        assert!(stored.client_order_id.starts_with("ins-"), "{}", stored.client_order_id);
        let replacing = rig
            .backend
            .rows()
            .into_iter()
            .find(|row| {
                row.intent.as_ref().unwrap().params.get("replaces_client_order_id").is_some()
            })
            .expect("the fresh intent");
        assert_eq!(replacing.intent.unwrap().client_order_id, stored.client_order_id);
    }

    #[tokio::test]
    async fn an_entry_refused_before_the_send_has_no_ticket_and_is_not_sent() {
        let rig = journal_rig();
        rig.exchange.set_refuse_before_send_next(true);
        let results = rig.executor.execute(vec![entry_action()]).await.unwrap();
        assert!(matches!(results[0], ActionResult::EntryNotSent { .. }), "{results:?}");
        assert!(rig.row_of_kind(IntentKind::Entry).outcome.unwrap().not_sent);
    }

    #[tokio::test]
    async fn an_entry_failing_after_its_ticket_stays_an_unknown_outcome() {
        let rig = journal_rig();
        rig.exchange.set_order_fail_next(true);
        let results = rig.executor.execute(vec![entry_action()]).await.unwrap();
        assert!(matches!(results[0], ActionResult::OrderFailed { .. }), "{results:?}");
        let outcome = rig.row_of_kind(IntentKind::Entry).outcome.unwrap();
        assert!(outcome.uncertain_at.is_some() && outcome.venue_outcome.is_none());
    }

    /// Reviewed statement counts for each real path, read off the counter that
    /// production exports.
    #[tokio::test]
    async fn each_order_path_issues_its_reviewed_statement_count() {
        let rig = journal_rig();
        rig.executor.execute(vec![entry_action()]).await.unwrap();
        assert_eq!(rig.journal.operation_statements_max(), 9, "entry");

        let rig = journal_rig();
        let position = rig.active_position(None).await;
        rig.executor.execute(vec![exit_action(position.id, dec!(0.1))]).await.unwrap();
        assert_eq!(rig.journal.operation_statements_max(), 9, "exit");

        let rig = journal_rig();
        let position = rig.active_position(None).await;
        rig.exchange.set_trading_rules(lot_rules(dec!(0.001)));
        rig.exchange.set_rules_rejections_next(1);
        rig.executor.execute(vec![exit_action(position.id, dec!(0.1))]).await.unwrap();
        assert_eq!(rig.journal.operation_statements_max(), 13, "exit with one retry");

        let rig = journal_rig();
        let position = rig.active_position(None).await;
        let old = rig.placed_stop(position.id).await;
        assert_eq!(rig.journal.operation_statements_max(), 9, "stop");
        rig.executor
            .execute(vec![EngineAction::ReplaceInsuranceStop {
                position_id: position.id,
                symbol: Symbol::from_pair(SYMBOL).unwrap(),
                side: OrderSide::Sell,
                quantity: Quantity::new(dec!(0.1)).unwrap(),
                previous_order_id: old,
                new_stop_price: Price::new(dec!(91000)).unwrap(),
            }])
            .await
            .unwrap();
        assert_eq!(rig.journal.operation_statements_max(), 18, "replacement");

        let rig = journal_rig();
        let position = rig.active_position(None).await;
        rig.exchange.set_trading_rules(lot_rules(dec!(0.001)));
        rig.exchange.set_rules_rejections_next(1);
        rig.backend.fail_commit(1, Some(JournalFault::Known));
        rig.executor.execute(vec![exit_action(position.id, dec!(0.1))]).await.unwrap();
        assert_eq!(rig.journal.operation_statements_max(), 22, "exit with a replaced retry");

        // The worst reviewed case: a replaced retry on a placement that then
        // withdraws both stops, because the leg left `Active` meanwhile.
        let rig = journal_rig();
        let position = rig.active_position(None).await;
        let old = rig.placed_stop(position.id).await;
        rig.exchange.set_trading_rules(lot_rules(dec!(0.001)));
        rig.exchange.set_rules_rejections_next(1);
        rig.backend.fail_commit(1, Some(JournalFault::Known));
        let store = Arc::clone(&rig.executor.store);
        let position_id = position.id;
        rig.exchange.set_after_next_stop_accepted(move || {
            let price = Price::new(dec!(90000)).unwrap();
            store
                .apply_event(&Event::PositionClosed {
                    position_id,
                    exit_reason: ExitReason::InsuranceStop,
                    entry_price: price,
                    exit_price: price,
                    realized_pnl: Decimal::ZERO,
                    total_fees: Decimal::ZERO,
                    closure_evidence: robson_domain::ClosureEvidence::real_exit_fill(None),
                    timestamp: chrono::Utc::now(),
                })
                .unwrap();
        });
        rig.executor
            .execute(vec![EngineAction::ReplaceInsuranceStop {
                position_id: position.id,
                symbol: Symbol::from_pair(SYMBOL).unwrap(),
                side: OrderSide::Sell,
                quantity: Quantity::new(dec!(0.1)).unwrap(),
                previous_order_id: old,
                new_stop_price: Price::new(dec!(91000)).unwrap(),
            }])
            .await
            .unwrap();
        assert_eq!(rig.journal.operation_statements_max(), 40, "the worst reviewed case");
        assert_eq!(rig.journal.operations_over_statement_bound(), 0);
        assert_eq!(rig.journal.operations_over_statement_bound(), 0);
    }

    /// With the journal's memories full and its writes failing, every
    /// protective send still reaches the venue, on the real executor path.
    #[tokio::test]
    async fn a_saturated_failing_journal_stops_no_protective_send() {
        // A cancel-first exit: the stop goes, and so does the market exit.
        let rig = journal_rig();
        let position = rig.active_position(None).await;
        let stop = rig.placed_stop(position.id).await;
        rig.backend.saturate(&rig.journal).await;
        let lost_before = rig.journal.non_reuse_ids_lost();
        let results = rig
            .executor
            .execute(vec![
                cancel_action(position.id, &stop),
                exit_action(position.id, dec!(0.1)),
            ])
            .await
            .unwrap();
        assert!(!rig.exchange.has_stop_order(&stop), "the stop was cancelled");
        assert_eq!(rig.sent_market_orders(), 1, "and the exit reached the venue");
        assert!(
            results.iter().any(|r| matches!(r, ActionResult::OrderPlaced { .. })),
            "{results:?}"
        );
        assert!(rig.journal.non_reuse_ids_lost() > lost_before);

        // An initial stop.
        let rig = journal_rig();
        let position = rig.active_position(None).await;
        rig.backend.saturate(&rig.journal).await;
        rig.placed_stop(position.id).await;
        assert_eq!(rig.exchange.protection_call_count(), 1);

        // A replacement places its successor and cancels its predecessor.
        let rig = journal_rig();
        let position = rig.active_position(None).await;
        let old = rig.placed_stop(position.id).await;
        rig.backend.saturate(&rig.journal).await;
        rig.executor
            .execute(vec![EngineAction::ReplaceInsuranceStop {
                position_id: position.id,
                symbol: Symbol::from_pair(SYMBOL).unwrap(),
                side: OrderSide::Sell,
                quantity: Quantity::new(dec!(0.1)).unwrap(),
                previous_order_id: old.clone(),
                new_stop_price: Price::new(dec!(91000)).unwrap(),
            }])
            .await
            .unwrap();
        assert_eq!(rig.exchange.protection_call_count(), 2);
        assert!(!rig.exchange.has_stop_order(&old));
    }

    #[tokio::test]
    async fn every_order_is_listed_in_flight_only_while_it_runs() {
        let rig = journal_rig();
        rig.executor.execute(vec![entry_action()]).await.unwrap();
        assert_eq!(rig.journal.operations_in_flight(), 0);
    }

    // ---- ADR-0056 §5 ----------------------------------------------------------

    fn lot_rules(step: Decimal) -> SymbolTradingRules {
        SymbolTradingRules::new(
            Symbol::from_pair("BTCUSDT").unwrap(),
            dec!(0.10),
            dec!(556.80),
            step,
            step,
            dec!(120),
            dec!(50),
            2,
            3,
        )
        .unwrap()
    }

    fn exit_action(position_id: PositionId, quantity: Decimal) -> EngineAction {
        EngineAction::PlaceExitOrder {
            position_id,
            cycle_id: None,
            symbol: Symbol::from_pair("BTCUSDT").unwrap(),
            side: robson_domain::OrderSide::Sell,
            quantity: Quantity::new(quantity).unwrap(),
            reason: ExitReason::TrailingStop,
        }
    }

    fn cancel_action(position_id: PositionId, order_id: &str) -> EngineAction {
        EngineAction::CancelInsuranceStop {
            position_id,
            symbol: Symbol::from_pair("BTCUSDT").unwrap(),
            order_id: order_id.to_string(),
        }
    }

    async fn live_stop(executor: &Executor<StubExchange, MemoryStore>) -> String {
        executor
            .exchange
            .place_stop_market_order(
                &Symbol::from_pair("BTCUSDT").unwrap(),
                robson_domain::OrderSide::Sell,
                Quantity::new(dec!(0.1)).unwrap(),
                robson_domain::Price::new(dec!(90000)).unwrap(),
                "ins-test",
            )
            .await
            .unwrap()
            .exchange_order_id
    }

    async fn saved_leg(executor: &Executor<StubExchange, MemoryStore>, side: Side) -> Position {
        let mut leg = Position::new(Uuid::now_v7(), Symbol::from_pair("BTCUSDT").unwrap(), side);
        leg.entry_price = Some(robson_domain::Price::new(dec!(96000)).unwrap());
        leg.quantity = Quantity::new(dec!(1)).unwrap();
        leg.fees_paid = dec!(2);
        executor.store.positions().save(&leg).await.unwrap();
        leg
    }

    #[tokio::test]
    async fn a_rules_rejection_refreshes_once_and_resubmits_with_the_same_client_id() {
        let executor = create_test_executor().await;
        executor.exchange.set_trading_rules(lot_rules(dec!(0.001)));
        executor.exchange.set_venue_rules(lot_rules(dec!(0.01)));

        let results =
            executor.execute(vec![exit_action(Uuid::now_v7(), dec!(0.105))]).await.unwrap();

        assert_eq!(executor.exchange.market_order_call_count(), 2);
        assert_eq!(executor.exchange.rules_refresh_call_count(), 1);
        match results.last().unwrap() {
            ActionResult::OrderPlaced {
                order,
                event: Some(Event::ExitOrderPlaced { .. }),
            } => {
                assert_eq!(order.filled_quantity.as_decimal(), dec!(0.10));
                assert_eq!(executor.exchange.market_order_client_ids(), [
                    order.client_order_id.clone(),
                    order.client_order_id.clone()
                ]);
            },
            other => panic!("a fill is an exit: {other:?}"),
        }
    }

    /// Only risk reduction refreshes and resubmits: an unprotected second entry
    /// is exactly what a retry must not risk.
    #[tokio::test]
    async fn an_entry_is_not_resubmitted_on_a_rules_rejection() {
        let executor = create_test_executor().await;
        executor.exchange.set_trading_rules(lot_rules(dec!(0.001)));
        executor.exchange.set_rules_rejections_next(1);
        let signal_id = Uuid::now_v7();

        let results = executor
            .execute(vec![EngineAction::PlaceEntryOrder {
                position_id: Uuid::now_v7(),
                cycle_id: Some(Uuid::now_v7()),
                symbol: Symbol::from_pair("BTCUSDT").unwrap(),
                side: robson_domain::OrderSide::Buy,
                quantity: Quantity::new(dec!(0.1)).unwrap(),
                order_id: Uuid::now_v7(),
                client_order_id: signal_id.to_string(),
                expected_price: robson_domain::Price::new(dec!(95000)).unwrap(),
                signal_id,
            }])
            .await
            .unwrap();

        assert!(matches!(results[0], ActionResult::OrderFailed { .. }));
        assert_eq!(executor.exchange.market_order_call_count(), 1);
        assert_eq!(executor.exchange.rules_refresh_call_count(), 0);
    }

    #[tokio::test]
    async fn a_second_rules_rejection_does_not_loop() {
        let executor = create_test_executor().await;
        executor.exchange.set_trading_rules(lot_rules(dec!(0.001)));
        executor.exchange.set_rules_rejections_next(100);

        let result = executor.execute(vec![exit_action(Uuid::now_v7(), dec!(0.1))]).await;

        assert!(matches!(result, Err(ExecError::RulesRejected(_))));
        assert_eq!(executor.exchange.market_order_call_count(), 2, "no loop");
        assert_eq!(executor.exchange.rules_refresh_call_count(), 1, "no loop");
    }

    #[tokio::test]
    async fn a_failed_refresh_returns_the_rejection_without_resubmitting() {
        let executor = create_test_executor().await;
        executor.exchange.set_trading_rules(lot_rules(dec!(0.001)));
        executor.exchange.set_rules_rejections_next(1);
        executor.exchange.set_rules_refresh_fails(true);

        let result = executor.execute(vec![exit_action(Uuid::now_v7(), dec!(0.1))]).await;

        assert!(matches!(result, Err(ExecError::RulesRejected(_))));
        assert_eq!(executor.exchange.market_order_call_count(), 1);
        assert_eq!(executor.exchange.rules_refresh_call_count(), 1);
    }

    #[tokio::test]
    async fn other_failures_are_not_retried_by_the_quantisation_rule() {
        let executor = create_test_executor().await;
        executor.exchange.set_trading_rules(lot_rules(dec!(0.001)));
        executor.exchange.set_order_fail_next(true);

        let result = executor.execute(vec![exit_action(Uuid::now_v7(), dec!(0.1))]).await;

        assert!(matches!(result, Err(ExecError::Exchange(_))));
        assert_eq!(executor.exchange.market_order_call_count(), 1);
        assert_eq!(executor.exchange.rules_refresh_call_count(), 0);
    }

    /// The precedence of §5: newest validated rules first, because the cache is
    /// never older than a snapshot taken from it. Numbers differ per source.
    #[tokio::test]
    async fn the_cache_wins_over_the_snapshot_and_the_snapshot_over_the_lookup() {
        let filled = |results: &[ActionResult]| match results.last() {
            Some(ActionResult::OrderPlaced { order, .. }) => order.filled_quantity.as_decimal(),
            other => panic!("expected an accepted order, got {other:?}"),
        };
        let no_wait = std::time::Duration::from_secs(5);

        // Cache at step 0.01, snapshot at step 0.001: 0.105 goes out as 0.10.
        let executor = create_test_executor().await;
        let mut position =
            Position::new(Uuid::now_v7(), Symbol::from_pair("BTCUSDT").unwrap(), Side::Long);
        position.rules_snapshot = Some(lot_rules(dec!(0.001)));
        executor.store.positions().save(&position).await.unwrap();
        executor.exchange.set_trading_rules(lot_rules(dec!(0.01)));
        let results = executor.execute(vec![exit_action(position.id, dec!(0.105))]).await.unwrap();
        assert_eq!(filled(&results), dec!(0.10));

        // No cache: the snapshot rounds 0.1055 to 0.105, and nothing is fetched.
        let executor = create_test_executor().await;
        executor.exchange.set_rules_fetch_hangs(true);
        executor.store.positions().save(&position).await.unwrap();
        let results = tokio::time::timeout(
            no_wait,
            executor.execute(vec![exit_action(position.id, dec!(0.1055))]),
        )
        .await
        .expect("a reduction that holds rules must not wait for a fetch")
        .unwrap();
        assert_eq!(filled(&results), dec!(0.105));

        // Nothing held: the lookup answers, so the first submission already rounds.
        let executor = create_test_executor().await;
        executor.exchange.set_venue_rules(lot_rules(dec!(0.01)));
        let results =
            executor.execute(vec![exit_action(Uuid::now_v7(), dec!(0.1055))]).await.unwrap();
        assert_eq!(filled(&results), dec!(0.10));
        assert_eq!(executor.exchange.market_order_call_count(), 1);

        // Nothing held and the lookup fails: the quantity goes out as given.
        let executor = create_test_executor().await;
        let results =
            executor.execute(vec![exit_action(Uuid::now_v7(), dec!(0.1055))]).await.unwrap();
        assert_eq!(filled(&results), dec!(0.1055));
    }

    /// A fill closes the leg, whatever it left. The fill goes on the record
    /// first, stamped when the venue filled it, and nothing moves on the
    /// leg before the close: this is the only durable record of the filled
    /// quantity.
    #[tokio::test]
    async fn a_short_exit_records_its_fill_and_is_an_exit() {
        let executor = create_test_executor().await;
        let leg = saved_leg(&executor, Side::Long).await;
        executor.exchange.set_short_fill_next(dec!(0.3));

        let results = executor.execute(vec![exit_action(leg.id, dec!(1))]).await.unwrap();

        let order = match &results[1] {
            ActionResult::OrderPlaced {
                order,
                event: Some(Event::ExitOrderPlaced { .. }),
            } => order,
            other => panic!("a fill is an exit, got {other:?}"),
        };
        match &results[0] {
            ActionResult::EventEmitted(Event::ResidualInventoryRecorded {
                order_kind,
                requested_quantity,
                accepted_quantity,
                residual_quantity,
                exchange_order_id,
                fill_price,
                fee,
                timestamp,
                ..
            }) => {
                assert_eq!(*order_kind, ReductionKind::Exit);
                assert_eq!(*requested_quantity, dec!(1));
                assert_eq!(*accepted_quantity, dec!(0.3));
                assert_eq!(*residual_quantity, dec!(0.7));
                assert_eq!(exchange_order_id.as_ref(), Some(&order.exchange_order_id));
                assert_eq!(*fill_price, Some(order.fill_price));
                assert_eq!(*fee, Some(order.fee));
                assert_eq!(*timestamp, order.filled_at);
            },
            other => panic!("expected the fill on record, got {other:?}"),
        }
        let after = executor.store.positions().find_by_id(leg.id).await.unwrap().unwrap();
        assert_eq!(after.quantity.as_decimal(), dec!(1));
        assert_eq!(after.residual_quantity, None, "evidence of a fill leaves nothing standing");
        assert_eq!(after.realized_pnl, leg.realized_pnl);
        assert_eq!(after.fees_paid, leg.fees_paid);
    }

    /// A residual that happens to equal the one the leg stands at does not
    /// suppress the record of a fill.
    #[tokio::test]
    async fn a_short_exit_records_its_fill_whatever_the_leg_stands_at() {
        let executor = create_test_executor().await;
        let mut leg = saved_leg(&executor, Side::Long).await;
        leg.residual_quantity = Some(dec!(0.7));
        executor.store.positions().save(&leg).await.unwrap();
        executor.exchange.set_short_fill_next(dec!(0.3));

        let results = executor.execute(vec![exit_action(leg.id, dec!(1))]).await.unwrap();

        assert!(matches!(
            &results[0],
            ActionResult::EventEmitted(Event::ResidualInventoryRecorded { .. })
        ));
    }

    /// A full exit has nothing to record.
    #[tokio::test]
    async fn a_full_exit_records_no_residual() {
        let executor = create_test_executor().await;
        let leg = saved_leg(&executor, Side::Long).await;

        let results = executor.execute(vec![exit_action(leg.id, dec!(1))]).await.unwrap();

        assert_eq!(results.len(), 1);
        assert!(matches!(results[0], ActionResult::OrderPlaced {
            event: Some(Event::ExitOrderPlaced { .. }),
            ..
        }));
    }

    /// An unchanged residual on a leg that stays open writes nothing.
    #[tokio::test]
    async fn an_unchanged_standing_residual_writes_no_new_event() {
        let executor = create_test_executor().await;
        let leg = saved_leg(&executor, Side::Long).await;
        executor.exchange.set_trading_rules(lot_rules(dec!(0.01)));
        let stop = || EngineAction::PlaceInsuranceStop {
            position_id: leg.id,
            symbol: Symbol::from_pair("BTCUSDT").unwrap(),
            side: robson_domain::OrderSide::Sell,
            quantity: Quantity::new(dec!(0.105)).unwrap(),
            stop_price: robson_domain::Price::new(dec!(90000)).unwrap(),
        };
        let residuals = |results: &[ActionResult]| {
            results
                .iter()
                .filter(|r| {
                    matches!(r, ActionResult::EventEmitted(Event::ResidualInventoryRecorded { .. }))
                })
                .count()
        };

        assert_eq!(residuals(&executor.execute(vec![stop()]).await.unwrap()), 1);
        assert_eq!(residuals(&executor.execute(vec![stop()]).await.unwrap()), 0);
        let after = executor.store.positions().find_by_id(leg.id).await.unwrap().unwrap();
        assert_eq!(after.residual_quantity, Some(dec!(0.005)));
    }

    /// A stop accepted after its leg left Active is taken back and recorded
    /// nowhere, whichever way it was placed. A replacement takes its
    /// predecessor with it.
    #[tokio::test]
    async fn a_stop_accepted_after_its_leg_closed_is_withdrawn() {
        for replacing in [false, true] {
            let executor = create_test_executor().await;
            let mut leg = saved_leg(&executor, Side::Long).await;
            leg.state = PositionState::Active {
                current_price: robson_domain::Price::new(dec!(96000)).unwrap(),
                trailing_stop: robson_domain::Price::new(dec!(90000)).unwrap(),
                favorable_extreme: robson_domain::Price::new(dec!(96000)).unwrap(),
                extreme_at: chrono::Utc::now(),
                insurance_stop_id: None,
                invalidation_guard_level: None,
                last_emitted_stop: None,
            };
            executor.store.positions().save(&leg).await.unwrap();
            let previous = live_stop(&executor).await;
            let stops_before = executor.exchange.stop_order_count();

            let store = Arc::clone(&executor.store);
            let position_id = leg.id;
            executor.exchange.set_after_next_stop_accepted(move || {
                let price = robson_domain::Price::new(dec!(90000)).unwrap();
                store
                    .apply_event(&Event::PositionClosed {
                        position_id,
                        exit_reason: ExitReason::InsuranceStop,
                        entry_price: price,
                        exit_price: price,
                        realized_pnl: Decimal::ZERO,
                        total_fees: Decimal::ZERO,
                        closure_evidence: robson_domain::ClosureEvidence::real_exit_fill(None),
                        timestamp: chrono::Utc::now(),
                    })
                    .unwrap();
            });

            let symbol = Symbol::from_pair("BTCUSDT").unwrap();
            let quantity = Quantity::new(dec!(1)).unwrap();
            let stop_price = robson_domain::Price::new(dec!(90000)).unwrap();
            let action = if replacing {
                EngineAction::ReplaceInsuranceStop {
                    position_id: leg.id,
                    symbol,
                    side: robson_domain::OrderSide::Sell,
                    quantity,
                    previous_order_id: previous.clone(),
                    new_stop_price: stop_price,
                }
            } else {
                EngineAction::PlaceInsuranceStop {
                    position_id: leg.id,
                    symbol,
                    side: robson_domain::OrderSide::Sell,
                    quantity,
                    stop_price,
                }
            };

            let results = executor.execute(vec![action]).await.unwrap();

            assert!(matches!(results.as_slice(), [ActionResult::StateUpdated]), "{results:?}");
            let expected = if replacing {
                stops_before - 1
            } else {
                stops_before
            };
            assert_eq!(executor.exchange.stop_order_count(), expected, "nothing new stays live");
            assert_eq!(executor.exchange.has_stop_order(&previous), !replacing);
            let events = executor.store.events().find_by_position(leg.id).await.unwrap();
            assert!(events.is_empty(), "{events:?}");
        }
    }

    /// The same race, one await later: the leg closes while the predecessor's
    /// cancel is out, and only the successor is left to withdraw.
    #[tokio::test]
    async fn a_leg_that_closes_during_the_predecessor_cancel_gets_its_successor_withdrawn() {
        let executor = create_test_executor().await;
        let mut leg = saved_leg(&executor, Side::Long).await;
        leg.state = PositionState::Active {
            current_price: robson_domain::Price::new(dec!(96000)).unwrap(),
            trailing_stop: robson_domain::Price::new(dec!(90000)).unwrap(),
            favorable_extreme: robson_domain::Price::new(dec!(96000)).unwrap(),
            extreme_at: chrono::Utc::now(),
            insurance_stop_id: None,
            invalidation_guard_level: None,
            last_emitted_stop: None,
        };
        executor.store.positions().save(&leg).await.unwrap();
        let previous = live_stop(&executor).await;

        let store = Arc::clone(&executor.store);
        let position_id = leg.id;
        executor.exchange.set_after_next_stop_cancel(move || {
            let price = robson_domain::Price::new(dec!(90000)).unwrap();
            store
                .apply_event(&Event::PositionClosed {
                    position_id,
                    exit_reason: ExitReason::InsuranceStop,
                    entry_price: price,
                    exit_price: price,
                    realized_pnl: Decimal::ZERO,
                    total_fees: Decimal::ZERO,
                    closure_evidence: robson_domain::ClosureEvidence::real_exit_fill(None),
                    timestamp: chrono::Utc::now(),
                })
                .unwrap();
        });

        let results = executor
            .execute(vec![EngineAction::ReplaceInsuranceStop {
                position_id: leg.id,
                symbol: Symbol::from_pair("BTCUSDT").unwrap(),
                side: robson_domain::OrderSide::Sell,
                quantity: Quantity::new(dec!(1)).unwrap(),
                previous_order_id: previous.clone(),
                new_stop_price: robson_domain::Price::new(dec!(90000)).unwrap(),
            }])
            .await
            .unwrap();

        assert!(matches!(results.as_slice(), [ActionResult::StateUpdated]), "{results:?}");
        assert_eq!(executor.exchange.stop_order_count(), 0, "neither stop stays live");
        let events = executor.store.events().find_by_position(leg.id).await.unwrap();
        assert!(events.is_empty(), "{events:?}");
    }

    #[tokio::test]
    async fn a_reduce_only_refusal_is_returned_as_it_is() {
        let executor = create_test_executor().await;
        executor.exchange.set_reduce_only_rejections_next(1);

        let result = executor.execute(vec![exit_action(Uuid::now_v7(), dec!(0.1))]).await;

        assert!(matches!(result, Err(ExecError::ReduceOnlyRejected(_))));
        assert_eq!(executor.exchange.market_order_call_count(), 1);
    }

    /// Cancel-first: the stop goes, then the exit fails. The cancel happened,
    /// so its event must reach the caller to be persisted, and the failure
    /// with it.
    #[tokio::test]
    async fn an_exit_that_fails_behind_a_cancel_keeps_the_cancel_on_the_record() {
        let executor = create_test_executor().await;
        let position_id = Uuid::now_v7();
        let stop = live_stop(&executor).await;
        executor.exchange.set_order_fail_next(true);

        let results = executor
            .execute(vec![
                cancel_action(position_id, &stop),
                exit_action(position_id, dec!(0.1)),
            ])
            .await
            .unwrap();

        assert!(!executor.exchange.has_stop_order(&stop));
        assert!(matches!(
            results[0],
            ActionResult::EventEmitted(Event::InsuranceStopCancelled { .. })
        ));
        assert!(matches!(results[1], ActionResult::ExitFailed { error: ExecError::Exchange(_) }));
    }

    /// Held rules say nothing is executable. They may be stale, so the venue is
    /// asked without waiting for anything: it refuses, and the refreshed rules
    /// agree. Nothing executed and the batch stops.
    #[tokio::test]
    async fn an_unexecutable_exit_is_decided_by_the_venue_and_stops_the_batch() {
        let executor = create_test_executor().await;
        executor.exchange.set_trading_rules(lot_rules(dec!(0.001)));
        executor.exchange.set_venue_rules(lot_rules(dec!(0.001)));
        let position_id = Uuid::now_v7();

        let results = executor
            .execute(vec![
                exit_action(position_id, dec!(0.0005)),
                EngineAction::EmitEvent(Event::ExitTriggered {
                    position_id,
                    reason: ExitReason::TrailingStop,
                    trigger_price: robson_domain::Price::new(dec!(90000)).unwrap(),
                    stop_price: robson_domain::Price::new(dec!(90000)).unwrap(),
                    timestamp: chrono::Utc::now(),
                }),
            ])
            .await
            .unwrap();

        assert_eq!(executor.exchange.market_order_call_count(), 1, "submitted raw, refused");
        assert_eq!(executor.exchange.rules_refresh_call_count(), 1);
        assert_eq!(results.len(), 1, "the rest of the batch never ran");
        assert!(matches!(
            &results[0],
            ActionResult::ExitNotExecutable {
                event: Some(Event::ResidualInventoryRecorded { .. }),
                residual,
            } if *residual == dec!(0.0005)
        ));
    }

    /// A stale view that says nothing is executable costs no wait: with a
    /// metadata fetch hanging, the order still goes out, and the venue
    /// accepts it.
    #[tokio::test]
    async fn a_stale_view_of_the_lot_does_not_delay_a_reduction() {
        let executor = create_test_executor().await;
        let mut position =
            Position::new(Uuid::now_v7(), Symbol::from_pair("BTCUSDT").unwrap(), Side::Long);
        position.rules_snapshot = Some(lot_rules(dec!(0.01)));
        executor.store.positions().save(&position).await.unwrap();
        executor.exchange.set_rules_fetch_hangs(true);

        let results = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            executor.execute(vec![exit_action(position.id, dec!(0.005))]),
        )
        .await
        .expect("nothing before the first submission may wait for metadata")
        .unwrap();

        assert!(matches!(
            &results[0],
            ActionResult::OrderPlaced { order, .. } if order.filled_quantity.as_decimal() == dec!(0.005)
        ));
        assert_eq!(executor.exchange.rules_refresh_call_count(), 0);
    }

    /// The refresh fails too: the venue refused and the rules held already said
    /// so.
    #[tokio::test]
    async fn an_unexecutable_exit_survives_a_failing_refresh() {
        let executor = create_test_executor().await;
        executor.exchange.set_trading_rules(lot_rules(dec!(0.001)));
        executor.exchange.set_venue_rules(lot_rules(dec!(0.001)));
        executor.exchange.set_rules_refresh_fails(true);

        let results =
            executor.execute(vec![exit_action(Uuid::now_v7(), dec!(0.0005))]).await.unwrap();

        assert!(matches!(results[0], ActionResult::ExitNotExecutable { .. }));
        assert_eq!(executor.exchange.market_order_call_count(), 1);
    }

    #[tokio::test]
    async fn protection_quantizes_on_the_real_lot_and_aligns_the_trigger_adversely() {
        let executor = create_test_executor().await;
        executor.exchange.set_trading_rules(lot_rules(dec!(0.01)));
        let position_id = Uuid::now_v7();
        let trigger = robson_domain::Price::new(dec!(62873.86105)).unwrap();

        let results = executor
            .execute(vec![EngineAction::PlaceInsuranceStop {
                position_id,
                symbol: Symbol::from_pair("BTCUSDT").unwrap(),
                side: robson_domain::OrderSide::Sell,
                quantity: Quantity::new(dec!(0.105)).unwrap(),
                stop_price: trigger,
            }])
            .await
            .unwrap();

        let placed = match &results[0] {
            ActionResult::OrderPlaced { order, .. } => {
                executor.exchange.stop_order(&order.exchange_order_id).unwrap()
            },
            other => panic!("expected a placed stop, got {other:?}"),
        };
        assert_eq!(placed.quantity.as_decimal(), dec!(0.10));
        assert_eq!(placed.stop_price.as_decimal(), dec!(62873.80), "adverse for a Sell stop");
        assert!(matches!(
            &results[1],
            ActionResult::EventEmitted(Event::ResidualInventoryRecorded {
                order_kind: ReductionKind::Protection,
                residual_quantity,
                ..
            }) if *residual_quantity == dec!(0.005)
        ));
    }

    #[tokio::test]
    async fn a_refused_stop_placement_is_audit_only_after_one_refresh() {
        let executor = create_test_executor().await;
        executor.exchange.set_trading_rules(lot_rules(dec!(0.001)));
        executor.exchange.set_rules_rejections_next(100);

        let results = executor
            .execute(vec![EngineAction::PlaceInsuranceStop {
                position_id: Uuid::now_v7(),
                symbol: Symbol::from_pair("BTCUSDT").unwrap(),
                side: robson_domain::OrderSide::Sell,
                quantity: Quantity::new(dec!(0.1)).unwrap(),
                stop_price: robson_domain::Price::new(dec!(90000)).unwrap(),
            }])
            .await
            .unwrap();

        assert_eq!(executor.exchange.protection_call_count(), 2);
        assert_eq!(executor.exchange.rules_refresh_call_count(), 1);
        assert!(matches!(
            &results[0],
            ActionResult::EventEmitted(Event::InsuranceStopFailed { .. })
        ));
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
    async fn test_entry_order_failure_is_not_retried() {
        let exchange = Arc::new(StubExchange::new(dec!(95000)));
        exchange.set_order_fail_next(true);
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
            ActionResult::OrderFailed {
                event: Event::EntryOrderFailed { cycle_id: actual, .. },
                error,
            } => {
                assert_eq!(*actual, cycle_id);
                assert!(error.contains("Simulated exchange failure"));
            },
            other => panic!("Expected non-retried OrderFailed, got {:?}", other),
        }
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
            stop_policy: StopPolicy::LegacyUncapped,
            stop_buffer_bps_at_arm: None,
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
                stop_policy: StopPolicy::LegacyUncapped,
                stop_buffer_bps_at_arm: None,
            }),
            EngineAction::EmitEvent(Event::EntrySignalReceived {
                position_id,
                signal_id,
                entry_price: robson_domain::Price::new(dec!(95000)).unwrap(),
                stop_loss: robson_domain::Price::new(dec!(93500)).unwrap(),
                quantity: Quantity::new(dec!(0.1)).unwrap(),
                initial_executable_stop: None,
                executable_span: None,
                cap_basis_distance: None,
                tick_size: None,
                rules_snapshot: None,
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

    // =========================================================================
    // ADR-0039 Insurance Stop Tests
    // =========================================================================

    fn insurance_executor() -> (Executor<StubExchange, MemoryStore>, Arc<StubExchange>) {
        let exchange = Arc::new(StubExchange::new(dec!(95000)));
        let journal = Arc::new(IntentJournal::new());
        let store = Arc::new(MemoryStore::new());
        let executor = Executor::new(Arc::clone(&exchange), journal, store);
        (executor, exchange)
    }

    fn place_insurance_action(position_id: Uuid, stop_price: Decimal) -> EngineAction {
        EngineAction::PlaceInsuranceStop {
            position_id,
            symbol: Symbol::from_pair("BTCUSDT").unwrap(),
            side: OrderSide::Sell,
            quantity: Quantity::new(dec!(0.1)).unwrap(),
            stop_price: Price::new(stop_price).unwrap(),
        }
    }

    #[tokio::test]
    async fn test_execute_place_insurance_stop_places_order_and_emits_event() {
        let (executor, exchange) = insurance_executor();
        let position_id = Uuid::now_v7();

        let results = executor
            .execute(vec![place_insurance_action(position_id, dec!(93500))])
            .await
            .unwrap();

        assert_eq!(results.len(), 1);
        // A reduce-only stop was accepted (unfilled) on the stub.
        assert_eq!(exchange.stop_order_count(), 1);

        match &results[0] {
            ActionResult::OrderPlaced {
                order,
                event: Some(Event::InsuranceStopPlaced { order_id, stop_price, .. }),
            } => {
                assert!(!order.exchange_order_id.is_empty());
                assert_eq!(order.filled_quantity.as_decimal(), dec!(0)); // accepted, unfilled
                assert_eq!(order_id, &order.exchange_order_id);
                assert_eq!(stop_price.as_decimal(), dec!(93500));
                assert!(exchange.has_stop_order(order_id));
            },
            other => panic!("Expected OrderPlaced with InsuranceStopPlaced, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_execute_place_insurance_stop_failure_emits_failed_event() {
        let (executor, exchange) = insurance_executor();
        exchange.set_order_fail_next(true);
        let position_id = Uuid::now_v7();

        // Must NOT error out of execute() — the batch continues.
        let results = executor
            .execute(vec![place_insurance_action(position_id, dec!(93500))])
            .await
            .unwrap();

        assert_eq!(results.len(), 1);
        match &results[0] {
            ActionResult::EventEmitted(Event::InsuranceStopFailed {
                stop_price, error, ..
            }) => {
                assert_eq!(stop_price.as_decimal(), dec!(93500));
                assert!(error.contains("Simulated stop order failure"));
            },
            other => panic!("Expected EventEmitted(InsuranceStopFailed), got {:?}", other),
        }
        // No stop order was left on the stub.
        assert_eq!(exchange.stop_order_count(), 0);
    }

    #[tokio::test]
    async fn test_execute_cancel_insurance_stop_cancels_order() {
        let (executor, exchange) = insurance_executor();
        let position_id = Uuid::now_v7();

        // Place a stop first so there is an order id to cancel.
        let order_id = match &executor
            .execute(vec![place_insurance_action(position_id, dec!(93500))])
            .await
            .unwrap()[0]
        {
            ActionResult::OrderPlaced {
                event: Some(Event::InsuranceStopPlaced { order_id, .. }),
                ..
            } => order_id.clone(),
            other => panic!("Expected OrderPlaced, got {:?}", other),
        };
        assert_eq!(exchange.stop_order_count(), 1);

        let cancel_action = EngineAction::CancelInsuranceStop {
            position_id,
            symbol: Symbol::from_pair("BTCUSDT").unwrap(),
            order_id: order_id.clone(),
        };
        let results = executor.execute(vec![cancel_action]).await.unwrap();
        assert_eq!(results.len(), 1);
        match &results[0] {
            ActionResult::EventEmitted(Event::InsuranceStopCancelled {
                order_id: cancelled_id,
                ..
            }) => {
                assert_eq!(cancelled_id, &order_id);
            },
            other => panic!("Expected EventEmitted(InsuranceStopCancelled), got {:?}", other),
        }

        // The stop was removed from the stub.
        assert_eq!(exchange.stop_order_count(), 0);
        assert!(!exchange.has_stop_order(&order_id));
    }

    /// Issue #154 deliverable 6: replacement is PLACE-THEN-CANCEL. When the
    /// new placement fails, the previous stop must remain live on the
    /// exchange (protection is never dropped before a validated
    /// replacement exists).
    #[tokio::test]
    async fn test_replace_insurance_stop_keeps_previous_when_placement_fails() {
        let (executor, exchange) = insurance_executor();
        let position_id = Uuid::now_v7();

        let previous_id = match &executor
            .execute(vec![place_insurance_action(position_id, dec!(93500))])
            .await
            .unwrap()[0]
        {
            ActionResult::OrderPlaced {
                event: Some(Event::InsuranceStopPlaced { order_id, .. }),
                ..
            } => order_id.clone(),
            other => panic!("Expected OrderPlaced, got {:?}", other),
        };

        exchange.set_order_fail_next(true);
        let replace_action = EngineAction::ReplaceInsuranceStop {
            position_id,
            symbol: Symbol::from_pair("BTCUSDT").unwrap(),
            side: OrderSide::Sell,
            quantity: Quantity::new(dec!(0.1)).unwrap(),
            previous_order_id: previous_id.clone(),
            new_stop_price: Price::new(dec!(94000)).unwrap(),
        };
        let results = executor.execute(vec![replace_action]).await.unwrap();

        assert!(matches!(
            &results[0],
            ActionResult::EventEmitted(Event::InsuranceStopFailed { .. })
        ));
        assert!(
            exchange.has_stop_order(&previous_id),
            "previous stop must remain live when the replacement placement fails"
        );
    }

    /// Issue #154 deliverable 6: on success, the replacement is placed and
    /// only then the previous stop is cancelled.
    #[tokio::test]
    async fn test_replace_insurance_stop_places_then_cancels() {
        let (executor, exchange) = insurance_executor();
        let position_id = Uuid::now_v7();

        let previous_id = match &executor
            .execute(vec![place_insurance_action(position_id, dec!(93500))])
            .await
            .unwrap()[0]
        {
            ActionResult::OrderPlaced {
                event: Some(Event::InsuranceStopPlaced { order_id, .. }),
                ..
            } => order_id.clone(),
            other => panic!("Expected OrderPlaced, got {:?}", other),
        };

        let replace_action = EngineAction::ReplaceInsuranceStop {
            position_id,
            symbol: Symbol::from_pair("BTCUSDT").unwrap(),
            side: OrderSide::Sell,
            quantity: Quantity::new(dec!(0.1)).unwrap(),
            previous_order_id: previous_id.clone(),
            new_stop_price: Price::new(dec!(94000)).unwrap(),
        };
        let results = executor.execute(vec![replace_action]).await.unwrap();

        match &results[0] {
            ActionResult::OrderPlaced {
                event: Some(Event::InsuranceStopReplaced { previous_order_id, order_id, .. }),
                ..
            } => {
                assert_eq!(previous_order_id, &previous_id);
                assert!(exchange.has_stop_order(order_id), "replacement must be live");
            },
            other => panic!("Expected OrderPlaced with InsuranceStopReplaced, got {:?}", other),
        }
        assert!(!exchange.has_stop_order(&previous_id), "previous stop must be cancelled");
        assert_eq!(exchange.stop_order_count(), 1);
    }

    // ---- ADR-0056 step 7, PR C: typed cancel classes
    // ------------------------------

    use crate::{
        order_journal::VenueOutcome,
        venue::{OrderLookup, StopCancelOutcome as VenueCancel},
    };

    /// Every class, each with its expected label; the last one is the broad
    /// variant and message that used to read as "cancelled".
    fn cancel_cases() -> Vec<(Result<VenueCancel, ExecError>, &'static str)> {
        vec![
            (
                Ok(VenueCancel::Cancelled {
                    filled_before_cancel: Decimal::ZERO,
                    fills: None,
                }),
                "cancelled",
            ),
            (
                Ok(VenueCancel::AlreadyGone {
                    final_state: OrderLookup::Terminal {
                        filled_qty: dec!(0.1),
                        cancelled_qty: Decimal::ZERO,
                        fills: None,
                    },
                }),
                "gone_terminal",
            ),
            (
                Ok(VenueCancel::AlreadyGone { final_state: OrderLookup::Unknown }),
                "gone_unresolved",
            ),
            (Ok(VenueCancel::Unknown), "indeterminate"),
            (Err(ExecError::Timeout("venue timed out".to_string())), "error"),
            (Err(ExecError::OrderRejected("Unknown order sent.".to_string())), "error"),
        ]
    }

    fn expected_outcome(label: &str) -> Option<VenueOutcome> {
        match label {
            "cancelled" => Some(VenueOutcome::Success),
            "gone_terminal" => Some(VenueOutcome::GoneTerminal),
            _ => None,
        }
    }

    fn replace_action(position_id: PositionId, previous: &str) -> EngineAction {
        EngineAction::ReplaceInsuranceStop {
            position_id,
            symbol: Symbol::from_pair(SYMBOL).unwrap(),
            side: OrderSide::Sell,
            quantity: Quantity::new(dec!(0.1)).unwrap(),
            previous_order_id: previous.to_string(),
            new_stop_price: Price::new(dec!(91000)).unwrap(),
        }
    }

    fn place_action(position_id: PositionId) -> EngineAction {
        EngineAction::PlaceInsuranceStop {
            position_id,
            symbol: Symbol::from_pair(SYMBOL).unwrap(),
            side: OrderSide::Sell,
            quantity: Quantity::new(dec!(0.1)).unwrap(),
            stop_price: Price::new(dec!(90000)).unwrap(),
        }
    }

    /// The batch under a bound, so a journal write that blocks fails the test
    /// instead of hanging it.
    async fn execute_bounded(
        rig: &JournalRig,
        actions: Vec<EngineAction>,
    ) -> ExecResult<Vec<ActionResult>> {
        tokio::time::timeout(std::time::Duration::from_secs(5), rig.executor.execute(actions))
            .await
            .expect("a journal write held the batch")
    }

    impl JournalRig {
        fn rows_of_kind(&self, kind: IntentKind) -> Vec<crate::order_journal::StubJournalRow> {
            self.backend
                .rows()
                .into_iter()
                .filter(|r| r.intent.as_ref().unwrap().kind == kind)
                .collect()
        }

        /// The one row of a cancel of `order_id`, ending as `label`.
        fn assert_cancel_row(&self, label: &str, order_id: &str) {
            let row = self
                .rows_of_kind(IntentKind::Cancel)
                .into_iter()
                .find(|r| r.intent.as_ref().unwrap().cancels_order_id.as_deref() == Some(order_id))
                .unwrap_or_else(|| panic!("{label}: no cancel of {order_id}"));
            let intent = row.intent.unwrap();
            assert!(intent.client_order_id.starts_with("cxl-"), "{label}: a local id");
            assert_eq!(row.attempts.len(), 1, "{label}");
            let outcome = row.outcome.unwrap_or_else(|| panic!("{label}: no outcome"));
            assert_eq!(outcome.cancel_class, Some(label), "{label}");
            assert_eq!(outcome.venue_outcome, expected_outcome(label), "{label}");
            assert_eq!(
                outcome.uncertain_at.is_some(),
                expected_outcome(label).is_none(),
                "{label}"
            );
            assert!(!outcome.not_sent, "{label}");
        }

        /// Each placement ended a plain success, untouched by any cancel.
        fn assert_placements_succeeded(&self, count: usize, label: &str) {
            let placements = self.rows_of_kind(IntentKind::Protection);
            assert_eq!(placements.len(), count, "{label}");
            for placement in placements {
                let outcome = placement.outcome.unwrap_or_else(|| panic!("{label}: no outcome"));
                assert_eq!(outcome.venue_outcome, Some(VenueOutcome::Success), "{label}");
                assert_eq!(outcome.cancel_class, None, "{label}");
                assert_eq!(outcome.uncertain_at, None, "{label}");
            }
        }

        async fn events(&self, position_id: PositionId) -> Vec<Event> {
            self.executor.store().events().find_by_position(position_id).await.unwrap()
        }

        async fn stop_id_of(&self, position_id: PositionId) -> Option<String> {
            let leg = self
                .executor
                .store()
                .positions()
                .find_by_id(position_id)
                .await
                .unwrap()
                .unwrap();
            match leg.state {
                PositionState::Active { insurance_stop_id, .. } => insurance_stop_id,
                other => panic!("the leg left Active: {other:?}"),
            }
        }

        /// A leg that closes the moment the next stop is accepted, so that
        /// stop is withdrawn.
        async fn leg_closing_at_the_next_stop(&self) -> Position {
            let position = self.active_position(None).await;
            let store = Arc::clone(&self.executor.store);
            let position_id = position.id;
            self.exchange.set_after_next_stop_accepted(move || {
                let price = Price::new(dec!(90000)).unwrap();
                store
                    .apply_event(&Event::PositionClosed {
                        position_id,
                        exit_reason: ExitReason::InsuranceStop,
                        entry_price: price,
                        exit_price: price,
                        realized_pnl: Decimal::ZERO,
                        total_fees: Decimal::ZERO,
                        closure_evidence: robson_domain::ClosureEvidence::real_exit_fill(None),
                        timestamp: chrono::Utc::now(),
                    })
                    .unwrap();
            });
            position
        }
    }

    /// Replacement, every class: the placement stays a success and the
    /// predecessor's cancel keeps its own class on its own intent.
    #[tokio::test]
    async fn a_replacement_succeeds_under_every_cancel_class() {
        for (outcome, label) in cancel_cases() {
            let rig = journal_rig();
            let position = rig.active_position(None).await;
            let old = rig.placed_stop(position.id).await;
            rig.exchange.set_stop_cancel_outcome_next(outcome);

            let results =
                rig.executor.execute(vec![replace_action(position.id, &old)]).await.unwrap();

            let new_id = match &results[0] {
                ActionResult::OrderPlaced {
                    event: Some(Event::InsuranceStopReplaced { previous_order_id, order_id, .. }),
                    ..
                } => {
                    assert_eq!(previous_order_id, &old, "{label}");
                    order_id.clone()
                },
                other => panic!("{label}: {other:?}"),
            };
            assert!(rig.exchange.has_stop_order(&new_id), "{label}: the successor is live");
            rig.assert_cancel_row(label, &old);
            rig.assert_placements_succeeded(2, label);
            let events = rig.events(position.id).await;
            assert!(
                !events.iter().any(|e| matches!(
                    e,
                    Event::InsuranceStopCancelled { .. } | Event::InsuranceStopFailed { .. }
                )),
                "{label}: {events:?}"
            );
        }
    }

    /// The case §2.3 item 4 names: placement succeeded, cancel uncertain, and
    /// the two are separate intents whose rows do not share anything.
    #[tokio::test]
    async fn a_successful_placement_never_absorbs_an_uncertain_cancel() {
        let rig = journal_rig();
        let position = rig.active_position(None).await;
        let old = rig.placed_stop(position.id).await;
        rig.exchange
            .set_stop_cancel_outcome_next(Err(ExecError::Timeout("cancel unanswered".to_string())));

        rig.executor.execute(vec![replace_action(position.id, &old)]).await.unwrap();

        let placements = rig.rows_of_kind(IntentKind::Protection);
        let cancels = rig.rows_of_kind(IntentKind::Cancel);
        assert_eq!((placements.len(), cancels.len()), (2, 1));
        let cancel = cancels.into_iter().next().unwrap();
        let cancel_intent = cancel.intent.unwrap();
        for placement in placements {
            let intent = placement.intent.unwrap();
            assert_ne!(intent.intent_id, cancel_intent.intent_id);
            assert_ne!(intent.client_order_id, cancel_intent.client_order_id);
            assert_eq!(intent.cancels_order_id, None);
            let outcome = placement.outcome.unwrap();
            assert_eq!(outcome.venue_outcome, Some(VenueOutcome::Success));
            assert_eq!(outcome.uncertain_at, None);
        }
        let outcome = cancel.outcome.unwrap();
        assert_eq!(outcome.venue_outcome, None);
        assert_eq!(outcome.cancel_class, Some("error"));
        assert!(outcome.uncertain_at.is_some());
    }

    /// Withdrawal, every class: the placement stays a success, the cancel
    /// keeps its class, and the leg is left alone.
    #[tokio::test]
    async fn a_withdrawal_records_every_cancel_class() {
        for (outcome, label) in cancel_cases() {
            let rig = journal_rig();
            let position = rig.leg_closing_at_the_next_stop().await;
            rig.exchange.set_stop_cancel_outcome_next(outcome);

            let results = rig.executor.execute(vec![place_action(position.id)]).await.unwrap();

            assert!(matches!(results[0], ActionResult::StateUpdated), "{label}: {results:?}");
            let placed = rig.row_of_kind(IntentKind::Protection);
            let evidence = placed.outcome.as_ref().unwrap().venue_evidence.clone().unwrap();
            let stop_id = evidence["exchange_order_id"].as_str().unwrap().to_string();
            rig.assert_cancel_row(label, &stop_id);
            rig.assert_placements_succeeded(1, label);
            assert!(
                rig.events(position.id).await.is_empty(),
                "{label}: a withdrawal emits nothing"
            );
        }
    }

    /// A replacement that withdraws both stops: each cancel is its own intent,
    /// and an uncertain first one does not stop the second.
    #[tokio::test]
    async fn each_withdrawn_stop_keeps_its_own_class() {
        let rig = journal_rig();
        let position = rig.leg_closing_at_the_next_stop().await;
        let old = live_stop(&rig.executor).await;
        rig.exchange.set_stop_cancel_outcome_next(Ok(VenueCancel::Unknown));

        rig.executor.execute(vec![replace_action(position.id, &old)]).await.unwrap();

        let placed = rig.row_of_kind(IntentKind::Protection);
        let evidence = placed.outcome.as_ref().unwrap().venue_evidence.clone().unwrap();
        let new_id = evidence["exchange_order_id"].as_str().unwrap().to_string();
        rig.assert_cancel_row("indeterminate", &new_id);
        rig.assert_cancel_row("cancelled", &old);
        assert!(!rig.exchange.has_stop_order(&old), "the second withdrawal went out");
        assert_eq!(rig.rows_of_kind(IntentKind::Cancel).len(), 2);
    }

    /// The software-exit table of §2.3 item 4, row by row.
    #[tokio::test]
    async fn a_software_exit_cancel_acts_on_its_class() {
        for (outcome, label) in cancel_cases() {
            let rig = journal_rig();
            let position = rig.active_position(Some("stop-1")).await;
            rig.exchange.set_stop_cancel_outcome_next(outcome);

            let results =
                rig.executor.execute(vec![cancel_action(position.id, "stop-1")]).await.unwrap();

            rig.assert_cancel_row(label, "stop-1");
            let events = rig.events(position.id).await;
            let cancelled = events
                .iter()
                .filter(|e| matches!(e, Event::InsuranceStopCancelled { .. }))
                .count();
            let failed =
                events.iter().filter(|e| matches!(e, Event::InsuranceStopFailed { .. })).count();
            let stop_id = rig.stop_id_of(position.id).await;
            match label {
                "cancelled" => {
                    assert!(
                        matches!(
                            results[0],
                            ActionResult::EventEmitted(Event::InsuranceStopCancelled { .. })
                        ),
                        "{label}: {results:?}"
                    );
                    assert_eq!((cancelled, failed), (1, 0), "{label}");
                    assert_eq!(stop_id, None, "{label}: the id goes with a real cancel");
                },
                "gone_terminal" | "gone_unresolved" => {
                    assert!(matches!(results[0], ActionResult::Skipped(_)), "{label}: {results:?}");
                    assert_eq!((cancelled, failed), (0, 0), "{label}");
                    assert_eq!(stop_id.as_deref(), Some("stop-1"), "{label}: the id stays");
                },
                "indeterminate" | "error" => {
                    assert!(
                        matches!(
                            results[0],
                            ActionResult::EventEmitted(Event::InsuranceStopFailed { .. })
                        ),
                        "{label}: {results:?}"
                    );
                    assert_eq!((cancelled, failed), (0, 1), "{label}");
                    assert_eq!(stop_id.as_deref(), Some("stop-1"), "{label}: the id stays");
                },
                other => panic!("unexpected label {other}"),
            }
        }
    }

    /// "The exit proceeds as today": the market exit runs behind every class.
    #[tokio::test]
    async fn the_exit_proceeds_behind_every_cancel_class() {
        for (outcome, label) in cancel_cases() {
            let rig = journal_rig();
            let position = rig.active_position(Some("stop-1")).await;
            rig.exchange.set_stop_cancel_outcome_next(outcome);

            let results = rig
                .executor
                .execute(vec![
                    cancel_action(position.id, "stop-1"),
                    exit_action(position.id, dec!(0.1)),
                ])
                .await
                .unwrap();

            assert!(
                results.iter().any(|r| matches!(r, ActionResult::OrderPlaced { .. })),
                "{label}: {results:?}"
            );
            assert_eq!(rig.sent_market_orders(), 1, "{label}");
            rig.assert_cancel_row(label, "stop-1");
            assert_eq!(rig.rows_of_kind(IntentKind::Exit).len(), 1, "{label}");
        }
    }

    /// A stop that really filled on the stub reaches `GoneTerminal` through the
    /// ordinary path, with its fills on the cancel intent's evidence.
    #[tokio::test]
    async fn a_filled_stop_records_its_fill_on_the_cancel_intent() {
        let rig = journal_rig();
        let position = rig.active_position(None).await;
        let stop = rig.placed_stop(position.id).await;
        rig.exchange.fill_stop_order(&stop).unwrap();

        let results = rig.executor.execute(vec![cancel_action(position.id, &stop)]).await.unwrap();

        assert!(matches!(results[0], ActionResult::Skipped(_)), "{results:?}");
        rig.assert_cancel_row("gone_terminal", &stop);
        let cancel = rig.row_of_kind(IntentKind::Cancel).outcome.unwrap();
        let evidence = cancel.venue_evidence.expect("the fills are on the cancel intent");
        assert_eq!(evidence["filled_qty"], serde_json::json!(dec!(0.1)));
        assert_eq!(
            evidence["fills"]["exchange_order_id"].as_str(),
            Some(format!("TRIGGERED-{stop}").as_str())
        );
    }

    /// `is_unknown_order_error` is gone: neither the variants nor the text it
    /// keyed on can record a cancellation or clear the stop id.
    #[tokio::test]
    async fn no_error_text_records_a_cancellation() {
        for text in [
            "Unknown order sent.",
            "Order does not exist.",
            "order already gone",
            "order cancelled",
        ] {
            for error in [
                ExecError::OrderRejected(text.to_string()),
                ExecError::Exchange(text.to_string()),
            ] {
                let case = format!("{error:?}");
                let rig = journal_rig();
                let position = rig.active_position(Some("stop-1")).await;
                rig.exchange.set_stop_cancel_outcome_next(Err(error));

                let results =
                    rig.executor.execute(vec![cancel_action(position.id, "stop-1")]).await.unwrap();

                assert!(
                    matches!(
                        results[0],
                        ActionResult::EventEmitted(Event::InsuranceStopFailed { .. })
                    ),
                    "{case}: {results:?}"
                );
                assert_eq!(rig.stop_id_of(position.id).await.as_deref(), Some("stop-1"), "{case}");
                rig.assert_cancel_row("error", "stop-1");
            }
        }
    }

    /// Replacement: a cancel's journal write that never answers, before the
    /// request or after it, costs only its bound. Writes 0 and 1 are the
    /// placement's, 2 and 3 the cancel's.
    #[tokio::test]
    async fn a_hung_cancel_write_never_holds_a_replacement() {
        for write in 2..4 {
            for (outcome, label) in cancel_cases() {
                let rig = journal_rig();
                let position = rig.active_position(None).await;
                let old = rig.placed_stop(position.id).await;
                rig.backend.fail_commit(write, Some(JournalFault::Hang));
                rig.exchange.set_stop_cancel_outcome_next(outcome);

                let results =
                    execute_bounded(&rig, vec![replace_action(position.id, &old)]).await.unwrap();

                assert!(
                    matches!(results[0], ActionResult::OrderPlaced {
                        event: Some(Event::InsuranceStopReplaced { .. }),
                        ..
                    }),
                    "{label} write {write}: {results:?}"
                );
                rig.journal.flush_pending().await;
                rig.assert_cancel_row(label, &old);
                rig.assert_placements_succeeded(2, label);
            }
        }
    }

    /// Withdrawal: the same, the placement's writes being 0 and 1.
    #[tokio::test]
    async fn a_hung_cancel_write_never_holds_a_withdrawal() {
        for write in 2..4 {
            for (outcome, label) in cancel_cases() {
                let rig = journal_rig();
                let position = rig.leg_closing_at_the_next_stop().await;
                rig.backend.fail_commit(write, Some(JournalFault::Hang));
                rig.exchange.set_stop_cancel_outcome_next(outcome);

                let results = execute_bounded(&rig, vec![place_action(position.id)]).await.unwrap();

                assert!(
                    matches!(results[0], ActionResult::StateUpdated),
                    "{label} write {write}: {results:?}"
                );
                rig.journal.flush_pending().await;
                assert_eq!(rig.rows_of_kind(IntentKind::Cancel).len(), 1, "{label} write {write}");
            }
        }
    }

    /// Software exit: the same, and the market exit behind the cancel still
    /// reaches the venue.
    #[tokio::test]
    async fn a_hung_cancel_write_never_holds_the_exit() {
        for write in 0..2 {
            for (outcome, label) in cancel_cases() {
                let rig = journal_rig();
                let position = rig.active_position(Some("stop-1")).await;
                rig.backend.fail_commit(write, Some(JournalFault::Hang));
                rig.exchange.set_stop_cancel_outcome_next(outcome);

                let results = execute_bounded(&rig, vec![
                    cancel_action(position.id, "stop-1"),
                    exit_action(position.id, dec!(0.1)),
                ])
                .await
                .unwrap();

                assert_eq!(rig.sent_market_orders(), 1, "{label} write {write}");
                assert!(
                    results.iter().any(|r| matches!(r, ActionResult::OrderPlaced { .. })),
                    "{label} write {write}: {results:?}"
                );
                rig.journal.flush_pending().await;
                rig.assert_cancel_row(label, "stop-1");
            }
        }
    }

    /// The cancel is not rate-limited: while the id stays on the leg, each
    /// attempt goes out again as its own intent, and an error audits each time.
    #[tokio::test]
    async fn a_software_exit_cancel_repeats_on_every_attempt() {
        let first = cancel_cases().into_iter().filter(|(_, label)| *label != "cancelled");
        let second = cancel_cases().into_iter().filter(|(_, label)| *label != "cancelled");
        for ((outcome, label), (again, _)) in first.zip(second) {
            let rig = journal_rig();
            let position = rig.active_position(Some("stop-1")).await;
            let audits = |events: Vec<Event>| {
                events.iter().filter(|e| matches!(e, Event::InsuranceStopFailed { .. })).count()
            };

            rig.exchange.set_stop_cancel_outcome_next(outcome);
            rig.executor.execute(vec![cancel_action(position.id, "stop-1")]).await.unwrap();
            let audited = audits(rig.events(position.id).await);
            rig.exchange.set_stop_cancel_outcome_next(again);
            rig.executor.execute(vec![cancel_action(position.id, "stop-1")]).await.unwrap();

            assert_eq!(rig.exchange.stop_cancel_call_count(), 2, "{label}");
            assert_eq!(rig.rows_of_kind(IntentKind::Cancel).len(), 2, "{label}");
            assert_eq!(audits(rig.events(position.id).await), audited * 2, "{label}");
        }
    }

    /// Regression: 2026-07-31 production incident.
    ///
    /// A conditional stop the exchange no longer knows about has almost always
    /// TRIGGERED. Recording that as `InsuranceStopCancelled` both misreports
    /// the outcome and clears `insurance_stop_id`, destroying the pointer the
    /// reverse-reconciliation order-fill path needs to resolve the real fill.
    #[tokio::test]
    async fn test_cancel_insurance_stop_already_gone_does_not_record_a_cancellation() {
        let (executor, exchange) = insurance_executor();
        let position_id = Uuid::now_v7();

        // No stop was ever placed, so the exchange reports AlreadyGone — the
        // same answer it gives for a stop that already fired.
        let cancel_action = EngineAction::CancelInsuranceStop {
            position_id,
            symbol: Symbol::from_pair("BTCUSDT").unwrap(),
            order_id: "2000001325469919".to_string(),
        };
        let results = executor.execute(vec![cancel_action]).await.unwrap();

        assert_eq!(results.len(), 1);
        match &results[0] {
            ActionResult::Skipped(reason) => {
                assert!(
                    reason.contains("2000001325469919"),
                    "the skip reason must name the stop id, got {reason}"
                );
            },
            other => panic!("Expected Skipped for an already-gone stop, got {:?}", other),
        }
        assert_eq!(exchange.stop_order_count(), 0);
    }
}
