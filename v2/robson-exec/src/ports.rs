//! Execution layer port definitions.
//!
//! Ports define the interfaces for external services (exchange, market data).
//! Adapters implement these ports for specific services (Binance, stub, etc.).

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use robson_domain::{OrderSide, Price, Quantity, Symbol};

use crate::error::ExecError;

// =============================================================================
// Exchange Port
// =============================================================================

/// Port for exchange operations (placing/canceling orders).
///
/// Implementations:
/// - `StubExchange` - For testing (immediate fills at configured price)
/// - `BinanceAdapter` - Real Binance isolated margin (Phase 9)
#[async_trait]
pub trait ExchangePort: Send + Sync {
    /// Validate account is in isolated margin mode with expected leverage.
    ///
    /// **SAFETY CHECK**: Must be called before placing any order.
    /// Fails if account is not in isolated margin mode or leverage != expected.
    ///
    /// # Arguments
    ///
    /// * `symbol` - Trading pair to check
    /// * `expected_leverage` - Expected leverage multiplier (e.g., 10)
    ///
    /// # Returns
    ///
    /// `Ok(MarginSettings)` if valid, `Err` with explanation if not.
    async fn validate_margin_settings(
        &self,
        symbol: &Symbol,
        expected_leverage: u8,
    ) -> Result<MarginSettings, ExecError>;

    /// Place a market order.
    ///
    /// # Arguments
    ///
    /// * `symbol` - Trading pair (e.g., BTCUSDT)
    /// * `side` - Buy or Sell
    /// * `quantity` - Amount to trade
    /// * `client_order_id` - Unique ID for idempotency
    ///
    /// # Returns
    ///
    /// `OrderResult` with fill details on success.
    async fn place_market_order(
        &self,
        symbol: &Symbol,
        side: OrderSide,
        quantity: Quantity,
        client_order_id: &str,
    ) -> Result<OrderResult, ExecError>;

    /// Cancel an existing order.
    ///
    /// # Arguments
    ///
    /// * `symbol` - Trading pair
    /// * `order_id` - Exchange order ID to cancel
    ///
    /// # Returns
    ///
    /// `Ok(())` if cancelled, error otherwise.
    async fn cancel_order(&self, symbol: &Symbol, order_id: &str) -> Result<(), ExecError>;

    /// Get current price for a symbol.
    ///
    /// # Arguments
    ///
    /// * `symbol` - Trading pair
    ///
    /// # Returns
    ///
    /// Current market price.
    async fn get_price(&self, symbol: &Symbol) -> Result<Price, ExecError>;

    /// Check if exchange is healthy/connected.
    async fn health_check(&self) -> Result<(), ExecError>;
}

/// Margin account settings for a symbol.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarginSettings {
    /// Is this an isolated margin account (not cross)
    pub is_isolated: bool,
    /// Current leverage multiplier
    pub leverage: u8,
    /// Symbol this applies to
    pub symbol: String,
}

/// Result of a successful order execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderResult {
    /// Exchange-assigned order ID
    pub exchange_order_id: String,
    /// Client-provided order ID (for idempotency)
    pub client_order_id: String,
    /// Actual fill price
    pub fill_price: Price,
    /// Actual filled quantity
    pub filled_quantity: Quantity,
    /// Trading fee paid
    pub fee: Decimal,
    /// Fee asset (e.g., "USDT", "BNB")
    pub fee_asset: String,
    /// When the order was filled
    pub filled_at: DateTime<Utc>,
}

// =============================================================================
// Market Data Port
// =============================================================================

/// Port for market data subscriptions.
///
/// Implementations:
/// - `StubMarketData` - For testing (configurable price stream)
/// - `BinanceWebSocket` - Real Binance WebSocket (Phase 9)
#[async_trait]
pub trait MarketDataPort: Send + Sync {
    /// Subscribe to price updates for a symbol.
    ///
    /// Returns a receiver that yields price updates.
    async fn subscribe(
        &self,
        symbol: &Symbol,
    ) -> Result<tokio::sync::mpsc::Receiver<PriceUpdate>, ExecError>;

    /// Unsubscribe from price updates.
    async fn unsubscribe(&self, symbol: &Symbol) -> Result<(), ExecError>;

    /// Get current snapshot price (without subscription).
    async fn get_price(&self, symbol: &Symbol) -> Result<Price, ExecError>;
}

/// Price update from market data feed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PriceUpdate {
    /// Trading pair
    pub symbol: Symbol,
    /// Current price
    pub price: Price,
    /// Update timestamp
    pub timestamp: DateTime<Utc>,
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn test_order_result_serialization() {
        let result = OrderResult {
            exchange_order_id: "12345".to_string(),
            client_order_id: "abc-123".to_string(),
            fill_price: Price::new(dec!(95000)).unwrap(),
            filled_quantity: Quantity::new(dec!(0.1)).unwrap(),
            fee: dec!(0.001),
            fee_asset: "BNB".to_string(),
            filled_at: Utc::now(),
        };

        let json = serde_json::to_string(&result).unwrap();
        let parsed: OrderResult = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.exchange_order_id, "12345");
        assert_eq!(parsed.fill_price.as_decimal(), dec!(95000));
    }

    #[test]
    fn test_price_update_serialization() {
        let update = PriceUpdate {
            symbol: Symbol::from_pair("BTCUSDT").unwrap(),
            price: Price::new(dec!(95000)).unwrap(),
            timestamp: Utc::now(),
        };

        let json = serde_json::to_string(&update).unwrap();
        let parsed: PriceUpdate = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.symbol.as_pair(), "BTCUSDT");
    }
}
