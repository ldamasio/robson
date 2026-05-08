//! Exchange reconciliation worker.
//!
//! Periodically compares exchange-open positions with Robson's in-memory store
//! and force-closes any position that is not tracked by the runtime.

use std::{sync::Arc, time::Duration};

use robson_exec::{ExchangePort, ExchangePosition};
use robson_store::Store;
use tokio::time::MissedTickBehavior;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::{
    error::{DaemonError, DaemonResult},
    event_bus::{DaemonEvent, EventBus},
};

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

        let worker = ReconciliationWorker::new(
            exchange.clone(),
            store.clone(),
            event_bus,
            Duration::from_secs(60),
            CancellationToken::new(),
        );

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
