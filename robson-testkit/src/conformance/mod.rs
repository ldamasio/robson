//! The venue conformance suite (ADR-0056 §10, step 8, 8d plan §2.4 and §4):
//! a fixed inventory of 22 clauses, a coverage class per clause per venue,
//! the scenarios that execute them, and [`check`], which accepts a run only
//! when it reports every clause once, exactly as its table classifies it.
//!
//! - **Inventory:** [`INVENTORY`], one entry per clause of §4's matrix.
//! - **Coverage** ([`Coverage`]): `Verified` (the venue simulates the scenario
//!   and the fixture executes the clause), `Scripted` (the venue's answer is
//!   injected: the helper's handling is asserted, never the venue's execution),
//!   `Characterized` (an existing outcome pinned exactly), `Pending` (the
//!   fixture cannot execute a clause that applies: not run) and `NotApplicable`
//!   (the clause's contract does not exist on this account kind: not run).
//! - **Tables:** each run declares a `const` table of [`Expected`] entries,
//!   reviewed with it; nothing turns a failure into `Pending` or
//!   `NotApplicable` at run time. [`run_table`] runs the scenarios of the
//!   entries that are run and reports the others as not run.
//! - **[`check`]** fails on a missing, duplicate or unknown clause in the table
//!   or the outcomes, on any failed or unrun scenario, on an outcome whose
//!   coverage differs from the table's, on a scenario that ran with no
//!   evidence, and on applicability: `NotApplicable` exactly where the clause
//!   does not apply to the venue's capabilities ([`applies`]), so it is never a
//!   fixture's limit in disguise.
//! - **Scenarios** ([`scenarios`]) drive the venue through its ports, as the
//!   core would, and the harness's hooks, as the market or the venue would, and
//!   assert their preconditions and effects from the venue's own state; each
//!   records a [`Trace`].
//!
//! Test support: no production crate depends on this crate.

pub mod scenarios;

use std::{fmt, future::Future, pin::Pin};

use robson_domain::Symbol;
use robson_exec::{
    venue::{AccountKind, ReplacementProtocol, Venue, VenueCapabilities},
    ExecError,
};

/// One clause of the conformance matrix (8d plan §4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Property {
    F1,
    F2,
    F3,
    N1,
    N2,
    N3a,
    N3b,
    R1,
    R2,
    R3,
    A1a,
    A1b,
    A2,
    A3,
    A4,
    A5b,
    A5p,
    B1,
    B2,
    B3,
    B4,
    B5,
}

use Property::*;

/// Every clause, in the matrix's order.
pub const INVENTORY: [Property; 22] = [
    F1, F2, F3, N1, N2, N3a, N3b, R1, R2, R3, A1a, A1b, A2, A3, A4, A5b, A5p, B1, B2, B3, B4, B5,
];

/// How a venue's run covers a clause.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Coverage {
    Verified,
    Scripted,
    Characterized,
    /// Not run: the fixture cannot execute it; why, and whose it is.
    Pending(&'static str),
    /// Not run: the clause's contract does not exist on this account kind.
    NotApplicable(&'static str),
}

impl Coverage {
    /// Whether a scenario runs for it.
    pub fn runs(self) -> bool {
        matches!(self, Self::Verified | Self::Scripted | Self::Characterized)
    }
}

/// A run's table entry: the clause and its coverage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Expected {
    pub property: Property,
    pub coverage: Coverage,
}

/// Whether a clause's contract exists on a venue with `capabilities` (8d
/// plan §4): R1 is `PlaceThenCancel`'s, R2 and R3 `CancelThenPlace`'s, A5b
/// and B1 to B5 a balance account's, A5p a position account's; every other
/// clause applies everywhere.
pub fn applies(property: Property, capabilities: &VenueCapabilities) -> bool {
    let balance = capabilities.account_kind == AccountKind::BalanceAccount;
    match property {
        R1 => capabilities.replacement == ReplacementProtocol::PlaceThenCancel,
        R2 | R3 => capabilities.replacement == ReplacementProtocol::CancelThenPlace,
        A5b | B1 | B2 | B3 | B4 | B5 => balance,
        A5p => !balance,
        _ => true,
    }
}

/// One thing a scenario saw or did, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// A port call, with what it was asked.
    Sent { call: String },
    /// Its answer, as given.
    Answered { call: String, answer: String },
    /// What the harness made the market or the venue do.
    Market { action: String },
    /// The venue's own state, read without a port call.
    State { what: String },
}

/// A scenario's evidence.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Trace(pub Vec<Event>);

/// A scenario's verdict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Pass,
    Fail(Vec<String>),
    /// Pending or not applicable: no scenario ran.
    NotRun,
}

/// What a run reports for one clause.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub property: Property,
    /// The coverage the scenario provides, which the table must name.
    pub coverage: Coverage,
    pub verdict: Verdict,
    pub trace: Trace,
}

impl Outcome {
    /// A clause the table does not run.
    pub fn not_run(expected: Expected) -> Self {
        Self {
            property: expected.property,
            coverage: expected.coverage,
            verdict: Verdict::NotRun,
            trace: Trace::default(),
        }
    }

    /// A clause the run has no scenario for: it can never pass.
    pub fn no_scenario(property: Property) -> Self {
        Self {
            property,
            coverage: Coverage::Pending("no scenario"),
            verdict: Verdict::Fail(vec![format!("{property:?} has no scenario on this venue")]),
            trace: Trace::default(),
        }
    }
}

/// A scenario in progress: its trace and its failed expectations, never a
/// panic, so a report names every failure.
pub struct Run {
    property: Property,
    coverage: Coverage,
    trace: Trace,
    failures: Vec<String>,
}

impl Run {
    pub fn new(property: Property, coverage: Coverage) -> Self {
        Self {
            property,
            coverage,
            trace: Trace::default(),
            failures: Vec::new(),
        }
    }

    /// Record a failure unless `holds`.
    pub fn expect(&mut self, holds: bool, what: impl fmt::Display) {
        if !holds {
            self.failures.push(what.to_string());
        }
    }

    pub fn sent(&mut self, call: impl Into<String>) {
        self.trace.0.push(Event::Sent { call: call.into() });
    }

    /// Record a port call's answer and hand it back.
    pub fn answered<T: fmt::Debug>(&mut self, call: impl Into<String>, answer: T) -> T {
        let answer_text = format!("{answer:?}");
        let answer_text: String = answer_text.chars().take(400).collect();
        self.trace.0.push(Event::Answered { call: call.into(), answer: answer_text });
        answer
    }

    pub fn market(&mut self, action: impl Into<String>) {
        self.trace.0.push(Event::Market { action: action.into() });
    }

    pub fn state(&mut self, what: impl Into<String>) {
        self.trace.0.push(Event::State { what: what.into() });
    }

    pub fn finish(self) -> Outcome {
        let verdict = if self.failures.is_empty() {
            Verdict::Pass
        } else {
            Verdict::Fail(self.failures)
        };
        Outcome {
            property: self.property,
            coverage: self.coverage,
            verdict,
            trace: self.trace,
        }
    }
}

/// A boxed, sendable future, for the harness's hooks.
pub type Boxed<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// What every venue's harness provides (8d plan §2.4, `ConformanceVenue`).
/// Each scenario builds a fresh harness, so no state crosses clauses.
pub trait Harness: Send + Sync {
    type V: Venue;
    fn venue(&self) -> &Self::V;
    /// The symbol it trades, seeded with a price of 100 and a holding of 1.
    fn symbol(&self) -> Symbol;
    /// Run one placement as the owner does: under a fresh grant where the
    /// venue sends one attempt per client id, then settled, a success when
    /// `accepted` says the venue took it.
    fn sending<'a, T: Send + 'a>(
        &'a self,
        client_id: &'a str,
        send: Boxed<'a, Result<T, ExecError>>,
        accepted: fn(&Result<T, ExecError>) -> bool,
    ) -> Boxed<'a, Result<T, ExecError>>;
    /// The venue authoritatively refuses the next placement.
    fn reject_next_placement(&self);
    /// The next placement reaches the venue and its answer is lost.
    fn lose_next_placement(&self);
    /// Hide `client_id`'s order from lookups, if the venue holds it.
    fn conceal(&self, client_id: &str);
    /// Show it again.
    fn reveal(&self, client_id: &str);
    /// Placements that reached the venue.
    fn placements(&self) -> u64;
    /// The exchange ids of the stops live on the venue, its own view.
    fn open_stops(&self) -> Vec<String>;
}

/// Run a table: every entry that runs through `scenario`, every other
/// reported as not run, in the table's order.
pub async fn run_table<F, Fut>(table: &[Expected], mut scenario: F) -> Vec<Outcome>
where
    F: FnMut(Property) -> Fut,
    Fut: Future<Output = Outcome>,
{
    let mut outcomes = Vec::with_capacity(table.len());
    for expected in table {
        if expected.coverage.runs() {
            outcomes.push(scenario(expected.property).await);
        } else {
            outcomes.push(Outcome::not_run(*expected));
        }
    }
    outcomes
}

/// Accept a run only when its table and its outcomes cover the inventory
/// exactly, as the module documentation states.
///
/// # Errors
/// Every reason the run is not accepted.
pub fn check(
    capabilities: &VenueCapabilities,
    table: &[Expected],
    outcomes: &[Outcome],
) -> Result<(), Vec<String>> {
    let mut problems = Vec::new();
    for property in INVENTORY {
        let entries = table.iter().filter(|e| e.property == property).count();
        if entries != 1 {
            problems.push(format!("the table has {entries} entries for {property:?}"));
        }
        let reported = outcomes.iter().filter(|o| o.property == property).count();
        match reported {
            1 => {},
            0 => problems.push(format!("{property:?} is missing from the outcomes")),
            n => problems.push(format!("{property:?} is reported {n} times")),
        }
    }
    if table.len() != INVENTORY.len() {
        problems.push(format!("the table has {} entries, not {}", table.len(), INVENTORY.len()));
    }
    for expected in table {
        let applicable = applies(expected.property, capabilities);
        let not_applicable = matches!(expected.coverage, Coverage::NotApplicable(_));
        if applicable == not_applicable {
            problems.push(format!(
                "{:?} is classified {:?}, but on this venue it {}",
                expected.property,
                expected.coverage,
                if applicable {
                    "applies"
                } else {
                    "does not apply"
                }
            ));
        }
    }
    for outcome in outcomes {
        let Some(expected) = table.iter().find(|e| e.property == outcome.property) else {
            problems.push(format!("{:?} is not in the table", outcome.property));
            continue;
        };
        if outcome.coverage != expected.coverage {
            problems.push(format!(
                "{:?} ran as {:?}, the table says {:?}",
                outcome.property, outcome.coverage, expected.coverage
            ));
        }
        match (&outcome.verdict, expected.coverage.runs()) {
            (Verdict::Pass, true) if outcome.trace.0.is_empty() => {
                problems.push(format!("{:?} passed with no evidence", outcome.property));
            },
            (Verdict::Pass, true) | (Verdict::NotRun, false) => {},
            (Verdict::Fail(why), _) => {
                problems.push(format!("{:?} failed: {}", outcome.property, why.join("; ")));
            },
            (Verdict::NotRun, true) => {
                problems.push(format!("{:?} was not run", outcome.property));
            },
            (Verdict::Pass, false) => {
                problems
                    .push(format!("{:?} ran though the table does not run it", outcome.property));
            },
        }
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems)
    }
}

/// The table's entries that are not run, for a run's report.
pub fn pending_list(table: &[Expected]) -> Vec<String> {
    table
        .iter()
        .filter(|e| !e.coverage.runs())
        .map(|e| format!("{:?}: {:?}", e.property, e.coverage))
        .collect()
}
