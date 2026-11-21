//! Independent native observation. This child can invalidate publication;
//! only the parent's real persisted-sample CAS can publish or clear faults.
use robson_exec::{clock_records::RawClockCapture, venue_clock::ClockError};
use tokio::time::MissedTickBehavior;

use super::*;

pub(super) async fn run(
    clock: Arc<NativeClock>,
    epoch: Uuid,
    state: Arc<Mutex<ClockState>>,
    cancel: CancellationToken,
) -> Result<(), TimeError> {
    let mut ticks = tokio::time::interval(Duration::from_secs(1));
    ticks.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut previous = None;
    loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => return Ok(()),
            _ = ticks.tick() => {},
        }
        {
            let mut state = state.lock().map_err(|_| TimeError::Native)?;
            if state.closing {
                return Ok(());
            }
            // Same decision lock as the publication CAS. Native reads only;
            // no procfs, SQL, HTTP, await, budget Closing or journal changes.
            observe(&mut state, &mut previous, epoch, clock.wall_bracket(epoch));
        }
    }
}

fn observe(
    state: &mut ClockState,
    previous: &mut Option<RawClockCapture>,
    epoch: Uuid,
    capture: Result<RawClockCapture, ClockError>,
) {
    let (capture, age) = match capture.and_then(|capture| {
        if capture.epoch != epoch || capture.domain != state.start.domain() {
            return Err(ClockError::Incomparable);
        }
        let age = capture.end_tick()?.elapsed_since(&state.last_valid.unwrap_or(state.start))?;
        Ok((capture, age))
    }) {
        Ok(valid) => valid,
        Err(_) => {
            state.invalidate(ClockFault::Native);
            // Recovery must not compare across a missing or invalid read.
            *previous = None;
            return;
        },
    };
    // Compute all predicates before invalidate removes the current sample,
    // so simultaneous causes are retained instead of overwriting one another.
    let wall = previous.map(|last| last.wall_agrees(&capture));
    let stale = state
        .published
        .as_ref()
        .is_some_and(|sample| sample.arithmetic.fits_capture(&capture) != Ok(true));
    // Validation and elapsed succeeded under the caller's same decision lock.
    // Clear active Native only; visible latches/publication are never healed.
    state.native_ok();
    match wall {
        Some(Ok(true)) => state.clear_active(ClockFault::WallStep),
        Some(_) => state.invalidate(ClockFault::WallStep),
        None => {},
    }
    if stale {
        state.invalidate(ClockFault::StaleSample);
    } else {
        state.clear_active(ClockFault::StaleSample);
    }
    if age >= Duration::from_secs(120) {
        state.invalidate(ClockFault::NoValidSample);
    } else {
        state.clear_active(ClockFault::NoValidSample);
    }
    // Retain exactly one bracket; a good reading never heals a prior fault.
    *previous = Some(capture);
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECOND: i128 = 1_000_000_000;

    fn capture(tick: i128, wall_offset: i128) -> RawClockCapture {
        let wall = SECOND + tick + wall_offset;
        RawClockCapture {
            epoch: Uuid::from_u128(1),
            wall_seconds: (wall / SECOND) as i64,
            wall_nanoseconds: (wall % SECOND) as u32,
            domain: robson_exec::venue_clock::ClockDomain::new(Uuid::from_u128(2), 0, 0).unwrap(),
            before_ns: tick,
            after_ns: tick,
        }
    }

    fn state() -> ClockState {
        ClockState {
            generation: 0,
            closing: false,
            published: None,
            start: capture(0, 0).end_tick().unwrap(),
            last_valid: None,
            faults: 0,
            active_faults: 0,
        }
    }

    fn published(server_ms: i64) -> Published {
        let raw = RawClockSample {
            id: Uuid::from_u128(3),
            tenant: Uuid::from_u128(4),
            venue: VenueId::new("binance:spot"),
            send: capture(0, 0),
            receive: capture(0, 0),
            server_ms,
        };
        Published {
            id: raw.id,
            arithmetic: raw.arithmetic().unwrap(),
        }
    }

    fn tick(state: &mut ClockState, previous: &mut Option<RawClockCapture>, raw: RawClockCapture) {
        observe(state, previous, Uuid::from_u128(1), Ok(raw));
    }

    #[test]
    fn no_valid_sample_is_inclusive_at_120s_and_uses_original_receive_end() {
        for origin in [0, 100 * SECOND] {
            let mut state = state();
            if origin != 0 {
                state.last_valid = Some(capture(origin, 0).end_tick().unwrap());
            }
            let mut previous = None;
            tick(&mut state, &mut previous, capture(origin + 120 * SECOND - 1, 0));
            assert_eq!((state.faults, state.generation), (0, 0));
            tick(&mut state, &mut previous, capture(origin + 120 * SECOND, 0));
            assert_eq!(state.faults, ClockFault::NoValidSample as u8);
            assert_eq!(state.generation, 1);
            tick(&mut state, &mut previous, capture(origin + 121 * SECOND, 0));
            assert_eq!(state.generation, 1, "same reason is not a new edge");
            assert_eq!(state.active_faults, ClockFault::NoValidSample as u8);
        }
    }

    #[test]
    fn native_failure_recovery_retains_cause_and_later_no_sample_without_heal() {
        let mut state = state();
        state.published = Some(published(1000));
        let mut previous = Some(capture(0, 0));
        observe(&mut state, &mut previous, Uuid::from_u128(1), Err(ClockError::Unsupported));
        assert!(previous.is_none());
        assert!(state.published.is_none());
        assert_eq!((state.faults, state.generation), (ClockFault::Native as u8, 1));
        tick(&mut state, &mut previous, capture(120 * SECOND, 0));
        assert_eq!(state.faults, ClockFault::Native as u8 | ClockFault::NoValidSample as u8);
        assert_eq!(state.active_faults, ClockFault::NoValidSample as u8);
        assert_eq!(state.generation, 2);
        tick(&mut state, &mut previous, capture(121 * SECOND, 0));
        assert_eq!(state.generation, 2);
        assert!(state.published.is_none(), "native success cannot republish");
    }

    #[test]
    fn flat_failures_coalesce_but_good_watchdog_observation_reopens_native_edge() {
        for fault in [
            ClockFault::Native,
            ClockFault::NoValidSample,
            ClockFault::StaleSample,
        ] {
            let mut flat = state();
            flat.invalidate(fault);
            flat.invalidate(fault);
            assert_eq!(flat.generation, 1);
            assert_eq!((flat.faults, flat.active_faults), (fault as u8, fault as u8));
        }
        let mut state = state();
        let mut previous = None;
        for _ in 0..2 {
            observe(&mut state, &mut previous, Uuid::from_u128(1), Err(ClockError::Unsupported));
        }
        assert_eq!((state.generation, state.active_faults), (1, ClockFault::Native as u8));
        tick(&mut state, &mut previous, capture(SECOND, 0));
        assert_eq!((state.faults, state.active_faults), (ClockFault::Native as u8, 0));
        let candidate_generation = state.generation;
        observe(&mut state, &mut previous, Uuid::from_u128(1), Err(ClockError::Unsupported));
        assert_eq!(state.generation, candidate_generation + 1);
        assert_ne!(candidate_generation, state.generation, "prior CAS generation is invalid");
        assert!(state.published.is_none());
    }

    #[test]
    fn each_adjacent_positive_or_opposite_jump_is_a_distinct_occurrence() {
        for second_offset in [200_000_000, 0] {
            let mut state = state();
            let mut previous = Some(capture(0, 0));
            tick(&mut state, &mut previous, capture(SECOND, 100_000_000));
            assert_eq!(state.generation, 1);
            tick(&mut state, &mut previous, capture(2 * SECOND, second_offset));
            assert_eq!(state.generation, 2, "consecutive jump cannot be coalesced");
            assert_eq!(state.faults, ClockFault::WallStep as u8);
            tick(&mut state, &mut previous, capture(3 * SECOND, second_offset));
            assert_eq!(state.active_faults, 0);
            assert_eq!(state.faults, ClockFault::WallStep as u8);
            assert!(state.published.is_none(), "healthy comparison cannot republish");
        }
    }

    #[test]
    fn wall_comparison_preserves_50ms_boundary_and_refuses_native_identity_changes() {
        for (offset, fault) in [(50_000_000, false), (50_000_001, true), (-50_000_001, true)] {
            let mut state = state();
            let mut previous = Some(capture(0, 0));
            tick(&mut state, &mut previous, capture(SECOND, offset));
            assert_eq!(state.faults != 0, fault);
            if fault {
                assert_eq!(state.faults, ClockFault::WallStep as u8);
            }
        }
        for domain_change in [false, true] {
            let mut state = state();
            let mut previous = Some(capture(0, 0));
            let mut changed = capture(SECOND, 0);
            if domain_change {
                changed.domain =
                    robson_exec::venue_clock::ClockDomain::new(Uuid::from_u128(5), 0, 0).unwrap();
            } else {
                changed.epoch = Uuid::from_u128(5);
            }
            tick(&mut state, &mut previous, changed);
            assert_eq!(state.faults, ClockFault::Native as u8);
            assert!(previous.is_none());
        }
        let mut state = state();
        let mut previous = Some(capture(SECOND, 0));
        tick(&mut state, &mut previous, capture(0, 0));
        assert_eq!(state.faults, ClockFault::WallStep as u8);
    }

    #[test]
    fn published_sample_uses_original_1s_60s_and_50ms_arithmetic_limits() {
        for (server_ms, age, offset, valid) in [
            (1900, 0, 0, true),
            (1901, 0, 0, false),
            (1000, 60 * SECOND, 0, true),
            (1000, 60 * SECOND + 1, 0, false),
            (1000, SECOND, 50_000_000, true),
            (1000, SECOND, 50_000_001, false),
        ] {
            let mut state = state();
            state.published = Some(published(server_ms));
            state.last_valid = Some(capture(0, 0).end_tick().unwrap());
            tick(&mut state, &mut None, capture(age, offset));
            assert_eq!(state.published.is_some(), valid);
            assert_eq!(
                state.faults,
                if valid {
                    0
                } else {
                    ClockFault::StaleSample as u8
                }
            );
            if !valid {
                let generation = state.generation;
                tick(&mut state, &mut None, capture(age, offset));
                assert_eq!(state.generation, generation);
                assert_eq!(
                    state.active_faults, 0,
                    "no published sample is no active stale condition"
                );
                assert_eq!(state.faults, ClockFault::StaleSample as u8);
            }
        }
    }

    #[test]
    fn simultaneous_causes_accumulate_and_generation_overflow_closes_permanently() {
        let mut state = state();
        state.published = Some(published(1000));
        let mut previous = Some(capture(0, 0));
        tick(&mut state, &mut previous, capture(120 * SECOND, 50_000_001));
        assert_eq!(
            state.faults,
            ClockFault::WallStep as u8
                | ClockFault::StaleSample as u8
                | ClockFault::NoValidSample as u8
        );
        assert_eq!(state.generation, 3);
        state.generation = u64::MAX;
        observe(&mut state, &mut previous, Uuid::from_u128(1), Err(ClockError::Unsupported));
        assert!(state.closing);
        assert_eq!(state.generation, u64::MAX);
        assert_eq!(state.faults, 15);
        assert!(state.published.is_none());
    }

    #[cfg(all(
        target_os = "linux",
        target_env = "gnu",
        target_pointer_width = "64",
        any(target_arch = "x86_64", target_arch = "aarch64")
    ))]
    #[tokio::test]
    async fn cancellation_returns_normally_without_invalidating_state() {
        let clock = Arc::new(NativeClock::initialize().unwrap());
        let state = Arc::new(Mutex::new(ClockState {
            start: clock.initial_capture(),
            ..state()
        }));
        let cancel = CancellationToken::new();
        cancel.cancel();
        assert_eq!(run(clock, Uuid::from_u128(1), Arc::clone(&state), cancel).await, Ok(()));
        let state = state.lock().unwrap();
        assert_eq!((state.generation, state.faults, state.closing), (0, 0, false));
    }
}
