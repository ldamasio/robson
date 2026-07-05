//! Property-based tests for discrete trailing under multi-source delivery
//! (ADR-0044 §5).
//!
//! The REST fallback (robsond `market_data`) can deliver prices concurrently
//! with a recovering WebSocket during mode transitions. These properties pin
//! the invariants that make that safe without any pipeline-level
//! deduplication:
//!
//! 1. **Source equivalence / order independence**: the final trailing stop
//!    depends only on the set of prices observed, not on their order,
//!    duplication, or which source delivered them. Discrete trailing is a pure
//!    function of the favorable extreme, and the extreme is a max/min.
//! 2. **Monotonicity**: the stop never regresses (never moves down for a long,
//!    never up for a short) under any delivery sequence.

use proptest::prelude::*;
use robson_domain::{Price, Side};
use robson_engine::trailing_stop::update_trailing_stop_discrete;
use rust_decimal::Decimal;

/// Fold a delivered price sequence through the discrete trailing update,
/// mirroring how the engine consumes MarketData: track the favorable extreme
/// and apply each accepted update.
fn fold_trailing(
    side: Side,
    entry: Decimal,
    initial_stop: Decimal,
    span: Decimal,
    prices: &[Decimal],
) -> (Decimal, Vec<Decimal>) {
    let entry_price = Price::new(entry).expect("valid entry");
    let mut stop = Price::new(initial_stop).expect("valid stop");
    let mut extreme = entry_price;
    let mut stop_path = Vec::new();

    for &p in prices {
        let price = Price::new(p).expect("valid price");
        if let Some(update) =
            update_trailing_stop_discrete(side, price, extreme, stop, entry_price, span)
        {
            stop = update.new_stop;
            extreme = update.new_favorable_extreme;
            stop_path.push(stop.as_decimal());
        } else {
            // Extreme still advances even when the stop does not move a full
            // span; mirror the engine's per-tick extreme tracking.
            match side {
                Side::Long => {
                    if price.as_decimal() > extreme.as_decimal() {
                        extreme = price;
                    }
                },
                Side::Short => {
                    if price.as_decimal() < extreme.as_decimal() {
                        extreme = price;
                    }
                },
            }
        }
    }
    (stop.as_decimal(), stop_path)
}

/// Strategy: an entry around 60k, a span between 50 and 2000, and a walk of
/// 1..60 prices within ±10 spans of entry (always positive).
fn scenario() -> impl Strategy<Value = (Decimal, Decimal, Vec<Decimal>)> {
    (50_000i64..70_000, 50i64..2_000).prop_flat_map(|(entry, span)| {
        let lo = entry - 10 * span;
        let hi = entry + 10 * span;
        (
            Just(Decimal::from(entry)),
            Just(Decimal::from(span)),
            proptest::collection::vec((lo.max(1)..hi).prop_map(Decimal::from), 1..60),
        )
    })
}

/// Interleave: duplicate every price (as if WS and REST both delivered it)
/// and append a shuffled replay of the whole series (late fallback catch-up).
fn duplicated_variant(prices: &[Decimal], seed_rotation: usize) -> Vec<Decimal> {
    let mut out = Vec::with_capacity(prices.len() * 3);
    for &p in prices {
        out.push(p);
        out.push(p); // concurrent duplicate delivery
    }
    // Rotated replay: same multiset, different order.
    let n = prices.len();
    for i in 0..n {
        out.push(prices[(i + seed_rotation) % n]);
    }
    out
}

proptest! {
    /// The final stop depends only on the observed price set: duplicating
    /// every delivery and replaying the series in a different order must not
    /// change the outcome (no double-applied steps, no order sensitivity).
    #[test]
    fn final_stop_is_delivery_invariant(
        (entry, span, prices) in scenario(),
        rotation in 0usize..59,
        long in proptest::bool::ANY,
    ) {
        let side = if long { Side::Long } else { Side::Short };
        let initial_stop = match side {
            Side::Long => entry - span,
            Side::Short => entry + span,
        };

        let (clean, _) = fold_trailing(side, entry, initial_stop, span, &prices);
        let noisy = duplicated_variant(&prices, rotation % prices.len().max(1));
        let (duplicated, _) = fold_trailing(side, entry, initial_stop, span, &noisy);

        prop_assert_eq!(
            clean,
            duplicated,
            "duplicated/interleaved delivery must converge to the same stop"
        );
    }

    /// The stop never regresses under any delivery sequence: monotonically
    /// non-decreasing for longs, non-increasing for shorts.
    #[test]
    fn stop_never_regresses(
        (entry, span, prices) in scenario(),
        long in proptest::bool::ANY,
    ) {
        let side = if long { Side::Long } else { Side::Short };
        let initial_stop = match side {
            Side::Long => entry - span,
            Side::Short => entry + span,
        };

        let (_, path) = fold_trailing(side, entry, initial_stop, span, &prices);
        let mut previous = initial_stop;
        for stop in path {
            match side {
                Side::Long => prop_assert!(
                    stop > previous,
                    "long stop must strictly advance upward (prev {previous}, got {stop})"
                ),
                Side::Short => prop_assert!(
                    stop < previous,
                    "short stop must strictly advance downward (prev {previous}, got {stop})"
                ),
            }
            previous = stop;
        }
    }
}
