use proptest::prelude::*;

use super::*;

fn domain() -> ClockDomain {
    ClockDomain::new(Uuid::from_u128(1), -2, 7).unwrap()
}

#[test]
fn unqualified_parts_preserve_extremes_and_refuse_malformed_values() {
    let d = domain();
    let last = TickCapture::new(d, i64::MAX, 999_999_999).unwrap();
    assert_eq!(last.nanoseconds(), (i64::MAX as u128) * 1_000_000_000 + 999_999_999);
    assert_eq!(
        last.elapsed_since(&TickCapture::new(d, 0, 0).unwrap()).unwrap(),
        Duration::new(i64::MAX as u64, 999_999_999)
    );
    for (s, n) in [(-1, 0), (0, -1), (0, 1_000_000_000)] {
        assert_eq!(TickCapture::new(d, s, n), Err(ClockError::InvalidTick));
    }
    assert!(ClockDomain::new(Uuid::nil(), 0, 0).is_err());
    assert!(ClockDomain::new(Uuid::from_u128(1), 0, 1_000_000_000).is_err());
    assert_eq!(d.offset(), (-2, 7));
    assert_eq!(d.boot(), Uuid::from_u128(1));
}

#[test]
fn equal_ticks_are_zero_but_reversal_and_different_domains_refuse() {
    let first = TickCapture::new(domain(), 100, 7).unwrap();
    assert_eq!(first.elapsed_since(&first), Ok(Duration::ZERO));
    assert_eq!(
        TickCapture::new(domain(), 100, 6).unwrap().elapsed_since(&first),
        Err(ClockError::Reversed)
    );
    for d in [
        ClockDomain::new(Uuid::from_u128(2), -2, 7).unwrap(),
        ClockDomain::new(Uuid::from_u128(1), -1, 7).unwrap(),
        ClockDomain::new(Uuid::from_u128(1), -2, 8).unwrap(),
    ] {
        assert_eq!(
            TickCapture::new(d, 200, 0).unwrap().elapsed_since(&first),
            Err(ClockError::Incomparable)
        );
    }
    // Persisted parts retain the original origin across process restart. A
    // simulated suspend advances BOOTTIME; no wall-clock value is an input.
    let restored = TickCapture::new(first.domain(), 100, 7).unwrap();
    assert_eq!(
        TickCapture::new(domain(), 86_500, 7).unwrap().elapsed_since(&restored),
        Ok(Duration::from_secs(86_400))
    );
}

proptest! {
    #[test]
    fn elapsed_matches_independent_signed_integer_oracle(
        a in 0i64..=i64::MAX, b in 0i64..=i64::MAX,
        an in 0i64..1_000_000_000, bn in 0i64..1_000_000_000,
        offset in any::<i64>(), ns in 0u32..1_000_000_000,
    ) {
        let d = ClockDomain::new(Uuid::from_u128(19), offset, ns).unwrap();
        let expected = (b as i128 - a as i128) * 1_000_000_000 + bn as i128 - an as i128;
        let actual = TickCapture::new(d, b, bn).unwrap()
            .elapsed_since(&TickCapture::new(d, a, an).unwrap());
        if expected < 0 { prop_assert_eq!(actual, Err(ClockError::Reversed)); }
        else { prop_assert_eq!(actual.unwrap().as_nanos(), expected as u128); }
    }
}
