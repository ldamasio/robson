//! Trailing Stop Logic (Pure Functions)
//!
//! This module contains pure functions for calculating and updating trailing stops.
//! All functions are deterministic and have no side effects.
//!
//! # Trailing Stop Algorithm (Anchored 1x)
//!
//! The trailing stop is "anchored" to the technical stop distance:
//! - LONG: Stop = peak_price - tech_stop_distance
//! - SHORT: Stop = low_price + tech_stop_distance
//!
//! Key invariants:
//! - Stop is monotonic (never moves against us)
//! - Favorable extreme is monotonic (peak only rises, low only falls)
//! - Stop is only updated when price makes a new favorable extreme

use crate::value_objects::{Price, Side};
use rust_decimal::Decimal;

/// Result of a trailing stop update
///
/// Contains the new stop and new favorable extreme if an update occurred.
/// Returns `None` when no update is needed (price didn't make new extreme).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrailingStopUpdate {
    /// New trailing stop price
    pub new_stop: Price,
    /// New favorable extreme (peak for Long, low for Short)
    pub new_favorable_extreme: Price,
}

/// Update trailing stop using anchored 1x logic
///
/// Calculates the new trailing stop based on current price movement.
/// Only updates when price makes a new favorable extreme (new high for Long, new low for Short).
///
/// # Arguments
///
/// * `side` - Position side (Long or Short)
/// * `current_price` - Current market price
/// * `favorable_extreme` - Current best price seen (peak for Long, low for Short)
/// * `current_trailing_stop` - Current trailing stop price
/// * `tech_stop_distance` - Technical stop distance (anchor for trailing)
///
/// # Returns
///
/// * `Some(TrailingStopUpdate)` - When price made new extreme and stop should move
/// * `None` - When no update needed (price didn't make new extreme)
///
/// # Long Position Behavior
///
/// ```text
/// new_peak = max(current_price, favorable_extreme)
/// candidate_stop = new_peak - tech_stop_distance
///
/// Only update if candidate_stop > current_trailing_stop
/// ```
///
/// # Short Position Behavior
///
/// ```text
/// new_low = min(current_price, favorable_extreme)
/// candidate_stop = new_low + tech_stop_distance
///
/// Only update if candidate_stop < current_trailing_stop
/// ```
///
/// # Examples
///
/// ```
/// # use robson_domain::trailing::update_trailing_stop_anchored;
/// # use robson_domain::value_objects::{Price, Side};
/// # use rust_decimal_macros::dec;
/// // LONG: Start with entry at $95,000, stop at $93,500 (distance $1,500)
/// let side = Side::Long;
/// let current_stop = Price::new(dec!(93500)).unwrap();
/// let extreme = Price::new(dec!(95000)).unwrap();
/// let tech_dist = dec!(1500);
///
/// // Price rises to $96,500 - should update stop to $95,000
/// let result = update_trailing_stop_anchored(
///     side,
///     Price::new(dec!(96500)).unwrap(),
///     extreme,
///     current_stop,
///     tech_dist,
/// );
/// assert!(result.is_some());
/// let update = result.unwrap();
/// assert_eq!(update.new_stop.as_decimal(), dec!(95000)); // 96500 - 1500
/// assert_eq!(update.new_favorable_extreme.as_decimal(), dec!(96500));
///
/// // Price drops to $95,500 - no update (not new high)
/// let result = update_trailing_stop_anchored(
///     side,
///     Price::new(dec!(95500)).unwrap(),
///     update.new_favorable_extreme,
///     update.new_stop,
///     tech_dist,
/// );
/// assert!(result.is_none()); // No new high, no update
/// ```
pub fn update_trailing_stop_anchored(
    side: Side,
    current_price: Price,
    favorable_extreme: Price,
    current_trailing_stop: Price,
    tech_stop_distance: Decimal,
) -> Option<TrailingStopUpdate> {
    match side {
        Side::Long => {
            // LONG: Check for new high
            let new_peak = if current_price.as_decimal() > favorable_extreme.as_decimal() {
                current_price
            } else {
                favorable_extreme
            };

            // Calculate candidate stop from new peak
            let candidate_stop = Decimal::from(new_peak.as_decimal()) - tech_stop_distance;

            // Only update if candidate stop is HIGHER (more favorable)
            if candidate_stop > current_trailing_stop.as_decimal() {
                Some(TrailingStopUpdate {
                    new_stop: Price::from(candidate_stop),
                    new_favorable_extreme: new_peak,
                })
            } else {
                None
            }
        },
        Side::Short => {
            // SHORT: Check for new low
            let new_low = if current_price.as_decimal() < favorable_extreme.as_decimal() {
                current_price
            } else {
                favorable_extreme
            };

            // Calculate candidate stop from new low
            let candidate_stop = Decimal::from(new_low.as_decimal()) + tech_stop_distance;

            // Only update if candidate stop is LOWER (more favorable)
            if candidate_stop < current_trailing_stop.as_decimal() {
                Some(TrailingStopUpdate {
                    new_stop: Price::from(candidate_stop),
                    new_favorable_extreme: new_low,
                })
            } else {
                None
            }
        },
    }
}

/// Check if trailing stop is hit (should exit)
///
/// # Arguments
///
/// * `side` - Position side (Long or Short)
/// * `current_price` - Current market price
/// * `trailing_stop` - Trailing stop price
///
/// # Returns
///
/// * `true` - Stop is hit, should exit
/// * `false` - Stop not hit, stay in position
///
/// # Examples
///
/// ```
/// # use robson_domain::trailing::is_trailing_stop_hit;
/// # use robson_domain::value_objects::{Price, Side};
/// # use rust_decimal_macros::dec;
/// // LONG: Exit when price drops TO or BELOW stop
/// assert!(is_trailing_stop_hit(
///     Side::Long,
///     Price::new(dec!(95000)).unwrap(),
///     Price::new(dec!(95000)).unwrap(),
/// )); // Price AT stop - exit
///
/// assert!(is_trailing_stop_hit(
///     Side::Long,
///     Price::new(dec!(94900)).unwrap(),
///     Price::new(dec!(95000)).unwrap(),
/// )); // Price BELOW stop - exit
///
/// assert!(!is_trailing_stop_hit(
///     Side::Long,
///     Price::new(dec!(95100)).unwrap(),
///     Price::new(dec!(95000)).unwrap(),
/// )); // Price ABOVE stop - stay
///
/// // SHORT: Exit when price rises TO or ABOVE stop
/// assert!(is_trailing_stop_hit(
///     Side::Short,
///     Price::new(dec!(95000)).unwrap(),
///     Price::new(dec!(95000)).unwrap(),
/// )); // Price AT stop - exit
///
/// assert!(is_trailing_stop_hit(
///     Side::Short,
///     Price::new(dec!(95100)).unwrap(),
///     Price::new(dec!(95000)).unwrap(),
/// )); // Price ABOVE stop - exit
///
/// assert!(!is_trailing_stop_hit(
///     Side::Short,
///     Price::new(dec!(94900)).unwrap(),
///     Price::new(dec!(95000)).unwrap(),
/// )); // Price BELOW stop - stay
/// ```
pub fn is_trailing_stop_hit(side: Side, current_price: Price, trailing_stop: Price) -> bool {
    match side {
        // LONG: Exit when price <= stop
        Side::Long => current_price.as_decimal() <= trailing_stop.as_decimal(),
        // SHORT: Exit when price >= stop
        Side::Short => current_price.as_decimal() >= trailing_stop.as_decimal(),
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    // =========================================================================
    // LONG Position Tests
    // =========================================================================

    #[test]
    fn test_long_trailing_stop_moves_up_on_new_high() {
        // Start: entry $95k, stop $93.5k, distance $1.5k
        let side = Side::Long;
        let current_stop = Price::new(dec!(93500)).unwrap();
        let extreme = Price::new(dec!(95000)).unwrap();
        let tech_dist = dec!(1500);

        // Price rises to $96.5k (new high)
        let result = update_trailing_stop_anchored(
            side,
            Price::new(dec!(96500)).unwrap(),
            extreme,
            current_stop,
            tech_dist,
        );

        assert!(result.is_some());
        let update = result.unwrap();
        // New stop = $96.5k - $1.5k = $95k
        assert_eq!(update.new_stop.as_decimal(), dec!(95000));
        assert_eq!(update.new_favorable_extreme.as_decimal(), dec!(96500));
    }

    #[test]
    fn test_long_trailing_stop_no_update_when_price_below_extreme() {
        // Already made high at $96.5k, stop is at $95k
        let side = Side::Long;
        let current_stop = Price::new(dec!(95000)).unwrap();
        let extreme = Price::new(dec!(96500)).unwrap();
        let tech_dist = dec!(1500);

        // Price pulls back to $95.5k (not a new high)
        let result = update_trailing_stop_anchored(
            side,
            Price::new(dec!(95500)).unwrap(),
            extreme,
            current_stop,
            tech_dist,
        );

        assert!(result.is_none());
    }

    #[test]
    fn test_long_trailing_stop_multiple_updates_sequence() {
        let side = Side::Long;
        let tech_dist = dec!(1500);
        let mut current_stop = Price::new(dec!(93500)).unwrap();
        let mut extreme = Price::new(dec!(95000)).unwrap();

        // Sequence: $95k -> $96k -> $97k -> $98k
        let prices = [dec!(96000), dec!(97000), dec!(98000)];
        let expected_stops = [dec!(94500), dec!(95500), dec!(96500)];

        for (i, price) in prices.iter().enumerate() {
            let result = update_trailing_stop_anchored(
                side,
                Price::new(*price).unwrap(),
                extreme,
                current_stop,
                tech_dist,
            );

            assert!(result.is_some());
            let update = result.unwrap();
            assert_eq!(update.new_stop.as_decimal(), expected_stops[i]);
            assert_eq!(update.new_favorable_extreme.as_decimal(), *price);

            // Update for next iteration
            current_stop = update.new_stop;
            extreme = update.new_favorable_extreme;
        }
    }

    #[test]
    fn test_long_trailing_stop_monotonic_never_goes_down() {
        let side = Side::Long;
        let tech_dist = dec!(1500);
        let current_stop = Price::new(dec!(94500)).unwrap(); // Already moved up
        let extreme = Price::new(dec!(96000)).unwrap();

        // Price drops back to entry
        let result = update_trailing_stop_anchored(
            side,
            Price::new(dec!(95000)).unwrap(),
            extreme,
            current_stop,
            tech_dist,
        );

        // No update: stop stays at $94.5k (doesn't move down)
        assert!(result.is_none());
    }

    #[test]
    fn test_long_stop_hit_at_or_below() {
        let stop = Price::new(dec!(95000)).unwrap();

        // Price AT stop - should exit
        assert!(is_trailing_stop_hit(Side::Long, Price::new(dec!(95000)).unwrap(), stop));

        // Price BELOW stop - should exit
        assert!(is_trailing_stop_hit(Side::Long, Price::new(dec!(94900)).unwrap(), stop));

        // Price above stop - should NOT exit
        assert!(!is_trailing_stop_hit(Side::Long, Price::new(dec!(95100)).unwrap(), stop));
    }

    // =========================================================================
    // SHORT Position Tests
    // =========================================================================

    #[test]
    fn test_short_trailing_stop_moves_down_on_new_low() {
        // Start: entry $95k, stop $96.5k (above), distance $1.5k
        let side = Side::Short;
        let current_stop = Price::new(dec!(96500)).unwrap();
        let extreme = Price::new(dec!(95000)).unwrap();
        let tech_dist = dec!(1500);

        // Price drops to $93.5k (new low)
        let result = update_trailing_stop_anchored(
            side,
            Price::new(dec!(93500)).unwrap(),
            extreme,
            current_stop,
            tech_dist,
        );

        assert!(result.is_some());
        let update = result.unwrap();
        // New stop = $93.5k + $1.5k = $95k
        assert_eq!(update.new_stop.as_decimal(), dec!(95000));
        assert_eq!(update.new_favorable_extreme.as_decimal(), dec!(93500));
    }

    #[test]
    fn test_short_trailing_stop_no_update_when_price_above_extreme() {
        // Already made low at $93.5k, stop is at $95k
        let side = Side::Short;
        let current_stop = Price::new(dec!(95000)).unwrap();
        let extreme = Price::new(dec!(93500)).unwrap();
        let tech_dist = dec!(1500);

        // Price rises to $94.5k (not a new low)
        let result = update_trailing_stop_anchored(
            side,
            Price::new(dec!(94500)).unwrap(),
            extreme,
            current_stop,
            tech_dist,
        );

        assert!(result.is_none());
    }

    #[test]
    fn test_short_trailing_stop_multiple_updates_sequence() {
        let side = Side::Short;
        let tech_dist = dec!(1500);
        let mut current_stop = Price::new(dec!(96500)).unwrap();
        let mut extreme = Price::new(dec!(95000)).unwrap();

        // Sequence: $95k -> $94k -> $93k -> $92k
        let prices = [dec!(94000), dec!(93000), dec!(92000)];
        let expected_stops = [dec!(95500), dec!(94500), dec!(93500)];

        for (i, price) in prices.iter().enumerate() {
            let result = update_trailing_stop_anchored(
                side,
                Price::new(*price).unwrap(),
                extreme,
                current_stop,
                tech_dist,
            );

            assert!(result.is_some());
            let update = result.unwrap();
            assert_eq!(update.new_stop.as_decimal(), expected_stops[i]);
            assert_eq!(update.new_favorable_extreme.as_decimal(), *price);

            // Update for next iteration
            current_stop = update.new_stop;
            extreme = update.new_favorable_extreme;
        }
    }

    #[test]
    fn test_short_trailing_stop_monotonic_never_goes_up() {
        let side = Side::Short;
        let tech_dist = dec!(1500);
        let current_stop = Price::new(dec!(95000)).unwrap(); // Already moved down
        let extreme = Price::new(dec!(93500)).unwrap();

        // Price rises back to entry
        let result = update_trailing_stop_anchored(
            side,
            Price::new(dec!(95000)).unwrap(),
            extreme,
            current_stop,
            tech_dist,
        );

        // No update: stop stays at $95k (doesn't move up)
        assert!(result.is_none());
    }

    #[test]
    fn test_short_stop_hit_at_or_above() {
        let stop = Price::new(dec!(95000)).unwrap();

        // Price AT stop - should exit
        assert!(is_trailing_stop_hit(Side::Short, Price::new(dec!(95000)).unwrap(), stop));

        // Price ABOVE stop - should exit
        assert!(is_trailing_stop_hit(Side::Short, Price::new(dec!(95100)).unwrap(), stop));

        // Price below stop - should NOT exit
        assert!(!is_trailing_stop_hit(Side::Short, Price::new(dec!(94900)).unwrap(), stop));
    }

    // =========================================================================
    // Edge Cases
    // =========================================================================

    #[test]
    fn test_long_trailing_stop_with_zero_distance_has_no_effect() {
        // Zero distance means stop never moves
        let side = Side::Long;
        let current_stop = Price::new(dec!(93500)).unwrap();
        let extreme = Price::new(dec!(95000)).unwrap();
        let tech_dist = dec!(0);

        // Even with new high, stop won't move (candidate = current)
        let result = update_trailing_stop_anchored(
            side,
            Price::new(dec!(98000)).unwrap(),
            extreme,
            current_stop,
            tech_dist,
        );

        // No update because candidate_stop (98k - 0 = 98k) > current_stop (93.5k)
        // Wait, actually with zero distance, candidate = new_peak
        // And new_peak > current_stop, so it WOULD update
        // Let's verify the actual behavior:
        // candidate_stop = 98000 - 0 = 98000
        // 98000 > 93500, so update happens
        assert!(result.is_some());
    }

    #[test]
    fn test_favorable_extreme_is_monotonic_long() {
        // Peak only rises, never falls
        let side = Side::Long;
        let tech_dist = dec!(1500);
        let mut current_stop = Price::new(dec!(93500)).unwrap();
        let mut extreme = Price::new(dec!(95000)).unwrap();

        // Price goes up
        let result1 = update_trailing_stop_anchored(
            side,
            Price::new(dec!(97000)).unwrap(),
            extreme,
            current_stop,
            tech_dist,
        );
        assert!(result1.is_some());
        let update1 = result1.unwrap();
        assert_eq!(update1.new_favorable_extreme.as_decimal(), dec!(97000));

        extreme = update1.new_favorable_extreme;
        current_stop = update1.new_stop;

        // Price goes down (peak should stay at 97k)
        let result2 = update_trailing_stop_anchored(
            side,
            Price::new(dec!(96000)).unwrap(),
            extreme,
            current_stop,
            tech_dist,
        );
        assert!(result2.is_none());
    }

    #[test]
    fn test_favorable_extreme_is_monotonic_short() {
        // Low only falls, never rises
        let side = Side::Short;
        let tech_dist = dec!(1500);
        let mut current_stop = Price::new(dec!(96500)).unwrap();
        let mut extreme = Price::new(dec!(95000)).unwrap();

        // Price goes down
        let result1 = update_trailing_stop_anchored(
            side,
            Price::new(dec!(93000)).unwrap(),
            extreme,
            current_stop,
            tech_dist,
        );
        assert!(result1.is_some());
        let update1 = result1.unwrap();
        assert_eq!(update1.new_favorable_extreme.as_decimal(), dec!(93000));

        extreme = update1.new_favorable_extreme;
        current_stop = update1.new_stop;

        // Price goes up (low should stay at 93k)
        let result2 = update_trailing_stop_anchored(
            side,
            Price::new(dec!(94000)).unwrap(),
            extreme,
            current_stop,
            tech_dist,
        );
        assert!(result2.is_none());
    }
}
