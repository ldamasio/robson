//! 8k-2 pilot detector (ADR-0056 step 8k; v16 §3.10, §C8): the operated
//! pair's base and locked base against Robson's durable ledger. Inert: no
//! production caller. It reads and classifies; [`apply`] records the one
//! consequence 8k-1 B needs, `venue_discrepancy` on the gate, and nothing here
//! writes. Production transport (8k-1), the other consequences and late
//! evidence (8k-3), release (8k-4) and cash (8l) are later slices. USDT and
//! every other asset are not evaluated here (SD7).
//!
//! One attempt: gate marks, the pre statement ([`read_pre`]), the inventory
//! triple, the operated subset and the keyed attribution post statement,
//! gate marks again, then [`qualify`]. Only a qualified attempt is
//! classified; [`Scan`] keeps at most three attempts and their actual cost.
use robson_exec::{
    clock_records::RawClockCapture,
    gate::{IncidentReason, Occurrence, OperationGate},
};
use rust_decimal::Decimal;
use uuid::Uuid;

use crate::spot_order_set_store::ExpectedGeneration;

mod classify;
mod snapshot;

pub(crate) use classify::{classify, consequence, Classification};
pub(crate) use snapshot::{
    read_pre, IntentRow, LegRow, LinkedOrder, OrderRow, PreRefusal, PreSnapshot,
};

#[derive(Clone, Copy)]
pub(crate) struct DetectorScope<'a> {
    pub(crate) expected: ExpectedGeneration<'a>,
    pub(crate) uid: &'a str,
    /// The exact raw operated pair; nothing else is parsed as an asset.
    pub(crate) operated: &'a str,
    /// From the route's rules; the ledger does not store it.
    pub(crate) base_asset: &'a str,
}

/// How the keyed attribution judged one operated open order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Attribution {
    Absent,
    Discovered,
    Unauthored,
    Ledger { intent_id: Uuid },
    Contradiction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OperatedOrder {
    pub(crate) symbol: String,
    pub(crate) order_id: u64,
    pub(crate) attribution: Attribution,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OperatedList {
    pub(crate) order_list_id: i64,
}

/// The operated subset of one inventory triple. Unrelated pairs and lists
/// are only counted, never parsed as another asset.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct Listing {
    pub(crate) orders: Vec<OperatedOrder>,
    pub(crate) lists: Vec<OperatedList>,
    pub(crate) unrelated_orders: usize,
    pub(crate) unrelated_lists: usize,
}

/// The account request interval `I = [send − u, receive + u]` and its cut
/// `send − u` (v16 §C8 step 2), in wall nanoseconds. `u` is ⌈u2 / 2⌉ of the
/// sample the ACCOUNT receipt was judged on; rounding up only widens `I`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ObservationInterval {
    pub(crate) start_ns: i128,
    pub(crate) end_ns: i128,
    pub(crate) cut_ns: i128,
}

impl ObservationInterval {
    /// From the ACCOUNT receipt's own send and receive brackets only.
    pub(crate) fn from_account(
        send: &RawClockCapture,
        receive: &RawClockCapture,
        u2: i128,
    ) -> Option<Self> {
        if u2 < 0 || send.epoch != receive.epoch || send.after_ns >= receive.before_ns {
            return None;
        }
        let wall = |capture: &RawClockCapture| {
            i128::from(capture.wall_seconds)
                .checked_mul(1_000_000_000)?
                .checked_add(i128::from(capture.wall_nanoseconds))
        };
        let u = u2.checked_add(1)? / 2;
        let start = wall(send)?.checked_sub(u)?;
        let end = wall(receive)?.checked_add(u)?;
        (start <= end).then_some(Self {
            start_ns: start,
            end_ns: end,
            cut_ns: start,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PostOutcome {
    Matched,
    /// The keyed snapshot returned no row: scope, UID, generation, epoch or
    /// the accounting version moved after pre.
    ScopeMoved,
    Sql,
    Timeout,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClockOutcome {
    Valid(ObservationInterval),
    Invalid,
}

/// What one attempt that reached its post statement observed. Generation and
/// pending are read in memory before pre and after post.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct QualificationMarks {
    pub(crate) generation_before: i64,
    pub(crate) generation_after: i64,
    pub(crate) pending_before: bool,
    pub(crate) pending_after: bool,
    pub(crate) post: PostOutcome,
    pub(crate) clock: ClockOutcome,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UnqualifiedReason {
    VersionChanged,
    GenerationChanged,
    PendingEvidence,
    Clock,
    Venue,
    Sql,
    Timeout,
    /// Pre found no row: scope, UID, generation or epoch is not current.
    Scope,
    /// Pre returned more rows than the fixture cap; nothing was decoded.
    Cap,
    Malformed,
}

impl UnqualifiedReason {
    /// A refusal no retry can change within one scan.
    fn permanent(self) -> bool {
        matches!(self, Self::Scope | Self::Cap | Self::Malformed)
    }
}

impl From<PreRefusal> for UnqualifiedReason {
    fn from(refusal: PreRefusal) -> Self {
        match refusal {
            PreRefusal::Scope => Self::Scope,
            PreRefusal::Cap => Self::Cap,
            PreRefusal::Malformed => Self::Malformed,
            PreRefusal::Timeout => Self::Timeout,
            PreRefusal::Sql => Self::Sql,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AttemptOutcome {
    Qualified(ObservationInterval),
    Unqualified(UnqualifiedReason),
}

/// C8 steps 1, 3 and 4 for an attempt whose triple completed. Any change
/// between before pre and after post, or any pending evidence, discards it.
pub(crate) fn qualify(marks: &QualificationMarks) -> AttemptOutcome {
    use UnqualifiedReason as U;
    let reason = match marks.post {
        PostOutcome::Sql => Some(U::Sql),
        PostOutcome::Timeout => Some(U::Timeout),
        PostOutcome::ScopeMoved => Some(U::VersionChanged),
        PostOutcome::Matched => None,
    }
    .or((marks.generation_before != marks.generation_after).then_some(U::GenerationChanged))
    .or((marks.pending_before || marks.pending_after).then_some(U::PendingEvidence));
    match (reason, marks.clock) {
        (Some(reason), _) => AttemptOutcome::Unqualified(reason),
        (None, ClockOutcome::Invalid) => AttemptOutcome::Unqualified(U::Clock),
        (None, ClockOutcome::Valid(interval)) => AttemptOutcome::Qualified(interval),
    }
}

/// Weights of ACCOUNT, the operated pair's OPEN_ORDERS and OPEN_ORDER_LIST,
/// in triple order.
pub(crate) const INVENTORY_WEIGHTS: [u32; 3] = [20, 6, 6];
pub(crate) const MAX_ATTEMPTS: u8 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct ScanCost {
    pub(crate) sql: u32,
    pub(crate) http: u32,
    pub(crate) weight: u32,
}

/// How far one attempt actually got.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Progress {
    pub(crate) pre: bool,
    /// Inventory requests handed off to the wire, a failing one included.
    pub(crate) responses: u8,
    /// Post is issued only after the whole triple.
    pub(crate) post: bool,
}

impl Progress {
    pub(crate) fn cost(&self) -> ScanCost {
        let responses = usize::from(self.responses.min(3));
        ScanCost {
            sql: u32::from(self.pre) + u32::from(self.post && responses == 3),
            http: responses as u32,
            weight: INVENTORY_WEIGHTS[..responses].iter().sum(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Attempt {
    pub(crate) progress: Progress,
    pub(crate) outcome: AttemptOutcome,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ScanOutcome {
    pub(crate) attempts: u8,
    pub(crate) cost: ScanCost,
    pub(crate) outcome: AttemptOutcome,
}

/// Accumulates attempts: counters never reset between attempts.
#[derive(Debug, Default)]
pub(crate) struct Scan {
    attempts: u8,
    cost: ScanCost,
    last: Option<AttemptOutcome>,
}

impl Scan {
    /// Records one attempt; true when the scan should try again.
    pub(crate) fn record(&mut self, attempt: Attempt) -> bool {
        let cost = attempt.progress.cost();
        self.attempts += 1;
        self.cost.sql += cost.sql;
        self.cost.http += cost.http;
        self.cost.weight += cost.weight;
        self.last = Some(attempt.outcome);
        match attempt.outcome {
            AttemptOutcome::Qualified(_) => false,
            AttemptOutcome::Unqualified(reason) => {
                !reason.permanent() && self.attempts < MAX_ATTEMPTS
            },
        }
    }

    pub(crate) fn finish(self) -> Option<ScanOutcome> {
        Some(ScanOutcome {
            attempts: self.attempts,
            cost: self.cost,
            outcome: self.last?,
        })
    }
}

/// The pure scan over attempts already made, at most [`MAX_ATTEMPTS`].
pub(crate) fn scan(attempts: impl IntoIterator<Item = Attempt>) -> Option<ScanOutcome> {
    let mut scan = Scan::default();
    for attempt in attempts {
        if !scan.record(attempt) {
            break;
        }
    }
    scan.finish()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Finding {
    BaseDeficit {
        holdings: Decimal,
        lower: Decimal,
    },
    BaseExcess {
        holdings: Decimal,
        upper: Decimal,
    },
    /// Locked base above the sum of Robson's live stop remainders.
    LockedForeign {
        locked: Decimal,
        robson: Decimal,
    },
    /// Locked base below that sum; the stops to look up exactly.
    LockedShort {
        locked: Decimal,
        robson: Decimal,
        stops: Vec<String>,
    },
    UnlinkedOperatedOrder {
        order_id: u64,
    },
    OperatedOrderList {
        order_list_id: i64,
    },
    /// A Robson live stop missing from the listing: never terminal by absence.
    NeedsExactEvidence {
        exchange_order_id: String,
    },
    /// Inside the interval only once open claims widen it.
    WithinClaims {
        holdings: Decimal,
        lower: Decimal,
        upper: Decimal,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IncompleteReason {
    MalformedAccount,
    ClosedLegWithoutWriteOff,
    InFlightOrder,
    AttributionContradiction,
    UndefinedPartition,
    Deficit,
    RecoveryState,
    UncertainIntent,
    UncertainCancel,
    Unrepresentable,
    Malformed,
}

/// No sell or close variant exists: a conflict is fenced and paged, never
/// sold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Consequence {
    FenceAndPage,
    ExactLookup,
    StrictRecovery,
    Hold,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BaseVerdict {
    Clean {
        interval: ObservationInterval,
    },
    Discrepancy {
        findings: Vec<Finding>,
        interval: ObservationInterval,
    },
    Incomplete {
        reason: IncompleteReason,
        interval: ObservationInterval,
    },
    Unqualified(UnqualifiedReason),
}

/// Clean or Discrepancy only from a qualified attempt.
pub(crate) fn conclude(scan: &ScanOutcome, classification: Option<Classification>) -> BaseVerdict {
    match (scan.outcome, classification) {
        (AttemptOutcome::Unqualified(reason), _) => BaseVerdict::Unqualified(reason),
        (AttemptOutcome::Qualified(_), None) => {
            BaseVerdict::Unqualified(UnqualifiedReason::Malformed)
        },
        (AttemptOutcome::Qualified(interval), Some(classification)) => match classification {
            Classification::Clean => BaseVerdict::Clean { interval },
            Classification::Discrepancy(findings) => {
                BaseVerdict::Discrepancy { findings, interval }
            },
            Classification::Incomplete(reason) => BaseVerdict::Incomplete { reason, interval },
        },
    }
}

/// The 8k-1 B consequence of one verdict: a qualified Discrepancy with a
/// fence-and-page finding records `venue_discrepancy`, which closes the
/// admission fence until the typed balance release (8k-4); the journal's
/// retry pass persists it. Nothing is sold, cancelled or written here. Every
/// other verdict changes nothing, so a later Clean clears nothing.
pub(crate) fn apply(verdict: &BaseVerdict, gate: &OperationGate) -> Option<Occurrence> {
    let BaseVerdict::Discrepancy { findings, .. } = verdict else {
        return None;
    };
    findings
        .iter()
        .any(|finding| consequence(finding) == Consequence::FenceAndPage)
        .then(|| gate.record(IncidentReason::VenueDiscrepancy))
}

#[cfg(test)]
mod tests;
