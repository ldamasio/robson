//! Operation permits and the stop phases (ADR-0056 step 7, PR S).
//!
//! Every operation that can mutate the venue runs under a permit, taken at its
//! entry, before any mutation and before the manager lock, and held in a
//! task-local scope for the whole operation: its result handling and its
//! protective follow-ups included. A nested operation runs under the permit it
//! is reached from and never takes another. A permit must be completed; one
//! dropped without completion, its future cancelled or panicked, counts as an
//! aborted operation and sets the journal latch.
//!
//! Permits come from three classes of slots, so ordinary API load can never
//! take the capacity of a close, `/panic` or an internal loop:
//!
//! | Class | Slots | Refused from |
//! |---|---|---|
//! | Internal, one reserved each: main loop, reconciliation scan, funding poll, startup | 4 | `Closing` |
//! | Protective API: cancel or close, `/panic`, MonthlyHalt, `/reconcile-close` | 8 | `Closing` |
//! | Ordinary API: arm, signal, approval, funding execute, spot USDT recovery | 16 | `Draining` |
//!
//! A class that is full refuses at once. Slots guarantee allocation, not
//! latency: holders still queue on the manager lock.
//!
//! **The choke point.** Every signed venue mutation calls
//! [`OperationGate::mutation`]. With a permit it is allowed in every phase.
//! Without one it is allowed only while `Open`, and it is counted and sets the
//! latch; from `Draining` on it is refused.
//!
//! **The fence (PR F, design §2.7).** The gate also holds the admission
//! fence, the generation and the open incidents. Whatever the daemon cannot
//! explain is recorded as an occurrence, in one critical section under this
//! mutex at the moment it is known and before any await: the fence closes, the
//! generation moves, the incident of its reason is opened or counted, and its
//! durable upsert waits in `persistence_pending` (the journal's retry loop
//! writes it). Protection never waits for that write. An entry is refused at
//! its send while the fence is closed, and the three admission sites refuse
//! too. Only an operator's audited release ([`OperationGate::release_through`],
//! reached from the journal's release) opens it again, and only when nothing
//! moved the generation meanwhile.
//!
//! The gate's mutex is a sync leaf, never held across an await. Lock order
//! (design §2.10): the manager lock, a stream's lock, this gate, the journal's
//! memory. The journal takes its memory under the gate wherever it records an
//! occurrence, and so does the send start.

use std::{
    collections::BTreeMap,
    fmt,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, MutexGuard, OnceLock,
    },
};

use chrono::{DateTime, Utc};
use tokio::{sync::Notify, time::Instant};
use tracing::{error, warn};

use crate::order_journal::{self, OPERATIONS_CAPACITY};

/// Ordinary API operations at once.
pub const ORDINARY_API_SLOTS: u32 = 16;
/// Protective API operations at once; ordinary ones never take these.
pub const PROTECTIVE_API_SLOTS: u32 = 8;
/// One reserved slot per internal loop.
pub const INTERNAL_SLOTS: u32 = 4;
/// Every slot. The operation registry holds one entry per permit, and keeps
/// the rest of its capacity for operations without one.
pub const ALLOCATED_SLOTS: u32 = ORDINARY_API_SLOTS + PROTECTIVE_API_SLOTS + INTERNAL_SLOTS;

const _: () = assert!(ALLOCATED_SLOTS as usize <= OPERATIONS_CAPACITY);

/// Where the daemon is in its life. Only ever moves forward.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum Phase {
    #[default]
    Open,
    /// Ordinary API operations are refused; everything else continues.
    Draining,
    /// Every new operation is refused; those holding a permit finish.
    Closing,
}

impl Phase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Draining => "draining",
            Self::Closing => "closing",
        }
    }
}

/// The internal loops, each with its reserved slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InternalSlot {
    MainLoop,
    ReconciliationScan,
    FundingPoll,
    Startup,
}

impl InternalSlot {
    fn index(self) -> usize {
        match self {
            Self::MainLoop => 0,
            Self::ReconciliationScan => 1,
            Self::FundingPoll => 2,
            Self::Startup => 3,
        }
    }
}

/// The slot class a permit takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SlotClass {
    Internal(InternalSlot),
    Ordinary,
    Protective,
}

impl SlotClass {
    pub fn label(self) -> &'static str {
        match self {
            Self::Internal(_) => "internal",
            Self::Ordinary => "ordinary",
            Self::Protective => "protective",
        }
    }

    /// The first phase that refuses this class.
    fn refused_from(self) -> Phase {
        match self {
            Self::Ordinary => Phase::Draining,
            Self::Internal(_) | Self::Protective => Phase::Closing,
        }
    }
}

/// What a permit was taken for: one per row of the design's scope table
/// (§2.6). The kind decides the class, so the table lives here once.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OperationKind {
    /// One main-loop event.
    MainLoop,
    /// One reconciliation scan, the startup scan aside.
    ReconciliationScan,
    /// One funding worker poll.
    FundingPoll,
    /// The startup phase: stale gate, startup scan, startup recovery.
    Startup,
    /// The manager's own signal listener, which only tests start. It plays the
    /// main loop's part and takes its slot.
    SignalListener,
    /// `POST /positions`; an `Immediate` arm enters under it.
    Arm,
    /// `POST /positions/:id/signal`.
    Signal,
    /// `POST /queries/:id/approve`.
    Approve,
    /// `POST /funding/execute`.
    FundingExecute,
    /// `POST /funding/recover-spot-usdt-to-futures`, when it executes.
    RecoverSpotUsdt,
    /// `DELETE /positions/:id`.
    CancelOrClose,
    /// `POST /panic`.
    Panic,
    /// `POST /monthly-halt`.
    MonthlyHalt,
    /// `POST /reconcile-close`; MonthlyHalt and `/panic` are reachable under
    /// it.
    ReconcileClose,
}

impl OperationKind {
    pub fn class(self) -> SlotClass {
        match self {
            Self::MainLoop | Self::SignalListener => SlotClass::Internal(InternalSlot::MainLoop),
            Self::ReconciliationScan => SlotClass::Internal(InternalSlot::ReconciliationScan),
            Self::FundingPoll => SlotClass::Internal(InternalSlot::FundingPoll),
            Self::Startup => SlotClass::Internal(InternalSlot::Startup),
            Self::Arm
            | Self::Signal
            | Self::Approve
            | Self::FundingExecute
            | Self::RecoverSpotUsdt => SlotClass::Ordinary,
            Self::CancelOrClose | Self::Panic | Self::MonthlyHalt | Self::ReconcileClose => {
                SlotClass::Protective
            },
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::MainLoop => "main_loop",
            Self::ReconciliationScan => "reconciliation_scan",
            Self::FundingPoll => "funding_poll",
            Self::Startup => "startup",
            Self::SignalListener => "signal_listener",
            Self::Arm => "arm",
            Self::Signal => "signal",
            Self::Approve => "approve",
            Self::FundingExecute => "funding_execute",
            Self::RecoverSpotUsdt => "recover_spot_usdt",
            Self::CancelOrClose => "cancel_or_close",
            Self::Panic => "panic",
            Self::MonthlyHalt => "monthly_halt",
            Self::ReconcileClose => "reconcile_close",
        }
    }
}

/// Why the admission fence closed: the reasons of `recovery_incidents`
/// (design §2.7). At most one incident of each is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum IncidentReason {
    /// The previous process did not stop clean at `marker_version` 2.
    UncleanStart,
    /// An intent's outcome is uncertain: it may have reached the venue.
    UncertainOutcome,
    /// A bounded send went out without its journal row.
    UnlandedSend,
    /// Journal history was discarded, or a row conflicted on retry.
    PersistenceFailure,
    /// A venue mutation ran without an operation permit.
    UnpermittedMutation,
    /// An event-log stream was suspended.
    StreamSuspended,
    /// An operation was dropped before it completed.
    OperationAborted,
    /// The generation counter of this epoch ran out.
    GenerationExhausted,
}

impl IncidentReason {
    pub const ALL: [Self; 8] = [
        Self::UncleanStart,
        Self::UncertainOutcome,
        Self::UnlandedSend,
        Self::PersistenceFailure,
        Self::UnpermittedMutation,
        Self::StreamSuspended,
        Self::OperationAborted,
        Self::GenerationExhausted,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::UncleanStart => "unclean_start",
            Self::UncertainOutcome => "uncertain_outcome",
            Self::UnlandedSend => "unlanded_send",
            Self::PersistenceFailure => "persistence_failure",
            Self::UnpermittedMutation => "unpermitted_mutation",
            Self::StreamSuspended => "stream_suspended",
            Self::OperationAborted => "operation_aborted",
            Self::GenerationExhausted => "generation_exhausted",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|reason| reason.as_str() == text)
    }

    /// A latch-class reason sets the journal latch, which only a restart
    /// clears, so its release waits for the next process.
    pub fn latches(self) -> bool {
        !matches!(self, Self::UncleanStart | Self::UncertainOutcome)
    }
}

/// An open incident: the occurrences of one reason since it was last released.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Incident {
    pub reason: IncidentReason,
    pub first_at: DateTime<Utc>,
    pub last_at: DateTime<Utc>,
    pub count: i64,
    pub first_generation: i64,
    pub last_generation: i64,
}

/// What recording one occurrence did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Occurrence {
    pub reason: IncidentReason,
    /// The generation it was recorded at.
    pub generation: i64,
    /// Whether it opened its reason's incident, rather than counting it again.
    pub opened: bool,
    /// Whether it took the last generation of this epoch.
    pub exhausted: bool,
}

impl Occurrence {
    /// Log it, outside every lock.
    pub fn log(&self) {
        let (reason, generation) = (self.reason.as_str(), self.generation);
        if self.opened {
            error!(reason, generation, "Incident opened; admissions are fenced until a release");
        } else {
            warn!(reason, generation, "Incident occurred again; the fence stays closed");
        }
        if self.exhausted {
            error!(generation, "Generation exhausted; latch set, fenced until a restart");
        }
    }
}

/// The generation counter's ceiling within one epoch: `n` stays below
/// `2^32 − 1`, and reaching it is itself an occurrence (design §2.7).
pub const GENERATION_COUNTER_LIMIT: u32 = u32::MAX;

/// What a start recorded about its epoch, for the pending list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EpochView {
    pub epoch_seq: i64,
    /// Why the start was unclean; `None` when its predecessor stopped clean.
    pub unclean_reason: Option<&'static str>,
}

#[derive(Debug)]
struct OpenIncident {
    incident: Incident,
    /// Bumped by every occurrence; the durable upsert is pending while it is
    /// ahead of `landed`.
    version: u64,
    landed: u64,
}

/// Why no permit was given. Nothing was done either way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermitRefusal {
    /// The class has no free slot.
    Busy(SlotClass),
    /// The daemon is stopping and refuses this class.
    Stopping(Phase),
}

impl fmt::Display for PermitRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Busy(class) => write!(f, "busy: every {} slot is taken", class.label()),
            Self::Stopping(phase) => write!(f, "stopping: refused while {}", phase.as_str()),
        }
    }
}

impl std::error::Error for PermitRefusal {}

/// A venue mutation refused at the choke point. Nothing was sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MutationRefused {
    /// No permit, and the daemon is stopping.
    Stopping(Phase),
}

impl fmt::Display for MutationRefused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Stopping(phase) => {
                write!(f, "venue mutation without a permit refused while {}", phase.as_str())
            },
        }
    }
}

impl std::error::Error for MutationRefused {}

/// Told of what the gate counts, so the daemon can export it.
pub trait GateObserver: Send + Sync {
    fn unpermitted_mutation(&self);
    fn operation_aborted(&self, kind: OperationKind);
}

/// The gate's counters at one instant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GateSnapshot {
    pub phase: Phase,
    pub fenced: bool,
    /// The current generation: `epoch_seq << 32 | n`.
    pub generation: i64,
    pub open_incidents: u32,
    /// Incidents whose durable upsert has not landed.
    pub incidents_persisting: u32,
    pub permits: u32,
    pub ordinary: u32,
    pub protective: u32,
    pub internal: u32,
    pub sends_started: u64,
    pub aborted: u64,
    pub unpermitted: u64,
    pub busy_ordinary: u64,
    pub busy_protective: u64,
}

#[derive(Debug)]
pub(crate) struct GateState {
    phase: Phase,
    /// The admission fence: closed by any occurrence, opened only by a
    /// release that found the generation unmoved.
    fenced: bool,
    /// The journal's latch, set with every latch-class occurrence.
    latch: Arc<AtomicBool>,
    /// The high half of the generation, from the process epoch.
    epoch_seq: i64,
    epoch: Option<EpochView>,
    /// The low half, `n`, bumped by every occurrence.
    counter: u32,
    incidents: BTreeMap<IncidentReason, OpenIncident>,
    permits: u32,
    ordinary: u32,
    protective: u32,
    internal: [bool; INTERNAL_SLOTS as usize],
    sends_started: u64,
    aborted: u64,
    unpermitted: u64,
    busy_ordinary: u64,
    busy_protective: u64,
}

impl GateState {
    fn new(latch: Arc<AtomicBool>) -> Self {
        Self {
            phase: Phase::Open,
            fenced: false,
            latch,
            epoch_seq: 0,
            epoch: None,
            counter: 0,
            incidents: BTreeMap::new(),
            permits: 0,
            ordinary: 0,
            protective: 0,
            internal: [false; INTERNAL_SLOTS as usize],
            sends_started: 0,
            aborted: 0,
            unpermitted: 0,
            busy_ordinary: 0,
            busy_protective: 0,
        }
    }

    /// Whether an entry may start its send now. Anything else may in every
    /// phase.
    pub(crate) fn entry_send_allowed(&self) -> bool {
        self.phase == Phase::Open && !self.fenced
    }

    pub(crate) fn phase(&self) -> Phase {
        self.phase
    }

    pub(crate) fn fenced(&self) -> bool {
        self.fenced
    }

    pub(crate) fn send_started(&mut self) {
        self.sends_started += 1;
    }

    /// `epoch_seq << 32 | n`, a signed value ordered numerically and
    /// monotonic across epochs through the strict epoch insert.
    pub(crate) fn generation(&self) -> i64 {
        (self.epoch_seq << 32) | i64::from(self.counter)
    }

    /// Record one occurrence (design §2.7): the latch first for a
    /// latch-class reason, then the fence, the generation and the incident,
    /// whose upsert is now pending. The caller holds the gate's mutex from the
    /// moment the occurrence is known and logs only after releasing it.
    pub(crate) fn occur(&mut self, reason: IncidentReason) -> Occurrence {
        if reason.latches() {
            self.latch.store(true, Ordering::SeqCst);
        }
        self.fenced = true;
        let exhausted = self.counter < GENERATION_COUNTER_LIMIT && {
            self.counter += 1;
            self.counter == GENERATION_COUNTER_LIMIT
        };
        let generation = self.generation();
        let now = Utc::now();
        let opened = self.note(reason, generation, now);
        if exhausted {
            // No generation is left to move: nothing can show a release that
            // the fence stayed closed, so it stays closed until a restart.
            self.latch.store(true, Ordering::SeqCst);
            self.note(IncidentReason::GenerationExhausted, generation, now);
        }
        Occurrence { reason, generation, opened, exhausted }
    }

    /// Open `reason`'s incident or count it again. Whether it opened.
    fn note(&mut self, reason: IncidentReason, generation: i64, at: DateTime<Utc>) -> bool {
        match self.incidents.get_mut(&reason) {
            Some(open) => {
                let incident = &mut open.incident;
                incident.last_at = incident.last_at.max(at);
                incident.count += 1;
                incident.last_generation = incident.last_generation.max(generation);
                open.version += 1;
                false
            },
            None => {
                self.incidents.insert(reason, OpenIncident {
                    incident: Incident {
                        reason,
                        first_at: at,
                        last_at: at,
                        count: 1,
                        first_generation: generation,
                        last_generation: generation,
                    },
                    version: 1,
                    landed: 0,
                });
                true
            },
        }
    }

    /// Set `n`, for a test that needs the end of an epoch's generations.
    #[cfg(test)]
    pub(crate) fn set_counter(&mut self, counter: u32) {
        self.counter = counter;
    }

    /// Incidents whose upsert has not landed: entries of
    /// `persistence_pending`, at most one per reason.
    pub(crate) fn incidents_persisting(&self) -> usize {
        self.incidents.values().filter(|open| open.version > open.landed).count()
    }

    /// The upserts to write, each with the version it carries.
    pub(crate) fn incidents_to_write(&self) -> Vec<(Incident, u64)> {
        self.incidents
            .values()
            .filter(|open| open.version > open.landed)
            .map(|open| (open.incident.clone(), open.version))
            .collect()
    }

    /// The upsert carrying `version` of `reason`'s incident landed.
    pub(crate) fn incident_landed(&mut self, reason: IncidentReason, version: u64) {
        if let Some(open) = self.incidents.get_mut(&reason) {
            open.landed = open.landed.max(version);
        }
    }

    /// Whether the orphan sweep may not cancel an `ins-` order now (design
    /// §2.9): an incident is open, the fence is closed or the latch is set.
    pub(crate) fn vetoes_orphan_cancel(&self) -> bool {
        self.fenced || !self.incidents.is_empty() || self.latch.load(Ordering::SeqCst)
    }
}

/// The permit gate. One per journal, shared with the venue's transport.
pub struct OperationGate {
    state: Mutex<GateState>,
    /// The journal's latch.
    latch: Arc<AtomicBool>,
    /// Woken when the last permit is released.
    drained: Notify,
    observer: OnceLock<Arc<dyn GateObserver>>,
}

impl fmt::Debug for OperationGate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OperationGate").field("state", &*self.lock()).finish()
    }
}

impl OperationGate {
    pub(crate) fn new(latch: Arc<AtomicBool>) -> Self {
        Self {
            state: Mutex::new(GateState::new(Arc::clone(&latch))),
            latch,
            drained: Notify::new(),
            observer: OnceLock::new(),
        }
    }

    pub(crate) fn lock(&self) -> MutexGuard<'_, GateState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Export what the gate counts through `observer`; the first one stays.
    pub fn set_observer(&self, observer: Arc<dyn GateObserver>) {
        let _ = self.observer.set(observer);
    }

    pub fn phase(&self) -> Phase {
        self.lock().phase
    }

    pub fn snapshot(&self) -> GateSnapshot {
        let state = self.lock();
        GateSnapshot {
            phase: state.phase,
            fenced: state.fenced,
            generation: state.generation(),
            open_incidents: state.incidents.len() as u32,
            incidents_persisting: state.incidents_persisting() as u32,
            permits: state.permits,
            ordinary: state.ordinary,
            protective: state.protective,
            internal: state.internal.iter().filter(|held| **held).count() as u32,
            sends_started: state.sends_started,
            aborted: state.aborted,
            unpermitted: state.unpermitted,
            busy_ordinary: state.busy_ordinary,
            busy_protective: state.busy_protective,
        }
    }

    /// Whether the admission fence is closed.
    pub fn fenced(&self) -> bool {
        self.lock().fenced
    }

    /// The current generation, which a release must quote.
    pub fn generation(&self) -> i64 {
        self.lock().generation()
    }

    /// The open incidents as this process knows them: those loaded at start
    /// and every occurrence since, until a release covers them.
    pub fn open_incidents(&self) -> Vec<Incident> {
        self.lock().incidents.values().map(|open| open.incident.clone()).collect()
    }

    /// The epoch this start recorded, if it recorded one.
    pub fn epoch(&self) -> Option<EpochView> {
        self.lock().epoch
    }

    /// Record an occurrence of `reason` (design §2.7), for a caller holding no
    /// lock of this gate or the journal.
    pub fn record(&self, reason: IncidentReason) -> Occurrence {
        let occurrence = self.lock().occur(reason);
        occurrence.log();
        occurrence
    }

    /// Seed the generation and the incidents from this process's epoch,
    /// before any venue request (design §3). `unclean` is the generation the
    /// epoch's own transaction recorded an unclean start at, `epoch_seq << 32
    /// | 1`; `open` is every incident on record no release covered, that one
    /// included. Any open incident closes the fence.
    pub fn begin_epoch(&self, view: EpochView, unclean: Option<i64>, open: Vec<Incident>) {
        let mut state = self.lock();
        state.epoch_seq = view.epoch_seq;
        state.epoch = Some(view);
        if unclean.is_some() {
            state.counter = state.counter.max(1);
        }
        for incident in open {
            match state.incidents.get_mut(&incident.reason) {
                // Only an occurrence before the epoch was known: merge, and
                // write the merged row.
                Some(here) => {
                    let mine = &mut here.incident;
                    mine.first_at = mine.first_at.min(incident.first_at);
                    mine.last_at = mine.last_at.max(incident.last_at);
                    mine.first_generation = mine.first_generation.min(incident.first_generation);
                    mine.last_generation = mine.last_generation.max(incident.last_generation);
                    mine.count += incident.count;
                    here.version += 1;
                },
                None => {
                    state.incidents.insert(incident.reason, OpenIncident {
                        incident,
                        version: 1,
                        landed: 1,
                    });
                },
            }
        }
        if !state.incidents.is_empty() {
            state.fenced = true;
        }
    }

    /// After a release covering generations up to `released`: drop the
    /// incidents it released, and open the fence only if the generation is
    /// still `released` and the latch clear. Whether the fence is open.
    pub(crate) fn release_through(&self, released: i64) -> bool {
        let mut state = self.lock();
        state.incidents.retain(|_, open| open.incident.last_generation > released);
        if state.generation() == released
            && state.incidents.is_empty()
            && !self.latch.load(Ordering::SeqCst)
        {
            state.fenced = false;
        }
        !state.fenced
    }

    /// Whether the orphan sweep's cancels are withheld now (design §2.9).
    pub fn orphan_cancel_vetoed(&self) -> bool {
        self.lock().vetoes_orphan_cancel()
    }

    /// Refuse the ordinary class from now on.
    pub fn begin_draining(&self) {
        let mut state = self.lock();
        state.phase = state.phase.max(Phase::Draining);
    }

    /// Refuse every new operation from now on.
    pub fn begin_closing(&self) {
        self.lock().phase = Phase::Closing;
    }

    /// Wait until no permit is held, or `deadline`. Whether none is.
    pub async fn wait_no_permits(&self, deadline: Instant) -> bool {
        loop {
            let drained = self.drained.notified();
            tokio::pin!(drained);
            drained.as_mut().enable();
            if self.lock().permits == 0 {
                return true;
            }
            if tokio::time::timeout_at(deadline, drained).await.is_err() {
                return self.lock().permits == 0;
            }
        }
    }

    /// Take a slot of `class`. Nothing else changes when it is refused.
    pub(crate) fn acquire(&self, class: SlotClass) -> Result<(), PermitRefusal> {
        let mut state = self.lock();
        if state.phase >= class.refused_from() {
            return Err(PermitRefusal::Stopping(state.phase));
        }
        match class {
            SlotClass::Ordinary if state.ordinary >= ORDINARY_API_SLOTS => {
                state.busy_ordinary += 1;
                return Err(PermitRefusal::Busy(class));
            },
            SlotClass::Ordinary => state.ordinary += 1,
            SlotClass::Protective if state.protective >= PROTECTIVE_API_SLOTS => {
                state.busy_protective += 1;
                return Err(PermitRefusal::Busy(class));
            },
            SlotClass::Protective => state.protective += 1,
            SlotClass::Internal(slot) => {
                if state.internal[slot.index()] {
                    return Err(PermitRefusal::Busy(class));
                }
                state.internal[slot.index()] = true;
            },
        }
        state.permits += 1;
        Ok(())
    }

    pub(crate) fn release(&self, class: SlotClass) {
        let mut state = self.lock();
        match class {
            SlotClass::Ordinary => state.ordinary = state.ordinary.saturating_sub(1),
            SlotClass::Protective => state.protective = state.protective.saturating_sub(1),
            SlotClass::Internal(slot) => state.internal[slot.index()] = false,
        }
        state.permits = state.permits.saturating_sub(1);
        let none_left = state.permits == 0;
        drop(state);
        if none_left {
            self.drained.notify_waiters();
        }
    }

    /// A permit dropped without completion: whatever it was doing may have
    /// reached the venue with nothing left to record the answer.
    pub(crate) fn abort(&self, kind: OperationKind) {
        // Its slot is still held: the permit releases it only after this
        // returns, so a stop that sees no permit already sees the latch. Set
        // under the mutex with the count all the same.
        let mut state = self.lock();
        state.aborted += 1;
        let occurrence = state.occur(IncidentReason::OperationAborted);
        drop(state);
        error!(kind = kind.as_str(), "Operation dropped before completing; latch set");
        occurrence.log();
        if let Some(observer) = self.observer.get() {
            observer.operation_aborted(kind);
        }
    }

    /// The choke point, called by every signed venue mutation before it is
    /// sent. `what` names the request, for the log.
    ///
    /// # Errors
    /// Without a permit of this gate, once the daemon is stopping.
    pub fn mutation(&self, what: &str) -> Result<(), MutationRefused> {
        if order_journal::holds_permit_of(self) {
            return Ok(());
        }
        let mut state = self.lock();
        if state.phase != Phase::Open {
            return Err(MutationRefused::Stopping(state.phase));
        }
        // Counted and latched before the mutex that let it through is
        // released: a stop that moves the phase after this sees the latch,
        // however long this thread then pauses before its request.
        state.unpermitted += 1;
        let occurrence = state.occur(IncidentReason::UnpermittedMutation);
        drop(state);
        #[cfg(test)]
        tests::after_an_unpermitted_pass(self);
        warn!(what, "Venue mutation without an operation permit; latch set");
        occurrence.log();
        if let Some(observer) = self.observer.get() {
            observer.unpermitted_mutation();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    thread_local! {
        /// Run where an unpermitted mutation has been let through and its
        /// thread could pause before its request.
        static AFTER_AN_UNPERMITTED_PASS: Cell<Option<fn(&OperationGate)>> = const { Cell::new(None) };
    }

    pub(super) fn after_an_unpermitted_pass(gate: &OperationGate) {
        if let Some(hook) = AFTER_AN_UNPERMITTED_PASS.with(Cell::take) {
            hook(gate);
        }
    }

    fn gate() -> OperationGate {
        OperationGate::new(Arc::new(AtomicBool::new(false)))
    }

    #[test]
    fn every_class_has_its_own_slots() {
        let gate = gate();
        for _ in 0..ORDINARY_API_SLOTS {
            gate.acquire(SlotClass::Ordinary).unwrap();
        }
        assert_eq!(
            gate.acquire(SlotClass::Ordinary),
            Err(PermitRefusal::Busy(SlotClass::Ordinary))
        );
        // A full ordinary class takes nothing from the others.
        for _ in 0..PROTECTIVE_API_SLOTS {
            gate.acquire(SlotClass::Protective).unwrap();
        }
        assert_eq!(
            gate.acquire(SlotClass::Protective),
            Err(PermitRefusal::Busy(SlotClass::Protective))
        );
        for slot in [
            InternalSlot::MainLoop,
            InternalSlot::ReconciliationScan,
            InternalSlot::FundingPoll,
            InternalSlot::Startup,
        ] {
            gate.acquire(SlotClass::Internal(slot)).unwrap();
            assert_eq!(
                gate.acquire(SlotClass::Internal(slot)),
                Err(PermitRefusal::Busy(SlotClass::Internal(slot)))
            );
        }
        let snapshot = gate.snapshot();
        assert_eq!(snapshot.permits, ALLOCATED_SLOTS);
        assert_eq!((snapshot.busy_ordinary, snapshot.busy_protective), (1, 1));

        gate.release(SlotClass::Ordinary);
        gate.acquire(SlotClass::Ordinary).unwrap();
    }

    #[test]
    fn the_phases_refuse_ordinary_first_and_everything_last() {
        let gate = gate();
        gate.begin_draining();
        assert_eq!(
            gate.acquire(SlotClass::Ordinary),
            Err(PermitRefusal::Stopping(Phase::Draining))
        );
        gate.acquire(SlotClass::Protective).unwrap();
        gate.acquire(SlotClass::Internal(InternalSlot::MainLoop)).unwrap();

        gate.begin_closing();
        gate.begin_draining();
        assert_eq!(gate.phase(), Phase::Closing, "a phase never moves back");
        for class in [
            SlotClass::Ordinary,
            SlotClass::Protective,
            SlotClass::Internal(InternalSlot::ReconciliationScan),
        ] {
            assert_eq!(gate.acquire(class), Err(PermitRefusal::Stopping(Phase::Closing)));
        }
        // Refusals while stopping are not counted as busy.
        assert_eq!(gate.snapshot().busy_ordinary, 0);
    }

    #[test]
    fn the_scope_table_maps_each_operation_to_its_class() {
        use OperationKind::*;
        for kind in [Arm, Signal, Approve, FundingExecute, RecoverSpotUsdt] {
            assert_eq!(kind.class(), SlotClass::Ordinary, "{kind:?}");
        }
        for kind in [CancelOrClose, Panic, MonthlyHalt, ReconcileClose] {
            assert_eq!(kind.class(), SlotClass::Protective, "{kind:?}");
        }
        assert_eq!(MainLoop.class(), SlotClass::Internal(InternalSlot::MainLoop));
        assert_eq!(SignalListener.class(), SlotClass::Internal(InternalSlot::MainLoop));
        assert_eq!(
            ReconciliationScan.class(),
            SlotClass::Internal(InternalSlot::ReconciliationScan)
        );
        assert_eq!(FundingPoll.class(), SlotClass::Internal(InternalSlot::FundingPoll));
        assert_eq!(Startup.class(), SlotClass::Internal(InternalSlot::Startup));
    }

    #[test]
    fn an_unpermitted_mutation_is_counted_while_open_and_refused_after() {
        let latch = Arc::new(AtomicBool::new(false));
        let gate = OperationGate::new(Arc::clone(&latch));
        assert_eq!(gate.mutation("POST /order"), Ok(()));
        assert_eq!(gate.snapshot().unpermitted, 1);
        assert!(latch.load(Ordering::SeqCst));

        gate.begin_draining();
        assert_eq!(gate.mutation("POST /order"), Err(MutationRefused::Stopping(Phase::Draining)));
        gate.begin_closing();
        assert_eq!(gate.mutation("DELETE /order"), Err(MutationRefused::Stopping(Phase::Closing)));
        assert_eq!(gate.snapshot().unpermitted, 1, "a refused one is not sent or counted");
    }

    /// A stop that moves the phase while an unpermitted mutation, already let
    /// through, has yet to reach its request finds the latch set and the
    /// mutation counted: it can never mark the stop clean past it.
    #[test]
    fn a_stop_after_an_unpermitted_mutation_passed_sees_its_latch() {
        thread_local! {
            static SEEN: Cell<Option<(bool, u64)>> = const { Cell::new(None) };
        }
        AFTER_AN_UNPERMITTED_PASS.with(|hook| {
            hook.set(Some(|gate| {
                gate.begin_draining();
                gate.begin_closing();
                let seen = (gate.latch.load(Ordering::SeqCst), gate.snapshot().unpermitted);
                SEEN.with(|cell| cell.set(Some(seen)));
            }))
        });
        let gate = gate();
        assert_eq!(gate.mutation("POST /order"), Ok(()));
        assert_eq!(SEEN.with(Cell::get), Some((true, 1)), "the stop saw the latch and the count");
        assert_eq!(gate.snapshot().unpermitted, 1);
    }

    #[tokio::test(start_paused = true)]
    async fn waiting_for_no_permit_ends_at_the_last_release_or_the_deadline() {
        let gate = Arc::new(gate());
        assert!(gate.wait_no_permits(Instant::now()).await);

        gate.acquire(SlotClass::Protective).unwrap();
        let started = Instant::now();
        let released = {
            let gate = Arc::clone(&gate);
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                gate.release(SlotClass::Protective);
            })
        };
        assert!(gate.wait_no_permits(started + std::time::Duration::from_secs(10)).await);
        assert_eq!(started.elapsed(), std::time::Duration::from_secs(3));
        released.await.unwrap();

        gate.acquire(SlotClass::Protective).unwrap();
        let started = Instant::now();
        assert!(!gate.wait_no_permits(started + std::time::Duration::from_secs(10)).await);
        assert_eq!(started.elapsed(), std::time::Duration::from_secs(10));
    }
}
