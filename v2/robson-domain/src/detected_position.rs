//! Safety Net: Detected Rogue Positions
//!
//! Types for positions that were opened outside of Robson v2
//! (e.g., manually via Binance app) and need risk management applied.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::{Price, Quantity, Side, Symbol};

// =============================================================================
// Detected Position
// =============================================================================

/// A position detected from Binance that was not created through Robson v2.
///
/// This represents a "rogue" position that needs risk management applied.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DetectedPosition {
    /// Unique identifier from Binance (exchange-assigned)
    pub binance_position_id: String,
    /// Trading symbol (e.g., BTCUSDT)
    pub symbol: Symbol,
    /// Position side (Long or Short)
    pub side: Side,
    /// Entry price (average price of position)
    pub entry_price: Price,
    /// Current position quantity
    pub quantity: Quantity,
    /// When this position was first detected by Robson
    pub detected_at: DateTime<Utc>,
    /// Last time this position was verified
    pub last_verified_at: DateTime<Utc>,
    /// Calculated stop loss (if set)
    pub calculated_stop: Option<CalculatedStop>,
}

impl DetectedPosition {
    /// Create a new detected position.
    pub fn new(
        binance_position_id: String,
        symbol: Symbol,
        side: Side,
        entry_price: Price,
        quantity: Quantity,
    ) -> Self {
        let now = Utc::now();
        Self {
            binance_position_id,
            symbol,
            side,
            entry_price,
            quantity,
            detected_at: now,
            last_verified_at: now,
            calculated_stop: None,
        }
    }

    /// Calculate a safety stop for this position using fixed 2% rule.
    ///
    /// - LONG: stop = entry × 0.98 (2% below)
    /// - SHORT: stop = entry × 1.02 (2% above)
    ///
    /// This respects the 1% risk rule with 10x leverage.
    pub fn calculate_safety_stop(&mut self) -> CalculatedStop {
        let stop_price = match self.side {
            Side::Long => {
                // LONG: stop is below entry (2%)
                let stop_value = self.entry_price.as_decimal() * Decimal::from(98u32) / Decimal::from(100u32);
                Price::new(stop_value).unwrap_or(self.entry_price)
            }
            Side::Short => {
                // SHORT: stop is above entry (2%)
                let stop_value = self.entry_price.as_decimal() * Decimal::from(102u32) / Decimal::from(100u32);
                Price::new(stop_value).unwrap_or(self.entry_price)
            }
        };

        let entry = self.entry_price.as_decimal();
        let stop = stop_price.as_decimal();
        let distance = (entry - stop).abs();
        let distance_pct = (distance / entry) * Decimal::from(100u32);

        let calculated_stop = CalculatedStop {
            stop_price,
            distance,
            distance_pct,
            method: StopMethod::Fixed2Percent,
            calculated_at: Utc::now(),
        };

        self.calculated_stop = Some(calculated_stop.clone());
        calculated_stop
    }

    /// Check if the current price has hit the calculated stop.
    ///
    /// Returns `true` if stop is hit and position should be closed.
    ///
    /// # Returns
    ///
    /// * `None` - No stop has been calculated yet
    /// * `Some(false)` - Stop exists but not hit yet
    /// * `Some(true)` - Stop has been hit, should exit
    pub fn is_stop_hit(&self, current_price: Price) -> Option<bool> {
        let stop = self.calculated_stop.as_ref()?;

        let hit = match self.side {
            Side::Long => current_price.as_decimal() <= stop.stop_price.as_decimal(),
            Side::Short => current_price.as_decimal() >= stop.stop_price.as_decimal(),
        };

        Some(hit)
    }

    /// Update the last verified timestamp.
    pub fn mark_verified(&mut self) {
        self.last_verified_at = Utc::now();
    }
}

// =============================================================================
// Calculated Stop
// =============================================================================

/// A calculated stop loss for a detected position.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CalculatedStop {
    /// The stop price
    pub stop_price: Price,
    /// Absolute distance from entry to stop
    pub distance: Decimal,
    /// Distance as percentage of entry price
    pub distance_pct: Decimal,
    /// Method used to calculate this stop
    pub method: StopMethod,
    /// When this stop was calculated
    pub calculated_at: DateTime<Utc>,
}

impl CalculatedStop {
    /// Check if a given price has hit this stop.
    ///
    /// # Arguments
    ///
    /// * `side` - Position side (determines direction)
    /// * `current_price` - Current market price
    ///
    /// # Returns
    ///
    /// * `true` - Stop is hit, should exit
    /// * `false` - Stop not hit, stay in position
    pub fn is_hit(&self, side: Side, current_price: Price) -> bool {
        match side {
            Side::Long => current_price.as_decimal() <= self.stop_price.as_decimal(),
            Side::Short => current_price.as_decimal() >= self.stop_price.as_decimal(),
        }
    }
}

/// Method used to calculate the stop loss.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StopMethod {
    /// Fixed 2% from entry (safety net default)
    Fixed2Percent,
    /// Volatility-based (ATR) - not implemented yet
    VolatilityBased,
    /// User-configured - not implemented yet
    UserConfigured,
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn test_detected_position_creation() {
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let entry = Price::new(dec!(95000)).unwrap();
        let quantity = Quantity::new(dec!(0.1)).unwrap();

        let position = DetectedPosition::new(
            "binance_123".to_string(),
            symbol,
            Side::Long,
            entry,
            quantity,
        );

        assert_eq!(position.binance_position_id, "binance_123");
        assert_eq!(position.side, Side::Long);
        assert_eq!(position.entry_price.as_decimal(), dec!(95000));
        assert_eq!(position.quantity.as_decimal(), dec!(0.1));
        assert!(position.calculated_stop.is_none());
    }

    #[test]
    fn test_calculate_safety_stop_long() {
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let entry = Price::new(dec!(95000)).unwrap();
        let quantity = Quantity::new(dec!(0.1)).unwrap();

        let mut position = DetectedPosition::new(
            "binance_123".to_string(),
            symbol,
            Side::Long,
            entry,
            quantity,
        );

        let stop = position.calculate_safety_stop();

        // LONG: stop = 95000 × 0.98 = 93100
        assert_eq!(stop.stop_price.as_decimal(), dec!(93100));
        assert_eq!(stop.distance, dec!(1900));
        assert_eq!(stop.distance_pct, dec!(2));
        assert_eq!(stop.method, StopMethod::Fixed2Percent);
        assert!(position.calculated_stop.is_some());
    }

    #[test]
    fn test_calculate_safety_stop_short() {
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let entry = Price::new(dec!(95000)).unwrap();
        let quantity = Quantity::new(dec!(0.1)).unwrap();

        let mut position = DetectedPosition::new(
            "binance_123".to_string(),
            symbol,
            Side::Short,
            entry,
            quantity,
        );

        let stop = position.calculate_safety_stop();

        // SHORT: stop = 95000 × 1.02 = 96900
        assert_eq!(stop.stop_price.as_decimal(), dec!(96900));
        assert_eq!(stop.distance, dec!(1900));
        assert_eq!(stop.distance_pct, dec!(2));
        assert_eq!(stop.method, StopMethod::Fixed2Percent);
    }

    #[test]
    fn test_is_stop_hit_long() {
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let entry = Price::new(dec!(95000)).unwrap();
        let quantity = Quantity::new(dec!(0.1)).unwrap();

        let mut position = DetectedPosition::new(
            "binance_123".to_string(),
            symbol,
            Side::Long,
            entry,
            quantity,
        );

        position.calculate_safety_stop();

        // Price above stop - not hit
        assert_eq!(position.is_stop_hit(Price::new(dec!(93200)).unwrap()), Some(false));
        assert_eq!(position.is_stop_hit(Price::new(dec!(95000)).unwrap()), Some(false));

        // Price at stop - hit
        assert_eq!(position.is_stop_hit(Price::new(dec!(93100)).unwrap()), Some(true));

        // Price below stop - hit
        assert_eq!(position.is_stop_hit(Price::new(dec!(93000)).unwrap()), Some(true));
    }

    #[test]
    fn test_is_stop_hit_short() {
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let entry = Price::new(dec!(95000)).unwrap();
        let quantity = Quantity::new(dec!(0.1)).unwrap();

        let mut position = DetectedPosition::new(
            "binance_123".to_string(),
            symbol,
            Side::Short,
            entry,
            quantity,
        );

        position.calculate_safety_stop();

        // Price below stop - not hit
        assert_eq!(position.is_stop_hit(Price::new(dec!(96800)).unwrap()), Some(false));
        assert_eq!(position.is_stop_hit(Price::new(dec!(95000)).unwrap()), Some(false));

        // Price at stop - hit
        assert_eq!(position.is_stop_hit(Price::new(dec!(96900)).unwrap()), Some(true));

        // Price above stop - hit
        assert_eq!(position.is_stop_hit(Price::new(dec!(97000)).unwrap()), Some(true));
    }

    #[test]
    fn test_is_stop_hit_returns_none_when_no_stop() {
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let entry = Price::new(dec!(95000)).unwrap();
        let quantity = Quantity::new(dec!(0.1)).unwrap();

        let position = DetectedPosition::new(
            "binance_123".to_string(),
            symbol,
            Side::Long,
            entry,
            quantity,
        );

        // No stop calculated yet
        assert!(position.calculated_stop.is_none());
        assert_eq!(position.is_stop_hit(Price::new(dec!(93000)).unwrap()), None);
    }

    #[test]
    fn test_mark_verified_updates_timestamp() {
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let entry = Price::new(dec!(95000)).unwrap();
        let quantity = Quantity::new(dec!(0.1)).unwrap();

        let mut position = DetectedPosition::new(
            "binance_123".to_string(),
            symbol,
            Side::Long,
            entry,
            quantity,
        );

        let original_time = position.last_verified_at;

        // Wait a tiny bit (not really testable without tokio::time::sleep)
        position.mark_verified();

        // Timestamp should be updated (equal or later)
        assert!(position.last_verified_at >= original_time);
    }

    #[test]
    fn test_calculated_stop_is_hit_method() {
        let stop = CalculatedStop {
            stop_price: Price::new(dec!(93100)).unwrap(),
            distance: dec!(1900),
            distance_pct: dec!(2),
            method: StopMethod::Fixed2Percent,
            calculated_at: Utc::now(),
        };

        // LONG: price at or below stop is hit
        assert!(stop.is_hit(Side::Long, Price::new(dec!(93100)).unwrap()));
        assert!(stop.is_hit(Side::Long, Price::new(dec!(93000)).unwrap()));
        assert!(!stop.is_hit(Side::Long, Price::new(dec!(93200)).unwrap()));

        // SHORT: price at or above stop is hit
        assert!(stop.is_hit(Side::Short, Price::new(dec!(93100)).unwrap()));
        assert!(stop.is_hit(Side::Short, Price::new(dec!(93200)).unwrap()));
        assert!(!stop.is_hit(Side::Short, Price::new(dec!(93000)).unwrap()));
    }

    #[test]
    fn test_serialize_deserialize_detected_position() {
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let entry = Price::new(dec!(95000)).unwrap();
        let quantity = Quantity::new(dec!(0.1)).unwrap();

        let mut position = DetectedPosition::new(
            "binance_123".to_string(),
            symbol,
            Side::Long,
            entry,
            quantity,
        );

        position.calculate_safety_stop();

        // Serialize
        let json = serde_json::to_string(&position).unwrap();

        // Deserialize
        let deserialized: DetectedPosition = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.binance_position_id, position.binance_position_id);
        assert_eq!(deserialized.side, position.side);
        assert_eq!(deserialized.entry_price, position.entry_price);
        assert!(deserialized.calculated_stop.is_some());
    }

    #[test]
    fn test_serialize_deserialize_calculated_stop() {
        let stop = CalculatedStop {
            stop_price: Price::new(dec!(93100)).unwrap(),
            distance: dec!(1900),
            distance_pct: dec!(2),
            method: StopMethod::Fixed2Percent,
            calculated_at: Utc::now(),
        };

        // Serialize
        let json = serde_json::to_string(&stop).unwrap();

        // Deserialize
        let deserialized: CalculatedStop = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.stop_price, stop.stop_price);
        assert_eq!(deserialized.distance, stop.distance);
        assert_eq!(deserialized.distance_pct, stop.distance_pct);
        assert_eq!(deserialized.method, stop.method);
    }
}
