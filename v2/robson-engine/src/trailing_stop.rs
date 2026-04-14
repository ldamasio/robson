//! Discrete Step Trailing Stop (v3 Policy)
//!
//! This module implements the v3 trailing stop using the "span" (palmo) technique.
//! The span is the technical stop distance calculated at position entry.
//!
//! # Algorithm (Discrete Step / Palmo)
//!
//! The stop moves in INTEGER MULTIPLES of the span, never continuously.
//!
//! For LONG:
//!   - entry = 95,000, stop_tecnico = 93,500, span = 1,500
//!   - price reaches 96,500 (entry + 1×span) → stop moves to 95,000 (entry - 0×span = breakeven)
//!   - price reaches 98,000 (entry + 2×span) → stop moves to 96,500
//!   - price reaches 99,500 (entry + 3×span) → stop moves to 98,000
//!   - price at 97,200 (between steps) → stop stays at 98,000 (no partial moves)
//!
//! # Key Invariants
//!
//! 1. Stop is MONOTONIC — never moves against the position
//! 2. Stop moves only in COMPLETE span steps — never reacts to partial movement
//! 3. Steps are anchored to ENTRY PRICE — not to current peak
//! 4. The system reacts to COMPLETE events only — never to "almost"

use robson_domain::{Price, Side};
use rust_decimal::Decimal;

/// Result of a trailing stop update
///
/// Contains the new stop price if an update occurred.
/// Returns `None` when no update is needed (price hasn't completed a new span step).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrailingStopUpdate {
    /// New trailing stop price
    pub new_stop: Price,
    /// New favorable extreme (peak for Long, low for Short)
    pub new_favorable_extreme: Price,
}

/// Update trailing stop using discrete step (span/palmo) logic
///
/// The stop moves in integer multiples of the span, anchored to entry price.
/// A new step is only completed when price advances a FULL span beyond the
/// previous step boundary.
///
/// # Arguments
///
/// * `side` - Position side (Long or Short)
/// * `current_price` - Current market price
/// * `favorable_extreme` - Current best price seen (peak for Long, low for Short)
/// * `current_trailing_stop` - Current trailing stop price
/// * `entry_price` - Entry price of the position (anchor for span steps)
/// * `span` - Technical stop distance (the "palmo" — unit of movement)
///
/// # Returns
///
/// * `Some(TrailingStopUpdate)` - When price completed a new span step
/// * `None` - When no update needed (price hasn't completed a full span step)
///
/// # Long Position Behavior
///
/// ```text
/// completed_spans = floor((peak - entry) / span)
/// candidate_stop = initial_stop + completed_spans × span
///
/// Only update if candidate_stop > current_trailing_stop
/// ```
///
/// # Short Position Behavior
///
/// ```text
/// completed_spans = floor((entry - low) / span)
/// candidate_stop = initial_stop - completed_spans × span
///
/// Only update if candidate_stop < current_trailing_stop
/// ```
///
/// # Examples
///
/// ```
/// # use robson_engine::trailing_stop::update_trailing_stop_discrete;
/// # use robson_domain::{Price, Side};
/// # use rust_decimal_macros::dec;
/// // LONG: entry $95,000, stop $93,500, span $1,500
/// let side = Side::Long;
/// let entry = Price::new(dec!(95000)).unwrap();
/// let initial_stop = Price::new(dec!(93500)).unwrap();
/// let span = dec!(1500);
///
/// // Price at $96,000 — NOT a full span above entry. No update.
/// let result = update_trailing_stop_discrete(
///     side,
///     Price::new(dec!(96000)).unwrap(),
///     Price::new(dec!(96000)).unwrap(),
///     initial_stop,
///     entry,
///     span,
/// );
/// assert!(result.is_none());
///
/// // Price at $96,500 — exactly 1 full span above entry. Stop moves to $95,000.
/// let result = update_trailing_stop_discrete(
///     side,
///     Price::new(dec!(96500)).unwrap(),
///     Price::new(dec!(96500)).unwrap(),
///     initial_stop,
///     entry,
///     span,
/// );
/// assert!(result.is_some());
/// let update = result.unwrap();
/// assert_eq!(update.new_stop.as_decimal(), dec!(95000));
/// ```
pub fn update_trailing_stop_discrete(
    side: Side,
    current_price: Price,
    favorable_extreme: Price,
    current_trailing_stop: Price,
    entry_price: Price,
    span: Decimal,
) -> Option<TrailingStopUpdate> {
    if span <= Decimal::ZERO {
        return None;
    }

    match side {
        Side::Long => {
            // Track the peak
            let new_peak = if current_price.as_decimal() > favorable_extreme.as_decimal() {
                current_price
            } else {
                favorable_extreme
            };

            // How many complete spans has price moved above entry?
            let profit_distance = new_peak.as_decimal() - entry_price.as_decimal();
            if profit_distance <= Decimal::ZERO {
                return None;
            }

            // Integer division: floor(profit_distance / span)
            let completed_spans = (profit_distance / span).floor();
            if completed_spans <= Decimal::ZERO {
                return None;
            }

            // initial_stop = entry - span, so:
            // candidate_stop = initial_stop + completed_spans × span
            //                = (entry - span) + completed_spans × span
            let initial_stop = entry_price.as_decimal() - span;
            let candidate_stop = initial_stop + completed_spans * span;

            // Only update if candidate is HIGHER than current stop (monotonic)
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
            // Track the low
            let new_low = if current_price.as_decimal() < favorable_extreme.as_decimal() {
                current_price
            } else {
                favorable_extreme
            };

            // How many complete spans has price moved below entry?
            let profit_distance = entry_price.as_decimal() - new_low.as_decimal();
            if profit_distance <= Decimal::ZERO {
                return None;
            }

            let completed_spans = (profit_distance / span).floor();
            if completed_spans <= Decimal::ZERO {
                return None;
            }

            // initial_stop = entry + span, so:
            // candidate_stop = initial_stop - completed_spans × span
            let initial_stop = entry_price.as_decimal() + span;
            let candidate_stop = initial_stop - completed_spans * span;

            // Only update if candidate is LOWER than current stop (monotonic)
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
/// # use robson_engine::trailing_stop::is_trailing_stop_hit;
/// # use robson_domain::{Price, Side};
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

    // Helper: standard LONG position setup
    // entry = $95,000, stop = $93,500, span = $1,500
    fn long_setup() -> (Side, Price, Price, Price, Decimal) {
        (
            Side::Long,
            Price::new(dec!(95000)).unwrap(), // entry
            Price::new(dec!(93500)).unwrap(), // initial stop
            Price::new(dec!(95000)).unwrap(), // initial extreme = entry
            dec!(1500),                       // span
        )
    }

    // Helper: standard SHORT position setup
    // entry = $95,000, stop = $96,500, span = $1,500
    fn short_setup() -> (Side, Price, Price, Price, Decimal) {
        (
            Side::Short,
            Price::new(dec!(95000)).unwrap(), // entry
            Price::new(dec!(96500)).unwrap(), // initial stop
            Price::new(dec!(95000)).unwrap(), // initial extreme = entry
            dec!(1500),                       // span
        )
    }

    // =========================================================================
    // LONG: Discrete Step Trailing
    // =========================================================================

    #[test]
    fn test_long_no_update_below_first_span() {
        let (side, entry, stop, extreme, span) = long_setup();

        // Price at $96,000 — less than 1 full span above entry ($96,500 needed)
        let result = update_trailing_stop_discrete(
            side,
            Price::new(dec!(96000)).unwrap(),
            extreme,
            stop,
            entry,
            span,
        );
        assert!(result.is_none());
    }

    #[test]
    fn test_long_no_update_just_below_span_boundary() {
        let (side, entry, stop, extreme, span) = long_setup();

        // Price at $96,499 — almost 1 span but not complete
        let result = update_trailing_stop_discrete(
            side,
            Price::new(dec!(96499)).unwrap(),
            extreme,
            stop,
            entry,
            span,
        );
        assert!(result.is_none());
    }

    #[test]
    fn test_long_update_at_exact_span_boundary() {
        let (side, entry, stop, extreme, span) = long_setup();

        // Price at $96,500 — exactly 1 span above entry
        let result = update_trailing_stop_discrete(
            side,
            Price::new(dec!(96500)).unwrap(),
            extreme,
            stop,
            entry,
            span,
        );
        assert!(result.is_some());
        let update = result.unwrap();
        // Stop moves to $95,000 (breakeven = initial_stop + 1×span)
        assert_eq!(update.new_stop.as_decimal(), dec!(95000));
    }

    #[test]
    fn test_long_full_sequence_from_spec() {
        // The canonical example from the v3 policy:
        // entry=95000, stop=93500, span=1500
        let (side, entry, _, _, span) = long_setup();
        let mut stop = Price::new(dec!(93500)).unwrap();
        let mut extreme = Price::new(dec!(95000)).unwrap();

        // Price 96,500 → stop 95,000
        let r = update_trailing_stop_discrete(
            side,
            Price::new(dec!(96500)).unwrap(),
            extreme,
            stop,
            entry,
            span,
        )
        .unwrap();
        assert_eq!(r.new_stop.as_decimal(), dec!(95000));
        stop = r.new_stop;
        extreme = r.new_favorable_extreme;

        // Price 98,000 → stop 96,500
        let r = update_trailing_stop_discrete(
            side,
            Price::new(dec!(98000)).unwrap(),
            extreme,
            stop,
            entry,
            span,
        )
        .unwrap();
        assert_eq!(r.new_stop.as_decimal(), dec!(96500));
        stop = r.new_stop;
        extreme = r.new_favorable_extreme;

        // Price 99,500 → stop 98,000
        let r = update_trailing_stop_discrete(
            side,
            Price::new(dec!(99500)).unwrap(),
            extreme,
            stop,
            entry,
            span,
        )
        .unwrap();
        assert_eq!(r.new_stop.as_decimal(), dec!(98000));
        stop = r.new_stop;
        extreme = r.new_favorable_extreme;

        // Price drops to 97,200 → no update (stop stays at 98,000)
        let r = update_trailing_stop_discrete(
            side,
            Price::new(dec!(97200)).unwrap(),
            extreme,
            stop,
            entry,
            span,
        );
        assert!(r.is_none());
        // Position should be closed by is_trailing_stop_hit (97,200 < 98,000)
        assert!(is_trailing_stop_hit(
            Side::Long,
            Price::new(dec!(97200)).unwrap(),
            stop
        ));
    }

    #[test]
    fn test_long_price_drops_near_stop_recovers_to_entry_no_action() {
        // v3 behavioral rule: price almost hits stop, doesn't, returns to entry.
        // Robson does NOTHING. Stop stays at original.
        let (side, entry, stop, extreme, span) = long_setup();

        // Price drops to $93,600 (near stop of $93,500 but doesn't hit)
        let r = update_trailing_stop_discrete(
            side,
            Price::new(dec!(93600)).unwrap(),
            extreme,
            stop,
            entry,
            span,
        );
        assert!(r.is_none());
        assert!(!is_trailing_stop_hit(
            Side::Long,
            Price::new(dec!(93600)).unwrap(),
            stop
        ));

        // Price recovers to entry ($95,000)
        let r = update_trailing_stop_discrete(
            side,
            Price::new(dec!(95000)).unwrap(),
            extreme,
            stop,
            entry,
            span,
        );
        assert!(r.is_none()); // No update — haven't completed a span of profit
    }

    #[test]
    fn test_long_monotonic_stop_never_goes_down() {
        let (side, entry, _, _, span) = long_setup();
        let mut stop = Price::new(dec!(93500)).unwrap();
        let mut extreme = Price::new(dec!(95000)).unwrap();

        // Price to 96,500 → stop to 95,000
        let r = update_trailing_stop_discrete(
            side,
            Price::new(dec!(96500)).unwrap(),
            extreme,
            stop,
            entry,
            span,
        )
        .unwrap();
        stop = r.new_stop;
        extreme = r.new_favorable_extreme;
        assert_eq!(stop.as_decimal(), dec!(95000));

        // Price drops back — stop must NOT go down
        let r = update_trailing_stop_discrete(
            side,
            Price::new(dec!(95500)).unwrap(),
            extreme,
            stop,
            entry,
            span,
        );
        assert!(r.is_none()); // Stop stays at 95,000
    }

    #[test]
    fn test_long_large_jump_skips_intermediate_spans() {
        let (side, entry, stop, extreme, span) = long_setup();

        // Price jumps directly to $99,500 (3 spans above entry)
        let r = update_trailing_stop_discrete(
            side,
            Price::new(dec!(99500)).unwrap(),
            extreme,
            stop,
            entry,
            span,
        )
        .unwrap();
        // 3 completed spans → stop = 93500 + 3×1500 = 98000
        assert_eq!(r.new_stop.as_decimal(), dec!(98000));
    }

    #[test]
    fn test_long_between_spans_no_update() {
        let (side, entry, _, _, span) = long_setup();

        // Already at 1 span, stop at 95,000
        let stop = Price::new(dec!(95000)).unwrap();
        let extreme = Price::new(dec!(96500)).unwrap();

        // Price at $97,500 — between span 1 boundary (96,500) and span 2 boundary (98,000)
        let r = update_trailing_stop_discrete(
            side,
            Price::new(dec!(97500)).unwrap(),
            extreme,
            stop,
            entry,
            span,
        );
        assert!(r.is_none()); // Not a full span yet
    }

    // =========================================================================
    // SHORT: Discrete Step Trailing
    // =========================================================================

    #[test]
    fn test_short_no_update_above_first_span() {
        let (side, entry, stop, extreme, span) = short_setup();

        // Price at $94,000 — less than 1 full span below entry ($93,500 needed)
        let result = update_trailing_stop_discrete(
            side,
            Price::new(dec!(94000)).unwrap(),
            extreme,
            stop,
            entry,
            span,
        );
        assert!(result.is_none());
    }

    #[test]
    fn test_short_update_at_exact_span_boundary() {
        let (side, entry, stop, extreme, span) = short_setup();

        // Price at $93,500 — exactly 1 span below entry
        let result = update_trailing_stop_discrete(
            side,
            Price::new(dec!(93500)).unwrap(),
            extreme,
            stop,
            entry,
            span,
        );
        assert!(result.is_some());
        let update = result.unwrap();
        // Stop moves to $95,000 (breakeven = initial_stop - 1×span)
        assert_eq!(update.new_stop.as_decimal(), dec!(95000));
    }

    #[test]
    fn test_short_full_sequence() {
        let (side, entry, _, _, span) = short_setup();
        let mut stop = Price::new(dec!(96500)).unwrap();
        let mut extreme = Price::new(dec!(95000)).unwrap();

        // Price 93,500 → stop 95,000
        let r = update_trailing_stop_discrete(
            side,
            Price::new(dec!(93500)).unwrap(),
            extreme,
            stop,
            entry,
            span,
        )
        .unwrap();
        assert_eq!(r.new_stop.as_decimal(), dec!(95000));
        stop = r.new_stop;
        extreme = r.new_favorable_extreme;

        // Price 92,000 → stop 93,500
        let r = update_trailing_stop_discrete(
            side,
            Price::new(dec!(92000)).unwrap(),
            extreme,
            stop,
            entry,
            span,
        )
        .unwrap();
        assert_eq!(r.new_stop.as_decimal(), dec!(93500));
        stop = r.new_stop;
        extreme = r.new_favorable_extreme;

        // Price 90,500 → stop 92,000
        let r = update_trailing_stop_discrete(
            side,
            Price::new(dec!(90500)).unwrap(),
            extreme,
            stop,
            entry,
            span,
        )
        .unwrap();
        assert_eq!(r.new_stop.as_decimal(), dec!(92000));
        stop = r.new_stop;
        extreme = r.new_favorable_extreme;

        // Price rises to 92,800 → no update, but stop hit
        let r = update_trailing_stop_discrete(
            side,
            Price::new(dec!(92800)).unwrap(),
            extreme,
            stop,
            entry,
            span,
        );
        assert!(r.is_none());
        assert!(is_trailing_stop_hit(
            Side::Short,
            Price::new(dec!(92800)).unwrap(),
            stop
        ));
    }

    #[test]
    fn test_short_monotonic_stop_never_goes_up() {
        let (side, entry, _, _, span) = short_setup();
        let mut stop = Price::new(dec!(96500)).unwrap();
        let mut extreme = Price::new(dec!(95000)).unwrap();

        // Price to 93,500 → stop to 95,000
        let r = update_trailing_stop_discrete(
            side,
            Price::new(dec!(93500)).unwrap(),
            extreme,
            stop,
            entry,
            span,
        )
        .unwrap();
        stop = r.new_stop;
        extreme = r.new_favorable_extreme;
        assert_eq!(stop.as_decimal(), dec!(95000));

        // Price rises back — stop must NOT go up
        let r = update_trailing_stop_discrete(
            side,
            Price::new(dec!(94500)).unwrap(),
            extreme,
            stop,
            entry,
            span,
        );
        assert!(r.is_none());
    }

    // =========================================================================
    // Stop Hit Tests (unchanged — same logic)
    // =========================================================================

    #[test]
    fn test_long_stop_hit_at_or_below() {
        let stop = Price::new(dec!(95000)).unwrap();
        assert!(is_trailing_stop_hit(
            Side::Long,
            Price::new(dec!(95000)).unwrap(),
            stop
        ));
        assert!(is_trailing_stop_hit(
            Side::Long,
            Price::new(dec!(94900)).unwrap(),
            stop
        ));
        assert!(!is_trailing_stop_hit(
            Side::Long,
            Price::new(dec!(95100)).unwrap(),
            stop
        ));
    }

    #[test]
    fn test_short_stop_hit_at_or_above() {
        let stop = Price::new(dec!(95000)).unwrap();
        assert!(is_trailing_stop_hit(
            Side::Short,
            Price::new(dec!(95000)).unwrap(),
            stop
        ));
        assert!(is_trailing_stop_hit(
            Side::Short,
            Price::new(dec!(95100)).unwrap(),
            stop
        ));
        assert!(!is_trailing_stop_hit(
            Side::Short,
            Price::new(dec!(94900)).unwrap(),
            stop
        ));
    }

    // =========================================================================
    // Edge Cases
    // =========================================================================

    #[test]
    fn test_zero_span_returns_none() {
        let (side, entry, stop, extreme, _) = long_setup();
        let result = update_trailing_stop_discrete(
            side,
            Price::new(dec!(98000)).unwrap(),
            extreme,
            stop,
            entry,
            dec!(0),
        );
        assert!(result.is_none());
    }

    #[test]
    fn test_negative_span_returns_none() {
        let (side, entry, stop, extreme, _) = long_setup();
        let result = update_trailing_stop_discrete(
            side,
            Price::new(dec!(98000)).unwrap(),
            extreme,
            stop,
            entry,
            dec!(-1500),
        );
        assert!(result.is_none());
    }

    #[test]
    fn test_price_at_entry_no_update() {
        let (side, entry, stop, extreme, span) = long_setup();
        let result = update_trailing_stop_discrete(
            side, entry, // price exactly at entry
            extreme, stop, entry, span,
        );
        assert!(result.is_none()); // zero profit distance
    }

    #[test]
    fn test_price_below_entry_long_no_update() {
        let (side, entry, stop, extreme, span) = long_setup();
        let result = update_trailing_stop_discrete(
            side,
            Price::new(dec!(94000)).unwrap(), // below entry
            extreme,
            stop,
            entry,
            span,
        );
        assert!(result.is_none()); // negative profit distance
    }

    #[test]
    fn test_favorable_extreme_tracked_correctly() {
        let (side, entry, stop, extreme, span) = long_setup();

        // Price hits 96,500 — first span
        let r = update_trailing_stop_discrete(
            side,
            Price::new(dec!(96500)).unwrap(),
            extreme,
            stop,
            entry,
            span,
        )
        .unwrap();
        assert_eq!(r.new_favorable_extreme.as_decimal(), dec!(96500));

        // Price drops to 95,500 — extreme should remain at 96,500
        let r2 = update_trailing_stop_discrete(
            side,
            Price::new(dec!(95500)).unwrap(),
            r.new_favorable_extreme,
            r.new_stop,
            entry,
            span,
        );
        assert!(r2.is_none()); // Peak was already 96,500, price didn't exceed it
    }
}
