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

#[cfg(feature = "crash-points")]
proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]
    /// Commission and the test ACCOUNT wrapper share one reserved first poll:
    /// identical results, one handoff at most per reservation, the poll runs
    /// only when admitted, and the admission mark is returned as produced.
    #[test]
    fn reserved_first_polls_match_an_independent_handoff_oracle(
        events in prop::collection::vec(0u8..8, 1..64),
    ) {
        let clock = NativeClock::initialize().unwrap();
        let epoch = Uuid::from_u128(3);
        let commission = numerical_state(true);
        let account = numerical_state(true);
        let mut now = 120u32;
        let (mut closing, mut pause, mut charged, mut dropped) = (false, false, false, false);
        let mut waiter = None;
        let mut handoffs = 0u64;
        let mut c_pending = Some(commission.try_reserve_commission(tick(now), tick(now + 10)).unwrap());
        let mut a_pending = Some(account.try_reserve_account(tick(now), tick(now + 10)).unwrap());
        for event in events {
            let refuse = event == 5;
            match event {
                // Stay inside the reservation deadline; expiry is the model's.
                0 if now < 128 => now += 1,
                1 => { commission.begin_closing(); account.begin_closing(); closing = true; },
                2 if waiter.is_none() && !closing => waiter = Some((
                    commission.protective_waiter().unwrap(),
                    account.protective_waiter().unwrap(),
                )),
                3 => waiter = None,
                4 => { observe(&commission, now, "0", 429); observe(&account, now, "0", 429); pause = true; },
                6 => { c_pending = None; a_pending = None; dropped = true; },
                _ => {},
            }
            let (Some(c), Some(a)) = (c_pending.as_mut(), a_pending.as_mut()) else {
                prop_assert!(dropped);
                continue;
            };
            // Admission precedes the budget check, which precedes the charge.
            let expected = if refuse {
                Err("Deferred")
            } else if closing {
                Err("Closing")
            } else if pause || waiter.is_some() {
                Err("Deferred")
            } else if charged {
                Err("Invalid")
            } else {
                Ok(())
            };
            let (mut c_polls, mut a_polls) = (0usize, 0usize);
            let c_result = commission.poll_commission_first(
                c, &clock, epoch,
                || if refuse { Err(BudgetError::Deferred) } else { Ok(tick(now)) },
                || { c_polls += 1; std::task::Poll::Pending },
            );
            let a_result = account.poll_account_first(
                a, &clock, epoch,
                || if refuse { Err(BudgetError::Deferred) } else { Ok((tick(now), now)) },
                || { a_polls += 1; std::task::Poll::Pending },
            );
            if let Ok((_, mark, _)) = &a_result {
                prop_assert_eq!(*mark, now, "the admission mark is the one produced");
            }
            let c_result = c_result.map(|_| ()).map_err(|e| format!("{e:?}"));
            let a_result = a_result.map(|_| ()).map_err(|e| format!("{e:?}"));
            prop_assert_eq!(&c_result, &a_result);
            prop_assert_eq!(c_result.map_err(|e| e.split(['(', ' ']).next().unwrap_or("").to_string()),
                expected.map_err(str::to_string));
            if expected.is_ok() {
                charged = true;
                handoffs += 1;
            }
            prop_assert_eq!(c_polls, usize::from(expected.is_ok()));
            prop_assert_eq!(a_polls, c_polls);
            prop_assert_eq!(commission.handoffs().unwrap(), handoffs);
            prop_assert_eq!(account.handoffs().unwrap(), handoffs);
            prop_assert!(handoffs <= 1, "one reservation, one handoff");
        }
    }
}

#[cfg(feature = "crash-points")]
#[test]
fn a_foreign_reservation_is_invalid_and_never_polled() {
    let clock = NativeClock::initialize().unwrap();
    let (owner, other) = (numerical_state(true), numerical_state(true));
    let mut foreign = other.try_reserve_account(tick(120), tick(130)).unwrap();
    let mut polled = false;
    let result = owner.poll_account_first(
        &mut foreign,
        &clock,
        Uuid::from_u128(3),
        || Ok((tick(120), ())),
        || {
            polled = true;
            std::task::Poll::Pending
        },
    );
    assert!(matches!(result, Err(BudgetError::Invalid)) && !polled);
    assert_eq!((owner.handoffs().unwrap(), other.handoffs().unwrap()), (0, 0));
}

#[cfg(feature = "crash-points")]
#[test]
fn dropped_unpolled_weight_is_refunded_and_handed_off_weight_remains_charged() {
    let clock = NativeClock::initialize().unwrap();
    for redeemed in [false, true] {
        let budget = numerical_state(true);
        let (now, due) = (tick(120), tick(130));
        let mut pending = budget.try_reserve_account(now, due).unwrap();
        if redeemed {
            let _ = budget
                .poll_account_first(
                    &mut pending,
                    &clock,
                    Uuid::from_u128(3),
                    || Ok((now, ())),
                    || std::task::Poll::Pending,
                )
                .unwrap();
        }
        drop(pending);
        let used = usize::from(redeemed);
        let full: Vec<_> =
            (used..20).map(|_| budget.try_reserve_account(now, due).unwrap()).collect();
        assert!(matches!(budget.try_reserve_account(now, due), Err(BudgetError::Deferred)));
        drop(full);
        assert!(budget.try_reserve_account(now, due).is_ok());
        assert_eq!(budget.handoffs().unwrap(), used as u64);
    }
}

#[cfg(feature = "crash-points")]
proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]
    #[test]
    fn each_inventory_endpoint_keeps_its_weight_across_drop_or_handoff(
        endpoint in 0usize..3,
        handed_off in any::<bool>(),
    ) {
        use robson_connectors::inventory_loopback::InventoryRead;
        let clock = NativeClock::initialize().unwrap();
        let budget = numerical_state(true);
        let (now, due) = (tick(120), tick(130));
        let read = InventoryRead::ALL[endpoint];
        let mut reservation = budget.try_reserve_inventory(read, now, due).unwrap();
        prop_assert_eq!(reservation.read(), read);
        if handed_off {
            let _ = budget.poll_account_first(&mut reservation, &clock, Uuid::from_u128(3),
                || Ok((now, ())), || std::task::Poll::Pending).unwrap();
        }
        drop(reservation);
        let remaining = 400 - u32::from(handed_off) * [20, 6, 6][endpoint];
        let mut probes = Vec::new();
        for _ in 0..remaining / 20 {
            probes.push(budget.try_reserve_account(now, due).unwrap());
        }
        let mut time = Vec::new();
        for _ in 0..remaining % 20 {
            time.push(budget.try_reserve_time(now, due).unwrap());
        }
        prop_assert!(matches!(budget.try_reserve_time(now, due), Err(BudgetError::Deferred)));
        prop_assert_eq!(budget.handoffs().unwrap(), u64::from(handed_off));
    }
}
