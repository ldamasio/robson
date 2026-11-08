//! Model properties of the ADR-0056 step 7 core, independent of its later PRs.
//!
//! Each schedule is generated as in #301 (`TestRunner::deterministic`), and
//! each rule set under test has a negative control: the same generator must
//! find a violation once the rule is weakened, or the property proves nothing.
//!
//! - The attempt rules of §2.1, in a model where commits and venue answers can
//!   be lost: `Absent` is never reported for an order the venue holds, and no
//!   further attempt on an id follows an uncertain commit. The first bounded
//!   send on an id after an uncertain commit is permitted, as §2.2 allows.
//! - The real `SubmitAttemptLedger` against an independent count of attempts.
//! - The per-stream persistence of §2.5, in a model where a simulated lock
//!   holder serializes the writers: no append after a failure on the same
//!   stream, and `last_seq` never skips an appended event. Production locking
//!   is not exercised here; it arrives with its own PR.

use std::collections::{HashMap, HashSet};

use proptest::{
    prelude::*,
    strategy::{Strategy, ValueTree},
    test_runner::TestRunner,
};
use robson_exec::venue::{AttemptEvidence, SubmitAttemptLedger};

const CASES: usize = 512;

// =============================================================================
// §2.1 attempt rules
// =============================================================================

#[derive(Clone, Copy, Debug, PartialEq)]
enum Commit {
    Landed,
    /// Refused before the statement was sent.
    KnownFailure,
    /// Timed out: the row may or may not exist.
    Uncertain {
        row_exists: bool,
    },
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum VenueAnswer {
    Accepted,
    /// Accepted, but the answer never came back.
    AcceptedAnswerLost,
    RejectedAnswerLost,
    Rejected,
}

#[derive(Clone, Copy, Debug)]
enum Step {
    /// A send attempt on a client id, after its pre-send commit.
    Attempt {
        id: u8,
        bounded: bool,
        commit: Commit,
        answer: VenueAnswer,
    },
    /// A retried `started` upsert for an earlier attempt arrives late.
    ReplayStarted { id: u8, attempt: usize },
    /// Process death: only durable rows survive.
    Restart,
}

/// The rules as the design states them, each of which a negative control can
/// switch off.
#[derive(Clone, Copy)]
struct Rules {
    /// Rule 1: `rejected` absorbs `started`.
    rejected_absorbs_started: bool,
    /// Rule 2: a further attempt commits together with every earlier attempt
    /// known only in memory.
    further_attempt_commits_earlier_ones: bool,
    /// Rule 2: an uncertain commit never authorizes a same-id send.
    uncertain_commit_refuses_the_send: bool,
    /// Rule 4: `Absent` needs at least one row.
    absent_requires_a_row: bool,
}

const DESIGN: Rules = Rules {
    rejected_absorbs_started: true,
    further_attempt_commits_earlier_ones: true,
    uncertain_commit_refuses_the_send: true,
    absent_requires_a_row: true,
};

#[derive(Clone, Copy, PartialEq)]
enum Row {
    Started,
    Rejected,
}

/// The journal, and the venue it cannot see.
#[derive(Default)]
struct World {
    rows: HashMap<u8, Vec<Row>>,
    /// Attempts known only in memory.
    unlanded: HashMap<u8, u32>,
    dead_ids: HashSet<u8>,
    /// Ground truth, invisible to the journal.
    venue_holds: HashSet<u8>,
    violations: Vec<String>,
}

impl World {
    fn apply(&mut self, step: Step, rules: Rules) {
        match step {
            Step::Attempt { id, bounded, commit, answer } => {
                self.attempt(id, bounded, commit, answer, rules)
            },
            Step::ReplayStarted { id, attempt } => {
                if let Some(row) = self.rows.get_mut(&id).and_then(|rows| rows.get_mut(attempt)) {
                    if *row == Row::Rejected && !rules.rejected_absorbs_started {
                        *row = Row::Started;
                        self.violations.push(format!("id {id} went back to started"));
                    }
                }
            },
            Step::Restart => {
                // Rule 3: no later process sends an id again.
                self.dead_ids.extend(self.rows.keys().copied());
                self.dead_ids.extend(self.unlanded.keys().copied());
                self.unlanded.clear();
            },
        }
    }

    fn attempt(
        &mut self,
        id: u8,
        bounded: bool,
        commit: Commit,
        answer: VenueAnswer,
        rules: Rules,
    ) {
        if self.dead_ids.contains(&id) {
            return;
        }
        let further_attempt = self.rows.contains_key(&id) || self.unlanded.contains_key(&id);
        let landed = match commit {
            Commit::Landed => true,
            Commit::KnownFailure => false,
            Commit::Uncertain { row_exists } => row_exists,
        };
        let refused = match commit {
            Commit::Landed => false,
            Commit::KnownFailure => !bounded || further_attempt,
            Commit::Uncertain { .. } => {
                !bounded || (further_attempt && rules.uncertain_commit_refuses_the_send)
            },
        };
        if landed {
            self.rows.entry(id).or_default().push(Row::Started);
            if rules.further_attempt_commits_earlier_ones {
                let earlier = self.unlanded.remove(&id).unwrap_or(0);
                self.rows.entry(id).or_default().extend((0..earlier).map(|_| Row::Started));
            }
        }
        if refused {
            // The caller mints a new id; this one is never sent again.
            self.dead_ids.insert(id);
            return;
        }
        if further_attempt && matches!(commit, Commit::Uncertain { .. }) {
            self.violations.push(format!("id {id} was sent after an uncertain commit"));
        }

        // The send. A bounded send whose row did not land is unlanded.
        if !landed {
            *self.unlanded.entry(id).or_default() += 1;
        }
        match answer {
            VenueAnswer::Accepted | VenueAnswer::AcceptedAnswerLost => {
                self.venue_holds.insert(id);
            },
            VenueAnswer::Rejected if landed => {
                if let Some(row) = self.rows.get_mut(&id).and_then(|rows| rows.first_mut()) {
                    *row = Row::Rejected;
                }
            },
            VenueAnswer::Rejected | VenueAnswer::RejectedAnswerLost => {},
        }
    }

    /// Rule 4, given the venue's not-found.
    fn absent(&self, id: u8, rules: Rules) -> bool {
        let rows = self.rows.get(&id).map(Vec::as_slice).unwrap_or_default();
        (!rows.is_empty() || !rules.absent_requires_a_row)
            && rows.iter().all(|row| *row == Row::Rejected)
            && !self.unlanded.contains_key(&id)
    }

    /// `Absent` for an id the venue holds is the false finality rule 4 exists
    /// to prevent.
    fn check_absent(&mut self, rules: Rules) {
        for id in 0..ID_SPACE {
            if self.venue_holds.contains(&id) && self.absent(id, rules) {
                self.violations.push(format!("id {id} reads Absent but the venue holds it"));
            }
        }
    }
}

const ID_SPACE: u8 = 4;

fn step() -> impl Strategy<Value = Step> {
    let commit = prop_oneof![
        Just(Commit::Landed),
        Just(Commit::KnownFailure),
        any::<bool>().prop_map(|row_exists| Commit::Uncertain { row_exists }),
    ];
    let answer = prop_oneof![
        Just(VenueAnswer::Accepted),
        Just(VenueAnswer::AcceptedAnswerLost),
        Just(VenueAnswer::Rejected),
        Just(VenueAnswer::RejectedAnswerLost),
    ];
    let id = 0..ID_SPACE;
    prop_oneof![
        6 => (id.clone(), any::<bool>(), commit, answer)
            .prop_map(|(id, bounded, commit, answer)| Step::Attempt { id, bounded, commit, answer }),
        1 => (id, 0usize..4).prop_map(|(id, attempt)| Step::ReplayStarted { id, attempt }),
        1 => Just(Step::Restart),
    ]
}

fn run_attempts(rules: Rules) -> Vec<String> {
    let schedules = proptest::collection::vec(step(), 1..40);
    let mut runner = TestRunner::deterministic();
    let mut found = Vec::new();
    for _ in 0..CASES {
        let schedule = schedules.new_tree(&mut runner).unwrap().current();
        let mut world = World::default();
        for step in &schedule {
            world.apply(*step, rules);
            // A false `Absent` at any moment counts, not just at the end.
            world.check_absent(rules);
        }
        found.extend(world.violations.into_iter().map(|v| format!("{v}: {schedule:?}")));
    }
    found
}

#[test]
fn in_the_attempt_model_absent_never_covers_a_held_order_and_no_further_attempt_follows_an_uncertain_commit(
) {
    assert_eq!(run_attempts(DESIGN), Vec::<String>::new());
}

#[test]
fn every_attempt_rule_is_needed() {
    let weakenings = [
        ("rejected absorbs started", Rules {
            rejected_absorbs_started: false,
            ..DESIGN
        }),
        ("completeness", Rules {
            further_attempt_commits_earlier_ones: false,
            ..DESIGN
        }),
        ("uncertain commit", Rules {
            uncertain_commit_refuses_the_send: false,
            ..DESIGN
        }),
        ("at least one row", Rules { absent_requires_a_row: false, ..DESIGN }),
    ];
    for (rule, weakened) in weakenings {
        assert!(!run_attempts(weakened).is_empty(), "the property must detect losing: {rule}");
    }
}

// =============================================================================
// The real SubmitAttemptLedger
// =============================================================================

#[derive(Clone, Copy, Debug)]
enum LedgerStep {
    Begin(u8),
    BeginThenReject(u8),
}

/// Ids the capacity can hold, plus one so some schedules truncate.
const LEDGER_IDS: u8 = 4;
const LEDGER_CAPACITY: usize = 3;

#[test]
fn the_real_ledger_reports_an_attempt_as_unresolved_until_it_is_rejected() {
    let step = prop_oneof![
        (0..LEDGER_IDS).prop_map(LedgerStep::Begin),
        (0..LEDGER_IDS).prop_map(LedgerStep::BeginThenReject),
    ];
    let schedules = proptest::collection::vec(step, 0..60);
    let mut runner = TestRunner::deterministic();

    for _ in 0..CASES {
        let schedule = schedules.new_tree(&mut runner).unwrap().current();
        let ledger = SubmitAttemptLedger::new(LEDGER_CAPACITY);
        // The oracle counts independently of the ledger.
        let mut unresolved = HashMap::<u8, u32>::new();
        let mut rejected = HashMap::<u8, u32>::new();
        let mut seen = HashSet::new();
        for step in &schedule {
            let (id, reject) = match *step {
                LedgerStep::Begin(id) => (id, false),
                LedgerStep::BeginThenReject(id) => (id, true),
            };
            seen.insert(id);
            let client_id = id.to_string();
            let attempt = ledger.begin(&client_id);
            if reject {
                attempt.rejected();
                *rejected.entry(id).or_default() += 1;
            } else {
                *unresolved.entry(id).or_default() += 1;
            }
        }

        let truncated = seen.len() > LEDGER_CAPACITY;
        for id in 0..LEDGER_IDS {
            let evidence = ledger.evidence(&id.to_string());
            let unresolved_here = unresolved.get(&id).copied().unwrap_or(0);
            let case = format!("id {id} in {schedule:?}: {evidence:?}");
            match evidence {
                AttemptEvidence::Truncated => assert!(truncated, "{case}"),
                AttemptEvidence::Unavailable => panic!("this ledger has no journal: {case}"),
                AttemptEvidence::NoRecord => {
                    assert!(truncated || !seen.contains(&id), "{case}")
                },
                AttemptEvidence::AllRejected { .. } => {
                    assert_eq!(unresolved_here, 0, "unresolved read as rejected: {case}");
                    assert!(!truncated && seen.contains(&id), "{case}");
                },
                AttemptEvidence::Unresolved {
                    unresolved: reported,
                    rejected: rejected_n,
                } => {
                    assert!(unresolved_here > 0, "{case}");
                    assert_eq!(reported as u32, unresolved_here, "{case}");
                    assert_eq!(
                        rejected_n as u32,
                        rejected.get(&id).copied().unwrap_or(0),
                        "{case}"
                    );
                },
            }
        }
    }
}

// =============================================================================
// §2.5 per-stream persistence
// =============================================================================

#[derive(Clone, Copy, Debug, PartialEq)]
enum Injected {
    None,
    AppendFails,
    ApplyFails,
}

#[derive(Clone, Copy, PartialEq)]
enum Pc {
    Check,
    Append,
    Apply(u64),
    Done,
}

#[derive(Default)]
struct StreamWorld {
    next_seq: u64,
    last_seq: u64,
    suspended: bool,
    failed: bool,
    holder: Option<usize>,
    violations: Vec<String>,
}

/// One step of writer `w`: check, append, apply. With `serialize`, a simulated
/// holder keeps the stream for the writer through the whole sequence; without
/// it, a failure can land between another writer's check and its append.
fn step_writer(
    world: &mut StreamWorld,
    pc: &mut Pc,
    w: usize,
    injected: Injected,
    serialize: bool,
) {
    if serialize && world.holder.is_some_and(|holder| holder != w) {
        return;
    }
    match *pc {
        Pc::Check => {
            if serialize {
                world.holder = Some(w);
            }
            if world.suspended {
                world.holder = None;
                *pc = Pc::Done;
            } else {
                *pc = Pc::Append;
            }
        },
        Pc::Append => {
            if world.failed {
                world.violations.push(format!("writer {w} appended after a failed write"));
            }
            if injected == Injected::AppendFails {
                world.failed = true;
                world.suspended = true;
                world.holder = None;
                *pc = Pc::Done;
                return;
            }
            world.next_seq += 1;
            *pc = Pc::Apply(world.next_seq);
        },
        Pc::Apply(seq) => {
            if injected == Injected::ApplyFails {
                world.failed = true;
                world.suspended = true;
            } else {
                if seq != world.last_seq + 1 {
                    world.violations.push(format!(
                        "last_seq moved from {} to {seq}, skipping an appended event",
                        world.last_seq
                    ));
                }
                world.last_seq = world.last_seq.max(seq);
            }
            world.holder = None;
            *pc = Pc::Done;
        },
        Pc::Done => {},
    }
}

fn run_writers(serialize: bool) -> Vec<String> {
    let injected = prop_oneof![
        3 => Just(Injected::None),
        1 => Just(Injected::AppendFails),
        1 => Just(Injected::ApplyFails),
    ];
    let writers = proptest::collection::vec(injected, 2..5);
    let schedule = proptest::collection::vec(0usize..4, 8..48);
    let mut runner = TestRunner::deterministic();
    let mut violations = Vec::new();
    for _ in 0..CASES {
        let writers = writers.new_tree(&mut runner).unwrap().current();
        let schedule = schedule.new_tree(&mut runner).unwrap().current();
        let mut world = StreamWorld::default();
        let mut pcs = vec![Pc::Check; writers.len()];
        for slot in &schedule {
            let w = slot % writers.len();
            step_writer(&mut world, &mut pcs[w], w, writers[w], serialize);
        }
        violations.extend(world.violations.into_iter().map(|v| format!("{v}: {writers:?}")));
    }
    violations
}

#[test]
fn with_a_simulated_stream_lock_writers_never_append_after_a_failure_or_skip_a_sequence() {
    assert_eq!(run_writers(true), Vec::<String>::new());
}

#[test]
fn without_the_simulated_stream_lock_the_writers_break_the_property() {
    assert!(!run_writers(false).is_empty(), "the property must detect a missing stream lock");
}
