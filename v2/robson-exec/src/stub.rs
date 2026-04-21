//! Stub implementations for testing.
//!
//! These implementations simulate exchange and market data behavior
//! without making real API calls.

use std::{collections::HashMap, sync::RwLock};

use async_trait::async_trait;
use chrono::Utc;
use robson_domain::{Candle, OrderSide, Price, Quantity, Symbol};
use rust_decimal::Decimal;

use crate::{
    error::ExecError,
    ports::{
        CandleInterval, ExchangePort, ExchangePosition, FuturesSettings, MarketDataPort, OhlcvPort,
        OrderResult, PriceUpdate,
    },
};

// =============================================================================
// Stub Exchange
// =============================================================================

/// Stub exchange for testing.
///
/// Simulates immediate fills at a configured price.
/// Default configuration: One-way position mode, 10x leverage.
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
    /// Simulated futures settings (position_mode, leverage)
    futures_settings: RwLock<(String, u8)>,
    /// Simulated open positions returned by reconciliation scans.
    open_positions: RwLock<HashMap<String, ExchangePosition>>,
}

impl StubExchange {
    /// Create a new stub exchange with default price.
    ///
    /// Default futures settings: position_mode="One-way", leverage=10
    pub fn new(default_price: Decimal) -> Self {
        Self {
            prices: RwLock::new(HashMap::new()),
            default_price,
            fee_rate: Decimal::new(1, 3), // 0.001 = 0.1%
            order_counter: RwLock::new(0),
            fail_next: RwLock::new(false),
            futures_settings: RwLock::new(("One-way".to_string(), 10)),
            open_positions: RwLock::new(HashMap::new()),
        }
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

    /// Get price for a symbol (or default).
    pub fn get_price_decimal(&self, symbol: &str) -> Decimal {
        let prices = self.prices.read().unwrap();
        prices.get(symbol).copied().unwrap_or(self.default_price)
    }

    /// Configure the next order to fail.
    pub fn set_fail_next(&self, fail: bool) {
        let mut fail_next = self.fail_next.write().unwrap();
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
        if self.should_fail() {
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
}
