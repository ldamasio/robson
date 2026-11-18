//! Generated schedule space for the native property test: feasible milestone
//! triples, demonstrated cancellation cuts and state-dependent round
//! operations.
use proptest::{
    prelude::*,
    strategy::ValueTree,
    test_runner::{RngAlgorithm, TestRng, TestRunner},
};

use super::*;

#[derive(Clone, Copy, Debug)]
pub(super) enum Round {
    Normal,
    LostReply,
    Cancel(Cut),
    Deferred,
    Ordinary39,
}

#[derive(Clone, Debug)]
pub(super) struct Schedule {
    pub(super) milestones: [u8; 3],
    // The commit-rejecting trigger is installed before B0 for the whole
    // fixture, so a deferred schedule only mixes Deferred and Cancel.
    pub(super) deferred_fixture: bool,
    pub(super) rounds: Vec<Round>,
}

pub(super) const MASTER_SEED: [u8; 32] = *b"robson-migration38-native-props1";
pub(super) const GENERATED_CASES: usize = 16;

// Feasible triples only: every received window <= 18 statements and COMMIT
// is statement 55, so m0 <= 16, consecutive gaps <= 18 and m2 >= 35, which
// forces m1 >= 17. The ranges are proven non-empty and valid exhaustively in
// milestone_ranges_are_feasible_and_complete, not by sampled runs.
const M0: std::ops::RangeInclusive<u8> = 1..=16;
fn m1_range(m0: u8) -> std::ops::RangeInclusive<u8> {
    (m0 + 1).max(17)..=(m0 + 18).min(51)
}
fn m2_range(m1: u8) -> std::ops::RangeInclusive<u8> {
    (m1 + 1).max(35)..=(m1 + 18).min(52)
}
fn feasible(m: [u8; 3]) -> bool {
    M0.contains(&m[0])
        && m[0] < m[1]
        && m[1] < m[2]
        && m[2] <= 52
        && m[1] - m[0] <= 18
        && m[2] - m[1] <= 18
        && m[2] >= 35
}
fn milestones() -> impl Strategy<Value = [u8; 3]> {
    M0.prop_flat_map(|a| (Just(a), m1_range(a)))
        .prop_flat_map(|(a, b)| (Just(a), Just(b), m2_range(b)))
        .prop_map(|(a, b, c)| [a, b, c])
}

#[test]
fn milestone_ranges_are_feasible_and_complete() {
    let mut generated = std::collections::BTreeSet::new();
    for a in M0 {
        let r1 = m1_range(a);
        assert!(!r1.is_empty(), "empty m1 range for m0={a}");
        for b in r1 {
            let r2 = m2_range(b);
            assert!(!r2.is_empty(), "empty m2 range for m0={a} m1={b}");
            for c in r2 {
                assert!(feasible([a, b, c]), "infeasible generated triple {a},{b},{c}");
                generated.insert([a, b, c]);
            }
        }
    }
    // Completeness: every feasible triple is reachable by the strategy.
    let mut all = 0;
    for a in 1u8..=52 {
        for b in 1u8..=52 {
            for c in 1u8..=52 {
                if feasible([a, b, c]) {
                    all += 1;
                    assert!(
                        generated.contains(&[a, b, c]),
                        "feasible triple {a},{b},{c} not generated"
                    );
                }
            }
        }
    }
    assert_eq!(generated.len(), all);
}

// Only the cancellation cuts already demonstrated by the fixed controls.
fn cut() -> impl Strategy<Value = Cut> {
    prop_oneof![
        Just(Cut::BeforeDdl(1)),
        Just(Cut::AfterDdl(13)),
        Just(Cut::BeforeDdl(27)),
        Just(Cut::AfterDdl(41)),
        Just(Cut::BeforeMetadata),
        Just(Cut::BeforeCommit),
    ]
}

fn unapplied(deferred: bool) -> BoxedStrategy<Round> {
    if deferred {
        prop_oneof![Just(Round::Deferred), cut().prop_map(Round::Cancel)].boxed()
    } else {
        prop_oneof![
            Just(Round::Normal),
            Just(Round::LostReply),
            cut().prop_map(Round::Cancel)
        ]
        .boxed()
    }
}

pub(super) fn schedule() -> impl Strategy<Value = Schedule> {
    (milestones(), any::<bool>(), 1usize..=2).prop_flat_map(|(milestones, deferred, n)| {
        unapplied(deferred).prop_flat_map(move |first| {
            let commits = matches!(first, Round::Normal | Round::LostReply);
            let second = if commits {
                Just(Round::Ordinary39).boxed()
            } else {
                unapplied(deferred)
            };
            let rounds = if n == 1 {
                Just(vec![first]).boxed()
            } else {
                second.prop_map(move |s| vec![first, s]).boxed()
            };
            rounds.prop_map(move |rounds| Schedule {
                milestones,
                deferred_fixture: deferred,
                rounds,
            })
        })
    })
}

/// Schedule `index` is the `index + 1`-th draw of one runner seeded with the
/// fixed master seed, so every exact test selector derives the same schedule.
pub(super) fn generated_schedule(index: usize) -> Schedule {
    assert!(index < GENERATED_CASES, "generated index outside 0..16");
    let mut runner = TestRunner::new_with_rng(
        ProptestConfig::default(),
        TestRng::from_seed(RngAlgorithm::ChaCha, &MASTER_SEED),
    );
    let strategy = schedule();
    let mut drawn = None;
    for _ in 0..=index {
        drawn = Some(
            strategy
                .new_tree(&mut runner)
                .expect("schedule strategy has no filters")
                .current(),
        );
    }
    drawn.expect("at least one draw")
}

#[test]
fn generated_schedules_are_reproducible_distinct_and_feasible() {
    let first: Vec<_> = (0..GENERATED_CASES).map(generated_schedule).collect();
    let again: Vec<_> = (0..GENERATED_CASES).map(generated_schedule).collect();
    let key = |s: &Schedule| format!("{s:?}");
    assert_eq!(
        first.iter().map(key).collect::<Vec<_>>(),
        again.iter().map(key).collect::<Vec<_>>()
    );
    let triples: std::collections::BTreeSet<_> = first.iter().map(|s| s.milestones).collect();
    let schedules: std::collections::BTreeSet<_> = first.iter().map(key).collect();
    assert!(triples.len() >= 2, "generated milestone triples are not distinct: {triples:?}");
    assert!(schedules.len() >= 3, "generated schedules are not distinct: {schedules:?}");
    for s in &first {
        assert!(feasible(s.milestones), "infeasible generated schedule {s:?}");
        assert!(!s.rounds.is_empty() && s.rounds.len() <= 2, "round count outside 1..=2: {s:?}");
    }
}
