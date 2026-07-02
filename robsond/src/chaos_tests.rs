use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};

use async_trait::async_trait;
use robson_domain::{Event, OrderSide, PositionId, Price, Quantity, Side, Symbol};
use robson_engine::{EngineAction, RiskCheck, RiskVerdict};
use robson_exec::{
    ExchangePort, ExecError, Executor, FuturesBalance, FuturesSettings, SpotBalance, SpotOrder,
    SpotOrderRequest, Transfer, TransferId, UniversalTransferType,
};
use robson_store::{
    EventRepository, MemoryStore, OrderRepository, PositionRepository, Store, StoreError,
};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use uuid::Uuid;

use crate::{
    error::{exit_code_for_daemon_error, DaemonError},
    market_data::next_reconnect_backoff_secs,
    query_engine::evaluate_risk_with_timeout_for_test,
};

struct TimeoutExchange {
    attempts: AtomicUsize,
}

impl TimeoutExchange {
    fn new() -> Self {
        Self { attempts: AtomicUsize::new(0) }
    }

    fn attempts(&self) -> usize {
        self.attempts.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl ExchangePort for TimeoutExchange {
    async fn validate_futures_settings(
        &self,
        symbol: &Symbol,
        expected_leverage: u8,
    ) -> Result<FuturesSettings, ExecError> {
        Ok(FuturesSettings {
            position_mode: "One-way".to_string(),
            leverage: expected_leverage,
            symbol: symbol.as_pair(),
        })
    }

    async fn place_market_order(
        &self,
        _symbol: &Symbol,
        _side: OrderSide,
        _quantity: Quantity,
        _client_order_id: &str,
        _reduce_only: bool,
    ) -> Result<robson_exec::OrderResult, ExecError> {
        self.attempts.fetch_add(1, Ordering::SeqCst);
        Err(ExecError::Timeout("simulated exchange timeout".to_string()))
    }

    async fn place_stop_market_order(
        &self,
        _symbol: &Symbol,
        _side: OrderSide,
        _quantity: Quantity,
        _stop_price: Price,
        _client_order_id: &str,
    ) -> Result<robson_exec::OrderResult, ExecError> {
        Err(ExecError::Timeout("simulated exchange timeout".to_string()))
    }

    async fn cancel_order(&self, _symbol: &Symbol, _order_id: &str) -> Result<(), ExecError> {
        Ok(())
    }

    async fn get_price(&self, _symbol: &Symbol) -> Result<Price, ExecError> {
        Price::new(dec!(100)).map_err(ExecError::Domain)
    }

    async fn health_check(&self) -> Result<(), ExecError> {
        Ok(())
    }

    async fn get_futures_balance(&self) -> Result<FuturesBalance, ExecError> {
        Ok(FuturesBalance {
            wallet_balance: dec!(10000),
            available_balance: dec!(10000),
        })
    }

    async fn get_spot_account_balances(&self) -> Result<Vec<SpotBalance>, ExecError> {
        Ok(vec![])
    }

    async fn get_spot_price(&self, _symbol: &str) -> Result<Price, ExecError> {
        Price::new(dec!(100)).map_err(ExecError::Domain)
    }

    async fn place_spot_market_order(
        &self,
        _request: SpotOrderRequest,
    ) -> Result<SpotOrder, ExecError> {
        Err(ExecError::Timeout("simulated exchange timeout".to_string()))
    }

    async fn get_spot_order(
        &self,
        _symbol: &str,
        _client_order_id: &str,
    ) -> Result<Option<SpotOrder>, ExecError> {
        Ok(None)
    }

    async fn universal_transfer(
        &self,
        _asset: &str,
        _amount: Decimal,
        _transfer_type: UniversalTransferType,
        _client_tran_key: &str,
    ) -> Result<TransferId, ExecError> {
        Err(ExecError::Timeout("simulated exchange timeout".to_string()))
    }

    async fn get_transfer_history(
        &self,
        _transfer_type: UniversalTransferType,
        _start_time: chrono::DateTime<chrono::Utc>,
    ) -> Result<Vec<Transfer>, ExecError> {
        Ok(vec![])
    }

    async fn get_all_open_positions(
        &self,
    ) -> Result<Vec<robson_exec::ExchangePosition>, ExecError> {
        Ok(vec![])
    }

    async fn close_position_market(
        &self,
        _symbol: &Symbol,
        _side: Side,
        _quantity: Quantity,
        _client_order_id: &str,
    ) -> Result<robson_exec::OrderResult, ExecError> {
        Err(ExecError::Timeout("simulated exchange timeout".to_string()))
    }

    async fn get_order_by_exchange_id(
        &self,
        _symbol: &Symbol,
        _order_id: &str,
    ) -> Result<Option<robson_exec::OrderResult>, ExecError> {
        Ok(None)
    }

    async fn get_user_trades_since(
        &self,
        _symbol: &Symbol,
        _since: chrono::DateTime<chrono::Utc>,
        _limit: u16,
    ) -> Result<Vec<robson_exec::UserTradeRecord>, ExecError> {
        Ok(vec![])
    }
}

struct FailingEventStore {
    inner: MemoryStore,
}

impl FailingEventStore {
    fn new() -> Self {
        Self { inner: MemoryStore::new() }
    }
}

#[async_trait]
impl EventRepository for FailingEventStore {
    async fn append(&self, _event: &Event) -> Result<i64, StoreError> {
        Err(StoreError::Database("simulated event log write failure".to_string()))
    }

    async fn find_by_position(&self, _position_id: PositionId) -> Result<Vec<Event>, StoreError> {
        Ok(vec![])
    }

    async fn find_by_position_after(
        &self,
        _position_id: PositionId,
        _after_seq: i64,
    ) -> Result<Vec<Event>, StoreError> {
        Ok(vec![])
    }

    async fn get_latest_seq(&self, _position_id: PositionId) -> Result<Option<i64>, StoreError> {
        Ok(None)
    }

    async fn get_all_events(&self) -> Result<Vec<Event>, StoreError> {
        Ok(vec![])
    }
}

impl Store for FailingEventStore {
    fn positions(&self) -> &dyn PositionRepository {
        self.inner.positions()
    }

    fn orders(&self) -> &dyn OrderRepository {
        self.inner.orders()
    }

    fn events(&self) -> &dyn EventRepository {
        self
    }
}

fn entry_order_action(position_id: PositionId) -> EngineAction {
    let signal_id = Uuid::now_v7();
    EngineAction::PlaceEntryOrder {
        position_id,
        cycle_id: Some(Uuid::now_v7()),
        symbol: Symbol::from_pair("BTCUSDT").unwrap(),
        side: OrderSide::Buy,
        quantity: Quantity::new(dec!(0.01)).unwrap(),
        order_id: Uuid::now_v7(),
        client_order_id: signal_id.to_string(),
        expected_price: Price::new(dec!(100000)).unwrap(),
        signal_id,
    }
}

#[tokio::test]
async fn chaos_risk_engine_slow_denies_by_timeout_safe_default() {
    let verdict = evaluate_risk_with_timeout_for_test(|| {
        std::thread::sleep(Duration::from_millis(250));
        RiskVerdict::Approved
    })
    .await;

    assert!(matches!(verdict, RiskVerdict::Rejected {
        check: RiskCheck::RiskEngineTimeout,
        ..
    }));
}

#[tokio::test]
async fn chaos_exchange_timeout_does_not_retry_ambiguous_entry_order() {
    let exchange = Arc::new(TimeoutExchange::new());
    let store = Arc::new(MemoryStore::new());
    let executor = Executor::new(exchange.clone(), Arc::new(Default::default()), store);

    let results = executor.execute(vec![entry_order_action(Uuid::now_v7())]).await.unwrap();

    assert_eq!(exchange.attempts(), 1);
    assert!(matches!(results.as_slice(), [robson_exec::ActionResult::OrderFailed { .. }]));
}

#[test]
fn chaos_websocket_disconnect_policy_reconnects_with_capped_backoff() {
    let mut backoff = 1;
    backoff = next_reconnect_backoff_secs(backoff);
    assert_eq!(backoff, 2);
    backoff = next_reconnect_backoff_secs(backoff);
    assert_eq!(backoff, 4);

    let capped = (0..10).fold(backoff, |current, _| next_reconnect_backoff_secs(current));
    assert_eq!(capped, 60);
}

#[test]
fn chaos_startup_stale_active_maps_to_exit_code_78() {
    let err = DaemonError::StartupStaleActiveDetected { count: 1, positions: vec![] };
    assert_eq!(exit_code_for_daemon_error(&err), 78);
}

#[tokio::test]
async fn chaos_event_log_write_failure_aborts_before_exchange_order() {
    let exchange = Arc::new(TimeoutExchange::new());
    let store = Arc::new(FailingEventStore::new());
    let executor = Executor::new(exchange.clone(), Arc::new(Default::default()), store);
    let position_id = Uuid::now_v7();
    let event = Event::PositionArmed {
        position_id,
        account_id: Uuid::nil(),
        symbol: Symbol::from_pair("BTCUSDT").unwrap(),
        side: Side::Long,
        tech_stop_distance: None,
        timestamp: chrono::Utc::now(),
    };

    let result = executor
        .execute(vec![
            EngineAction::EmitEvent(event),
            entry_order_action(position_id),
        ])
        .await;

    assert!(result.is_err());
    assert_eq!(exchange.attempts(), 0);
}
