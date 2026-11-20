//! Numerical/schedule controls only. A hand-built state in this private test
//! module cannot supply the actual bootstrap context or production readiness.
use proptest::prelude::*;
use robson_exec::venue_clock::{ClockDomain, TickCapture};
use uuid::Uuid;

use super::*;

fn tick(seconds: u32) -> TickCapture {
    TickCapture::new(ClockDomain::new(Uuid::from_u128(1), 0, 0).unwrap(), i64::from(seconds), 0)
        .unwrap()
}
fn numerical_state(unclean: bool) -> SharedBudget {
    SharedBudget(Arc::new(Mutex::new(State {
        model: BudgetTransitions::unknown_history(tick(0)).unwrap(),
        start: tick(0),
        unclean,
        daily: 0,
        pause: None,
        protective_waiters: 0,
        handoffs: 0,
        closing: false,
        heads: super::super::heads::Heads::default(),
    })))
}
fn observe(budget: &SharedBudget, now: u32, weight: &str, status: u16) {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert("x-mbx-used-weight-1m", weight.parse().unwrap());
    budget.0.lock().unwrap().heads.observe(
        status,
        &headers,
        Some(robson_exec::clock_records::RawClockCapture {
            epoch: Uuid::from_u128(2),
            wall_seconds: 1_794_614_400,
            wall_nanoseconds: 0,
            domain: tick(now).domain(),
            before_ns: i128::from(now) * 1_000_000_000,
            after_ns: i128::from(now) * 1_000_000_000,
        }),
    );
}
#[test]
fn unknown_clean_restart_has_no_early_weight_and_healthy_progress() {
    for unclean in [false, true] {
        let budget = numerical_state(unclean);
        let threshold = if unclean { 120 } else { 60 };
        assert!(matches!(
            budget.try_reserve_time(tick(threshold - 1), tick(threshold + 9)),
            Err(BudgetError::Deferred)
        ));
        let pending = budget.try_reserve_time(tick(threshold), tick(threshold + 10)).unwrap();
        drop(pending);
        let waiter = budget.protective_waiter().unwrap();
        assert!(matches!(
            budget.try_reserve_time(tick(threshold), tick(threshold + 10)),
            Err(BudgetError::Deferred)
        ));
        drop(waiter);
        assert!(budget.try_reserve_time(tick(threshold), tick(threshold + 10)).is_ok());
        assert_eq!(budget.handoffs().unwrap(), 0, "reservation is not a send");
    }
}
#[test]
fn expired_header_restriction_cannot_be_refunded_by_lower_observation() {
    let budget = numerical_state(false);
    observe(&budget, 120, "3000", 200);
    observe(&budget, 121, "0", 200);
    assert!(matches!(
        budget.try_reserve_time(tick(239), tick(249)),
        Err(BudgetError::Deferred)
    ));
    assert!(budget.try_reserve_time(tick(240), tick(250)).is_ok());
    assert!(!budget.pending_pause(), "ordinary response creates no ban");
    observe(&budget, 250, "0", 429);
    assert!(budget.pending_pause(), "observed restriction is not durable yet");
    assert!(matches!(
        budget.try_reserve_time(tick(1000), tick(1010)),
        Err(BudgetError::Deferred)
    ));
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]
    #[test]
    fn reservation_head_priority_and_closing_histories_match_independent_oracle(
        events in prop::collection::vec(0u8..8,1..128),
    ) {
        let budget=numerical_state(true);
        let mut now=120u32;
        let mut closing=false;
        let mut pause=false;
        let mut high_until=0u32;
        let mut waiter=None;
        // Every history starts with an actual positive reservation, so an
        // always-refuse implementation cannot satisfy this property.
        prop_assert!(budget.try_reserve_time(tick(now),tick(now+10)).is_ok());
        for event in events {
            match event {
                0=>now+=1,
                1 if waiter.is_none() && !closing=>waiter=Some(budget.protective_waiter().unwrap()),
                2=>{waiter=None;},
                3=>{observe(&budget,now,"3000",200);high_until=high_until.max(now+120);},
                4=>{budget.begin_closing();closing=true;},
                6=>observe(&budget,now,"0",200),
                7=>{observe(&budget,now,"0",429);pause=true;},
                _=>{},
            }
            let expected=!closing&&!pause&&waiter.is_none()&&now>=high_until;
            let result=budget.try_reserve_time(tick(now),tick(now+10));
            prop_assert_eq!(result.is_ok(),expected);
            drop(result);
            prop_assert_eq!(budget.pending_pause(),pause);
            prop_assert_eq!(budget.handoffs().unwrap(),0);
        }
    }
}
