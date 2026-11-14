//! Deliberately usable legacy surfaces under either canonical account kind.
//! This local fixture is never selectable by the daemon factory. Exposing
//! futures/funding even with a balance capability makes a missing guard visible
//! instead of masking it with an unsupported adapter or missing credentials.
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use robson_domain::{OrderSide, Price, Quantity, Side, Symbol, SymbolTradingRules};
use robson_exec::{
    gate::OperationGate,
    order_journal::OrderJournal,
    venue::{
        AccountKind, AccountPort, Cash, EntryOrReduce, ExecutionCostEnvelope, FundingPort,
        InventoryPort, Mark, OrderLookup, PlacedStop, PositionAccountPort, ReplaceOutcome,
        SettlementPort, SettlementRecord, StopCancelOutcome, StopParams, StopRef, TradingPort,
        Venue, VenueCapabilities, VenueId,
    },
    ExchangePosition, ExecError, FuturesBalance, FuturesSettings, OpenOrderRecord, OrderResult,
    SpotBalance, SpotOrder, SpotOrderRequest, StubExchange, Transfer, TransferId,
    UniversalTransferType, UserTradeRecord,
};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;

pub(crate) struct WorkerVenue {
    pub(crate) delegate: StubExchange,
    pub(crate) capabilities: VenueCapabilities,
    requests: AtomicUsize,
    funding_accesses: AtomicUsize,
    position_surface: bool,
}
impl WorkerVenue {
    pub(crate) fn new(kind: AccountKind, funding: bool) -> Self {
        let delegate = StubExchange::new(dec!(95000));
        let delegate = if funding {
            delegate
        } else {
            delegate.without_funding()
        };
        delegate.set_futures_balance(dec!(12000));
        delegate.set_spot_balance("USDT", dec!(100), Decimal::ZERO);
        delegate.set_spot_balance("ETH", dec!(1), Decimal::ZERO);
        delegate.set_price("ETHUSDT", dec!(3000));
        let mut capabilities = AccountPort::capabilities(&delegate);
        capabilities.account_kind = kind;
        Self {
            delegate,
            capabilities,
            requests: AtomicUsize::new(0),
            funding_accesses: AtomicUsize::new(0),
            position_surface: true,
        }
    }
    pub(crate) fn without_position_account(mut self) -> Self {
        self.position_surface = false;
        self
    }
    pub(crate) fn requests(&self) -> usize {
        self.requests.load(Ordering::SeqCst)
    }
    pub(crate) fn funding_accesses(&self) -> usize {
        self.funding_accesses.load(Ordering::SeqCst)
    }
    fn observe(&self) {
        self.requests.fetch_add(1, Ordering::SeqCst);
    }
}

#[async_trait]
impl TradingPort for WorkerVenue {
    async fn trading_rules(&self, symbol: &Symbol) -> Result<SymbolTradingRules, ExecError> {
        self.observe();
        TradingPort::trading_rules(&self.delegate, symbol).await
    }

    async fn trading_rules_preview(
        &self,
        symbol: &Symbol,
    ) -> Result<SymbolTradingRules, ExecError> {
        self.observe();
        TradingPort::trading_rules_preview(&self.delegate, symbol).await
    }

    async fn trading_rules_fresh(&self, symbol: &Symbol) -> Result<SymbolTradingRules, ExecError> {
        self.observe();
        TradingPort::trading_rules_fresh(&self.delegate, symbol).await
    }

    fn last_validated_rules(&self, symbol: &Symbol) -> Option<SymbolTradingRules> {
        TradingPort::last_validated_rules(&self.delegate, symbol)
    }

    fn rules_refresh_due(&self, symbol: &Symbol) -> bool {
        TradingPort::rules_refresh_due(&self.delegate, symbol)
    }

    async fn refresh_trading_rules(
        &self,
        symbol: &Symbol,
    ) -> Result<SymbolTradingRules, ExecError> {
        self.observe();
        TradingPort::refresh_trading_rules(&self.delegate, symbol).await
    }

    async fn place_market_order(
        &self,
        symbol: &Symbol,
        side: OrderSide,
        quantity: Quantity,
        client_order_id: &str,
        intent: EntryOrReduce,
    ) -> Result<OrderResult, ExecError> {
        self.observe();
        TradingPort::place_market_order(
            &self.delegate,
            symbol,
            side,
            quantity,
            client_order_id,
            intent,
        )
        .await
    }

    async fn place_protection(
        &self,
        symbol: &Symbol,
        close_side: OrderSide,
        quantity: Quantity,
        trigger: Price,
        client_order_id: &str,
    ) -> Result<PlacedStop, ExecError> {
        self.observe();
        TradingPort::place_protection(
            &self.delegate,
            symbol,
            close_side,
            quantity,
            trigger,
            client_order_id,
        )
        .await
    }

    async fn cancel_protection(
        &self,
        symbol: &Symbol,
        stop: &StopRef,
    ) -> Result<StopCancelOutcome, ExecError> {
        self.observe();
        TradingPort::cancel_protection(&self.delegate, symbol, stop).await
    }

    async fn replace_protection(
        &self,
        symbol: &Symbol,
        old: &StopRef,
        new: StopParams,
    ) -> Result<ReplaceOutcome, ExecError> {
        self.observe();
        TradingPort::replace_protection(&self.delegate, symbol, old, new).await
    }

    async fn cancel_order(&self, symbol: &Symbol, order_id: &str) -> Result<(), ExecError> {
        self.observe();
        TradingPort::cancel_order(&self.delegate, symbol, order_id).await
    }

    async fn order_by_client_id(
        &self,
        symbol: &Symbol,
        client_order_id: &str,
    ) -> Result<OrderLookup, ExecError> {
        self.observe();
        TradingPort::order_by_client_id(&self.delegate, symbol, client_order_id).await
    }

    async fn order_by_exchange_id(
        &self,
        symbol: &Symbol,
        order_id: &str,
    ) -> Result<Option<OrderResult>, ExecError> {
        self.observe();
        TradingPort::order_by_exchange_id(&self.delegate, symbol, order_id).await
    }

    async fn protection_fill(
        &self,
        symbol: &Symbol,
        stop: &StopRef,
    ) -> Result<Option<OrderResult>, ExecError> {
        self.observe();
        TradingPort::protection_fill(&self.delegate, symbol, stop).await
    }

    async fn open_protective_orders(
        &self,
        symbol: &Symbol,
    ) -> Result<Vec<OpenOrderRecord>, ExecError> {
        self.observe();
        TradingPort::open_protective_orders(&self.delegate, symbol).await
    }

    async fn user_trades_since(
        &self,
        symbol: &Symbol,
        since: DateTime<Utc>,
        limit: u16,
    ) -> Result<Vec<UserTradeRecord>, ExecError> {
        self.observe();
        TradingPort::user_trades_since(&self.delegate, symbol, since, limit).await
    }

    async fn price(&self, symbol: &Symbol) -> Result<Price, ExecError> {
        self.observe();
        TradingPort::price(&self.delegate, symbol).await
    }

    async fn health(&self) -> Result<(), ExecError> {
        self.observe();
        TradingPort::health(&self.delegate).await
    }
}

#[async_trait]
impl AccountPort for WorkerVenue {
    async fn cash(&self, asset: &str) -> Result<Cash, ExecError> {
        self.observe();
        AccountPort::cash(&self.delegate, asset).await
    }

    async fn valuation(&self, symbol: &Symbol) -> Result<Mark, ExecError> {
        self.observe();
        AccountPort::valuation(&self.delegate, symbol).await
    }

    async fn execution_costs(&self, symbol: &Symbol) -> Result<ExecutionCostEnvelope, ExecError> {
        self.observe();
        AccountPort::execution_costs(&self.delegate, symbol).await
    }

    fn capabilities(&self) -> VenueCapabilities {
        self.capabilities.clone()
    }
}

impl Venue for WorkerVenue {
    fn id(&self) -> &VenueId {
        self.delegate.id()
    }
    fn attach_gate(&self, gate: Arc<OperationGate>) {
        self.delegate.attach_gate(gate);
    }
    fn attach_order_journal(&self, journal: Arc<OrderJournal>) {
        self.delegate.attach_order_journal(journal);
    }
    fn position_account(&self) -> Option<&dyn PositionAccountPort> {
        self.position_surface.then_some(self)
    }
    fn inventory(&self) -> Option<&dyn InventoryPort> {
        None
    }
    fn settlement(&self) -> &dyn SettlementPort {
        self
    }
    fn funding(&self) -> Option<&dyn FundingPort> {
        self.funding_accesses.fetch_add(1, Ordering::SeqCst);
        self.capabilities.has_funding.then_some(self)
    }
}
#[async_trait]
impl PositionAccountPort for WorkerVenue {
    async fn validate_position_settings(
        &self,
        symbol: &Symbol,
        leverage: u8,
    ) -> Result<FuturesSettings, ExecError> {
        self.observe();
        self.delegate.validate_position_settings(symbol, leverage).await
    }
    async fn balance(&self) -> Result<FuturesBalance, ExecError> {
        self.observe();
        self.delegate.balance().await
    }
    async fn open_positions(&self) -> Result<Vec<ExchangePosition>, ExecError> {
        self.observe();
        self.delegate.open_positions().await
    }
    async fn close_position_market(
        &self,
        symbol: &Symbol,
        side: Side,
        qty: Quantity,
        id: &str,
    ) -> Result<OrderResult, ExecError> {
        self.observe();
        self.delegate.close_position_market(symbol, side, qty, id).await
    }
}
#[async_trait]
impl SettlementPort for WorkerVenue {
    async fn settlement_since(
        &self,
        since: DateTime<Utc>,
        limit: u16,
    ) -> Result<Vec<SettlementRecord>, ExecError> {
        self.observe();
        self.delegate.settlement_since(since, limit).await
    }
}
#[async_trait]
impl FundingPort for WorkerVenue {
    async fn spot_balances(&self) -> Result<Vec<SpotBalance>, ExecError> {
        self.observe();
        self.delegate.spot_balances().await
    }
    async fn spot_price(&self, symbol: &str) -> Result<Price, ExecError> {
        self.observe();
        self.delegate.spot_price(symbol).await
    }
    async fn spot_symbol_is_trading(&self, symbol: &str) -> Result<bool, ExecError> {
        self.observe();
        self.delegate.spot_symbol_is_trading(symbol).await
    }
    async fn place_spot_market_order(
        &self,
        request: SpotOrderRequest,
    ) -> Result<SpotOrder, ExecError> {
        self.observe();
        self.delegate.place_spot_market_order(request).await
    }
    async fn spot_order(&self, symbol: &str, id: &str) -> Result<Option<SpotOrder>, ExecError> {
        self.observe();
        self.delegate.spot_order(symbol, id).await
    }
    async fn transfer(
        &self,
        asset: &str,
        amount: Decimal,
        kind: UniversalTransferType,
        key: &str,
    ) -> Result<TransferId, ExecError> {
        self.observe();
        self.delegate.transfer(asset, amount, kind, key).await
    }
    async fn transfer_history(
        &self,
        kind: UniversalTransferType,
        since: DateTime<Utc>,
    ) -> Result<Vec<Transfer>, ExecError> {
        self.observe();
        self.delegate.transfer_history(kind, since).await
    }
}

/// Any store access is a fixture failure, including a transaction or
/// projection.
pub(crate) struct NoAccessStore;
#[async_trait]
impl robson_store::Store for NoAccessStore {
    fn positions(&self) -> &dyn robson_store::PositionRepository {
        panic!("unexpected position repository access");
    }
    fn orders(&self) -> &dyn robson_store::OrderRepository {
        panic!("unexpected order repository access");
    }
    fn events(&self) -> &dyn robson_store::EventRepository {
        panic!("unexpected event repository access");
    }
    fn apply_event(&self, _: &robson_domain::Event) -> Result<(), robson_store::StoreError> {
        panic!("unexpected projection write");
    }
    async fn begin_transaction(&self) -> Result<(), robson_store::StoreError> {
        panic!("unexpected transaction");
    }
    async fn commit(&self) -> Result<(), robson_store::StoreError> {
        panic!("unexpected commit");
    }
    async fn rollback(&self) -> Result<(), robson_store::StoreError> {
        panic!("unexpected rollback");
    }
}

pub(crate) fn manager<S: robson_store::Store + 'static>(
    exchange: &Arc<WorkerVenue>,
    store: Arc<S>,
) -> Arc<tokio::sync::RwLock<crate::position_manager::PositionManager<WorkerVenue, S>>> {
    let executor = Arc::new(robson_exec::Executor::new(
        exchange.clone(),
        Arc::new(robson_exec::IntentJournal::new()),
        store.clone(),
    ));
    Arc::new(tokio::sync::RwLock::new(crate::position_manager::PositionManager::new(
        robson_engine::Engine::new(robson_domain::RiskConfig::new(dec!(10000)).unwrap()),
        executor,
        store,
        Arc::new(crate::event_bus::EventBus::new(32)),
        Arc::new(crate::query_engine::TracingQueryRecorder),
        robson_domain::TradingPolicy::default(),
    )))
}

#[cfg(feature = "postgres")]
pub(crate) fn disconnected_pool() -> sqlx::PgPool {
    // No URL or credentials, and no eager connection. Tests close it before
    // direct misuse, so a missing guard cannot contact another database.
    sqlx::postgres::PgPoolOptions::new()
        .min_connections(0)
        .connect_lazy_with(sqlx::postgres::PgConnectOptions::new().host("127.0.0.1").port(1))
}

/// Capability refusal must complete before waiting for any external resource.
pub(crate) async fn immediately<F: std::future::Future>(future: F) -> F::Output {
    let mut future = std::pin::pin!(future);
    std::future::poll_fn(|cx| match future.as_mut().poll(cx) {
        std::task::Poll::Ready(result) => std::task::Poll::Ready(result),
        std::task::Poll::Pending => panic!("account refusal waited on a resource"),
    })
    .await
}

#[cfg(feature = "postgres")]
pub(crate) async fn counted_pool(
    pool: &sqlx::PgPool,
) -> (robson_testkit::db_proxy::DbProxy, sqlx::PgPool) {
    use sqlx::postgres::{PgPoolOptions, PgSslMode};
    let options = pool.connect_options();
    let upstream = tokio::net::lookup_host((options.get_host(), options.get_port()))
        .await
        .unwrap()
        .next()
        .unwrap();
    let proxy = robson_testkit::db_proxy::DbProxy::start(upstream).await.unwrap();
    let proxied = PgPoolOptions::new()
        .max_connections(1)
        .min_connections(1)
        .test_before_acquire(false)
        .acquire_timeout(std::time::Duration::from_secs(2))
        .connect_with(
            options
                .as_ref()
                .clone()
                .host("127.0.0.1")
                .port(proxy.addr().port())
                .ssl_mode(PgSslMode::Disable),
        )
        .await
        .unwrap();
    (proxy, proxied)
}
