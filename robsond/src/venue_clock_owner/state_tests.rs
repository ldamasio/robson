//! Schedule controls on real state methods. Fault events are descriptive;
//! NativeClock good observations are actual syscalls, never a fake clock or
//! a bootstrap/persistence/Ready grant. PG races remain separate acceptance.
use proptest::prelude::*;

use super::*;

fn state(clock: &NativeClock) -> Mutex<ClockState> {
    Mutex::new(ClockState {
        generation: 0,
        closing: false,
        published: None,
        start: clock.initial_capture(),
        last_valid: None,
        faults: 0,
        active_faults: 0,
    })
}

#[test]
fn owner_good_native_observation_separates_failure_episodes_without_healing_latches() {
    let clock = NativeClock::initialize().unwrap();
    let state = state(&clock);
    let mut state = state.lock().unwrap();
    state.invalidate(ClockFault::Native);
    assert_eq!((state.generation, state.faults, state.active_faults), (1, 1, 1));
    state.capture_native(&clock, Uuid::from_u128(1)).unwrap();
    let candidate_generation = state.generation;
    assert_eq!((state.faults, state.active_faults), (1, 0), "native good never heals the latch");
    state.invalidate(ClockFault::Native);
    assert_ne!(
        candidate_generation, state.generation,
        "second Native episode revokes the candidate even before watchdog recovery observation"
    );
    assert_eq!((state.generation, state.faults, state.active_faults), (2, 1, 1));
    assert!(state.published.is_none());
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]
    #[test]
    fn active_episodes_and_real_owner_observations_preserve_independent_generation_oracle(
        events in prop::collection::vec(0u8..7,1..128),
    ) {
        let clock=NativeClock::initialize().unwrap();
        let state=state(&clock);
        let mut state=state.lock().unwrap();
        let mut generation=0u64;
        let mut active_native=false;
        let mut active_no_valid=false;
        let mut active_stale=false;
        let mut latched=0u8;
        for event in events {
            match event {
                0=>{
                    if !active_native {generation+=1;}
                    active_native=true;latched|=1;
                    state.invalidate(ClockFault::Native);
                },
                1=>{
                    state.capture_native(&clock,Uuid::from_u128(1)).unwrap();
                    active_native=false;
                },
                2=>{
                    generation+=1;latched|=2;
                    state.invalidate(ClockFault::WallStep);
                },
                3=>{
                    if !active_no_valid {generation+=1;}
                    active_no_valid=true;latched|=4;
                    state.invalidate(ClockFault::NoValidSample);
                },
                4=>{state.clear_active(ClockFault::NoValidSample);active_no_valid=false;},
                5=>{
                    if !active_stale {generation+=1;}
                    active_stale=true;latched|=8;
                    state.invalidate(ClockFault::StaleSample);
                },
                _=>{state.clear_active(ClockFault::StaleSample);active_stale=false;},
            }
            prop_assert_eq!(state.generation,generation);
            prop_assert_eq!(state.faults,latched);
            prop_assert_eq!(state.active_faults&1!=0,active_native);
            prop_assert_eq!(state.active_faults&4!=0,active_no_valid);
            prop_assert_eq!(state.active_faults&8!=0,active_stale);
            prop_assert!(state.published.is_none(),"no observation/fault event can publish");
        }
    }
}
