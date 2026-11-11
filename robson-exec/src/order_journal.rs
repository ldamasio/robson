//! Durable order journal (ADR-0056 step 7, PR J).
//!
//! One intent per logical order and one attempt per submission, written ahead
//! of the request. The rules, from the design's §2.1:
//!
//! 1. **Monotonic.** An attempt goes `started → rejected` and nothing else, so
//!    replays, reordering and lost acknowledgements converge.
//! 2. **Completeness.** A further attempt on a client id commits together with
//!    every earlier attempt on it, including attempts known only in memory. A
//!    failed or uncertain commit refuses the send.
//! 3. **No resend after a restart or discard.** The client id is unique in the
//!    durable table and a further attempt needs its history in memory, so a
//!    later process, or this one after discarding the history, cannot use a
//!    landed id. An id with no row that is no longer held in memory cannot be
//!    told from a fresh one: there non-reuse rests on callers minting fresh
//!    ids.
//! 4. **`Absent`** needs at least one row, all rejected, and nothing unlanded
//!    in memory; see [`OrderJournal::attempt_evidence`].
//!
//! **Write classes.** An entry is [`WriteClass::Strict`]: no commit, no
//! request. A reduction, a protective stop and a cancel are
//! [`WriteClass::Bounded`]: the commit is tried for at most [`COMMIT_BOUND`]
//! and the request goes out anyway, setting the latch if the row did not land.
//!
//! **Between this PR and the fence.** Outcome classes are recorded and nothing
//! acts on them: an uncertain intent opens no incident and closes no admission.
//! The latch keeps a stop from marking itself clean, and from PR P it closes
//! admissions until a restart. Later PRs of step 7 add the rest.
//!
//! **Operations and permits (PR S).** An operation is one entry of
//! `operations_in_flight`, in a task-local scope. One taken with a permit
//! ([`OrderJournal::acquire`]) belongs to the permit for the whole operation:
//! an entry, its result handling and its stop are one operation, and so is
//! everything a nested call does under it. The send ticket is issued at the
//! send's start under the gate's rules ([`begin_send`]).
//!
//! **Stream suspensions (PR P).** The retry set, `persistence_pending`, also
//! carries the record of an event-log stream that refuses appends
//! ([`OrderJournal::record_suspension`]), which sets the latch. From PR P the
//! latch closes admissions until a restart, at the manager's admission checks
//! and at an entry's send.
//!
//! **Occurrences and release (PR F).** Every place here that learns of
//! something it cannot explain records an occurrence on the gate (see
//! [`crate::gate`]) in the same critical section, with the gate's mutex taken
//! before this journal's memory: an uncertain outcome (stamped with the
//! generation it was recorded at, `uncertain_generation`), a bounded send
//! without its row, discarded history, a conflicting row, a suspended stream.
//! The incident upserts wait in `persistence_pending` beside the rows and are
//! written by the same retry loop. [`OrderJournal::release`] is the audited
//! release: refused while the latch is set, the quoted generation is stale or
//! anything is persisting; then one transaction tombstones the uncertain
//! intents and releases the incidents it covers, and the fence opens only if
//! the generation did not move meanwhile.
//!
//! The sync mutex here is a leaf and is never held across an await. The gate's
//! mutex comes before it in the lock order, and nothing here takes the gate's
//! while holding this one.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    fmt,
    future::Future,
    sync::{
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
        Arc, Mutex, MutexGuard,
    },
    time::Duration,
};

use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, Utc};
use robson_domain::PositionId;
use serde_json::Value;
use tracing::{error, warn};
use uuid::Uuid;

use crate::{
    cancel::{classify_cancel, CancelClass},
    error::ExecError,
    gate::{
        orphan_sweep_cancel, GateState, Incident, IncidentReason, Occurrence, OperationGate,
        OperationKind, OrphanSweepCancel, PermitRefusal, Phase, ALLOCATED_SLOTS,
    },
    ports::StopCancelOutcome,
    quantize::Submission,
    venue::{AttemptEvidence, VenueId},
};

/// How long one journal commit may wait.
pub const COMMIT_BOUND: Duration = Duration::from_millis(500);
/// `persistence_pending`: intents with something that has not landed.
pub const RETRY_SET_CAPACITY: usize = 1024;
/// `operations_in_flight`: one entry per permit, at most [`ALLOCATED_SLOTS`],
/// and the rest for operations without one.
pub const OPERATIONS_CAPACITY: usize = 32;
/// Registry entries for operations without a permit: a test, or a path the
/// choke point counts as unpermitted.
const UNPERMITTED_OPERATIONS: usize = OPERATIONS_CAPACITY - ALLOCATED_SLOTS as usize;
/// Intents written by one pass of the retry loop, so a recovering database is
/// not asked for the whole set at once.
const RETRY_PASS_LIMIT: usize = 64;
/// One pass stops starting commits after this many commit bounds, so it holds
/// the loop for at most 2.5 s at the default bound, however slow each commit.
const RETRY_PASS_COMMIT_BOUNDS: u32 = 4;
/// Client ids sent with no row and no room in the retry set. Each is kept for
/// the life of the process, at about 100 bytes, so at most some 400 KB. Past
/// this the id is not kept and the send still goes out: a full journal must
/// never stop a reduction. Its non-reuse then rests on the id being fresh, as
/// it does for a process that died with no row.
pub const DISCARDED_CAPACITY: usize = 4096;
/// Statements charged to one journal operation before it needs review
/// (guardrails §1.1). A commit is charged what it plans to run, so a commit
/// that fails before sending anything is still charged in full. Reviewed
/// bounds, each counting `BEGIN` and `COMMIT`:
///
/// | Operation | Statements |
/// |---|---|
/// | Entry (pre-send commit, result) | 9 |
/// | Exit, stop or sweep close (one quantisation retry adds 4) | 9, 13 |
/// | The same with a refused retry replaced by a fresh intent | 22 |
/// | Cancel | 9 |
/// | Stop placement with its predecessor's cancel | 31 |
/// | Software exit: cancel, then exit | 31 |
/// | Placement that withdraws both stops | 40 |
///
/// Above the guardrails' default review threshold of 15, on purpose: the
/// writes are few and small, each ordered pair is one order's durability, and
/// the alternative is folding them into one statement per commit, which saves
/// statements but not commits.
pub const OPERATION_STATEMENT_BOUND: u32 = 40;
/// The operated account: one per venue until a venue has several.
pub const DEFAULT_ACCOUNT_SCOPE: &str = "operated";

/// What kind of logical order an intent is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IntentKind {
    Entry,
    Exit,
    Protection,
    Cancel,
    SweepClose,
}

impl IntentKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Entry => "entry",
            Self::Exit => "exit",
            Self::Protection => "protection",
            Self::Cancel => "cancel",
            Self::SweepClose => "sweep_close",
        }
    }
}

/// How a pre-send commit may fail and still let the request go out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteClass {
    Strict,
    Bounded,
}

/// A send the journal will not authorize.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendRefusal {
    /// Proven no-send: no request left and the row did not land.
    NotSent(String),
    /// The commit may have landed. No request left, and none may under this
    /// client id.
    UncertainCommit(String),
}

impl fmt::Display for SendRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotSent(reason) => write!(f, "order journal refused the send: {reason}"),
            Self::UncertainCommit(reason) => {
                write!(f, "order journal commit is uncertain, nothing sent: {reason}")
            },
        }
    }
}

impl From<SendRefusal> for ExecError {
    fn from(refusal: SendRefusal) -> Self {
        ExecError::SendRefused(refusal.to_string())
    }
}

/// Why a commit failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommitFailure {
    /// The COMMIT was never sent, so nothing can have landed.
    Known(String),
    /// The client id already belongs to another intent (rule 3).
    IdSpent,
    /// A COMMIT without an answer: the row may exist.
    Uncertain(String),
}

/// The intent's identity and parameters, written once.
#[derive(Debug, Clone, PartialEq)]
pub struct IntentRow {
    pub intent_id: Uuid,
    pub venue_id: String,
    pub account_scope: String,
    pub client_order_id: String,
    pub position_id: Option<PositionId>,
    pub kind: IntentKind,
    pub cancels_order_id: Option<String>,
    pub params: Value,
    pub created_at: DateTime<Utc>,
}

/// One submission. `rejected_at` only ever goes from `None` to `Some`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttemptRow {
    pub attempt_id: Uuid,
    pub created_at: DateTime<Utc>,
    pub rejected_at: Option<DateTime<Utc>>,
}

/// A definite answer from the venue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VenueOutcome {
    Success,
    Rejected,
    /// A cancel found the order gone, with its final state from the venue.
    GoneTerminal,
}

impl VenueOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Rejected => "rejected",
            Self::GoneTerminal => "gone_terminal",
        }
    }
}

/// What the intent ended as.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct OutcomeRow {
    /// `None` for uncertain and for not sent.
    pub venue_outcome: Option<VenueOutcome>,
    pub cancel_class: Option<&'static str>,
    pub not_sent: bool,
    pub venue_evidence: Option<Value>,
    pub uncertain_at: Option<DateTime<Utc>>,
    /// The generation its `uncertain_outcome` occurrence was recorded at: what
    /// a release quoting that generation or a later one tombstones.
    pub uncertain_generation: Option<i64>,
}

impl OutcomeRow {
    /// An uncertain outcome: the intent may have reached the venue and its
    /// answer is unknown.
    pub fn is_uncertain(&self) -> bool {
        self.uncertain_at.is_some()
    }
}

/// Everything one commit writes about one intent, in one transaction.
#[derive(Debug, Clone, PartialEq)]
pub struct JournalBatch {
    pub intent: IntentRow,
    pub attempts: Vec<AttemptRow>,
    pub outcome: Option<OutcomeRow>,
}

impl JournalBatch {
    /// Statements this batch is charged, for one transaction: `BEGIN`, the
    /// intent, the attempts, the outcome, `COMMIT`. What it plans to run, not a
    /// measure of what reached the database.
    pub fn statements(&self) -> u32 {
        3 + u32::from(!self.attempts.is_empty()) + u32::from(self.outcome.is_some())
    }
}

/// An event-log stream that refuses appends (ADR-0056 step 7, PR P): a write
/// to it failed, so nothing more is appended to it in this process. Written
/// once through the retry set; replays converge on the first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamSuspensionRow {
    pub suspension_id: Uuid,
    pub stream_key: String,
    /// `position` or `system`.
    pub stream_kind: &'static str,
    /// The seq that did not apply. `None` when the append failed, or landed
    /// with its seq unknown.
    pub failed_seq: Option<i64>,
    /// A system stream's month, as its first day.
    pub month: Option<NaiveDate>,
    pub error: String,
    /// The process that suspended it.
    pub epoch_id: Option<Uuid>,
    pub suspended_at: DateTime<Utc>,
}

/// Statements a suspension's write is charged: `BEGIN`, the insert, `COMMIT`.
const SUSPENSION_STATEMENTS: u32 = 3;
/// Statements an incident's upsert is charged: `BEGIN`, the upsert, `COMMIT`.
const INCIDENT_STATEMENTS: u32 = 3;
/// How long a release's transaction may take before it is reported unknown.
pub const RELEASE_BOUND: Duration = Duration::from_secs(10);

/// One audited release, as the backend writes it in one transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseRecord {
    pub decision_id: Uuid,
    /// The generation the operator quoted; everything at or below it is
    /// covered.
    pub generation: i64,
    pub actor: String,
    /// `verified` or `asserted`.
    pub auth_kind: &'static str,
    pub note: String,
    /// The accounting treatment of what remains (`unbooked_accepted`).
    pub treatment: &'static str,
    pub decided_at: DateTime<Utc>,
}

/// What a release's transaction did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ReleaseCounts {
    pub intents_tombstoned: u64,
    pub incidents_released: u64,
}

/// Why a release was refused. Nothing was written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReleaseRefusal {
    /// The latch is set: only a restart clears it.
    Latched,
    /// The quoted generation is not the current one.
    Stale { current: i64 },
    /// Something in `persistence_pending` has not landed.
    Persisting { pending: usize },
    /// The transaction failed, or its answer did not come within
    /// [`RELEASE_BOUND`]: it may have committed, and the fence stays closed.
    Backend(String),
}

impl fmt::Display for ReleaseRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Latched => write!(f, "latched: the journal latch is set until a restart"),
            Self::Stale { current } => write!(f, "stale: the current generation is {current}"),
            Self::Persisting { pending } => {
                write!(f, "persisting: {pending} entries have not landed")
            },
            Self::Backend(error) => write!(f, "the release did not complete: {error}"),
        }
    }
}

/// What an accepted release did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReleaseReceipt {
    pub decision_id: Uuid,
    pub generation: i64,
    pub counts: ReleaseCounts,
    /// Whether the fence opened: nothing moved the generation meanwhile.
    pub fence_open: bool,
}

/// Durable side of the journal.
#[async_trait]
pub trait JournalBackend: Send + Sync + 'static {
    /// Write `batch` in one transaction. Writes are upserts that absorb
    /// repeats, so a batch may be replayed.
    async fn commit(&self, batch: &JournalBatch) -> Result<(), CommitFailure>;

    /// Whether each attempt on record for the client id is rejected.
    async fn attempt_states(
        &self,
        venue_id: &str,
        account_scope: &str,
        client_order_id: &str,
    ) -> Result<Vec<bool>, String>;

    /// Whether the journal can be written at all, for the startup check.
    async fn check_ready(&self) -> Result<(), String> {
        Ok(())
    }

    /// Record a stream suspension in one transaction. An open suspension of
    /// the same stream already on record stands, so a replay changes nothing.
    async fn record_suspension(
        &self,
        suspension: &StreamSuspensionRow,
    ) -> Result<(), CommitFailure>;

    /// Upsert the open incident of `incident.reason` in one transaction. The
    /// values only ever grow, so a replay or a reordering changes nothing;
    /// against a released row it opens a new one.
    async fn upsert_incident(&self, incident: &Incident) -> Result<(), CommitFailure>;

    /// The release's one transaction: tombstone the intents stamped at or
    /// below its generation with no outcome and no tombstone, release the
    /// incidents whose last generation is at or below it, and write the
    /// decision.
    async fn release(&self, release: &ReleaseRecord) -> Result<ReleaseCounts, String>;
}

/// For a daemon with no database: every write succeeds and nothing is kept.
/// Reads fail, so no lookup can conclude `Absent` from it.
pub struct UnpersistedBackend;

#[async_trait]
impl JournalBackend for UnpersistedBackend {
    async fn commit(&self, _batch: &JournalBatch) -> Result<(), CommitFailure> {
        Ok(())
    }

    async fn attempt_states(&self, _: &str, _: &str, _: &str) -> Result<Vec<bool>, String> {
        Err("no durable journal is configured".to_string())
    }

    async fn record_suspension(&self, _: &StreamSuspensionRow) -> Result<(), CommitFailure> {
        Ok(())
    }

    async fn upsert_incident(&self, _: &Incident) -> Result<(), CommitFailure> {
        Ok(())
    }

    /// A release is durable or it is nothing: with no journal it is refused,
    /// and the fence stays closed until a restart.
    async fn release(&self, _: &ReleaseRecord) -> Result<ReleaseCounts, String> {
        Err("no durable journal is configured".to_string())
    }
}

/// How [`StubJournalBackend`] answers the next commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JournalFault {
    /// This commit passes.
    Pass,
    /// Nothing lands; a known failure.
    Known,
    /// The commit lands and its acknowledgement is lost.
    AckLost,
    /// Nothing lands, and the client cannot tell.
    NotLandedUncertain,
    /// Never answers: the caller's bound decides.
    Hang,
    /// The id belongs to another intent.
    IdSpent,
}

/// What the stub holds for a client id.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct StubJournalRow {
    pub intent: Option<IntentRow>,
    pub attempts: Vec<AttemptRow>,
    pub outcome: Option<OutcomeRow>,
    /// The release that tombstoned it.
    pub tombstoned_by: Option<Uuid>,
}

/// An incident row the stub holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StubIncidentRow {
    pub incident: Incident,
    pub released_by: Option<Uuid>,
}

/// In-memory backend with scripted failures, applying the same upsert rules as
/// the PostgreSQL one. Faults are taken in order; with none queued it passes.
#[derive(Default)]
pub struct StubJournalBackend {
    rows: Mutex<HashMap<String, StubJournalRow>>,
    faults: Mutex<VecDeque<JournalFault>>,
    commits: Mutex<Vec<JournalBatch>>,
    read_fails: AtomicBool,
    commit_delay: Mutex<Duration>,
    fail_all: Mutex<Option<JournalFault>>,
    suspensions: Mutex<Vec<StreamSuspensionRow>>,
    suspension_writes_fail: AtomicBool,
    incidents: Mutex<Vec<StubIncidentRow>>,
    incident_writes_fail: AtomicBool,
    incident_write_attempts: AtomicU64,
    incident_write_hold: Mutex<Option<Arc<tokio::sync::Notify>>>,
    commit_hold: Mutex<Option<(usize, Arc<tokio::sync::Notify>)>>,
    releases: Mutex<Vec<ReleaseRecord>>,
    release_fails: AtomicBool,
}

impl StubJournalBackend {
    pub fn push_fault(&self, fault: JournalFault) {
        self.faults.lock().unwrap().push_back(fault);
    }

    /// Script `fault` for commit number `index` (from 0), passing those before.
    pub fn fail_commit(&self, index: usize, fault: Option<JournalFault>) {
        let mut faults = self.faults.lock().unwrap();
        faults.clear();
        if let Some(fault) = fault {
            for _ in 0..index {
                faults.push_back(JournalFault::Pass);
            }
            faults.push_back(fault);
        }
    }

    /// How long every commit takes from now on.
    pub fn set_commit_delay(&self, delay: Duration) {
        *self.commit_delay.lock().unwrap() = delay;
    }

    /// Every commit with no scripted fault of its own answers `fault`.
    pub fn set_fail_all(&self, fault: Option<JournalFault>) {
        *self.fail_all.lock().unwrap() = fault;
    }

    /// Fill both of `journal`'s memories and leave every commit failing.
    pub async fn saturate(&self, journal: &Arc<OrderJournal>) {
        self.set_fail_all(Some(JournalFault::Known));
        let operation = journal.begin_operation();
        operation
            .scope(async {
                for index in 0..RETRY_SET_CAPACITY + DISCARDED_CAPACITY {
                    let intent = journal.open_intent(IntentSpec {
                        position_id: None,
                        kind: IntentKind::Exit,
                        client_order_id: format!("saturating-{index}"),
                        cancels_order_id: None,
                        params: Value::Null,
                    });
                    let sent = journal.before_send(&intent, WriteClass::Bounded).await;
                    assert_eq!(sent, Ok(()), "a bounded first send always goes out");
                }
            })
            .await;
    }

    pub fn set_read_fails(&self, fails: bool) {
        self.read_fails.store(fails, Ordering::SeqCst);
    }

    pub fn row(&self, client_order_id: &str) -> Option<StubJournalRow> {
        self.rows.lock().unwrap().get(client_order_id).cloned()
    }

    pub fn rows(&self) -> Vec<StubJournalRow> {
        self.rows.lock().unwrap().values().cloned().collect()
    }

    /// Every batch handed to `commit`, landed or not, in order.
    pub fn commits(&self) -> Vec<JournalBatch> {
        self.commits.lock().unwrap().clone()
    }

    /// Every suspension write fails, as if that table refused this role,
    /// while intents' commits go on as scripted.
    pub fn set_suspension_writes_fail(&self, fail: bool) {
        self.suspension_writes_fail.store(fail, Ordering::SeqCst);
    }

    /// The stream suspensions on record, in the order they landed.
    pub fn suspensions(&self) -> Vec<StreamSuspensionRow> {
        self.suspensions.lock().unwrap().clone()
    }

    /// Hold the next incident upsert, before it is applied, until the returned
    /// handle is notified.
    pub fn hold_next_incident_write(&self) -> Arc<tokio::sync::Notify> {
        let hold = Arc::new(tokio::sync::Notify::new());
        *self.incident_write_hold.lock().unwrap() = Some(Arc::clone(&hold));
        hold
    }

    /// Incident upserts begun so far, held or not.
    pub fn incident_write_attempts(&self) -> u64 {
        self.incident_write_attempts.load(Ordering::SeqCst)
    }

    /// Hold commit number `index` (from 0), before it is applied, until the
    /// returned handle is notified.
    pub fn hold_commit(&self, index: usize) -> Arc<tokio::sync::Notify> {
        let hold = Arc::new(tokio::sync::Notify::new());
        *self.commit_hold.lock().unwrap() = Some((index, Arc::clone(&hold)));
        hold
    }

    /// Every incident upsert fails, while the other writes go on as scripted.
    pub fn set_incident_writes_fail(&self, fail: bool) {
        self.incident_writes_fail.store(fail, Ordering::SeqCst);
    }

    /// Every release's transaction fails.
    pub fn set_release_fails(&self, fail: bool) {
        self.release_fails.store(fail, Ordering::SeqCst);
    }

    /// The incident rows on record, open and released, oldest first.
    pub fn incidents(&self) -> Vec<StubIncidentRow> {
        self.incidents.lock().unwrap().clone()
    }

    /// The releases on record.
    pub fn releases(&self) -> Vec<ReleaseRecord> {
        self.releases.lock().unwrap().clone()
    }

    /// The next scripted fault, or the standing one.
    fn next_fault(&self) -> Option<JournalFault> {
        let scripted = self.faults.lock().unwrap().pop_front();
        scripted.or(*self.fail_all.lock().unwrap())
    }

    fn land_suspension(&self, suspension: &StreamSuspensionRow) {
        let mut landed = self.suspensions.lock().unwrap();
        if !landed.iter().any(|row| row.stream_key == suspension.stream_key) {
            landed.push(suspension.clone());
        }
    }

    fn land(&self, batch: &JournalBatch) -> Result<(), CommitFailure> {
        let mut rows = self.rows.lock().unwrap();
        let row = rows.entry(batch.intent.client_order_id.clone()).or_default();
        if row.intent.as_ref().is_some_and(|i| i.intent_id != batch.intent.intent_id) {
            return Err(CommitFailure::IdSpent);
        }
        row.intent = Some(batch.intent.clone());
        for attempt in &batch.attempts {
            match row.attempts.iter_mut().find(|a| a.attempt_id == attempt.attempt_id) {
                Some(stored) => {
                    stored.rejected_at = stored.rejected_at.or(attempt.rejected_at);
                },
                None => row.attempts.push(attempt.clone()),
            }
        }
        if let Some(outcome) = &batch.outcome {
            let stored = row.outcome.get_or_insert_with(OutcomeRow::default);
            stored.venue_outcome = stored.venue_outcome.or(outcome.venue_outcome);
            stored.cancel_class = stored.cancel_class.or(outcome.cancel_class);
            stored.not_sent |= outcome.not_sent;
            stored.venue_evidence =
                stored.venue_evidence.take().or_else(|| outcome.venue_evidence.clone());
            stored.uncertain_at = stored.uncertain_at.or(outcome.uncertain_at);
            stored.uncertain_generation =
                stored.uncertain_generation.or(outcome.uncertain_generation);
        }
        Ok(())
    }
}

#[async_trait]
impl JournalBackend for StubJournalBackend {
    async fn commit(&self, batch: &JournalBatch) -> Result<(), CommitFailure> {
        let index = {
            let mut commits = self.commits.lock().unwrap();
            commits.push(batch.clone());
            commits.len() - 1
        };
        let hold = {
            let mut armed = self.commit_hold.lock().unwrap();
            match armed.take() {
                Some((at, hold)) if at == index => Some(hold),
                other => {
                    *armed = other;
                    None
                },
            }
        };
        if let Some(hold) = hold {
            hold.notified().await;
        }
        let delay = *self.commit_delay.lock().unwrap();
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
        match self.next_fault() {
            None | Some(JournalFault::Pass) => self.land(batch),
            Some(JournalFault::Known) => Err(CommitFailure::Known("injected".to_string())),
            Some(JournalFault::AckLost) => {
                self.land(batch)?;
                Err(CommitFailure::Uncertain("injected: ack lost".to_string()))
            },
            Some(JournalFault::NotLandedUncertain) => {
                Err(CommitFailure::Uncertain("injected: not landed".to_string()))
            },
            Some(JournalFault::Hang) => std::future::pending().await,
            Some(JournalFault::IdSpent) => Err(CommitFailure::IdSpent),
        }
    }

    async fn attempt_states(&self, _: &str, _: &str, id: &str) -> Result<Vec<bool>, String> {
        if self.read_fails.load(Ordering::SeqCst) {
            return Err("injected read failure".to_string());
        }
        let rows = self.rows.lock().unwrap();
        Ok(rows
            .get(id)
            .map(|row| row.attempts.iter().map(|a| a.rejected_at.is_some()).collect())
            .unwrap_or_default())
    }

    async fn record_suspension(
        &self,
        suspension: &StreamSuspensionRow,
    ) -> Result<(), CommitFailure> {
        if self.suspension_writes_fail.load(Ordering::SeqCst) {
            return Err(CommitFailure::Known("injected: suspensions refused".to_string()));
        }
        match self.next_fault() {
            None | Some(JournalFault::Pass) => {
                self.land_suspension(suspension);
                Ok(())
            },
            Some(JournalFault::Known | JournalFault::IdSpent) => {
                Err(CommitFailure::Known("injected".to_string()))
            },
            Some(JournalFault::AckLost) => {
                self.land_suspension(suspension);
                Err(CommitFailure::Uncertain("injected: ack lost".to_string()))
            },
            Some(JournalFault::NotLandedUncertain) => {
                Err(CommitFailure::Uncertain("injected: not landed".to_string()))
            },
            Some(JournalFault::Hang) => std::future::pending().await,
        }
    }

    async fn upsert_incident(&self, incident: &Incident) -> Result<(), CommitFailure> {
        self.incident_write_attempts.fetch_add(1, Ordering::SeqCst);
        let hold = self.incident_write_hold.lock().unwrap().take();
        if let Some(hold) = hold {
            hold.notified().await;
        }
        if self.incident_writes_fail.load(Ordering::SeqCst) {
            return Err(CommitFailure::Known("injected: incidents refused".to_string()));
        }
        let mut rows = self.incidents.lock().unwrap();
        match rows
            .iter_mut()
            .find(|row| row.released_by.is_none() && row.incident.reason == incident.reason)
        {
            Some(row) => {
                let open = &mut row.incident;
                open.first_at = open.first_at.min(incident.first_at);
                open.last_at = open.last_at.max(incident.last_at);
                open.count = open.count.max(incident.count);
                open.first_generation = open.first_generation.min(incident.first_generation);
                open.last_generation = open.last_generation.max(incident.last_generation);
            },
            None => rows.push(StubIncidentRow {
                incident: incident.clone(),
                released_by: None,
            }),
        }
        Ok(())
    }

    async fn release(&self, release: &ReleaseRecord) -> Result<ReleaseCounts, String> {
        let delay = *self.commit_delay.lock().unwrap();
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
        if self.release_fails.load(Ordering::SeqCst) {
            return Err("injected: release refused".to_string());
        }
        let mut counts = ReleaseCounts::default();
        for row in self.rows.lock().unwrap().values_mut() {
            let covered = row.outcome.as_ref().is_some_and(|outcome| {
                outcome.venue_outcome.is_none()
                    && !outcome.not_sent
                    && outcome.uncertain_at.is_some()
                    && outcome.uncertain_generation.is_some_and(|g| g <= release.generation)
            });
            if covered && row.tombstoned_by.is_none() {
                row.tombstoned_by = Some(release.decision_id);
                counts.intents_tombstoned += 1;
            }
        }
        for row in self.incidents.lock().unwrap().iter_mut() {
            if row.released_by.is_none() && row.incident.last_generation <= release.generation {
                row.released_by = Some(release.decision_id);
                counts.incidents_released += 1;
            }
        }
        self.releases.lock().unwrap().push(release.clone());
        Ok(counts)
    }
}

/// An order about to be journaled.
#[derive(Debug, Clone)]
pub struct IntentSpec {
    pub position_id: Option<PositionId>,
    pub kind: IntentKind,
    pub client_order_id: String,
    pub cancels_order_id: Option<String>,
    pub params: Value,
}

/// What became of an orphan sweep's cancel.
#[derive(Debug)]
pub enum OrphanCancel {
    /// The veto held: nothing was sent.
    Withheld,
    /// It went to the venue, and this is its class.
    Sent(CancelClass),
}

/// One `ins-` order the orphan sweep withheld (design §2.9).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WithheldOrder {
    pub symbol: String,
    pub exchange_order_id: String,
    pub client_order_id: String,
}

/// The last scan's withheld orders: at most [`WITHHELD_REPORT_CAPACITY`]
/// listed, `total` counting every one. A report only: the veto applied to
/// every order, listed or not.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WithheldReport {
    pub orders: Vec<WithheldOrder>,
    pub total: u64,
    pub truncated: bool,
    pub at: Option<DateTime<Utc>>,
}

/// Withheld orders one report lists.
pub const WITHHELD_REPORT_CAPACITY: usize = 500;

impl WithheldReport {
    /// Count `order`, and list it while there is room. Whether it was listed.
    pub fn withhold(&mut self, order: WithheldOrder) -> bool {
        self.total += 1;
        if self.orders.len() < WITHHELD_REPORT_CAPACITY {
            self.orders.push(order);
            return true;
        }
        self.truncated = true;
        false
    }
}

/// Where a bounded send with no row was remembered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UnlandedKept {
    RetrySet,
    /// Only its id, in the discarded set.
    Discarded,
    /// Nowhere: its non-reuse rests on the id being fresh.
    Lost,
}

/// Handle on an intent registered with [`OrderJournal::open_intent`].
#[derive(Debug, Clone)]
pub struct JournaledIntent {
    intent_id: Uuid,
    client_order_id: String,
    operation: u64,
}

impl JournaledIntent {
    pub fn intent_id(&self) -> Uuid {
        self.intent_id
    }

    pub fn client_order_id(&self) -> &str {
        &self.client_order_id
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Landed {
    Nothing,
    Started,
    Rejected,
}

#[derive(Debug)]
struct AttemptMemory {
    row: AttemptRow,
    landed: Landed,
}

impl AttemptMemory {
    fn current(&self) -> Landed {
        if self.row.rejected_at.is_some() {
            Landed::Rejected
        } else {
            Landed::Started
        }
    }

    fn is_unlanded(&self) -> bool {
        self.landed != self.current()
    }
}

#[derive(Debug)]
struct IntentMemory {
    row: IntentRow,
    operation: u64,
    attempts: Vec<AttemptMemory>,
    outcome: Option<OutcomeRow>,
    outcome_landed: bool,
    /// A commit on this id failed or is uncertain: nothing more goes out.
    sealed: bool,
    /// The pre-send commit itself is uncertain: not a proven no-send.
    commit_uncertain: bool,
    ticket_issued: bool,
    /// Sends reported under this id, one per request that left.
    tickets: u32,
    /// The journal refused a further attempt on this id before it left.
    retry_refused: bool,
    /// The one fresh intent that took over after that refusal.
    replaced_by: Option<JournaledIntent>,
    in_retry_set: bool,
}

impl IntentMemory {
    fn has_unlanded(&self) -> bool {
        self.attempts.iter().any(AttemptMemory::is_unlanded)
            || (self.outcome.is_some() && !self.outcome_landed)
    }

    fn batch(&self) -> JournalBatch {
        JournalBatch {
            intent: self.row.clone(),
            attempts: self.attempts.iter().map(|attempt| attempt.row.clone()).collect(),
            outcome: self.outcome.clone(),
        }
    }

    fn mark_landed(&mut self, batch: &JournalBatch) {
        for row in &batch.attempts {
            if let Some(attempt) =
                self.attempts.iter_mut().find(|a| a.row.attempt_id == row.attempt_id)
            {
                let landed = if row.rejected_at.is_some() {
                    Landed::Rejected
                } else {
                    Landed::Started
                };
                // Monotonic here too: an older snapshot never lowers it.
                if attempt.landed != Landed::Rejected {
                    attempt.landed = landed;
                }
            }
        }
        if batch.outcome.is_some() && batch.outcome == self.outcome {
            self.outcome_landed = true;
        }
    }
}

#[derive(Debug, Clone)]
struct OperationIntent {
    client_order_id: String,
    kind: IntentKind,
    ticket_issued: bool,
    /// The venue order a cancel targets. Kept here, for the operation's whole
    /// life, because the intent's own memory can be collected before it ends.
    cancels_order_id: Option<String>,
}

/// One entry of `operations_in_flight`.
#[derive(Debug, Default)]
struct OperationEntry {
    /// `None` for an operation without a permit.
    permit: Option<OperationKind>,
    intents: Vec<OperationIntent>,
}

/// One operation in flight, for the pending list: never released.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationInFlight {
    pub permit: Option<OperationKind>,
    /// Its intents: client id, kind, whether a send started.
    pub intents: Vec<(String, IntentKind, bool)>,
}

/// One entry of `persistence_pending`, for the pending list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PendingEntry {
    /// An intent's rows or outcome, by client id.
    Intent(String),
    /// A stream suspension, by stream key.
    Suspension(String),
    /// An incident's upsert.
    Incident(IncidentReason),
}

#[derive(Debug, Default)]
struct JournalState {
    /// Intents in flight or with something unlanded. Removed once finished
    /// and landed, so at most the retry set plus the orders running now. A
    /// refused retry's original stays while it names its replacement, until
    /// its completion or its operation's end consumes the link
    /// ([`OrderJournal::unlink`]): an order running now too.
    intents: HashMap<String, IntentMemory>,
    /// `persistence_pending`, in client ids.
    retry_set: VecDeque<String>,
    /// `persistence_pending` too: stream suspensions not yet on record. The
    /// two share [`RETRY_SET_CAPACITY`].
    suspensions: VecDeque<StreamSuspensionRow>,
    /// `operations_in_flight`. A list is as long as its operation's orders.
    operations: HashMap<u64, OperationEntry>,
    /// Entries of `operations` held by operations without a permit.
    unpermitted_listed: usize,
    /// Ids sent with no row and no room in the retry set: the only record
    /// that they were used (rule 3). Any other id that leaves memory either
    /// has a durable row, which the unique index holds for good, or was never
    /// sent.
    discarded: HashSet<String>,
}

impl JournalState {
    /// Entries in `persistence_pending`.
    fn pending(&self) -> usize {
        self.retry_set.len() + self.suspensions.len()
    }
}

/// Rules, memory and registries of the order journal.
pub struct OrderJournal {
    backend: Arc<dyn JournalBackend>,
    venue_id: String,
    account_scope: String,
    commit_bound: Duration,
    state: Mutex<JournalState>,
    /// Shared with the gate, which sets it too.
    latch: Arc<AtomicBool>,
    gate: Arc<OperationGate>,
    next_operation: AtomicU64,
    statements_total: AtomicU64,
    operation_statements_max: AtomicU32,
    operations_over_statement_bound: AtomicU64,
    non_reuse_ids_lost: AtomicU64,
    /// Held for one whole retry pass: passes never overlap (an async lock,
    /// taken with no other lock held).
    flush_pass: tokio::sync::Mutex<()>,
    release_bound: Duration,
    /// The orphan sweep's last report, a sync leaf held with no other lock.
    withheld: Mutex<WithheldReport>,
}

impl OrderJournal {
    pub fn new(backend: Arc<dyn JournalBackend>, venue: &VenueId) -> Self {
        let latch = Arc::new(AtomicBool::new(false));
        Self {
            backend,
            venue_id: venue.as_str().to_string(),
            account_scope: DEFAULT_ACCOUNT_SCOPE.to_string(),
            commit_bound: COMMIT_BOUND,
            state: Mutex::default(),
            gate: Arc::new(OperationGate::new(Arc::clone(&latch))),
            latch,
            next_operation: AtomicU64::new(1),
            statements_total: AtomicU64::new(0),
            operation_statements_max: AtomicU32::new(0),
            operations_over_statement_bound: AtomicU64::new(0),
            non_reuse_ids_lost: AtomicU64::new(0),
            flush_pass: tokio::sync::Mutex::new(()),
            release_bound: RELEASE_BOUND,
            withheld: Mutex::default(),
        }
    }

    /// A journal that keeps nothing, for a daemon or a test with no database.
    pub fn unpersisted() -> Self {
        Self::new(Arc::new(UnpersistedBackend), &VenueId::new("unpersisted"))
    }

    /// Shorten the commit bound, for tests that wait it out.
    pub fn with_commit_bound(mut self, bound: Duration) -> Self {
        self.commit_bound = bound;
        self
    }

    /// Shorten the release's bound, for tests that wait it out.
    pub fn with_release_bound(mut self, bound: Duration) -> Self {
        self.release_bound = bound;
        self
    }

    fn lock(&self) -> MutexGuard<'_, JournalState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Whether a bounded send went out without its row, history was discarded,
    /// a venue mutation ran without a permit or an operation was aborted.
    /// Stays set for the life of the process.
    pub fn latch_set(&self) -> bool {
        self.latch.load(Ordering::SeqCst)
    }

    /// The permit gate, shared with the venue's transport.
    pub fn gate(&self) -> &Arc<OperationGate> {
        &self.gate
    }

    /// The gate's mutex, then this journal's memory: the lock order of every
    /// critical section that can record an occurrence.
    fn lock_both(&self) -> (MutexGuard<'_, GateState>, MutexGuard<'_, JournalState>) {
        let gate = self.gate.lock();
        (gate, self.lock())
    }

    /// Nothing here is in flight or waiting to land: no intent in memory and
    /// nothing in `persistence_pending`, incident upserts included. Part of a
    /// clean stop's check.
    pub fn is_quiescent(&self) -> bool {
        let (gate, state) = self.lock_both();
        state.intents.is_empty() && state.pending() == 0 && gate.incidents_persisting() == 0
    }

    /// Entries waiting in `persistence_pending`: rows, suspensions and
    /// incident upserts.
    pub fn retry_set_depth(&self) -> usize {
        let (gate, state) = self.lock_both();
        state.pending() + gate.incidents_persisting()
    }

    /// What `persistence_pending` holds, for the pending list.
    pub fn pending_entries(&self) -> Vec<PendingEntry> {
        let (gate, state) = self.lock_both();
        let incidents = gate.incidents_to_write().into_iter();
        let mut entries: Vec<PendingEntry> =
            incidents.map(|(incident, _)| PendingEntry::Incident(incident.reason)).collect();
        entries.extend(
            state.suspensions.iter().map(|s| PendingEntry::Suspension(s.stream_key.clone())),
        );
        entries.extend(state.retry_set.iter().cloned().map(PendingEntry::Intent));
        entries
    }

    /// The operations in flight, for the pending list.
    pub fn operations_in_flight_view(&self) -> Vec<OperationInFlight> {
        let state = self.lock();
        let mut ids: Vec<&u64> = state.operations.keys().collect();
        ids.sort();
        ids.into_iter()
            .map(|id| {
                let entry = &state.operations[id];
                OperationInFlight {
                    permit: entry.permit,
                    intents: entry
                        .intents
                        .iter()
                        .map(|i| (i.client_order_id.clone(), i.kind, i.ticket_issued))
                        .collect(),
                }
            })
            .collect()
    }

    /// Record that an event-log stream refuses appends (ADR-0056 step 7, PR
    /// P), through the retry set, with its `stream_suspended` occurrence, which
    /// sets the latch and closes the fence. No await: the retry loop writes it.
    /// With the retry set full it is not kept: discarded history, one more
    /// occurrence.
    pub fn record_suspension(&self, suspension: StreamSuspensionRow) {
        let (mut gate, mut state) = self.lock_both();
        let mut occurrences = vec![gate.occur(IncidentReason::StreamSuspended)];
        let kept = if state.suspensions.iter().any(|row| row.stream_key == suspension.stream_key) {
            true
        } else if state.pending() >= RETRY_SET_CAPACITY {
            occurrences.push(gate.occur(IncidentReason::PersistenceFailure));
            false
        } else {
            state.suspensions.push_back(suspension.clone());
            true
        };
        drop((gate, state));
        if !kept {
            error!(
                stream_key = %suspension.stream_key,
                "Journal retry set is full; this stream suspension is not recorded durably"
            );
        }
        occurrences.iter().for_each(Occurrence::log);
    }

    /// Replace the orphan sweep's report with the last scan's.
    pub fn replace_withheld_report(&self, report: WithheldReport) {
        *self.withheld.lock().unwrap_or_else(|e| e.into_inner()) = report;
    }

    /// The orphan sweep's last report.
    pub fn withheld_report(&self) -> WithheldReport {
        self.withheld.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Bounded sends whose id could be kept neither in the retry set nor as a
    /// discarded id, so nothing here stops that id being used again. Only ever
    /// grows.
    pub fn non_reuse_ids_lost(&self) -> u64 {
        self.non_reuse_ids_lost.load(Ordering::Relaxed)
    }

    /// Statements charged so far, by every commit of this journal.
    pub fn statements_total(&self) -> u64 {
        self.statements_total.load(Ordering::Relaxed)
    }

    /// The most statements charged to any finished operation.
    pub fn operation_statements_max(&self) -> u32 {
        self.operation_statements_max.load(Ordering::Relaxed)
    }

    /// Finished operations that went past [`OPERATION_STATEMENT_BOUND`].
    pub fn operations_over_statement_bound(&self) -> u64 {
        self.operations_over_statement_bound.load(Ordering::Relaxed)
    }

    /// Whether the database can take journal writes (startup check).
    pub async fn check_ready(&self) -> Result<(), String> {
        self.backend.check_ready().await
    }

    /// Entries in `operations_in_flight`.
    pub fn operations_in_flight(&self) -> usize {
        self.lock().operations.len()
    }

    /// Whether the intent is in flight or has something unlanded here.
    pub fn is_live(&self, client_order_id: &str) -> bool {
        self.lock().intents.contains_key(client_order_id)
    }

    /// Whether a live operation owns the venue order `exchange_order_id`, whose
    /// client id is `client_order_id` (ADR-0056 step 7, PR O): one of its
    /// intents is that order, or cancels it. Read from `operations_in_flight`,
    /// which lists an intent from [`Self::open_intent`], before its request,
    /// until its operation ends, whatever became of the intent's own memory: a
    /// stop stays listed until after its leg recorded it, and an order a live
    /// operation is cancelling stays its own. Memory only, so it answers when
    /// persistence failed. One sync section, with no other lock taken.
    pub fn owned_by_live_operation(&self, client_order_id: &str, exchange_order_id: &str) -> bool {
        self.lock().operations.values().any(|operation| {
            operation.intents.iter().any(|intent| {
                intent.client_order_id == client_order_id
                    || intent.cancels_order_id.as_deref() == Some(exchange_order_id)
            })
        })
    }

    /// Start an operation without a permit: its intents are listed in
    /// `operations_in_flight` until the returned guard drops. Past the entries
    /// the permits leave over it runs unlisted and an entry cannot be
    /// authorized under it; a reduction still goes out. Its venue mutations
    /// are unpermitted at the choke point.
    pub fn begin_operation(self: &Arc<Self>) -> Operation {
        let id = self.next_operation.fetch_add(1, Ordering::Relaxed);
        let mut state = self.lock();
        let listed = state.unpermitted_listed < UNPERMITTED_OPERATIONS;
        if listed {
            state.unpermitted_listed += 1;
            state.operations.insert(id, OperationEntry::default());
        } else {
            warn!(capacity = UNPERMITTED_OPERATIONS, "Journal operation registry is full");
        }
        Operation {
            journal: Arc::clone(self),
            id,
            statements: Arc::new(AtomicU32::new(0)),
            permit: None,
            listed_unpermitted: listed,
        }
    }

    /// Take a permit for an operation of `kind`, with its slot and its entry
    /// in `operations_in_flight`, which a permit always has. Nothing is done
    /// when it is refused.
    ///
    /// # Errors
    /// The class is full, or the daemon is stopping and refuses it.
    pub fn acquire(self: &Arc<Self>, kind: OperationKind) -> Result<Permit, PermitRefusal> {
        self.gate.acquire(kind.class())?;
        let id = self.next_operation.fetch_add(1, Ordering::Relaxed);
        self.lock()
            .operations
            .insert(id, OperationEntry { permit: Some(kind), intents: Vec::new() });
        Ok(Permit {
            operation: Some(Operation {
                journal: Arc::clone(self),
                id,
                statements: Arc::new(AtomicU32::new(0)),
                permit: Some(kind),
                listed_unpermitted: false,
            }),
            kind,
            completed: false,
        })
    }

    /// Run `work` under the permit this task holds, or under a new one of
    /// `kind` when it holds none. A nested operation never takes a second slot
    /// or changes class.
    ///
    /// # Errors
    /// A new permit was refused; nothing ran.
    pub async fn with_permit<F: Future>(
        self: &Arc<Self>,
        kind: OperationKind,
        work: F,
    ) -> Result<F::Output, PermitRefusal> {
        if in_operation() {
            return Ok(work.await);
        }
        Ok(self.acquire(kind)?.run(work).await)
    }

    /// Run `work` in this task's operation, or in a new one without a permit
    /// when the task is in none.
    pub async fn within_operation<F: Future>(self: &Arc<Self>, work: F) -> F::Output {
        if in_operation() {
            return work.await;
        }
        self.begin_operation().scope(work).await
    }

    /// Run an admission in this task's operation, or in one of its own when
    /// the task is in none, and say whether the entry `client_order_id`
    /// reached its send. Read from the operation's registry, on `Ok` and `Err`
    /// alike, before an operation of its own ends.
    pub async fn run_admission<F: Future>(
        self: &Arc<Self>,
        client_order_id: Option<&str>,
        work: F,
    ) -> (F::Output, bool) {
        if let Ok(id) = CURRENT_OPERATION.try_with(|current| current.id) {
            let output = work.await;
            return (output, self.entry_ticket_in(id, client_order_id));
        }
        let operation = self.begin_operation();
        let output = operation.scope(work).await;
        let ticket = self.entry_ticket_in(operation.id, client_order_id);
        (output, ticket)
    }

    /// Whether operation `id` lists an entry, `client_order_id` when given,
    /// whose send started.
    fn entry_ticket_in(&self, id: u64, client_order_id: Option<&str>) -> bool {
        self.lock().operations.get(&id).is_some_and(|listed| {
            listed.intents.iter().any(|intent| {
                intent.kind == IntentKind::Entry
                    && intent.ticket_issued
                    && client_order_id.is_none_or(|wanted| intent.client_order_id == wanted)
            })
        })
    }

    /// Register an intent in memory. Nothing is written until
    /// [`Self::before_send`].
    pub fn open_intent(&self, spec: IntentSpec) -> JournaledIntent {
        let operation = CURRENT_OPERATION.try_with(|current| current.id).unwrap_or(0);
        let intent_id = Uuid::now_v7();
        let handle = JournaledIntent {
            intent_id,
            client_order_id: spec.client_order_id.clone(),
            operation,
        };
        let mut state = self.lock();
        if state.intents.contains_key(&spec.client_order_id) {
            // The id is in use here: `before_send` refuses this handle.
            return handle;
        }
        if let Some(listed) = state.operations.get_mut(&operation) {
            listed.intents.push(OperationIntent {
                client_order_id: spec.client_order_id.clone(),
                kind: spec.kind,
                ticket_issued: false,
                cancels_order_id: spec.cancels_order_id.clone(),
            });
        }
        let row = IntentRow {
            intent_id,
            venue_id: self.venue_id.clone(),
            account_scope: self.account_scope.clone(),
            client_order_id: spec.client_order_id.clone(),
            position_id: spec.position_id,
            kind: spec.kind,
            cancels_order_id: spec.cancels_order_id,
            params: spec.params,
            created_at: Utc::now(),
        };
        state.intents.insert(spec.client_order_id, IntentMemory {
            row,
            operation,
            attempts: Vec::new(),
            outcome: None,
            outcome_landed: false,
            sealed: false,
            commit_uncertain: false,
            ticket_issued: false,
            tickets: 0,
            retry_refused: false,
            replaced_by: None,
            in_retry_set: false,
        });
        handle
    }

    async fn bounded_commit(&self, batch: &JournalBatch) -> Result<(), CommitFailure> {
        let statements = batch.statements();
        self.statements_total.fetch_add(u64::from(statements), Ordering::Relaxed);
        let _ = CURRENT_OPERATION.try_with(|current| {
            current.statements.fetch_add(statements, Ordering::Relaxed);
        });
        match tokio::time::timeout(self.commit_bound, self.backend.commit(batch)).await {
            Ok(result) => result,
            Err(_) => Err(CommitFailure::Uncertain(format!(
                "no answer within {} ms",
                self.commit_bound.as_millis()
            ))),
        }
    }

    /// Commit one more attempt ahead of its request. `Ok` authorizes exactly
    /// one send under this client id.
    pub async fn before_send(
        &self,
        intent: &JournaledIntent,
        class: WriteClass,
    ) -> Result<(), SendRefusal> {
        let (batch, attempt_id, first) = {
            let mut state = self.lock();
            if class == WriteClass::Strict && !state.operations.contains_key(&intent.operation) {
                return Err(SendRefusal::NotSent("operation is not registered".to_string()));
            }
            if state.discarded.contains(&intent.client_order_id) {
                // Refused for good, but on the handle that sent it this is a
                // refused retry like any other, eligible for a fresh id.
                if let Some(memory) = state.intents.get_mut(&intent.client_order_id) {
                    if memory.row.intent_id == intent.intent_id {
                        memory.retry_refused = !memory.attempts.is_empty();
                    }
                }
                return Err(SendRefusal::NotSent("the id was discarded".to_string()));
            }
            let Some(memory) = state.intents.get_mut(&intent.client_order_id) else {
                return Err(SendRefusal::NotSent("the id's history is gone".to_string()));
            };
            if memory.row.intent_id != intent.intent_id {
                return Err(SendRefusal::NotSent("the id is in use".to_string()));
            }
            if memory.sealed {
                memory.retry_refused = !memory.attempts.is_empty();
                return Err(SendRefusal::NotSent(
                    "an earlier commit on this id failed".to_string(),
                ));
            }
            let first = memory.attempts.is_empty();
            let attempt_id = Uuid::now_v7();
            memory.attempts.push(AttemptMemory {
                row: AttemptRow {
                    attempt_id,
                    created_at: Utc::now(),
                    rejected_at: None,
                },
                landed: Landed::Nothing,
            });
            (memory.batch(), attempt_id, first)
        };

        let result = self.bounded_commit(&batch).await;
        let (mut gate, mut state) = self.lock_both();
        if !state.intents.contains_key(&intent.client_order_id) {
            return Err(SendRefusal::NotSent("the id's history is gone".to_string()));
        }
        let failure = match result {
            Ok(()) => {
                if let Some(memory) = state.intents.get_mut(&intent.client_order_id) {
                    memory.mark_landed(&batch);
                }
                return Ok(());
            },
            Err(failure) => failure,
        };
        // The first attempt of a bounded write goes out with or without its
        // row, whatever the journal can still remember. Only an id already
        // spent stays home.
        if class == WriteClass::Bounded && first && failure != CommitFailure::IdSpent {
            // The occurrence before the request: the fence is closed by the
            // time it leaves.
            let mut occurrences = vec![gate.occur(IncidentReason::UnlandedSend)];
            let (kept, overflow) =
                self.remember_unlanded_send(&mut gate, &mut state, &intent.client_order_id);
            occurrences.extend(overflow);
            drop((gate, state));
            let client_order_id = &intent.client_order_id;
            match kept {
                UnlandedKept::RetrySet => {},
                UnlandedKept::Discarded => {
                    error!(%client_order_id, "Journal retry set is full; keeping only this send's id");
                },
                UnlandedKept::Lost => error!(
                    %client_order_id,
                    "Journal cannot keep this send's id; non-reuse rests on freshness"
                ),
            }
            warn!(%client_order_id, ?failure, "Bounded send goes out without its journal row");
            occurrences.iter().for_each(Occurrence::log);
            return Ok(());
        }
        drop(gate);
        let Some(memory) = state.intents.get_mut(&intent.client_order_id) else {
            return Err(SendRefusal::NotSent("the id's history is gone".to_string()));
        };
        memory.sealed = true;
        memory.retry_refused = !first;
        match failure {
            CommitFailure::IdSpent => {
                memory.attempts.retain(|attempt| attempt.row.attempt_id != attempt_id);
                Err(SendRefusal::NotSent("the client id is already spent".to_string()))
            },
            CommitFailure::Known(reason) => {
                memory.attempts.retain(|attempt| attempt.row.attempt_id != attempt_id);
                Err(SendRefusal::NotSent(reason))
            },
            CommitFailure::Uncertain(reason) => {
                memory.commit_uncertain = true;
                Err(SendRefusal::UncertainCommit(reason))
            },
        }
    }

    /// Remember a send with no row: in the retry set, or failing that as a
    /// discarded id, with the overflow's `persistence_failure` occurrence.
    /// With both full the id is not kept and the send still goes out; the
    /// loss is counted. The caller logs.
    fn remember_unlanded_send(
        &self,
        gate: &mut GateState,
        state: &mut JournalState,
        client_order_id: &str,
    ) -> (UnlandedKept, Option<Occurrence>) {
        let overflow = match Self::try_enqueue(gate, state, client_order_id) {
            Ok(()) => return (UnlandedKept::RetrySet, None),
            Err(overflow) => overflow,
        };
        if state.discarded.len() < DISCARDED_CAPACITY {
            state.discarded.insert(client_order_id.to_string());
            return (UnlandedKept::Discarded, overflow);
        }
        self.non_reuse_ids_lost.fetch_add(1, Ordering::Relaxed);
        (UnlandedKept::Lost, overflow)
    }

    /// Authorize one submission of a bounded reduction and return the client id
    /// to send it under.
    pub async fn authorize_reduction(
        &self,
        intent: &JournaledIntent,
        submission: Submission,
        fresh_client_order_id: impl FnOnce() -> String,
    ) -> Result<String, ExecError> {
        match submission {
            Submission::First | Submission::SameIdRetry => {
                self.before_send(intent, WriteClass::Bounded).await?;
                Ok(intent.client_order_id.clone())
            },
            Submission::FreshIdAfterRefusedRetry { rules_rejection } => {
                let fresh = self
                    .replace_refused_retry(intent, fresh_client_order_id(), rules_rejection)
                    .await?;
                Ok(fresh.client_order_id)
            },
        }
    }

    /// Open the one fresh intent that replaces a refused resubmission. Only for
    /// an intent whose retry the journal refused with exactly one request gone
    /// out, the first, and only once; the refused id stays closed. It is a
    /// bounded first attempt like any other.
    async fn replace_refused_retry(
        &self,
        intent: &JournaledIntent,
        fresh_client_order_id: String,
        rules_rejection: String,
    ) -> Result<JournaledIntent, SendRefusal> {
        let spec = {
            let state = self.lock();
            let Some(memory) = state.intents.get(&intent.client_order_id) else {
                return Err(SendRefusal::NotSent("the id's history is gone".to_string()));
            };
            // Once only: a link is consumed only by the original's completion
            // or its operation's end, and an ended original has its outcome.
            if memory.row.intent_id != intent.intent_id
                || !memory.retry_refused
                || memory.tickets != 1
                || memory.replaced_by.is_some()
                || memory.outcome.is_some()
            {
                return Err(SendRefusal::NotSent("this retry cannot be replaced".to_string()));
            }
            let mut params = memory.row.params.clone();
            if let Value::Object(fields) = &mut params {
                fields.insert(
                    "replaces_client_order_id".to_string(),
                    Value::String(intent.client_order_id.clone()),
                );
                fields.insert("first_rejection".to_string(), Value::String(rules_rejection));
            }
            IntentSpec {
                position_id: memory.row.position_id,
                kind: memory.row.kind,
                client_order_id: fresh_client_order_id,
                cancels_order_id: None,
                params,
            }
        };
        let fresh = self.open_intent(spec);
        if let Some(memory) = self.lock().intents.get_mut(&intent.client_order_id) {
            memory.replaced_by = Some(fresh.clone());
        }
        // The refused original is uncertain from here: one request went out
        // under it with no definite answer and none more will. Its occurrence,
        // stamp and incident are registered now, before the replacement's
        // commit or venue answer is awaited (design §2.2).
        self.settle_refused_original(intent);
        self.before_send(&fresh, WriteClass::Bounded).await?;
        Ok(fresh)
    }

    /// Register the uncertain ending of a refused retry's original, at once
    /// and with no await: its occurrence closes the fence and stamps it, and
    /// its write waits in the retry set. Nothing when its ending is already
    /// registered or is not uncertain.
    fn settle_refused_original(&self, intent: &JournaledIntent) {
        let outcome = {
            let state = self.lock();
            let Some(memory) = state.intents.get(&intent.client_order_id) else {
                return;
            };
            if memory.row.intent_id != intent.intent_id || memory.outcome.is_some() {
                return;
            }
            Self::failed_outcome(memory)
        };
        if let Some(outcome) = outcome.filter(OutcomeRow::is_uncertain) {
            self.register_outcome(intent, outcome);
        }
    }

    fn replacement_of(&self, intent: &JournaledIntent) -> Option<JournaledIntent> {
        let state = self.lock();
        state
            .intents
            .get(&intent.client_order_id)
            .and_then(|memory| memory.replaced_by.clone())
    }

    /// Whether a request under this intent reached the venue adapter's send.
    pub fn ticket_issued(&self, intent: &JournaledIntent) -> bool {
        self.lock()
            .intents
            .get(&intent.client_order_id)
            .is_some_and(|memory| memory.ticket_issued)
    }

    /// The send start, the linearization point (design §2.6): under the gate's
    /// mutex, with the journal's memory taken inside it, an entry is refused
    /// unless the daemon is open and unfenced, a typed no-send; otherwise the
    /// send is counted and its ticket issued, in one critical section with no
    /// await. A send with no intent here (funding) is counted and has no
    /// ticket.
    fn begin_send(&self, client_order_id: &str) -> Result<(), SendRefusal> {
        let mut gate = self.gate.lock();
        let mut state = self.lock();
        let entry = state
            .intents
            .get(client_order_id)
            .is_some_and(|memory| memory.row.kind == IntentKind::Entry);
        // From PR P the latch closes admissions until a restart. The admission
        // checks see it first; this is for one already past them.
        if entry && self.latch.load(Ordering::SeqCst) {
            return Err(SendRefusal::NotSent(
                "entry refused at its send: the journal latch is set".to_string(),
            ));
        }
        // From PR F the fence: an entry ticketed before it closed is in
        // flight; one reaching here after is a proven no-send.
        if entry && !gate.entry_send_allowed() {
            let why = if gate.fenced() {
                "fenced"
            } else {
                gate.phase().as_str()
            };
            return Err(SendRefusal::NotSent(format!(
                "entry refused at its send: the daemon is {why}"
            )));
        }
        gate.send_started();
        let Some(memory) = state.intents.get_mut(client_order_id) else {
            return Ok(());
        };
        memory.ticket_issued = true;
        memory.tickets += 1;
        let operation = memory.operation;
        if let Some(listed) = state.operations.get_mut(&operation) {
            if let Some(entry) =
                listed.intents.iter_mut().find(|e| e.client_order_id == client_order_id)
            {
                entry.ticket_issued = true;
            }
        }
        Ok(())
    }

    fn note_rejected(&self, client_order_id: &str) {
        let mut state = self.lock();
        let Some(memory) = state.intents.get_mut(client_order_id) else {
            return;
        };
        if let Some(attempt) = memory.attempts.last_mut() {
            attempt.row.rejected_at.get_or_insert_with(Utc::now);
        }
    }

    /// The venue accepted the order. After a replacement the acceptance is the
    /// fresh intent's, and the refused one ends on its own attempts.
    pub async fn finish_success(&self, intent: &JournaledIntent, evidence: Option<Value>) {
        let success = OutcomeRow {
            venue_outcome: Some(VenueOutcome::Success),
            venue_evidence: evidence,
            ..OutcomeRow::default()
        };
        let Some(fresh) = self.replacement_of(intent) else {
            self.write_outcome(intent, success).await;
            return;
        };
        // Both endings are registered before either write is awaited: a
        // completion cancelled during a write leaves the known acceptance, with
        // its evidence, in the retry set, not an unfinished replacement for
        // the operation's end to take for uncertain.
        let accepted = self.register_outcome(&fresh, success);
        let refused = self.register_failed_own(intent);
        if let Some(batch) = refused {
            self.commit_outcome(intent, &batch).await;
        }
        if let Some(batch) = accepted {
            self.commit_outcome(&fresh, &batch).await;
        }
        self.unlink_replacement(intent);
    }

    /// The order ended without a venue acceptance: `Rejected` if every attempt
    /// was authoritatively refused, marked not sent if no request left,
    /// otherwise uncertain.
    pub async fn finish_failed(&self, intent: &JournaledIntent) {
        let Some(fresh) = self.replacement_of(intent) else {
            self.finish_failed_own(intent).await;
            return;
        };
        // As for a success: both endings registered, the replacement's first,
        // before either write is awaited.
        let replaced = self.register_failed_own(&fresh);
        let refused = self.register_failed_own(intent);
        if let Some(batch) = replaced {
            self.commit_outcome(&fresh, &batch).await;
        }
        if let Some(batch) = refused {
            self.commit_outcome(intent, &batch).await;
        }
        self.unlink_replacement(intent);
    }

    /// Completion consumed a refused retry's link to its replacement: the
    /// original no longer names it, and goes now unless something of it still
    /// waits to land (then [`Self::landed`] collects it).
    fn unlink_replacement(&self, original: &JournaledIntent) {
        let mut state = self.lock();
        if state
            .intents
            .get(&original.client_order_id)
            .is_some_and(|memory| memory.row.intent_id == original.intent_id)
        {
            Self::unlink(&mut state, &original.client_order_id);
        }
    }

    /// Drop the link an original holds and collect it once nothing of it waits
    /// in the retry set: its ending landed, or overflowed and was discarded.
    fn unlink(state: &mut JournalState, client_order_id: &str) {
        let Some(memory) = state.intents.get_mut(client_order_id) else {
            return;
        };
        if memory.replaced_by.take().is_none() {
            return;
        }
        if memory.outcome.is_some() && !memory.in_retry_set {
            state.intents.remove(client_order_id);
        }
    }

    async fn finish_failed_own(&self, intent: &JournaledIntent) {
        if let Some(batch) = self.register_failed_own(intent) {
            self.commit_outcome(intent, &batch).await;
        }
    }

    /// The synchronous half of [`Self::finish_failed_own`]: seal the intent
    /// and register its failed ending, unless it has one. The batch still to
    /// write, or `None` when nothing is (no request left, or it has landed).
    fn register_failed_own(&self, intent: &JournaledIntent) -> Option<JournalBatch> {
        let ending = {
            let mut state = self.lock();
            let memory = state.intents.get_mut(&intent.client_order_id)?;
            if memory.row.intent_id != intent.intent_id {
                return None;
            }
            memory.sealed = true;
            // Settled already (a refused retry's original): only its write,
            // and none once it has landed.
            match &memory.outcome {
                Some(_) if !memory.has_unlanded() => return None,
                Some(_) => Err(memory.batch()),
                None => Ok(Self::failed_outcome(memory)),
            }
        };
        match ending {
            Err(registered) => Some(registered),
            Ok(Some(outcome)) => self.register_outcome(intent, outcome),
            Ok(None) => {
                self.lock().intents.remove(&intent.client_order_id);
                None
            },
        }
    }

    /// `None` when nothing was sent and no row can exist: nothing to write.
    ///
    /// `not_sent` is a proof, and only a ticket can supply it. A cancel does
    /// not pass through the attempt ledger, so it has no ticket either way:
    /// once its attempt exists it may have reached the venue.
    fn failed_outcome(memory: &IntentMemory) -> Option<OutcomeRow> {
        if !memory.ticket_issued && !memory.commit_uncertain {
            if memory.attempts.is_empty() {
                return None;
            }
            if memory.row.kind != IntentKind::Cancel {
                return Some(OutcomeRow { not_sent: true, ..OutcomeRow::default() });
            }
        }
        if memory.ticket_issued && memory.attempts.iter().all(|a| a.row.rejected_at.is_some()) {
            return Some(OutcomeRow {
                venue_outcome: Some(VenueOutcome::Rejected),
                ..OutcomeRow::default()
            });
        }
        Some(OutcomeRow {
            uncertain_at: Some(Utc::now()),
            ..OutcomeRow::default()
        })
    }

    /// A cancel's result, under the class [`classify_cancel`] gave it.
    pub async fn finish_cancel(&self, intent: &JournaledIntent, class: &CancelClass) {
        let outcome = OutcomeRow {
            venue_outcome: class.venue_outcome(),
            cancel_class: Some(class.label()),
            venue_evidence: class.evidence(),
            uncertain_at: (!class.is_definite()).then(Utc::now),
            ..OutcomeRow::default()
        };
        self.write_outcome(intent, outcome).await;
    }

    /// Run `cancel` as its own intent, under a local id that never reaches the
    /// venue, and classify what it answered. The cancel goes out whatever the
    /// journal says, and the journal's wait is bounded.
    pub async fn journaled_cancel<F>(
        &self,
        position_id: Option<PositionId>,
        order_id: &str,
        cancel: F,
    ) -> CancelClass
    where
        F: Future<Output = Result<StopCancelOutcome, ExecError>>,
    {
        let intent = self.open_intent(IntentSpec {
            position_id,
            kind: IntentKind::Cancel,
            client_order_id: format!("cxl-{}", Uuid::now_v7().simple()),
            cancels_order_id: Some(order_id.to_string()),
            params: Value::Null,
        });
        if let Err(refusal) = self.before_send(&intent, WriteClass::Bounded).await {
            warn!(%order_id, %refusal, "Cancel goes out without a journal row");
        }
        let class = classify_cancel(cancel.await);
        self.finish_cancel(&intent, &class).await;
        class
    }

    /// An orphan sweep's cancel of the `ins-` order `order_id` (design §2.9):
    /// withheld with nothing written or sent while the veto holds, checked
    /// once before its journal write; otherwise journaled like any cancel and
    /// run as an orphan sweep's cancel, so the transport re-checks the veto at
    /// its handoff, after that write. Refused there, nothing left: the intent
    /// ends `not_sent`, class `withheld`, a proven no-send and no occurrence.
    pub async fn journaled_orphan_cancel<F>(&self, order_id: &str, cancel: F) -> OrphanCancel
    where
        F: Future<Output = Result<StopCancelOutcome, ExecError>>,
    {
        if self.gate.orphan_cancel_vetoed() {
            return OrphanCancel::Withheld;
        }
        let intent = self.open_intent(IntentSpec {
            position_id: None,
            kind: IntentKind::Cancel,
            client_order_id: format!("cxl-{}", Uuid::now_v7().simple()),
            cancels_order_id: Some(order_id.to_string()),
            params: Value::Null,
        });
        if let Err(refusal) = self.before_send(&intent, WriteClass::Bounded).await {
            warn!(%order_id, %refusal, "Orphan cancel goes on without a journal row");
        }
        let handoff = Arc::new(OrphanSweepCancel::default());
        let result = orphan_sweep_cancel(Arc::clone(&handoff), cancel).await;
        if handoff.withheld() && !handoff.passed() {
            self.write_outcome(&intent, OutcomeRow {
                not_sent: true,
                cancel_class: Some("withheld"),
                ..OutcomeRow::default()
            })
            .await;
            return OrphanCancel::Withheld;
        }
        let class = classify_cancel(result);
        self.finish_cancel(&intent, &class).await;
        OrphanCancel::Sent(class)
    }

    /// The result's write. Never fails: the caller keeps the venue's answer
    /// either way. It is registered for retry before the wait, so a future
    /// dropped mid-commit leaves it in the retry set, counted and flushed. An
    /// uncertain outcome records its `uncertain_outcome` occurrence first, in
    /// the same critical section, and carries the generation it got: the fence
    /// is closed before anything is awaited (design §2.2).
    async fn write_outcome(&self, intent: &JournaledIntent, outcome: OutcomeRow) {
        if let Some(batch) = self.register_outcome(intent, outcome) {
            self.commit_outcome(intent, &batch).await;
        }
    }

    /// The synchronous half of [`Self::write_outcome`]: in one critical
    /// section, the gate's mutex before this journal's memory, an uncertain
    /// outcome records its occurrence and carries its generation, and the
    /// outcome is registered for retry. The batch to commit, or `None` when the
    /// intent is not this one in memory.
    fn register_outcome(
        &self,
        intent: &JournaledIntent,
        mut outcome: OutcomeRow,
    ) -> Option<JournalBatch> {
        let (batch, occurrence, discarded) = {
            let (mut gate, mut state) = self.lock_both();
            let memory = state.intents.get_mut(&intent.client_order_id)?;
            if memory.row.intent_id != intent.intent_id {
                return None;
            }
            let occurrence = outcome.is_uncertain().then(|| {
                let occurrence = gate.occur(IncidentReason::UncertainOutcome);
                outcome.uncertain_generation = Some(occurrence.generation);
                occurrence
            });
            memory.sealed = true;
            memory.outcome = Some(outcome);
            memory.outcome_landed = false;
            let batch = memory.batch();
            let discarded = Self::enqueue(&mut gate, &mut state, &intent.client_order_id);
            (batch, occurrence, discarded)
        };
        if let Some(occurrence) = occurrence {
            occurrence.log();
        }
        if let Some(occurrence) = discarded {
            Self::report_discarded(&intent.client_order_id, &occurrence);
        }
        Some(batch)
    }

    /// The registered outcome's write, bounded; on failure the retry set
    /// keeps it.
    async fn commit_outcome(&self, intent: &JournaledIntent, batch: &JournalBatch) {
        match self.bounded_commit(batch).await {
            Ok(()) => Self::landed(&mut self.lock(), &intent.client_order_id, batch),
            Err(failure) => {
                warn!(
                    client_order_id = %intent.client_order_id,
                    ?failure,
                    "Journal result write failed; it will be retried"
                );
                // Unless the retry set had no room for it: then its history
                // goes now, as an overflow's does, or with its link.
                let mut state = self.lock();
                let unkept = state.intents.get(&intent.client_order_id).is_some_and(|memory| {
                    memory.outcome.is_some() && !memory.in_retry_set && memory.replaced_by.is_none()
                });
                if unkept {
                    state.intents.remove(&intent.client_order_id);
                }
            },
        }
    }

    /// Add to `persistence_pending`. On overflow the history is discarded:
    /// a `persistence_failure` occurrence, returned for the caller to report
    /// with [`Self::report_discarded`] once it holds no lock.
    fn enqueue(
        gate: &mut GateState,
        state: &mut JournalState,
        client_order_id: &str,
    ) -> Option<Occurrence> {
        Self::try_enqueue(gate, state, client_order_id).err().flatten()
    }

    fn report_discarded(client_order_id: &str, occurrence: &Occurrence) {
        error!(%client_order_id, "Journal retry set is full; discarding this intent's history");
        occurrence.log();
    }

    /// `Err` on overflow: the history is discarded, with its
    /// `persistence_failure` occurrence (which sets the latch), and an intent
    /// that has its outcome is forgotten here. `Err(None)` when the intent is
    /// not in memory at all.
    fn try_enqueue(
        gate: &mut GateState,
        state: &mut JournalState,
        client_order_id: &str,
    ) -> Result<(), Option<Occurrence>> {
        let Some(memory) = state.intents.get_mut(client_order_id) else {
            return Err(None);
        };
        if memory.in_retry_set {
            return Ok(());
        }
        if state.retry_set.len() + state.suspensions.len() < RETRY_SET_CAPACITY {
            memory.in_retry_set = true;
            state.retry_set.push_back(client_order_id.to_string());
            return Ok(());
        }
        memory.sealed = true;
        // A refused retry's original keeps its memory while it names its
        // replacement: unlinking collects it.
        if memory.outcome.is_some() && memory.replaced_by.is_none() {
            state.intents.remove(client_order_id);
        }
        Err(Some(gate.occur(IncidentReason::PersistenceFailure)))
    }

    fn landed(state: &mut JournalState, client_order_id: &str, batch: &JournalBatch) {
        let Some(memory) = state.intents.get_mut(client_order_id) else {
            return;
        };
        memory.mark_landed(batch);
        if !memory.has_unlanded() {
            // A refused retry's original that still names its replacement
            // stays: its completion reads the link. Unlinking collects it.
            let finished = memory.outcome.is_some() && memory.replaced_by.is_none();
            let queued = memory.in_retry_set;
            memory.in_retry_set = false;
            if queued {
                state.retry_set.retain(|id| id != client_order_id);
            }
            if finished {
                state.intents.remove(client_order_id);
            }
        }
    }

    /// One pass of the retry loop. Incident upserts go first, then stream
    /// suspensions, then intents. Each part stops at its first failure,
    /// sending that entry to the back, so a dead database costs one bound per
    /// part and pass and one bad row cannot starve the others, of its own part
    /// or of another: an upsert or a suspension that keeps failing never keeps
    /// an intent's rows from being tried. The pass stops starting commits
    /// after a few bounds even when each succeeds slowly. Returns the entries
    /// written.
    pub async fn flush_pending(&self) -> usize {
        // One pass at a time: the retry loop and the stop's final flush never
        // write the same snapshot twice, so an incident upsert delayed in one
        // cannot land after another landed it and a release removed it.
        let _pass = self.flush_pass.lock().await;
        let started = std::time::Instant::now();
        let pass_bound = self.commit_bound * RETRY_PASS_COMMIT_BOUNDS;
        let mut written = self.flush_incidents(started, pass_bound).await;
        written += self.flush_suspensions(started, pass_bound).await;
        let batches: Vec<(String, JournalBatch)> = {
            let state = self.lock();
            state
                .retry_set
                .iter()
                .take(RETRY_PASS_LIMIT.saturating_sub(written))
                .filter_map(|id| state.intents.get(id).map(|m| (id.clone(), m.batch())))
                .collect()
        };
        for (client_order_id, batch) in batches {
            if started.elapsed() >= pass_bound {
                break;
            }
            match self.bounded_commit(&batch).await {
                Ok(()) => {
                    Self::landed(&mut self.lock(), &client_order_id, &batch);
                    written += 1;
                },
                Err(failure) => {
                    let (mut gate, mut state) = self.lock_both();
                    if failure == CommitFailure::IdSpent {
                        let occurrence = gate.occur(IncidentReason::PersistenceFailure);
                        state.retry_set.retain(|id| *id != client_order_id);
                        match state.intents.get_mut(&client_order_id) {
                            // Its link stays for its completion; unlinking
                            // collects it.
                            Some(memory) if memory.replaced_by.is_some() => {
                                memory.in_retry_set = false;
                            },
                            _ => {
                                state.intents.remove(&client_order_id);
                            },
                        }
                        drop((gate, state));
                        error!(%client_order_id, "Journal row conflicts with another intent");
                        occurrence.log();
                    } else if let Some(position) =
                        state.retry_set.iter().position(|id| *id == client_order_id)
                    {
                        if let Some(id) = state.retry_set.remove(position) {
                            state.retry_set.push_back(id);
                        }
                    }
                    break;
                },
            }
        }
        written
    }

    /// The incidents part of a retry pass, first: at most one upsert per
    /// reason, and each is what tells the next start the fence is closed.
    /// Returns the upserts written; a failure ends this part.
    async fn flush_incidents(&self, started: std::time::Instant, pass_bound: Duration) -> usize {
        let pending = self.gate.lock().incidents_to_write();
        let mut written = 0;
        for (incident, version) in pending {
            if started.elapsed() >= pass_bound {
                break;
            }
            self.statements_total
                .fetch_add(u64::from(INCIDENT_STATEMENTS), Ordering::Relaxed);
            let upsert = self.backend.upsert_incident(&incident);
            let failure = match tokio::time::timeout(self.commit_bound, upsert).await {
                Ok(Ok(())) => None,
                Ok(Err(failure)) => Some(failure),
                Err(_) => Some(CommitFailure::Uncertain("no answer within the bound".to_string())),
            };
            match failure {
                None => {
                    self.gate.lock().incident_landed(incident.reason, version);
                    written += 1;
                },
                Some(failure) => {
                    warn!(
                        reason = incident.reason.as_str(),
                        ?failure,
                        "Incident not recorded yet; it will be retried"
                    );
                    return written;
                },
            }
        }
        written
    }

    /// The suspensions part of a retry pass, first: they are few, and each one
    /// is what tells the next start a stream needs repair. Returns the entries
    /// written; a failure ends this part, with that entry sent to the back,
    /// and the pass goes on to the intents.
    async fn flush_suspensions(&self, started: std::time::Instant, pass_bound: Duration) -> usize {
        let pending: Vec<StreamSuspensionRow> =
            self.lock().suspensions.iter().take(RETRY_PASS_LIMIT).cloned().collect();
        let mut written = 0;
        for suspension in pending {
            if started.elapsed() >= pass_bound {
                break;
            }
            self.statements_total
                .fetch_add(u64::from(SUSPENSION_STATEMENTS), Ordering::Relaxed);
            let write = self.backend.record_suspension(&suspension);
            let failure = match tokio::time::timeout(self.commit_bound, write).await {
                Ok(Ok(())) => None,
                Ok(Err(failure)) => Some(failure),
                Err(_) => Some(CommitFailure::Uncertain("no answer within the bound".to_string())),
            };
            let mut state = self.lock();
            let position = state
                .suspensions
                .iter()
                .position(|row| row.suspension_id == suspension.suspension_id);
            let Some(position) = position else {
                continue;
            };
            let row = state.suspensions.remove(position);
            match failure {
                None => written += 1,
                Some(failure) => {
                    warn!(
                        stream_key = %suspension.stream_key,
                        ?failure,
                        "Stream suspension not recorded yet; it will be retried"
                    );
                    if let Some(row) = row {
                        state.suspensions.push_back(row);
                    }
                    return written;
                },
            }
        }
        written
    }

    /// The audited release (design §2.7): risk acceptance of everything up to
    /// the generation `request.generation`, never a venue outcome and never a
    /// booking. Refused, with nothing written, while the latch is set, when
    /// the quoted generation is not the current one, or while anything waits
    /// in `persistence_pending`. Then the backend's one transaction tombstones
    /// the uncertain intents stamped at or below it and releases the incidents
    /// whose last generation is at or below it, and the fence opens only if
    /// the generation is still the quoted one and the latch clear. A
    /// transaction that fails or does not answer within [`RELEASE_BOUND`]
    /// leaves the fence closed; it may have committed, and a retry quoting the
    /// same generation finds nothing more to do and opens it.
    ///
    /// # Errors
    /// [`ReleaseRefusal`], with nothing changed here.
    pub async fn release(&self, request: ReleaseRecord) -> Result<ReleaseReceipt, ReleaseRefusal> {
        {
            let (gate, state) = self.lock_both();
            if self.latch_set() {
                return Err(ReleaseRefusal::Latched);
            }
            if gate.generation() != request.generation {
                return Err(ReleaseRefusal::Stale { current: gate.generation() });
            }
            let pending = state.pending() + gate.incidents_persisting();
            if pending > 0 {
                return Err(ReleaseRefusal::Persisting { pending });
            }
        }
        let release = self.backend.release(&request);
        let counts = match tokio::time::timeout(self.release_bound, release).await {
            Ok(Ok(counts)) => counts,
            Ok(Err(error)) => return Err(ReleaseRefusal::Backend(error)),
            Err(_) => {
                return Err(ReleaseRefusal::Backend(format!(
                    "no answer within {} ms; it may have committed",
                    self.release_bound.as_millis()
                )))
            },
        };
        let fence_open = self.gate.release_through(request.generation);
        Ok(ReleaseReceipt {
            decision_id: request.decision_id,
            generation: request.generation,
            counts,
            fence_open,
        })
    }

    /// Evidence for the finality rule, read after the venue's not-found.
    /// Memory is read on both sides of the database read: a row lands before
    /// its memory goes, and an attempt that starts during the read is seen.
    /// A failed read concludes nothing.
    pub async fn attempt_evidence(&self, client_order_id: &str) -> AttemptEvidence {
        const IN_MEMORY: AttemptEvidence =
            AttemptEvidence::Unresolved { unresolved: 1, rejected: 0 };
        if self.is_live(client_order_id) {
            return IN_MEMORY;
        }
        let read =
            self.backend
                .attempt_states(&self.venue_id, &self.account_scope, client_order_id);
        let states = match tokio::time::timeout(self.commit_bound, read).await {
            Ok(Ok(states)) => states,
            Ok(Err(error)) => {
                warn!(%client_order_id, %error, "Attempt evidence is unreadable");
                return AttemptEvidence::Unavailable;
            },
            Err(_) => return AttemptEvidence::Unavailable,
        };
        if self.is_live(client_order_id) {
            return IN_MEMORY;
        }
        let rejected = states.iter().filter(|rejected| **rejected).count() as u64;
        let unresolved = states.len() as u64 - rejected;
        match (rejected, unresolved) {
            (0, 0) => AttemptEvidence::NoRecord,
            (rejected, 0) => AttemptEvidence::AllRejected { rejected },
            (rejected, unresolved) => AttemptEvidence::Unresolved { unresolved, rejected },
        }
    }
}

tokio::task_local! {
    static CURRENT_OPERATION: CurrentOperation;
}

#[derive(Clone)]
struct CurrentOperation {
    journal: Arc<OrderJournal>,
    id: u64,
    statements: Arc<AtomicU32>,
    /// Set when the operation holds a permit.
    permit: Option<OperationKind>,
}

/// Called immediately before a request is sent, with no await in between: by
/// the ledger, or by a test double that has none. Inside an operation it is
/// the gate's send start: an entry is refused unless the daemon is open,
/// otherwise the ticket is issued. Outside one it does nothing: no entry can
/// be authorized there, and the choke point sees the request.
///
/// # Errors
/// An entry while the daemon is stopping: a proven no-send.
pub fn begin_send(client_order_id: &str) -> Result<(), SendRefusal> {
    CURRENT_OPERATION
        .try_with(|current| current.journal.begin_send(client_order_id))
        .unwrap_or(Ok(()))
}

/// The ticket without the gate's refusal, for a test double that only needs
/// the ticket; [`begin_send`] is what every send goes through.
pub fn report_send_started(client_order_id: &str) {
    let _ = begin_send(client_order_id);
}

/// Whether this task holds a permit of `gate`.
pub(crate) fn holds_permit_of(gate: &OperationGate) -> bool {
    CURRENT_OPERATION
        .try_with(|current| current.permit.is_some() && std::ptr::eq(&*current.journal.gate, gate))
        .unwrap_or(false)
}

/// The kind of permit this task holds, if any.
pub fn held_permit() -> Option<OperationKind> {
    CURRENT_OPERATION.try_with(|current| current.permit).ok().flatten()
}

/// The phase of the gate this task's operation belongs to, if it is in one.
pub fn current_phase() -> Option<Phase> {
    CURRENT_OPERATION.try_with(|current| current.journal.gate.phase()).ok()
}

/// The ledger calls this on an authoritative venue rejection.
pub(crate) fn report_rejected(client_order_id: &str) {
    let _ = CURRENT_OPERATION.try_with(|current| current.journal.note_rejected(client_order_id));
}

/// Whether this task is already inside an operation.
pub fn in_operation() -> bool {
    CURRENT_OPERATION.try_with(|_| ()).is_ok()
}

/// One entry of `operations_in_flight`, for as long as this guard lives. An
/// intent still unfinished when it drops (its future was dropped mid-send) is
/// recorded as uncertain through the retry set.
pub struct Operation {
    journal: Arc<OrderJournal>,
    id: u64,
    statements: Arc<AtomicU32>,
    permit: Option<OperationKind>,
    listed_unpermitted: bool,
}

impl Operation {
    /// Run `work` as this operation.
    pub async fn scope<F: Future>(&self, work: F) -> F::Output {
        let current = CurrentOperation {
            journal: Arc::clone(&self.journal),
            id: self.id,
            statements: Arc::clone(&self.statements),
            permit: self.permit,
        };
        CURRENT_OPERATION.scope(current, work).await
    }

    /// Statements charged to this operation's journal commits so far.
    pub fn journal_statements(&self) -> u32 {
        self.statements.load(Ordering::Relaxed)
    }

    /// Whether an entry under this operation reached its send. Read it before
    /// the guard drops, on `Ok` and on `Err` alike.
    pub fn entry_ticket_issued(&self) -> bool {
        self.journal.lock().operations.get(&self.id).is_some_and(|listed| {
            listed
                .intents
                .iter()
                .any(|intent| intent.kind == IntentKind::Entry && intent.ticket_issued)
        })
    }
}

impl Drop for Operation {
    fn drop(&mut self) {
        let statements = self.statements.load(Ordering::Relaxed);
        self.journal.operation_statements_max.fetch_max(statements, Ordering::Relaxed);
        if statements > OPERATION_STATEMENT_BOUND {
            self.journal.operations_over_statement_bound.fetch_add(1, Ordering::Relaxed);
            warn!(statements, "Journal operation went past its reviewed statement bound");
        }
        let (mut occurrences, mut discarded) = (Vec::new(), Vec::new());
        let (mut gate, mut state) = self.journal.lock_both();
        state.operations.remove(&self.id);
        if self.listed_unpermitted {
            state.unpermitted_listed -= 1;
        }
        let abandoned: HashSet<String> = state
            .intents
            .iter()
            .filter(|(_, memory)| memory.operation == self.id && memory.outcome.is_none())
            .map(|(id, _)| id.clone())
            .collect();
        for client_order_id in abandoned {
            let Some(memory) = state.intents.get_mut(&client_order_id) else {
                continue;
            };
            memory.sealed = true;
            match OrderJournal::failed_outcome(memory) {
                Some(mut outcome) => {
                    // Dropped mid-flight: the venue's answer is unknown.
                    if !outcome.not_sent && outcome.venue_outcome.is_none() {
                        outcome.uncertain_at.get_or_insert_with(Utc::now);
                    }
                    if outcome.is_uncertain() {
                        let occurrence = gate.occur(IncidentReason::UncertainOutcome);
                        outcome.uncertain_generation = Some(occurrence.generation);
                        occurrences.push(occurrence);
                    }
                    memory.outcome = Some(outcome);
                    if let Some(occurrence) =
                        OrderJournal::enqueue(&mut gate, &mut state, &client_order_id)
                    {
                        discarded.push((client_order_id, occurrence));
                    }
                },
                None => {
                    state.intents.remove(&client_order_id);
                },
            }
        }
        // Links no completion consumed (cancelled or unwound before it): each
        // original goes now unless its ending still waits in the retry set,
        // an overflowed one too, so nothing stays without an owner.
        let linked: Vec<String> = state
            .intents
            .iter()
            .filter(|(_, memory)| memory.operation == self.id && memory.replaced_by.is_some())
            .map(|(id, _)| id.clone())
            .collect();
        for client_order_id in linked {
            OrderJournal::unlink(&mut state, &client_order_id);
        }
        drop((gate, state));
        occurrences.iter().for_each(Occurrence::log);
        for (client_order_id, occurrence) in &discarded {
            OrderJournal::report_discarded(client_order_id, occurrence);
        }
    }
}

/// An operation permit (ADR-0056 step 7, PR S): a slot of its kind's class
/// and the operation's entry in `operations_in_flight`, held in a task-local
/// scope for the whole operation. It must be completed: dropped without
/// completion, its future cancelled or panicked before it returned, it counts
/// as aborted and sets the latch, whatever its rows say. An operation that
/// returned an error has completed.
pub struct Permit {
    /// Taken out on drop, so the registry entry goes, with any abandoned
    /// intent put in the retry set, before the slot is released.
    operation: Option<Operation>,
    kind: OperationKind,
    completed: bool,
}

impl Permit {
    pub fn kind(&self) -> OperationKind {
        self.kind
    }

    /// Run `work` under this permit and complete it when `work` returns.
    /// Dropping the returned future before then aborts the permit.
    pub async fn run<F: Future>(mut self, work: F) -> F::Output {
        let output = self.scope(work).await;
        self.completed = true;
        output
    }

    /// Run `work` under this permit, which stays held.
    pub async fn scope<F: Future>(&self, work: F) -> F::Output {
        match &self.operation {
            Some(operation) => operation.scope(work).await,
            None => work.await,
        }
    }

    /// The operation ran to its end.
    pub fn complete(mut self) {
        self.completed = true;
    }
}

impl Drop for Permit {
    fn drop(&mut self) {
        let Some(operation) = self.operation.take() else {
            return;
        };
        let gate = Arc::clone(&operation.journal.gate);
        if !self.completed {
            gate.abort(self.kind);
        }
        drop(operation);
        gate.release(self.kind.class());
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::venue::SubmitAttemptLedger;

    const BOUND: Duration = Duration::from_millis(30);

    fn journal() -> (Arc<StubJournalBackend>, Arc<OrderJournal>) {
        let backend = Arc::new(StubJournalBackend::default());
        let journal = OrderJournal::new(backend.clone(), &VenueId::new("test:venue"));
        (backend, Arc::new(journal.with_commit_bound(BOUND)))
    }

    fn spec(id: &str, kind: IntentKind) -> IntentSpec {
        IntentSpec {
            position_id: None,
            kind,
            client_order_id: id.to_string(),
            cancels_order_id: None,
            params: json!({}),
        }
    }

    /// Run `work` inside an operation, keeping it open.
    async fn in_op<F: Future>(journal: &Arc<OrderJournal>, work: F) -> (F::Output, Operation) {
        let operation = journal.begin_operation();
        (operation.scope(work).await, operation)
    }

    fn states(row: &StubJournalRow) -> Vec<bool> {
        row.attempts.iter().map(|a| a.rejected_at.is_some()).collect()
    }

    #[tokio::test]
    async fn an_intent_and_its_first_attempt_commit_together() {
        let (backend, journal) = journal();
        let ((), _op) = in_op(&journal, async {
            let intent = journal.open_intent(spec("a", IntentKind::Entry));
            journal.before_send(&intent, WriteClass::Strict).await.unwrap();
        })
        .await;
        let commits = backend.commits();
        assert_eq!(commits.len(), 1);
        assert_eq!(commits[0].attempts.len(), 1);
        assert_eq!(backend.row("a").unwrap().intent.unwrap().kind, IntentKind::Entry);
    }

    #[tokio::test]
    async fn a_strict_commit_with_a_known_failure_is_a_proven_no_send() {
        let (backend, journal) = journal();
        backend.push_fault(JournalFault::Known);
        let (refusal, _op) = in_op(&journal, async {
            let intent = journal.open_intent(spec("a", IntentKind::Entry));
            let refusal = journal.before_send(&intent, WriteClass::Strict).await.unwrap_err();
            journal.finish_failed(&intent).await;
            refusal
        })
        .await;
        assert!(matches!(refusal, SendRefusal::NotSent(_)), "{refusal:?}");
        assert!(backend.row("a").is_none(), "no row, so nothing to mark");
        assert!(!journal.is_live("a"), "nothing left to write");
        assert!(!journal.latch_set(), "strict never sends without its row");
    }

    #[tokio::test]
    async fn a_strict_commit_that_times_out_is_uncertain_and_is_recorded() {
        let (backend, journal) = journal();
        backend.push_fault(JournalFault::Hang);
        let (refusal, _op) = in_op(&journal, async {
            let intent = journal.open_intent(spec("a", IntentKind::Entry));
            let refusal = journal.before_send(&intent, WriteClass::Strict).await.unwrap_err();
            journal.finish_failed(&intent).await;
            refusal
        })
        .await;
        assert!(matches!(refusal, SendRefusal::UncertainCommit(_)), "{refusal:?}");
        let row = backend.row("a").expect("the outcome write lands the row");
        let outcome = row.outcome.unwrap();
        assert!(outcome.uncertain_at.is_some() && outcome.venue_outcome.is_none());
        assert!(!outcome.not_sent, "an uncertain commit is not a proven no-send");
    }

    #[tokio::test]
    async fn a_bounded_send_goes_out_without_its_row_and_sets_the_latch() {
        for fault in [
            JournalFault::Known,
            JournalFault::NotLandedUncertain,
            JournalFault::Hang,
        ] {
            let (backend, journal) = journal();
            backend.push_fault(fault);
            let (result, _op) = in_op(&journal, async {
                let intent = journal.open_intent(spec("a", IntentKind::Exit));
                journal.before_send(&intent, WriteClass::Bounded).await
            })
            .await;
            assert_eq!(result, Ok(()), "{fault:?}");
            assert!(journal.latch_set(), "{fault:?}");
            // The intent, and its `unlanded_send` incident's upsert (PR F).
            assert_eq!(journal.retry_set_depth(), 2, "{fault:?}");
            assert!(journal.gate().fenced(), "{fault:?}");
        }
    }

    #[tokio::test]
    async fn the_commit_waits_at_most_its_bound() {
        let (backend, journal) = journal();
        backend.push_fault(JournalFault::Hang);
        let started = std::time::Instant::now();
        let (_, _op) = in_op(&journal, async {
            let intent = journal.open_intent(spec("a", IntentKind::Exit));
            journal.before_send(&intent, WriteClass::Bounded).await
        })
        .await;
        assert!(started.elapsed() < Duration::from_secs(2), "{:?}", started.elapsed());
    }

    /// Rule 2: the commit of a further attempt carries the earlier attempt even
    /// when that one only exists in memory.
    #[tokio::test]
    async fn a_further_attempt_commits_with_the_attempts_known_only_in_memory() {
        let (backend, journal) = journal();
        backend.push_fault(JournalFault::NotLandedUncertain);
        let ((), _op) = in_op(&journal, async {
            let intent = journal.open_intent(spec("a", IntentKind::Exit));
            journal.before_send(&intent, WriteClass::Bounded).await.unwrap();
            assert!(backend.row("a").is_none(), "the first row did not land");
            journal.before_send(&intent, WriteClass::Bounded).await.unwrap();
        })
        .await;
        let row = backend.row("a").unwrap();
        assert_eq!(row.attempts.len(), 2, "landed together");
    }

    #[tokio::test]
    async fn a_further_attempt_is_refused_when_its_commit_fails_or_is_uncertain() {
        for (fault, uncertain) in [
            (JournalFault::Known, false),
            (JournalFault::AckLost, true),
            (JournalFault::NotLandedUncertain, true),
            (JournalFault::Hang, true),
        ] {
            let (backend, journal) = journal();
            let ((), _op) = in_op(&journal, async {
                let intent = journal.open_intent(spec("a", IntentKind::Exit));
                journal.before_send(&intent, WriteClass::Bounded).await.unwrap();
                backend.push_fault(fault);
                let refusal = journal.before_send(&intent, WriteClass::Bounded).await.unwrap_err();
                assert_eq!(
                    matches!(refusal, SendRefusal::UncertainCommit(_)),
                    uncertain,
                    "{fault:?}: {refusal:?}"
                );
                // The id is sealed: nothing further even with the database back.
                let again = journal.before_send(&intent, WriteClass::Bounded).await;
                assert!(matches!(again, Err(SendRefusal::NotSent(_))), "{fault:?}");
            })
            .await;
        }
    }

    /// Rule 3: the unique id, whichever process commits it next.
    #[tokio::test]
    async fn a_spent_id_is_refused_even_on_a_bounded_write() {
        let (backend, first) = journal();
        let ((), _op) = in_op(&first, async {
            let intent = first.open_intent(spec("a", IntentKind::Exit));
            first.before_send(&intent, WriteClass::Bounded).await.unwrap();
        })
        .await;

        let later = Arc::new(
            OrderJournal::new(backend.clone(), &VenueId::new("test:venue"))
                .with_commit_bound(BOUND),
        );
        for class in [WriteClass::Bounded, WriteClass::Strict] {
            let (refusal, _op) = in_op(&later, async {
                let intent = later.open_intent(spec("a", IntentKind::Exit));
                let refusal = later.before_send(&intent, class).await;
                later.finish_failed(&intent).await;
                refusal
            })
            .await;
            assert!(matches!(refusal, Err(SendRefusal::NotSent(_))), "{class:?}: {refusal:?}");
        }
    }

    #[tokio::test]
    async fn an_id_in_use_in_this_process_is_refused() {
        let (_backend, journal) = journal();
        let ((), _op) = in_op(&journal, async {
            let first = journal.open_intent(spec("a", IntentKind::Exit));
            let duplicate = journal.open_intent(spec("a", IntentKind::Exit));
            assert!(matches!(
                journal.before_send(&duplicate, WriteClass::Bounded).await,
                Err(SendRefusal::NotSent(_))
            ));
            journal.before_send(&first, WriteClass::Bounded).await.unwrap();
        })
        .await;
    }

    /// Rule 1 at the write level: a stale replay cannot lower a rejection.
    #[tokio::test]
    async fn rejected_absorbs_started_in_either_order() {
        let (backend, journal) = journal();
        let ledger = SubmitAttemptLedger::new(8);
        let ((), _op) = in_op(&journal, async {
            let intent = journal.open_intent(spec("a", IntentKind::Exit));
            journal.before_send(&intent, WriteClass::Bounded).await.unwrap();
            let started = backend.commits()[0].clone();
            ledger.begin("a").rejected();
            journal.finish_failed(&intent).await;
            backend.commit(&started).await.unwrap();
            assert_eq!(states(&backend.row("a").unwrap()), vec![true]);
        })
        .await;
    }

    #[tokio::test]
    async fn the_ledger_reports_the_ticket_and_a_rejection_to_the_operation() {
        let (backend, journal) = journal();
        let ledger = SubmitAttemptLedger::new(8);
        let operation = journal.begin_operation();
        operation
            .scope(async {
                let intent = journal.open_intent(spec("a", IntentKind::Entry));
                journal.before_send(&intent, WriteClass::Strict).await.unwrap();
                assert!(!journal.ticket_issued(&intent), "no ticket until the send");
                ledger.begin("a").rejected();
                assert!(journal.ticket_issued(&intent));
                journal.finish_failed(&intent).await;
            })
            .await;
        assert!(operation.entry_ticket_issued());
        let row = backend.row("a").unwrap();
        assert_eq!(row.outcome.unwrap().venue_outcome, Some(VenueOutcome::Rejected));
        assert_eq!(states(&backend.row("a").unwrap()), vec![true]);
    }

    /// Finished intents leave memory; the ticket stays readable on the guard.
    #[tokio::test]
    async fn the_ticket_survives_its_intent_finishing() {
        let (_backend, journal) = journal();
        let ledger = SubmitAttemptLedger::new(8);
        let operation = journal.begin_operation();
        operation
            .scope(async {
                let intent = journal.open_intent(spec("a", IntentKind::Entry));
                journal.before_send(&intent, WriteClass::Strict).await.unwrap();
                let _ = ledger.begin("a");
                journal.finish_failed(&intent).await;
                assert!(!journal.is_live("a"));
            })
            .await;
        assert!(operation.entry_ticket_issued());
    }

    #[tokio::test]
    async fn no_ticket_after_a_commit_is_marked_not_sent() {
        let (backend, journal) = journal();
        let ((), op) = in_op(&journal, async {
            let intent = journal.open_intent(spec("a", IntentKind::Entry));
            journal.before_send(&intent, WriteClass::Strict).await.unwrap();
            journal.finish_failed(&intent).await;
        })
        .await;
        assert!(!op.entry_ticket_issued());
        assert!(backend.row("a").unwrap().outcome.unwrap().not_sent);
    }

    #[tokio::test]
    async fn a_ticket_without_a_definite_answer_is_uncertain() {
        let (backend, journal) = journal();
        let ledger = SubmitAttemptLedger::new(8);
        let ((), _op) = in_op(&journal, async {
            let intent = journal.open_intent(spec("a", IntentKind::Exit));
            journal.before_send(&intent, WriteClass::Bounded).await.unwrap();
            let _ = ledger.begin("a");
            journal.finish_failed(&intent).await;
        })
        .await;
        let outcome = backend.row("a").unwrap().outcome.unwrap();
        assert!(outcome.uncertain_at.is_some() && outcome.venue_outcome.is_none());
    }

    /// One unresolved attempt beside a rejected one is uncertain.
    #[tokio::test]
    async fn a_rejection_after_an_unresolved_attempt_stays_uncertain() {
        let (backend, journal) = journal();
        let ledger = SubmitAttemptLedger::new(8);
        let ((), _op) = in_op(&journal, async {
            let intent = journal.open_intent(spec("a", IntentKind::Exit));
            journal.before_send(&intent, WriteClass::Bounded).await.unwrap();
            let _unresolved = ledger.begin("a");
            journal.before_send(&intent, WriteClass::Bounded).await.unwrap();
            ledger.begin("a").rejected();
            journal.finish_failed(&intent).await;
        })
        .await;
        let row = backend.row("a").unwrap();
        // Only the attempt the rejection belonged to is rejected.
        assert_eq!(states(&row), vec![false, true]);
        assert_eq!(row.outcome.unwrap().venue_outcome, None);
    }

    #[tokio::test]
    async fn a_failed_result_write_goes_to_the_retry_set_and_lands_later() {
        let (backend, journal) = journal();
        let ledger = SubmitAttemptLedger::new(8);
        let ((), _op) = in_op(&journal, async {
            let intent = journal.open_intent(spec("a", IntentKind::Exit));
            journal.before_send(&intent, WriteClass::Bounded).await.unwrap();
            let _ = ledger.begin("a");
            backend.push_fault(JournalFault::Hang);
            journal.finish_success(&intent, Some(json!({"order": 1}))).await;
        })
        .await;
        assert!(backend.row("a").unwrap().outcome.is_none());
        assert_eq!(journal.retry_set_depth(), 1);
        assert!(!journal.latch_set(), "a result in the retry set is not a lost send");
        assert_eq!(journal.flush_pending().await, 1);
        assert_eq!(journal.retry_set_depth(), 0);
        assert_eq!(
            backend.row("a").unwrap().outcome.unwrap().venue_outcome,
            Some(VenueOutcome::Success)
        );
        assert!(!journal.is_live("a"));
    }

    #[tokio::test]
    async fn a_pass_stops_at_the_first_failure_and_sends_it_to_the_back() {
        let (backend, journal) = journal();
        let ((), _op) = in_op(&journal, async {
            for id in ["a", "b"] {
                backend.push_fault(JournalFault::Known);
                let intent = journal.open_intent(spec(id, IntentKind::Exit));
                journal.before_send(&intent, WriteClass::Bounded).await.unwrap();
            }
        })
        .await;
        backend.push_fault(JournalFault::Known);
        // The `unlanded_send` incident's upsert lands first (PR F); then one
        // failure ends the intents' part of the pass.
        assert_eq!(journal.flush_pending().await, 1);
        assert_eq!(backend.commits().len(), 3, "one failure ends the pass");
        // `b` goes first now, so a row that never lands cannot starve the rest.
        backend.push_fault(JournalFault::Pass);
        backend.push_fault(JournalFault::Known);
        assert_eq!(journal.flush_pending().await, 1);
        assert_eq!(backend.commits()[3].intent.client_order_id, "b");
    }

    #[tokio::test]
    async fn retry_set_overflow_sets_the_latch_and_the_id_is_never_reused() {
        let (backend, journal) = journal();
        let ledger = SubmitAttemptLedger::new(8);
        let ((), _op) = in_op(&journal, async {
            // Result writes that fail: they fill the set without latching.
            for index in 0..RETRY_SET_CAPACITY {
                let id = format!("f{index}");
                let intent = journal.open_intent(spec(&id, IntentKind::Exit));
                journal.before_send(&intent, WriteClass::Bounded).await.unwrap();
                let _ = ledger.begin(&id);
                backend.push_fault(JournalFault::Known);
                journal.finish_success(&intent, None).await;
            }
        })
        .await;
        assert_eq!(journal.retry_set_depth(), RETRY_SET_CAPACITY);
        assert!(!journal.latch_set(), "nothing has been lost yet");

        let ((), _op) = in_op(&journal, async {
            let intent = journal.open_intent(spec("over", IntentKind::Exit));
            journal.before_send(&intent, WriteClass::Bounded).await.unwrap();
            backend.push_fault(JournalFault::Known);
            journal.finish_success(&intent, None).await;
            // The set is full; the overflow's `persistence_failure` incident is
            // an upsert of its own beside it, one per reason (PR F).
            assert_eq!(journal.retry_set_depth(), RETRY_SET_CAPACITY + 1);
            assert!(journal.latch_set(), "the overflow latches");
            assert!(!journal.is_live("over"), "the discarded history is gone");
            assert!(matches!(
                journal.before_send(&intent, WriteClass::Bounded).await,
                Err(SendRefusal::NotSent(_))
            ));
        })
        .await;
    }

    /// Every slot taken, then every entry permits leave over: the registry
    /// holds them all and no more.
    #[tokio::test]
    async fn the_operations_registry_is_bounded_and_refuses_only_entries_past_the_cap() {
        let (_backend, journal) = journal();
        let kinds = std::iter::repeat_n(OperationKind::Arm, 16)
            .chain(std::iter::repeat_n(OperationKind::Panic, 8))
            .chain([
                OperationKind::MainLoop,
                OperationKind::ReconciliationScan,
                OperationKind::FundingPoll,
                OperationKind::Startup,
            ]);
        let permits: Vec<Permit> = kinds.map(|kind| journal.acquire(kind).unwrap()).collect();
        assert_eq!(permits.len(), ALLOCATED_SLOTS as usize);
        let held: Vec<Operation> =
            (0..UNPERMITTED_OPERATIONS).map(|_| journal.begin_operation()).collect();
        assert_eq!(journal.operations_in_flight(), OPERATIONS_CAPACITY);

        let extra = journal.begin_operation();
        assert_eq!(journal.operations_in_flight(), OPERATIONS_CAPACITY);
        extra
            .scope(async {
                let entry = journal.open_intent(spec("e", IntentKind::Entry));
                assert!(matches!(
                    journal.before_send(&entry, WriteClass::Strict).await,
                    Err(SendRefusal::NotSent(_))
                ));
                let exit = journal.open_intent(spec("x", IntentKind::Exit));
                assert_eq!(journal.before_send(&exit, WriteClass::Bounded).await, Ok(()));
            })
            .await;
        drop(extra);
        drop(held);
        permits.into_iter().for_each(Permit::complete);
        assert_eq!(journal.operations_in_flight(), 0);
        assert_eq!(journal.gate().snapshot().permits, 0);
    }

    /// Operations without a permit keep to the entries the permits leave
    /// over, with no permit held too: past them an entry is refused and a
    /// reduction still goes out.
    #[tokio::test]
    async fn operations_without_a_permit_list_only_in_the_unallocated_entries() {
        let (_backend, journal) = journal();
        let held: Vec<Operation> =
            (0..UNPERMITTED_OPERATIONS).map(|_| journal.begin_operation()).collect();
        assert_eq!(journal.operations_in_flight(), UNPERMITTED_OPERATIONS);

        let extra = journal.begin_operation();
        assert_eq!(journal.operations_in_flight(), UNPERMITTED_OPERATIONS);
        extra
            .scope(async {
                let entry = journal.open_intent(spec("e", IntentKind::Entry));
                assert!(matches!(
                    journal.before_send(&entry, WriteClass::Strict).await,
                    Err(SendRefusal::NotSent(_))
                ));
            })
            .await;
        drop(held);
        let listed = journal.begin_operation();
        assert_eq!(journal.operations_in_flight(), 1, "a released entry is free again");
        drop((extra, listed));
    }

    /// A future dropped mid-send: the intent ends uncertain, not forgotten.
    #[tokio::test]
    async fn an_operation_dropped_mid_send_records_its_intent_as_uncertain() {
        let (backend, journal) = journal();
        let ledger = SubmitAttemptLedger::new(8);
        let ((), op) = in_op(&journal, async {
            let intent = journal.open_intent(spec("a", IntentKind::Exit));
            journal.before_send(&intent, WriteClass::Bounded).await.unwrap();
            let _ = ledger.begin("a");
        })
        .await;
        drop(op);
        // The intent, and its `uncertain_outcome` incident's upsert (PR F).
        assert_eq!(journal.retry_set_depth(), 2);
        journal.flush_pending().await;
        let outcome = backend.row("a").unwrap().outcome.unwrap();
        assert!(outcome.uncertain_at.is_some());
        assert!(!journal.is_live("a"));
    }

    /// A cancel has no ticket, so an interrupted one is never proven unsent:
    /// it may have reached the venue before the future was dropped.
    #[tokio::test]
    async fn an_interrupted_cancel_is_uncertain_and_never_not_sent() {
        let (backend, journal) = journal();
        let operation = journal.begin_operation();
        let cancel = journal.journaled_cancel(None, "77", std::future::pending());
        let interrupted = tokio::time::timeout(BOUND, operation.scope(cancel)).await;
        assert!(interrupted.is_err(), "the cancel never answers");
        drop(operation);

        journal.flush_pending().await;
        let row = backend.rows().pop().expect("the cancel is on record");
        let outcome = row.outcome.expect("it ends with an outcome");
        assert!(!outcome.not_sent, "a cancel that may have gone out is not proven unsent");
        assert!(outcome.uncertain_at.is_some());
        assert_eq!(outcome.venue_outcome, None);
    }

    /// The result write's future is dropped while its commit is out: the
    /// accepted result is already in the retry set, counted and flushable.
    #[tokio::test]
    async fn a_dropped_result_write_is_already_in_the_retry_set() {
        let (backend, journal) = journal();
        let ledger = SubmitAttemptLedger::new(8);
        let operation = journal.begin_operation();
        let intent = operation
            .scope(async {
                let intent = journal.open_intent(spec("a", IntentKind::Exit));
                journal.before_send(&intent, WriteClass::Bounded).await.unwrap();
                let _ = ledger.begin("a");
                intent
            })
            .await;

        backend.push_fault(JournalFault::Hang);
        let write = operation.scope(journal.finish_success(&intent, Some(json!({"order": 1}))));
        let dropped = tokio::time::timeout(Duration::from_millis(1), write).await;
        assert!(dropped.is_err(), "the write is still waiting on its commit");
        drop(operation);

        assert_eq!(journal.retry_set_depth(), 1);
        assert_eq!(journal.flush_pending().await, 1);
        assert_eq!(journal.retry_set_depth(), 0);
        assert!(!journal.is_live("a"));
        let outcome = backend.row("a").unwrap().outcome.unwrap();
        assert_eq!(outcome.venue_outcome, Some(VenueOutcome::Success));
    }

    /// Fill `persistence_pending` with results that did not land, without
    /// latching.
    async fn fill_retry_set(journal: &Arc<OrderJournal>, backend: &StubJournalBackend) {
        let ledger = SubmitAttemptLedger::new(8);
        let ((), _op) = in_op(journal, async {
            for index in 0..RETRY_SET_CAPACITY {
                let id = format!("f{index}");
                let intent = journal.open_intent(spec(&id, IntentKind::Exit));
                journal.before_send(&intent, WriteClass::Bounded).await.unwrap();
                let _ = ledger.begin(&id);
                backend.push_fault(JournalFault::Known);
                journal.finish_success(&intent, None).await;
            }
        })
        .await;
        assert_eq!(journal.retry_set_depth(), RETRY_SET_CAPACITY);
    }

    /// Rule 3 after overflow: a bounded send with no row and no room in the
    /// retry set, then the same id through a new handle, database recovered.
    #[tokio::test]
    async fn a_discarded_id_stays_refused_through_a_new_handle() {
        let (backend, journal) = journal();
        fill_retry_set(&journal, &backend).await;
        let ledger = SubmitAttemptLedger::new(8);

        let ((), _op) = in_op(&journal, async {
            let first = journal.open_intent(spec("gone", IntentKind::Exit));
            backend.push_fault(JournalFault::NotLandedUncertain);
            journal.before_send(&first, WriteClass::Bounded).await.unwrap();
            let _ = ledger.begin("gone");
            backend.push_fault(JournalFault::Known);
            journal.finish_success(&first, None).await;
            assert!(!journal.is_live("gone"), "its history is discarded");
            assert!(backend.row("gone").is_none(), "it never landed");

            // Everything works again, and a brand new handle asks for the id.
            let again = journal.open_intent(spec("gone", IntentKind::Exit));
            let refusal = journal.before_send(&again, WriteClass::Bounded).await;
            assert!(matches!(refusal, Err(SendRefusal::NotSent(_))), "{refusal:?}");
            let refusal = journal.before_send(&again, WriteClass::Strict).await;
            assert!(matches!(refusal, Err(SendRefusal::NotSent(_))), "{refusal:?}");
            journal.finish_failed(&again).await;
        })
        .await;
        assert!(backend.row("gone").is_none(), "no row was made for it");
        assert!(journal.latch_set());
    }

    /// Saturation never stops a bounded first send: with both memories full it
    /// goes out, the lost id is counted, and everything else holds.
    #[tokio::test]
    async fn a_bounded_send_goes_out_with_both_memories_full() {
        let (backend, journal) = journal();
        fill_retry_set(&journal, &backend).await;
        let ((), _op) = in_op(&journal, async {
            for index in 0..DISCARDED_CAPACITY {
                let intent = journal.open_intent(spec(&format!("d{index}"), IntentKind::Exit));
                backend.push_fault(JournalFault::Known);
                journal.before_send(&intent, WriteClass::Bounded).await.unwrap();
            }
            assert_eq!(journal.non_reuse_ids_lost(), 0, "every id so far was kept");

            for fault in [
                JournalFault::Known,
                JournalFault::NotLandedUncertain,
                JournalFault::Hang,
            ] {
                let id = format!("{fault:?}");
                let saturated = journal.open_intent(spec(&id, IntentKind::Exit));
                backend.push_fault(fault);
                let sent = journal.before_send(&saturated, WriteClass::Bounded).await;
                assert_eq!(sent, Ok(()), "{fault:?}");

                // A further attempt keeps its completeness rule.
                let further = journal.before_send(&saturated, WriteClass::Bounded).await;
                assert!(matches!(further, Err(SendRefusal::NotSent(_))), "{fault:?}: {further:?}");
            }
            assert_eq!(journal.non_reuse_ids_lost(), 3);
            assert!(journal.latch_set());

            // An entry is not bounded: no row, no request, as ever.
            let entry = journal.open_intent(spec("entry", IntentKind::Entry));
            backend.push_fault(JournalFault::Known);
            let refusal = journal.before_send(&entry, WriteClass::Strict).await;
            assert!(matches!(refusal, Err(SendRefusal::NotSent(_))), "{refusal:?}");

            // A spent id is still refused.
            let spent = journal.open_intent(spec("spent", IntentKind::Exit));
            backend.push_fault(JournalFault::IdSpent);
            let refusal = journal.before_send(&spent, WriteClass::Bounded).await;
            assert!(matches!(refusal, Err(SendRefusal::NotSent(_))), "{refusal:?}");
        })
        .await;
    }

    #[tokio::test]
    async fn a_retry_pass_stops_starting_commits_after_its_bound() {
        let (backend, journal) = journal();
        let ((), _op) = in_op(&journal, async {
            for index in 0..RETRY_PASS_LIMIT {
                backend.push_fault(JournalFault::Known);
                let intent = journal.open_intent(spec(&format!("p{index}"), IntentKind::Exit));
                journal.before_send(&intent, WriteClass::Bounded).await.unwrap();
            }
        })
        .await;
        backend.set_commit_delay(BOUND / 2);

        let written = journal.flush_pending().await;

        assert!(written > 0, "it did make progress");
        assert!(written < RETRY_PASS_LIMIT, "it stopped at the pass bound: {written}");
    }

    /// The reviewed per-operation statement counts, on the counter production
    /// exports.
    #[tokio::test]
    async fn each_operation_issues_its_reviewed_statement_count() {
        let (_backend, journal) = journal();
        let ledger = SubmitAttemptLedger::new(8);

        let ((), entry) = in_op(&journal, async {
            let intent = journal.open_intent(spec("e", IntentKind::Entry));
            journal.before_send(&intent, WriteClass::Strict).await.unwrap();
            let _ = ledger.begin("e");
            journal.finish_success(&intent, None).await;
        })
        .await;
        assert_eq!(entry.journal_statements(), 9, "an order");

        let ((), retried) = in_op(&journal, async {
            let intent = journal.open_intent(spec("x", IntentKind::Exit));
            journal.before_send(&intent, WriteClass::Bounded).await.unwrap();
            journal.before_send(&intent, WriteClass::Bounded).await.unwrap();
            journal.finish_failed(&intent).await;
        })
        .await;
        assert_eq!(retried.journal_statements(), 13, "one quantisation retry");

        let (_, cancel) =
            in_op(&journal, journal.journaled_cancel(None, "77", async { Ok(cancelled()) })).await;
        assert_eq!(cancel.journal_statements(), 9, "a cancel");
    }

    #[tokio::test]
    async fn an_operation_past_its_bound_is_counted() {
        let (_backend, journal) = journal();
        let ((), operation) = in_op(&journal, async {
            for index in 0..5 {
                let intent = journal.open_intent(spec(&format!("o{index}"), IntentKind::Exit));
                journal.before_send(&intent, WriteClass::Bounded).await.unwrap();
                journal.finish_failed(&intent).await;
            }
        })
        .await;
        assert_eq!(operation.journal_statements(), 45);
        assert_eq!(journal.operations_over_statement_bound(), 0, "counted when it ends");
        drop(operation);
        assert_eq!(journal.operations_over_statement_bound(), 1);
        assert_eq!(journal.operation_statements_max(), 45);
    }

    /// A refused retry with one request out may be replaced, once; the refused
    /// id stays closed.
    #[tokio::test]
    async fn a_refused_retry_is_replaced_once_and_its_id_stays_closed() {
        let (backend, journal) = journal();
        let ledger = SubmitAttemptLedger::new(8);
        let ((), _op) = in_op(&journal, async {
            let intent = journal.open_intent(spec("a", IntentKind::Exit));
            journal.before_send(&intent, WriteClass::Bounded).await.unwrap();
            let _ = ledger.begin("a");
            backend.push_fault(JournalFault::Known);
            journal.before_send(&intent, WriteClass::Bounded).await.unwrap_err();

            let fresh = journal
                .replace_refused_retry(&intent, "b".to_string(), "rules".to_string())
                .await
                .unwrap();
            assert_eq!(fresh.client_order_id(), "b");
            let again = journal
                .replace_refused_retry(&intent, "c".to_string(), "rules".to_string())
                .await;
            assert!(again.is_err(), "one fallback only: {again:?}");
            assert!(journal.before_send(&intent, WriteClass::Bounded).await.is_err());

            let _ = ledger.begin("b");
            // The original's ending fails to land at completion: it stays in
            // memory, in the retry set and unlinked, and is still not
            // replaceable.
            backend.push_fault(JournalFault::Known);
            journal.finish_success(&intent, None).await;
            assert!(journal.lock().intents["a"].replaced_by.is_none(), "the link is consumed");
            let after = journal
                .replace_refused_retry(&intent, "d".to_string(), "rules".to_string())
                .await;
            assert!(after.is_err(), "none after its completion either: {after:?}");
            journal.flush_pending().await;
        })
        .await;
        let sealed = backend.row("a").unwrap();
        assert!(
            sealed.outcome.unwrap().uncertain_at.is_some(),
            "the refused id is not a success"
        );
        let fresh = backend.row("b").unwrap();
        assert_eq!(fresh.outcome.unwrap().venue_outcome, Some(VenueOutcome::Success));
        assert_eq!(fresh.intent.unwrap().params["replaces_client_order_id"], "a");
    }

    /// Nothing refused, or a retry that did leave: no replacement.
    #[tokio::test]
    async fn a_retry_that_was_not_refused_or_did_leave_cannot_be_replaced() {
        let (backend, journal) = journal();
        let ledger = SubmitAttemptLedger::new(8);
        let ((), _op) = in_op(&journal, async {
            let accepted = journal.open_intent(spec("a", IntentKind::Exit));
            journal.before_send(&accepted, WriteClass::Bounded).await.unwrap();
            let _ = ledger.begin("a");
            let not_refused =
                journal.replace_refused_retry(&accepted, "b".to_string(), "r".to_string()).await;
            assert!(not_refused.is_err(), "{not_refused:?}");

            // Two requests left under this id, then a third was refused.
            let twice = journal.open_intent(spec("t", IntentKind::Exit));
            journal.before_send(&twice, WriteClass::Bounded).await.unwrap();
            let _ = ledger.begin("t");
            journal.before_send(&twice, WriteClass::Bounded).await.unwrap();
            let _ = ledger.begin("t");
            backend.push_fault(JournalFault::Known);
            journal.before_send(&twice, WriteClass::Bounded).await.unwrap_err();
            let out_twice =
                journal.replace_refused_retry(&twice, "u".to_string(), "r".to_string()).await;
            assert!(out_twice.is_err(), "{out_twice:?}");
        })
        .await;
    }

    /// Memory entries with no owner: neither waiting in the retry set nor
    /// held by a live operation.
    fn unowned(journal: &OrderJournal) -> Vec<String> {
        let state = journal.lock();
        let mut ids: Vec<String> = state
            .intents
            .iter()
            .filter(|(_, memory)| {
                !memory.in_retry_set && !state.operations.contains_key(&memory.operation)
            })
            .map(|(id, _)| id.clone())
            .collect();
        ids.sort();
        ids
    }

    /// Review round 2: a fallback whose operation is cancelled while the
    /// replacement waits at the venue, before its completion consumes the
    /// link. With the retry set full the original's uncertain ending
    /// overflowed, so it has an outcome and no retry-set entry, and only its
    /// link kept it; with room, its ending landed. Either way the operation's
    /// end consumes the link and collects it, the abandoned replacement is
    /// recorded as uncertain like any other, nothing is left without an
    /// owner, and once the retry set drains, memory is empty.
    #[tokio::test]
    async fn a_cancelled_fallback_leaves_nothing_unowned() {
        for full in [true, false] {
            let (backend, journal) = journal();
            let ledger = SubmitAttemptLedger::new(8);
            if full {
                backend.set_fail_all(Some(JournalFault::Known));
                let ((), filling) = in_op(&journal, async {
                    for index in 0..RETRY_SET_CAPACITY {
                        let intent = journal
                            .open_intent(spec(&format!("filling-{index}"), IntentKind::Exit));
                        journal.before_send(&intent, WriteClass::Bounded).await.unwrap();
                    }
                })
                .await;
                drop(filling);
                backend.set_fail_all(None);
                assert_eq!(journal.lock().retry_set.len(), RETRY_SET_CAPACITY);
            }

            let operation = journal.begin_operation();
            let reduction = operation.scope(async {
                let intent = journal.open_intent(spec("a", IntentKind::Exit));
                journal.before_send(&intent, WriteClass::Bounded).await.unwrap();
                let _ = ledger.begin("a");
                backend.push_fault(JournalFault::NotLandedUncertain);
                journal.before_send(&intent, WriteClass::Bounded).await.unwrap_err();
                journal
                    .replace_refused_retry(&intent, "b".to_string(), "rules".to_string())
                    .await
                    .unwrap();
                let _ = ledger.begin("b");
                if !full {
                    journal.flush_pending().await;
                    assert!(backend.row("a").unwrap().outcome.is_some(), "its ending landed");
                }
                // At the venue, never answering: cancelled from outside.
                std::future::pending::<()>().await;
            });
            let cancelled = tokio::time::timeout(Duration::from_millis(50), reduction).await;
            assert!(cancelled.is_err());
            {
                let state = journal.lock();
                let original = &state.intents["a"];
                assert!(original.replaced_by.is_some() && original.outcome.is_some());
                assert!(!original.in_retry_set, "full {full}: overflowed or landed");
            }
            drop(operation);

            {
                let state = journal.lock();
                assert!(!state.intents.contains_key("a"), "full {full}: the original is collected");
                if full {
                    assert!(!state.intents.contains_key("b"), "the replacement overflowed too");
                } else {
                    let fresh = &state.intents["b"];
                    assert!(fresh.outcome.as_ref().is_some_and(OutcomeRow::is_uncertain));
                    assert!(fresh.in_retry_set);
                }
                let expected = if full { RETRY_SET_CAPACITY } else { 1 };
                assert_eq!(state.intents.len(), expected, "full {full}: within the bound");
            }
            assert_eq!(unowned(&journal), Vec::<String>::new(), "full {full}");
            for _ in 0..RETRY_SET_CAPACITY {
                if journal.retry_set_depth() == 0 {
                    break;
                }
                journal.flush_pending().await;
            }
            assert!(journal.lock().intents.is_empty(), "full {full}: memory returns to empty");
            assert_eq!(journal.retry_set_depth(), 0, "full {full}");
        }
    }

    /// Review round 3: the replacement is accepted while the original's
    /// uncertain ending, registered at the fallback, has not landed. Its
    /// completion is cancelled while that ending's write is held, and the
    /// operation ends. The acceptance was registered before anything was
    /// awaited, so draining writes the replacement's success with its
    /// evidence: no uncertainty for it, no new occurrence, the fence as it
    /// was.
    #[tokio::test]
    async fn a_replacement_accepted_keeps_its_success_when_completion_is_cancelled() {
        let (backend, journal) = journal();
        let ledger = SubmitAttemptLedger::new(8);
        let evidence = json!({"orderId": 7, "status": "FILLED"});
        let operation = journal.begin_operation();
        let intent = operation
            .scope(async {
                let intent = journal.open_intent(spec("a", IntentKind::Exit));
                journal.before_send(&intent, WriteClass::Bounded).await.unwrap();
                let _ = ledger.begin("a");
                backend.push_fault(JournalFault::NotLandedUncertain);
                journal.before_send(&intent, WriteClass::Bounded).await.unwrap_err();
                journal
                    .replace_refused_retry(&intent, "b".to_string(), "rules".to_string())
                    .await
                    .unwrap();
                let _ = ledger.begin("b");
                intent
            })
            .await;
        assert!(journal.lock().intents["a"].has_unlanded(), "the original's ending is unflushed");
        let before = (journal.gate().fenced(), journal.gate().generation(), reasons(&journal));
        assert!(before.0, "fenced by the original");
        let counted = journal.gate().open_incidents()[0].count;

        let held = backend.hold_commit(backend.commits().len());
        let completion = operation.scope(journal.finish_success(&intent, Some(evidence.clone())));
        let cancelled = tokio::time::timeout(Duration::from_millis(20), completion).await;
        assert!(cancelled.is_err(), "cancelled with its first write held");
        drop(operation);
        held.notify_one();

        let after = (journal.gate().fenced(), journal.gate().generation(), reasons(&journal));
        assert_eq!(after, before, "no new occurrence, the fence as it was");
        assert_eq!(journal.gate().open_incidents()[0].count, counted);
        for _ in 0..8 {
            if journal.retry_set_depth() == 0 {
                break;
            }
            journal.flush_pending().await;
        }
        assert_eq!(journal.retry_set_depth(), 0);
        let fresh = backend.row("b").unwrap().outcome.expect("the acceptance is on record");
        assert_eq!(fresh.venue_outcome, Some(VenueOutcome::Success));
        assert_eq!(fresh.venue_evidence, Some(evidence));
        assert!(fresh.uncertain_at.is_none() && fresh.uncertain_generation.is_none());
        let original = backend.row("a").unwrap().outcome.expect("the original's ending");
        assert!(original.uncertain_at.is_some());
        assert_eq!(journal.gate().generation(), before.1, "draining added nothing");
        assert!(journal.lock().intents.is_empty(), "the link went with the operation");
        assert_eq!(unowned(&journal), Vec::<String>::new());
    }

    // Rule 4 ------------------------------------------------------------------

    #[tokio::test]
    async fn no_row_reads_no_record_and_a_read_failure_concludes_nothing() {
        let (backend, journal) = journal();
        assert_eq!(journal.attempt_evidence("a").await, AttemptEvidence::NoRecord);
        backend.set_read_fails(true);
        assert_eq!(journal.attempt_evidence("a").await, AttemptEvidence::Unavailable);
    }

    /// Runs `during_read` while the attempt rows are being read.
    struct DuringRead {
        real: Arc<StubJournalBackend>,
        during_read: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    }

    #[async_trait]
    impl JournalBackend for DuringRead {
        async fn commit(&self, batch: &JournalBatch) -> Result<(), CommitFailure> {
            self.real.commit(batch).await
        }

        async fn record_suspension(
            &self,
            suspension: &StreamSuspensionRow,
        ) -> Result<(), CommitFailure> {
            self.real.record_suspension(suspension).await
        }

        async fn upsert_incident(&self, incident: &Incident) -> Result<(), CommitFailure> {
            self.real.upsert_incident(incident).await
        }

        async fn release(&self, release: &ReleaseRecord) -> Result<ReleaseCounts, String> {
            self.real.release(release).await
        }

        async fn attempt_states(&self, v: &str, s: &str, id: &str) -> Result<Vec<bool>, String> {
            let states = self.real.attempt_states(v, s, id).await;
            if let Some(during_read) = self.during_read.lock().unwrap().take() {
                during_read();
            }
            states
        }
    }

    fn journal_with_read_hook() -> (Arc<DuringRead>, Arc<OrderJournal>) {
        let backend = Arc::new(DuringRead {
            real: Arc::new(StubJournalBackend::default()),
            during_read: Mutex::new(None),
        });
        let journal = OrderJournal::new(backend.clone(), &VenueId::new("test:venue"));
        (backend, Arc::new(journal.with_commit_bound(BOUND)))
    }

    /// An attempt that starts during the read is seen after it.
    #[tokio::test]
    async fn an_attempt_starting_during_the_read_blocks_absent() {
        let (backend, journal) = journal_with_read_hook();
        let during = Arc::clone(&journal);
        let registered = during.begin_operation();
        let (opened, opener) = (Arc::new(Mutex::new(None)), Arc::clone(&journal));
        let slot = Arc::clone(&opened);
        *backend.during_read.lock().unwrap() = Some(Box::new(move || {
            *slot.lock().unwrap() = Some(opener.open_intent(spec("a", IntentKind::Exit)));
        }));
        assert!(matches!(
            journal.attempt_evidence("a").await,
            AttemptEvidence::Unresolved { .. }
        ));
        drop(registered);
    }

    /// The memory check before the read: rows that already read as all
    /// rejected say nothing while the intent is still live here, even if it
    /// leaves memory during the read.
    #[tokio::test]
    async fn an_intent_live_when_the_read_starts_blocks_absent() {
        let (backend, journal) = journal_with_read_hook();
        let ((), _op) = in_op(&journal, async {
            let intent = journal.open_intent(spec("a", IntentKind::Exit));
            journal.before_send(&intent, WriteClass::Bounded).await.unwrap();
            // The durable row reads rejected; the intent is not finished.
            let mut rejected = backend.real.commits().pop().unwrap();
            rejected.attempts[0].rejected_at = Some(Utc::now());
            backend.real.commit(&rejected).await.unwrap();

            let finisher = Arc::clone(&journal);
            *backend.during_read.lock().unwrap() = Some(Box::new(move || {
                finisher.lock().intents.remove("a");
            }));
            assert!(matches!(
                journal.attempt_evidence("a").await,
                AttemptEvidence::Unresolved { .. }
            ));
        })
        .await;
    }

    #[tokio::test]
    async fn an_attempt_known_only_in_memory_blocks_absent() {
        let (backend, journal) = journal();
        let ledger = SubmitAttemptLedger::new(8);
        let ((), _op) = in_op(&journal, async {
            let intent = journal.open_intent(spec("a", IntentKind::Exit));
            journal.before_send(&intent, WriteClass::Bounded).await.unwrap();
            ledger.begin("a").rejected();
            journal.finish_failed(&intent).await;
            assert!(matches!(journal.attempt_evidence("a").await, AttemptEvidence::AllRejected {
                rejected: 1
            }));

            // Unlanded rejection on another id: durable says started.
            let other = journal.open_intent(spec("b", IntentKind::Exit));
            journal.before_send(&other, WriteClass::Bounded).await.unwrap();
            ledger.begin("b").rejected();
            backend.push_fault(JournalFault::Known);
            journal.finish_failed(&other).await;
            assert!(matches!(
                journal.attempt_evidence("b").await,
                AttemptEvidence::Unresolved { .. }
            ));
        })
        .await;
    }

    /// What decides `Absent` when the journal is attached: the rows, not the
    /// local count.
    #[tokio::test]
    async fn an_attached_ledger_answers_from_the_durable_rows() {
        let (backend, journal) = journal();
        let ledger = SubmitAttemptLedger::new(8);
        ledger.attach_journal(journal.clone());
        // Local evidence of an all-rejected id with no row: not good enough.
        ledger.begin("no-row").rejected();
        assert_eq!(ledger.final_evidence("no-row").await, AttemptEvidence::NoRecord);

        let ((), _op) = in_op(&journal, async {
            let intent = journal.open_intent(spec("a", IntentKind::Exit));
            journal.before_send(&intent, WriteClass::Bounded).await.unwrap();
            ledger.begin("a").rejected();
            journal.finish_failed(&intent).await;
        })
        .await;
        assert!(matches!(ledger.final_evidence("a").await, AttemptEvidence::AllRejected {
            rejected: 1
        }));
        // An unresolved attempt seen only by the ledger keeps it unresolved.
        let _unresolved = ledger.begin("a");
        assert!(matches!(ledger.final_evidence("a").await, AttemptEvidence::Unresolved { .. }));
        backend.set_read_fails(true);
        assert_eq!(ledger.final_evidence("a").await, AttemptEvidence::Unavailable);
    }

    fn cancelled() -> StopCancelOutcome {
        StopCancelOutcome::Cancelled {
            filled_before_cancel: rust_decimal::Decimal::ZERO,
            fills: None,
        }
    }

    fn gone(final_state: crate::venue::OrderLookup) -> StopCancelOutcome {
        StopCancelOutcome::AlreadyGone { final_state }
    }

    fn terminal(filled_qty: rust_decimal::Decimal) -> crate::venue::OrderLookup {
        crate::venue::OrderLookup::Terminal {
            filled_qty,
            cancelled_qty: rust_decimal::Decimal::ZERO,
            fills: None,
        }
    }

    /// The row each cancel input leaves: the class on the row, the outcome a
    /// definite class writes, and `uncertain_at` for every other one.
    #[tokio::test]
    async fn a_cancel_row_carries_its_class() {
        use crate::venue::OrderLookup;
        let cases: Vec<(Result<StopCancelOutcome, ExecError>, &str, Option<VenueOutcome>)> = vec![
            (Ok(cancelled()), "cancelled", Some(VenueOutcome::Success)),
            (
                Ok(gone(terminal(rust_decimal::Decimal::ONE))),
                "gone_terminal",
                Some(VenueOutcome::GoneTerminal),
            ),
            (Ok(gone(OrderLookup::Unknown)), "gone_unresolved", None),
            (
                Ok(gone(OrderLookup::Open { filled_qty: rust_decimal::Decimal::ZERO })),
                "gone_unresolved",
                None,
            ),
            (Ok(StopCancelOutcome::Indeterminate), "indeterminate", None),
            (Err(ExecError::Timeout("t".to_string())), "error", None),
            (Err(ExecError::OrderRejected("Unknown order sent.".to_string())), "error", None),
        ];
        for (result, label, venue_outcome) in cases {
            let (backend, journal) = journal();
            let (class, _op) =
                in_op(&journal, journal.journaled_cancel(None, "77", async { result })).await;
            assert_eq!(class.label(), label);
            let row = backend.rows().pop().unwrap();
            assert_eq!(row.intent.as_ref().unwrap().cancels_order_id.as_deref(), Some("77"));
            assert_eq!(row.attempts.len(), 1);
            let outcome = row.outcome.unwrap();
            assert_eq!(outcome.cancel_class, Some(label));
            assert_eq!(outcome.venue_outcome, venue_outcome, "{label}");
            assert_eq!(outcome.uncertain_at.is_some(), venue_outcome.is_none(), "{label}");
            assert!(!outcome.not_sent, "{label}");
        }
    }

    /// A cancel that found the order gone with its final state keeps the
    /// fills on its own intent; nothing else writes them there.
    #[tokio::test]
    async fn a_gone_terminal_cancel_writes_its_fills_as_evidence() {
        let (backend, journal) = journal();
        let (class, _op) = in_op(
            &journal,
            journal.journaled_cancel(None, "77", async {
                Ok(gone(terminal(rust_decimal_macros::dec!(0.25))))
            }),
        )
        .await;
        assert_eq!(class.label(), "gone_terminal");
        let evidence = backend.rows().pop().unwrap().outcome.unwrap().venue_evidence;
        let evidence = evidence.expect("the terminal state is on the record");
        assert_eq!(evidence["filled_qty"], serde_json::json!(rust_decimal_macros::dec!(0.25)));
        assert_eq!(evidence["fills"], Value::Null);
    }

    #[tokio::test]
    async fn a_cancel_goes_out_when_the_journal_cannot_take_it() {
        let (backend, journal) = journal();
        backend.push_fault(JournalFault::Known);
        let (class, _op) =
            in_op(&journal, journal.journaled_cancel(None, "77", async { Ok(cancelled()) })).await;
        assert!(matches!(class, CancelClass::Cancelled { .. }));
        assert!(journal.latch_set());
    }
    // ---- Permits (PR S) --------------------------------------------------

    /// A permit dropped before it completed: aborted, latched, its slot back.
    #[tokio::test]
    async fn a_permit_dropped_before_completing_is_aborted_and_sets_the_latch() {
        let (_backend, journal) = journal();
        drop(journal.acquire(OperationKind::Panic).unwrap());
        let gate = journal.gate().snapshot();
        assert_eq!((gate.aborted, gate.permits, gate.protective), (1, 0, 0));
        assert!(journal.latch_set());
        assert_eq!(journal.operations_in_flight(), 0);
    }

    /// An operation that returned, with an error or not, has completed.
    #[tokio::test]
    async fn a_permit_whose_work_returned_an_error_has_completed() {
        let (_backend, journal) = journal();
        let permit = journal.acquire(OperationKind::Arm).unwrap();
        let failed: Result<(), &str> = permit.run(async { Err("refused") }).await;
        assert!(failed.is_err());
        assert_eq!(journal.gate().snapshot().aborted, 0);
        assert!(!journal.latch_set());
        assert_eq!(journal.gate().snapshot().permits, 0);
    }

    /// Cancelled mid-send after its attempt row landed: still aborted, and its
    /// intent ends uncertain through the retry set.
    #[tokio::test]
    async fn a_permit_cancelled_after_its_row_landed_is_aborted_and_its_intent_uncertain() {
        let (backend, journal) = journal();
        let ledger = SubmitAttemptLedger::new(8);
        let permit = journal.acquire(OperationKind::CancelOrClose).unwrap();
        let work = permit.run(async {
            let intent = journal.open_intent(spec("x", IntentKind::Exit));
            journal.before_send(&intent, WriteClass::Bounded).await.unwrap();
            let _attempt = ledger.begin_send("x").unwrap();
            std::future::pending::<()>().await;
        });
        assert!(tokio::time::timeout(BOUND, work).await.is_err());

        let gate = journal.gate().snapshot();
        assert_eq!((gate.aborted, gate.permits), (1, 0));
        assert!(journal.latch_set());
        assert_eq!(backend.row("x").unwrap().attempts.len(), 1, "its row had landed");
        // The intent, and two incidents: the abort and the uncertain outcome.
        assert_eq!(journal.retry_set_depth(), 3);
        journal.flush_pending().await;
        let outcome = backend.row("x").unwrap().outcome.unwrap();
        assert!(outcome.uncertain_at.is_some() && !outcome.not_sent, "{outcome:?}");
    }

    /// Nested work runs under the permit it is reached from: no second slot
    /// and no other class, even with that class full.
    #[tokio::test]
    async fn nested_work_reuses_the_held_permit() {
        let (_backend, journal) = journal();
        let ordinary: Vec<Permit> = (0..crate::gate::ORDINARY_API_SLOTS)
            .map(|_| journal.acquire(OperationKind::Arm).unwrap())
            .collect();
        let permit = journal.acquire(OperationKind::ReconcileClose).unwrap();
        let nested = permit
            .run(async {
                journal
                    .with_permit(OperationKind::Panic, async {
                        journal
                            .with_permit(OperationKind::Arm, async {
                                (held_permit(), journal.gate().snapshot())
                            })
                            .await
                    })
                    .await
            })
            .await;
        let (held, gate) = nested.unwrap().unwrap();
        assert_eq!(held, Some(OperationKind::ReconcileClose));
        assert_eq!(gate.protective, 1);
        assert_eq!(gate.permits, crate::gate::ORDINARY_API_SLOTS + 1);
        ordinary.into_iter().for_each(Permit::complete);
    }

    /// The send start: once the daemon is not open an entry is refused, with
    /// no ticket and no attempt; anything else still starts and is ticketed.
    /// The refused entry ends as a proven no-send.
    #[tokio::test]
    async fn the_send_start_refuses_an_entry_after_open_and_tickets_the_rest() {
        let (backend, journal) = journal();
        let ledger = SubmitAttemptLedger::new(8);
        let permit = journal.acquire(OperationKind::MainLoop).unwrap();
        permit
            .run(async {
                let open = journal.open_intent(spec("e-open", IntentKind::Entry));
                journal.before_send(&open, WriteClass::Strict).await.unwrap();
                ledger.begin_send("e-open").unwrap();
                assert!(journal.ticket_issued(&open));
                journal.finish_success(&open, None).await;

                journal.gate().begin_draining();
                let refused = journal.open_intent(spec("e-draining", IntentKind::Entry));
                journal.before_send(&refused, WriteClass::Strict).await.unwrap();
                assert!(matches!(ledger.begin_send("e-draining"), Err(SendRefusal::NotSent(_))));
                assert!(!journal.ticket_issued(&refused));
                assert_eq!(ledger.evidence("e-draining"), AttemptEvidence::NoRecord);
                journal.finish_failed(&refused).await;

                journal.gate().begin_closing();
                let exit = journal.open_intent(spec("x", IntentKind::Exit));
                journal.before_send(&exit, WriteClass::Bounded).await.unwrap();
                ledger.begin_send("x").unwrap();
                assert!(journal.ticket_issued(&exit));
                journal.finish_success(&exit, None).await;
                // A send with no intent here, a funding order, still starts.
                ledger.begin_send("funding-1").unwrap();
            })
            .await;

        assert_eq!(journal.gate().snapshot().sends_started, 3);
        assert!(backend.row("e-draining").unwrap().outcome.unwrap().not_sent);
    }

    /// From PR P the latch closes admissions: an entry past the manager's
    /// checks is refused at its send, a typed no-send with no ticket; a
    /// reduction still goes out.
    #[tokio::test]
    async fn the_latch_refuses_an_entry_at_its_send_and_nothing_else() {
        let (backend, journal) = journal();
        let ledger = SubmitAttemptLedger::new(8);
        let permit = journal.acquire(OperationKind::MainLoop).unwrap();
        permit
            .run(async {
                journal.record_suspension(suspension("position:a"));
                assert!(journal.latch_set());
                let entry = journal.open_intent(spec("e-latched", IntentKind::Entry));
                journal.before_send(&entry, WriteClass::Strict).await.unwrap();
                let refused = ledger.begin_send("e-latched").err();
                assert!(
                    matches!(&refused, Some(SendRefusal::NotSent(reason)) if reason.contains("latch")),
                    "{refused:?}"
                );
                assert!(!journal.ticket_issued(&entry));
                journal.finish_failed(&entry).await;

                let exit = journal.open_intent(spec("x-latched", IntentKind::Exit));
                journal.before_send(&exit, WriteClass::Bounded).await.unwrap();
                ledger.begin_send("x-latched").unwrap();
                assert!(journal.ticket_issued(&exit));
                journal.finish_success(&exit, None).await;
            })
            .await;
        assert!(backend.row("e-latched").unwrap().outcome.unwrap().not_sent);
    }

    fn suspension(stream_key: &str) -> StreamSuspensionRow {
        StreamSuspensionRow {
            suspension_id: Uuid::now_v7(),
            stream_key: stream_key.to_string(),
            stream_kind: "position",
            failed_seq: None,
            month: None,
            error: "injected".to_string(),
            epoch_id: None,
            suspended_at: Utc::now(),
        }
    }

    /// A stream suspension goes through the retry set: the latch at once, the
    /// row by the retry loop, retried until it lands, recorded once, and the
    /// journal is not quiescent meanwhile.
    #[tokio::test]
    async fn a_stream_suspension_is_recorded_through_the_retry_set() {
        let (backend, journal) = journal();
        journal.record_suspension(suspension("position:a"));
        journal.record_suspension(suspension("position:a"));
        assert!(journal.latch_set());
        // One per stream, and one `stream_suspended` incident (PR F).
        assert_eq!(journal.retry_set_depth(), 2, "one per stream");
        assert!(!journal.is_quiescent());

        backend.push_fault(JournalFault::Known);
        // The incident's upsert lands; the suspension's write fails.
        assert_eq!(journal.flush_pending().await, 1);
        assert_eq!(journal.retry_set_depth(), 1, "kept for the next pass");
        assert!(backend.suspensions().is_empty());
        assert_eq!(backend.incidents().len(), 1);

        backend.push_fault(JournalFault::AckLost);
        journal.flush_pending().await;
        assert_eq!(journal.retry_set_depth(), 1, "an uncertain write is retried");
        assert_eq!(journal.flush_pending().await, 1);
        assert_eq!(journal.retry_set_depth(), 0);
        assert!(journal.is_quiescent());
        let recorded = backend.suspensions();
        assert_eq!(recorded.len(), 1, "a replay converges on the first");
        assert_eq!(recorded[0].stream_key, "position:a");
    }

    /// A suspension whose write keeps failing, while intents' commits would
    /// land, never keeps an intent's rows from being tried: every pass still
    /// reaches them.
    #[tokio::test]
    async fn a_failing_suspension_write_does_not_starve_the_intents() {
        let (backend, journal) = journal();
        let ledger = SubmitAttemptLedger::new(8);
        let ((), _op) = in_op(&journal, async {
            let intent = journal.open_intent(spec("x-pending", IntentKind::Exit));
            journal.before_send(&intent, WriteClass::Bounded).await.unwrap();
            let _ = ledger.begin("x-pending");
            backend.push_fault(JournalFault::Known);
            journal.finish_success(&intent, None).await;
        })
        .await;
        backend.set_suspension_writes_fail(true);
        journal.record_suspension(suspension("position:refused"));
        // The suspension, the intent and the `stream_suspended` incident.
        assert_eq!(journal.retry_set_depth(), 3);

        // The intent lands, and so does the incident's upsert.
        assert_eq!(journal.flush_pending().await, 2, "the intent lands");
        assert_eq!(journal.flush_pending().await, 0);
        assert_eq!(journal.retry_set_depth(), 1, "the suspension waits, and only it");
        let landed = backend.row("x-pending").unwrap().outcome.unwrap();
        assert_eq!(landed.venue_outcome, Some(VenueOutcome::Success));
        assert!(!journal.is_quiescent());

        backend.set_suspension_writes_fail(false);
        assert_eq!(journal.flush_pending().await, 1);
        assert!(journal.is_quiescent());
    }

    /// A full retry set keeps no suspension, and the latch is set all the same.
    #[tokio::test]
    async fn a_suspension_past_a_full_retry_set_still_sets_the_latch() {
        let (backend, journal) = journal();
        fill_retry_set(&journal, &backend).await;
        journal.record_suspension(suspension("position:full"));
        assert!(journal.latch_set());
        // Full, plus the two incidents' upserts: the suspension and its
        // discarded record (PR F).
        assert_eq!(journal.retry_set_depth(), RETRY_SET_CAPACITY + 2);
        let reasons: Vec<_> = journal.gate().open_incidents().iter().map(|i| i.reason).collect();
        assert_eq!(reasons, [
            IncidentReason::PersistenceFailure,
            IncidentReason::StreamSuspended
        ]);
    }

    /// An entry and its stop under one permit are one operation: both are in
    /// its registry entry, the admission reads its own entry's ticket, and the
    /// statements are charged to the whole operation.
    #[tokio::test]
    async fn an_entry_and_its_stop_are_one_operation_under_a_permit() {
        let (_backend, journal) = journal();
        let ledger = SubmitAttemptLedger::new(8);
        let permit = journal.acquire(OperationKind::Arm).unwrap();
        permit
            .run(async {
                let ((), ticket) = journal
                    .run_admission(Some("e"), async {
                        let entry = journal.open_intent(spec("e", IntentKind::Entry));
                        journal.before_send(&entry, WriteClass::Strict).await.unwrap();
                        ledger.begin_send("e").unwrap();
                        journal.finish_success(&entry, None).await;
                    })
                    .await;
                assert!(ticket);
                let stop = journal.open_intent(spec("ins-1", IntentKind::Protection));
                journal.before_send(&stop, WriteClass::Bounded).await.unwrap();
                ledger.begin_send("ins-1").unwrap();
                journal.finish_success(&stop, None).await;
                assert_eq!(journal.operations_in_flight(), 1);
                let ((), other) = journal.run_admission(Some("e2"), async {}).await;
                assert!(!other, "another entry's ticket is not this one's");
            })
            .await;
        assert_eq!(journal.operation_statements_max(), 18, "entry 9 and stop 9");
    }

    /// The choke point knows a permit by its gate: one from another journal,
    /// or an operation without one, is unpermitted here.
    #[tokio::test]
    async fn only_a_permit_of_this_gate_counts_at_its_choke_point() {
        let (_mine_backend, mine) = journal();
        let (_other_backend, other) = journal();
        other
            .acquire(OperationKind::Panic)
            .unwrap()
            .run(async {
                mine.gate().mutation("POST /x").unwrap();
                other.gate().mutation("POST /x").unwrap();
            })
            .await;
        mine.begin_operation()
            .scope(async { mine.gate().mutation("POST /x").unwrap() })
            .await;
        assert_eq!(mine.gate().snapshot().unpermitted, 2);
        assert_eq!(other.gate().snapshot().unpermitted, 0);
    }

    // =========================================================================
    // PR F: occurrences, the fence, the generation and the audited release
    // =========================================================================

    use crate::gate::{EpochView, Incident, IncidentReason, GENERATION_COUNTER_LIMIT};

    /// An exit that was sent and has no definite answer: uncertain.
    async fn uncertain_exit(journal: &Arc<OrderJournal>, id: &str) {
        let ledger = SubmitAttemptLedger::new(8);
        let ((), _op) = in_op(journal, async {
            let intent = journal.open_intent(spec(id, IntentKind::Exit));
            journal.before_send(&intent, WriteClass::Bounded).await.unwrap();
            let _ = ledger.begin(id);
            journal.finish_failed(&intent).await;
        })
        .await;
    }

    fn reasons(journal: &OrderJournal) -> Vec<IncidentReason> {
        journal.gate().open_incidents().iter().map(|incident| incident.reason).collect()
    }

    fn release_of(generation: i64) -> ReleaseRecord {
        ReleaseRecord {
            decision_id: Uuid::now_v7(),
            generation,
            actor: "operator@example.com".to_string(),
            auth_kind: "verified",
            note: "reviewed".to_string(),
            treatment: "unbooked_accepted",
            decided_at: Utc::now(),
        }
    }

    fn epoch(journal: &OrderJournal, epoch_seq: i64) {
        let view = EpochView { epoch_seq, unclean_reason: None };
        journal.gate().begin_epoch(view, None, Vec::new());
    }

    /// Design §2.2: an uncertain outcome closes the fence before anything is
    /// awaited. The outcome's write here never answers, yet after its first
    /// poll the fence is closed, the generation moved and the intent stamped
    /// with it, and an entry reaching its send is refused.
    #[tokio::test]
    async fn an_uncertain_outcome_fences_before_its_write_is_awaited() {
        let (backend, journal) = journal();
        epoch(&journal, 7);
        let ledger = SubmitAttemptLedger::new(8);
        let ((), _op) = in_op(&journal, async {
            let intent = journal.open_intent(spec("x", IntentKind::Exit));
            journal.before_send(&intent, WriteClass::Bounded).await.unwrap();
            let _ = ledger.begin("x");
            assert!(!journal.gate().fenced());

            backend.set_fail_all(Some(JournalFault::Hang));
            let mut write = Box::pin(journal.finish_failed(&intent));
            // One poll: the write is now waiting on its commit, and nothing
            // before that wait yielded.
            assert!(tokio::time::timeout(Duration::ZERO, &mut write).await.is_err());
            assert!(journal.gate().fenced(), "the fence closed before the await");
            assert_eq!(journal.gate().generation(), (7 << 32) | 1);
            assert_eq!(reasons(&journal), [IncidentReason::UncertainOutcome]);
            assert!(!journal.latch_set(), "not a latch-class reason");

            let entry = journal.open_intent(spec("e", IntentKind::Entry));
            backend.set_fail_all(None);
            journal.before_send(&entry, WriteClass::Strict).await.unwrap();
            let refused = ledger.begin_send("e").err();
            assert!(
                matches!(&refused, Some(SendRefusal::NotSent(why)) if why.contains("fenced")),
                "{refused:?}"
            );
            // A reduction still goes out under the fence.
            let exit = journal.open_intent(spec("x2", IntentKind::Exit));
            journal.before_send(&exit, WriteClass::Bounded).await.unwrap();
            ledger.begin_send("x2").unwrap();
            journal.finish_success(&exit, None).await;
            drop(write);
        })
        .await;
        journal.flush_pending().await;
        let stamped = backend.row("x").unwrap().outcome.unwrap();
        assert_eq!(stamped.uncertain_generation, Some((7 << 32) | 1));
        assert!(stamped.uncertain_at.is_some());
    }

    /// Every occurrence moves the generation, a repeat of an open reason
    /// included, and is counted on its one incident; a latch-class reason sets
    /// the latch; each incident's upsert waits in `persistence_pending` until
    /// it lands, and one that moved while its write was out stays pending.
    #[tokio::test]
    async fn every_occurrence_moves_the_generation_and_counts_on_its_incident() {
        let (backend, journal) = journal();
        epoch(&journal, 3);
        uncertain_exit(&journal, "a").await;
        uncertain_exit(&journal, "b").await;
        assert_eq!(journal.gate().generation(), (3 << 32) | 2);
        let incidents = journal.gate().open_incidents();
        assert_eq!(incidents.len(), 1);
        assert_eq!(
            (incidents[0].count, incidents[0].first_generation, incidents[0].last_generation),
            (2, (3 << 32) | 1, (3 << 32) | 2)
        );
        assert!(!journal.latch_set());

        let occurrence = journal.gate().record(IncidentReason::UnpermittedMutation);
        assert_eq!(occurrence.generation, (3 << 32) | 3);
        assert!(occurrence.opened && journal.latch_set());
        assert_eq!(journal.gate().snapshot().incidents_persisting, 2);

        backend.set_incident_writes_fail(true);
        journal.flush_pending().await;
        assert_eq!(journal.gate().snapshot().incidents_persisting, 2, "not landed");
        backend.set_incident_writes_fail(false);
        journal.flush_pending().await;
        assert_eq!(journal.retry_set_depth(), 0);
        assert_eq!(backend.incidents().len(), 2);
        let row = backend
            .incidents()
            .into_iter()
            .find(|row| row.incident.reason == IncidentReason::UncertainOutcome)
            .unwrap();
        assert_eq!(row.incident.count, 2);

        uncertain_exit(&journal, "c").await;
        assert_eq!(journal.gate().snapshot().incidents_persisting, 1, "moved again");
    }

    /// Each latch-class source records its own reason with the latch set; the
    /// two others close the fence with the latch clear.
    #[test]
    fn the_latch_class_is_every_reason_but_two() {
        for reason in IncidentReason::ALL {
            let (_backend, journal) = journal();
            journal.gate().record(reason);
            assert!(journal.gate().fenced(), "{reason:?}");
            let latch_class =
                !matches!(reason, IncidentReason::UncleanStart | IncidentReason::UncertainOutcome);
            assert_eq!(journal.latch_set(), latch_class, "{reason:?}");
            assert_eq!(IncidentReason::parse(reason.as_str()), Some(reason));
        }
    }

    /// The last generation of an epoch is itself an occurrence: latch set,
    /// `generation_exhausted` opened, and the generation stays where it is.
    #[test]
    fn running_out_of_generations_latches_and_fences_until_a_restart() {
        let (_backend, journal) = journal();
        epoch(&journal, 1);
        journal.gate().lock().set_counter(GENERATION_COUNTER_LIMIT - 1);
        let last = journal.gate().record(IncidentReason::UncertainOutcome);
        assert!(last.exhausted);
        assert_eq!(last.generation, (1 << 32) | i64::from(GENERATION_COUNTER_LIMIT));
        assert!(journal.latch_set());
        assert_eq!(reasons(&journal), [
            IncidentReason::UncertainOutcome,
            IncidentReason::GenerationExhausted
        ]);
        let after = journal.gate().record(IncidentReason::UncertainOutcome);
        assert_eq!(after.generation, last.generation, "nothing left to move");
        assert!(!after.exhausted);
    }

    /// A start seeds the generation from its epoch and loads the incidents no
    /// release covered: any of them closes the fence.
    #[test]
    fn a_start_with_open_incidents_on_record_is_fenced() {
        let (_backend, journal) = journal();
        let view = EpochView {
            epoch_seq: 9,
            unclean_reason: Some("no_marker"),
        };
        let unclean = Incident {
            reason: IncidentReason::UncleanStart,
            first_at: Utc::now(),
            last_at: Utc::now(),
            count: 1,
            first_generation: (9 << 32) | 1,
            last_generation: (9 << 32) | 1,
        };
        journal.gate().begin_epoch(view, Some((9 << 32) | 1), vec![unclean]);
        assert!(journal.gate().fenced());
        assert_eq!(journal.gate().generation(), (9 << 32) | 1);
        assert_eq!(journal.retry_set_depth(), 0, "loaded rows are on record already");
        assert_eq!(journal.gate().epoch(), Some(view));

        let (_backend, clean) = super::tests::journal();
        clean
            .gate()
            .begin_epoch(EpochView { epoch_seq: 10, unclean_reason: None }, None, vec![]);
        assert!(!clean.gate().fenced());
        assert_eq!(clean.gate().generation(), 10 << 32);
    }

    /// Design §2.7: a release is refused while the latch is set, for a stale
    /// generation and while anything persists, and writes nothing then.
    #[tokio::test]
    async fn a_release_is_refused_latched_stale_or_persisting() {
        let (backend, journal) = journal();
        epoch(&journal, 2);
        backend.set_incident_writes_fail(true);
        uncertain_exit(&journal, "a").await;
        let current = journal.gate().generation();

        let stale = journal.release(release_of(current - 1)).await;
        assert_eq!(stale, Err(ReleaseRefusal::Stale { current }));
        let persisting = journal.release(release_of(current)).await;
        assert_eq!(persisting, Err(ReleaseRefusal::Persisting { pending: 1 }));

        backend.set_incident_writes_fail(false);
        journal.flush_pending().await;
        journal.gate().record(IncidentReason::OperationAborted);
        journal.flush_pending().await;
        let latched = journal.release(release_of(journal.gate().generation())).await;
        assert_eq!(latched, Err(ReleaseRefusal::Latched));
        assert!(backend.releases().is_empty());
        assert!(journal.gate().fenced());
    }

    /// An accepted release tombstones exactly the uncertain intents stamped at
    /// or below its generation, releases the incidents it covers and, with
    /// the generation unmoved, opens the fence. A row with a definite outcome,
    /// one never sent and one stamped later stay as they are.
    #[tokio::test]
    async fn a_release_tombstones_what_it_covers_and_opens_the_fence() {
        let (backend, journal) = journal();
        epoch(&journal, 4);
        uncertain_exit(&journal, "uncertain").await;
        let ledger = SubmitAttemptLedger::new(8);
        let ((), _op) = in_op(&journal, async {
            let done = journal.open_intent(spec("done", IntentKind::Exit));
            journal.before_send(&done, WriteClass::Bounded).await.unwrap();
            let _ = ledger.begin("done");
            journal.finish_success(&done, None).await;
            let unsent = journal.open_intent(spec("unsent", IntentKind::Exit));
            journal.before_send(&unsent, WriteClass::Bounded).await.unwrap();
            journal.finish_failed(&unsent).await;
        })
        .await;
        journal.flush_pending().await;
        let quoted = journal.gate().generation();

        let receipt = journal.release(release_of(quoted)).await.unwrap();
        assert_eq!(receipt.counts.intents_tombstoned, 1);
        assert_eq!(receipt.counts.incidents_released, 1);
        assert!(receipt.fence_open && !journal.gate().fenced());
        assert!(journal.gate().open_incidents().is_empty());
        assert!(backend.row("uncertain").unwrap().tombstoned_by.is_some());
        for untouched in ["done", "unsent"] {
            assert!(backend.row(untouched).unwrap().tombstoned_by.is_none(), "{untouched}");
        }

        // A later uncertainty is its own: fenced again, and not covered.
        uncertain_exit(&journal, "later").await;
        journal.flush_pending().await;
        assert!(journal.gate().fenced());
        let again = journal.release(release_of(quoted)).await;
        assert!(matches!(again, Err(ReleaseRefusal::Stale { .. })), "{again:?}");
        assert!(backend.row("later").unwrap().tombstoned_by.is_none());
    }

    /// A release whose generation moved while its transaction ran releases
    /// what it quoted, keeps the newer incident open and leaves the fence
    /// closed.
    #[tokio::test]
    async fn a_release_overtaken_by_an_occurrence_leaves_the_fence_closed() {
        let (backend, journal) = journal();
        epoch(&journal, 5);
        uncertain_exit(&journal, "a").await;
        journal.flush_pending().await;
        let quoted = journal.gate().generation();
        backend.set_commit_delay(Duration::from_millis(5));

        let release = journal.release(release_of(quoted));
        let overtaking = async {
            tokio::task::yield_now().await;
            journal.gate().record(IncidentReason::UncertainOutcome);
        };
        let (receipt, ()) = tokio::join!(release, overtaking);
        let receipt = receipt.unwrap();
        assert!(!receipt.fence_open);
        assert!(journal.gate().fenced());
        let open = journal.gate().open_incidents();
        assert_eq!(open.len(), 1);
        assert!(open[0].last_generation > quoted);
    }

    /// A release whose transaction fails changes nothing: the fence stays
    /// closed and the incident open. Without a durable journal there is no
    /// release at all.
    #[tokio::test]
    async fn a_release_that_did_not_complete_keeps_the_fence_closed() {
        let (backend, journal) = journal();
        uncertain_exit(&journal, "a").await;
        journal.flush_pending().await;
        backend.set_release_fails(true);
        let failed = journal.release(release_of(journal.gate().generation())).await;
        assert!(matches!(failed, Err(ReleaseRefusal::Backend(_))), "{failed:?}");
        assert!(journal.gate().fenced());
        assert_eq!(reasons(&journal), [IncidentReason::UncertainOutcome]);

        let unpersisted = Arc::new(OrderJournal::unpersisted());
        unpersisted.gate().record(IncidentReason::UncleanStart);
        unpersisted.flush_pending().await;
        let refused = unpersisted.release(release_of(unpersisted.gate().generation())).await;
        assert!(matches!(refused, Err(ReleaseRefusal::Backend(_))), "{refused:?}");
        assert!(unpersisted.gate().fenced());
    }

    /// Review round 1: retry passes never overlap. With one pass's incident
    /// upsert held, a second pass, the stop's final flush say, waits for it
    /// instead of writing the same snapshot again; a release meanwhile answers
    /// `persisting`; once the first lands the release goes through and nothing
    /// is written after it.
    #[tokio::test]
    async fn retry_passes_never_overlap() {
        let (backend, journal) = journal();
        journal.gate().record(IncidentReason::UncleanStart);
        let held = backend.hold_next_incident_write();
        let pass = |journal: &Arc<OrderJournal>| {
            let journal = Arc::clone(journal);
            tokio::spawn(async move { journal.flush_pending().await })
        };
        let first = pass(&journal);
        while backend.incident_write_attempts() == 0 {
            tokio::task::yield_now().await;
        }
        let second = pass(&journal);
        for _ in 0..50 {
            tokio::task::yield_now().await;
        }
        assert_eq!(backend.incident_write_attempts(), 1, "the second pass waits");
        let early = journal.release(release_of(journal.gate().generation())).await;
        assert!(matches!(early, Err(ReleaseRefusal::Persisting { .. })), "{early:?}");

        held.notify_one();
        assert_eq!(first.await.unwrap(), 1);
        assert_eq!(second.await.unwrap(), 0, "nothing left for it");
        let released = journal.release(release_of(journal.gate().generation())).await.unwrap();
        assert!(released.fence_open);
        assert_eq!(backend.incident_write_attempts(), 1);
        assert!(backend.incidents().iter().all(|row| row.released_by.is_some()));
    }

    /// Each source of the latch records its reason: a bounded send without
    /// its row, discarded history, a conflicting row on retry, a suspended
    /// stream, an aborted operation and an unpermitted mutation.
    #[tokio::test]
    async fn each_latch_source_records_its_reason() {
        let (backend, unlanded) = journal();
        backend.push_fault(JournalFault::Known);
        let ((), _op) = in_op(&unlanded, async {
            let intent = unlanded.open_intent(spec("a", IntentKind::Exit));
            unlanded.before_send(&intent, WriteClass::Bounded).await.unwrap();
        })
        .await;
        assert_eq!(reasons(&unlanded), [IncidentReason::UnlandedSend]);

        let (backend, conflicting) = journal();
        let ((), _op) = in_op(&conflicting, async {
            let intent = conflicting.open_intent(spec("a", IntentKind::Exit));
            conflicting.before_send(&intent, WriteClass::Bounded).await.unwrap();
            backend.push_fault(JournalFault::Known);
            conflicting.finish_success(&intent, None).await;
        })
        .await;
        backend.push_fault(JournalFault::IdSpent);
        conflicting.flush_pending().await;
        assert_eq!(reasons(&conflicting), [IncidentReason::PersistenceFailure]);

        let (_backend, suspended) = journal();
        suspended.record_suspension(StreamSuspensionRow {
            suspension_id: Uuid::now_v7(),
            stream_key: "position:a".to_string(),
            stream_kind: "position",
            failed_seq: None,
            month: None,
            error: "injected".to_string(),
            epoch_id: None,
            suspended_at: Utc::now(),
        });
        assert_eq!(reasons(&suspended), [IncidentReason::StreamSuspended]);

        let (_backend, aborted) = journal();
        drop(aborted.acquire(OperationKind::Panic).unwrap());
        assert_eq!(reasons(&aborted), [IncidentReason::OperationAborted]);

        let (_backend, unpermitted) = journal();
        unpermitted.gate().mutation("POST /order").unwrap();
        assert_eq!(reasons(&unpermitted), [IncidentReason::UnpermittedMutation]);

        for journal in [unlanded, conflicting, suspended, aborted, unpermitted] {
            assert!(journal.latch_set() && journal.gate().fenced());
        }
    }

    // =========================================================================
    // PR F: the orphan-cancel veto (design §2.9)
    // =========================================================================

    async fn the_orphans_cancel(
        gate: Arc<OperationGate>,
        then_fence: bool,
    ) -> Result<StopCancelOutcome, ExecError> {
        if then_fence {
            // Something opens an incident after the sweep's own check.
            gate.record(IncidentReason::UncertainOutcome);
        }
        gate.mutation("DELETE /fapi/v1/algoOrder")
            .map_err(|refused| ExecError::SendRefused(refused.to_string()))?;
        Ok(cancelled())
    }

    fn cancel_rows(backend: &StubJournalBackend) -> Vec<StubJournalRow> {
        backend
            .rows()
            .into_iter()
            .filter(|row| row.intent.as_ref().unwrap().kind == IntentKind::Cancel)
            .collect()
    }

    /// While an incident is open an orphan's cancel is withheld before its
    /// journal write: no intent, nothing sent, no new occurrence.
    #[tokio::test]
    async fn an_orphan_cancel_is_withheld_while_an_incident_is_open() {
        let (backend, journal) = journal();
        journal.gate().record(IncidentReason::UncleanStart);
        let generation = journal.gate().generation();
        let ((), _op) = in_op(&journal, async {
            let never = async { panic!("a withheld cancel is never polled") };
            let cancelled = journal.journaled_orphan_cancel("77", never).await;
            assert!(matches!(cancelled, OrphanCancel::Withheld), "{cancelled:?}");
        })
        .await;
        journal.flush_pending().await;
        assert!(cancel_rows(&backend).is_empty());
        assert_eq!(journal.gate().generation(), generation, "a withheld cancel is no occurrence");
    }

    /// The binding check is at the handoff, after the journal's write: an
    /// incident opened in between refuses the request there. Nothing left, so
    /// the intent is a proven no-send, class `withheld`, and no uncertain
    /// outcome is recorded for it.
    #[tokio::test]
    async fn the_handoff_withholds_an_orphan_cancel_the_fence_overtook() {
        let (backend, journal) = journal();
        let ((), _op) = in_op(&journal, async {
            let cancel = the_orphans_cancel(Arc::clone(journal.gate()), true);
            let cancelled = journal.journaled_orphan_cancel("77", cancel).await;
            assert!(matches!(cancelled, OrphanCancel::Withheld), "{cancelled:?}");
        })
        .await;
        journal.flush_pending().await;
        let rows = cancel_rows(&backend);
        assert_eq!(rows.len(), 1);
        let outcome = rows[0].outcome.clone().unwrap();
        assert!(outcome.not_sent && outcome.uncertain_at.is_none(), "{outcome:?}");
        assert_eq!(outcome.cancel_class, Some("withheld"));
        assert_eq!(
            reasons(&journal),
            [IncidentReason::UncertainOutcome],
            "only the overtaking one"
        );
        let gate = journal.gate().snapshot();
        assert_eq!((gate.orphan_cancels_passed, gate.orphan_cancels_withheld), (0, 1));
    }

    /// With nothing open the orphan's cancel goes out and is classified as any
    /// cancel; a protective cancel outside the sweep is never subject to the
    /// veto, fence or not.
    #[tokio::test]
    async fn only_the_sweeps_cancels_are_vetoed() {
        let (backend, journal) = journal();
        let ((), _op) = in_op(&journal, async {
            let cancel = the_orphans_cancel(Arc::clone(journal.gate()), false);
            let cancelled = journal.journaled_orphan_cancel("77", cancel).await;
            assert!(matches!(cancelled, OrphanCancel::Sent(CancelClass::Cancelled { .. })));
        })
        .await;
        journal.flush_pending().await;
        assert_eq!(
            cancel_rows(&backend)[0].outcome.clone().unwrap().cancel_class,
            Some("cancelled")
        );

        journal.gate().record(IncidentReason::UncertainOutcome);
        journal
            .acquire(OperationKind::CancelOrClose)
            .unwrap()
            .run(async {
                let class = journal
                    .journaled_cancel(None, "78", async {
                        journal.gate().mutation("DELETE /fapi/v1/algoOrder").unwrap();
                        Ok(cancelled())
                    })
                    .await;
                assert!(class.is_definite(), "a follow-up's cancel goes out under the fence");
            })
            .await;
    }

    /// A cancel one of whose requests passed the handoff may have reached the
    /// venue: refused on a later one, it is uncertain, never `not_sent`.
    #[tokio::test]
    async fn an_orphan_cancel_that_passed_the_handoff_once_is_not_a_no_send() {
        let (backend, journal) = journal();
        let ((), _op) = in_op(&journal, async {
            let gate = Arc::clone(journal.gate());
            let cancel = async move {
                gate.mutation("DELETE /fapi/v1/algoOrder").unwrap();
                gate.record(IncidentReason::UncleanStart);
                gate.mutation("DELETE /fapi/v1/order")
                    .map_err(|refused| ExecError::SendRefused(refused.to_string()))?;
                Ok(cancelled())
            };
            let cancelled = journal.journaled_orphan_cancel("77", cancel).await;
            assert!(matches!(cancelled, OrphanCancel::Sent(CancelClass::Errored(_))));
        })
        .await;
        journal.flush_pending().await;
        let outcome = cancel_rows(&backend)[0].outcome.clone().unwrap();
        assert!(!outcome.not_sent && outcome.uncertain_at.is_some(), "{outcome:?}");
    }

    /// The report lists at most 500 withheld orders and counts every one.
    #[test]
    fn the_withheld_report_is_capped_and_counts_past_its_cap() {
        let mut report = WithheldReport::default();
        for index in 0..WITHHELD_REPORT_CAPACITY + 2 {
            let listed = report.withhold(WithheldOrder {
                symbol: "BTCUSDT".to_string(),
                exchange_order_id: index.to_string(),
                client_order_id: format!("ins-{index}"),
            });
            assert_eq!(listed, index < WITHHELD_REPORT_CAPACITY);
        }
        assert_eq!(report.orders.len(), WITHHELD_REPORT_CAPACITY);
        assert_eq!(report.total, WITHHELD_REPORT_CAPACITY as u64 + 2);
        assert!(report.truncated);
    }

    /// ADR-0056 step 7, PR O: an operation owns the orders it placed and the
    /// orders it cancels until it ends, after the intents' own memory has gone
    /// and whatever became of their persistence.
    #[tokio::test]
    async fn a_live_operation_owns_its_orders_until_it_ends() {
        async fn place_and_cancel(journal: &Arc<OrderJournal>) {
            let stop = journal.open_intent(spec("ins-w", IntentKind::Protection));
            let _ = journal.before_send(&stop, WriteClass::Bounded).await;
            journal.finish_success(&stop, None).await;
            let cancel = journal.open_intent(IntentSpec {
                cancels_order_id: Some("V-1".to_string()),
                ..spec("cxl-v", IntentKind::Cancel)
            });
            let _ = journal.before_send(&cancel, WriteClass::Bounded).await;
            let cancelled = CancelClass::Cancelled {
                filled_before_cancel: rust_decimal::Decimal::ZERO,
                fills: None,
            };
            journal.finish_cancel(&cancel, &cancelled).await;
        }

        // Landed: both intents leave memory, the operation still owns both.
        let (backend, landed) = journal();
        let ((), operation) = in_op(&landed, place_and_cancel(&landed)).await;
        assert!(backend.row("ins-w").unwrap().outcome.is_some(), "it landed");
        assert!(!landed.is_live("ins-w") && !landed.is_live("cxl-v"), "their memory is gone");
        assert!(landed.owned_by_live_operation("ins-w", "W-1"), "the stop it placed");
        assert!(landed.owned_by_live_operation("ins-other", "V-1"), "the order it cancels");
        assert!(!landed.owned_by_live_operation("ins-other", "OTHER-1"));
        drop(operation);
        assert!(!landed.owned_by_live_operation("ins-w", "W-1"));
        assert!(!landed.owned_by_live_operation("ins-other", "V-1"));

        // Every commit failing, the retry set full and the history discarded.
        let (backend, failing) = journal();
        backend.saturate(&failing).await;
        let ((), operation) = in_op(&failing, place_and_cancel(&failing)).await;
        assert!(failing.latch_set());
        assert!(!failing.is_live("ins-w") && !failing.is_live("cxl-v"), "discarded");
        assert!(backend.row("ins-w").is_none() && backend.row("cxl-v").is_none());
        assert!(failing.owned_by_live_operation("ins-w", "W-1"));
        assert!(failing.owned_by_live_operation("ins-other", "V-1"));
        drop(operation);
        assert!(!failing.owned_by_live_operation("ins-w", "W-1"));
        assert!(!failing.owned_by_live_operation("ins-other", "V-1"));
    }
}
