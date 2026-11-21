//! PS-5a: every reservation takes its `now` under the budget lock. The
//! limiter's model refuses for good a `now` older than the last it observed
//! (`request_budget/local_model.rs`), so a capture taken before the lock by
//! one reserver could reach it after a newer one's and disable the budget
//! for the rest of the process. On injected ticks (no real time waited).
use std::sync::{
    atomic::{AtomicBool, AtomicI64, Ordering},
    Mutex as StdMutex,
};

use proptest::prelude::*;
use robson_exec::venue_clock::{ClockDomain, TickCapture};
use uuid::Uuid;

use super::*;

const SECOND: i64 = 1_000_000_000;

fn tick(nanoseconds: i64) -> TickCapture {
    let domain = ClockDomain::new(Uuid::from_u128(7), 0, 0).unwrap();
    TickCapture::new(domain, nanoseconds / SECOND, nanoseconds % SECOND).unwrap()
}

const READS: [ReadKind; 5] = [
    ReadKind::Account,
    ReadKind::OperatedOpenOrders,
    ReadKind::OpenOrderLists,
    ReadKind::ExchangeInfo,
    ReadKind::Kline,
];

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// The TIME sampler, up to four readers and the commission's reserver
    /// (8h's supervised path, Root's commission ruling) reserve at once, each
    /// `now` from one shared monotonic tick source that yields right after
    /// the capture (widening any window between capture and lock). Whatever
    /// the interleaving: every capture runs while the budget lock is held
    /// (checked in the capture itself; each entry is first called once
    /// uncontended, where a pre-lock capture finds the lock free whatever the
    /// threads' schedule), no reservation is ever refused as
    /// `Invalid` (the model never sees time go back), every admitted one
    /// charges its fixed weight (TIME 1, commission 20), and the minute's
    /// admitted weight is within the steady 400, equal to the model's own
    /// usage.
    #[test]
    fn property_reservations_take_now_under_the_lock(
        readers in 1_usize..=4,
        plan in proptest::collection::vec(proptest::collection::vec(0_usize..5, 1..14), 4),
        samples in 1_usize..8,
        commissions in 0_usize..4,
    ) {
        let budget = SharedBudget::non_trading(tick(0), None, false).unwrap();
        // Past the unknown-history window (no weight before start + 60 s).
        let ticks = AtomicI64::new(61 * SECOND);
        let (shared, unlocked) = (budget.time_owner(), AtomicBool::new(false));
        let at = || {
            if shared.0.try_lock().is_ok() {
                unlocked.store(true, Ordering::SeqCst);
            }
            let now = ticks.fetch_add(1_000_000, Ordering::SeqCst);
            std::thread::yield_now();
            Ok::<_, BudgetError>((tick(now), tick(now + 10 * SECOND)))
        };
        let admitted = StdMutex::new(Vec::new());
        let invalid = StdMutex::new(Vec::new());
        let keep = |weight: u32, result: Result<PendingTime, BudgetError>| match result {
            Ok(pending) => admitted.lock().unwrap().push((weight, pending)),
            Err(BudgetError::Invalid) => invalid.lock().unwrap().push(weight),
            Err(_) => {},
        };
        keep(1, budget.time_owner().reserve_now(1, at));
        keep(20, budget.reserve_read_now(READS[0], at).map(|pending| pending.0));
        keep(20, budget.time_owner().try_reserve_commission(at).map(|pending| pending.0));
        std::thread::scope(|scope| {
            let (budget, at, keep) = (&budget, &at, &keep);
            scope.spawn(move || {
                for _ in 0..samples {
                    keep(1, budget.time_owner().reserve_now(1, at));
                }
            });
            scope.spawn(move || {
                for _ in 0..commissions {
                    let reserved = budget.time_owner().try_reserve_commission(at);
                    keep(20, reserved.map(|pending| pending.0));
                }
            });
            for steps in plan.iter().take(readers) {
                scope.spawn(move || {
                    for &step in steps {
                        let read = READS[step];
                        let reserved = budget.reserve_read_now(read, at);
                        assert!(reserved.as_ref().map_or(true, |p| p.read() == read));
                        keep(read.weight(), reserved.map(|pending| pending.0));
                    }
                });
            }
        });
        prop_assert!(!unlocked.load(Ordering::SeqCst), "a capture ran without the budget lock");
        prop_assert!(invalid.lock().unwrap().is_empty(), "the model saw time go back");
        let charged: u64 = admitted.lock().unwrap().iter().map(|(w, _)| u64::from(*w)).sum();
        prop_assert!(charged <= 400, "{charged} over the steady minute");
        let now = ticks.fetch_add(1_000_000, Ordering::SeqCst);
        let usage = budget.time_owner().usage(|| Some(tick(now)));
        prop_assert_eq!(usage, Some((charged, 0)));
    }
}

/// The sampler's TIME reservation takes its `now` under the lock too:
/// `admitted` stays for the spacing rule and the persistence bound only.
#[test]
fn the_sampler_reserves_with_now_under_the_lock() {
    let owner = include_str!("../venue_clock_owner.rs");
    let owner = &owner[..owner.find("#[cfg(test)]\nmod ").unwrap_or(owner.len())];
    assert_eq!(owner.matches(".reserve_now(1, || {").count(), 1, "the TIME reservation");
    assert!(!owner.contains(".try_reserve_time("), "a pre-lock TIME reservation");
}

/// 8h's supervised commission reservation takes its `now` inside the
/// reservation's closure, under the lock (Root's ruling
/// ps-5/ROOT-decision-commission-now-ordering.json).
#[test]
fn the_commission_reserves_with_now_under_the_lock() {
    let owner = include_str!("../venue_clock_owner/commission.rs");
    let body = &owner[owner.find("fn reserve_commission(").unwrap()..];
    let body = &body[..body.find("\n    }\n").unwrap()];
    let closure = body.find(".try_reserve_commission(|| {").expect("an under-lock reservation");
    assert!(body.find(".capture()").is_some_and(|capture| capture > closure), "{body}");
}
