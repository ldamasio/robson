//! Market Data Types
//!
//! Canonical market data types used across Robson v2.
//! These are exchange-agnostic and can be used for both live and simulated data.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::Symbol;

// =============================================================================
// Tick
// =============================================================================

/// Single trade tick from an exchange.
///
/// Represents the smallest unit of market data - a single trade execution.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Tick {
    /// Trading symbol
    pub symbol: Symbol,
    /// Price at which the trade occurred
    pub price: Decimal,
    /// Quantity traded
    pub quantity: Decimal,
    /// Timestamp of the trade (exchange time)
    pub timestamp: DateTime<Utc>,
    /// Trade ID (unique identifier from exchange)
    pub trade_id: String,
}

impl Tick {
    /// Create a new tick.
    pub fn new(
        symbol: Symbol,
        price: Decimal,
        quantity: Decimal,
        timestamp: DateTime<Utc>,
        trade_id: String,
    ) -> Self {
        Self {
            symbol,
            price,
            quantity,
            timestamp,
            trade_id,
        }
    }

    /// Create a tick for testing.
    #[cfg(test)]
    pub fn test_tick(symbol: &str, price: i64, quantity: i64) -> Self {
        Self::new(
            Symbol::from_pair(symbol).unwrap(),
            Decimal::from(price),
            Decimal::from(quantity),
            Utc::now(),
            "test".to_string(),
        )
    }
}

// =============================================================================
// Candle
// =============================================================================

/// OHLCV candlestick data.
///
/// Represents aggregated trade data over a time interval.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Candle {
    /// Trading symbol
    pub symbol: Symbol,
    /// Open price
    pub open: Decimal,
    /// High price
    pub high: Decimal,
    /// Low price
    pub low: Decimal,
    /// Close price
    pub close: Decimal,
    /// Total volume traded
    pub volume: Decimal,
    /// Number of trades in this candle
    pub trades: u64,
    /// Candle open time
    pub open_time: DateTime<Utc>,
    /// Candle close time
    pub close_time: DateTime<Utc>,
}

impl Candle {
    /// Create a new candle.
    pub fn new(
        symbol: Symbol,
        open: Decimal,
        high: Decimal,
        low: Decimal,
        close: Decimal,
        volume: Decimal,
        trades: u64,
        open_time: DateTime<Utc>,
        close_time: DateTime<Utc>,
    ) -> Self {
        Self {
            symbol,
            open,
            high,
            low,
            close,
            volume,
            trades,
            open_time,
            close_time,
        }
    }

    /// Create a candle from a tick (for 1-second or tick aggregation).
    pub fn from_tick(tick: &Tick) -> Self {
        Self {
            symbol: tick.symbol.clone(),
            open: tick.price,
            high: tick.price,
            low: tick.price,
            close: tick.price,
            volume: tick.quantity,
            trades: 1,
            open_time: tick.timestamp,
            close_time: tick.timestamp,
        }
    }

    /// Update candle with a new tick.
    pub fn update_with_tick(&mut self, tick: &Tick) {
        self.high = self.high.max(tick.price);
        self.low = self.low.min(tick.price);
        self.close = tick.price;
        self.volume += tick.quantity;
        self.trades += 1;
        self.close_time = self.close_time.max(tick.timestamp);
    }

    /// Get the candle's midpoint price.
    pub fn midpoint(&self) -> Option<Decimal> {
        let mid = (self.high + self.low) / Decimal::from(2);
        if mid > Decimal::ZERO { Some(mid) } else { None }
    }
}

// =============================================================================
// Order Book Snapshot
// =============================================================================

/// Snapshot of the order book at a point in time.
///
/// Represents the best bids and asks on the order book.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrderBookSnapshot {
    /// Trading symbol
    pub symbol: Symbol,
    /// Best bid prices (descending)
    pub bids: Vec<(Decimal, Decimal)>, // (price, quantity)
    /// Best ask prices (ascending)
    pub asks: Vec<(Decimal, Decimal)>, // (price, quantity)
    /// Timestamp of the snapshot
    pub timestamp: DateTime<Utc>,
}

impl OrderBookSnapshot {
    /// Create a new order book snapshot.
    pub fn new(
        symbol: Symbol,
        bids: Vec<(Decimal, Decimal)>,
        asks: Vec<(Decimal, Decimal)>,
        timestamp: DateTime<Utc>,
    ) -> Self {
        Self { symbol, bids, asks, timestamp }
    }

    /// Get the best bid price (highest bid).
    pub fn best_bid(&self) -> Option<Decimal> {
        self.bids.first().map(|(price, _)| *price)
    }

    /// Get the best ask price (lowest ask).
    pub fn best_ask(&self) -> Option<Decimal> {
        self.asks.first().map(|(price, _)| *price)
    }

    /// Get the midpoint price (best bid + best ask) / 2.
    pub fn midpoint(&self) -> Option<Decimal> {
        match (self.best_bid(), self.best_ask()) {
            (Some(bid), Some(ask)) => Some((bid + ask) / Decimal::from(2)),
            _ => None,
        }
    }

    /// Get the spread (best ask - best bid).
    pub fn spread(&self) -> Option<Decimal> {
        match (self.best_bid(), self.best_ask()) {
            (Some(bid), Some(ask)) => Some(ask - bid),
            _ => None,
        }
    }
}

// =============================================================================
// Market Data Event (unified feed)
// =============================================================================

/// Unified market data event from the feed.
///
/// Wraps different types of market data for distribution to subscribers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum MarketDataEvent {
    /// Single trade tick
    Tick(Tick),
    /// Candlestick data (aggregated)
    Candle(Candle),
    /// Order book snapshot
    OrderBook(OrderBookSnapshot),
}

impl MarketDataEvent {
    /// Get the symbol of the event.
    pub fn symbol(&self) -> Symbol {
        match self {
            MarketDataEvent::Tick(t) => t.symbol.clone(),
            MarketDataEvent::Candle(c) => c.symbol.clone(),
            MarketDataEvent::OrderBook(ob) => ob.symbol.clone(),
        }
    }

    /// Get the timestamp of the event.
    pub fn timestamp(&self) -> DateTime<Utc> {
        match self {
            MarketDataEvent::Tick(t) => t.timestamp,
            MarketDataEvent::Candle(c) => c.close_time,
            MarketDataEvent::OrderBook(ob) => ob.timestamp,
        }
    }

    /// Get the price if available (Tick, Candle, or OrderBook midpoint).
    pub fn price(&self) -> Option<Decimal> {
        match self {
            MarketDataEvent::Tick(t) => Some(t.price),
            MarketDataEvent::Candle(c) => Some(c.close),
            MarketDataEvent::OrderBook(ob) => ob.midpoint(),
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn test_tick_creation() {
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let tick = Tick::new(symbol, dec!(95000), dec!(0.1), Utc::now(), "12345".to_string());

        assert_eq!(tick.price, dec!(95000));
        assert_eq!(tick.quantity, dec!(0.1));
    }

    #[test]
    fn test_candle_from_tick() {
        let tick = Tick::test_tick("BTCUSDT", 95000, 100);
        let candle = Candle::from_tick(&tick);

        assert_eq!(candle.open, dec!(95000));
        assert_eq!(candle.high, dec!(95000));
        assert_eq!(candle.low, dec!(95000));
        assert_eq!(candle.close, dec!(95000));
        assert_eq!(candle.volume, dec!(100));
        assert_eq!(candle.trades, 1);
    }

    #[test]
    fn test_candle_update_with_tick() {
        let tick1 = Tick::test_tick("BTCUSDT", 95000, 100);
        let mut candle = Candle::from_tick(&tick1);

        let tick2 = Tick::test_tick("BTCUSDT", 95100, 50);
        candle.update_with_tick(&tick2);

        assert_eq!(candle.open, dec!(95000));
        assert_eq!(candle.high, dec!(95100));
        assert_eq!(candle.low, dec!(95000));
        assert_eq!(candle.close, dec!(95100));
        assert_eq!(candle.volume, dec!(150));
        assert_eq!(candle.trades, 2);
    }

    #[test]
    fn test_order_book_best_prices() {
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let bids = vec![(dec!(94999), dec!(1.0)), (dec!(94998), dec!(2.0))];
        let asks = vec![(dec!(95001), dec!(1.0)), (dec!(95002), dec!(2.0))];
        let ob = OrderBookSnapshot::new(symbol, bids, asks, Utc::now());

        assert_eq!(ob.best_bid(), Some(dec!(94999)));
        assert_eq!(ob.best_ask(), Some(dec!(95001)));
        assert_eq!(ob.midpoint(), Some(dec!(95000)));
        assert_eq!(ob.spread(), Some(dec!(2)));
    }

    #[test]
    fn test_market_data_event_symbol() {
        let tick = Tick::test_tick("ETHUSDT", 3000, 10);
        let event = MarketDataEvent::Tick(tick);

        assert_eq!(event.symbol().as_pair(), "ETHUSDT");
        assert_eq!(event.price(), Some(dec!(3000)));
    }
}
