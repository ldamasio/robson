//! PS-3 controls without a database or venue: the first scheduled cycle's
//! delay, the request slot, the publication CAS and the caller's deadline,
//! each against an independent oracle (proptest interleavings).
use std::sync::atomic::{AtomicUsize, Ordering};

use proptest::prelude::*;
use robson_exec::{
    clock_records::RawClockCapture,
    venue_clock::{ClockDomain, TickCapture},
};
use tokio::sync::{oneshot, oneshot::error::TryRecvError};
use tokio_util::sync::CancellationToken;

use super::{
    on_demand::{Request, Requests},
    *,
};
use crate::{
    process_epoch::{Epoch, Predecessor},
    venue_budget::state::SharedBudget,
};

const SECOND: i128 = 1_000_000_000;

fn domain() -> ClockDomain {
    ClockDomain::new(Uuid::from_u128(2), 0, 0).unwrap()
}

fn at(seconds: i64) -> TickCapture {
    TickCapture::new(domain(), seconds, 0).unwrap()
}

fn capture(tick: i128) -> RawClockCapture {
    let wall = SECOND + tick;
    RawClockCapture {
        epoch: Uuid::from_u128(1),
        wall_seconds: (wall / SECOND) as i64,
        wall_nanoseconds: (wall % SECOND) as u32,
        domain: domain(),
        before_ns: tick,
        after_ns: tick,
    }
}

fn sample(id: u128) -> Published {
    let raw = RawClockSample {
        id: Uuid::from_u128(id),
        tenant: Uuid::from_u128(4),
        venue: VenueId::new("binance:spot"),
        send: capture(0),
        receive: capture(0),
        server_ms: 1_000,
    };
    Published {
        id: raw.id,
        arithmetic: raw.arithmetic().unwrap(),
        u2: raw.numbers().unwrap().u2,
    }
}

fn state() -> ClockState {
    ClockState {
        generation: 0,
        closing: false,
        published: None,
        start: at(0),
        last_valid: None,
        faults: 0,
        active_faults: 0,
    }
}

/// PLAN §6.6: the first scheduled cycle comes at start + 61 s, or + 121 s
/// after an unclean start, so it never falls inside the budget's grant
/// window (60 s) or unclean barrier (120 s), whenever the sampler spawns.
#[test]
fn the_first_sample_waits_for_the_grant() {
    let secs = Duration::from_secs;
    assert_eq!(first_delay(at(100), at(100), false).unwrap(), secs(61));
    assert_eq!(first_delay(at(100), at(130), false).unwrap(), secs(31));
    assert_eq!(first_delay(at(100), at(100), true).unwrap(), secs(121));
    assert_eq!(first_delay(at(100), at(190), true).unwrap(), secs(31));
    assert_eq!(first_delay(at(100), at(400), true).unwrap(), Duration::ZERO);
    for elapsed in 0..300u64 {
        for (unclean, window) in [(false, 60), (true, 120)] {
            let delay = first_delay(at(100), at(100 + elapsed as i64), unclean).unwrap();
            assert!(elapsed + delay.as_secs() > window, "{elapsed} s, unclean {unclean}");
        }
    }
}

/// Every receiver with the request (by take order) it joined.
type Joined = Vec<(Option<oneshot::Receiver<Result<Uuid, TimeError>>>, usize)>;

#[derive(Debug, Clone)]
enum SlotEvent {
    Join(u8),
    Drop(u8),
    Take,
    Answer,
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// Root's condition (PS-3 size decision): the request slot holds at most
    /// four live waiters (a further join is refused, nothing queues past the
    /// cap); a join shares the request the sampler has not taken yet, never a
    /// taken one; an answer reaches exactly the taken request's live waiters.
    #[test]
    fn property_the_request_slot_holds_at_most_four_waiters(
        events in prop::collection::vec(
            prop_oneof![
                (0u8..20).prop_map(SlotEvent::Join),
                (0u8..32).prop_map(SlotEvent::Drop),
                Just(SlotEvent::Take),
                Just(SlotEvent::Answer),
            ],
            1..96,
        ),
    ) {
        let requests = Requests::default();
        let mut joined: Joined = Vec::new();
        let mut taken: Option<(Request, usize)> = None;
        let mut next_request = 0usize;
        let mut answered = 0u128;
        for event in events {
            let live_pending = joined
                .iter()
                .filter(|(rx, request)| *request == next_request && rx.is_some())
                .count();
            match event {
                SlotEvent::Join(ms) => {
                    let deadline = Instant::now() + Duration::from_millis(u64::from(ms));
                    match requests.join(deadline) {
                        Ok(rx) => {
                            prop_assert!(live_pending < 4, "a fifth live waiter was queued");
                            joined.push((Some(rx), next_request));
                        },
                        Err(error) => {
                            prop_assert_eq!(error, TimeError::Resource);
                            prop_assert_eq!(live_pending, 4, "refused below the cap");
                        },
                    }
                },
                SlotEvent::Drop(index) => {
                    if let Some(entry) = joined.get_mut(usize::from(index)) {
                        entry.0 = None;
                    }
                },
                SlotEvent::Take => {
                    if taken.is_none() {
                        match requests.try_take() {
                            Some(request) => {
                                prop_assert!(joined.iter().any(|(_, r)| *r == next_request));
                                taken = Some((request, next_request));
                                next_request += 1;
                            },
                            None => prop_assert!(joined.iter().all(|(_, r)| *r < next_request)),
                        }
                    }
                },
                SlotEvent::Answer => {
                    if let Some((request, index)) = taken.take() {
                        answered += 1;
                        request.answer(Ok(Uuid::from_u128(answered)));
                        for (rx, request) in joined.iter_mut() {
                            let Some(receiver) = rx.as_mut() else { continue };
                            let got = receiver.try_recv();
                            if *request == index {
                                prop_assert_eq!(got, Ok(Ok(Uuid::from_u128(answered))));
                                *rx = None;
                            } else {
                                prop_assert_eq!(got, Err(TryRecvError::Empty), "answered by another request");
                            }
                        }
                    }
                },
            }
        }
    }

    /// PLAN §8 PS-3 property, on the real dispatch (`next_cycle`, the request
    /// slot, `Notify`) and the real publication CAS, concurrently on paused
    /// time: callers join at random times with random deadlines and may be
    /// dropped; faults and the closing interleave; the cycle stands in for
    /// the HTTP exchange and the persistence. One cycle at a time; no sample
    /// published after closing or after a fault edge since its cycle began
    /// (the state_tests oracle); every answer from a cycle that began after
    /// its caller asked; every caller answered, expired or dropped by its
    /// deadline; the sampler ends once cancelled.
    #[test]
    fn property_one_time_cycle_and_no_late_publication(
        script in prop::collection::vec((0u8..6, 0u64..2_500, 0u64..3_000), 1..48),
    ) {
        dispatch_script(script, Duration::from_secs(1))?;
    }

    /// Root's condition (PS-3 size decision): an on-demand caller's deadline
    /// bounds both the HTTP request and the persistence, and a request whose
    /// deadline has passed is refused before any reservation (nothing
    /// charged, no spacing spent, no cycle numbered).
    #[test]
    fn property_the_caller_deadline_bounds_every_cycle_step(
        offsets in prop::collection::vec((0u64..20_000, 0u64..20_000, 0u64..5_000), 1..16),
    ) {
        let now = Instant::now();
        for (deadline_ms, at_ms, _) in &offsets {
            let deadline = now + Duration::from_millis(*deadline_ms);
            let at = now + Duration::from_millis(*at_ms);
            let bound = bounded(Some(deadline), at);
            prop_assert!(bound <= deadline && bound <= at && (bound == deadline || bound == at));
            prop_assert_eq!(bounded(None, at), at);
        }
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        runtime.block_on(async {
            let mut owner = offline_owner();
            let handoffs = owner.budget.handoffs();
            for (_, _, past_ms) in &offsets {
                let deadline = Instant::now() - Duration::from_millis(*past_ms);
                let refused = owner.run_cycle_kind(CycleKind::OnDemand { deadline }).await;
                prop_assert_eq!(refused, Err(TimeError::Expired));
                prop_assert_eq!(owner.cycle, 0, "a refused request numbered a cycle");
                prop_assert!(owner.last_on_demand.is_none(), "a refused request spent the spacing");
                prop_assert_eq!(owner.budget.handoffs(), handoffs);
            }
            Ok(())
        })?;
    }
}

/// The dispatch property's run of one script (see its doc comment).
fn dispatch_script(script: Vec<(u8, u64, u64)>, tick: Duration) -> Result<(), TestCaseError> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .start_paused(true)
        .build()
        .unwrap();
    runtime.block_on(async {
        let requests = Arc::new(Requests::default());
        let state = Arc::new(Mutex::new(state()));
        let cancel = CancellationToken::new();
        let in_flight = Arc::new(AtomicUsize::new(0));
        // (sample, handoff) of every cycle, and the oracle's generation.
        let handoffs: Arc<Mutex<Vec<(Uuid, Instant)>>> = Arc::default();
        let oracle = Arc::new(Mutex::new((0u64, 0u8)));
        let (cycles, shared, flight, log, edges) = (
            Arc::clone(&requests),
            Arc::clone(&state),
            Arc::clone(&in_flight),
            Arc::clone(&handoffs),
            Arc::clone(&oracle),
        );
        let stop = cancel.clone();
        let sampler = tokio::spawn(async move {
            // Ticks every `tick` compete with the requests (production: 60 s).
            let mut ticks = tokio::time::interval(tick);
            ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            let mut id = 0u128;
            while let Some(request) = lifecycle::next_cycle(&cycles, &mut ticks, &stop).await {
                assert_eq!(flight.fetch_add(1, Ordering::SeqCst), 0, "two cycles in flight");
                id += 1;
                let (generation, seen) =
                    (shared.lock().unwrap().generation, edges.lock().unwrap().0);
                tokio::time::sleep(Duration::from_millis(200)).await;
                log.lock().unwrap().push((Uuid::from_u128(id), Instant::now()));
                tokio::time::sleep(Duration::from_millis(200)).await;
                let mut state = shared.lock().unwrap();
                let result = state.publish(generation, sample(id), at(1));
                let mut oracle = edges.lock().unwrap();
                assert_eq!(
                    result.is_ok(),
                    !state.closing && seen == oracle.0,
                    "publication oracle"
                );
                if result.is_ok() {
                    oracle.1 = 0;
                }
                drop(oracle);
                drop(state);
                flight.fetch_sub(1, Ordering::SeqCst);
                if let Some(request) = request {
                    request.answer(result);
                }
            }
        });
        let mut callers = Vec::new();
        let mut closed_at = None;
        // A healthy caller first: it must be answered unless the closing
        // comes within 2.5 s.
        for (kind, wait, deadline) in [(0, 0, 10_000)].into_iter().chain(script) {
            tokio::time::sleep(Duration::from_millis(wait)).await;
            match kind {
                0..=2 if !cancel.is_cancelled() => {
                    let asked = Instant::now();
                    let due = asked + Duration::from_millis(deadline);
                    let receiver = requests.join(due);
                    let abandon = kind == 2;
                    let caller = tokio::spawn(async move {
                        let Ok(receiver) = receiver else { return None };
                        let limit = if abandon {
                            asked + Duration::from_millis(deadline / 2)
                        } else {
                            due
                        };
                        let answer = timeout_at(limit, receiver).await;
                        assert!(
                            Instant::now() <= limit + Duration::from_millis(1),
                            "waited past its deadline"
                        );
                        Some(answer)
                    });
                    callers.push((asked, deadline, abandon, caller));
                },
                3 | 4 => {
                    let fault =
                        [ClockFault::WallStep, ClockFault::StaleSample][usize::from(kind - 3)];
                    let mut oracle = oracle.lock().unwrap();
                    if fault == ClockFault::WallStep || oracle.1 & fault as u8 == 0 {
                        oracle.0 += 1;
                    }
                    oracle.1 |= fault as u8;
                    state.lock().unwrap().invalidate(fault);
                },
                5 => {
                    let mut oracle = oracle.lock().unwrap();
                    if !state.lock().unwrap().closing {
                        oracle.0 += 1;
                    }
                    state.lock().unwrap().begin_closing();
                    cancel.cancel();
                    closed_at.get_or_insert(Instant::now());
                },
                _ => {},
            }
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
        cancel.cancel();
        let closed_at = *closed_at.get_or_insert(Instant::now());
        prop_assert!(sampler.await.is_ok(), "the sampler failed or did not end");
        let log = handoffs.lock().unwrap().clone();
        for (asked, deadline, abandon, caller) in callers {
            let outcome = caller.await.unwrap();
            // Worst case before its own cycle ends: the cycle in flight and
            // one competing tick's cycle, 400 ms each, then its own (a 1 s tick
            // cannot be ready again 400 ms after the previous one).
            let due = asked + Duration::from_millis(2_500);
            if !abandon && deadline >= 2_500 && due < closed_at && outcome.is_some() {
                prop_assert!(
                    matches!(outcome, Some(Ok(Ok(_)))),
                    "a live request was lost: {outcome:?}"
                );
            }
            if let Some(Ok(Ok(Ok(id)))) = outcome {
                let handoff = log.iter().find(|(sample, _)| *sample == id).map(|(_, at)| *at);
                prop_assert!(
                    handoff.is_some_and(|at| at > asked),
                    "an answer from an earlier cycle"
                );
            }
        }
        prop_assert!(state.lock().unwrap().published.is_none() || !state.lock().unwrap().closing);
        Ok(())
    })
}

/// Review r3's counterexample, kept: a caller joining while the first
/// request's cycle runs, behind ready ticks, is still answered.
#[test]
fn a_request_behind_a_cycle_and_ready_ticks_is_answered() {
    dispatch_script(vec![(0, 801, 2_500)], Duration::from_secs(1)).unwrap();
}

/// Review r4: a sampler already waiting, with its next tick a minute away
/// (production's interval), is woken by a caller's request and answers it
/// well before the caller's deadline; a lost wake-up would leave the caller
/// to expire.
#[test]
fn a_waiting_sampler_wakes_for_a_request() {
    dispatch_script(vec![(0, 1_000, 2_500)], Duration::from_secs(60)).unwrap();
}

/// A non-trading TIME owner whose budget is inside its grant window (no
/// reservation can succeed), with a lazy pool and a client never reached.
fn offline_owner() -> ClockOwner {
    let clock = Arc::new(NativeClock::initialize().unwrap());
    let budget = SharedBudget::non_trading(clock.capture().unwrap(), None, false).unwrap();
    let pool = sqlx::postgres::PgPoolOptions::new()
        .connect_lazy("postgres://offline@127.0.0.1:1/none")
        .unwrap();
    let start = EpochStart {
        epoch: Epoch {
            epoch_id: Uuid::from_u128(5),
            epoch_seq: 1,
        },
        predecessor_epoch_id: None,
        predecessor: Predecessor::None,
        unclean_generation: None,
        stamped_intents: 0,
        open_incidents: Vec::new(),
    };
    let client = Arc::new(BinanceRestClient::new(String::new(), String::new()));
    ClockOwner::non_trading(clock, client, &pool, Uuid::from_u128(4), &start, &budget).unwrap()
}

/// The spawned owner's reader handle, offline: no evidence before a sample,
/// an expired request answered without any reservation, and the closing
/// joins the sampler and the watchdog (first cycle 121 s away: unclean).
#[tokio::test]
async fn an_offline_owner_answers_its_readers_and_closes() {
    let owner = offline_owner();
    assert!(owner.first > Duration::from_secs(120) && owner.first <= Duration::from_secs(121));
    let handle = owner.handle();
    let closing = owner.spawn();
    assert!(handle.clock_evidence().is_err(), "evidence before any sample");
    let expired = handle.request_sample(Instant::now()).await;
    assert_eq!(expired, Err(TimeError::Expired));
    closing.begin_closing();
    assert_eq!(
        handle.request_sample(Instant::now() + Duration::from_secs(1)).await,
        Err(TimeError::Closing)
    );
    assert!(closing.join(Instant::now() + Duration::from_secs(5)).await);
}
