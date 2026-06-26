//! Stub implementations for testing.
//!
//! These implementations simulate exchange and market data behavior
//! without making real API calls.

use std::{
    collections::{HashMap, HashSet},
    sync::RwLock,
};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use robson_domain::{Candle, OrderSide, Price, Quantity, RiskConfig, Symbol};
use rust_decimal::Decimal;

use crate::{
    error::ExecError,
    ports::{
        CandleInterval, ExchangePort, ExchangePosition, FuturesBalance, FuturesSettings,
        MarketDataPort, OhlcvPort, OrderResult, PriceUpdate, SpotBalance, SpotOrder,
        SpotOrderQuantity, SpotOrderRequest, SpotOrderSide, Transfer, TransferId,
        UniversalTransferType, UserTradeRecord,
    },
};

// =============================================================================
// Stub Exchange
// =============================================================================

/// Stub exchange for testing.
///
/// Simulates immediate fills at a configured price.
/// Default configuration: One-way position mode, 1x leverage.
pub struct StubExchange {
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
    /// Simulated orders retrievable by `get_order_by_exchange_id`.
    orders: RwLock<HashMap<String, OrderResult>>,
    /// Simulated user trades retrievable by `get_user_trades_since`.
    user_trades: RwLock<HashMap<String, Vec<UserTradeRecord>>>,
    spot_balances: RwLock<HashMap<String, SpotBalance>>,
    spot_orders: RwLock<HashMap<String, SpotOrder>>,
    transfers: RwLock<Vec<Transfer>>,
    trading_symbols: RwLock<Option<HashSet<String>>>,
    spot_order_calls: RwLock<u64>,
    transfer_calls: RwLock<u64>,
}

impl StubExchange {
    /// Create a new stub exchange with default price and balance.
    ///
    /// Default futures settings: position_mode="One-way", leverage=RiskConfig::LEVERAGE
    /// Default futures balance: 10,000 USDT.
    pub fn new(default_price: Decimal) -> Self {
        Self {
            prices: RwLock::new(HashMap::new()),
            default_price,
            fee_rate: Decimal::new(1, 3), // 0.001 = 0.1%
            order_counter: RwLock::new(0),
            fail_next: RwLock::new(false),
            order_fail_next: RwLock::new(false),
            futures_settings: RwLock::new(("One-way".to_string(), RiskConfig::LEVERAGE)),
            open_positions: RwLock::new(HashMap::new()),
            futures_balance: RwLock::new(Decimal::from(10000)),
            orders: RwLock::new(HashMap::new()),
            user_trades: RwLock::new(HashMap::new()),
            spot_balances: RwLock::new(HashMap::new()),
            spot_orders: RwLock::new(HashMap::new()),
            transfers: RwLock::new(Vec::new()),
            trading_symbols: RwLock::new(None),
            spot_order_calls: RwLock::new(0),
            transfer_calls: RwLock::new(0),
        }
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

    /// Configure the next operation to fail.
    pub fn set_fail_next(&self, fail: bool) {
        let mut fail_next = self.fail_next.write().unwrap();
        *fail_next = fail;
    }

    /// Configure the next futures order placement to fail after account
    /// validation.
    pub fn set_order_fail_next(&self, fail: bool) {
        let mut fail_next = self.order_fail_next.write().unwrap();
        *fail_next = fail;
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

    pub fn set_spot_balance(&self, asset: &str, free: Decimal, locked: Decimal) {
        self.spot_balances
            .write()
            .unwrap()
            .insert(asset.to_string(), SpotBalance { asset: asset.to_string(), free, locked });
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
}

#[async_trait]
impl ExchangePort for StubExchange {
    async fn validate_futures_settings(
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

    async fn place_market_order(
        &self,
        symbol: &Symbol,
        _side: OrderSide,
        quantity: Quantity,
        client_order_id: &str,
        _reduce_only: bool,
    ) -> Result<OrderResult, ExecError> {
        // Check if we should simulate a failure
        if self.should_fail() || self.should_fail_order() {
            return Err(ExecError::Exchange("Simulated exchange failure".to_string()));
        }

        // Get current price
        let price = self.get_price_decimal(&symbol.as_pair());

        // Calculate fee
        let notional = price * quantity.as_decimal();
        let fee = notional * self.fee_rate;

        // Generate order ID
        let exchange_order_id = self.next_order_id();

        Ok(OrderResult {
            exchange_order_id,
            client_order_id: client_order_id.to_string(),
            fill_price: Price::new(price).unwrap(),
            filled_quantity: quantity,
            fee,
            fee_asset: "USDT".to_string(),
            filled_at: Utc::now(),
        })
    }

    async fn cancel_order(&self, _symbol: &Symbol, order_id: &str) -> Result<(), ExecError> {
        if self.should_fail() {
            return Err(ExecError::Exchange("Simulated cancel failure".to_string()));
        }

        // Stub: just log and return success
        tracing::debug!(order_id, "Stub: order cancelled");
        Ok(())
    }

    async fn get_price(&self, symbol: &Symbol) -> Result<Price, ExecError> {
        if self.should_fail() {
            return Err(ExecError::Exchange("Simulated price fetch failure".to_string()));
        }

        let price = self.get_price_decimal(&symbol.as_pair());
        Ok(Price::new(price).unwrap())
    }

    async fn health_check(&self) -> Result<(), ExecError> {
        if self.should_fail() {
            return Err(ExecError::Exchange("Simulated health check failure".to_string()));
        }
        Ok(())
    }

    async fn get_futures_balance(&self) -> Result<FuturesBalance, ExecError> {
        if self.should_fail() {
            return Err(ExecError::Exchange("Simulated futures balance fetch failure".to_string()));
        }
        let balance = *self.futures_balance.read().unwrap();
        Ok(FuturesBalance {
            wallet_balance: balance,
            available_balance: balance,
        })
    }

    async fn get_spot_account_balances(&self) -> Result<Vec<SpotBalance>, ExecError> {
        if self.should_fail() {
            return Err(ExecError::Exchange("Simulated spot balance fetch failure".to_string()));
        }
        Ok(self.spot_balances.read().unwrap().values().cloned().collect())
    }

    async fn get_spot_price(&self, symbol: &str) -> Result<Price, ExecError> {
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

    async fn get_spot_order(
        &self,
        _symbol: &str,
        client_order_id: &str,
    ) -> Result<Option<SpotOrder>, ExecError> {
        if self.should_fail() {
            return Err(ExecError::Exchange("Simulated spot order query failure".to_string()));
        }
        Ok(self.spot_orders.read().unwrap().get(client_order_id).cloned())
    }

    async fn universal_transfer(
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

    async fn get_transfer_history(
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

    async fn get_all_open_positions(&self) -> Result<Vec<ExchangePosition>, ExecError> {
        if self.should_fail() {
            return Err(ExecError::Exchange("Simulated reconciliation scan failure".to_string()));
        }

        let positions = self.open_positions.read().unwrap();
        Ok(positions.values().cloned().collect())
    }

    async fn close_position_market(
        &self,
        symbol: &Symbol,
        side: robson_domain::Side,
        quantity: Quantity,
        client_order_id: &str,
    ) -> Result<OrderResult, ExecError> {
        if self.should_fail() {
            return Err(ExecError::Exchange("Simulated reconciliation close failure".to_string()));
        }

        let key = Self::position_key(symbol, side);
        let removed = {
            let mut positions = self.open_positions.write().unwrap();
            positions.remove(&key)
        };

        if removed.is_none() {
            return Err(ExecError::OrderRejected(format!(
                "No simulated open position found for {} {:?}",
                symbol.as_pair(),
                side
            )));
        }

        let price = self.get_price_decimal(&symbol.as_pair());
        let fee = price * quantity.as_decimal() * self.fee_rate;
        let exchange_order_id = self.next_order_id();

        Ok(OrderResult {
            exchange_order_id,
            client_order_id: client_order_id.to_string(),
            fill_price: Price::new(price).unwrap(),
            filled_quantity: quantity,
            fee,
            fee_asset: "USDT".to_string(),
            filled_at: Utc::now(),
        })
    }

    async fn get_order_by_exchange_id(
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

    async fn get_user_trades_since(
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
}
