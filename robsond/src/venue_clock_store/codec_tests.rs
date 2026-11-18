use robson_exec::{
    clock_records::{RawClockCapture, TICK_MAX},
    venue_clock::ClockDomain,
};
use sqlx::{Arguments, Execute};
use uuid::Uuid;

use super::codec::*;

#[test]
fn original_metadata_is_required_including_fractional_zero() {
    for (scale, fits) in [
        (None, None),
        (Some(1), Some(true)),
        (Some(0), None),
        (Some(0), Some(false)),
        (Some(28), Some(true)),
    ] {
        assert!(metadata(scale, fits).is_err(), "{scale:?} {fits:?}");
    }
    assert!(metadata(Some(0), Some(true)).is_ok());
}

#[test]
fn checked_decimal_preserves_full_integer_extrema() {
    for n in [
        0,
        1,
        TICK_MAX,
        robson_exec::clock_records::U2_MAX,
        robson_exec::clock_records::THETA_MIN,
    ] {
        let d = decimal(n, i128::MIN, i128::MAX).unwrap();
        assert_eq!(d.scale(), 0);
        assert_eq!(d.mantissa(), n);
    }
    assert!(decimal(TICK_MAX + 1, 0, TICK_MAX).is_err());
    assert!(decimal(-1, 0, TICK_MAX).is_err());
    assert!(decimal(i128::MAX, i128::MIN, i128::MAX).is_err());
}

#[test]
fn complete_and_absent_capture_use_exactly_nine_builtin_arguments() {
    let raw = RawClockCapture {
        epoch: Uuid::from_u128(1),
        wall_seconds: i64::MIN,
        wall_nanoseconds: 999_999_999,
        domain: ClockDomain::new(Uuid::from_u128(2), -1, 999_999_999).unwrap(),
        before_ns: TICK_MAX - 1,
        after_ns: TICK_MAX,
    };
    for capture in [Some(&raw), None] {
        let bind = CaptureBind::new(capture).unwrap();
        let mut query = bind.bind(sqlx::query("SELECT $1,$2,$3,$4,$5,$6,$7,$8,$9"));
        assert_eq!(query.take_arguments().unwrap().unwrap().len(), 9);
    }
    assert!(CaptureBind::new(Some(&RawClockCapture { after_ns: TICK_MAX + 1, ..raw })).is_err());
}
