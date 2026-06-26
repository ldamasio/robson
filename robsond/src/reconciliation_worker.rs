//! Exchange reconciliation worker.
//!
//! Periodically compares exchange-open positions with Robson's in-memory store
//! and force-closes any position that is not tracked by the runtime.

use std::{collections::HashMap, sync::Arc, time::Duration};

use chrono::{DateTime, Datelike, Utc};
use robson_domain::{
    Event, OrderFillEvidence, Position, PositionId, PositionState, Quantity,
    ReconciliationEvidence, Side, Symbol, UserTradeEvidence,
};
use robson_exec::{ExchangePort, ExchangePosition, OrderResult, UserTradeRecord};
use robson_store::Store;
use rust_decimal::Decimal;
use tokio::{
    sync::{Mutex, RwLock},
    time::{Instant, MissedTickBehavior},
};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::{
    error::{DaemonError, DaemonResult},
    event_bus::{DaemonEvent, EventBus},
    position_manager::{PositionManager, ReconcileCloseOutcome, ReconciledCloseInput},
};

/// Background worker that reconciles exchange state with Robson state.
pub struct ReconciliationWorker<E: ExchangePort + 'static, S: Store + 'static> {
    exchange: Arc<E>,
    position_manager: Arc<RwLock<PositionManager<E, S>>>,
    store: Arc<S>,
    event_bus: Arc<EventBus>,
    scan_interval: Duration,
    missing_grace: Duration,
    missing_observations: Arc<Mutex<HashMap<PositionId, MissingObservation>>>,
    shutdown_token: CancellationToken,
}

#[derive(Debug, Clone)]
struct MissingObservation {
    symbol: Symbol,
    side: Side,
    expected_quantity: Quantity,
    first_observed_missing_at: DateTime<Utc>,
    first_observed_instant: Instant,
}

// ---------------------------------------------------------------------------
// Free functions — evidence gathering helpers extracted from
// ReconciliationWorker for reuse by the planned startup auto_reconcile path
// (Slice 5B2B).
// ---------------------------------------------------------------------------

pub(crate) async fn gather_real_evidence<E, S>(
    exchange: &Arc<E>,
    store: &Arc<S>,
    position: &Position,
    expected_quantity: Quantity,
    observed_at_floor: DateTime<Utc>,
) -> DaemonResult<Option<ReconciledCloseInput>>
where
    E: ExchangePort + 'static,
    S: Store + 'static,
{
    if let Some(input) = gather_order_fill_evidence(exchange, store, position).await? {
        return Ok(Some(input));
    }

    gather_user_trade_evidence::<E>(exchange, position, expected_quantity, observed_at_floor).await
}

pub(crate) async fn gather_order_fill_evidence<E, S>(
    exchange: &Arc<E>,
    store: &Arc<S>,
    position: &Position,
) -> DaemonResult<Option<ReconciledCloseInput>>
where
    E: ExchangePort + 'static,
    S: Store + 'static,
{
    let candidate_order_id = match &position.state {
        PositionState::Active { insurance_stop_id, .. } => {
            insurance_stop_id.or(position.insurance_stop_id)
        },
        _ => position.insurance_stop_id,
    };
    let Some(order_id) = candidate_order_id else {
        return Ok(None);
    };

    let Some(order) = store.orders().find_by_id(order_id).await? else {
        warn!(
            position_id = %position.id,
            %order_id,
            "Reverse reconciliation could not resolve local insurance stop order"
        );
        return Ok(None);
    };
    let Some(exchange_order_id) = order.exchange_order_id else {
        warn!(
            position_id = %position.id,
            %order_id,
            "Reverse reconciliation local insurance stop has no exchange order id"
        );
        return Ok(None);
    };

    let Some(result) =
        exchange.get_order_by_exchange_id(&position.symbol, &exchange_order_id).await?
    else {
        return Ok(None);
    };

    Ok(Some(input_from_order_result(position.id, result)))
}

pub(crate) async fn gather_user_trade_evidence<E>(
    exchange: &Arc<E>,
    position: &Position,
    expected_quantity: Quantity,
    observed_at_floor: DateTime<Utc>,
) -> DaemonResult<Option<ReconciledCloseInput>>
where
    E: ExchangePort + 'static,
{
    const USER_TRADES_LIMIT: u16 = 100;

    let trades = exchange
        .get_user_trades_since(&position.symbol, observed_at_floor, USER_TRADES_LIMIT)
        .await?;
    let mut compatible = trades.into_iter().filter(|trade| {
        trade.filled_at >= observed_at_floor && trade.filled_quantity == expected_quantity
    });

    let Some(first) = compatible.next() else {
        return Ok(None);
    };
    if compatible.next().is_some() {
        warn!(
            position_id = %position.id,
            symbol = %position.symbol.as_pair(),
            side = ?position.side,
            expected_quantity = %expected_quantity,
            "Reverse reconciliation found multiple compatible user trades, leaving unresolved"
        );
        return Ok(None);
    }

    Ok(Some(input_from_user_trade(position.id, first)))
}

pub(crate) fn input_from_order_result(
    position_id: PositionId,
    result: OrderResult,
) -> ReconciledCloseInput {
    ReconciledCloseInput {
        position_id,
        exit_price: result.fill_price,
        filled_quantity: result.filled_quantity,
        fee: result.fee,
        fee_asset: result.fee_asset.clone(),
        closed_at: result.filled_at,
        evidence: ReconciliationEvidence::OrderFillRecord(OrderFillEvidence {
            exchange_order_id: result.exchange_order_id,
            fill_price: result.fill_price,
            filled_quantity: result.filled_quantity,
            fee: result.fee,
            fee_asset: result.fee_asset,
            filled_at: result.filled_at,
        }),
    }
}

pub(crate) fn input_from_user_trade(
    position_id: PositionId,
    trade: UserTradeRecord,
) -> ReconciledCloseInput {
    ReconciledCloseInput {
        position_id,
        exit_price: trade.fill_price,
        filled_quantity: trade.filled_quantity,
        fee: trade.fee,
        fee_asset: trade.fee_asset.clone(),
        closed_at: trade.filled_at,
        evidence: ReconciliationEvidence::UserTradeRecord(UserTradeEvidence {
            exchange_order_id: trade.exchange_order_id,
            exchange_trade_id: trade.exchange_trade_id,
            fill_price: trade.fill_price,
            filled_quantity: trade.filled_quantity,
            fee: trade.fee,
            fee_asset: trade.fee_asset,
            filled_at: trade.filled_at,
        }),
    }
}

impl<E: ExchangePort + 'static, S: Store + 'static> ReconciliationWorker<E, S> {
    /// Create a new reconciliation worker.
    pub fn new(
        exchange: Arc<E>,
        position_manager: Arc<RwLock<PositionManager<E, S>>>,
        store: Arc<S>,
        event_bus: Arc<EventBus>,
        scan_interval: Duration,
        shutdown_token: CancellationToken,
    ) -> Self {
        Self::new_with_missing_grace(
            exchange,
            position_manager,
            store,
            event_bus,
            scan_interval,
            scan_interval,
            shutdown_token,
        )
    }

    pub(crate) fn new_with_missing_grace(
        exchange: Arc<E>,
        position_manager: Arc<RwLock<PositionManager<E, S>>>,
        store: Arc<S>,
        event_bus: Arc<EventBus>,
        scan_interval: Duration,
        missing_grace: Duration,
        shutdown_token: CancellationToken,
    ) -> Self {
        Self {
            exchange,
            position_manager,
            store,
            event_bus,
            scan_interval,
            missing_grace,
            missing_observations: Arc::new(Mutex::new(HashMap::new())),
            shutdown_token,
        }
    }

    /// Run the periodic reconciliation loop until shutdown.
    pub async fn run(self) -> DaemonResult<()> {
        let mut interval = tokio::time::interval(self.scan_interval);
        interval.set_missed_tick_behavior(MissedTickBehavior::Skip);

        // Consume the immediate tick; startup reconciliation is handled
        // explicitly by the daemon before this loop starts.
        interval.tick().await;

        loop {
            tokio::select! {
                _ = self.shutdown_token.cancelled() => {
                    info!("Reconciliation worker shutting down");
                    break Ok(());
                }
                _ = interval.tick() => {
                    if let Err(error) = self.scan_and_reconcile().await {
                        error!(error = %error, "Reconciliation scan failed");
                    }
                }
            }
        }
    }

    /// Run one reconciliation pass, returning how many untracked positions were
    /// closed.
    pub async fn scan_and_reconcile(&self) -> DaemonResult<usize> {
        let exchange_positions = self.exchange.get_all_open_positions().await?;
        let mut reconciled = 0usize;

        for exchange_position in &exchange_positions {
            if self.is_tracked(&exchange_position).await? {
                continue;
            }

            self.handle_untracked_position(exchange_position.clone()).await?;
            reconciled += 1;
        }

        reconciled += self.reconcile_local_missing_positions(&exchange_positions).await?;

        if reconciled > 0 {
            self.recalibrate_capital_base_after_manual_drift(reconciled).await?;
        } else {
            self.recalibrate_capital_base_after_pure_financial_drift().await?;
        }

        Ok(reconciled)
    }

    /// Alias used by daemon startup gating for a one-shot blocking pass.
    pub async fn scan_and_reconcile_blocking(&self) -> DaemonResult<usize> {
        self.scan_and_reconcile().await
    }

    async fn is_tracked(&self, exchange_position: &ExchangePosition) -> DaemonResult<bool> {
        Ok(self
            .store
            .positions()
            .find_active_by_symbol_and_side(&exchange_position.symbol, exchange_position.side)
            .await?
            .is_some())
    }

    async fn reconcile_local_missing_positions(
        &self,
        exchange_positions: &[ExchangePosition],
    ) -> DaemonResult<usize> {
        let open_positions = self.store.positions().find_active().await?;
        let mut reconciled = 0usize;

        for position in open_positions {
            let present_on_exchange = exchange_positions.iter().any(|exchange_position| {
                exchange_position.symbol == position.symbol
                    && exchange_position.side == position.side
            });

            if present_on_exchange {
                self.clear_missing_observation(position.id).await;
                continue;
            }

            match &position.state {
                PositionState::Active { .. } => {
                    if self.handle_missing_active_position(&position).await? {
                        reconciled += 1;
                    }
                },
                PositionState::Entering { .. } | PositionState::Exiting { .. } => {
                    self.handle_missing_non_active_position(&position).await;
                },
                PositionState::Armed
                | PositionState::Closed { .. }
                | PositionState::Error { .. }
                | PositionState::Cancelled => {},
            }
        }

        Ok(reconciled)
    }

    async fn clear_missing_observation(&self, position_id: PositionId) {
        let mut observations = self.missing_observations.lock().await;
        observations.remove(&position_id);
    }

    async fn handle_missing_active_position(&self, position: &Position) -> DaemonResult<bool> {
        let now = Utc::now();
        let observation = {
            let mut observations = self.missing_observations.lock().await;
            match observations.get(&position.id).cloned() {
                Some(observation) => observation,
                None => {
                    observations.insert(
                        position.id,
                        MissingObservation {
                            symbol: position.symbol.clone(),
                            side: position.side,
                            expected_quantity: position.quantity,
                            first_observed_missing_at: now,
                            first_observed_instant: Instant::now(),
                        },
                    );
                    debug!(
                        position_id = %position.id,
                        symbol = %position.symbol.as_pair(),
                        side = ?position.side,
                        quantity = %position.quantity,
                        "Reverse reconciliation first observed Active position missing on exchange"
                    );
                    return Ok(false);
                },
            }
        };

        if observation.first_observed_instant.elapsed() < self.missing_grace {
            debug!(
                position_id = %position.id,
                symbol = %position.symbol.as_pair(),
                side = ?position.side,
                "Reverse reconciliation waiting for missing-position grace period"
            );
            return Ok(false);
        }

        let confirmed_missing_at = Utc::now();
        match gather_real_evidence(
            &self.exchange,
            &self.store,
            position,
            observation.expected_quantity,
            observation.first_observed_missing_at,
        )
        .await?
        {
            Some(input) => {
                let outcome = {
                    let manager = self.position_manager.read().await;
                    manager.reconcile_close(input).await?
                };

                match outcome {
                    ReconcileCloseOutcome::Closed | ReconcileCloseOutcome::AlreadyTerminal => {
                        self.clear_missing_observation(position.id).await;
                        Ok(matches!(outcome, ReconcileCloseOutcome::Closed))
                    },
                    ReconcileCloseOutcome::SkippedNonActive { state } => {
                        warn!(
                            position_id = %position.id,
                            state,
                            "Reverse reconciliation close skipped by PositionManager"
                        );
                        self.clear_missing_observation(position.id).await;
                        Ok(false)
                    },
                    ReconcileCloseOutcome::RejectedUnsupportedEvidence { source } => {
                        self.emit_unresolved(
                            position,
                            &observation,
                            confirmed_missing_at,
                            format!("unsupported_evidence:{source}"),
                        )
                        .await;
                        Ok(false)
                    },
                    ReconcileCloseOutcome::RejectedInconsistentEvidence { field } => {
                        self.emit_unresolved(
                            position,
                            &observation,
                            confirmed_missing_at,
                            format!("inconsistent_evidence:{field}"),
                        )
                        .await;
                        Ok(false)
                    },
                }
            },
            None => {
                self.emit_unresolved(
                    position,
                    &observation,
                    confirmed_missing_at,
                    "no_unambiguous_real_fill_evidence".to_string(),
                )
                .await;
                Ok(false)
            },
        }
    }

    async fn handle_missing_non_active_position(&self, position: &Position) {
        let observed_at = Utc::now();
        warn!(
            position_id = %position.id,
            state = %position.state.name(),
            symbol = %position.symbol.as_pair(),
            side = ?position.side,
            %observed_at,
            "Reverse reconciliation detected missing non-Active position, skipped"
        );
        self.event_bus.send(DaemonEvent::ReconciliationStaleNonActiveDetected {
            position_id: position.id,
            state: position.state.name().to_string(),
            symbol: position.symbol.clone(),
            side: position.side,
            observed_at,
        });
    }

    async fn emit_unresolved(
        &self,
        position: &Position,
        observation: &MissingObservation,
        confirmed_missing_at: DateTime<Utc>,
        reason: String,
    ) {
        error!(
            position_id = %position.id,
            symbol = %observation.symbol.as_pair(),
            side = ?observation.side,
            expected_quantity = %observation.expected_quantity,
            %reason,
            first_observed_missing_at = %observation.first_observed_missing_at,
            %confirmed_missing_at,
            "Reverse reconciliation stale Active unresolved"
        );
        self.event_bus.send(DaemonEvent::ReconciliationStaleActiveUnresolved {
            position_id: position.id,
            symbol: observation.symbol.clone(),
            side: observation.side,
            first_observed_missing_at: observation.first_observed_missing_at,
            confirmed_missing_at,
            reason,
        });
        self.clear_missing_observation(position.id).await;
    }

    async fn handle_untracked_position(
        &self,
        exchange_position: ExchangePosition,
    ) -> DaemonResult<()> {
        warn!(
            symbol = %exchange_position.symbol.as_pair(),
            side = ?exchange_position.side,
            quantity = %exchange_position.quantity,
            entry_price = %exchange_position.entry_price,
            "UNTRACKED position detected, closing"
        );

        self.event_bus.send(DaemonEvent::RoguePositionDetected {
            symbol: exchange_position.symbol.as_pair(),
            side: exchange_position.side,
            entry_price: exchange_position.entry_price,
            // No technical stop exists for untracked exchange positions.
            stop_price: exchange_position.entry_price,
        });

        let client_order_id = Uuid::now_v7().to_string();
        match self
            .exchange
            .close_position_market(
                &exchange_position.symbol,
                exchange_position.side,
                exchange_position.quantity,
                &client_order_id,
            )
            .await
        {
            Ok(order) => {
                info!(
                    symbol = %exchange_position.symbol.as_pair(),
                    side = ?exchange_position.side,
                    order_id = %order.exchange_order_id,
                    executed_quantity = %order.filled_quantity,
                    "UNTRACKED position closed"
                );

                self.event_bus.send(DaemonEvent::SafetyExitExecuted {
                    symbol: exchange_position.symbol.as_pair(),
                    order_id: order.exchange_order_id,
                    executed_quantity: order.filled_quantity.as_decimal(),
                });

                Ok(())
            },
            Err(error) => {
                error!(
                    symbol = %exchange_position.symbol.as_pair(),
                    side = ?exchange_position.side,
                    error = %error,
                    "Failed to close UNTRACKED position"
                );

                self.event_bus.send(DaemonEvent::SafetyExitFailed {
                    symbol: exchange_position.symbol.as_pair(),
                    error: error.to_string(),
                });

                Err(DaemonError::Exec(error))
            },
        }
    }

    async fn recalibrate_capital_base_after_manual_drift(
        &self,
        reconciled_positions: usize,
    ) -> DaemonResult<()> {
        self.recalibrate_capital_base(
            format!("reconciliation_worker:positions_reconciled={reconciled_positions}"),
            Some(reconciled_positions),
        )
        .await
    }

    async fn recalibrate_capital_base_after_pure_financial_drift(&self) -> DaemonResult<()> {
        let now = Utc::now();
        let risk_open = self.live_risk_open_positions().await?;
        let all_open = self.store.positions().find_active().await?;
        let armed_count = all_open
            .iter()
            .filter(|position| matches!(position.state, PositionState::Armed))
            .count();

        if !risk_open.is_empty() || armed_count > 0 {
            debug!(
                risk_open_count = risk_open.len(),
                armed_count,
                "Pure financial drift scan skipped while Robson positions are open or armed"
            );
            return Ok(());
        }

        let wallet_balance = self.exchange.get_futures_balance().await?.wallet_balance;
        let previous_capital_base = {
            let manager = self.position_manager.read().await;
            manager.load_monthly_state(now).await?.capital_base
        };
        let closed = self.store.positions().find_closed_in_month(now.year(), now.month()).await?;
        let robson_month_net: Decimal =
            closed.iter().map(|position| position.realized_pnl - position.fees_paid).sum();
        let expected_wallet_balance = previous_capital_base + robson_month_net;
        let unexplained_delta = wallet_balance - expected_wallet_balance;

        if decimal_abs(unexplained_delta) <= financial_drift_tolerance() {
            return Ok(());
        }

        warn!(
            %wallet_balance,
            %expected_wallet_balance,
            %previous_capital_base,
            %robson_month_net,
            %unexplained_delta,
            "Pure financial account drift detected; recalibrating capital_base"
        );

        self.recalibrate_capital_base(
            format!(
                "financial_drift:wallet_balance={wallet_balance};expected_wallet_balance={expected_wallet_balance};unexplained_delta={unexplained_delta}"
            ),
            None,
        )
        .await
    }

    async fn live_risk_open_positions(&self) -> DaemonResult<Vec<Position>> {
        let manager = self.position_manager.read().await;
        manager.live_risk_open_positions().await
    }

    async fn recalibrate_capital_base(
        &self,
        evidence: String,
        reconciled_positions: Option<usize>,
    ) -> DaemonResult<()> {
        let now = Utc::now();
        let wallet_balance = self.exchange.get_futures_balance().await?.wallet_balance;
        let previous_capital_base = {
            let manager = self.position_manager.read().await;
            manager.load_monthly_state(now).await?.capital_base
        };

        let carried_risk_committed =
            Self::calculate_committed_carried_risk(&self.live_risk_open_positions().await?);
        let armed_count = self
            .store
            .positions()
            .find_active()
            .await?
            .iter()
            .filter(|position| matches!(position.state, PositionState::Armed))
            .count() as u32;
        let armed_risk = previous_capital_base * Decimal::new(1, 2) * Decimal::from(armed_count);
        let carried_risk = carried_risk_committed + armed_risk;
        let new_capital_base = (wallet_balance - carried_risk).max(Decimal::ZERO);

        if new_capital_base == previous_capital_base {
            info!(
                %previous_capital_base,
                %wallet_balance,
                %carried_risk,
                ?reconciled_positions,
                "Capital base recalibration skipped: value unchanged after reconciliation"
            );
            return Ok(());
        }

        info!(
            %previous_capital_base,
            %new_capital_base,
            %wallet_balance,
            %carried_risk,
            ?reconciled_positions,
            "Capital base recalibrated after manual account drift"
        );

        let event = Event::CapitalBaseRecalibrated {
            previous_capital_base,
            new_capital_base,
            wallet_balance,
            carried_risk,
            reason: "manual_account_change".to_string(),
            evidence,
            month: now.month(),
            year: now.year(),
            timestamp: now,
        };

        let manager = self.position_manager.read().await;
        manager.emit_domain_event(event).await?;

        Ok(())
    }

    fn calculate_committed_carried_risk(positions: &[Position]) -> Decimal {
        positions
            .iter()
            .filter_map(|position| {
                let qty = position.quantity.as_decimal();
                if qty == Decimal::ZERO {
                    return None;
                }

                let (entry, stop) = match &position.state {
                    PositionState::Active { trailing_stop, .. } => {
                        (position.entry_price?.as_decimal(), trailing_stop.as_decimal())
                    },
                    PositionState::Entering { expected_entry, .. } => {
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
                    Side::Long => (entry - stop) * qty,
                    Side::Short => (stop - entry) * qty,
                };

                Some(risk.max(Decimal::ZERO))
            })
            .sum()
    }
}

fn financial_drift_tolerance() -> Decimal {
    Decimal::new(1, 2)
}

fn decimal_abs(value: Decimal) -> Decimal {
    if value < Decimal::ZERO {
        -value
    } else {
        value
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use chrono::Utc;
    use robson_domain::{
        ExitReason, Order, OrderSide, Position, PositionState, Price, Quantity, RiskConfig, Side,
        Symbol, TradingPolicy,
    };
    use robson_engine::Engine;
    use robson_exec::{Executor, IntentJournal, OrderResult, StubExchange, UserTradeRecord};
    use robson_store::{MemoryStore, Store};
    use rust_decimal::Decimal;
    use rust_decimal_macros::dec;
    use tokio::sync::RwLock;
    use tokio_util::sync::CancellationToken;

    use super::*;
    use crate::query_engine::TracingQueryRecorder;

    fn create_position_manager(
        exchange: Arc<StubExchange>,
        store: Arc<MemoryStore>,
        event_bus: Arc<EventBus>,
    ) -> Arc<RwLock<PositionManager<StubExchange, MemoryStore>>> {
        let journal = Arc::new(IntentJournal::new());
        let executor = Arc::new(Executor::new(exchange, journal, store.clone()));
        let risk_config = RiskConfig::new(dec!(10000)).unwrap();
        let engine = Engine::new(risk_config);

        Arc::new(RwLock::new(PositionManager::new(
            engine,
            executor,
            store,
            event_bus,
            Arc::new(TracingQueryRecorder),
            TradingPolicy::default(),
        )))
    }

    fn create_worker(
        exchange: Arc<StubExchange>,
        store: Arc<MemoryStore>,
        event_bus: Arc<EventBus>,
        missing_grace: Duration,
    ) -> ReconciliationWorker<StubExchange, MemoryStore> {
        let position_manager =
            create_position_manager(exchange.clone(), store.clone(), event_bus.clone());
        ReconciliationWorker::new_with_missing_grace(
            exchange,
            position_manager,
            store,
            event_bus,
            Duration::from_secs(60),
            missing_grace,
            CancellationToken::new(),
        )
    }

    fn tracked_active_position(symbol: Symbol, side: Side) -> Position {
        let mut position = Position::new(Uuid::now_v7(), symbol, side);
        position.entry_price = Some(Price::new(dec!(100)).unwrap());
        position.quantity = Quantity::new(dec!(0.010)).unwrap();
        position.state = PositionState::Active {
            current_price: Price::new(dec!(101)).unwrap(),
            trailing_stop: Price::new(dec!(99)).unwrap(),
            favorable_extreme: Price::new(dec!(101)).unwrap(),
            extreme_at: Utc::now(),
            insurance_stop_id: None,
            last_emitted_stop: None,
        };
        position
    }

    fn order_result(
        exchange_order_id: &str,
        price: Decimal,
        quantity: Decimal,
        filled_at: chrono::DateTime<Utc>,
    ) -> OrderResult {
        OrderResult {
            exchange_order_id: exchange_order_id.to_string(),
            client_order_id: format!("client-{exchange_order_id}"),
            fill_price: Price::new(price).unwrap(),
            filled_quantity: Quantity::new(quantity).unwrap(),
            fee: dec!(0.01),
            fee_asset: "USDT".to_string(),
            filled_at,
        }
    }

    fn user_trade(
        exchange_trade_id: &str,
        exchange_order_id: &str,
        price: Decimal,
        quantity: Decimal,
        filled_at: chrono::DateTime<Utc>,
    ) -> UserTradeRecord {
        UserTradeRecord {
            exchange_order_id: exchange_order_id.to_string(),
            exchange_trade_id: exchange_trade_id.to_string(),
            fill_price: Price::new(price).unwrap(),
            filled_quantity: Quantity::new(quantity).unwrap(),
            fee: dec!(0.01),
            fee_asset: "USDT".to_string(),
            filled_at,
        }
    }

    async fn attach_insurance_order(
        store: &Arc<MemoryStore>,
        position: &mut Position,
        exchange_order_id: &str,
    ) {
        let order = {
            let mut order = Order::new_stop_loss_limit(
                position.id,
                position.symbol.clone(),
                OrderSide::Sell,
                position.quantity,
                Price::new(dec!(99)).unwrap(),
                Price::new(dec!(99)).unwrap(),
            );
            order.exchange_order_id = Some(exchange_order_id.to_string());
            order
        };

        if let PositionState::Active { insurance_stop_id, .. } = &mut position.state {
            *insurance_stop_id = Some(order.id);
        }
        position.insurance_stop_id = Some(order.id);
        store.orders().save(&order).await.unwrap();
        store.positions().save(position).await.unwrap();
    }

    async fn close_events_for(store: &Arc<MemoryStore>, position_id: PositionId) -> usize {
        store
            .events()
            .find_by_position(position_id)
            .await
            .unwrap()
            .iter()
            .filter(|event| matches!(event, robson_domain::Event::PositionClosed { .. }))
            .count()
    }

    async fn capital_base_recalibration_events(
        store: &Arc<MemoryStore>,
    ) -> Vec<robson_domain::Event> {
        store
            .events()
            .get_all_events()
            .await
            .unwrap()
            .into_iter()
            .filter(|event| matches!(event, robson_domain::Event::CapitalBaseRecalibrated { .. }))
            .collect()
    }

    async fn save_closed_position_with_pnl_and_fees(
        store: &Arc<MemoryStore>,
        realized_pnl: Decimal,
        fees_paid: Decimal,
    ) {
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let now = Utc::now();
        let exit_price = Price::new(dec!(100) + realized_pnl).unwrap();
        let mut position = Position::new(Uuid::now_v7(), symbol, Side::Long);
        position.entry_price = Some(Price::new(dec!(100)).unwrap());
        position.quantity = Quantity::new(dec!(1)).unwrap();
        position.realized_pnl = realized_pnl;
        position.fees_paid = fees_paid;
        position.closed_at = Some(now);
        position.updated_at = now;
        position.state = PositionState::Closed {
            exit_price,
            realized_pnl,
            exit_reason: ExitReason::TrailingStop,
        };
        store.positions().save(&position).await.unwrap();
    }

    #[tokio::test]
    async fn test_pure_financial_drift_recalibrates_capital_base_without_open_risk() {
        let exchange = Arc::new(StubExchange::new(dec!(100)));
        let store = Arc::new(MemoryStore::new());
        let event_bus = Arc::new(EventBus::new(16));
        exchange.set_futures_balance(dec!(7500));
        let worker = create_worker(exchange, store.clone(), event_bus, Duration::from_secs(0));

        assert_eq!(worker.scan_and_reconcile().await.unwrap(), 0);

        let events = capital_base_recalibration_events(&store).await;
        assert_eq!(events.len(), 1);
        match &events[0] {
            robson_domain::Event::CapitalBaseRecalibrated {
                previous_capital_base,
                new_capital_base,
                wallet_balance,
                carried_risk,
                evidence,
                ..
            } => {
                assert_eq!(*previous_capital_base, dec!(10000));
                assert_eq!(*new_capital_base, dec!(7500));
                assert_eq!(*wallet_balance, dec!(7500));
                assert_eq!(*carried_risk, Decimal::ZERO);
                assert!(evidence.starts_with("financial_drift:"));
            },
            event => panic!("unexpected event: {event:?}"),
        }
    }

    #[tokio::test]
    async fn test_pure_financial_drift_ignores_robson_closed_pnl() {
        let exchange = Arc::new(StubExchange::new(dec!(100)));
        let store = Arc::new(MemoryStore::new());
        let event_bus = Arc::new(EventBus::new(16));
        save_closed_position_with_pnl_and_fees(&store, dec!(200), dec!(0.50)).await;
        exchange.set_futures_balance(dec!(10199.50));
        let worker = create_worker(exchange, store.clone(), event_bus, Duration::from_secs(0));

        assert_eq!(worker.scan_and_reconcile().await.unwrap(), 0);

        assert!(capital_base_recalibration_events(&store).await.is_empty());
    }

    #[tokio::test]
    async fn test_reconciliation_detects_and_closes_untracked_position() {
        let exchange = Arc::new(StubExchange::new(dec!(100)));
        let store = Arc::new(MemoryStore::new());
        let event_bus = Arc::new(EventBus::new(16));
        let mut receiver = event_bus.subscribe();

        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        exchange.set_open_position(
            symbol.clone(),
            Side::Long,
            Quantity::new(dec!(0.010)).unwrap(),
            Price::new(dec!(100)).unwrap(),
        );

        let worker = create_worker(exchange.clone(), store, event_bus, Duration::from_secs(60));

        let reconciled = worker.scan_and_reconcile().await.unwrap();
        assert_eq!(reconciled, 1);
        assert_eq!(exchange.open_positions_len(), 0);

        let first = receiver.recv().await.unwrap().unwrap();
        assert!(matches!(first, DaemonEvent::RoguePositionDetected { .. }));

        let second = receiver.recv().await.unwrap().unwrap();
        assert!(matches!(second, DaemonEvent::SafetyExitExecuted { .. }));
    }

    #[tokio::test]
    async fn test_reconciliation_skips_tracked_position() {
        let exchange = Arc::new(StubExchange::new(dec!(100)));
        let store = Arc::new(MemoryStore::new());
        let event_bus = Arc::new(EventBus::new(16));
        let mut receiver = event_bus.subscribe();

        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        exchange.set_open_position(
            symbol.clone(),
            Side::Long,
            Quantity::new(dec!(0.010)).unwrap(),
            Price::new(dec!(100)).unwrap(),
        );
        store
            .positions()
            .save(&tracked_active_position(symbol, Side::Long))
            .await
            .unwrap();

        let worker = create_worker(exchange.clone(), store, event_bus, Duration::from_secs(60));

        let reconciled = worker.scan_and_reconcile().await.unwrap();
        assert_eq!(reconciled, 0);
        assert_eq!(exchange.open_positions_len(), 1);
        assert!(receiver.try_recv().is_none());
    }

    #[tokio::test]
    async fn test_missing_active_first_observation_does_not_close() {
        let exchange = Arc::new(StubExchange::new(dec!(100)));
        let store = Arc::new(MemoryStore::new());
        let event_bus = Arc::new(EventBus::new(16));
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let position = tracked_active_position(symbol, Side::Long);
        let position_id = position.id;
        store.positions().save(&position).await.unwrap();

        let worker = create_worker(exchange, store.clone(), event_bus, Duration::from_secs(0));

        let reconciled = worker.scan_and_reconcile().await.unwrap();

        assert_eq!(reconciled, 0);
        let loaded = store.positions().find_by_id(position_id).await.unwrap().unwrap();
        assert!(matches!(loaded.state, PositionState::Active { .. }));
        assert_eq!(close_events_for(&store, position_id).await, 0);
    }

    #[tokio::test]
    async fn test_missing_active_second_observation_order_fill_closes() {
        let exchange = Arc::new(StubExchange::new(dec!(100)));
        let store = Arc::new(MemoryStore::new());
        let event_bus = Arc::new(EventBus::new(16));
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let mut position = tracked_active_position(symbol, Side::Long);
        let position_id = position.id;
        attach_insurance_order(&store, &mut position, "EX-ORDER-1").await;
        exchange.set_order_result(
            "EX-ORDER-1",
            order_result("EX-ORDER-1", dec!(90), dec!(0.010), Utc::now()),
        );
        let worker = create_worker(exchange, store.clone(), event_bus, Duration::from_secs(0));

        assert_eq!(worker.scan_and_reconcile().await.unwrap(), 0);
        assert_eq!(worker.scan_and_reconcile().await.unwrap(), 1);

        let closed = store.positions().find_by_id(position_id).await.unwrap().unwrap();
        assert!(matches!(
            closed.state,
            PositionState::Closed {
                exit_reason: ExitReason::ReconciledMissingOnExchange,
                ..
            }
        ));
        assert_eq!(close_events_for(&store, position_id).await, 1);
    }

    #[tokio::test]
    async fn test_order_fill_has_priority_over_user_trade() {
        let exchange = Arc::new(StubExchange::new(dec!(100)));
        let store = Arc::new(MemoryStore::new());
        let event_bus = Arc::new(EventBus::new(16));
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let mut position = tracked_active_position(symbol.clone(), Side::Long);
        let position_id = position.id;
        attach_insurance_order(&store, &mut position, "EX-ORDER-1").await;
        let now = Utc::now();
        exchange
            .set_order_result("EX-ORDER-1", order_result("EX-ORDER-1", dec!(90), dec!(0.010), now));
        exchange.set_user_trades(
            &symbol.as_pair(),
            vec![user_trade(
                "TRADE-1",
                "EX-ORDER-2",
                dec!(91),
                dec!(0.010),
                now,
            )],
        );
        let worker = create_worker(exchange, store.clone(), event_bus, Duration::from_secs(0));

        worker.scan_and_reconcile().await.unwrap();
        worker.scan_and_reconcile().await.unwrap();

        let events = store.events().find_by_position(position_id).await.unwrap();
        let evidence = events
            .iter()
            .find_map(|event| {
                if let robson_domain::Event::PositionClosed { closure_evidence, .. } = event {
                    Some(closure_evidence)
                } else {
                    None
                }
            })
            .expect("PositionClosed event must be emitted");
        assert!(matches!(
            evidence,
            robson_domain::ClosureEvidence::Reconciled(ReconciliationEvidence::OrderFillRecord(_))
        ));
    }

    #[tokio::test]
    async fn test_missing_active_single_matching_user_trade_closes() {
        let exchange = Arc::new(StubExchange::new(dec!(100)));
        let store = Arc::new(MemoryStore::new());
        let event_bus = Arc::new(EventBus::new(16));
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let position = tracked_active_position(symbol.clone(), Side::Long);
        let position_id = position.id;
        store.positions().save(&position).await.unwrap();
        let worker =
            create_worker(exchange.clone(), store.clone(), event_bus, Duration::from_secs(0));

        worker.scan_and_reconcile().await.unwrap();
        exchange.set_user_trades(
            &symbol.as_pair(),
            vec![user_trade(
                "TRADE-1",
                "EX-ORDER-2",
                dec!(90),
                dec!(0.010),
                Utc::now(),
            )],
        );
        assert_eq!(worker.scan_and_reconcile().await.unwrap(), 1);

        let closed = store.positions().find_by_id(position_id).await.unwrap().unwrap();
        assert!(matches!(closed.state, PositionState::Closed { .. }));
    }

    #[tokio::test]
    async fn test_missing_active_zero_user_trades_is_unresolved() {
        let exchange = Arc::new(StubExchange::new(dec!(100)));
        let store = Arc::new(MemoryStore::new());
        let event_bus = Arc::new(EventBus::new(16));
        let mut receiver = event_bus.subscribe();
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let position = tracked_active_position(symbol, Side::Long);
        let position_id = position.id;
        store.positions().save(&position).await.unwrap();
        let worker = create_worker(exchange, store.clone(), event_bus, Duration::from_secs(0));

        worker.scan_and_reconcile().await.unwrap();
        assert_eq!(worker.scan_and_reconcile().await.unwrap(), 0);

        let loaded = store.positions().find_by_id(position_id).await.unwrap().unwrap();
        assert!(matches!(loaded.state, PositionState::Active { .. }));
        assert_eq!(close_events_for(&store, position_id).await, 0);
        let event = receiver.recv().await.unwrap().unwrap();
        assert!(matches!(event, DaemonEvent::ReconciliationStaleActiveUnresolved { .. }));
    }

    #[tokio::test]
    async fn test_missing_active_multiple_matching_user_trades_is_unresolved() {
        let exchange = Arc::new(StubExchange::new(dec!(100)));
        let store = Arc::new(MemoryStore::new());
        let event_bus = Arc::new(EventBus::new(16));
        let mut receiver = event_bus.subscribe();
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let position = tracked_active_position(symbol.clone(), Side::Long);
        let position_id = position.id;
        store.positions().save(&position).await.unwrap();
        let worker =
            create_worker(exchange.clone(), store.clone(), event_bus, Duration::from_secs(0));

        worker.scan_and_reconcile().await.unwrap();
        let now = Utc::now();
        exchange.set_user_trades(
            &symbol.as_pair(),
            vec![
                user_trade("TRADE-1", "EX-ORDER-2", dec!(90), dec!(0.010), now),
                user_trade("TRADE-2", "EX-ORDER-3", dec!(91), dec!(0.010), now),
            ],
        );
        assert_eq!(worker.scan_and_reconcile().await.unwrap(), 0);

        let loaded = store.positions().find_by_id(position_id).await.unwrap().unwrap();
        assert!(matches!(loaded.state, PositionState::Active { .. }));
        assert_eq!(close_events_for(&store, position_id).await, 0);
        let event = receiver.recv().await.unwrap().unwrap();
        assert!(matches!(event, DaemonEvent::ReconciliationStaleActiveUnresolved { .. }));
    }

    #[tokio::test]
    async fn test_missing_active_quantity_mismatch_user_trade_is_unresolved() {
        let exchange = Arc::new(StubExchange::new(dec!(100)));
        let store = Arc::new(MemoryStore::new());
        let event_bus = Arc::new(EventBus::new(16));
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let position = tracked_active_position(symbol.clone(), Side::Long);
        let position_id = position.id;
        store.positions().save(&position).await.unwrap();
        let worker =
            create_worker(exchange.clone(), store.clone(), event_bus, Duration::from_secs(0));

        worker.scan_and_reconcile().await.unwrap();
        exchange.set_user_trades(
            &symbol.as_pair(),
            vec![user_trade(
                "TRADE-1",
                "EX-ORDER-2",
                dec!(90),
                dec!(0.020),
                Utc::now(),
            )],
        );
        assert_eq!(worker.scan_and_reconcile().await.unwrap(), 0);

        let loaded = store.positions().find_by_id(position_id).await.unwrap().unwrap();
        assert!(matches!(loaded.state, PositionState::Active { .. }));
        assert_eq!(close_events_for(&store, position_id).await, 0);
    }

    #[tokio::test]
    async fn test_reconciliation_close_is_idempotent_after_closed() {
        let exchange = Arc::new(StubExchange::new(dec!(100)));
        let store = Arc::new(MemoryStore::new());
        let event_bus = Arc::new(EventBus::new(16));
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let mut position = tracked_active_position(symbol, Side::Long);
        let position_id = position.id;
        attach_insurance_order(&store, &mut position, "EX-ORDER-1").await;
        exchange.set_order_result(
            "EX-ORDER-1",
            order_result("EX-ORDER-1", dec!(90), dec!(0.010), Utc::now()),
        );
        let worker = create_worker(exchange, store.clone(), event_bus, Duration::from_secs(0));

        worker.scan_and_reconcile().await.unwrap();
        assert_eq!(worker.scan_and_reconcile().await.unwrap(), 1);
        assert_eq!(worker.scan_and_reconcile().await.unwrap(), 0);

        assert_eq!(close_events_for(&store, position_id).await, 1);
    }

    #[tokio::test]
    async fn test_position_reappears_clears_missing_grace() {
        let exchange = Arc::new(StubExchange::new(dec!(100)));
        let store = Arc::new(MemoryStore::new());
        let event_bus = Arc::new(EventBus::new(16));
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let position = tracked_active_position(symbol.clone(), Side::Long);
        let position_id = position.id;
        store.positions().save(&position).await.unwrap();
        let worker =
            create_worker(exchange.clone(), store.clone(), event_bus, Duration::from_secs(0));

        worker.scan_and_reconcile().await.unwrap();
        assert!(worker.missing_observations.lock().await.contains_key(&position_id));
        exchange.set_open_position(
            symbol,
            Side::Long,
            Quantity::new(dec!(0.010)).unwrap(),
            Price::new(dec!(100)).unwrap(),
        );
        worker.scan_and_reconcile().await.unwrap();

        assert!(!worker.missing_observations.lock().await.contains_key(&position_id));
        let loaded = store.positions().find_by_id(position_id).await.unwrap().unwrap();
        assert!(matches!(loaded.state, PositionState::Active { .. }));
    }

    #[tokio::test]
    async fn test_cross_side_exchange_long_does_not_satisfy_local_short() {
        let exchange = Arc::new(StubExchange::new(dec!(100)));
        let store = Arc::new(MemoryStore::new());
        let event_bus = Arc::new(EventBus::new(16));
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let mut position = tracked_active_position(symbol.clone(), Side::Short);
        let position_id = position.id;
        attach_insurance_order(&store, &mut position, "EX-ORDER-1").await;
        exchange.set_open_position(
            symbol,
            Side::Long,
            Quantity::new(dec!(0.010)).unwrap(),
            Price::new(dec!(100)).unwrap(),
        );
        exchange.set_order_result(
            "EX-ORDER-1",
            order_result("EX-ORDER-1", dec!(90), dec!(0.010), Utc::now()),
        );
        let worker = create_worker(exchange, store.clone(), event_bus, Duration::from_secs(0));

        worker.scan_and_reconcile().await.unwrap();
        assert_eq!(worker.scan_and_reconcile().await.unwrap(), 1);

        let closed = store.positions().find_by_id(position_id).await.unwrap().unwrap();
        assert!(matches!(closed.state, PositionState::Closed { .. }));
    }

    #[tokio::test]
    async fn test_missing_entering_skipped_without_close() {
        let exchange = Arc::new(StubExchange::new(dec!(100)));
        let store = Arc::new(MemoryStore::new());
        let event_bus = Arc::new(EventBus::new(16));
        let mut receiver = event_bus.subscribe();
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let mut position = tracked_active_position(symbol, Side::Long);
        position.state = PositionState::Entering {
            entry_order_id: Uuid::now_v7(),
            expected_entry: Price::new(dec!(100)).unwrap(),
            signal_id: Uuid::now_v7(),
        };
        let position_id = position.id;
        store.positions().save(&position).await.unwrap();
        let worker = create_worker(exchange, store.clone(), event_bus, Duration::from_secs(0));

        assert_eq!(worker.scan_and_reconcile().await.unwrap(), 0);

        let loaded = store.positions().find_by_id(position_id).await.unwrap().unwrap();
        assert!(matches!(loaded.state, PositionState::Entering { .. }));
        assert_eq!(close_events_for(&store, position_id).await, 0);
        let event = receiver.recv().await.unwrap().unwrap();
        assert!(matches!(
            event,
            DaemonEvent::ReconciliationStaleNonActiveDetected { state, .. } if state == "entering"
        ));
    }

    #[tokio::test]
    async fn test_missing_exiting_skipped_without_close() {
        let exchange = Arc::new(StubExchange::new(dec!(100)));
        let store = Arc::new(MemoryStore::new());
        let event_bus = Arc::new(EventBus::new(16));
        let mut receiver = event_bus.subscribe();
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let mut position = tracked_active_position(symbol, Side::Long);
        position.state = PositionState::Exiting {
            exit_order_id: Uuid::now_v7(),
            exit_reason: ExitReason::TrailingStop,
        };
        let position_id = position.id;
        store.positions().save(&position).await.unwrap();
        let worker = create_worker(exchange, store.clone(), event_bus, Duration::from_secs(0));

        assert_eq!(worker.scan_and_reconcile().await.unwrap(), 0);

        let loaded = store.positions().find_by_id(position_id).await.unwrap().unwrap();
        assert!(matches!(loaded.state, PositionState::Exiting { .. }));
        assert_eq!(close_events_for(&store, position_id).await, 0);
        let event = receiver.recv().await.unwrap().unwrap();
        assert!(matches!(
            event,
            DaemonEvent::ReconciliationStaleNonActiveDetected { state, .. } if state == "exiting"
        ));
    }

    // -------------------------------------------------------------------------
    // TD-2026-05-05-001 — Stale-Active drift baseline canary (Slice 0).
    //
    // Documents the *current, buggy* behavior of the asymmetric reconciliation
    // loop: when a position is `Active` in Robson's store but absent on the
    // exchange, today's worker is a no-op. It does not detect the drift, does
    // not emit any event, and does not transition the local state.
    //
    // This test pins that observation. When Slice 4 lands the symmetric loop
    // and `reconcile_close`, the post-condition assertions in this test are
    // expected to fail; they will be inverted as part of that slice (the
    // position MUST then be `Closed` and a `CorePositionClosed` event MUST be
    // observed). Until then, the test passing is the diagnostic — not the
    // green check mark.
    //
    // See:
    //   - docs/implementation/TD-2026-05-05-001-CORE-LIFECYCLE-DRIFT.md
    //   - docs/analysis/2026-05-08-lifecycle-drift-repro.md
    //   - docs/technical-debt.md  (TD-2026-05-05-001)
    //   - docs/policies/UNTRACKED-POSITION-RECONCILIATION.md  (I3, pending)
    // -------------------------------------------------------------------------
    #[tokio::test]
    async fn test_reconciliation_does_not_close_active_missing_on_exchange() {
        let exchange = Arc::new(StubExchange::new(dec!(100)));
        let store = Arc::new(MemoryStore::new());
        let event_bus = Arc::new(EventBus::new(16));
        let mut receiver = event_bus.subscribe();

        let symbol = Symbol::from_pair("BTCUSDT").unwrap();

        // Robson's store believes a Long position is Active...
        let position = tracked_active_position(symbol.clone(), Side::Long);
        let position_id = position.id;
        store.positions().save(&position).await.unwrap();

        // ...but the exchange returns an empty open-positions list.
        // No `set_open_position` call. exchange.open_positions_len() == 0.
        assert_eq!(exchange.open_positions_len(), 0);

        let worker =
            create_worker(exchange.clone(), store.clone(), event_bus, Duration::from_secs(60));

        let reconciled = worker.scan_and_reconcile().await.unwrap();

        // Current behavior: the worker only walks `exchange.get_all_open_positions()`.
        // Empty list → zero iterations → no UNTRACKED action and, critically,
        // no reverse check against `store.find_active()`. Reconciled count is 0.
        assert_eq!(
            reconciled, 0,
            "baseline canary: today's asymmetric loop reports 0 even when \
             a stale Active exists locally"
        );

        // The Robson store is unchanged — the position is still Active.
        // This is the bug. Slice 4 will flip this assertion.
        let still_active = store
            .positions()
            .find_by_id(position_id)
            .await
            .unwrap()
            .expect("position must still exist in store");
        assert!(
            matches!(still_active.state, PositionState::Active { .. }),
            "baseline canary: stale Active is NOT transitioned to Closed today \
             (TD-2026-05-05-001)"
        );

        // No domain event was emitted: no PositionClosed, no CorePositionClosed,
        // no RoguePositionDetected, no SafetyExitExecuted, no
        // ReconciliationStaleNonActiveDetected. The drift is silent.
        assert!(
            receiver.try_recv().is_none(),
            "baseline canary: no event emitted today for Robson-Active / \
             exchange-missing drift"
        );
    }
}
