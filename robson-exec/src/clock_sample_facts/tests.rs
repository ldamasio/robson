use super::*;
use crate::venue_clock::ClockDomain;

fn epoch() -> Uuid {
    Uuid::from_u128(7)
}
fn tick(ns: i128) -> TickCapture {
    TickCapture::new(
        ClockDomain::new(Uuid::from_u128(1), 0, 0).unwrap(),
        (ns / SECOND).try_into().unwrap(),
        (ns % SECOND).try_into().unwrap(),
    )
    .unwrap()
}
fn mark(wall: i128, low: i128, high: i128) -> WallMarkCandidate {
    WallMarkCandidate::new(
        epoch(),
        wall.div_euclid(SECOND).try_into().unwrap(),
        wall.rem_euclid(SECOND).try_into().unwrap(),
        tick(low),
        tick(high),
    )
    .unwrap()
}
fn sample(
    send: WallMarkCandidate,
    receive: WallMarkCandidate,
    server: i64,
) -> Result<SampleFacts, FactsError> {
    let venue = VenueId::new(VenueId::BINANCE_SPOT);
    SampleFacts::calculate(SampleCandidate {
        id: Uuid::from_u128(2),
        tenant: Uuid::from_u128(3),
        venue: &venue,
        epoch: epoch(),
        send,
        receive,
        server_ms: server,
    })
}
fn zero() -> SampleFacts {
    sample(mark(0, 0, 0), mark(0, 0, 0), 0).unwrap()
}

#[test]
fn sample_bound_preserves_signed_half_nanoseconds_and_exact_threshold() {
    let at = sample(mark(0, 0, 0), mark(0, 0, 0), 900).unwrap();
    assert_eq!(at.twice_offset_ns(), 1_800_000_000);
    assert_eq!(at.twice_bound_ns(), 2 * SECOND);
    assert_eq!(at.bound_ns_ceil(), Ok(SECOND));
    assert!(at.fits_bound());
    let over = sample(mark(0, 0, 0), mark(0, 1, 1), 900).unwrap();
    assert_eq!(over.twice_bound_ns(), 2 * SECOND + 1);
    assert_eq!(over.bound_ns_ceil(), Ok(SECOND + 1));
    assert!(!over.fits_bound());
    assert!(over.fits_discrepancy());
    let negative = sample(mark(1, 0, 0), mark(2, 1, 1), 0).unwrap();
    assert_eq!(negative.twice_offset_ns(), -3);
    assert_eq!(negative.twice_bound_ns(), 200_000_004);
    assert_eq!(negative.bound_ns_ceil(), Ok(100_000_002));
    let wide = sample(mark(0, 0, 1), mark(0, 1, 2), 900).unwrap();
    assert_eq!(wide.rtt_ns(), (0, 2));
    assert_eq!(wide.twice_bound_ns(), 2 * SECOND + 2);
    assert!(!wide.fits_bound(), "RTT upper endpoint, never lower or midpoint");
}

#[test]
fn every_discrepancy_endpoint_is_checked_before_millisecond_rounding() {
    for delta in [DISCREPANCY, DISCREPANCY + 1] {
        let exact = sample(mark(0, 0, 0), mark(delta, 0, 0), 0).unwrap();
        assert_eq!(exact.fits_discrepancy(), delta == DISCREPANCY);
        let negative = sample(mark(0, 0, 0), mark(0, delta, delta), 0).unwrap();
        assert_eq!(negative.fits_discrepancy(), delta == DISCREPANCY);
        for (wall, low, high) in [(0, 0, delta), (delta, 0, delta)] {
            let uncertain = sample(mark(0, 0, 0), mark(wall, low, high), 0).unwrap();
            assert_eq!(uncertain.fits_discrepancy(), delta == DISCREPANCY);
        }
    }
    let invalid = sample(mark(0, 0, 0), mark(0, 0, DISCREPANCY + 1), 0).unwrap();
    let facts = invalid.compare(&mark(0, DISCREPANCY + 1, DISCREPANCY + 1)).unwrap();
    assert!(!facts.fits_sample(), "later capture cannot repair a bad sample");
}

#[test]
fn capture_uses_original_receive_upper_age_and_exact_discrepancy() {
    let sample = zero();
    for age in [60 * SECOND - 1, 60 * SECOND, 60 * SECOND + 1] {
        let facts = sample.compare(&mark(age, age, age)).unwrap();
        assert_eq!(facts.age_ns(), (age, age));
        assert_eq!(facts.wall_delta_ns(), age);
        assert_eq!(facts.fits_age(), age <= 60 * SECOND);
        assert!(facts.fits_discrepancy());
        assert!(facts.fits_sample());
        assert_eq!(facts.sample_id(), Uuid::from_u128(2));
        assert_eq!(facts.twice_bound_ns(), sample.twice_bound_ns());
    }
    let broad = sample.compare(&mark(60 * SECOND, 60 * SECOND, 60 * SECOND + 1)).unwrap();
    assert!(!broad.fits_age(), "lower age cannot certify the upper interval");
    for delta in [DISCREPANCY, DISCREPANCY + 1] {
        for wall in [SECOND - delta, SECOND + delta] {
            let facts = sample.compare(&mark(wall, SECOND, SECOND)).unwrap();
            assert_eq!(facts.fits_discrepancy(), delta == DISCREPANCY);
        }
        for (wall, low, high) in [
            (SECOND, SECOND, SECOND + delta),
            (SECOND + delta, SECOND, SECOND + delta),
        ] {
            let facts = sample.compare(&mark(wall, low, high)).unwrap();
            assert_eq!(facts.fits_discrepancy(), delta == DISCREPANCY);
        }
    }
    let first = sample.compare(&mark(59 * SECOND, 59 * SECOND, 59 * SECOND)).unwrap();
    assert!(first.fits_age());
    assert!(!sample.compare(&mark(61 * SECOND, 61 * SECOND, 61 * SECOND)).unwrap().fits_age());
    assert_eq!(sample.original_marks(), (mark(0, 0, 0), mark(0, 0, 0)));
}

#[test]
fn identity_order_and_wall_reversal_cannot_become_numerical_proof() {
    let good = mark(0, 0, 0);
    let venue = VenueId::new(VenueId::BINANCE_SPOT);
    let wrong = VenueId::new("binance:usdm");
    for which in 0..6 {
        let mut input = SampleCandidate {
            id: Uuid::from_u128(2),
            tenant: Uuid::from_u128(3),
            venue: &venue,
            epoch: epoch(),
            send: good,
            receive: good,
            server_ms: 0,
        };
        match which {
            0 => input.id = Uuid::nil(),
            1 => input.tenant = Uuid::nil(),
            2 => input.venue = &wrong,
            3 => input.epoch = Uuid::nil(),
            4 => input.send.epoch = Uuid::from_u128(8),
            _ => input.receive.epoch = Uuid::from_u128(8),
        }
        assert_eq!(SampleFacts::calculate(input), Err(FactsError::Identity));
    }
    assert_eq!(sample(good, good, -1), Err(FactsError::Timestamp));
    assert_eq!(sample(mark(1, 0, 0), mark(0, 1, 1), 0), Err(FactsError::ReversedWall));
    assert_eq!(
        sample(mark(0, 0, 2), mark(1, 1, 3), 0),
        Err(FactsError::Clock(ClockError::Reversed))
    );
    assert_eq!(zero().compare(&mark(-1, 1, 1)), Err(FactsError::ReversedWall));
    let future = sample(mark(10, 10, 10), mark(11, 11, 11), 0).unwrap();
    assert_eq!(future.compare(&mark(9, 9, 9)), Err(FactsError::Clock(ClockError::Reversed)));
    for different in [
        ClockDomain::new(Uuid::from_u128(9), 0, 0).unwrap(),
        ClockDomain::new(Uuid::from_u128(1), 0, 1).unwrap(),
    ] {
        let other = TickCapture::new(different, 0, 0).unwrap();
        assert_eq!(
            WallMarkCandidate::new(epoch(), 0, 0, tick(0), other),
            Err(FactsError::Clock(ClockError::Incomparable))
        );
        let changed = WallMarkCandidate::new(epoch(), 0, 0, other, other).unwrap();
        assert_eq!(zero().compare(&changed), Err(FactsError::Clock(ClockError::Incomparable)));
    }
    let mut next = good;
    next.epoch = Uuid::from_u128(8);
    assert_eq!(
        zero().compare(&next),
        Err(FactsError::Identity),
        "same boot cannot import a prior process C8 sample"
    );
    assert_eq!(
        WallMarkCandidate::new(epoch(), 0, 0, tick(1), tick(0)),
        Err(FactsError::Clock(ClockError::Reversed))
    );
    assert_eq!(
        WallMarkCandidate::new(Uuid::nil(), 0, 0, tick(0), tick(0)),
        Err(FactsError::Identity)
    );
    assert_eq!(
        WallMarkCandidate::new(epoch(), 0, 1_000_000_000, tick(0), tick(0)),
        Err(FactsError::Timestamp)
    );
}

#[test]
fn accepted_native_extremes_do_not_wrap_or_lose_original_identity() {
    let maximum = i128::from(i64::MAX) * SECOND + 999_999_999;
    let minimum = i128::from(i64::MIN) * SECOND;
    for wall in [minimum, -1, 0, maximum] {
        let s =
            sample(mark(wall, maximum, maximum), mark(wall, maximum, maximum), i64::MAX).unwrap();
        let theta = 2 * i128::from(i64::MAX) * 1_000_000 - 2 * wall;
        assert_eq!(s.twice_offset_ns(), theta);
        assert_eq!(s.twice_bound_ns(), theta.abs() + 200_000_000);
        assert_eq!(s.identity(), (Uuid::from_u128(2), Uuid::from_u128(3), epoch()));
        assert!(s.fits_discrepancy());
    }
    let huge = sample(mark(minimum, 0, 0), mark(maximum, maximum, maximum), i64::MAX).unwrap();
    assert!(!huge.fits_discrepancy());
    assert!(!huge.fits_bound());
    // Private corrupted-state seam tests checked failure, not an input API.
    let mut impossible = zero();
    impossible.u2 = i128::MAX;
    assert_eq!(impossible.bound_ns_ceil(), Err(FactsError::Range));
    assert_eq!(discrepancy(i128::MIN, (1, 1)), Err(FactsError::Range));
    assert_eq!(discrepancy(i128::MIN, (0, 0)), Err(FactsError::Range));
    assert!(std::mem::size_of::<SampleFacts>() <= 384);
    assert!(std::mem::size_of::<CaptureFacts>() <= 96);
    eprintln!(
        "C8 type sizes: mark={} sample={} capture={}",
        std::mem::size_of::<WallMarkCandidate>(),
        std::mem::size_of::<SampleFacts>(),
        std::mem::size_of::<CaptureFacts>()
    );
}

proptest::proptest! {
    #![proptest_config(proptest::test_runner::Config::with_cases(256))]
    #[test]
    fn independent_integer_intervals_match_all_numerical_facts(
        wall0 in -1_000_000_000_000_i64..1_000_000_000_000,
        wall_span in 0_i64..200_000_000,
        send_width in 0_i64..2_000_000,
        travel in 0_i64..150_000_000,
        receive_width in 0_i64..2_000_000,
        server in 0_i64..2_000_000,
        age in 0_i64..61_000_000_000,
        later_width in 0_i64..2_000_000,
        later_skew in -60_000_000_i64..60_000_000,
    ) {
        let(w,span,sw,tr,rw,a,lw,sk)=(i128::from(wall0),i128::from(wall_span),i128::from(send_width),i128::from(travel),i128::from(receive_width),i128::from(age),i128::from(later_width),i128::from(later_skew));
        let rbegin=sw+tr;let rend=rbegin+rw;
        let s=sample(mark(w,0,sw),mark(w+span,rbegin,rend),server).unwrap();
        let theta=2*i128::from(server)*1_000_000-2*w-span;
        let bound=theta.abs()+rend+200_000_000;
        proptest::prop_assert_eq!(s.rtt_ns(),(tr,rend));
        proptest::prop_assert_eq!(s.twice_offset_ns(),theta);
        proptest::prop_assert_eq!(s.twice_bound_ns(),bound);
        proptest::prop_assert_eq!(s.bound_ns_ceil().unwrap(),bound/2+bound%2);
        proptest::prop_assert_eq!(s.fits_bound(),bound<=2_000_000_000);
        let sample_d=(span-tr).abs().max((span-rend).abs())<=50_000_000;
        proptest::prop_assert_eq!(s.fits_discrepancy(),sample_d);
        let delta=a+sk;
        let comparison=s.compare(&mark(w+span+delta,rend+a,rend+a+lw));
        if delta<0 { proptest::prop_assert_eq!(comparison,Err(FactsError::ReversedWall)); }
        else {
            let c=comparison.unwrap();let upper=rw+a+lw;
            proptest::prop_assert_eq!(c.age_ns(),(a,upper));
            proptest::prop_assert_eq!(c.wall_delta_ns(),delta);
            proptest::prop_assert_eq!(c.fits_sample(),bound<=2_000_000_000&&sample_d);
            proptest::prop_assert_eq!(c.fits_age(),upper<=60_000_000_000);
            proptest::prop_assert_eq!(c.fits_discrepancy(),(delta-a).abs().max((delta-upper).abs())<=50_000_000);
        }
    }
}
