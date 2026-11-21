//! Local value conversions only; real PgRow/consumer coverage requires PG.
use proptest::prelude::*;
use robson_exec::clock_records::{THETA_MAX, THETA_MIN, TICK_MAX, U2_MAX};

use super::*;

#[test]
fn exact_integer_boundaries_round_trip_without_decimal_decode() {
    for (lo, hi) in [(0, TICK_MAX), (THETA_MIN, THETA_MAX), (0, U2_MAX)] {
        for n in [lo, hi, 0] {
            assert_eq!(parse_integer(&n.to_string(), lo, hi), Ok(n));
        }
        assert!(parse_integer(&(lo - 1).to_string(), lo, hi).is_err());
        assert!(parse_integer(&(hi + 1).to_string(), lo, hi).is_err());
    }
    assert_eq!(parse_integer("-0", THETA_MIN, THETA_MAX), Ok(0));
}

#[test]
fn fractions_nonfinite_overflow_and_incomplete_strings_refuse() {
    for value in [
        "",
        " ",
        "1 ",
        " 1",
        "1.0",
        "0.1",
        "1e2",
        "NaN",
        "Infinity",
        "1x",
        "--1",
        "170141183460469231731687303715884105728",
        "0000000000000000000000000000000",
    ] {
        assert!(parse_integer(value, THETA_MIN, THETA_MAX).is_err(), "{value:?}");
    }
}

#[test]
fn checked_audit_conversion_preserves_epoch_and_microseconds() {
    for (pg, unix) in [
        (0, 946_684_800_000_000),
        (-946_684_800_000_000, 0),
        (-1, 946_684_799_999_999),
        (1, 946_684_800_000_001),
    ] {
        assert_eq!(timestamp(pg).unwrap().timestamp_micros(), unix);
    }
    assert!(timestamp(i64::MIN).is_err());
    assert!(timestamp(i64::MAX).is_err());
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 128, failure_persistence: None, ..ProptestConfig::default() })]

    #[test]
    fn original_signed_clock_range_round_trips(n in THETA_MIN..=THETA_MAX) {
        prop_assert_eq!(parse_integer(&n.to_string(), THETA_MIN, THETA_MAX), Ok(n));
    }

    #[test]
    fn finite_pg_audit_values_preserve_exact_microseconds(pg in -1_000_000_000_000_000i64..=1_000_000_000_000_000) {
        prop_assert_eq!(timestamp(pg).unwrap().timestamp_micros(), pg + 946_684_800_000_000);
    }
}
