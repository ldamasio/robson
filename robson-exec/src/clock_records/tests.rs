use proptest::prelude::*;

use super::*;

fn sample() -> RawClockSample {
    let domain = ClockDomain::new(Uuid::from_u128(1), -1, 999_999_999).unwrap();
    let send = RawClockCapture {
        epoch: Uuid::from_u128(2),
        wall_seconds: 1,
        wall_nanoseconds: 0,
        domain,
        before_ns: 0,
        after_ns: 0,
    };
    RawClockSample {
        id: Uuid::from_u128(3),
        tenant: Uuid::from_u128(4),
        venue: VenueId::new(VenueId::BINANCE_SPOT),
        send,
        receive: send,
        server_ms: 1000,
    }
}

#[test]
fn original_response_and_full_domain_survive_recomputation() {
    let mut s = sample();
    s.server_ms = 1001;
    s.receive.wall_nanoseconds = 1;
    s.receive.after_ns = 1;
    let before = s.clone();
    let numbers = s.numbers().unwrap();
    assert_eq!(s, before);
    assert_eq!(s.server_ms, 1001);
    assert_eq!(numbers.theta2, 1_999_999);
    assert_eq!(numbers.r_low, 0);
    assert_eq!(numbers.r_high, 1);
    assert_eq!(numbers.u2, 202_000_000);
    assert_eq!(numbers.failures, 0);
    assert_eq!(s.receive.domain.offset(), (-1, 999_999_999));
}

#[test]
fn capture_representation_refuses_bad_components_without_saturation() {
    let original = sample().send;
    for invalid in [
        RawClockCapture { before_ns: -1, ..original },
        RawClockCapture { after_ns: TICK_MAX + 1, ..original },
        RawClockCapture { before_ns: 1, after_ns: 0, ..original },
        RawClockCapture {
            wall_nanoseconds: 1_000_000_000,
            ..original
        },
        RawClockCapture { epoch: Uuid::nil(), ..original },
    ] {
        assert!(invalid.check().is_err());
    }
    for wall_seconds in [i64::MIN, i64::MAX] {
        RawClockCapture {
            wall_seconds,
            wall_nanoseconds: 999_999_999,
            before_ns: TICK_MAX,
            after_ns: TICK_MAX,
            ..original
        }
        .check()
        .unwrap();
    }
}

#[test]
fn numerical_failure_is_retained_as_diagnostics() {
    let mut s = sample();
    s.server_ms = i64::MAX;
    assert_eq!(s.numbers().unwrap().failures, 1);
    s.receive.after_ns = 50_000_001;
    assert_eq!(s.numbers().unwrap().failures, 3);
    s.server_ms = -1;
    assert!(s.numbers().is_err());
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]
    #[test]
    fn exact_raw_sample_agrees_with_independent_integer_oracle(
        wall in any::<i64>(), nanos in 0u32..1_000_000_000,
        server in 0i64..=i64::MAX, rtt in 0i128..=100_000_001,
    ) {
        let mut s = sample();
        s.send.wall_seconds = wall; s.send.wall_nanoseconds = nanos;
        s.receive = s.send; s.receive.after_ns = rtt; s.server_ms = server;
        let n = s.numbers().unwrap();
        let wall_ns = i128::from(wall)*1_000_000_000+i128::from(nanos);
        let theta = i128::from(server)*2_000_000-2*wall_ns;
        let u2 = theta.abs()+rtt+200_000_000;
        prop_assert_eq!((n.theta2,n.r_low,n.r_high,n.u2),(theta,0,rtt,u2));
        prop_assert_eq!(n.failures,i16::from(u2>2_000_000_000)|(i16::from(rtt>50_000_000)<<1));
        prop_assert!((THETA_MIN..=THETA_MAX).contains(&theta));
        prop_assert!(u2<=U2_MAX);
    }
}
