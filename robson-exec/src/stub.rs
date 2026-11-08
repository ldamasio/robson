//! Stub implementations for testing.
//!
//! These implementations simulate exchange and market data behavior
//! without making real API calls.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::{Mutex, RwLock},
    time::Duration,
};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use robson_domain::{Candle, OrderSide, Price, Quantity, RiskConfig, Symbol};
use rust_decimal::Decimal;

use crate::{
    error::ExecError,
    ports::{
        CandleInterval, ExchangePosition, FuturesBalance, FuturesSettings, IncomeRecord,
        MarketDataPort, OhlcvPort, OpenOrderRecord, OrderResult, PriceUpdate, SpotBalance,
        SpotOrder, SpotOrderQuantity, SpotOrderRequest, SpotOrderSide, TradeFeedMessage,
        TradeFeedPort, TradeStream, Transfer, TransferId, UniversalTransferType, UserTradeRecord,
    },
    venue::{
        self, AccountKind, Cash, EntryOrReduce, ExecutionCostEnvelope, FeeAssetPolicy,
        FundingTerms, Mark, MarketKind, OrderLookup, PlacedStop, ProtectionKind, ReplaceOutcome,
        ReplacementProtocol, StopCancelOutcome, StopParams, StopRef, SubmitAttemptLedger,
        VenueCapabilities, VenueFinalityRule, VenueId,
    },
};

// =============================================================================
// Stub Exchange
// =============================================================================

/// A protective stop algo order recorded on the stub exchange (ADR-0039).
///
/// Placed via `place_stop_market_order` and removed on `cancel_order`, so tests
/// can assert the insurance-stop lifecycle without a real exchange.
#[derive(Debug, Clone)]
pub struct StubStopOrder {
    /// Exchange-assigned algo id (e.g. `STUB-1`)
    pub exchange_order_id: String,
    /// Client order id supplied by the caller
    pub client_order_id: String,
    /// Trading pair the order protects
    pub symbol: Symbol,
    /// Close side for the protected position
    pub side: OrderSide,
    /// Protected quantity
    pub quantity: Quantity,
    /// Chart-derived stop price
    pub stop_price: Price,
    /// When the order was placed
    pub placed_at: DateTime<Utc>,
}

/// Bound on each stub order history and on its submit-attempt evidence. The
/// stub also backs the daemon's stub runtime, which can run indefinitely.
const STUB_HISTORY_CAPACITY: usize = 4096;

/// Insertion-ordered map of at most `capacity` entries; the oldest is dropped
/// when full. A lookup whose record was dropped falls through to the attempt
/// evidence and reads `Unknown`: the attempt behind an accepted order never
/// resolves as rejected.
struct BoundedHistory<V> {
    capacity: usize,
    entries: HashMap<String, V>,
    order: VecDeque<String>,
}

impl<V> BoundedHistory<V> {
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            entries: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    fn insert(&mut self, key: String, value: V) {
        if !self.entries.contains_key(&key) {
            if self.order.len() >= self.capacity {
                if let Some(oldest) = self.order.pop_front() {
                    self.entries.remove(&oldest);
                }
            }
            self.order.push_back(key.clone());
        }
        self.entries.insert(key, value);
    }

    fn get(&self, key: &str) -> Option<&V> {
        self.entries.get(key)
    }

    fn values(&self) -> impl Iterator<Item = &V> {
        self.entries.values()
    }
}

/// Stub exchange for testing.
///
/// Simulates immediate fills at a configured price.
/// Default configuration: One-way position mode, 1x leverage.
pub struct StubExchange {
    /// Venue id (`stub`).
    venue_id: VenueId,
    /// Whether the optional funding surface is exposed (default: yes).
    has_funding: bool,
    /// Current prices by symbol
    prices: RwLock<HashMap<String, Decimal>>,
    /// Default price for unknown symbols
    default_price: Decimal,
    /// Simulated fee rate (0.001 = 0.1%)
    fee_rate: Decimal,
    /// Order counter for generating IDs
    order_counter: RwLock<u64>,
    /// Whether to simulate failures
    fail_next: RwLock<bool>,
    /// Whether to simulate a futures order placement failure only.
    order_fail_next: RwLock<bool>,
    /// Simulated futures settings (position_mode, leverage)
    futures_settings: RwLock<(String, u8)>,
    /// Simulated open positions returned by reconciliation scans.
    open_positions: RwLock<HashMap<String, ExchangePosition>>,
    /// Simulated futures account balance.
    futures_balance: RwLock<Decimal>,
    /// Simulated unrealized P&L reported next to the wallet balance.
    unrealized_pnl: RwLock<Decimal>,
    /// Market orders accepted via `place_market_order` or
    /// `close_position_market`, keyed by client id, for `order_by_client_id`
    /// (ADR-0056). Bounded.
    client_orders: RwLock<BoundedHistory<OrderResult>>,
    /// Submit-attempt evidence per client id (placements, accepted or not).
    /// Bounded; in memory, see its restart limitation.
    submit_attempts: SubmitAttemptLedger,
    /// Protective stops cancelled on the stub, keyed by algo id, so lookups
    /// by client id still see them as terminal. Bounded.
    cancelled_stop_orders: RwLock<BoundedHistory<StubStopOrder>>,
    /// Simulated regular orders retrievable by `get_order_by_exchange_id`.
    orders: RwLock<HashMap<String, OrderResult>>,
    /// Simulated user trades retrievable by `get_user_trades_since`.
    user_trades: RwLock<HashMap<String, Vec<UserTradeRecord>>>,
    spot_balances: RwLock<HashMap<String, SpotBalance>>,
    spot_orders: RwLock<HashMap<String, SpotOrder>>,
    transfers: RwLock<Vec<Transfer>>,
    trading_symbols: RwLock<Option<HashSet<String>>>,
    spot_order_calls: RwLock<u64>,
    transfer_calls: RwLock<u64>,
    /// Protective stop algo orders placed via `place_stop_market_order`, keyed
    /// by algo id (ADR-0039). Removed on cancel so tests can assert
    /// placement and cancellation.
    stop_orders: RwLock<HashMap<String, StubStopOrder>>,
    /// Simulated real triggered order fills keyed by protective stop algo id.
    filled_stop_orders: RwLock<HashMap<String, OrderResult>>,
    /// Simulated income items retrievable by `get_income_since` (ADR-0045).
    income_records: RwLock<Vec<IncomeRecord>>,
    /// When set, the next reduce-only market order is rejected the way the
    /// exchange refuses to reduce an already-flat position (Binance -2022).
    reduce_only_rejections_next: RwLock<u32>,
    /// The next reduce-only market order executes only this much.
    short_fill_next: RwLock<Option<Decimal>>,
    /// The next protective stop cancel fails.
    cancel_fails_next: RwLock<bool>,
    /// Runs once, right after the next protective stop is accepted: whatever
    /// happened on the leg while the placement was on its way.
    after_next_stop_accepted: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    /// Runs once, right after the next stop cancel has taken effect.
    after_next_stop_cancel: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    /// Runs once, after the next open-orders read has what it will return.
    after_next_open_orders_read: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    /// Runs once, after the next positions read has what it will return.
    after_next_positions_read: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    /// Stops that triggered, whose market order is still working: no longer
    /// open and not filled yet.
    triggered_stop_orders: RwLock<HashMap<String, StubStopOrder>>,
    /// The stops `protection_fill` was asked about, in order.
    protection_fill_lookups: RwLock<Vec<String>>,
    /// Open-orders reads fail, and nothing else does.
    open_orders_fail: RwLock<bool>,
    /// The client id of every market order submitted, accepted or not.
    market_order_client_ids: RwLock<Vec<String>>,
    /// When set, the next market order or protective stop is authoritatively
    /// rejected by the simulated venue.
    reject_order_next: RwLock<bool>,
    refuse_before_send_next: RwLock<bool>,
    /// Futures market orders attempted via `place_market_order`, rejected or
    /// not, so tests can assert how many exit attempts reached the exchange.
    market_order_calls: RwLock<u64>,
    /// Simulated symbol trading rules served by `trading_rules` (issue
    /// #154). Unset symbols report rules as unavailable, preserving the
    /// legacy behavior of pre-metadata tests.
    trading_rules: RwLock<HashMap<String, robson_domain::SymbolTradingRules>>,
    /// The venue's own current rules, when they differ from what is cached
    /// (ADR-0056 §5): placements are judged by them, and a refresh brings
    /// them into the cache.
    venue_rules: RwLock<HashMap<String, robson_domain::SymbolTradingRules>>,
    /// The next placements refused with `ExecError::RulesRejected`.
    rules_rejections_next: RwLock<u32>,
    /// `trading_rules_fresh` fails: fresh metadata cannot be obtained.
    fresh_rules_unavailable: RwLock<bool>,
    /// Every metadata fetch (`trading_rules`, `trading_rules_fresh`,
    /// `refresh_trading_rules`) hangs.
    rules_fetch_hangs: RwLock<bool>,
    /// `refresh_trading_rules` fails.
    rules_refresh_fails: RwLock<bool>,
    /// What `rules_refresh_due` reports.
    rules_refresh_due: RwLock<bool>,
    /// `refresh_trading_rules` calls.
    rules_refresh_calls: RwLock<u64>,
    /// `place_protection` calls, accepted or not.
    protection_calls: RwLock<u64>,
    /// `close_position_market` calls, accepted or not.
    position_close_calls: RwLock<u64>,
    /// Per-symbol `trading_rules` calls so tests can enforce constant-count
    /// metadata resolution instead of per-position exchange I/O.
    trading_rules_calls: RwLock<HashMap<String, u64>>,
}

impl StubExchange {
    /// Create a new stub exchange with default price and balance.
    ///
    /// Default futures settings: position_mode="One-way",
    /// leverage=RiskConfig::LEVERAGE Default futures balance: 10,000 USDT.
    pub fn new(default_price: Decimal) -> Self {
        Self::with_history_capacity(default_price, STUB_HISTORY_CAPACITY)
    }

    pub(crate) fn with_history_capacity(default_price: Decimal, capacity: usize) -> Self {
        Self {
            venue_id: VenueId::new(STUB_VENUE_ID),
            has_funding: true,
            prices: RwLock::new(HashMap::new()),
            default_price,
            fee_rate: Decimal::new(1, 3), // 0.001 = 0.1%
            order_counter: RwLock::new(0),
            fail_next: RwLock::new(false),
            order_fail_next: RwLock::new(false),
            futures_settings: RwLock::new(("One-way".to_string(), RiskConfig::LEVERAGE)),
            open_positions: RwLock::new(HashMap::new()),
            futures_balance: RwLock::new(Decimal::from(10000)),
            unrealized_pnl: RwLock::new(Decimal::ZERO),
            client_orders: RwLock::new(BoundedHistory::new(capacity)),
            submit_attempts: SubmitAttemptLedger::new(capacity),
            cancelled_stop_orders: RwLock::new(BoundedHistory::new(capacity)),
            orders: RwLock::new(HashMap::new()),
            user_trades: RwLock::new(HashMap::new()),
            spot_balances: RwLock::new(HashMap::new()),
            spot_orders: RwLock::new(HashMap::new()),
            transfers: RwLock::new(Vec::new()),
            trading_symbols: RwLock::new(None),
            spot_order_calls: RwLock::new(0),
            transfer_calls: RwLock::new(0),
            stop_orders: RwLock::new(HashMap::new()),
            filled_stop_orders: RwLock::new(HashMap::new()),
            income_records: RwLock::new(Vec::new()),
            reduce_only_rejections_next: RwLock::new(0),
            short_fill_next: RwLock::new(None),
            cancel_fails_next: RwLock::new(false),
            after_next_stop_accepted: Mutex::new(None),
            after_next_stop_cancel: Mutex::new(None),
            after_next_open_orders_read: Mutex::new(None),
            after_next_positions_read: Mutex::new(None),
            protection_fill_lookups: RwLock::new(Vec::new()),
            triggered_stop_orders: RwLock::new(HashMap::new()),
            open_orders_fail: RwLock::new(false),
            market_order_client_ids: RwLock::new(Vec::new()),
            reject_order_next: RwLock::new(false),
            refuse_before_send_next: RwLock::new(false),
            market_order_calls: RwLock::new(0),
            trading_rules: RwLock::new(HashMap::new()),
            venue_rules: RwLock::new(HashMap::new()),
            rules_rejections_next: RwLock::new(0),
            fresh_rules_unavailable: RwLock::new(false),
            rules_fetch_hangs: RwLock::new(false),
            rules_refresh_fails: RwLock::new(false),
            rules_refresh_due: RwLock::new(false),
            rules_refresh_calls: RwLock::new(0),
            protection_calls: RwLock::new(0),
            position_close_calls: RwLock::new(0),
            trading_rules_calls: RwLock::new(HashMap::new()),
        }
    }

    /// Drop the optional funding surface: `funding()` is `None`, as on a venue
    /// without a spot→derivatives conversion route (ADR-0056 §3).
    pub fn without_funding(mut self) -> Self {
        self.has_funding = false;
        self
    }

    /// Install simulated trading rules for a symbol (issue #154).
    pub fn set_trading_rules(&self, rules: robson_domain::SymbolTradingRules) {
        self.trading_rules.write().unwrap().insert(rules.symbol().as_pair(), rules);
    }

    /// Number of simulated trading-rules resolutions for one symbol.
    pub fn set_venue_rules(&self, rules: robson_domain::SymbolTradingRules) {
        self.venue_rules.write().unwrap().insert(rules.symbol().as_pair(), rules);
    }

    /// The next market order fails inside the adapter, before any request: what
    /// a real adapter does on an entry with no fresh metadata.
    pub fn set_refuse_before_send_next(&self, refuse: bool) {
        *self.refuse_before_send_next.write().unwrap() = refuse;
    }

    /// Refuse the next `count` placements with `ExecError::RulesRejected`.
    pub fn set_rules_rejections_next(&self, count: u32) {
        *self.rules_rejections_next.write().unwrap() = count;
    }

    /// Make `trading_rules_fresh` fail while stale rules stay available.
    pub fn set_fresh_rules_unavailable(&self, unavailable: bool) {
        *self.fresh_rules_unavailable.write().unwrap() = unavailable;
    }

    /// Make every metadata fetch hang: a caller that waits for one never
    /// returns.
    pub fn set_rules_fetch_hangs(&self, hangs: bool) {
        *self.rules_fetch_hangs.write().unwrap() = hangs;
    }

    pub fn set_rules_refresh_fails(&self, fails: bool) {
        *self.rules_refresh_fails.write().unwrap() = fails;
    }

    pub fn set_rules_refresh_due(&self, due: bool) {
        *self.rules_refresh_due.write().unwrap() = due;
    }

    pub fn rules_refresh_call_count(&self) -> u64 {
        *self.rules_refresh_calls.read().unwrap()
    }

    pub fn protection_call_count(&self) -> u64 {
        *self.protection_calls.read().unwrap()
    }

    pub fn position_close_call_count(&self) -> u64 {
        *self.position_close_calls.read().unwrap()
    }

    /// Forget the cached rules, as a restart does.
    pub fn clear_cached_rules(&self) {
        self.trading_rules.write().unwrap().clear();
    }

    /// The rules rejection this placement gets, if any: an armed count first,
    /// then the venue's own lot rules.
    fn rules_rejection(&self, symbol: &Symbol, quantity: Quantity) -> Option<ExecError> {
        let mut armed = self.rules_rejections_next.write().unwrap();
        if *armed > 0 {
            *armed -= 1;
            return Some(ExecError::RulesRejected("Simulated rules rejection".to_string()));
        }
        let venue_rules = self.venue_rules.read().unwrap();
        let rules = venue_rules.get(&symbol.as_pair())?;
        rules
            .validate_order_qty(quantity.as_decimal())
            .err()
            .map(|error| ExecError::RulesRejected(error.to_string()))
    }

    pub fn trading_rules_call_count(&self, symbol: &str) -> u64 {
        self.trading_rules_calls.read().unwrap().get(symbol).copied().unwrap_or(0)
    }

    /// Create a stub exchange with a specific futures balance.
    pub fn with_balance(default_price: Decimal, balance: Decimal) -> Self {
        let exchange = Self::new(default_price);
        *exchange.futures_balance.write().unwrap() = balance;
        exchange
    }

    /// Set simulated futures settings (for testing failure scenarios).
    ///
    /// # Arguments
    ///
    /// * `position_mode` - Position mode (e.g., "One-way", "Hedge")
    /// * `leverage` - Leverage multiplier
    pub fn set_futures_settings(&self, position_mode: &str, leverage: u8) {
        let mut settings = self.futures_settings.write().unwrap();
        *settings = (position_mode.to_string(), leverage);
    }

    /// Set price for a specific symbol.
    pub fn set_price(&self, symbol: &str, price: Decimal) {
        let mut prices = self.prices.write().unwrap();
        prices.insert(symbol.to_string(), price);
    }

    pub fn set_trading_symbols(&self, symbols: &[&str]) {
        *self.trading_symbols.write().unwrap() =
            Some(symbols.iter().map(|symbol| (*symbol).to_string()).collect());
    }

    /// Get price for a symbol (or default).
    pub fn get_price_decimal(&self, symbol: &str) -> Decimal {
        let prices = self.prices.read().unwrap();
        prices.get(symbol).copied().unwrap_or(self.default_price)
    }

    /// Configure the next operation to fail. For a placement this simulates
    /// an indeterminate outcome: the attempt stays unresolved.
    pub fn set_fail_next(&self, fail: bool) {
        let mut fail_next = self.fail_next.write().unwrap();
        *fail_next = fail;
    }

    /// Configure the next futures order placement to fail after account
    /// validation, with an indeterminate outcome like [`Self::set_fail_next`].
    pub fn set_order_fail_next(&self, fail: bool) {
        let mut fail_next = self.order_fail_next.write().unwrap();
        *fail_next = fail;
    }

    /// Configure the next reduce-only market order to be rejected as Binance
    /// rejects reductions of an already-flat position (-2022).
    pub fn set_reject_reduce_only_next(&self, reject: bool) {
        self.set_reduce_only_rejections_next(u32::from(reject));
    }

    /// Refuse the next `count` reduce-only market orders as -2022.
    pub fn set_reduce_only_rejections_next(&self, count: u32) {
        *self.reduce_only_rejections_next.write().unwrap() = count;
    }

    /// The next reduce-only market order executes only `quantity`, as when
    /// the position turns out smaller than the order.
    pub fn set_short_fill_next(&self, quantity: Decimal) {
        *self.short_fill_next.write().unwrap() = Some(quantity);
    }

    /// The next protective stop cancel fails.
    pub fn set_cancel_fails_next(&self, fails: bool) {
        *self.cancel_fails_next.write().unwrap() = fails;
    }

    pub fn market_order_client_ids(&self) -> Vec<String> {
        self.market_order_client_ids.read().unwrap().clone()
    }

    /// Run `hook` once, right after the next protective stop is accepted.
    pub fn set_after_next_stop_accepted(&self, hook: impl FnOnce() + Send + 'static) {
        *self.after_next_stop_accepted.lock().unwrap() = Some(Box::new(hook));
    }

    /// Run `hook` once, right after the next stop cancel takes effect.
    pub fn set_after_next_stop_cancel(&self, hook: impl FnOnce() + Send + 'static) {
        *self.after_next_stop_cancel.lock().unwrap() = Some(Box::new(hook));
    }
    /// Run `hook` once, after the next open-orders read has what it will
    /// return.
    pub fn set_after_next_open_orders_read(&self, hook: impl FnOnce() + Send + 'static) {
        *self.after_next_open_orders_read.lock().unwrap() = Some(Box::new(hook));
    }

    /// Run `hook` once, after the next positions read has what it will return.
    pub fn set_after_next_positions_read(&self, hook: impl FnOnce() + Send + 'static) {
        *self.after_next_positions_read.lock().unwrap() = Some(Box::new(hook));
    }

    /// The stop triggered and its market order is working: it leaves the open
    /// orders, and `fill_stop_order_for` can still fill it.
    pub fn trigger_stop_order(&self, exchange_order_id: &str) {
        let order = self.stop_orders.write().unwrap().remove(exchange_order_id);
        if let Some(order) = order {
            self.triggered_stop_orders
                .write()
                .unwrap()
                .insert(exchange_order_id.to_string(), order);
        }
    }

    /// The venue no longer holds this symbol and side.
    pub fn clear_open_position(&self, symbol: &Symbol, side: robson_domain::Side) {
        let key = Self::position_key(symbol, side);
        self.open_positions.write().unwrap().remove(&key);
    }

    /// The stops `protection_fill` was asked about, in order.
    pub fn protection_fill_lookups(&self) -> Vec<String> {
        self.protection_fill_lookups.read().unwrap().clone()
    }

    /// Fail open-orders reads, and nothing else.
    pub fn set_open_orders_fail(&self, fail: bool) {
        *self.open_orders_fail.write().unwrap() = fail;
    }

    /// Configure the next market order or protective stop to be authoritatively
    /// rejected by the venue (the only failure that can lead to `Absent`).
    pub fn set_reject_order_next(&self, reject: bool) {
        *self.reject_order_next.write().unwrap() = reject;
    }

    /// Futures market orders attempted via `place_market_order` (including
    /// rejected ones).
    pub fn market_order_call_count(&self) -> u64 {
        *self.market_order_calls.read().unwrap()
    }

    /// Generate a unique order ID.
    fn next_order_id(&self) -> String {
        let mut counter = self.order_counter.write().unwrap();
        *counter += 1;
        format!("STUB-{}", *counter)
    }

    /// Check if we should fail the next operation.
    fn should_fail(&self) -> bool {
        let mut fail_next = self.fail_next.write().unwrap();
        let fail = *fail_next;
        *fail_next = false; // Reset after check
        fail
    }

    fn should_fail_order(&self) -> bool {
        let mut fail_next = self.order_fail_next.write().unwrap();
        let fail = *fail_next;
        *fail_next = false;
        fail
    }

    fn should_reject_order(&self) -> bool {
        let mut reject_next = self.reject_order_next.write().unwrap();
        let reject = *reject_next;
        *reject_next = false;
        reject
    }

    fn position_key(symbol: &Symbol, side: robson_domain::Side) -> String {
        format!("{}:{:?}", symbol.as_pair(), side)
    }

    /// Seed an exchange position for reconciliation tests.
    pub fn set_open_position(
        &self,
        symbol: Symbol,
        side: robson_domain::Side,
        quantity: Quantity,
        entry_price: Price,
    ) {
        let key = Self::position_key(&symbol, side);
        let mut positions = self.open_positions.write().unwrap();
        positions.insert(key, ExchangePosition { symbol, side, quantity, entry_price });
    }

    /// Number of currently simulated open positions.
    pub fn open_positions_len(&self) -> usize {
        self.open_positions.read().unwrap().len()
    }

    /// Set simulated futures account balance.
    pub fn set_futures_balance(&self, balance: Decimal) {
        *self.futures_balance.write().unwrap() = balance;
    }

    /// Set the simulated unrealized P&L (reported separately; the wallet
    /// balance is not affected).
    pub fn set_unrealized_pnl(&self, unrealized_pnl: Decimal) {
        *self.unrealized_pnl.write().unwrap() = unrealized_pnl;
    }

    /// Seed a simulated order for evidence-retrieval tests.
    pub fn set_order_result(&self, order_id: &str, result: OrderResult) {
        let mut orders = self.orders.write().unwrap();
        orders.insert(order_id.to_string(), result);
    }

    /// Seed simulated user trades for evidence-retrieval tests.
    pub fn set_user_trades(&self, symbol: &str, trades: Vec<UserTradeRecord>) {
        let mut user_trades = self.user_trades.write().unwrap();
        user_trades.insert(symbol.to_string(), trades);
    }

    /// Seed simulated income items for income-ledger tests (ADR-0045).
    pub fn set_income_records(&self, records: Vec<IncomeRecord>) {
        *self.income_records.write().unwrap() = records;
    }

    pub fn set_spot_balance(&self, asset: &str, free: Decimal, locked: Decimal) {
        self.spot_balances.write().unwrap().insert(asset.to_string(), SpotBalance {
            asset: asset.to_string(),
            free,
            locked,
        });
    }

    pub fn set_spot_order(&self, order: SpotOrder) {
        self.spot_orders.write().unwrap().insert(order.client_order_id.clone(), order);
    }

    pub fn set_transfer(&self, transfer: Transfer) {
        self.transfers.write().unwrap().push(transfer);
    }

    pub fn spot_order_call_count(&self) -> u64 {
        *self.spot_order_calls.read().unwrap()
    }

    pub fn transfer_call_count(&self) -> u64 {
        *self.transfer_calls.read().unwrap()
    }

    /// Number of protective stop orders currently live on the stub (ADR-0039).
    pub fn stop_order_count(&self) -> usize {
        self.stop_orders.read().unwrap().len()
    }

    /// Look up a protective stop order by its algo id (ADR-0039).
    pub fn find_stop_order(&self, exchange_order_id: &str) -> Option<StubStopOrder> {
        self.stop_orders.read().unwrap().get(exchange_order_id).cloned()
    }

    /// True if a protective stop order with this algo id is still live.
    pub fn stop_order(&self, exchange_order_id: &str) -> Option<StubStopOrder> {
        self.stop_orders.read().unwrap().get(exchange_order_id).cloned()
    }

    pub fn has_stop_order(&self, exchange_order_id: &str) -> bool {
        self.stop_orders.read().unwrap().contains_key(exchange_order_id)
    }

    /// Simulate a protective stop order being filled (ADR-0039).
    ///
    /// Removes the order from the live set (a filled `STOP_MARKET` is no longer
    /// open), records a real triggered-order fill for `get_stop_order_fill`,
    /// and returns the removed algo order for tests that need to inspect it.
    pub fn fill_stop_order(&self, exchange_order_id: &str) -> Option<StubStopOrder> {
        let quantity = self.stop_orders.read().unwrap().get(exchange_order_id)?.quantity;
        self.fill_stop_order_for(exchange_order_id, quantity)
    }

    /// [`Self::fill_stop_order`] executing only `filled_quantity`, as a
    /// reduce-only stop does when the position is smaller than the stop.
    pub fn fill_stop_order_for(
        &self,
        exchange_order_id: &str,
        filled_quantity: Quantity,
    ) -> Option<StubStopOrder> {
        self.fill_stop_order_at(exchange_order_id, filled_quantity, Utc::now())
    }

    /// [`Self::fill_stop_order_for`], with the fill at `filled_at`.
    pub fn fill_stop_order_at(
        &self,
        exchange_order_id: &str,
        filled_quantity: Quantity,
        filled_at: DateTime<Utc>,
    ) -> Option<StubStopOrder> {
        let open = self.stop_orders.write().unwrap().remove(exchange_order_id);
        let order =
            open.or_else(|| self.triggered_stop_orders.write().unwrap().remove(exchange_order_id))?;
        let fill_price = self.get_price_decimal(&order.symbol.as_pair());
        let fee = fill_price * filled_quantity.as_decimal() * self.fee_rate;
        let triggered_order_id = format!("TRIGGERED-{}", order.exchange_order_id);
        let result = OrderResult {
            exchange_order_id: triggered_order_id,
            client_order_id: order.client_order_id.clone(),
            fill_price: Price::new(fill_price).unwrap(),
            filled_quantity,
            fee,
            fee_asset: "USDT".to_string(),
            filled_at,
        };
        self.filled_stop_orders
            .write()
            .unwrap()
            .insert(exchange_order_id.to_string(), result);
        Some(order)
    }

    /// Snapshot of all currently live protective stop orders (ADR-0039).
    pub fn stop_orders_snapshot(&self) -> Vec<StubStopOrder> {
        self.stop_orders.read().unwrap().values().cloned().collect()
    }
}

#[async_trait]
impl venue::TradingPort for StubExchange {
    async fn trading_rules(
        &self,
        symbol: &Symbol,
    ) -> Result<robson_domain::SymbolTradingRules, ExecError> {
        let symbol_pair = symbol.as_pair();
        *self
            .trading_rules_calls
            .write()
            .unwrap()
            .entry(symbol_pair.clone())
            .or_default() += 1;
        if *self.rules_fetch_hangs.read().unwrap() {
            std::future::pending::<()>().await;
        }
        let fetched = self.venue_rules.read().unwrap().get(&symbol_pair).cloned();
        if let Some(rules) = fetched {
            self.trading_rules.write().unwrap().insert(symbol_pair.clone(), rules);
        }
        self.trading_rules.read().unwrap().get(&symbol_pair).cloned().ok_or_else(|| {
            ExecError::Exchange(format!("stub has no trading rules configured for {}", symbol_pair))
        })
    }

    async fn trading_rules_fresh(
        &self,
        symbol: &Symbol,
    ) -> Result<robson_domain::SymbolTradingRules, ExecError> {
        if *self.fresh_rules_unavailable.read().unwrap() {
            return Err(ExecError::Exchange(format!(
                "stub has no fresh trading rules for {}",
                symbol.as_pair()
            )));
        }
        venue::TradingPort::trading_rules(self, symbol).await
    }

    fn last_validated_rules(&self, symbol: &Symbol) -> Option<robson_domain::SymbolTradingRules> {
        self.trading_rules.read().unwrap().get(&symbol.as_pair()).cloned()
    }

    fn rules_refresh_due(&self, _symbol: &Symbol) -> bool {
        *self.rules_refresh_due.read().unwrap()
    }

    async fn refresh_trading_rules(
        &self,
        symbol: &Symbol,
    ) -> Result<robson_domain::SymbolTradingRules, ExecError> {
        *self.rules_refresh_calls.write().unwrap() += 1;
        if *self.rules_fetch_hangs.read().unwrap() {
            std::future::pending::<()>().await;
        }
        if *self.rules_refresh_fails.read().unwrap() {
            return Err(ExecError::Exchange("Simulated rules refresh failure".to_string()));
        }
        let symbol_pair = symbol.as_pair();
        let fetched = self.venue_rules.read().unwrap().get(&symbol_pair).cloned();
        if let Some(rules) = fetched {
            self.trading_rules.write().unwrap().insert(symbol_pair.clone(), rules);
        }
        self.trading_rules.read().unwrap().get(&symbol_pair).cloned().ok_or_else(|| {
            ExecError::Exchange(format!("stub has no trading rules configured for {}", symbol_pair))
        })
    }

    async fn place_market_order(
        &self,
        symbol: &Symbol,
        _side: OrderSide,
        quantity: Quantity,
        client_order_id: &str,
        intent: EntryOrReduce,
    ) -> Result<OrderResult, ExecError> {
        if std::mem::take(&mut *self.refuse_before_send_next.write().unwrap()) {
            return Err(ExecError::Exchange("Simulated refusal before the send".to_string()));
        }
        *self.market_order_calls.write().unwrap() += 1;
        self.market_order_client_ids.write().unwrap().push(client_order_id.to_string());
        let attempt = self.submit_attempts.begin(client_order_id);

        // Check if we should simulate a failure
        if self.should_fail() || self.should_fail_order() {
            return Err(ExecError::Exchange("Simulated exchange failure".to_string()));
        }

        if self.should_reject_order() {
            attempt.rejected();
            return Err(ExecError::OrderRejected("Simulated venue rejection".to_string()));
        }

        // A rules rejection leaves the attempt unresolved: it is not finality
        // evidence (see `ExecError::RulesRejected`).
        if let Some(error) = self.rules_rejection(symbol, quantity) {
            return Err(error);
        }

        // Simulate the exchange refusing to reduce an already-flat position,
        // with the same error shape production surfaces (Binance -2022).
        if intent.is_reduce() {
            let mut count = self.reduce_only_rejections_next.write().unwrap();
            if *count > 0 {
                *count -= 1;
                attempt.rejected();
                return Err(ExecError::ReduceOnlyRejected(
                    "Binance API error: -2022 - ReduceOnly Order is rejected.".to_string(),
                ));
            }
        }

        // A reduction can execute less than it asked for.
        let quantity = match self.short_fill_next.write().unwrap().take() {
            Some(short) if intent.is_reduce() => Quantity::new(short).unwrap(),
            _ => quantity,
        };

        // Get current price
        let price = self.get_price_decimal(&symbol.as_pair());

        // Calculate fee
        let notional = price * quantity.as_decimal();
        let fee = notional * self.fee_rate;

        // Generate order ID
        let exchange_order_id = self.next_order_id();

        let result = OrderResult {
            exchange_order_id,
            client_order_id: client_order_id.to_string(),
            fill_price: Price::new(price).unwrap(),
            filled_quantity: quantity,
            fee,
            fee_asset: "USDT".to_string(),
            filled_at: Utc::now(),
        };
        self.client_orders
            .write()
            .unwrap()
            .insert(client_order_id.to_string(), result.clone());
        Ok(result)
    }

    async fn place_protection(
        &self,
        symbol: &Symbol,
        side: OrderSide,
        quantity: Quantity,
        stop_price: Price,
        client_order_id: &str,
    ) -> Result<PlacedStop, ExecError> {
        *self.protection_calls.write().unwrap() += 1;
        let attempt = self.submit_attempts.begin(client_order_id);
        // Simulate placement failure when configured (mirrors market order path).
        if self.should_fail() || self.should_fail_order() {
            return Err(ExecError::Exchange("Simulated stop order failure".to_string()));
        }

        if self.should_reject_order() {
            attempt.rejected();
            return Err(ExecError::OrderRejected("Simulated venue rejection".to_string()));
        }

        if let Some(error) = self.rules_rejection(symbol, quantity) {
            return Err(error);
        }

        let exchange_order_id = self.next_order_id();
        let placed_at = Utc::now();

        // Record the accepted (unfilled) protective order for inspection.
        self.stop_orders
            .write()
            .unwrap()
            .insert(exchange_order_id.clone(), StubStopOrder {
                exchange_order_id: exchange_order_id.clone(),
                client_order_id: client_order_id.to_string(),
                symbol: symbol.clone(),
                side,
                quantity,
                stop_price,
                placed_at,
            });

        tracing::debug!(
            symbol = %symbol.as_pair(),
            ?side,
            exchange_order_id,
            stop_price = %stop_price.as_decimal(),
            "Stub: protective stop order placed (accepted, unfilled)"
        );

        if let Some(hook) = self.after_next_stop_accepted.lock().unwrap().take() {
            hook();
        }

        // Accepted but not filled: protective price recorded, zero fill/fee.
        Ok(PlacedStop {
            stop: StopRef::new(exchange_order_id.clone()),
            acceptance: OrderResult {
                exchange_order_id,
                client_order_id: client_order_id.to_string(),
                fill_price: stop_price,
                filled_quantity: Quantity::zero(),
                fee: Decimal::ZERO,
                fee_asset: "USDT".to_string(),
                filled_at: placed_at,
            },
        })
    }

    async fn cancel_order(&self, _symbol: &Symbol, order_id: &str) -> Result<(), ExecError> {
        if self.should_fail() {
            return Err(ExecError::Exchange("Simulated cancel failure".to_string()));
        }

        // Remove a protective stop if present (ADR-0039); harmless for other orders.
        if let Some(order) = self.stop_orders.write().unwrap().remove(order_id) {
            self.cancelled_stop_orders.write().unwrap().insert(order_id.to_string(), order);
        }

        // Stub: just log and return success
        tracing::debug!(order_id, "Stub: order cancelled");
        Ok(())
    }

    async fn cancel_protection(
        &self,
        _symbol: &Symbol,
        stop: &StopRef,
    ) -> Result<StopCancelOutcome, ExecError> {
        if self.should_fail() {
            return Err(ExecError::Exchange("Simulated stop cancel failure".to_string()));
        }
        if std::mem::take(&mut *self.cancel_fails_next.write().unwrap()) {
            return Err(ExecError::Timeout("Simulated stop cancel timeout".to_string()));
        }

        let algo_id = stop.exchange_order_id.as_str();
        let removed = self.stop_orders.write().unwrap().remove(algo_id);
        let existed = removed.is_some();
        if let Some(order) = removed {
            self.cancelled_stop_orders.write().unwrap().insert(algo_id.to_string(), order);
        }

        tracing::debug!(algo_id, existed, "Stub: stop algo order cancel");
        if let Some(hook) = self.after_next_stop_cancel.lock().unwrap().take() {
            hook();
        }
        Ok(if existed {
            StopCancelOutcome::Cancelled {
                filled_before_cancel: Decimal::ZERO,
                fills: None,
            }
        } else {
            // The stub knows a triggered stop's fill without any I/O.
            let final_state = match self.filled_stop_orders.read().unwrap().get(algo_id) {
                Some(fill) => OrderLookup::Terminal {
                    filled_qty: fill.filled_quantity.as_decimal(),
                    cancelled_qty: Decimal::ZERO,
                    fills: Some(fill.clone()),
                },
                None => OrderLookup::Unknown,
            };
            StopCancelOutcome::AlreadyGone { final_state }
        })
    }

    async fn replace_protection(
        &self,
        symbol: &Symbol,
        old: &StopRef,
        new: StopParams,
    ) -> Result<ReplaceOutcome, ExecError> {
        venue::replace_place_then_cancel(self, &self.submit_attempts, symbol, old, new).await
    }

    async fn order_by_client_id(
        &self,
        _symbol: &Symbol,
        client_order_id: &str,
    ) -> Result<OrderLookup, ExecError> {
        if self.should_fail() {
            return Err(ExecError::Exchange("Simulated order_by_client_id failure".to_string()));
        }

        let terminal = |fill: &OrderResult| OrderLookup::Terminal {
            filled_qty: fill.filled_quantity.as_decimal(),
            cancelled_qty: Decimal::ZERO,
            fills: Some(fill.clone()),
        };
        if let Some(fill) = self.client_orders.read().unwrap().get(client_order_id) {
            return Ok(terminal(fill));
        }
        if let Some(fill) = self
            .orders
            .read()
            .unwrap()
            .values()
            .chain(self.filled_stop_orders.read().unwrap().values())
            .find(|order| order.client_order_id == client_order_id)
        {
            return Ok(terminal(fill));
        }
        if self
            .stop_orders
            .read()
            .unwrap()
            .values()
            .any(|order| order.client_order_id == client_order_id)
        {
            return Ok(OrderLookup::Open { filled_qty: Decimal::ZERO });
        }
        if let Some(order) = self
            .cancelled_stop_orders
            .read()
            .unwrap()
            .values()
            .find(|order| order.client_order_id == client_order_id)
        {
            return Ok(OrderLookup::Terminal {
                filled_qty: Decimal::ZERO,
                cancelled_qty: order.quantity.as_decimal(),
                fills: None,
            });
        }
        Ok(STUB_FINALITY
            .classify_not_found(self.submit_attempts.final_evidence(client_order_id).await))
    }

    async fn open_protective_orders(
        &self,
        symbol: &Symbol,
    ) -> Result<Vec<OpenOrderRecord>, ExecError> {
        if self.should_fail() || *self.open_orders_fail.read().unwrap() {
            return Err(ExecError::Exchange("Simulated open-orders query failure".to_string()));
        }

        // Stub only tracks protective STOP_MARKET orders; report the live ones
        // for the requested symbol.
        let orders = self
            .stop_orders
            .read()
            .unwrap()
            .values()
            .filter(|order| order.symbol.as_pair() == symbol.as_pair())
            .cloned()
            .map(|order| OpenOrderRecord {
                exchange_order_id: order.exchange_order_id,
                client_order_id: order.client_order_id,
                order_type: "STOP_MARKET".to_string(),
                reduce_only: true,
                stop_price: Some(order.stop_price),
                side: order.side,
            })
            .collect();

        if let Some(hook) = self.after_next_open_orders_read.lock().unwrap().take() {
            hook();
        }
        Ok(orders)
    }

    async fn price(&self, symbol: &Symbol) -> Result<Price, ExecError> {
        if self.should_fail() {
            return Err(ExecError::Exchange("Simulated price fetch failure".to_string()));
        }

        let price = self.get_price_decimal(&symbol.as_pair());
        Ok(Price::new(price).unwrap())
    }

    async fn health(&self) -> Result<(), ExecError> {
        if self.should_fail() {
            return Err(ExecError::Exchange("Simulated health check failure".to_string()));
        }
        Ok(())
    }

    async fn order_by_exchange_id(
        &self,
        _symbol: &Symbol,
        order_id: &str,
    ) -> Result<Option<OrderResult>, ExecError> {
        if self.should_fail() {
            return Err(ExecError::Exchange(
                "Simulated get_order_by_exchange_id failure".to_string(),
            ));
        }

        let orders = self.orders.read().unwrap();
        Ok(orders.get(order_id).cloned())
    }

    async fn protection_fill(
        &self,
        _symbol: &Symbol,
        stop: &StopRef,
    ) -> Result<Option<OrderResult>, ExecError> {
        self.protection_fill_lookups
            .write()
            .unwrap()
            .push(stop.exchange_order_id.clone());
        if self.should_fail() {
            return Err(ExecError::Exchange("Simulated get_stop_order_fill failure".to_string()));
        }

        let algo_id = stop.exchange_order_id.as_str();

        if let Some(result) = self.filled_stop_orders.read().unwrap().get(algo_id).cloned() {
            return Ok(Some(result));
        }

        let orders = self.orders.read().unwrap();
        Ok(orders.get(algo_id).cloned())
    }

    async fn user_trades_since(
        &self,
        symbol: &Symbol,
        since: DateTime<Utc>,
        limit: u16,
    ) -> Result<Vec<UserTradeRecord>, ExecError> {
        if self.should_fail() {
            return Err(ExecError::Exchange("Simulated get_user_trades_since failure".to_string()));
        }

        let trades = self.user_trades.read().unwrap();
        let symbol_trades = trades.get(&symbol.as_pair()).cloned().unwrap_or_default();

        let mut filtered: Vec<UserTradeRecord> =
            symbol_trades.into_iter().filter(|t| t.filled_at >= since).collect();

        filtered.sort_by(|a, b| a.filled_at.cmp(&b.filled_at));
        filtered.truncate(limit as usize);

        Ok(filtered)
    }
}

#[async_trait]
impl venue::FundingPort for StubExchange {
    async fn spot_balances(&self) -> Result<Vec<SpotBalance>, ExecError> {
        if self.should_fail() {
            return Err(ExecError::Exchange("Simulated spot balance fetch failure".to_string()));
        }
        Ok(self.spot_balances.read().unwrap().values().cloned().collect())
    }

    async fn spot_price(&self, symbol: &str) -> Result<Price, ExecError> {
        if self.should_fail() {
            return Err(ExecError::Exchange("Simulated spot price fetch failure".to_string()));
        }
        Ok(Price::new(self.get_price_decimal(symbol)).unwrap())
    }

    async fn spot_symbol_is_trading(&self, symbol: &str) -> Result<bool, ExecError> {
        if self.should_fail() {
            return Err(ExecError::Exchange("Simulated symbol lookup failure".to_string()));
        }
        if let Some(symbols) = self.trading_symbols.read().unwrap().as_ref() {
            return Ok(symbols.contains(symbol));
        }
        Ok(self.prices.read().unwrap().contains_key(symbol))
    }

    async fn place_spot_market_order(
        &self,
        request: SpotOrderRequest,
    ) -> Result<SpotOrder, ExecError> {
        // Never resolves as rejected: lookups do not query the spot namespace.
        self.submit_attempts.begin(&request.client_order_id);
        if self.should_fail() {
            return Err(ExecError::Exchange("Simulated spot order failure".to_string()));
        }
        *self.spot_order_calls.write().unwrap() += 1;

        let price = self.get_price_decimal(&request.symbol);
        let base_qty = match request.quantity_kind {
            SpotOrderQuantity::Base => request.quantity,
            SpotOrderQuantity::Quote => request.quantity / price,
        };
        let quote_qty = match request.side {
            SpotOrderSide::Sell => base_qty * price,
            SpotOrderSide::Buy => request.quantity,
        };
        let fee = match request.side {
            SpotOrderSide::Sell => quote_qty * self.fee_rate,
            SpotOrderSide::Buy => base_qty * self.fee_rate,
        };
        let asset = request.symbol.trim_end_matches("USDT");

        if request.side == SpotOrderSide::Sell {
            let mut balances = self.spot_balances.write().unwrap();
            let balance = balances.entry(asset.to_string()).or_insert_with(|| SpotBalance {
                asset: asset.to_string(),
                free: Decimal::ZERO,
                locked: Decimal::ZERO,
            });
            balance.free = (balance.free - base_qty).max(Decimal::ZERO);
            let usdt = balances.entry("USDT".to_string()).or_insert_with(|| SpotBalance {
                asset: "USDT".to_string(),
                free: Decimal::ZERO,
                locked: Decimal::ZERO,
            });
            usdt.free += quote_qty - fee;
        } else if request.side == SpotOrderSide::Buy
            && request.quantity_kind == SpotOrderQuantity::Quote
        {
            let quote_asset = request.symbol.strip_prefix("USDT").ok_or_else(|| {
                ExecError::Exchange(format!(
                    "Stub spot BUY quote order requires an inverse USDT pair, got {}",
                    request.symbol
                ))
            })?;
            let mut balances = self.spot_balances.write().unwrap();
            let quote = balances.entry(quote_asset.to_string()).or_insert_with(|| SpotBalance {
                asset: quote_asset.to_string(),
                free: Decimal::ZERO,
                locked: Decimal::ZERO,
            });
            quote.free = (quote.free - request.quantity).max(Decimal::ZERO);
            let usdt = balances.entry("USDT".to_string()).or_insert_with(|| SpotBalance {
                asset: "USDT".to_string(),
                free: Decimal::ZERO,
                locked: Decimal::ZERO,
            });
            usdt.free += base_qty - fee;
        }

        let order = SpotOrder {
            symbol: request.symbol,
            exchange_order_id: self.next_order_id(),
            client_order_id: request.client_order_id.clone(),
            status: "FILLED".to_string(),
            executed_qty: base_qty,
            cummulative_quote_qty: quote_qty,
            fee,
            fee_asset: "USDT".to_string(),
            transact_time: Utc::now(),
        };
        self.spot_orders.write().unwrap().insert(request.client_order_id, order.clone());
        Ok(order)
    }

    async fn spot_order(
        &self,
        _symbol: &str,
        client_order_id: &str,
    ) -> Result<Option<SpotOrder>, ExecError> {
        if self.should_fail() {
            return Err(ExecError::Exchange("Simulated spot order query failure".to_string()));
        }
        Ok(self.spot_orders.read().unwrap().get(client_order_id).cloned())
    }

    async fn transfer(
        &self,
        asset: &str,
        amount: Decimal,
        transfer_type: UniversalTransferType,
        client_tran_key: &str,
    ) -> Result<TransferId, ExecError> {
        if self.should_fail() {
            return Err(ExecError::Exchange("Simulated universal transfer failure".to_string()));
        }
        *self.transfer_calls.write().unwrap() += 1;
        let transfer_id = TransferId(self.next_order_id());
        let transfer = Transfer {
            transfer_id: transfer_id.clone(),
            client_tran_key: Some(client_tran_key.to_string()),
            asset: asset.to_string(),
            amount,
            transfer_type,
            status: "CONFIRMED".to_string(),
            timestamp: Utc::now(),
        };
        self.transfers.write().unwrap().push(transfer);
        if asset == "USDT" {
            let mut balances = self.spot_balances.write().unwrap();
            if let Some(usdt) = balances.get_mut("USDT") {
                usdt.free = (usdt.free - amount).max(Decimal::ZERO);
            }
            *self.futures_balance.write().unwrap() += amount;
        }
        Ok(transfer_id)
    }

    async fn transfer_history(
        &self,
        transfer_type: UniversalTransferType,
        start_time: DateTime<Utc>,
    ) -> Result<Vec<Transfer>, ExecError> {
        if self.should_fail() {
            return Err(ExecError::Exchange("Simulated transfer history failure".to_string()));
        }
        Ok(self
            .transfers
            .read()
            .unwrap()
            .iter()
            .filter(|t| t.transfer_type == transfer_type && t.timestamp >= start_time)
            .cloned()
            .collect())
    }
}

#[async_trait]
impl venue::PositionAccountPort for StubExchange {
    async fn validate_position_settings(
        &self,
        symbol: &Symbol,
        expected_leverage: u8,
    ) -> Result<FuturesSettings, ExecError> {
        if self.should_fail() {
            return Err(ExecError::Exchange("Simulated futures check failure".to_string()));
        }

        let (position_mode, leverage) = {
            let settings = self.futures_settings.read().unwrap();
            settings.clone()
        };

        // Safety check: fail if not One-way mode
        if position_mode != "One-way" {
            return Err(ExecError::FuturesSafetyViolation {
                expected: "One-way position mode".to_string(),
                actual: format!("{} mode", position_mode),
                advice: "Switch to One-way position mode before trading".to_string(),
            });
        }

        // Safety check: fail if leverage doesn't match
        if leverage != expected_leverage {
            return Err(ExecError::FuturesSafetyViolation {
                expected: format!("{}x leverage", expected_leverage),
                actual: format!("{}x leverage", leverage),
                advice: format!("Set leverage to {}x before trading", expected_leverage),
            });
        }

        Ok(FuturesSettings {
            position_mode,
            leverage,
            symbol: symbol.as_pair(),
        })
    }

    async fn balance(&self) -> Result<FuturesBalance, ExecError> {
        if self.should_fail() {
            return Err(ExecError::Exchange("Simulated futures balance fetch failure".to_string()));
        }
        let balance = *self.futures_balance.read().unwrap();
        Ok(FuturesBalance {
            wallet_balance: balance,
            available_balance: balance,
            unrealized_pnl: *self.unrealized_pnl.read().unwrap(),
        })
    }

    async fn open_positions(&self) -> Result<Vec<ExchangePosition>, ExecError> {
        if self.should_fail() {
            return Err(ExecError::Exchange("Simulated reconciliation scan failure".to_string()));
        }

        let held: Vec<ExchangePosition> =
            self.open_positions.read().unwrap().values().cloned().collect();
        if let Some(hook) = self.after_next_positions_read.lock().unwrap().take() {
            hook();
        }
        Ok(held)
    }

    async fn close_position_market(
        &self,
        symbol: &Symbol,
        side: robson_domain::Side,
        quantity: Quantity,
        client_order_id: &str,
    ) -> Result<OrderResult, ExecError> {
        *self.position_close_calls.write().unwrap() += 1;
        let attempt = self.submit_attempts.begin(client_order_id);
        if self.should_fail() {
            return Err(ExecError::Exchange("Simulated reconciliation close failure".to_string()));
        }
        if let Some(error) = self.rules_rejection(symbol, quantity) {
            return Err(error);
        }

        // Takes what was submitted: what rounding left stays for the next scan.
        let key = Self::position_key(symbol, side);
        let held = {
            let mut positions = self.open_positions.write().unwrap();
            match positions.get_mut(&key) {
                Some(position) if position.quantity.as_decimal() > quantity.as_decimal() => {
                    let rest = position.quantity.as_decimal() - quantity.as_decimal();
                    position.quantity = Quantity::new(rest).unwrap();
                    true
                },
                Some(_) => positions.remove(&key).is_some(),
                None => false,
            }
        };

        if !held {
            attempt.rejected();
            return Err(ExecError::OrderRejected(format!(
                "No simulated open position found for {} {:?}",
                symbol.as_pair(),
                side
            )));
        }

        let price = self.get_price_decimal(&symbol.as_pair());
        let fee = price * quantity.as_decimal() * self.fee_rate;
        let exchange_order_id = self.next_order_id();

        let result = OrderResult {
            exchange_order_id,
            client_order_id: client_order_id.to_string(),
            fill_price: Price::new(price).unwrap(),
            filled_quantity: quantity,
            fee,
            fee_asset: "USDT".to_string(),
            filled_at: Utc::now(),
        };
        self.client_orders
            .write()
            .unwrap()
            .insert(client_order_id.to_string(), result.clone());
        Ok(result)
    }
}

#[async_trait]
impl venue::SettlementPort for StubExchange {
    async fn settlement_since(
        &self,
        since: DateTime<Utc>,
        limit: u16,
    ) -> Result<Vec<IncomeRecord>, ExecError> {
        if self.should_fail() {
            return Err(ExecError::Exchange("Simulated get_income_since failure".to_string()));
        }

        let records = self.income_records.read().unwrap();
        let mut filtered: Vec<IncomeRecord> =
            records.iter().filter(|r| r.income_time >= since).cloned().collect();

        filtered.sort_by(|a, b| a.income_time.cmp(&b.income_time));
        filtered.truncate(limit as usize);

        Ok(filtered)
    }
}

/// Venue id of the stub exchange.
pub const STUB_VENUE_ID: &str = "stub";

/// The stub applies the same rule as a real venue. Only a simulated rejection
/// (`set_reject_order_next`, `set_reject_reduce_only_next`, closing a flat
/// position) counts as authoritative; a simulated failure is indeterminate.
pub const STUB_FINALITY: VenueFinalityRule = VenueFinalityRule::EveryAttemptRejected;

#[async_trait]
impl venue::AccountPort for StubExchange {
    async fn cash(&self, asset: &str) -> Result<Cash, ExecError> {
        if self.should_fail() {
            return Err(ExecError::Exchange("Simulated cash fetch failure".to_string()));
        }
        if asset != "USDT" {
            return Err(ExecError::Exchange(format!("stub futures account holds no {asset}")));
        }
        Ok(Cash {
            free: *self.futures_balance.read().unwrap(),
            locked: Decimal::ZERO,
            as_of: Utc::now(),
        })
    }

    async fn valuation(&self, symbol: &Symbol) -> Result<Mark, ExecError> {
        let price = venue::TradingPort::price(self, symbol).await?;
        Ok(Mark { price, as_of: Utc::now() })
    }

    async fn execution_costs(&self, _symbol: &Symbol) -> Result<ExecutionCostEnvelope, ExecError> {
        let fee_bps = self.fee_rate * Decimal::from(10_000);
        Ok(ExecutionCostEnvelope {
            maker_bps: fee_bps,
            taker_bps: fee_bps,
            gap_bps: Decimal::ZERO,
            fee_asset_policy: FeeAssetPolicy::Quote,
            funding: Some(FundingTerms {
                interval: Duration::from_secs(8 * 60 * 60),
                rate_bps: Decimal::ZERO,
            }),
            as_of: Utc::now(),
            source: "stub: simulated fee rate".to_string(),
        })
    }

    fn capabilities(&self) -> VenueCapabilities {
        VenueCapabilities {
            market_kind: MarketKind::PerpUsdM,
            account_kind: AccountKind::PositionAccount,
            supports_short: true,
            reduce_only_wire_flag: true,
            protection: ProtectionKind::MarketOnTrigger,
            replacement: ReplacementProtocol::PlaceThenCancel,
            has_funding: self.has_funding,
            order_finality: STUB_FINALITY,
        }
    }
}

impl venue::Venue for StubExchange {
    fn attach_order_journal(&self, journal: std::sync::Arc<crate::order_journal::OrderJournal>) {
        self.submit_attempts.attach_journal(journal);
    }

    fn id(&self) -> &VenueId {
        &self.venue_id
    }

    fn position_account(&self) -> Option<&dyn venue::PositionAccountPort> {
        Some(self)
    }

    fn inventory(&self) -> Option<&dyn venue::InventoryPort> {
        None
    }

    fn settlement(&self) -> &dyn venue::SettlementPort {
        self
    }

    fn funding(&self) -> Option<&dyn venue::FundingPort> {
        if self.has_funding {
            Some(self)
        } else {
            None
        }
    }
}

// =============================================================================
// Stub Market Data
// =============================================================================

/// Stub market data provider for testing.
///
/// Allows manual price injection for testing scenarios.
pub struct StubMarketData {
    /// Current prices by symbol
    prices: RwLock<HashMap<String, Decimal>>,
    /// Active subscriptions (symbol -> sender)
    subscriptions: RwLock<HashMap<String, tokio::sync::mpsc::Sender<PriceUpdate>>>,
}

impl StubMarketData {
    /// Create a new stub market data provider.
    pub fn new() -> Self {
        Self {
            prices: RwLock::new(HashMap::new()),
            subscriptions: RwLock::new(HashMap::new()),
        }
    }

    /// Set price and notify subscribers.
    pub async fn set_price(&self, symbol: &Symbol, price: Decimal) {
        // Update stored price
        {
            let mut prices = self.prices.write().unwrap();
            prices.insert(symbol.as_pair(), price);
        }

        // Notify subscribers
        let subscriptions = self.subscriptions.read().unwrap();
        if let Some(sender) = subscriptions.get(&symbol.as_pair()) {
            let update = PriceUpdate {
                symbol: symbol.clone(),
                price: Price::new(price).unwrap(),
                timestamp: Utc::now(),
            };

            // Ignore send errors (subscriber may have dropped)
            let _ = sender.send(update).await;
        }
    }

    /// Inject a price update to all subscribers of a symbol.
    pub async fn inject_price_update(&self, update: PriceUpdate) {
        // Update stored price
        {
            let mut prices = self.prices.write().unwrap();
            prices.insert(update.symbol.as_pair(), update.price.as_decimal());
        }

        // Notify subscribers
        let subscriptions = self.subscriptions.read().unwrap();
        if let Some(sender) = subscriptions.get(&update.symbol.as_pair()) {
            let _ = sender.send(update).await;
        }
    }
}

impl Default for StubMarketData {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl MarketDataPort for StubMarketData {
    async fn subscribe(
        &self,
        symbol: &Symbol,
    ) -> Result<tokio::sync::mpsc::Receiver<PriceUpdate>, ExecError> {
        let (tx, rx) = tokio::sync::mpsc::channel(100);

        let mut subscriptions = self.subscriptions.write().map_err(|e| {
            ExecError::Exchange(format!("Failed to acquire subscription lock: {}", e))
        })?;

        subscriptions.insert(symbol.as_pair(), tx);

        Ok(rx)
    }

    async fn unsubscribe(&self, symbol: &Symbol) -> Result<(), ExecError> {
        let mut subscriptions = self.subscriptions.write().map_err(|e| {
            ExecError::Exchange(format!("Failed to acquire subscription lock: {}", e))
        })?;

        subscriptions.remove(&symbol.as_pair());
        Ok(())
    }

    async fn get_price(&self, symbol: &Symbol) -> Result<Price, ExecError> {
        let prices = self
            .prices
            .read()
            .map_err(|e| ExecError::Exchange(format!("Failed to acquire price lock: {}", e)))?;

        let price = prices
            .get(&symbol.as_pair())
            .copied()
            .ok_or_else(|| ExecError::Exchange(format!("No price for {}", symbol.as_pair())))?;

        Ok(Price::new(price).unwrap())
    }
}

// =============================================================================
// Stub Trade Feed
// =============================================================================

/// Stream base reported by [`StubTradeFeed`].
pub const STUB_FEED_ENDPOINT: &str = "stub://trades";

/// In-memory trade feed: no network, ticks arrive through [`Self::push`].
pub struct StubTradeFeed {
    endpoints: Vec<String>,
    trades: tokio::sync::broadcast::Sender<(Symbol, Price)>,
}

impl StubTradeFeed {
    /// Create a feed with no connected streams.
    pub fn new() -> Self {
        let (trades, _) = tokio::sync::broadcast::channel(256);
        Self {
            endpoints: vec![STUB_FEED_ENDPOINT.to_string()],
            trades,
        }
    }

    /// Deliver a trade to every connected stream of `symbol`. Returns the
    /// number of connected streams of any symbol.
    pub fn push(&self, symbol: &Symbol, price: Price) -> usize {
        self.trades.send((symbol.clone(), price)).unwrap_or(0)
    }
}

impl Default for StubTradeFeed {
    fn default() -> Self {
        Self::new()
    }
}

struct StubTradeStream {
    symbol: Symbol,
    trades: tokio::sync::broadcast::Receiver<(Symbol, Price)>,
}

#[async_trait]
impl TradeStream for StubTradeStream {
    async fn next_message(&mut self) -> Option<Result<TradeFeedMessage, String>> {
        loop {
            match self.trades.recv().await {
                Ok((symbol, price)) if symbol == self.symbol => {
                    return Some(Ok(TradeFeedMessage::Trade { symbol, price }));
                },
                Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {},
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return None,
            }
        }
    }
}

#[async_trait]
impl TradeFeedPort for StubTradeFeed {
    fn endpoints(&self) -> &[String] {
        &self.endpoints
    }

    async fn connect(
        &self,
        _endpoint: &str,
        symbol: &Symbol,
    ) -> Result<Box<dyn TradeStream>, String> {
        Ok(Box::new(StubTradeStream {
            symbol: symbol.clone(),
            trades: self.trades.subscribe(),
        }))
    }
}

// =============================================================================
// Stub OHLCV
// =============================================================================

/// Stub OHLCV provider for tests and in-memory daemon mode.
#[derive(Clone)]
pub struct StubOhlcv {
    candles: Vec<Candle>,
}

impl StubOhlcv {
    /// Create a stub from a fixed candle sequence.
    pub fn new(candles: Vec<Candle>) -> Self {
        Self { candles }
    }

    /// Create a 100-candle fixture with two supports and two resistances.
    pub fn with_default_technical_levels() -> Self {
        let symbol = Symbol::from_pair("BTCUSDT").expect("static test symbol must be valid");
        Self::new(default_technical_stop_candles(symbol))
    }
}

impl Default for StubOhlcv {
    fn default() -> Self {
        Self::with_default_technical_levels()
    }
}

#[async_trait]
impl OhlcvPort for StubOhlcv {
    async fn fetch_candles(
        &self,
        _symbol: &Symbol,
        _interval: CandleInterval,
        _limit: u16,
    ) -> Result<Vec<Candle>, ExecError> {
        Ok(self.candles.clone())
    }
}

fn default_technical_stop_candles(symbol: Symbol) -> Vec<Candle> {
    let base = Decimal::from(95_000u32);
    let now = Utc::now();
    let mut candles: Vec<Candle> = (0..100)
        .map(|_| {
            Candle::new(symbol.clone(), base, base, base, base, Decimal::from(100u32), 10, now, now)
        })
        .collect();

    candles[50] = Candle::new(
        symbol.clone(),
        base,
        Decimal::from(97_000u32),
        Decimal::from(93_000u32),
        base,
        Decimal::from(100u32),
        10,
        now,
        now,
    );
    candles[70] = Candle::new(
        symbol,
        base,
        Decimal::from(100_000u32),
        Decimal::from(90_000u32),
        base,
        Decimal::from(100u32),
        10,
        now,
        now,
    );

    candles
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use rust_decimal_macros::dec;

    use super::*;
    use crate::ports::ExchangePort;

    #[tokio::test]
    async fn test_stub_exchange_place_order() {
        let exchange = StubExchange::new(dec!(95000));
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();

        let result = exchange
            .place_market_order(
                &symbol,
                OrderSide::Buy,
                Quantity::new(dec!(0.1)).unwrap(),
                "test-1",
                false,
            )
            .await
            .unwrap();

        assert_eq!(result.fill_price.as_decimal(), dec!(95000));
        assert_eq!(result.filled_quantity.as_decimal(), dec!(0.1));
        assert!(result.fee > Decimal::ZERO);
        assert_eq!(result.client_order_id, "test-1");
    }

    #[tokio::test]
    async fn test_stub_exchange_custom_price() {
        let exchange = StubExchange::new(dec!(95000));
        exchange.set_price("ETHUSDT", dec!(3000));

        let eth = Symbol::from_pair("ETHUSDT").unwrap();
        let btc = Symbol::from_pair("BTCUSDT").unwrap();

        let eth_result = exchange
            .place_market_order(
                &eth,
                OrderSide::Buy,
                Quantity::new(dec!(1.0)).unwrap(),
                "eth-1",
                false,
            )
            .await
            .unwrap();

        let btc_result = exchange
            .place_market_order(
                &btc,
                OrderSide::Buy,
                Quantity::new(dec!(0.1)).unwrap(),
                "btc-1",
                false,
            )
            .await
            .unwrap();

        assert_eq!(eth_result.fill_price.as_decimal(), dec!(3000));
        assert_eq!(btc_result.fill_price.as_decimal(), dec!(95000)); // Default
    }

    #[tokio::test]
    async fn test_stub_exchange_simulated_failure() {
        let exchange = StubExchange::new(dec!(95000));
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();

        // Configure failure
        exchange.set_fail_next(true);

        let result = exchange
            .place_market_order(
                &symbol,
                OrderSide::Buy,
                Quantity::new(dec!(0.1)).unwrap(),
                "fail-1",
                false,
            )
            .await;

        assert!(result.is_err());

        // Next call should succeed
        let result = exchange
            .place_market_order(
                &symbol,
                OrderSide::Buy,
                Quantity::new(dec!(0.1)).unwrap(),
                "ok-1",
                false,
            )
            .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_stub_market_data_subscription() {
        let market_data = StubMarketData::new();
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();

        let mut rx = market_data.subscribe(&symbol).await.unwrap();

        // Inject price update
        market_data.set_price(&symbol, dec!(96000)).await;

        // Should receive the update
        let update = rx.recv().await.unwrap();
        assert_eq!(update.price.as_decimal(), dec!(96000));
        assert_eq!(update.symbol.as_pair(), "BTCUSDT");
    }

    #[tokio::test]
    async fn test_stub_market_data_get_price() {
        let market_data = StubMarketData::new();
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();

        // Set price
        market_data.set_price(&symbol, dec!(95000)).await;

        // Get price
        let price = market_data.get_price(&symbol).await.unwrap();
        assert_eq!(price.as_decimal(), dec!(95000));
    }

    #[tokio::test]
    async fn test_stub_market_data_unknown_symbol() {
        let market_data = StubMarketData::new();
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();

        // Should fail for unknown symbol
        let result = market_data.get_price(&symbol).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_stub_exchange_get_order_by_exchange_id() {
        let exchange = StubExchange::new(dec!(95000));
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();

        let order_result = OrderResult {
            exchange_order_id: "12345".to_string(),
            client_order_id: "test-coid".to_string(),
            fill_price: Price::new(dec!(95000)).unwrap(),
            filled_quantity: Quantity::new(dec!(0.1)).unwrap(),
            fee: dec!(9.5),
            fee_asset: "USDT".to_string(),
            filled_at: Utc::now(),
        };

        exchange.set_order_result("12345", order_result.clone());

        let found = exchange.get_order_by_exchange_id(&symbol, "12345").await.unwrap();
        assert!(found.is_some());
        let found = found.unwrap();
        assert_eq!(found.exchange_order_id, "12345");
        assert_eq!(found.fill_price.as_decimal(), dec!(95000));

        let missing = exchange.get_order_by_exchange_id(&symbol, "99999").await.unwrap();
        assert!(missing.is_none());
    }

    #[tokio::test]
    async fn test_stub_exchange_get_user_trades_since_filters_sorts_and_limits() {
        let exchange = StubExchange::new(dec!(95000));
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let now = Utc::now();

        let trades = vec![
            UserTradeRecord {
                exchange_order_id: "100".to_string(),
                exchange_trade_id: "1".to_string(),
                fill_price: Price::new(dec!(94000)).unwrap(),
                filled_quantity: Quantity::new(dec!(0.05)).unwrap(),
                fee: dec!(4.7),
                fee_asset: "USDT".to_string(),
                filled_at: now - chrono::Duration::minutes(10),
            },
            UserTradeRecord {
                exchange_order_id: "102".to_string(),
                exchange_trade_id: "3".to_string(),
                fill_price: Price::new(dec!(96000)).unwrap(),
                filled_quantity: Quantity::new(dec!(0.05)).unwrap(),
                fee: dec!(4.8),
                fee_asset: "USDT".to_string(),
                filled_at: now + chrono::Duration::minutes(1),
            },
            UserTradeRecord {
                exchange_order_id: "101".to_string(),
                exchange_trade_id: "2".to_string(),
                fill_price: Price::new(dec!(95000)).unwrap(),
                filled_quantity: Quantity::new(dec!(0.05)).unwrap(),
                fee: dec!(4.75),
                fee_asset: "USDT".to_string(),
                filled_at: now - chrono::Duration::minutes(5),
            },
        ];

        exchange.set_user_trades("BTCUSDT", trades);

        let since = now - chrono::Duration::minutes(7);
        let filtered = exchange.get_user_trades_since(&symbol, since, 1).await.unwrap();

        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].exchange_trade_id, "2");
    }

    #[tokio::test]
    async fn test_stub_exchange_get_user_trades_since_empty() {
        let exchange = StubExchange::new(dec!(95000));
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();

        let trades = exchange.get_user_trades_since(&symbol, Utc::now(), 100).await.unwrap();
        assert!(trades.is_empty());
    }

    #[tokio::test]
    async fn test_stub_spot_order_and_transfer_are_queryable_for_idempotency() {
        let exchange = StubExchange::new(dec!(100));
        exchange.set_price("ABCUSDT", dec!(10));
        exchange.set_spot_balance("ABC", dec!(5), Decimal::ZERO);
        exchange.set_spot_balance("USDT", dec!(1), Decimal::ZERO);

        let request = SpotOrderRequest {
            symbol: "ABCUSDT".to_string(),
            side: SpotOrderSide::Sell,
            quantity_kind: SpotOrderQuantity::Base,
            quantity: dec!(5),
            client_order_id: "spot-1".to_string(),
        };
        let order = exchange.place_spot_market_order(request).await.unwrap();
        assert_eq!(order.status, "FILLED");
        assert_eq!(exchange.spot_order_call_count(), 1);

        let found = exchange.get_spot_order("ABCUSDT", "spot-1").await.unwrap();
        assert!(found.is_some());

        let transfer_id = exchange
            .universal_transfer("USDT", dec!(10), UniversalTransferType::MainUmfuture, "transfer-1")
            .await
            .unwrap();
        assert!(!transfer_id.0.is_empty());
        assert_eq!(exchange.transfer_call_count(), 1);

        let history = exchange
            .get_transfer_history(
                UniversalTransferType::MainUmfuture,
                Utc::now() - chrono::Duration::minutes(1),
            )
            .await
            .unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].client_tran_key.as_deref(), Some("transfer-1"));
    }

    mod finality {
        use super::*;
        use crate::venue::{AttemptEvidence, PositionAccountPort, TradingPort};

        fn btc() -> Symbol {
            Symbol::from_pair("BTCUSDT").unwrap()
        }

        fn qty() -> Quantity {
            Quantity::new(dec!(0.01)).unwrap()
        }

        async fn market(s: &StubExchange, id: &str) -> Result<OrderResult, ExecError> {
            let symbol = btc();
            let order = TradingPort::place_market_order(
                s,
                &symbol,
                OrderSide::Buy,
                qty(),
                id,
                EntryOrReduce::Entry,
            );
            order.await
        }

        async fn stop(s: &StubExchange, id: &str) -> Result<PlacedStop, ExecError> {
            let price = Price::new(dec!(90000)).unwrap();
            s.place_protection(&btc(), OrderSide::Sell, qty(), price, id).await
        }

        async fn lookup(s: &StubExchange, id: &str) -> OrderLookup {
            TradingPort::order_by_client_id(s, &btc(), id).await.unwrap()
        }

        async fn is_unknown(s: &StubExchange, id: &str) -> bool {
            matches!(lookup(s, id).await, OrderLookup::Unknown)
        }

        async fn is_absent(s: &StubExchange, id: &str) -> bool {
            matches!(lookup(s, id).await, OrderLookup::Absent { finality } if finality == STUB_FINALITY)
        }

        #[tokio::test]
        async fn only_authoritative_rejections_lead_to_absent() {
            let s = StubExchange::new(dec!(100));
            assert!(is_unknown(&s, "never").await);

            s.set_reject_order_next(true);
            assert!(stop(&s, "refused-stop").await.is_err());
            assert!(is_absent(&s, "refused-stop").await);
            s.set_reject_order_next(true);
            assert!(market(&s, "refused-market").await.is_err());
            assert!(is_absent(&s, "refused-market").await);

            s.set_reject_reduce_only_next(true);
            let symbol = btc();
            let reduce = TradingPort::place_market_order(
                &s,
                &symbol,
                OrderSide::Sell,
                qty(),
                "refused-reduce",
                EntryOrReduce::Reduce,
            );
            assert!(reduce.await.is_err());
            assert!(is_absent(&s, "refused-reduce").await);

            let flat = PositionAccountPort::close_position_market(
                &s,
                &symbol,
                robson_domain::Side::Long,
                qty(),
                "refused-close",
            );
            assert!(flat.await.is_err());
            assert!(is_absent(&s, "refused-close").await);

            // A simulated failure is indeterminate, also after a rejected retry.
            s.set_order_fail_next(true);
            assert!(stop(&s, "failed").await.is_err());
            assert!(is_unknown(&s, "failed").await);
            s.set_reject_order_next(true);
            assert!(stop(&s, "failed").await.is_err());
            assert!(is_unknown(&s, "failed").await);
        }

        #[tokio::test]
        async fn truncated_evidence_keeps_a_rejected_retry_unknown() {
            let s = StubExchange::with_history_capacity(dec!(100), 2);
            s.set_order_fail_next(true);
            assert!(stop(&s, "indeterminate").await.is_err());
            s.set_reject_order_next(true);
            assert!(stop(&s, "refused").await.is_err());
            assert!(is_absent(&s, "refused").await);

            // The third client id exceeds capacity.
            market(&s, "overflow").await.unwrap();
            assert_eq!(s.submit_attempts.evidence("refused"), AttemptEvidence::Truncated);
            assert!(is_unknown(&s, "refused").await);

            // A fresh record for this retry would read `AllRejected`.
            s.set_reject_order_next(true);
            assert!(stop(&s, "indeterminate").await.is_err());
            assert_eq!(s.submit_attempts.evidence("indeterminate"), AttemptEvidence::Truncated);
            assert!(is_unknown(&s, "indeterminate").await);
            // Unrelated to the evidence, a retained record still answers.
            assert!(matches!(lookup(&s, "overflow").await, OrderLookup::Terminal { .. }));
        }

        #[tokio::test]
        async fn evicted_client_order_reads_unknown() {
            let mut s = StubExchange::with_history_capacity(dec!(100), 2);
            // Evidence kept, to prove the eviction alone cannot yield `Absent`.
            s.submit_attempts = SubmitAttemptLedger::new(64);

            // Rejected, then accepted: the evicted record leaves mixed evidence.
            s.set_reject_order_next(true);
            assert!(market(&s, "c0").await.is_err());
            market(&s, "c0").await.unwrap();
            for id in ["c1", "c2", "c3"] {
                market(&s, id).await.unwrap();
                assert!(s.client_orders.read().unwrap().entries.len() <= 2);
                assert!(s.client_orders.read().unwrap().order.len() <= 2);
            }

            assert_eq!(s.submit_attempts.evidence("c0"), AttemptEvidence::Unresolved {
                unresolved: 1,
                rejected: 1
            });
            assert!(is_unknown(&s, "c0").await);
            assert!(is_unknown(&s, "c1").await);
            assert!(matches!(lookup(&s, "c2").await, OrderLookup::Terminal { .. }));
            assert!(matches!(lookup(&s, "c3").await, OrderLookup::Terminal { .. }));
        }

        #[tokio::test]
        async fn evicted_cancelled_stop_reads_unknown() {
            let mut s = StubExchange::with_history_capacity(dec!(100), 2);
            s.submit_attempts = SubmitAttemptLedger::new(64);

            for id in ["s0", "s1", "s2"] {
                let placed = stop(&s, id).await.unwrap();
                s.cancel_protection(&btc(), &placed.stop).await.unwrap();
                assert!(s.cancelled_stop_orders.read().unwrap().entries.len() <= 2);
            }
            let legacy = stop(&s, "s3").await.unwrap();
            TradingPort::cancel_order(&s, &btc(), &legacy.stop.exchange_order_id)
                .await
                .unwrap();
            assert_eq!(s.cancelled_stop_orders.read().unwrap().entries.len(), 2);

            assert!(is_unknown(&s, "s0").await);
            assert!(is_unknown(&s, "s1").await);
            assert!(matches!(lookup(&s, "s2").await, OrderLookup::Terminal { .. }));
            assert!(matches!(lookup(&s, "s3").await, OrderLookup::Terminal { .. }));
        }

        #[test]
        fn history_keeps_insertion_order_when_a_key_repeats() {
            let mut history = BoundedHistory::new(2);
            history.insert("a".to_string(), 1);
            history.insert("b".to_string(), 2);
            history.insert("a".to_string(), 3);
            assert_eq!(history.get("a"), Some(&3));
            history.insert("c".to_string(), 4);
            assert_eq!(history.get("a"), None);
            assert_eq!(history.get("b"), Some(&2));
            assert_eq!(history.get("c"), Some(&4));
            assert_eq!(history.order.len(), 2);
        }
    }
}
