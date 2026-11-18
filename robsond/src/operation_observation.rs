//! Fixed-size whole-scan observations. Query attempts are not wire
//! acknowledgements.
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc, LazyLock,
};

use prometheus::{
    register_histogram, register_int_counter, register_int_counter_vec, register_int_gauge,
    Histogram, IntCounter, IntCounterVec, IntGauge,
};
use tracing::{
    field::{Field, Visit},
    span::{Attributes, Id, Record},
    Subscriber,
};
use tracing_subscriber::{
    layer::{Context, Layer},
    registry::LookupSpan,
};

const REVIEW_THRESHOLD: u64 = 15;
const ENDPOINTS: [&str; 10] = [
    "inventory",
    "conditional",
    "order",
    "open_orders",
    "trades",
    "wallet",
    "price",
    "rules",
    "klines",
    "other",
];
#[derive(Clone, Copy)]
#[repr(u64)]
enum IoKind {
    Query          = 1,
    Begin          = 2,
    Commit         = 3,
    RollbackQueued = 4,
    Http           = 5,
    Local          = 6,
    Incomplete     = 7,
}
impl IoKind {
    fn from_code(value: u64) -> Option<Self> {
        match value {
            1 => Some(Self::Query),
            2 => Some(Self::Begin),
            3 => Some(Self::Commit),
            4 => Some(Self::RollbackQueued),
            5 => Some(Self::Http),
            6 => Some(Self::Local),
            7 => Some(Self::Incomplete),
            _ => None,
        }
    }
}
static OVER: LazyLock<IntCounter> = LazyLock::new(|| {
    register_int_counter!(
        "robsond_reconciliation_work_over_threshold_total",
        "Scans crossing DB attempts plus twice HTTP submissions > 15"
    )
    .unwrap()
});
static INCOMPLETE: LazyLock<IntCounter> = LazyLock::new(|| {
    register_int_counter!(
        "robsond_reconciliation_work_incomplete_total",
        "Scans with incomplete attribution or saturated counters"
    )
    .unwrap()
});
static PENDING: LazyLock<IntGauge> = LazyLock::new(|| {
    register_int_gauge!(
        "robsond_reconciliation_work_pending",
        "Observations retained by foreground or existing pending work"
    )
    .unwrap()
});
static WORK: LazyLock<Histogram> = LazyLock::new(|| {
    register_histogram!(
        "robsond_reconciliation_work",
        "Observed scan work including late work; incomplete observations are explicitly counted",
        vec![0., 1., 2., 4., 8., 15., 16., 24., 32., 64., 128., 256., 1024.]
    )
    .unwrap()
});
static DB: LazyLock<IntCounter> = LazyLock::new(|| {
    register_int_counter!(
        "robsond_reconciliation_db_attempts_total",
        "Reached public SQLx query and transaction attempts"
    )
    .unwrap()
});
static HTTP: LazyLock<IntCounter> = LazyLock::new(|| {
    register_int_counter!(
        "robsond_reconciliation_http_submissions_total",
        "Real single-shot HTTP submissions; not venue receipt proof"
    )
    .unwrap()
});
static LOCAL: LazyLock<IntCounter> = LazyLock::new(|| {
    register_int_counter!(
        "robsond_reconciliation_local_calls_total",
        "Reached local repository operations, excluded from network score"
    )
    .unwrap()
});

#[derive(Default)]
struct Counts {
    #[cfg(test)]
    db_kinds: [AtomicU64; 4],
    db: AtomicU64,
    http: AtomicU64,
    local: AtomicU64,
    score: AtomicU64,
    over: AtomicBool,
    incomplete: AtomicBool,
    outcome: AtomicU64,
}
impl Counts {
    fn incomplete(&self) {
        if !self.incomplete.swap(true, Ordering::Relaxed) {
            INCOMPLETE.inc();
        }
    }
    fn add(&self, counter: &AtomicU64, amount: u64) -> u64 {
        let old = counter
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| Some(n.saturating_add(amount)))
            .unwrap_or_else(|n| n);
        if old.checked_add(amount).is_none() {
            self.incomplete();
        }
        old.saturating_add(amount)
    }
    fn attempt(&self, kind: u64, endpoint: Option<u64>) {
        #[cfg(test)]
        if (1..=4).contains(&kind) {
            self.db_kinds[(kind - 1) as usize].fetch_add(1, Ordering::Relaxed);
        }
        let weight = match IoKind::from_code(kind) {
            Some(IoKind::Query | IoKind::Begin | IoKind::Commit | IoKind::RollbackQueued) => {
                self.add(&self.db, 1);
                DB.inc();
                1
            },
            Some(IoKind::Http) => {
                self.add(&self.http, 1);
                HTTP.inc();
                let index = endpoint.and_then(|v| usize::try_from(v).ok()).filter(|v| *v < 9);
                if index.is_none() {
                    self.incomplete();
                }
                HTTP_ENDPOINT.with_label_values(&[ENDPOINTS[index.unwrap_or(9)]]).inc();
                2
            },
            Some(IoKind::Local) => {
                self.add(&self.local, 1);
                LOCAL.inc();
                0
            },
            Some(IoKind::Incomplete) => {
                self.incomplete();
                0
            },
            _ => {
                self.incomplete();
                0
            },
        };
        let score = self.add(&self.score, weight);
        if score > REVIEW_THRESHOLD && !self.over.swap(true, Ordering::Relaxed) {
            OVER.inc();
            tracing::warn!(target: "robsond::operation_summary", score, threshold = REVIEW_THRESHOLD, "Reconciliation work requires cardinality review and optimization");
        }
    }
}
struct Root(Arc<Counts>);
struct Attempt {
    owner: Arc<Counts>,
    kind: u64,
    endpoint: Option<u64>,
    reached: AtomicBool,
}
#[derive(Default)]
struct Fields {
    kind: Option<u64>,
    endpoint: Option<u64>,
    outcome: Option<u64>,
}
impl Visit for Fields {
    fn record_u64(&mut self, field: &Field, value: u64) {
        match field.name() {
            "io_kind" => self.kind = Some(value),
            "io_endpoint" => self.endpoint = Some(value),
            "outcome" => self.outcome = Some(value),
            _ => {},
        }
    }
    fn record_debug(&mut self, _: &Field, _: &dyn std::fmt::Debug) {}
}
static HTTP_ENDPOINT: LazyLock<IntCounterVec> = LazyLock::new(|| {
    register_int_counter_vec!(
        "robsond_reconciliation_http_endpoint_total",
        "Real submissions by fixed path class; exchange guardrail weight is two",
        &["endpoint"]
    )
    .unwrap()
});
static OUTCOMES: LazyLock<IntCounterVec> = LazyLock::new(|| {
    register_int_counter_vec!(
        "robsond_reconciliation_observations_total",
        "Final observed outcome; does not establish financial acceptance",
        &["outcome"]
    )
    .unwrap()
});
/// Materialize zero counters before a first crossing, independently of logs.
pub fn initialize_metrics() {
    LazyLock::force(&OVER);
    LazyLock::force(&INCOMPLETE);
    LazyLock::force(&PENDING);
    LazyLock::force(&WORK);
    LazyLock::force(&DB);
    LazyLock::force(&HTTP);
    LazyLock::force(&LOCAL);
    for endpoint in ENDPOINTS {
        HTTP_ENDPOINT.with_label_values(&[endpoint]);
    }
    for outcome in ["success", "error", "cancelled"] {
        OUTCOMES.with_label_values(&[outcome]);
    }
}
/// Independent of the text log filter. No SQL/URL/identifier fields are
/// visited.
pub struct OperationLayer;
/// Only observation callsites enable this layer; unrelated debug/SQL logging
/// keeps its existing text-filter decision and does not become globally
/// enabled.
pub fn layer<S>() -> impl Layer<S>
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    OperationLayer.with_filter(tracing_subscriber::filter::filter_fn(|metadata| {
        metadata.target() == "robson_operation"
    }))
}
impl<S: Subscriber + for<'a> LookupSpan<'a>> Layer<S> for OperationLayer {
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        let Some(span) = ctx.span(id) else {
            return;
        };
        if attrs.metadata().target() != "robson_operation" {
            return;
        }
        if attrs.metadata().name() == "reconciliation_work" {
            PENDING.inc();
            span.extensions_mut().insert(Root(Arc::new(Counts::default())));
            return;
        }
        let mut fields = Fields::default();
        attrs.record(&mut fields);
        let Some(kind) = fields.kind else {
            return;
        };
        let owner = span
            .scope()
            .skip(1)
            .find_map(|parent| parent.extensions().get::<Root>().map(|root| Arc::clone(&root.0)));
        if let Some(owner) = owner {
            span.extensions_mut().insert(Attempt {
                owner,
                kind,
                endpoint: fields.endpoint,
                reached: AtomicBool::new(false),
            });
        }
    }
    fn on_enter(&self, id: &Id, ctx: Context<'_, S>) {
        let Some(span) = ctx.span(id) else {
            return;
        };
        let extensions = span.extensions();
        if let Some(attempt) = extensions.get::<Attempt>() {
            if !attempt.reached.swap(true, Ordering::Relaxed) {
                attempt.owner.attempt(attempt.kind, attempt.endpoint);
            }
        }
    }
    fn on_record(&self, id: &Id, values: &Record<'_>, ctx: Context<'_, S>) {
        let Some(span) = ctx.span(id) else {
            return;
        };
        let mut fields = Fields::default();
        values.record(&mut fields);
        let extensions = span.extensions();
        if let (Some(root), Some(outcome)) = (extensions.get::<Root>(), fields.outcome) {
            root.0.outcome.store(outcome, Ordering::Relaxed);
        }
    }
    fn on_close(&self, id: Id, ctx: Context<'_, S>) {
        let Some(span) = ctx.span(&id) else {
            return;
        };
        let extensions = span.extensions();
        if let Some(root) = extensions.get::<Root>() {
            let counts = &root.0;
            if counts.outcome.load(Ordering::Relaxed) == 0 {
                counts.incomplete();
            }
            OUTCOMES
                .with_label_values(&[match counts.outcome.load(Ordering::Relaxed) {
                    1 => "success",
                    2 => "error",
                    _ => "cancelled",
                }])
                .inc();
            // Prometheus requires f64; this is a non-financial integer work count.
            #[allow(clippy::disallowed_types)]
            let work_count_for_histogram = counts.score.load(Ordering::Relaxed) as f64;
            WORK.observe(work_count_for_histogram);
            PENDING.dec();
            tracing::info!(target: "robsond::operation_summary", db_attempts = counts.db.load(Ordering::Relaxed), http_submissions = counts.http.load(Ordering::Relaxed), local_calls = counts.local.load(Ordering::Relaxed), incomplete = counts.incomplete.load(Ordering::Relaxed), outcome = counts.outcome.load(Ordering::Relaxed), "Reconciliation observation ended after its retained work");
        }
    }
}

#[cfg(all(test, any(feature = "postgres", feature = "crash-points")))]
pub(crate) mod probe {
    use std::sync::Mutex;

    use tracing_subscriber::prelude::*;

    use super::*;
    type Latest = Arc<Mutex<Option<Arc<Counts>>>>;
    #[derive(Clone)]
    struct Capture(Latest);
    impl<S: Subscriber + for<'a> LookupSpan<'a>> Layer<S> for Capture {
        fn on_new_span(&self, _: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
            if let Some(span) = ctx.span(id) {
                if let Some(root) = span.extensions().get::<Root>() {
                    *self.0.lock().unwrap() = Some(Arc::clone(&root.0));
                }
            }
        }
    }
    pub(crate) struct Probe {
        dispatch: tracing::Dispatch,
        // Retain a second registered dispatcher until after the real one drops.
        // It is never installed as a default and enables no callsite.
        _registration_guard: tracing::Dispatch,
        latest: Latest,
    }
    impl Probe {
        pub(crate) fn new() -> Self {
            let latest = Arc::new(Mutex::new(None));
            // tracing-core 0.1.36's JustOne callsite path asks the registering
            // thread's default. An unobserved peer can otherwise cache NEVER
            // while this local probe is the only registered dispatcher.
            let registration_guard =
                tracing::Dispatch::new(tracing::subscriber::NoSubscriber::default());
            let subscriber =
                tracing_subscriber::registry().with(layer()).with(Capture(Arc::clone(&latest)));
            Self {
                dispatch: tracing::Dispatch::new(subscriber),
                _registration_guard: registration_guard,
                latest,
            }
        }
        pub(crate) fn dispatch(&self) -> tracing::Dispatch {
            self.dispatch.clone()
        }
        /// Drop the probe after first releasing its captured Counts. A peer
        /// test thread rebuilding callsite interest can briefly upgrade the
        /// globally registered (weak) dispatcher, keeping this subscriber and
        /// its Capture alive for an instant after the probe drops; clearing
        /// the capture first means it no longer holds the observed origin, so
        /// strong-count ownership checks see only the test's own reference.
        pub(crate) fn release(self) {
            self.latest.lock().unwrap().take();
        }
        pub(crate) fn snapshot(&self) -> [u64; 9] {
            let guard = self.latest.lock().unwrap();
            let c = guard.as_ref().expect("real observation root");
            [
                c.db.load(Ordering::Relaxed),
                c.http.load(Ordering::Relaxed),
                c.local.load(Ordering::Relaxed),
                c.score.load(Ordering::Relaxed),
                u64::from(c.incomplete.load(Ordering::Relaxed)),
                c.db_kinds[0].load(Ordering::Relaxed),
                c.db_kinds[1].load(Ordering::Relaxed),
                c.db_kinds[2].load(Ordering::Relaxed),
                c.db_kinds[3].load(Ordering::Relaxed),
            ]
        }
    }

    #[cfg(test)]
    mod tests {
        use tracing::{instrument::WithSubscriber, Instrument};

        use super::*;

        // One new callsite, shared by the unobserved first touch and the real
        // polled future. No SQL is executed or replayed by this regression.
        fn boundary() -> tracing::Span {
            tracing::info_span!(target: "robson_operation", "operation_io", io_kind = 1_u64)
        }

        #[tokio::test]
        async fn callsite_first_registered_without_subscriber_is_counted_when_polled() {
            let probe = Probe::new();
            std::thread::spawn(|| {
                tracing::dispatcher::get_default(|dispatch| {
                    assert!(dispatch.is::<tracing::subscriber::NoSubscriber>());
                });
                assert!(boundary().is_disabled(), "first touch has no subscriber");
            })
            .join()
            .expect("unobserved registration thread joined");

            let polled = AtomicU64::new(0);
            robson_exec::observation::reconciliation(async {
                async {
                    polled.fetch_add(1, Ordering::Relaxed);
                }
                .instrument(boundary())
                .await;
                tracing::Span::current().record("outcome", 1_u64);
            })
            .with_subscriber(probe.dispatch())
            .await;

            assert_eq!(polled.load(Ordering::Relaxed), 1, "real future body polled once");
            assert_eq!(
                probe.snapshot(),
                [1, 0, 0, 1, 0, 1, 0, 0, 0],
                "the one reached boundary remains observed after an unobserved first registration"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    include!("operation_observation_heap_tests.rs");
    include!("operation_observation_retry_tests.rs");

    use std::{
        future::{poll_fn, Future},
        task::Poll,
    };

    use tracing::{instrument::WithSubscriber, Instrument};
    use tracing_subscriber::prelude::*;

    use super::*;

    fn current() -> Arc<Counts> {
        tracing::Span::with_subscriber(&tracing::Span::current(), |(id, dispatch)| {
            let registry = dispatch.downcast_ref::<tracing_subscriber::Registry>().unwrap();
            let span = registry.span(id).unwrap();
            span.scope()
                .find_map(|s| s.extensions().get::<Root>().map(|r| Arc::clone(&r.0)))
                .unwrap()
        })
        .unwrap()
    }
    fn io(kind: u64) -> tracing::Span {
        tracing::info_span!(target: "robson_operation", "operation_io", io_kind = kind)
    }
    #[tokio::test]
    async fn pending_repolls_and_cancel_are_one_reached_attempt() {
        async {
            robson_exec::observation::reconciliation(async {
                let counts = current();
                let mut future = Box::pin(std::future::pending::<()>().instrument(io(1)));
                for _ in 0..3 {
                    poll_fn(|cx| {
                        assert!(future.as_mut().poll(cx).is_pending());
                        Poll::Ready(())
                    })
                    .await;
                }
                drop(future);
                assert_eq!(counts.db.load(Ordering::Relaxed), 1, "polls are not queries");
                assert_eq!(counts.score.load(Ordering::Relaxed), 1);
            })
            .await;
        }
        .with_subscriber(tracing_subscriber::registry().with(layer()))
        .await;
    }
    #[tokio::test]
    async fn nearest_scan_owns_work_and_inherited_task_is_charged_once() {
        async {
            robson_exec::observation::reconciliation(async {
                let outer = current();
                let inner = robson_exec::observation::reconciliation(async {
                    let counts = current();
                    async {
                        tokio::task::yield_now().await;
                    }
                    .instrument(io(1))
                    .await;
                    counts
                })
                .await;
                let task = tokio::spawn(
                    robson_exec::observation::inherit_reconciliation(async {
                        async {
                            tokio::task::yield_now().await;
                        }
                        .instrument(io(1))
                        .await;
                    })
                    .with_current_subscriber(),
                );
                task.await.unwrap();
                assert_eq!(inner.db.load(Ordering::Relaxed), 1);
                assert_eq!(outer.db.load(Ordering::Relaxed), 1, "nested work must not count twice");
            })
            .await;
        }
        .with_subscriber(tracing_subscriber::registry().with(layer()))
        .await;
    }
    struct CountedBackend {
        inner: robson_exec::order_journal::StubJournalBackend,
        actual: AtomicU64,
    }
    #[async_trait::async_trait]
    impl robson_exec::order_journal::JournalBackend for CountedBackend {
        async fn commit(
            &self,
            batch: &robson_exec::order_journal::JournalBatch,
        ) -> Result<(), robson_exec::order_journal::CommitFailure> {
            self.actual.fetch_add(1, Ordering::Relaxed);
            self.inner.commit(batch).instrument(io(1)).await
        }
        async fn attempt_states(&self, v: &str, a: &str, c: &str) -> Result<Vec<bool>, String> {
            self.inner.attempt_states(v, a, c).await
        }
        async fn record_suspension(
            &self,
            row: &robson_exec::order_journal::StreamSuspensionRow,
        ) -> Result<(), robson_exec::order_journal::CommitFailure> {
            self.actual.fetch_add(1, Ordering::Relaxed);
            self.inner.record_suspension(row).instrument(io(1)).await
        }
        async fn upsert_incident(
            &self,
            row: &robson_exec::gate::Incident,
        ) -> Result<(), robson_exec::order_journal::CommitFailure> {
            self.actual.fetch_add(1, Ordering::Relaxed);
            self.inner.upsert_incident(row).instrument(io(1)).await
        }
        async fn release(
            &self,
            row: &robson_exec::order_journal::ReleaseRecord,
        ) -> Result<robson_exec::order_journal::ReleaseCounts, String> {
            self.inner.release(row).await
        }
    }
    fn suspension() -> robson_exec::order_journal::StreamSuspensionRow {
        robson_exec::order_journal::StreamSuspensionRow {
            suspension_id: uuid::Uuid::now_v7(),
            stream_key: "position:observed-test".into(),
            stream_kind: "position",
            failed_seq: None,
            month: None,
            error: "test refusal".into(),
            epoch_id: None,
            suspended_at: chrono::Utc::now(),
        }
    }
    #[tokio::test]
    async fn actual_pending_journal_flush_retains_origin_then_releases_it() {
        use robson_exec::order_journal::{OrderJournal, StubJournalBackend};
        async {
            for _ in 0..16 {
                let backend = Arc::new(CountedBackend {
                    inner: StubJournalBackend::default(),
                    actual: AtomicU64::new(0),
                });
                let journal = OrderJournal::new(
                    backend.clone(),
                    &robson_exec::venue::VenueId::new("observed-test"),
                );
                let counts = robson_exec::observation::reconciliation(async {
                    let counts = current();
                    journal.record_suspension(suspension());
                    tracing::Span::current().record("outcome", 1_u64);
                    counts
                })
                .await;
                assert_eq!(counts.db.load(Ordering::Relaxed), 0);
                let written = journal.flush_pending().await;
                assert_eq!(written, 2, "incident plus suspension use the real retry path");
                assert_eq!(backend.actual.load(Ordering::Relaxed), 2);
                assert_eq!(
                    counts.db.load(Ordering::Relaxed),
                    2,
                    "late IO belongs to the returned scan"
                );
                assert!(!counts.incomplete.load(Ordering::Relaxed));
                assert_eq!(
                    Arc::strong_count(&counts),
                    1,
                    "landed work retains no observation owner"
                );
            }
        }
        .with_subscriber(tracing_subscriber::registry().with(layer()))
        .await;
    }
    #[tokio::test]
    async fn coalesced_origins_are_both_incomplete_and_flush_is_never_duplicated() {
        use robson_exec::order_journal::{OrderJournal, StubJournalBackend};
        async {
            let backend = Arc::new(CountedBackend {
                inner: StubJournalBackend::default(),
                actual: AtomicU64::new(0),
            });
            let journal = OrderJournal::new(
                backend.clone(),
                &robson_exec::venue::VenueId::new("observed-test"),
            );
            let first = robson_exec::observation::reconciliation(async {
                let counts = current();
                journal.record_suspension(suspension());
                tracing::Span::current().record("outcome", 1_u64);
                counts
            })
            .await;
            let second = robson_exec::observation::reconciliation(async {
                let counts = current();
                journal.record_suspension(suspension());
                tracing::Span::current().record("outcome", 1_u64);
                counts
            })
            .await;
            assert!(first.incomplete.load(Ordering::Relaxed));
            assert!(second.incomplete.load(Ordering::Relaxed));
            assert_eq!(journal.flush_pending().await, 2);
            assert_eq!(backend.actual.load(Ordering::Relaxed), 2);
            assert_eq!(first.db.load(Ordering::Relaxed) + second.db.load(Ordering::Relaxed), 2);
        }
        .with_subscriber(tracing_subscriber::registry().with(layer()))
        .await;
    }
    #[tokio::test]
    async fn captured_unmeasured_work_cannot_inherit_a_later_scan() {
        async {
            let reached = Arc::new(AtomicU64::new(0));
            let physical = Arc::clone(&reached);
            let detached = robson_exec::observation::inherit_reconciliation(async move {
                let active = robson_exec::observation::reconciliation_active();
                async {
                    physical.fetch_add(1, Ordering::Relaxed);
                }
                .instrument(io(1))
                .await;
                active
            });
            robson_exec::observation::reconciliation(async {
                let counts = current();
                let active = detached.await;
                assert!(!active, "captured false context must not select measured transport");
                assert_eq!(reached.load(Ordering::Relaxed), 1);
                assert_eq!(
                    counts.db.load(Ordering::Relaxed),
                    0,
                    "parentless barrier prevents ambient attribution"
                );
                assert!(
                    counts.incomplete.load(Ordering::Relaxed),
                    "ambient causal/physical gap is explicit"
                );
                tracing::Span::current().record("outcome", 1_u64);
            })
            .await;
        }
        .with_subscriber(tracing_subscriber::registry().with(layer()))
        .await;
    }
    #[tokio::test]
    async fn unowned_pending_flush_counts_current_physical_prefix_as_incomplete() {
        use robson_exec::order_journal::{OrderJournal, StubJournalBackend};
        async {
            let backend = Arc::new(CountedBackend {
                inner: StubJournalBackend::default(),
                actual: AtomicU64::new(0),
            });
            let journal = OrderJournal::new(
                backend.clone(),
                &robson_exec::venue::VenueId::new("observed-test"),
            );
            journal.record_suspension(suspension());
            robson_exec::observation::reconciliation(async {
                let counts = current();
                let written = journal.flush_pending().await;
                assert_eq!(written, 2);
                assert_eq!(backend.actual.load(Ordering::Relaxed), 2);
                assert_eq!(
                    counts.db.load(Ordering::Relaxed),
                    2,
                    "unowned current work stays in physical prefix"
                );
                assert!(
                    counts.incomplete.load(Ordering::Relaxed),
                    "unknown causality cannot become complete"
                );
                assert!(journal.latch_set(), "observation must not clear the existing fence/latch");
                tracing::Span::current().record("outcome", 1_u64);
            })
            .await;
        }
        .with_subscriber(tracing_subscriber::registry().with(layer()))
        .await;
    }
    #[tokio::test]
    async fn old_origin_flushed_inside_a_new_scan_stays_old_and_counts_once() {
        use robson_exec::order_journal::{OrderJournal, StubJournalBackend};
        async {
            let backend = Arc::new(CountedBackend {
                inner: StubJournalBackend::default(),
                actual: AtomicU64::new(0),
            });
            let journal = OrderJournal::new(
                backend.clone(),
                &robson_exec::venue::VenueId::new("observed-test"),
            );
            let old = robson_exec::observation::reconciliation(async {
                let counts = current();
                journal.record_suspension(suspension());
                tracing::Span::current().record("outcome", 1_u64);
                counts
            })
            .await;
            robson_exec::observation::reconciliation(async {
                let new = current();
                let written = journal.flush_pending().await;
                assert_eq!(written, 2);
                assert_eq!(backend.actual.load(Ordering::Relaxed), 2);
                assert_eq!(old.db.load(Ordering::Relaxed), 2);
                assert_eq!(
                    new.db.load(Ordering::Relaxed),
                    0,
                    "known old work never moves to its caller"
                );
                assert!(!old.incomplete.load(Ordering::Relaxed));
                assert!(!new.incomplete.load(Ordering::Relaxed));
                tracing::Span::current().record("outcome", 1_u64);
            })
            .await;
        }
        .with_subscriber(tracing_subscriber::registry().with(layer()))
        .await;
    }
    proptest::proptest! {
        #[test]
        fn saturation_matches_wide_independent_oracle(initial in proptest::prelude::any::<u64>(), amount in proptest::prelude::any::<u64>()) {
            let counts = Counts::default(); counts.db.store(initial, Ordering::Relaxed);
            let expected = (u128::from(initial) + u128::from(amount)).min(u128::from(u64::MAX)) as u64;
            let actual = counts.add(&counts.db, amount);
            proptest::prop_assert_eq!(actual, expected);
            proptest::prop_assert_eq!(counts.db.load(Ordering::Relaxed), expected);
            proptest::prop_assert_eq!(counts.incomplete.load(Ordering::Relaxed), u128::from(initial) + u128::from(amount) > u128::from(u64::MAX));
        }
        #[test]
        fn async_schedules_count_reached_work_not_poll_count(order in proptest::collection::vec(proptest::prelude::any::<bool>(), 0..32)) {
            let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
            runtime.block_on(async {
                async {
                    robson_exec::observation::reconciliation(async {
                        let counts = current();
                        let reached = Arc::new(AtomicU64::new(0));
                        let mut tasks = Vec::new();
                        for spawn in &order {
                            let physical = Arc::clone(&reached);
                            let work = async move { async { tokio::task::yield_now().await; physical.fetch_add(1, Ordering::Relaxed); }.instrument(io(1)).await; };
                            if *spawn { tasks.push(tokio::spawn(robson_exec::observation::inherit_reconciliation(work).with_current_subscriber())); }
                            else { work.await; }
                        }
                        for task in tasks { task.await.unwrap(); }
                        assert_eq!(reached.load(Ordering::Relaxed), order.len() as u64);
                        assert_eq!(counts.db.load(Ordering::Relaxed), reached.load(Ordering::Relaxed));
                    }).await;
                }.with_subscriber(tracing_subscriber::registry().with(layer())).await;
            });
        }
    }
    #[cfg(feature = "postgres")]
    #[sqlx::test(migrations = "../migrations")]
    #[ignore = "requires DATABASE_URL"]
    async fn real_history_boundaries_count_success_and_failed_preparation(pool: sqlx::PgPool) {
        async {
            robson_exec::observation::reconciliation(async {
                let counts = current();
                let month = chrono::DateTime::parse_from_rfc3339("2027-03-01T00:00:00Z")
                    .unwrap()
                    .with_timezone(&chrono::Utc);
                let ok = robson_store::find_positions_overlapping_month(
                    &pool,
                    uuid::Uuid::now_v7(),
                    month,
                )
                .await;
                assert!(ok.is_ok());
                assert_eq!(counts.db.load(Ordering::Relaxed), 1);
                // Schema failure is setup outside the observed query delta, not a new business
                // path.
                sqlx::query("ALTER TABLE positions_current RENAME TO observation_hidden_positions")
                    .execute(&pool)
                    .await
                    .unwrap();
                let failed = robson_store::find_positions_overlapping_month(
                    &pool,
                    uuid::Uuid::now_v7(),
                    month,
                )
                .await;
                assert!(failed.is_err());
                assert_eq!(
                    counts.db.load(Ordering::Relaxed),
                    2,
                    "failed preparation is still an attempted query"
                );
            })
            .await;
        }
        .with_subscriber(tracing_subscriber::registry().with(layer()))
        .await;
    }
}

#[cfg(all(test, feature = "crash-points"))]
#[path = "operation_observation_http_tests.rs"]
mod http_tests;
