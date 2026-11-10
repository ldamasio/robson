//! The two-phase stop (ADR-0056 step 7, PR S).
//!
//! SIGTERM or SIGINT starts it. Its clock starts at the signal: the stop ends
//! by [`STOP_DEADLINE`] after it, and the process exits by [`HARD_EXIT`] after
//! it whatever is still running, inside the 30 s Kubernetes gives the pod
//! before SIGKILL.
//!
//! 1. **Draining.** The ordinary API class is refused; ticks, heals, exits,
//!    sweeps, internal loops and protective API operations continue. It ends at
//!    the earlier of [`DRAIN_LIMIT`] and the first moment no permit is held.
//! 2. **Closing.** Every new operation is refused, and the main loop, the
//!    workers, the detectors and the API listener are cancelled. The operations
//!    already holding a permit finish, then the journal is flushed.
//! 3. **Marker.** Only a clean pass marks the epoch: every permit released in
//!    time, none aborted, the journal holding nothing unlanded and its latch
//!    clear. A deadline, an abort or a latch leaves the epoch unmarked, and the
//!    next start sees an unclean predecessor.
//!
//! The daemon stops waiting for anything once the stop has its result, and the
//! runtime's teardown gets what is left of the deadline. Signals are received
//! on a thread of their own, outside the daemon's runtime, which then waits out
//! [`HARD_EXIT`]: should the process still be alive then, it exits it with
//! [`HARD_EXIT_CODE`], writing no marker.
//!
//! The stop never takes the manager lock and never awaits under a sync lock.

use std::{
    sync::{Arc, OnceLock},
    time::Duration,
};

use async_trait::async_trait;
use robson_exec::order_journal::OrderJournal;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

/// Longest Draining.
pub const DRAIN_LIMIT: Duration = Duration::from_secs(12);
/// From the signal to the end of the stop, whatever is still running.
pub const STOP_DEADLINE: Duration = Duration::from_secs(25);
/// From the signal to the process's exit, whatever is still running.
pub const HARD_EXIT: Duration = Duration::from_secs(27);
/// The exit code at [`HARD_EXIT`] (`EX_SOFTWARE`).
pub const HARD_EXIT_CODE: i32 = 70;
/// Pause between flush passes that wrote nothing.
const FLUSH_RETRY_PAUSE: Duration = Duration::from_millis(100);

/// When this process received its stop signal.
static SIGNALLED_AT: OnceLock<std::time::Instant> = OnceLock::new();

/// The stop's bounds; tests shorten them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StopTimings {
    pub drain_limit: Duration,
    pub deadline: Duration,
}

impl Default for StopTimings {
    fn default() -> Self {
        Self {
            drain_limit: DRAIN_LIMIT,
            deadline: STOP_DEADLINE,
        }
    }
}

/// A request to stop, and when it was first made: the stop's clock starts
/// there, not when the stop gets to run.
#[derive(Debug, Clone, Default)]
pub struct StopRequest {
    token: CancellationToken,
    asked_at: Arc<OnceLock<Instant>>,
}

impl StopRequest {
    /// Ask for the stop at `at`; a later ask keeps the first time.
    pub fn ask_at(&self, at: Instant) {
        self.asked_at.get_or_init(|| at);
        self.token.cancel();
    }

    /// Ask for the stop now.
    pub fn ask(&self) {
        self.ask_at(Instant::now());
    }

    pub fn is_asked(&self) -> bool {
        self.token.is_cancelled()
    }

    /// Wait for the stop to be asked for. When it was.
    pub async fn asked(&self) -> Instant {
        self.token.cancelled().await;
        *self.asked_at.get_or_init(Instant::now)
    }
}

/// A stop signal received at `at`: from here, the runtime's teardown gets what
/// is left of the deadline.
pub fn signal_received(at: std::time::Instant) {
    let _ = SIGNALLED_AT.set(at);
}

/// Wait out the hard bound of a stop signal received at `at` on the calling
/// thread, then exit the process, unclean, unless it has exited by then. Call
/// it on a thread of its own: nothing the async runtime does can hold it back.
pub fn hard_exit_after(at: std::time::Instant) -> ! {
    let exit_at = at + HARD_EXIT;
    loop {
        let now = std::time::Instant::now();
        if now >= exit_at {
            break;
        }
        std::thread::sleep(exit_at - now);
    }
    error!(
        after_ms = HARD_EXIT.as_millis(),
        code = HARD_EXIT_CODE,
        "Stop: hard bound reached with the process still running; exiting unclean, epoch left \
         unmarked"
    );
    std::process::exit(HARD_EXIT_CODE)
}

/// How long the runtime's teardown may take: what is left of the stop's
/// deadline after a signal, one whole deadline without one.
pub fn teardown_bound() -> Duration {
    match SIGNALLED_AT.get() {
        Some(at) => (*at + STOP_DEADLINE).saturating_duration_since(std::time::Instant::now()),
        None => STOP_DEADLINE,
    }
}

/// How a stop ended. Only `Clean` writes the marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopResult {
    Clean,
    /// Permits, the flush or the marker were still going at the deadline.
    Deadline,
    /// An operation was dropped before it completed.
    Aborted,
    /// The journal's latch is set.
    Latched,
    /// The journal still holds something that has not landed.
    Unlanded,
    /// The marker could not be written.
    MarkerFailed,
}

impl StopResult {
    pub fn label(self) -> &'static str {
        match self {
            Self::Clean => "clean",
            Self::Deadline => "deadline",
            Self::Aborted => "aborted",
            Self::Latched => "latched",
            Self::Unlanded => "unlanded",
            Self::MarkerFailed => "marker_failed",
        }
    }
}

/// What a stop did and how long each phase took.
#[derive(Debug, Clone, Copy)]
pub struct StopReport {
    pub result: StopResult,
    /// From the request to the end of Draining.
    pub draining: Duration,
    /// From the end of Draining to the result.
    pub closing: Duration,
    /// Where the absolute deadline falls, for what runs after the stop.
    pub deadline: Instant,
}

/// Writes this epoch's clean-stop marker.
#[async_trait]
pub trait StopMarker: Send + Sync {
    async fn mark_clean(&self) -> Result<(), String>;
}

/// Wait for `stop`, then run the stop from when it was asked for. `close`
/// cancels the loops, workers, detectors and listener as Closing begins.
pub async fn stop_when_asked(
    stop: StopRequest,
    journal: Arc<OrderJournal>,
    close: impl FnOnce(),
    marker: Option<Box<dyn StopMarker>>,
    timings: StopTimings,
) -> StopReport {
    let asked_at = stop.asked().await;
    info!(waited_ms = asked_at.elapsed().as_millis(), "Stop requested: draining");
    let report = run_stop(&journal, asked_at, close, marker.as_deref(), timings).await;
    crate::metrics::GRACEFUL_STOP.with_label_values(&[report.result.label()]).inc();
    crate::metrics::observe_order_journal(&journal);
    let (draining_ms, closing_ms) = (report.draining.as_millis(), report.closing.as_millis());
    match report.result {
        StopResult::Clean => info!(draining_ms, closing_ms, "Stop: clean, epoch marked"),
        result => warn!(
            draining_ms,
            closing_ms,
            result = result.label(),
            "Stop: not clean, epoch left unmarked"
        ),
    }
    report
}

/// The stop itself, its clock started at `asked_at`.
pub async fn run_stop(
    journal: &OrderJournal,
    asked_at: Instant,
    close: impl FnOnce(),
    marker: Option<&dyn StopMarker>,
    timings: StopTimings,
) -> StopReport {
    let deadline = asked_at + timings.deadline;
    let gate = journal.gate();

    gate.begin_draining();
    gate.wait_no_permits((asked_at + timings.drain_limit).min(deadline)).await;
    let draining = asked_at.elapsed();

    gate.begin_closing();
    info!(draining_ms = draining.as_millis(), "Stop: closing");
    close();
    let result = closing(journal, marker, deadline).await;
    StopReport {
        result,
        draining,
        closing: asked_at.elapsed().saturating_sub(draining),
        deadline,
    }
}

async fn closing(
    journal: &OrderJournal,
    marker: Option<&dyn StopMarker>,
    deadline: Instant,
) -> StopResult {
    if !journal.gate().wait_no_permits(deadline).await {
        return StopResult::Deadline;
    }
    // The final flush: every pass is bounded, and a database that takes
    // nothing is retried until the deadline.
    while journal.retry_set_depth() > 0 {
        match tokio::time::timeout_at(deadline, journal.flush_pending()).await {
            Err(_) => return StopResult::Deadline,
            Ok(0) => {
                tokio::time::sleep_until((Instant::now() + FLUSH_RETRY_PAUSE).min(deadline)).await;
                if Instant::now() >= deadline {
                    return StopResult::Deadline;
                }
            },
            Ok(_) => {},
        }
    }
    if journal.gate().snapshot().aborted > 0 {
        return StopResult::Aborted;
    }
    if journal.latch_set() {
        return StopResult::Latched;
    }
    if !journal.is_quiescent() {
        return StopResult::Unlanded;
    }
    let Some(marker) = marker else {
        return StopResult::Clean;
    };
    robson_exec::crash_point!(STOP_BEFORE_MARKER);
    let marked = match tokio::time::timeout_at(deadline, marker.mark_clean()).await {
        // The update may still commit, or have committed with its answer
        // lost: every check above passed, so the marker it leaves is true.
        Err(_) => StopResult::Deadline,
        Ok(Err(error)) => {
            warn!(%error, "Stop: the clean-stop marker was not written");
            StopResult::MarkerFailed
        },
        Ok(Ok(())) => StopResult::Clean,
    };
    robson_exec::crash_point!(STOP_AFTER_MARKER);
    marked
}

#[cfg(test)]
mod tests {
    use std::sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Mutex,
    };

    use robson_exec::{
        gate::{OperationKind, PermitRefusal, Phase},
        order_journal::{
            IntentKind, IntentSpec, JournalFault, OrderJournal, StubJournalBackend, WriteClass,
        },
        venue::VenueId,
    };

    use super::*;

    #[derive(Default)]
    struct Marker {
        marked: AtomicUsize,
        fails: bool,
        hangs: bool,
    }

    #[async_trait]
    impl StopMarker for Marker {
        async fn mark_clean(&self) -> Result<(), String> {
            if self.hangs {
                std::future::pending::<()>().await;
            }
            self.marked.fetch_add(1, Ordering::SeqCst);
            if self.fails {
                Err("refused".to_string())
            } else {
                Ok(())
            }
        }
    }

    fn stub_journal() -> (Arc<StubJournalBackend>, Arc<OrderJournal>) {
        let backend = Arc::new(StubJournalBackend::default());
        let journal = OrderJournal::new(backend.clone(), &VenueId::new("test:venue"))
            .with_commit_bound(Duration::from_millis(50));
        (backend, Arc::new(journal))
    }

    fn secs(seconds: u64) -> Duration {
        Duration::from_secs(seconds)
    }

    /// An idle daemon: Draining ends at once, nothing to flush, epoch marked.
    #[tokio::test(start_paused = true)]
    async fn an_idle_stop_is_clean_at_once_and_marks_the_epoch() {
        let (_backend, journal) = stub_journal();
        let marker = Marker::default();
        let closed = AtomicBool::new(false);
        let report = run_stop(
            &journal,
            Instant::now(),
            || closed.store(true, Ordering::SeqCst),
            Some(&marker),
            StopTimings::default(),
        )
        .await;
        assert_eq!(report.result, StopResult::Clean);
        assert_eq!((report.draining, report.closing), (Duration::ZERO, Duration::ZERO));
        assert_eq!(marker.marked.load(Ordering::SeqCst), 1);
        assert!(closed.load(Ordering::SeqCst));
        assert_eq!(journal.gate().phase(), Phase::Closing);
    }

    /// A protective close in flight: Draining waits for it and no longer,
    /// refusing ordinary work meanwhile and nothing else.
    #[tokio::test(start_paused = true)]
    async fn draining_lasts_until_the_last_permit_and_refuses_only_ordinary_work() {
        let (_backend, journal) = stub_journal();
        let close = journal.acquire(OperationKind::CancelOrClose).unwrap();
        let during = {
            let journal = Arc::clone(&journal);
            tokio::spawn(async move {
                tokio::time::sleep(secs(5)).await;
                assert_eq!(
                    journal.acquire(OperationKind::Arm).err(),
                    Some(PermitRefusal::Stopping(Phase::Draining))
                );
                journal.acquire(OperationKind::Panic).unwrap().complete();
                journal.acquire(OperationKind::MainLoop).unwrap().complete();
                close.complete();
            })
        };
        let marker = Marker::default();
        let report =
            run_stop(&journal, Instant::now(), || {}, Some(&marker), StopTimings::default()).await;
        during.await.unwrap();
        assert_eq!(report.result, StopResult::Clean);
        assert_eq!((report.draining, report.closing), (secs(5), Duration::ZERO));
    }

    /// Draining stops at its limit; Closing waits for what is in flight,
    /// refuses every new operation, and ends clean when that work ends.
    #[tokio::test(start_paused = true)]
    async fn closing_waits_for_the_operations_in_flight_and_refuses_new_ones() {
        let (_backend, journal) = stub_journal();
        let scan = journal.acquire(OperationKind::ReconciliationScan).unwrap();
        let closed_at = Arc::new(Mutex::new(None));
        let during = {
            let journal = Arc::clone(&journal);
            tokio::spawn(async move {
                tokio::time::sleep(secs(20)).await;
                for kind in [
                    OperationKind::Panic,
                    OperationKind::MainLoop,
                    OperationKind::Arm,
                ] {
                    assert_eq!(
                        journal.acquire(kind).err(),
                        Some(PermitRefusal::Stopping(Phase::Closing))
                    );
                }
                scan.complete();
            })
        };
        let started = Instant::now();
        let at = Arc::clone(&closed_at);
        let marker = Marker::default();
        let report = run_stop(
            &journal,
            Instant::now(),
            move || *at.lock().unwrap() = Some(started.elapsed()),
            Some(&marker),
            StopTimings::default(),
        )
        .await;
        during.await.unwrap();
        assert_eq!(report.result, StopResult::Clean);
        assert_eq!((report.draining, report.closing), (DRAIN_LIMIT, secs(8)));
        assert_eq!(*closed_at.lock().unwrap(), Some(DRAIN_LIMIT), "cancelled as Closing began");
        assert_eq!(marker.marked.load(Ordering::SeqCst), 1);
    }

    /// An operation still running at the deadline: the stop ends there and
    /// the epoch stays unmarked.
    #[tokio::test(start_paused = true)]
    async fn the_deadline_ends_an_unclean_stop_without_a_marker() {
        let (_backend, journal) = stub_journal();
        let _stuck = journal.acquire(OperationKind::Panic).unwrap();
        let marker = Marker::default();
        let started = Instant::now();
        let report =
            run_stop(&journal, Instant::now(), || {}, Some(&marker), StopTimings::default()).await;
        assert_eq!(report.result, StopResult::Deadline);
        assert_eq!((report.draining, report.closing), (DRAIN_LIMIT, STOP_DEADLINE - DRAIN_LIMIT));
        assert_eq!(started.elapsed(), STOP_DEADLINE);
        assert_eq!(marker.marked.load(Ordering::SeqCst), 0);
    }

    /// An aborted permit, or the latch, leaves the epoch unmarked.
    #[tokio::test(start_paused = true)]
    async fn an_aborted_permit_or_the_latch_leaves_the_epoch_unmarked() {
        let (_backend, journal) = stub_journal();
        drop(journal.acquire(OperationKind::Arm).unwrap());
        let marker = Marker::default();
        let report =
            run_stop(&journal, Instant::now(), || {}, Some(&marker), StopTimings::default()).await;
        assert_eq!(report.result, StopResult::Aborted);

        let (_backend, journal) = journal_with_latch();
        let report =
            run_stop(&journal, Instant::now(), || {}, Some(&marker), StopTimings::default()).await;
        assert_eq!(report.result, StopResult::Latched);
        assert_eq!(marker.marked.load(Ordering::SeqCst), 0);
    }

    fn journal_with_latch() -> (Arc<StubJournalBackend>, Arc<OrderJournal>) {
        let (backend, journal) = stub_journal();
        journal.gate().mutation("POST /fapi/v1/order").unwrap();
        assert!(journal.latch_set());
        (backend, journal)
    }

    /// Leave one result write in the retry set.
    async fn unlanded_result(backend: &StubJournalBackend, journal: &Arc<OrderJournal>) {
        let operation = journal.begin_operation();
        operation
            .scope(async {
                let intent = journal.open_intent(IntentSpec {
                    position_id: None,
                    kind: IntentKind::Exit,
                    client_order_id: "x".to_string(),
                    cancels_order_id: None,
                    params: serde_json::Value::Null,
                });
                journal.before_send(&intent, WriteClass::Bounded).await.unwrap();
                backend.set_fail_all(Some(JournalFault::Known));
                journal.finish_success(&intent, None).await;
            })
            .await;
        assert_eq!(journal.retry_set_depth(), 1);
        assert!(!journal.latch_set(), "a result in the retry set is not lost");
    }

    /// The final flush: a result that lands during the stop lets it end clean;
    /// one that never lands holds it to the deadline, unmarked.
    #[tokio::test(start_paused = true)]
    async fn the_final_flush_must_land_before_the_marker() {
        let (backend, journal) = stub_journal();
        unlanded_result(&backend, &journal).await;
        let recovers = {
            let backend = Arc::clone(&backend);
            tokio::spawn(async move {
                tokio::time::sleep(secs(3)).await;
                backend.set_fail_all(None);
            })
        };
        let marker = Marker::default();
        let report =
            run_stop(&journal, Instant::now(), || {}, Some(&marker), StopTimings::default()).await;
        recovers.await.unwrap();
        assert_eq!(report.result, StopResult::Clean);
        assert!(report.closing >= secs(3) && report.closing < secs(4), "{report:?}");
        assert_eq!(journal.retry_set_depth(), 0);

        let (backend, journal) = stub_journal();
        unlanded_result(&backend, &journal).await;
        let report =
            run_stop(&journal, Instant::now(), || {}, Some(&marker), StopTimings::default()).await;
        assert_eq!(report.result, StopResult::Deadline);
        assert_eq!(report.draining + report.closing, STOP_DEADLINE);
        assert_eq!(marker.marked.load(Ordering::SeqCst), 1, "only the first stop marked");
    }

    /// Something still in the journal's memory, from an operation that holds
    /// no permit: not quiescent, so not clean.
    #[tokio::test(start_paused = true)]
    async fn an_intent_still_in_memory_leaves_the_epoch_unmarked() {
        let (_backend, journal) = stub_journal();
        let operation = journal.begin_operation();
        operation
            .scope(async {
                journal.open_intent(IntentSpec {
                    position_id: None,
                    kind: IntentKind::Exit,
                    client_order_id: "open".to_string(),
                    cancels_order_id: None,
                    params: serde_json::Value::Null,
                })
            })
            .await;
        let marker = Marker::default();
        let report =
            run_stop(&journal, Instant::now(), || {}, Some(&marker), StopTimings::default()).await;
        assert_eq!(report.result, StopResult::Unlanded);
        assert_eq!(marker.marked.load(Ordering::SeqCst), 0);
        drop(operation);
    }

    /// A marker the database refuses, or never answers, is not a clean stop.
    #[tokio::test(start_paused = true)]
    async fn a_marker_that_fails_or_hangs_is_not_a_clean_stop() {
        let (_backend, journal) = stub_journal();
        let failing = Marker { fails: true, ..Marker::default() };
        let report =
            run_stop(&journal, Instant::now(), || {}, Some(&failing), StopTimings::default()).await;
        assert_eq!(report.result, StopResult::MarkerFailed);

        let (_backend, journal) = stub_journal();
        let hanging = Marker { hangs: true, ..Marker::default() };
        let report =
            run_stop(&journal, Instant::now(), || {}, Some(&hanging), StopTimings::default()).await;
        assert_eq!(report.result, StopResult::Deadline);
        assert_eq!(report.draining + report.closing, STOP_DEADLINE);
    }

    /// The clock starts at the request: a stop that gets to run late, as when
    /// the signal came during startup, has only what is left of its deadline.
    #[tokio::test(start_paused = true)]
    async fn the_stop_clock_starts_at_the_request_not_when_the_stop_runs() {
        let (_backend, journal) = stub_journal();
        let _stuck = journal.acquire(OperationKind::Panic).unwrap();
        let stop = StopRequest::default();
        stop.ask();
        let asked_at = Instant::now();
        tokio::time::advance(secs(10)).await;
        stop.ask();
        assert_eq!(stop.asked().await, asked_at, "a second ask keeps the first time");

        let running = Instant::now();
        let marker = Marker::default();
        let report = stop_when_asked(
            stop,
            Arc::clone(&journal),
            || {},
            Some(Box::new(marker)),
            StopTimings::default(),
        )
        .await;
        assert_eq!(report.result, StopResult::Deadline);
        assert_eq!(report.deadline, asked_at + STOP_DEADLINE);
        assert_eq!(running.elapsed(), STOP_DEADLINE - secs(10));
        assert_eq!((report.draining, report.closing), (DRAIN_LIMIT, STOP_DEADLINE - DRAIN_LIMIT));
    }

    /// Without a signal, teardown gets one whole deadline.
    #[test]
    fn teardown_without_a_signal_gets_a_whole_deadline() {
        assert!(SIGNALLED_AT.get().is_none(), "no test raises a stop signal");
        assert_eq!(teardown_bound(), STOP_DEADLINE);
    }

    /// 25 s for the stop and 27 s for the process leave the pod's 30 s grace
    /// a margin before SIGKILL, and Draining is the shorter part of the stop.
    #[test]
    fn the_deadline_and_the_hard_bound_fit_inside_the_grace_period() {
        assert_eq!((DRAIN_LIMIT, STOP_DEADLINE, HARD_EXIT), (secs(12), secs(25), secs(27)));
        assert!(DRAIN_LIMIT < STOP_DEADLINE && STOP_DEADLINE < HARD_EXIT);
        assert!(HARD_EXIT + secs(3) <= secs(30));
        assert_ne!(HARD_EXIT_CODE, 0);
    }
}
