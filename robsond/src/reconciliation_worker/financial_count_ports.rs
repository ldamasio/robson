// Test-only adapters: count attempts at the real port/repository boundaries.
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use chrono::{DateTime, Utc};
use robson_domain::{OrderSide, SymbolTradingRules};
use robson_exec::ports::*;
use robson_store::{EventRepository, OrderRepository, PositionRepository, StoreError};

#[derive(Default)]
struct CountedStore {
    inner: MemoryStore,
    risk: AtomicUsize,
    active: AtomicUsize,
    fail_risk: AtomicBool,
    fail_active: AtomicBool,
}
#[async_trait::async_trait]
impl Store for CountedStore {
    fn positions(&self) -> &dyn PositionRepository {
        self
    }
    fn orders(&self) -> &dyn OrderRepository {
        self.inner.orders()
    }
    fn events(&self) -> &dyn EventRepository {
        self.inner.events()
    }
    fn apply_event(&self, event: &robson_domain::Event) -> Result<(), StoreError> {
        self.inner.apply_event(event)
    }
}
#[async_trait::async_trait]
impl PositionRepository for CountedStore {
    async fn save(&self, position: &Position) -> Result<(), StoreError> {
        self.inner.positions().save(position).await
    }
    async fn find_by_id(&self, id: PositionId) -> Result<Option<Position>, StoreError> {
        self.inner.positions().find_by_id(id).await
    }
    async fn find_by_account(&self, account_id: Uuid) -> Result<Vec<Position>, StoreError> {
        self.inner.positions().find_by_account(account_id).await
    }
    async fn find_active(&self) -> Result<Vec<Position>, StoreError> {
        self.active.fetch_add(1, Ordering::SeqCst);
        if self.fail_active.load(Ordering::SeqCst) {
            return Err(StoreError::Connection("counted local fault".into()));
        }
        self.inner.positions().find_active().await
    }
    async fn find_risk_open(&self) -> Result<Vec<Position>, StoreError> {
        self.risk.fetch_add(1, Ordering::SeqCst);
        if self.fail_risk.load(Ordering::SeqCst) {
            return Err(StoreError::Connection("counted local fault".into()));
        }
        self.inner.positions().find_risk_open().await
    }
    async fn find_by_state(&self, state: &str) -> Result<Vec<Position>, StoreError> {
        self.inner.positions().find_by_state(state).await
    }
    async fn find_active_by_symbol_and_side(
        &self,
        symbol: &robson_domain::Symbol,
        side: robson_domain::Side,
    ) -> Result<Option<Position>, StoreError> {
        self.inner.positions().find_active_by_symbol_and_side(symbol, side).await
    }
    async fn delete(&self, id: PositionId) -> Result<(), StoreError> {
        self.inner.positions().delete(id).await
    }
    async fn find_closed_in_month(
        &self,
        year: i32,
        month: u32,
    ) -> Result<Vec<Position>, StoreError> {
        self.inner.positions().find_closed_in_month(year, month).await
    }
    async fn find_all_closed(&self) -> Result<Vec<Position>, StoreError> {
        self.inner.positions().find_all_closed().await
    }
}
struct CountedExchange {
    inner: StubExchange,
    inventory: AtomicUsize,
    wallet: AtomicUsize,
    fail_inventory: AtomicBool,
    fail_wallet: AtomicBool,
}
impl CountedExchange {
    fn new() -> Self {
        Self {
            inner: StubExchange::new(dec!(100)),
            inventory: AtomicUsize::new(0),
            wallet: AtomicUsize::new(0),
            fail_inventory: AtomicBool::new(false),
            fail_wallet: AtomicBool::new(false),
        }
    }
}
#[async_trait::async_trait]
impl ExchangePort for CountedExchange {
    fn last_validated_rules(&self, symbol: &Symbol) -> Option<SymbolTradingRules> {
        self.inner.last_validated_rules(symbol)
    }
    async fn validate_futures_settings(
        &self,
        symbol: &Symbol,
        expected_leverage: u8,
    ) -> Result<FuturesSettings, ExecError> {
        self.inner.validate_futures_settings(symbol, expected_leverage).await
    }
    async fn trading_rules(&self, symbol: &Symbol) -> Result<SymbolTradingRules, ExecError> {
        self.inner.trading_rules(symbol).await
    }
    async fn trading_rules_preview(
        &self,
        symbol: &Symbol,
    ) -> Result<SymbolTradingRules, ExecError> {
        self.inner.trading_rules_preview(symbol).await
    }
    async fn trading_rules_fresh(&self, symbol: &Symbol) -> Result<SymbolTradingRules, ExecError> {
        self.inner.trading_rules_fresh(symbol).await
    }
    async fn refresh_trading_rules(
        &self,
        symbol: &Symbol,
    ) -> Result<SymbolTradingRules, ExecError> {
        self.inner.refresh_trading_rules(symbol).await
    }
    async fn place_market_order(
        &self,
        symbol: &Symbol,
        side: OrderSide,
        quantity: Quantity,
        client_order_id: &str,
        reduce_only: bool,
    ) -> Result<OrderResult, ExecError> {
        self.inner
            .place_market_order(symbol, side, quantity, client_order_id, reduce_only)
            .await
    }
    async fn place_stop_market_order(
        &self,
        symbol: &Symbol,
        side: OrderSide,
        quantity: Quantity,
        stop_price: Price,
        client_order_id: &str,
    ) -> Result<OrderResult, ExecError> {
        self.inner
            .place_stop_market_order(symbol, side, quantity, stop_price, client_order_id)
            .await
    }
    async fn cancel_stop_market_order(
        &self,
        symbol: &Symbol,
        algo_id: &str,
    ) -> Result<StopCancelOutcome, ExecError> {
        self.inner.cancel_stop_market_order(symbol, algo_id).await
    }
    async fn cancel_order(&self, symbol: &Symbol, order_id: &str) -> Result<(), ExecError> {
        self.inner.cancel_order(symbol, order_id).await
    }
    async fn get_open_orders(&self, symbol: &Symbol) -> Result<Vec<OpenOrderRecord>, ExecError> {
        self.inner.get_open_orders(symbol).await
    }
    async fn get_price(&self, symbol: &Symbol) -> Result<Price, ExecError> {
        self.inner.get_price(symbol).await
    }
    async fn health_check(&self) -> Result<(), ExecError> {
        self.inner.health_check().await
    }
    async fn get_futures_balance(&self) -> Result<FuturesBalance, ExecError> {
        self.wallet.fetch_add(1, Ordering::SeqCst);
        if self.fail_wallet.load(Ordering::SeqCst) {
            return Err(ExecError::Exchange("counted wallet fault".into()));
        }
        self.inner.get_futures_balance().await
    }
    async fn get_spot_account_balances(&self) -> Result<Vec<SpotBalance>, ExecError> {
        self.inner.get_spot_account_balances().await
    }
    async fn get_spot_price(&self, symbol: &str) -> Result<Price, ExecError> {
        self.inner.get_spot_price(symbol).await
    }
    async fn spot_symbol_is_trading(&self, symbol: &str) -> Result<bool, ExecError> {
        self.inner.spot_symbol_is_trading(symbol).await
    }
    async fn place_spot_market_order(
        &self,
        request: SpotOrderRequest,
    ) -> Result<SpotOrder, ExecError> {
        self.inner.place_spot_market_order(request).await
    }
    async fn get_spot_order(
        &self,
        symbol: &str,
        client_order_id: &str,
    ) -> Result<Option<SpotOrder>, ExecError> {
        self.inner.get_spot_order(symbol, client_order_id).await
    }
    async fn universal_transfer(
        &self,
        asset: &str,
        amount: Decimal,
        transfer_type: UniversalTransferType,
        client_tran_key: &str,
    ) -> Result<TransferId, ExecError> {
        self.inner
            .universal_transfer(asset, amount, transfer_type, client_tran_key)
            .await
    }
    async fn get_transfer_history(
        &self,
        transfer_type: UniversalTransferType,
        start_time: DateTime<Utc>,
    ) -> Result<Vec<Transfer>, ExecError> {
        self.inner.get_transfer_history(transfer_type, start_time).await
    }
    async fn get_all_open_positions(&self) -> Result<Vec<ExchangePosition>, ExecError> {
        self.inventory.fetch_add(1, Ordering::SeqCst);
        if self.fail_inventory.load(Ordering::SeqCst) {
            return Err(ExecError::Exchange("counted inventory fault".into()));
        }
        self.inner.get_all_open_positions().await
    }
    async fn close_position_market(
        &self,
        symbol: &Symbol,
        side: Side,
        quantity: Quantity,
        client_order_id: &str,
    ) -> Result<OrderResult, ExecError> {
        self.inner.close_position_market(symbol, side, quantity, client_order_id).await
    }
    async fn get_order_by_exchange_id(
        &self,
        symbol: &Symbol,
        order_id: &str,
    ) -> Result<Option<OrderResult>, ExecError> {
        self.inner.get_order_by_exchange_id(symbol, order_id).await
    }
    async fn get_stop_order_fill(
        &self,
        symbol: &Symbol,
        algo_id: &str,
    ) -> Result<Option<OrderResult>, ExecError> {
        self.inner.get_stop_order_fill(symbol, algo_id).await
    }
    async fn get_user_trades_since(
        &self,
        symbol: &Symbol,
        since: DateTime<Utc>,
        limit: u16,
    ) -> Result<Vec<UserTradeRecord>, ExecError> {
        self.inner.get_user_trades_since(symbol, since, limit).await
    }
}
