//! Local capability fixture: canonical legacy delegate, no selectable test
//! venue. Every async account/legacy operation is counted, so denial assertions
//! cover reads as well as sends. Only the canonical capability result is
//! restricted.
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use robson_domain::{OrderSide, Price, Quantity, Side, Symbol, SymbolTradingRules};
use robson_exec::{
    venue::{AccountPort, Cash, ExecutionCostEnvelope, Mark, VenueCapabilities},
    ExchangePort, ExchangePosition, ExecError, FuturesBalance, FuturesSettings, OpenOrderRecord,
    OrderResult, SpotBalance, SpotOrder, SpotOrderRequest, StopCancelOutcome, StubExchange,
    Transfer, TransferId, UniversalTransferType, UserTradeRecord,
};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;

pub(crate) struct RestrictedExchange {
    delegate: StubExchange,
    requests: AtomicUsize,
}

impl RestrictedExchange {
    pub(crate) fn new() -> Self {
        let delegate = StubExchange::new(dec!(100));
        delegate.set_futures_balance(dec!(10000));
        Self { delegate, requests: AtomicUsize::new(0) }
    }

    pub(crate) fn requests(&self) -> usize {
        self.requests.load(Ordering::SeqCst)
    }

    fn requested(&self) {
        self.requests.fetch_add(1, Ordering::SeqCst);
    }
}

#[async_trait]
impl AccountPort for RestrictedExchange {
    async fn cash(&self, asset: &str) -> Result<Cash, ExecError> {
        self.requested();
        AccountPort::cash(&self.delegate, asset).await
    }
    async fn valuation(&self, symbol: &Symbol) -> Result<Mark, ExecError> {
        self.requested();
        AccountPort::valuation(&self.delegate, symbol).await
    }
    async fn execution_costs(&self, symbol: &Symbol) -> Result<ExecutionCostEnvelope, ExecError> {
        self.requested();
        AccountPort::execution_costs(&self.delegate, symbol).await
    }
    fn capabilities(&self) -> VenueCapabilities {
        robson_testkit::balance_stub::spot_capabilities()
    }
}

#[async_trait]
impl ExchangePort for RestrictedExchange {
    async fn trading_rules(&self, symbol: &Symbol) -> Result<SymbolTradingRules, ExecError> {
        self.requested();
        ExchangePort::trading_rules(&self.delegate, symbol).await
    }
    async fn validate_futures_settings(
        &self,
        symbol: &Symbol,
        expected_leverage: u8,
    ) -> Result<FuturesSettings, ExecError> {
        self.requested();
        ExchangePort::validate_futures_settings(&self.delegate, symbol, expected_leverage).await
    }
    async fn place_market_order(
        &self,
        symbol: &Symbol,
        side: OrderSide,
        quantity: Quantity,
        client_order_id: &str,
        reduce_only: bool,
    ) -> Result<OrderResult, ExecError> {
        self.requested();
        ExchangePort::place_market_order(
            &self.delegate,
            symbol,
            side,
            quantity,
            client_order_id,
            reduce_only,
        )
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
        self.requested();
        ExchangePort::place_stop_market_order(
            &self.delegate,
            symbol,
            side,
            quantity,
            stop_price,
            client_order_id,
        )
        .await
    }
    async fn cancel_stop_market_order(
        &self,
        symbol: &Symbol,
        algo_id: &str,
    ) -> Result<StopCancelOutcome, ExecError> {
        self.requested();
        ExchangePort::cancel_stop_market_order(&self.delegate, symbol, algo_id).await
    }
    async fn cancel_order(&self, symbol: &Symbol, order_id: &str) -> Result<(), ExecError> {
        self.requested();
        ExchangePort::cancel_order(&self.delegate, symbol, order_id).await
    }
    async fn get_open_orders(&self, symbol: &Symbol) -> Result<Vec<OpenOrderRecord>, ExecError> {
        self.requested();
        ExchangePort::get_open_orders(&self.delegate, symbol).await
    }
    async fn get_price(&self, symbol: &Symbol) -> Result<Price, ExecError> {
        self.requested();
        ExchangePort::get_price(&self.delegate, symbol).await
    }
    async fn health_check(&self) -> Result<(), ExecError> {
        self.requested();
        ExchangePort::health_check(&self.delegate).await
    }
    async fn get_futures_balance(&self) -> Result<FuturesBalance, ExecError> {
        self.requested();
        ExchangePort::get_futures_balance(&self.delegate).await
    }
    async fn get_spot_account_balances(&self) -> Result<Vec<SpotBalance>, ExecError> {
        self.requested();
        ExchangePort::get_spot_account_balances(&self.delegate).await
    }
    async fn get_spot_price(&self, symbol: &str) -> Result<Price, ExecError> {
        self.requested();
        ExchangePort::get_spot_price(&self.delegate, symbol).await
    }
    async fn place_spot_market_order(
        &self,
        request: SpotOrderRequest,
    ) -> Result<SpotOrder, ExecError> {
        self.requested();
        ExchangePort::place_spot_market_order(&self.delegate, request).await
    }
    async fn get_spot_order(
        &self,
        symbol: &str,
        client_order_id: &str,
    ) -> Result<Option<SpotOrder>, ExecError> {
        self.requested();
        ExchangePort::get_spot_order(&self.delegate, symbol, client_order_id).await
    }
    async fn universal_transfer(
        &self,
        asset: &str,
        amount: Decimal,
        transfer_type: UniversalTransferType,
        client_tran_key: &str,
    ) -> Result<TransferId, ExecError> {
        self.requested();
        ExchangePort::universal_transfer(
            &self.delegate,
            asset,
            amount,
            transfer_type,
            client_tran_key,
        )
        .await
    }
    async fn get_transfer_history(
        &self,
        transfer_type: UniversalTransferType,
        start_time: DateTime<Utc>,
    ) -> Result<Vec<Transfer>, ExecError> {
        self.requested();
        ExchangePort::get_transfer_history(&self.delegate, transfer_type, start_time).await
    }
    async fn get_all_open_positions(&self) -> Result<Vec<ExchangePosition>, ExecError> {
        self.requested();
        ExchangePort::get_all_open_positions(&self.delegate).await
    }
    async fn close_position_market(
        &self,
        symbol: &Symbol,
        side: Side,
        quantity: Quantity,
        client_order_id: &str,
    ) -> Result<OrderResult, ExecError> {
        self.requested();
        ExchangePort::close_position_market(&self.delegate, symbol, side, quantity, client_order_id)
            .await
    }
    async fn get_order_by_exchange_id(
        &self,
        symbol: &Symbol,
        order_id: &str,
    ) -> Result<Option<OrderResult>, ExecError> {
        self.requested();
        ExchangePort::get_order_by_exchange_id(&self.delegate, symbol, order_id).await
    }
    async fn get_stop_order_fill(
        &self,
        symbol: &Symbol,
        algo_id: &str,
    ) -> Result<Option<OrderResult>, ExecError> {
        self.requested();
        ExchangePort::get_stop_order_fill(&self.delegate, symbol, algo_id).await
    }
    async fn get_user_trades_since(
        &self,
        symbol: &Symbol,
        since: DateTime<Utc>,
        limit: u16,
    ) -> Result<Vec<UserTradeRecord>, ExecError> {
        self.requested();
        ExchangePort::get_user_trades_since(&self.delegate, symbol, since, limit).await
    }
}

#[tokio::test]
async fn restricted_fixture_counts_reads_and_uses_canonical_delegate() {
    let exchange = RestrictedExchange::new();
    let symbol = Symbol::from_pair("ETHUSDT").unwrap();
    let capabilities = exchange.capabilities();
    assert!(!capabilities.supports_short);
    assert_eq!(capabilities.account_kind, robson_exec::venue::AccountKind::BalanceAccount);
    assert_eq!(exchange.requests(), 0, "static capability inspection is not I/O");
    assert_eq!(exchange.cash("USDT").await.unwrap().free, dec!(10000));
    assert_eq!(exchange.valuation(&symbol).await.unwrap().price, Price::new(dec!(100)).unwrap());
    assert_eq!(
        ExchangePort::get_price(&exchange, &symbol).await.unwrap(),
        Price::new(dec!(100)).unwrap()
    );
    assert_eq!(exchange.requests(), 3, "both canonical and legacy reads must be counted");
}
