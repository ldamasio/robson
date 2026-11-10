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
//! acts on them: an uncertain intent opens no incident and closes no admission,
//! and the latch only shows on a metric and keeps a stop from marking itself
//! clean. Later PRs of step 7 add the rest.
//!
//! **Operations and permits (PR S).** An operation is one entry of
//! `operations_in_flight`, in a task-local scope. One taken with a permit
//! ([`OrderJournal::acquire`]) belongs to the permit for the whole operation:
//! an entry, its result handling and its stop are one operation, and so is
//! everything a nested call does under it. The send ticket is issued at the
//! send's start under the gate's rules ([`begin_send`]).
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
use chrono::{DateTime, Utc};
use robson_domain::PositionId;
use serde_json::Value;
use tracing::{error, warn};
use uuid::Uuid;

use crate::{
    cancel::{classify_cancel, CancelClass},
    error::ExecError,
    gate::{OperationGate, OperationKind, PermitRefusal, Phase, ALLOCATED_SLOTS},
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
        }
        Ok(())
    }
}

#[async_trait]
impl JournalBackend for StubJournalBackend {
    async fn commit(&self, batch: &JournalBatch) -> Result<(), CommitFailure> {
        self.commits.lock().unwrap().push(batch.clone());
        let delay = *self.commit_delay.lock().unwrap();
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
        let scripted = self.faults.lock().unwrap().pop_front();
        let fault = scripted.or(*self.fail_all.lock().unwrap());
        match fault {
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
}

#[derive(Debug, Default)]
struct JournalState {
    /// Intents in flight or with something unlanded. Removed once finished
    /// and landed, so at most the retry set plus the orders running now.
    intents: HashMap<String, IntentMemory>,
    /// `persistence_pending`, in client ids.
    retry_set: VecDeque<String>,
    /// `operations_in_flight`. A list is as long as its operation's orders.
    operations: HashMap<u64, Vec<OperationIntent>>,
    /// Entries of `operations` held by operations without a permit.
    unpermitted_listed: usize,
    /// Ids sent with no row and no room in the retry set: the only record
    /// that they were used (rule 3). Any other id that leaves memory either
    /// has a durable row, which the unique index holds for good, or was never
    /// sent.
    discarded: HashSet<String>,
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

    /// Nothing here is in flight or waiting to land: no intent in memory and
    /// nothing in the retry set. Part of a clean stop's check.
    pub fn is_quiescent(&self) -> bool {
        let state = self.lock();
        state.intents.is_empty() && state.retry_set.is_empty()
    }

    /// Entries waiting in `persistence_pending`.
    pub fn retry_set_depth(&self) -> usize {
        self.lock().retry_set.len()
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
            state.operations.insert(id, Vec::new());
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
        self.lock().operations.insert(id, Vec::new());
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
            listed.iter().any(|intent| {
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
            listed.push(OperationIntent {
                client_order_id: spec.client_order_id.clone(),
                kind: spec.kind,
                ticket_issued: false,
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
        let mut state = self.lock();
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
            self.remember_unlanded_send(&mut state, &intent.client_order_id);
            warn!(
                client_order_id = %intent.client_order_id,
                ?failure,
                "Bounded send goes out without its journal row"
            );
            self.latch.store(true, Ordering::SeqCst);
            return Ok(());
        }
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
    /// discarded id. With both full the id is not kept and the send still goes
    /// out; the loss is counted and logged.
    fn remember_unlanded_send(&self, state: &mut JournalState, client_order_id: &str) {
        if Self::try_enqueue(state, &self.latch, client_order_id) {
            return;
        }
        if state.discarded.len() < DISCARDED_CAPACITY {
            error!(%client_order_id, "Journal retry set is full; keeping only this send's id");
            state.discarded.insert(client_order_id.to_string());
            return;
        }
        self.non_reuse_ids_lost.fetch_add(1, Ordering::Relaxed);
        error!(%client_order_id, "Journal cannot keep this send's id; non-reuse rests on freshness");
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
            if memory.row.intent_id != intent.intent_id
                || !memory.retry_refused
                || memory.tickets != 1
                || memory.replaced_by.is_some()
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
        self.before_send(&fresh, WriteClass::Bounded).await?;
        Ok(fresh)
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
        if entry && !gate.entry_send_allowed() {
            return Err(SendRefusal::NotSent(format!(
                "entry refused at its send: the daemon is {}",
                gate.phase().as_str()
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
            if let Some(entry) = listed.iter_mut().find(|e| e.client_order_id == client_order_id) {
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
        let replacement = self.replacement_of(intent);
        if replacement.is_some() {
            self.finish_failed_own(intent).await;
        }
        let accepted = replacement.as_ref().unwrap_or(intent);
        self.write_outcome(accepted, OutcomeRow {
            venue_outcome: Some(VenueOutcome::Success),
            venue_evidence: evidence,
            ..OutcomeRow::default()
        })
        .await;
    }

    /// The order ended without a venue acceptance: `Rejected` if every attempt
    /// was authoritatively refused, marked not sent if no request left,
    /// otherwise uncertain.
    pub async fn finish_failed(&self, intent: &JournaledIntent) {
        if let Some(fresh) = self.replacement_of(intent) {
            self.finish_failed_own(&fresh).await;
        }
        self.finish_failed_own(intent).await;
    }

    async fn finish_failed_own(&self, intent: &JournaledIntent) {
        let outcome = {
            let mut state = self.lock();
            let Some(memory) = state.intents.get_mut(&intent.client_order_id) else {
                return;
            };
            if memory.row.intent_id != intent.intent_id {
                return;
            }
            memory.sealed = true;
            Self::failed_outcome(memory)
        };
        match outcome {
            Some(outcome) => self.write_outcome(intent, outcome).await,
            None => {
                self.lock().intents.remove(&intent.client_order_id);
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

    /// The result's write. Never fails: the caller keeps the venue's answer
    /// either way. It is registered for retry before the wait, so a future
    /// dropped mid-commit leaves it in the retry set, counted and flushed.
    async fn write_outcome(&self, intent: &JournaledIntent, outcome: OutcomeRow) {
        let batch = {
            let mut state = self.lock();
            let Some(memory) = state.intents.get_mut(&intent.client_order_id) else {
                return;
            };
            if memory.row.intent_id != intent.intent_id {
                return;
            }
            memory.sealed = true;
            memory.outcome = Some(outcome);
            memory.outcome_landed = false;
            let batch = memory.batch();
            Self::enqueue(&mut state, &self.latch, &intent.client_order_id);
            batch
        };
        match self.bounded_commit(&batch).await {
            Ok(()) => Self::landed(&mut self.lock(), &intent.client_order_id, &batch),
            Err(failure) => warn!(
                client_order_id = %intent.client_order_id,
                ?failure,
                "Journal result write failed; it will be retried"
            ),
        }
    }

    /// Add to `persistence_pending`, logging an overflow.
    fn enqueue(state: &mut JournalState, latch: &AtomicBool, client_order_id: &str) {
        if !Self::try_enqueue(state, latch, client_order_id) {
            error!(%client_order_id, "Journal retry set is full; discarding this intent's history");
        }
    }

    /// `false` on overflow: the history is discarded, the latch is set, and an
    /// intent that has its outcome is forgotten here.
    fn try_enqueue(state: &mut JournalState, latch: &AtomicBool, client_order_id: &str) -> bool {
        let Some(memory) = state.intents.get_mut(client_order_id) else {
            return false;
        };
        if memory.in_retry_set {
            return true;
        }
        if state.retry_set.len() < RETRY_SET_CAPACITY {
            memory.in_retry_set = true;
            state.retry_set.push_back(client_order_id.to_string());
            return true;
        }
        latch.store(true, Ordering::SeqCst);
        memory.sealed = true;
        if memory.outcome.is_some() {
            state.intents.remove(client_order_id);
        }
        false
    }

    fn landed(state: &mut JournalState, client_order_id: &str, batch: &JournalBatch) {
        let Some(memory) = state.intents.get_mut(client_order_id) else {
            return;
        };
        memory.mark_landed(batch);
        if !memory.has_unlanded() {
            let finished = memory.outcome.is_some();
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

    /// One pass of the retry loop. Stops at the first failure, sending that
    /// entry to the back, so a dead database costs one bound per pass and one
    /// bad row cannot starve the others, and stops starting commits after a
    /// few bounds even when each succeeds slowly. Returns the entries written.
    pub async fn flush_pending(&self) -> usize {
        let batches: Vec<(String, JournalBatch)> = {
            let state = self.lock();
            state
                .retry_set
                .iter()
                .take(RETRY_PASS_LIMIT)
                .filter_map(|id| state.intents.get(id).map(|m| (id.clone(), m.batch())))
                .collect()
        };
        let mut written = 0;
        let started = std::time::Instant::now();
        let pass_bound = self.commit_bound * RETRY_PASS_COMMIT_BOUNDS;
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
                    let mut state = self.lock();
                    if failure == CommitFailure::IdSpent {
                        self.latch.store(true, Ordering::SeqCst);
                        state.retry_set.retain(|id| *id != client_order_id);
                        state.intents.remove(&client_order_id);
                        error!(%client_order_id, "Journal row conflicts with another intent");
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
        let mut state = self.journal.lock();
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
                    memory.outcome = Some(outcome);
                    OrderJournal::enqueue(&mut state, &self.journal.latch, &client_order_id);
                },
                None => {
                    state.intents.remove(&client_order_id);
                },
            }
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
            assert_eq!(journal.retry_set_depth(), 1, "{fault:?}");
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
        assert_eq!(journal.flush_pending().await, 0);
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
            assert_eq!(journal.retry_set_depth(), RETRY_SET_CAPACITY);
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
        assert_eq!(journal.retry_set_depth(), 1);
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
            journal.finish_success(&intent, None).await;
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
        assert_eq!(journal.retry_set_depth(), 1);
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
}
