use std::mem::size_of;

use proptest::prelude::*;
use uuid::Uuid;

use super::*;
use crate::venue_clock::{ClockDomain, ClockError, TickCapture};

const SECOND: u128 = 1_000_000_000;

fn domain() -> ClockDomain {
    ClockDomain::new(Uuid::from_u128(1), 0, 0).unwrap()
}
fn tick(nanos: u128) -> TickCapture {
    TickCapture::new(domain(), (nanos / SECOND).try_into().unwrap(), (nanos % SECOND) as i64)
        .unwrap()
}
fn cost(weight: u32, orders: u32) -> RequestCostCandidate {
    RequestCostCandidate::new(weight, orders).unwrap()
}
fn no_startup_cap() -> StartupCapsFacts {
    restart_cap_candidates(&tick(0), &tick(60 * SECOND), EpochHistoryCandidate::UnknownPriorHistory)
        .unwrap()
}
fn pending_usage(weight: u32, orders: usize) -> UsageFacts {
    let mut charges = vec![ChargeCandidate::Pending(cost(1, 1)); orders];
    if weight > orders as u32 {
        charges.push(ChargeCandidate::Pending(cost(weight - orders as u32, 0)));
    }
    local_usage_candidates(&charges, &tick(0)).unwrap()
}

#[test]
fn pending_never_ages_and_sent_uses_handoff() {
    let pending = ChargeCandidate::Pending(cost(1, 1));
    let far_future = TickCapture::new(domain(), i64::MAX, 999_999_999).unwrap();
    let usage = local_usage_candidates(&[pending], &far_future).unwrap();
    assert_eq!((usage.weight(), usage.short_orders()), (1, 1));
    let sent = ChargeCandidate::Sent { cost: cost(1, 1), handoff: tick(SECOND) };
    for (age, expected) in [
        (0, (1, 1)),
        (10 * SECOND - 1, (1, 1)),
        (10 * SECOND, (1, 0)),
        (10 * SECOND + 1, (1, 0)),
        (60 * SECOND - 1, (1, 0)),
        (60 * SECOND, (0, 0)),
        (60 * SECOND + 1, (0, 0)),
    ] {
        let usage = local_usage_candidates(&[sent], &tick(SECOND + age)).unwrap();
        assert_eq!((usage.weight(), usage.short_orders()), expected, "age={age}");
    }
    let components = [20, 80, 6].map(|weight| ChargeCandidate::Pending(cost(weight, 0)));
    // Account + all open orders + order lists: three physical read costs.
    assert_eq!(local_usage_candidates(&components, &tick(0)).unwrap().weight(), 106);
    assert_eq!(local_usage_candidates(&[pending, pending], &tick(0)).unwrap().weight(), 2);
}

#[test]
fn input_bounds_reject_without_truncating_or_allocating_results() {
    for (weight, orders) in [
        (0, 0),
        (0, 1),
        (1201, 0),
        (u32::MAX, 1),
        (1, 2),
        (1, u32::MAX),
    ] {
        assert_eq!(RequestCostCandidate::new(weight, orders), Err(BudgetFactError::InvalidCost));
    }
    assert!(RequestCostCandidate::new(1200, 1).is_ok());
    let empty = local_usage_candidates(&[], &tick(0)).unwrap();
    assert_eq!((empty.weight(), empty.short_orders()), (0, 0));
    let mut charges = vec![ChargeCandidate::Pending(cost(1200, 1)); 1200];
    let usage = local_usage_candidates(&charges, &tick(0)).unwrap();
    assert_eq!((usage.weight(), usage.short_orders()), (1_440_000, 1200));
    charges.push(ChargeCandidate::Sent { cost: cost(1, 0), handoff: tick(SECOND) });
    // Length refusal precedes the final invalid-time record; no prefix is accepted.
    assert_eq!(local_usage_candidates(&charges, &tick(0)), Err(BudgetFactError::TooManyCharges));
    assert!(size_of::<ChargeCandidate>() <= 128);
    assert!(size_of::<UsageFacts>() <= 16);
    assert!(size_of::<LimitFacts>() <= 64);
    println!(
        "record={} max_input={} usage={} limits={} startup={}",
        size_of::<ChargeCandidate>(),
        1200 * size_of::<ChargeCandidate>(),
        size_of::<UsageFacts>(),
        size_of::<LimitFacts>(),
        size_of::<StartupCapsFacts>()
    );
}

#[test]
fn incomparable_or_reversed_time_never_becomes_free_usage() {
    let start = tick(SECOND);
    let changed = [
        ClockDomain::new(Uuid::from_u128(2), 0, 0).unwrap(),
        ClockDomain::new(Uuid::from_u128(1), 1, 0).unwrap(),
        ClockDomain::new(Uuid::from_u128(1), 0, 1).unwrap(),
    ];
    for d in changed {
        let now = TickCapture::new(d, 200_000, 0).unwrap();
        assert_eq!(
            local_usage_candidates(
                &[ChargeCandidate::Sent { cost: cost(1, 1), handoff: start }],
                &now
            ),
            Err(BudgetFactError::Clock(ClockError::Incomparable))
        );
        assert_eq!(
            attempt_age_candidate(&start, &now),
            Err(BudgetFactError::Clock(ClockError::Incomparable))
        );
        assert_eq!(
            daily_retention_candidate(&start, &now),
            Err(BudgetFactError::Clock(ClockError::Incomparable))
        );
        assert_eq!(
            restart_cap_candidates(&start, &now, EpochHistoryCandidate::UnknownPriorHistory),
            Err(BudgetFactError::Clock(ClockError::Incomparable))
        );
    }
    let earlier = tick(SECOND - 1);
    assert_eq!(
        local_usage_candidates(
            &[ChargeCandidate::Sent { cost: cost(1, 1), handoff: start }],
            &earlier
        ),
        Err(BudgetFactError::Clock(ClockError::Reversed))
    );
    assert_eq!(
        attempt_age_candidate(&start, &earlier),
        Err(BudgetFactError::Clock(ClockError::Reversed))
    );
    assert_eq!(
        daily_retention_candidate(&start, &earlier),
        Err(BudgetFactError::Clock(ClockError::Reversed))
    );
    assert_eq!(
        restart_cap_candidates(&start, &earlier, EpochHistoryCandidate::UnknownPriorHistory),
        Err(BudgetFactError::Clock(ClockError::Reversed))
    );
}

proptest! {
    #[test]
    fn bounded_usage_matches_independent_integer_oracle(
        rows in prop::collection::vec((1u32..=1200, 0u32..=1, any::<bool>(), 0u64..=70_000_000_000), 0..64)
    ) {
        let mut expected = (0u128, 0u128);
        let charges: Vec<_> = rows.iter().map(|&(weight, orders, pending, age)| {
            if pending || age < 60_000_000_000 { expected.0 += weight as u128; }
            if pending || age < 10_000_000_000 { expected.1 += orders as u128; }
            if pending { ChargeCandidate::Pending(cost(weight, orders)) }
            else { ChargeCandidate::Sent { cost: cost(weight, orders), handoff: tick(100 * SECOND - age as u128) } }
        }).collect();
        let facts = local_usage_candidates(&charges, &tick(100 * SECOND)).unwrap();
        prop_assert_eq!((facts.weight() as u128, facts.short_orders() as u128), expected);
    }
}

#[test]
fn prospective_caps_preserve_every_class_and_daily_ceiling() {
    for total in [399, 400, 401, 599, 600, 601, 1199, 1200, 1201] {
        for class in [Class::Steady, Class::Protective] {
            let facts = additional_cost_facts(
                pending_usage(total - 1, 0),
                UnqualifiedDailyCount(0),
                cost(1, 0),
                class,
                no_startup_cap(),
            )
            .unwrap();
            assert_eq!(facts.usage().weight(), total as u64);
            assert_eq!(
                facts.fits_normal_arithmetic(),
                total <= if class == Class::Steady { 400 } else { 600 }
            );
            assert_eq!(
                facts.fits_expanded_arithmetic(),
                if class == Class::Protective {
                    Some(total <= 1200)
                } else {
                    None
                }
            );
        }
    }
    for total in [9, 10, 11, 19, 20, 21] {
        let facts = additional_cost_facts(
            pending_usage(total - 1, (total - 1) as usize),
            UnqualifiedDailyCount(0),
            cost(1, 1),
            Class::Protective,
            no_startup_cap(),
        )
        .unwrap();
        assert_eq!(facts.usage().short_orders(), total as u64);
        assert_eq!(facts.fits_normal_arithmetic(), total <= 10);
        assert_eq!(facts.fits_expanded_arithmetic(), Some(total <= 20));
    }
    for class in [Class::Steady, Class::Protective] {
        for (daily, order_units, expected) in [
            (9999, 1, true),
            (10000, 1, false),
            (10000, 0, true),
            (19999, 1, false),
        ] {
            let facts = additional_cost_facts(
                pending_usage(0, 0),
                UnqualifiedDailyCount(daily),
                cost(1, order_units),
                class,
                no_startup_cap(),
            )
            .unwrap();
            assert_eq!(facts.daily(), daily + order_units as u64);
            assert_eq!(facts.fits_normal_arithmetic(), expected);
            assert_eq!(
                facts.fits_expanded_arithmetic(),
                (class == Class::Protective).then_some(expected)
            );
        }
    }
    assert_eq!(
        additional_cost_facts(
            pending_usage(0, 0),
            UnqualifiedDailyCount(u64::MAX),
            cost(1, 1),
            Class::Protective,
            no_startup_cap()
        ),
        Err(BudgetFactError::Overflow)
    );
    let over = additional_cost_facts(
        pending_usage(0, 0),
        UnqualifiedDailyCount(u64::MAX),
        cost(1, 0),
        Class::Protective,
        no_startup_cap(),
    )
    .unwrap();
    assert!(!over.fits_normal_arithmetic());
    assert_eq!(over.fits_expanded_arithmetic(), Some(false));
}

#[test]
fn restart_caps_never_expand_emergency_or_assume_clean_history() {
    for (k60, k10) in [(0, 1), (1, 0), (0, 0)] {
        assert_eq!(
            restart_cap_candidates(&tick(0), &tick(0), EpochHistoryCandidate::Comparable {
                k60,
                k10
            }),
            Err(BudgetFactError::InvalidEpochs)
        );
    }
    for k in 1..=16 {
        let caps = restart_cap_candidates(&tick(0), &tick(0), EpochHistoryCandidate::Comparable {
            k60: k,
            k10: k,
        })
        .unwrap();
        assert_eq!(caps.weight(), Some((200u128 / 2u128.pow((k - 1) as u32)) as u64));
        assert_eq!(caps.short_orders(), Some((2u128 / 2u128.pow((k - 1) as u32)) as u64));
    }
    for history in [
        EpochHistoryCandidate::UnknownPriorHistory,
        EpochHistoryCandidate::Comparable { k60: u64::MAX, k10: u64::MAX },
    ] {
        for (elapsed, expected) in [
            (0, (Some(0), Some(0))),
            (10 * SECOND - 1, (Some(0), Some(0))),
            (10 * SECOND, (Some(0), None)),
            (60 * SECOND - 1, (Some(0), None)),
            (60 * SECOND, (None, None)),
        ] {
            let caps = restart_cap_candidates(&tick(0), &tick(elapsed), history).unwrap();
            assert_eq!((caps.weight(), caps.short_orders()), expected);
        }
    }
    let caps = restart_cap_candidates(&tick(0), &tick(0), EpochHistoryCandidate::Comparable {
        k60: 1,
        k10: 1,
    })
    .unwrap();
    for class in [Class::Steady, Class::Protective] {
        let within = additional_cost_facts(
            pending_usage(199, 1),
            UnqualifiedDailyCount(0),
            cost(1, 1),
            class,
            caps,
        )
        .unwrap();
        assert!(within.fits_normal_arithmetic());
        for (weight, orders) in [(200, 0), (2, 2)] {
            let blocked = additional_cost_facts(
                pending_usage(weight, orders),
                UnqualifiedDailyCount(0),
                cost(1, 1),
                class,
                caps,
            )
            .unwrap();
            assert!(!blocked.fits_normal_arithmetic());
            assert_eq!(
                blocked.fits_expanded_arithmetic(),
                (class == Class::Protective).then_some(false)
            );
        }
    }
}

proptest! {
    #[test]
    fn integer_halving_series_cannot_mint_restart_capacity(restarts in 1u32..=64) {
        let (mut weight, mut orders) = (0u64, 0u64);
        let (mut denominator, mut oracle_weight, mut oracle_orders) = (1u128, 0u128, 0u128);
        for k in 1..=restarts {
            let caps = restart_cap_candidates(&tick(0), &tick(0), EpochHistoryCandidate::Comparable { k60: k as u64, k10: k as u64 }).unwrap();
            weight += caps.weight().unwrap(); orders += caps.short_orders().unwrap();
            oracle_weight += 200 / denominator; oracle_orders += 2 / denominator;
            denominator *= 2;
        }
        prop_assert_eq!((weight as u128, orders as u128), (oracle_weight, oracle_orders));
        prop_assert!(weight <= 400 && orders <= 4);
        prop_assert!(weight >= 200 && orders >= 2);
    }
}

#[test]
fn handoff_and_daily_age_guards_keep_distinct_boundaries() {
    for (age, expected) in [
        (0, true),
        (60 * SECOND - 1, true),
        (60 * SECOND, true),
        (60 * SECOND + 1, false),
    ] {
        assert_eq!(
            attempt_age_candidate(&tick(SECOND), &tick(SECOND + age))
                .unwrap()
                .within_guard(),
            expected
        );
    }
    assert!(attempt_age_candidate(&tick(0), &tick(SECOND / 2)).unwrap().within_guard());
    for (age, expected) in [
        (86400 * SECOND, false),
        (86460 * SECOND - 1, false),
        (86460 * SECOND, true),
        (86460 * SECOND + 1, true),
    ] {
        assert_eq!(
            daily_retention_candidate(&tick(0), &tick(age)).unwrap().threshold_elapsed(),
            expected
        );
    }
    let max = TickCapture::new(domain(), i64::MAX, 999_999_999).unwrap();
    assert!(attempt_age_candidate(&max, &max).unwrap().within_guard());
    assert!(!attempt_age_candidate(&tick(0), &max).unwrap().within_guard());
    assert!(daily_retention_candidate(&tick(0), &max).unwrap().threshold_elapsed());
}

#[test]
fn quarantine_requires_original_matching_coordinates_without_proving_them() {
    let cohort = Uuid::from_u128(10);
    let recovery = Uuid::from_u128(11);
    // Invented coordinates remain unqualified even when the predicate is true.
    let anchor = QuarantineAnchorCandidate { cohort, recovery, observed: tick(SECOND) };
    assert_eq!(
        quarantine_elapsed_candidate(None, cohort, recovery, &tick(0)),
        Err(BudgetFactError::MissingAnchor)
    );
    for (c, r) in [
        (Uuid::nil(), recovery),
        (cohort, Uuid::nil()),
        (recovery, recovery),
        (cohort, cohort),
    ] {
        assert_eq!(
            quarantine_elapsed_candidate(Some(&anchor), c, r, &tick(100_000 * SECOND)),
            Err(BudgetFactError::IdentityMismatch)
        );
    }
    for (age, expected) in [
        (0, false),
        (86460 * SECOND - 1, false),
        (86460 * SECOND, true),
        (86460 * SECOND + 1, true),
    ] {
        assert_eq!(
            quarantine_elapsed_candidate(Some(&anchor), cohort, recovery, &tick(SECOND + age))
                .unwrap()
                .threshold_elapsed(),
            expected
        );
        assert_eq!(anchor.observed, tick(SECOND));
    }
    assert_eq!(
        quarantine_elapsed_candidate(Some(&anchor), cohort, recovery, &tick(0)),
        Err(BudgetFactError::Clock(ClockError::Reversed))
    );
    for d in [
        ClockDomain::new(Uuid::from_u128(2), 0, 0).unwrap(),
        ClockDomain::new(Uuid::from_u128(1), 0, 1).unwrap(),
    ] {
        assert_eq!(
            quarantine_elapsed_candidate(
                Some(&anchor),
                cohort,
                recovery,
                &TickCapture::new(d, 100_000, 0).unwrap()
            ),
            Err(BudgetFactError::Clock(ClockError::Incomparable))
        );
    }
}
