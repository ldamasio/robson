//! Exchange reconciliation worker.
//!
//! Periodically compares exchange-open positions with Robson's in-memory store
//! and force-closes any position that is not tracked by the runtime.

use std::{sync::Arc, time::Duration};

use tokio::time::MissedTickBehavior;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::{
    error::{DaemonError, DaemonResult},
    event_bus::{DaemonEvent, EventBus},
};
use robson_exec::{ExchangePort, ExchangePosition};
use robson_store::Store;

/// Background worker that reconciles exchange state with Robson state.
pub struct ReconciliationWorker<E: ExchangePort + 'static, S: Store + 'static> {
    exchange: Arc<E>,
    store: Arc<S>,
    event_bus: Arc<EventBus>,
    scan_interval: Duration,
    shutdown_token: CancellationToken,
}

impl<E: ExchangePort + 'static, S: Store + 'static> ReconciliationWorker<E, S> {
    /// Create a new reconciliation worker.
    pub fn new(
        exchange: Arc<E>,
        store: Arc<S>,
        event_bus: Arc<EventBus>,
        scan_interval: Duration,
        shutdown_token: CancellationToken,
    ) -> Self {
        Self {
            exchange,
            store,
            event_bus,
            scan_interval,
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

        for exchange_position in exchange_positions {
            if self.is_tracked(&exchange_position).await? {
                continue;
            }

            self.handle_untracked_position(exchange_position).await?;
            reconciled += 1;
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
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use chrono::Utc;
    use robson_domain::{Position, PositionState, Price, Quantity, Side, Symbol};
    use robson_exec::StubExchange;
    use robson_store::{MemoryStore, Store};
    use rust_decimal_macros::dec;
    use tokio_util::sync::CancellationToken;

    use super::*;

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

        let worker = ReconciliationWorker::new(
            exchange.clone(),
            store,
            event_bus,
            Duration::from_secs(60),
            CancellationToken::new(),
        );

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

        let worker = ReconciliationWorker::new(
            exchange.clone(),
            store,
            event_bus,
            Duration::from_secs(60),
            CancellationToken::new(),
        );

        let reconciled = worker.scan_and_reconcile().await.unwrap();
        assert_eq!(reconciled, 0);
        assert_eq!(exchange.open_positions_len(), 1);
        assert!(receiver.try_recv().is_none());
    }
}
