//! Execution layer port definitions.
//!
//! Ports define the interfaces for external services (exchange, market data).
//! Adapters implement these ports for specific services (Binance, stub, etc.).

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use robson_domain::{Candle, OrderSide, Price, Quantity, Side, Symbol};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::error::ExecError;

// =============================================================================
// Exchange Port
// =============================================================================

/// Futures account balance snapshot from the exchange.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FuturesBalance {
    /// Total wallet balance (includes unrealized PnL).
    pub wallet_balance: Decimal,
    /// Balance available for new positions (excludes margin on open positions).
    pub available_balance: Decimal,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SpotBalance {
    pub asset: String,
    pub free: Decimal,
    pub locked: Decimal,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum SpotOrderSide {
    Buy,
    Sell,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum SpotOrderQuantity {
    Base,
    Quote,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SpotOrderRequest {
    pub symbol: String,
    pub side: SpotOrderSide,
    pub quantity_kind: SpotOrderQuantity,
    pub quantity: Decimal,
    pub client_order_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SpotOrder {
    pub symbol: String,
    pub exchange_order_id: String,
    pub client_order_id: String,
    pub status: String,
    pub executed_qty: Decimal,
    pub cummulative_quote_qty: Decimal,
    pub fee: Decimal,
    pub fee_asset: String,
    pub transact_time: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum UniversalTransferType {
    MainUmfuture,
}

impl UniversalTransferType {
    pub fn as_binance_str(&self) -> &'static str {
        match self {
            Self::MainUmfuture => "MAIN_UMFUTURE",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TransferId(pub String);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Transfer {
    pub transfer_id: TransferId,
    pub client_tran_key: Option<String>,
    pub asset: String,
    pub amount: Decimal,
    pub transfer_type: UniversalTransferType,
    pub status: String,
    pub timestamp: DateTime<Utc>,
}

/// Port for exchange operations (placing/canceling orders).
///
/// Implementations:
/// - `StubExchange` - For testing (immediate fills at configured price)
/// - `BinanceAdapter` - Real Binance USD-M Futures
#[async_trait]
pub trait ExchangePort: Send + Sync {
    /// Validate account is in One-way position mode with expected leverage.
    ///
    /// **SAFETY CHECK**: Must be called before placing any order.
    /// Fails if account is not in One-way mode or leverage != expected.
    ///
    /// # Arguments
    ///
    /// * `symbol` - Trading pair to check
    /// * `expected_leverage` - Expected leverage multiplier (e.g., 1)
    ///
    /// # Returns
    ///
    /// `Ok(FuturesSettings)` if valid, `Err` with explanation if not.
    async fn validate_futures_settings(
        &self,
        symbol: &Symbol,
        expected_leverage: u8,
    ) -> Result<FuturesSettings, ExecError>;

    /// Place a market order.
    ///
    /// # Arguments
    ///
    /// * `symbol` - Trading pair (e.g., BTCUSDT)
    /// * `side` - Buy or Sell
    /// * `quantity` - Amount to trade
    /// * `client_order_id` - Unique ID for idempotency
    /// * `reduce_only` - If true, order only reduces existing position (exit)
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
        reduce_only: bool,
    ) -> Result<OrderResult, ExecError>;

    /// Place a reduce-only protective `STOP_MARKET` order (ADR-0039).
    ///
    /// The insurance stop lives on the exchange so stop enforcement survives
    /// daemon downtime. Binance implements these as Algo Order API
    /// conditional orders; the returned `OrderResult.exchange_order_id`
    /// carries the exchange-assigned algoId, not the eventual triggered
    /// orderId. The order is accepted but not filled; the result has no fill
    /// data (zero filled quantity / fee). Use `cancel_stop_market_order` to
    /// remove it.
    ///
    /// # Arguments
    ///
    /// * `symbol` - Trading pair (e.g., BTCUSDT)
    /// * `side` - Close side for the position (Long → Sell, Short → Buy)
    /// * `quantity` - Quantity to protect (the open position size)
    /// * `stop_price` - Chart-derived trailing stop price (never a percentage
    ///   of entry — see AGENTS.md rule 6 / ADR-0021)
    /// * `client_order_id` - Unique ID for idempotency
    ///
    /// # Returns
    ///
    /// `OrderResult` for the accepted (unfilled) order on success.
    async fn place_stop_market_order(
        &self,
        symbol: &Symbol,
        side: OrderSide,
        quantity: Quantity,
        stop_price: Price,
        client_order_id: &str,
    ) -> Result<OrderResult, ExecError>;

    /// Cancel an existing reduce-only protective `STOP_MARKET` order.
    ///
    /// `algo_id` is the id returned by `place_stop_market_order`;
    /// implementations must route this through the exchange's
    /// conditional/algo-order cancel API, not the regular order cancel
    /// endpoint.
    async fn cancel_stop_market_order(
        &self,
        symbol: &Symbol,
        algo_id: &str,
    ) -> Result<(), ExecError>;

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

    /// Query currently open (unfilled) orders for a symbol (ADR-0039).
    ///
    /// Used only by the reconciliation worker's orphan insurance-order sweep
    /// and startup recovery's insurance-stop heal path. Implementations return
    /// open conditional/algo stop orders, not regular `/order` open orders. An
    /// open reduce-only `STOP_MARKET` whose `client_order_id` carries the
    /// robsond `ins-` prefix but does not protect any tracked-open position is
    /// cancelled.
    ///
    /// # Arguments
    ///
    /// * `symbol` - Trading pair to scan for open orders
    ///
    /// # Returns
    ///
    /// Open orders for the symbol on success (empty when none are live).
    async fn get_open_orders(&self, symbol: &Symbol) -> Result<Vec<OpenOrderRecord>, ExecError>;

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

    /// Query the USDT-M futures account balance.
    ///
    /// Returns wallet balance (total equity including unrealized PnL) and
    /// available balance (free for new positions). Used for capital base
    /// derivation per ADR-0024 §6.
    async fn get_futures_balance(&self) -> Result<FuturesBalance, ExecError>;

    async fn get_spot_account_balances(&self) -> Result<Vec<SpotBalance>, ExecError>;

    async fn get_spot_price(&self, symbol: &str) -> Result<Price, ExecError>;

    async fn spot_symbol_is_trading(&self, symbol: &str) -> Result<bool, ExecError> {
        let _ = symbol;
        Err(ExecError::Exchange("spot symbol trading lookup is not implemented".to_string()))
    }

    async fn place_spot_market_order(
        &self,
        request: SpotOrderRequest,
    ) -> Result<SpotOrder, ExecError>;

    async fn get_spot_order(
        &self,
        symbol: &str,
        client_order_id: &str,
    ) -> Result<Option<SpotOrder>, ExecError>;

    async fn universal_transfer(
        &self,
        asset: &str,
        amount: Decimal,
        transfer_type: UniversalTransferType,
        client_tran_key: &str,
    ) -> Result<TransferId, ExecError>;

    async fn get_transfer_history(
        &self,
        transfer_type: UniversalTransferType,
        start_time: DateTime<Utc>,
    ) -> Result<Vec<Transfer>, ExecError>;

    /// Query every currently open exchange position.
    ///
    /// Used by the daemon reconciliation worker to detect positions that
    /// exist on the exchange but are not tracked in Robson state.
    async fn get_all_open_positions(&self) -> Result<Vec<ExchangePosition>, ExecError>;

    /// Close an existing exchange position using a reduce-only market order.
    ///
    /// `side` represents the position side being closed, not the outbound
    /// order side.
    async fn close_position_market(
        &self,
        symbol: &Symbol,
        side: Side,
        quantity: Quantity,
        client_order_id: &str,
    ) -> Result<OrderResult, ExecError>;

    /// Query a specific order by its exchange-assigned id.
    ///
    /// Returns `Some(OrderResult)` if the order exists and has fill data.
    /// Returns `None` if the order is not found or has not been filled.
    ///
    /// Used by the reconciliation pipeline to retrieve `OrderFillEvidence`
    /// for reverse-reconciliation closes (TD-2026-05-05-001).
    async fn get_order_by_exchange_id(
        &self,
        symbol: &Symbol,
        order_id: &str,
    ) -> Result<Option<OrderResult>, ExecError>;

    /// Query a protective stop by algo id and resolve its real triggered fill.
    ///
    /// Returns `Ok(None)` when the conditional order has not triggered. When
    /// the exchange reports a real triggered order id, implementations must
    /// resolve that real order via the regular order query path and return its
    /// `OrderResult`; Policy 11 evidence is from the actual order, never from
    /// algo-level estimates.
    async fn get_stop_order_fill(
        &self,
        symbol: &Symbol,
        algo_id: &str,
    ) -> Result<Option<OrderResult>, ExecError>;

    /// Query user trade history for a symbol since a given timestamp.
    ///
    /// Returns trades ordered oldest-first. Used by the reconciliation
    /// pipeline as evidence source 2 (user trades) when a specific order
    /// id is not known (TD-2026-05-05-001).
    async fn get_user_trades_since(
        &self,
        symbol: &Symbol,
        since: DateTime<Utc>,
        limit: u16,
    ) -> Result<Vec<UserTradeRecord>, ExecError>;
}

/// USD-M Futures account settings for a symbol.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FuturesSettings {
    /// Position mode (e.g., "One-way")
    pub position_mode: String,
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

/// An open (unfilled) conditional/algo stop order observed on the exchange.
///
/// Returned by `get_open_orders` so the reconciliation worker can detect and
/// cancel orphaned robsond-authored insurance stops (ADR-0039).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OpenOrderRecord {
    /// Exchange-assigned algo id
    pub exchange_order_id: String,
    /// Client-provided order id (robsond insurance stops carry the `ins-`
    /// prefix)
    pub client_order_id: String,
    /// Order type as reported by the exchange (e.g. `STOP_MARKET`, `MARKET`)
    pub order_type: String,
    /// Whether the order can only reduce an existing position
    pub reduce_only: bool,
    /// Stop/trigger price for conditional algo orders (`STOP_MARKET`); `None`
    /// otherwise
    pub stop_price: Option<Price>,
    /// Outbound order side
    pub side: OrderSide,
}

/// Individual trade from exchange trade history.
///
/// Used by `get_user_trades_since` to provide evidence for
/// reverse-reconciliation closes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserTradeRecord {
    /// Exchange-assigned order id of the originating order.
    pub exchange_order_id: String,
    /// Exchange-assigned trade id.
    pub exchange_trade_id: String,
    /// Fill price reported by the exchange.
    pub fill_price: Price,
    /// Filled quantity reported by the exchange.
    pub filled_quantity: Quantity,
    /// Trading fee paid.
    pub fee: Decimal,
    /// Fee asset (e.g. "USDT", "BNB").
    pub fee_asset: String,
    /// When the trade occurred (exchange-reported).
    pub filled_at: DateTime<Utc>,
}

/// Open position observed directly on the exchange.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExchangePosition {
    /// Trading pair (e.g. BTCUSDT)
    pub symbol: Symbol,
    /// Position direction on the exchange
    pub side: Side,
    /// Open quantity
    pub quantity: Quantity,
    /// Average entry price reported by the exchange
    pub entry_price: Price,
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
// OHLCV Port
// =============================================================================

/// Candlestick interval for OHLCV requests.
///
/// Values map directly to Binance kline interval strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CandleInterval {
    /// 1-minute candles
    OneMinute,
    /// 5-minute candles
    FiveMinutes,
    /// 15-minute candles — primary interval for `TechnicalStopAnalyzer`
    /// (REQ-CORE-TECHSTOP-004)
    FifteenMinutes,
    /// 1-hour candles
    OneHour,
    /// 4-hour candles
    FourHours,
    /// 1-day candles
    OneDay,
}

impl CandleInterval {
    /// Binance REST API interval string for this interval.
    pub fn as_binance_str(&self) -> &'static str {
        match self {
            CandleInterval::OneMinute => "1m",
            CandleInterval::FiveMinutes => "5m",
            CandleInterval::FifteenMinutes => "15m",
            CandleInterval::OneHour => "1h",
            CandleInterval::FourHours => "4h",
            CandleInterval::OneDay => "1d",
        }
    }
}

/// Port for fetching historical OHLCV (candlestick) data.
///
/// Used by callers of `TechnicalStopAnalyzer` in `robson-engine` to supply
/// historical chart data before computing chart-derived stop levels.
///
/// Implementations:
/// - `BinanceOhlcvAdapter` in `robsond` — fetches from Binance REST klines
/// - Stub for testing — configurable candle sequences
///
/// # Hexagonal placement
///
/// This port is defined in `robson-exec` (the ports layer). The concrete
/// adapter belongs in `robsond` (composition root). `robson-engine` never
/// depends on this port directly — callers fetch candles first, then pass
/// `Vec<Candle>` into the pure analyzer.
#[async_trait]
pub trait OhlcvPort: Send + Sync {
    /// Fetch historical candles for a symbol, ordered oldest-first.
    ///
    /// # Arguments
    ///
    /// * `symbol` - Trading pair (e.g., `BTCUSDT`)
    /// * `interval` - Candle interval (`FifteenMinutes` per
    ///   REQ-CORE-TECHSTOP-004)
    /// * `limit` - Number of candles to fetch (use ≥ 100; max 1000)
    ///
    /// # Returns
    ///
    /// Candles ordered oldest-first. The caller must ensure enough history
    /// exists before passing to `TechnicalStopAnalyzer`.
    async fn fetch_candles(
        &self,
        symbol: &Symbol,
        interval: CandleInterval,
        limit: u16,
    ) -> Result<Vec<Candle>, ExecError>;
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use rust_decimal_macros::dec;

    use super::*;

    #[test]
    fn test_futures_settings_serialization() {
        let settings = FuturesSettings {
            position_mode: "One-way".to_string(),
            leverage: 10,
            symbol: "BTCUSDT".to_string(),
        };

        let json = serde_json::to_string(&settings).unwrap();
        let parsed: FuturesSettings = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.position_mode, "One-way");
        assert_eq!(parsed.leverage, 10);
    }

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
