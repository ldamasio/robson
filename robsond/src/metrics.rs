//! Prometheus metrics for robsond.
//!
//! Exposes counters and gauges for operational monitoring:
//!
//! - `robsond_cycles_total` — completed engine cycles (market tick processing)
//! - `robsond_orders_total` — exchange orders placed (entry + exit)
//! - `robsond_risk_denials_total` — risk gate rejections, labelled by check
//! - `robsond_detector_rearms_after_failure_total` — detectors re-armed after
//!   an admission failed before the executor, labelled by flow
//! - `robsond_position_pnl` — realized PnL per closed position
//! - `robsond_active_positions` — currently open position count
//! - `robsond_stale_active_positions` — open book positions missing on exchange
//! - `robsond_reconciliation_scans_total` — completed reconciliation scans by
//!   result
//! - `robsond_reconciliation_scan_in_progress` — reconciliation scan currently
//!   running
//! - `robsond_reconciliation_last_attempt_timestamp_seconds` — last scan start
//!   time
//! - `robsond_reconciliation_last_completed_timestamp_seconds` — last completed
//!   scan time
//! - `robsond_monthly_halt_active` — MonthlyHalt circuit breaker (0 or 1)
//! - `robsond_monthly_budget_basis_invalid` — ADR-0051 §1 accounting alarm (0
//!   or 1)
//! - `robsond_monthly_budget_basis_last_read_timestamp_seconds` — last time
//!   that alarm read the persisted basis
//! - `robsond_budget_model_shadow_slots_delta` — dormant ADR-0051 NFS slots
//!   minus HWM slots
//! - `robsond_market_data_ws_failures_total` — failed WS attempts by endpoint
//! - `robsond_funding_sagas_blocked` — started funding sagas no route can
//!   progress
//! - `robsond_sse_connections` — currently connected SSE clients on `/events`
//! - `robsond_sse_events_total` — public SSE events sent, labelled by event
//!   type
//! - `robsond_sse_disconnects_total` — SSE stream terminations on `/events`
//! - `robsond_unpermitted_mutations_total` — venue mutations without an
//!   operation permit (ADR-0056 step 7)
//! - `robsond_aborted_operations_total` — operations dropped before completing
//! - `robsond_api_slot_busy_total` — API requests refused on a full slot class,
//!   labelled by class
//! - `robsond_graceful_stop_total` — stops, labelled by result
//! - `robsond_unclean_start` — why this process's start counts as unclean
//! - `robsond_streams_suspended` — event-log streams refusing appends, by kind

use std::sync::LazyLock;

use prometheus::{
    self, register_counter, register_counter_vec, register_gauge, register_gauge_vec,
    register_int_gauge, register_int_gauge_vec, Counter, CounterVec, Gauge, GaugeVec, IntGauge,
    IntGaugeVec,
};

/// Total completed engine cycles (each market tick processed).
pub static CYCLES: LazyLock<CounterVec> = LazyLock::new(|| {
    register_counter_vec!(
        "robsond_cycles_total",
        "Total completed engine cycles",
        &["result"] // result: success, error
    )
    .expect("failed to register robsond_cycles_total")
});

/// Total exchange orders placed.
pub static ORDERS: LazyLock<CounterVec> = LazyLock::new(|| {
    register_counter_vec!(
        "robsond_orders_total",
        "Total exchange orders placed",
        &["side"] // side: entry, exit
    )
    .expect("failed to register robsond_orders_total")
});

/// Risk gate denials, labelled by which check failed.
pub static RISK_DENIALS: LazyLock<CounterVec> = LazyLock::new(|| {
    register_counter_vec!(
        "robsond_risk_denials_total",
        "Risk gate rejections by check type",
        &["check"] // check: max_open_positions, total_exposure, etc.
    )
    .expect("failed to register robsond_risk_denials_total")
});

/// Detectors re-armed after an admission failed before the executor (ADR-0051
/// Recovery), labelled by flow: `signal`, `approval`, `approval_expiry`. Each
/// is a position a database failure would otherwise have left Armed and idle
/// until a restart. A rate that keeps rising means admissions keep failing.
pub static DETECTOR_REARMS_AFTER_FAILURE: LazyLock<CounterVec> = LazyLock::new(|| {
    register_counter_vec!(
        "robsond_detector_rearms_after_failure_total",
        "Detectors re-armed after an admission failed before the executor",
        &["flow"]
    )
    .expect("failed to register robsond_detector_rearms_after_failure_total")
});

/// Zero series for every flow, so the counter exports from startup and the
/// first re-arm shows as an increase.
pub fn init_detector_rearms_after_failure() {
    for flow in ["signal", "approval", "approval_expiry"] {
        DETECTOR_REARMS_AFTER_FAILURE.with_label_values(&[flow]);
    }
}

/// 1 once a bounded send went out without its journal row, journal history
/// was discarded, an operation was aborted, a venue mutation ran without a
/// permit or an event-log stream was suspended (ADR-0056 step 7). Stays until
/// restart, and from PR P admissions stay closed while it is set.
pub static JOURNAL_LATCH: LazyLock<Gauge> = LazyLock::new(|| {
    register_gauge!(
        "robsond_journal_latch",
        "1 when a send went out without its journal row or journal history was discarded"
    )
    .expect("failed to register robsond_journal_latch")
});

/// Intents with something that has not landed in the journal yet.
pub static JOURNAL_RETRY_SET_DEPTH: LazyLock<Gauge> = LazyLock::new(|| {
    register_gauge!(
        "robsond_journal_retry_set_depth",
        "Intents waiting for their journal rows to land"
    )
    .expect("failed to register robsond_journal_retry_set_depth")
});

/// Sends the journal authorized without being able to keep their id, so
/// non-reuse rests on the id being fresh. Process-local: it resets on restart.
/// It counts those authorizations, not a verified inventory of orders with no
/// row: a later write may still land, or the adapter may refuse locally.
pub static JOURNAL_NON_REUSE_IDS_LOST: LazyLock<Gauge> = LazyLock::new(|| {
    register_gauge!(
        "robsond_journal_non_reuse_ids_lost",
        "Sends whose client id the order journal could not retain"
    )
    .expect("failed to register robsond_journal_non_reuse_ids_lost")
});

/// Statements charged to the journal (guardrails §1.1): what each commit plans
/// to run, so a commit that failed before sending anything is charged in full.
pub static JOURNAL_STATEMENTS: LazyLock<Gauge> = LazyLock::new(|| {
    register_gauge!(
        "robsond_journal_statements",
        "Statements charged by the order journal: planned per commit"
    )
    .expect("failed to register robsond_journal_statements")
});

/// The most statements charged to one journal operation since the start. The
/// reviewed bound is `OPERATION_STATEMENT_BOUND`.
pub static JOURNAL_OPERATION_STATEMENTS_MAX: LazyLock<Gauge> = LazyLock::new(|| {
    register_gauge!(
        "robsond_journal_operation_statements_max",
        "Most statements charged to one order journal operation"
    )
    .expect("failed to register robsond_journal_operation_statements_max")
});

/// Journal operations that went past their reviewed statement bound.
pub static JOURNAL_OPERATIONS_OVER_STATEMENT_BOUND: LazyLock<Gauge> = LazyLock::new(|| {
    register_gauge!(
        "robsond_journal_operations_over_statement_bound",
        "Order journal operations past their reviewed statement bound"
    )
    .expect("failed to register robsond_journal_operations_over_statement_bound")
});

/// Venue mutations that reached the choke point without an operation permit
/// while the daemon was open (ADR-0056 step 7). Each sets the journal latch;
/// once the daemon is stopping such a mutation is refused, not counted.
pub static UNPERMITTED_MUTATIONS: LazyLock<Counter> = LazyLock::new(|| {
    register_counter!(
        "robsond_unpermitted_mutations_total",
        "Venue mutations sent without an operation permit"
    )
    .expect("failed to register robsond_unpermitted_mutations_total")
});

/// Operations whose permit was dropped before they completed: cancelled or
/// panicked mid-flight. Each sets the journal latch.
pub static ABORTED_OPERATIONS: LazyLock<Counter> = LazyLock::new(|| {
    register_counter!("robsond_aborted_operations_total", "Operations dropped before completing")
        .expect("failed to register robsond_aborted_operations_total")
});

/// API requests answered 503 `busy` because their slot class was full, by
/// class: `ordinary` or `protective`.
pub static API_SLOT_BUSY: LazyLock<CounterVec> = LazyLock::new(|| {
    register_counter_vec!(
        "robsond_api_slot_busy_total",
        "API requests refused because their slot class was full",
        &["class"]
    )
    .expect("failed to register robsond_api_slot_busy_total")
});

/// Stops, by result: `clean`, or why it was not. Counted as the process exits,
/// so a scrape may never see it; the durable record is the epoch's marker and
/// the next start's [`UNCLEAN_START`].
pub static GRACEFUL_STOP: LazyLock<CounterVec> = LazyLock::new(|| {
    register_counter_vec!("robsond_graceful_stop_total", "Graceful stops by result", &["result"])
        .expect("failed to register robsond_graceful_stop_total")
});

/// 1 for the reason this process's start counts as unclean: `no_predecessor`,
/// `no_marker`, or `marker_version` (every start until the fence PR writes
/// version 2). Recorded only: nothing acts on it yet.
pub static UNCLEAN_START: LazyLock<GaugeVec> = LazyLock::new(|| {
    register_gauge_vec!(
        "robsond_unclean_start",
        "Why this process's start counts as unclean (1 for the reason)",
        &["reason"]
    )
    .expect("failed to register robsond_unclean_start")
});

/// Event-log streams that refuse appends, by `kind` (`position`, `system`):
/// suspended in this process, or loaded at start and not repaired (ADR-0056
/// step 7, PR P). A suspended system stream of the current month keeps
/// admissions closed.
pub static STREAMS_SUSPENDED: LazyLock<IntGaugeVec> = LazyLock::new(|| {
    register_int_gauge_vec!(
        "robsond_streams_suspended",
        "Event-log streams refusing appends, by kind",
        &["kind"]
    )
    .expect("failed to register robsond_streams_suspended")
});

/// Publish how many streams of each kind are suspended; both kinds export,
/// at 0 from startup.
pub fn observe_streams_suspended(positions: usize, systems: usize) {
    let count = |n: usize| i64::try_from(n).unwrap_or(i64::MAX);
    STREAMS_SUSPENDED.with_label_values(&["position"]).set(count(positions));
    STREAMS_SUSPENDED.with_label_values(&["system"]).set(count(systems));
}

/// Zero series for the gate's metrics, so they export from startup.
pub fn init_operation_gate_metrics() {
    LazyLock::force(&UNPERMITTED_MUTATIONS);
    LazyLock::force(&ABORTED_OPERATIONS);
    for class in ["ordinary", "protective"] {
        API_SLOT_BUSY.with_label_values(&[class]);
    }
}

/// Counts what the operation gate reports.
pub struct GateMetrics;

impl robson_exec::gate::GateObserver for GateMetrics {
    fn unpermitted_mutation(&self) {
        UNPERMITTED_MUTATIONS.inc();
    }

    fn operation_aborted(&self, _kind: robson_exec::gate::OperationKind) {
        ABORTED_OPERATIONS.inc();
    }
}

/// Publish the journal's latch, retry-set depth and statement counts.
pub fn observe_order_journal(journal: &robson_exec::order_journal::OrderJournal) {
    JOURNAL_LATCH.set(if journal.latch_set() { 1.0 } else { 0.0 });
    JOURNAL_RETRY_SET_DEPTH.set(journal.retry_set_depth() as f64);
    JOURNAL_NON_REUSE_IDS_LOST.set(journal.non_reuse_ids_lost() as f64);
    JOURNAL_STATEMENTS.set(journal.statements_total() as f64);
    JOURNAL_OPERATION_STATEMENTS_MAX.set(f64::from(journal.operation_statements_max()));
    JOURNAL_OPERATIONS_OVER_STATEMENT_BOUND.set(journal.operations_over_statement_bound() as f64);
}

/// Realized PnL per closed position (gauge — set once per close event).
pub static POSITION_PNL: LazyLock<GaugeVec> = LazyLock::new(|| {
    register_gauge_vec!("robsond_position_pnl", "Realized PnL for closed positions", &[
        "position_id"
    ])
    .expect("failed to register robsond_position_pnl")
});

/// Currently open position count.
pub static ACTIVE_POSITIONS: LazyLock<Gauge> = LazyLock::new(|| {
    register_gauge!("robsond_active_positions", "Number of currently open positions")
        .expect("failed to register robsond_active_positions")
});

/// Open book positions that are missing on the exchange and require
/// reconciliation.
pub static STALE_ACTIVE_POSITIONS: LazyLock<Gauge> = LazyLock::new(|| {
    register_gauge!(
        "robsond_stale_active_positions",
        "Number of open book positions missing on the exchange"
    )
    .expect("failed to register robsond_stale_active_positions")
});

/// Open positions whose exchange-side insurance stop is known to sit behind
/// the software stop plan because a placement or re-placement failed
/// (ADR-0039). Non-zero means daemon-down exposure is wider than one rung
/// until the tick-cadence retry heals it.
pub static INSURANCE_STOP_STALE_POSITIONS: LazyLock<Gauge> = LazyLock::new(|| {
    register_gauge!(
        "robsond_insurance_stop_stale_positions",
        "Number of open positions whose exchange insurance stop is behind the software stop plan"
    )
    .expect("failed to register robsond_insurance_stop_stale_positions")
});

/// Exchange-reconciliation scans, labelled by `completed` or `error`.
///
/// This is owned exclusively by `ReconciliationWorker`. Startup and periodic
/// scans use the same instrumentation path, so a process that never completes
/// its startup scan cannot look healthy merely because the periodic task has
/// not been spawned yet. `completed` means the worker's top-level scan returned
/// `Ok`; best-effort substeps that already log and continue remain outside this
/// liveness signal and must not be inferred as fully healthy from it.
pub static RECONCILIATION_SCANS: LazyLock<CounterVec> = LazyLock::new(|| {
    register_counter_vec!(
        "robsond_reconciliation_scans_total",
        "Completed exchange reconciliation scans by result",
        &["result"]
    )
    .expect("failed to register robsond_reconciliation_scans_total")
});

/// Whether the singleton reconciliation worker currently has a scan in flight.
///
/// A value stuck at 1 while the last-completed timestamp ages exposes a hung or
/// panicked exchange call. Production starts exactly one reconciliation scan
/// at a time; startup completes before the periodic worker is spawned.
pub static RECONCILIATION_SCAN_IN_PROGRESS: LazyLock<Gauge> = LazyLock::new(|| {
    register_gauge!(
        "robsond_reconciliation_scan_in_progress",
        "Whether an exchange reconciliation scan is currently running (0 or 1)"
    )
    .expect("failed to register robsond_reconciliation_scan_in_progress")
});

/// Unix timestamp of the most recent reconciliation scan attempt.
pub static RECONCILIATION_LAST_ATTEMPT_TIMESTAMP_SECONDS: LazyLock<Gauge> = LazyLock::new(|| {
    register_gauge!(
        "robsond_reconciliation_last_attempt_timestamp_seconds",
        "Unix timestamp of the most recent exchange reconciliation scan attempt"
    )
    .expect("failed to register robsond_reconciliation_last_attempt_timestamp_seconds")
});

/// Unix timestamp of the most recent completed reconciliation scan.
///
/// This gauge is never advanced on a top-level error. Alerting can therefore
/// distinguish a worker that is attempting and failing from one that still
/// completes its top-level account scans. It deliberately does not claim that
/// every best-effort substep succeeded.
pub static RECONCILIATION_LAST_COMPLETED_TIMESTAMP_SECONDS: LazyLock<Gauge> = LazyLock::new(|| {
    register_gauge!(
        "robsond_reconciliation_last_completed_timestamp_seconds",
        "Unix timestamp of the most recent completed exchange reconciliation scan"
    )
    .expect("failed to register robsond_reconciliation_last_completed_timestamp_seconds")
});

/// Symbols the orphan sweep visited only because a leg closed there recently,
/// each one open-orders request (AGENTS.md guardrail on request counts).
pub static RECONCILIATION_CLOSED_SYMBOL_VISITS: LazyLock<Counter> = LazyLock::new(|| {
    register_counter!(
        "robsond_reconciliation_closed_symbol_visits_total",
        "Symbols the orphan sweep visited only because a leg closed there recently"
    )
    .expect("failed to register robsond_reconciliation_closed_symbol_visits_total")
});

/// Metric handles used by one reconciliation worker.
///
/// Production instances clone the registered global collectors. Tests inject
/// unregistered collectors so parallel workers cannot satisfy each other's
/// assertions or overwrite each other's gauges.
#[derive(Clone)]
pub(crate) struct ReconciliationMetrics {
    scans: CounterVec,
    closed_symbol_visits: Counter,
    scan_in_progress: Gauge,
    last_attempt_timestamp_seconds: Gauge,
    last_completed_timestamp_seconds: Gauge,
}

impl ReconciliationMetrics {
    pub(crate) fn global() -> Self {
        Self {
            scans: (*RECONCILIATION_SCANS).clone(),
            closed_symbol_visits: (*RECONCILIATION_CLOSED_SYMBOL_VISITS).clone(),
            scan_in_progress: (*RECONCILIATION_SCAN_IN_PROGRESS).clone(),
            last_attempt_timestamp_seconds: (*RECONCILIATION_LAST_ATTEMPT_TIMESTAMP_SECONDS)
                .clone(),
            last_completed_timestamp_seconds: (*RECONCILIATION_LAST_COMPLETED_TIMESTAMP_SECONDS)
                .clone(),
        }
    }

    pub(crate) fn closed_symbol_visit(&self) {
        self.closed_symbol_visits.inc();
    }

    pub(crate) fn scan_started(&self, timestamp_seconds: i64) {
        self.scan_in_progress.set(1.0);
        self.last_attempt_timestamp_seconds.set(timestamp_seconds as f64);
    }

    pub(crate) fn scan_completed(&self, timestamp_seconds: i64) {
        self.scans.with_label_values(&["completed"]).inc();
        self.last_completed_timestamp_seconds.set(timestamp_seconds as f64);
        self.scan_in_progress.set(0.0);
    }

    pub(crate) fn scan_failed(&self) {
        self.scans.with_label_values(&["error"]).inc();
        self.scan_in_progress.set(0.0);
    }

    #[cfg(test)]
    pub(crate) fn unregistered() -> Self {
        Self {
            scans: CounterVec::new(
                prometheus::Opts::new(
                    "test_reconciliation_scans_total",
                    "Test reconciliation scans by result",
                ),
                &["result"],
            )
            .expect("test reconciliation counter must be valid"),
            closed_symbol_visits: Counter::new(
                "test_reconciliation_closed_symbol_visits_total",
                "Test reconciliation closed-symbol visits",
            )
            .expect("test reconciliation closed-symbol counter must be valid"),
            scan_in_progress: Gauge::new(
                "test_reconciliation_scan_in_progress",
                "Test reconciliation scan in progress",
            )
            .expect("test reconciliation in-progress gauge must be valid"),
            last_attempt_timestamp_seconds: Gauge::new(
                "test_reconciliation_last_attempt_timestamp_seconds",
                "Test reconciliation last attempt timestamp",
            )
            .expect("test reconciliation last-attempt gauge must be valid"),
            last_completed_timestamp_seconds: Gauge::new(
                "test_reconciliation_last_completed_timestamp_seconds",
                "Test reconciliation last completed timestamp",
            )
            .expect("test reconciliation last-completed gauge must be valid"),
        }
    }

    #[cfg(test)]
    pub(crate) fn closed_symbol_visit_count(&self) -> f64 {
        self.closed_symbol_visits.get()
    }

    #[cfg(test)]
    pub(crate) fn completed_count(&self) -> f64 {
        self.scans.with_label_values(&["completed"]).get()
    }

    #[cfg(test)]
    pub(crate) fn error_count(&self) -> f64 {
        self.scans.with_label_values(&["error"]).get()
    }

    #[cfg(test)]
    pub(crate) fn scan_in_progress(&self) -> f64 {
        self.scan_in_progress.get()
    }

    #[cfg(test)]
    pub(crate) fn last_attempt_timestamp_seconds(&self) -> f64 {
        self.last_attempt_timestamp_seconds.get()
    }

    #[cfg(test)]
    pub(crate) fn last_completed_timestamp_seconds(&self) -> f64 {
        self.last_completed_timestamp_seconds.get()
    }
}

/// MonthlyHalt circuit breaker state (0 = normal, 1 = halted).
pub static MONTHLY_HALT_ACTIVE: LazyLock<Gauge> = LazyLock::new(|| {
    register_gauge!(
        "robsond_monthly_halt_active",
        "MonthlyHalt circuit breaker (0=normal, 1=halted)"
    )
    .expect("failed to register robsond_monthly_halt_active")
});

/// ADR-0051 §1 accounting alarm: the current month's persisted budget basis is
/// not positive. Refreshed from `monthly_state` every minute, so a restart does
/// not lose it. MonthlyHalt does not fire on this state, so this is its signal.
pub static MONTHLY_BUDGET_BASIS_INVALID: LazyLock<Gauge> = LazyLock::new(|| {
    register_gauge!(
        "robsond_monthly_budget_basis_invalid",
        "Whether the current month's budget basis is invalid, capital_base <= 0 (0 or 1)"
    )
    .expect("failed to register robsond_monthly_budget_basis_invalid")
});

/// Unix timestamp of the alarm's last successful read of the budget basis. Zero
/// until the first one: `MONTHLY_BUDGET_BASIS_INVALID` also reads 0 then, so
/// this is what tells a process that never read the basis from a valid one.
pub static MONTHLY_BUDGET_BASIS_LAST_READ_TIMESTAMP_SECONDS: LazyLock<Gauge> =
    LazyLock::new(|| {
        register_gauge!(
            "robsond_monthly_budget_basis_last_read_timestamp_seconds",
            "Unix timestamp of the last successful read of the monthly budget basis"
        )
        .expect("failed to register robsond_monthly_budget_basis_last_read_timestamp_seconds")
    });

/// Dormant ADR-0051 shadow comparison: net-from-start slots minus HWM slots.
pub static BUDGET_MODEL_SHADOW_SLOTS_DELTA: LazyLock<Gauge> = LazyLock::new(|| {
    register_gauge!(
        "robsond_budget_model_shadow_slots_delta",
        "Net-from-start monthly budget slots minus HWM slots"
    )
    .expect("failed to register robsond_budget_model_shadow_slots_delta")
});

/// Started funding sagas held because no route has a funding surface (ADR-0056
/// §3). Refreshed on every funding worker poll, so it returns to zero.
pub static FUNDING_SAGAS_BLOCKED: LazyLock<IntGauge> = LazyLock::new(|| {
    register_int_gauge!(
        "robsond_funding_sagas_blocked",
        "Started funding sagas no route can progress"
    )
    .expect("failed to register robsond_funding_sagas_blocked")
});

/// Market data mode per feed (0 = WS, 1 = REST fallback) — ADR-0044.
pub static MARKET_DATA_MODE: LazyLock<GaugeVec> = LazyLock::new(|| {
    register_gauge_vec!(
        "robsond_market_data_mode",
        "Market data source mode per feed (0=ws, 1=rest_fallback)",
        &["venue", "symbol"]
    )
    .expect("failed to register robsond_market_data_mode")
});

/// Seconds since the last WS tick per feed — ADR-0044.
pub static MARKET_DATA_SILENT_SECONDS: LazyLock<GaugeVec> = LazyLock::new(|| {
    register_gauge_vec!(
        "robsond_market_data_silent_seconds",
        "Seconds since the last WebSocket tick per feed",
        &["venue", "symbol"]
    )
    .expect("failed to register robsond_market_data_silent_seconds")
});

/// REST fallback price polls per symbol, by outcome — ADR-0044 request
/// budget telemetry.
pub static MARKET_DATA_FALLBACK_POLLS: LazyLock<CounterVec> = LazyLock::new(|| {
    register_counter_vec!(
        "robsond_market_data_fallback_polls_total",
        "REST fallback price polls by outcome",
        &["venue", "symbol", "outcome"] // outcome: ok, error
    )
    .expect("failed to register robsond_market_data_fallback_polls_total")
});

/// Failed WebSocket attempts per feed, endpoint, and failure reason.
pub static MARKET_DATA_WS_FAILURES: LazyLock<CounterVec> = LazyLock::new(|| {
    register_counter_vec!(
        "robsond_market_data_ws_failures_total",
        "Failed market-data WebSocket attempts by endpoint and reason",
        &["venue", "symbol", "endpoint", "reason"]
    )
    .expect("failed to register robsond_market_data_ws_failures_total")
});

/// Currently connected SSE clients on `/events`.
///
/// Incremented when a client stream opens and decremented when it ends. A
/// dead or dropped SSE stream must never read as a live one — silence must be
/// visible — so the decrement is wired through [`SseConnectionGuard`].
pub static SSE_CONNECTIONS: LazyLock<Gauge> = LazyLock::new(|| {
    register_gauge!("robsond_sse_connections", "Currently connected SSE clients on /events")
        .expect("failed to register robsond_sse_connections")
});

/// Public SSE events sent over `/events`, labelled by the public event type
/// name (e.g. `position.changed`). Heartbeat keep-alive comments are emitted
/// by axum's `KeepAlive` layer and are intentionally NOT counted here.
pub static SSE_EVENTS: LazyLock<CounterVec> = LazyLock::new(|| {
    register_counter_vec!("robsond_sse_events_total", "Public SSE events sent by event type", &[
        "type"
    ])
    .expect("failed to register robsond_sse_events_total")
});

/// SSE stream terminations on `/events` — bumped whether the client
/// disconnects or the stream ends normally.
pub static SSE_DISCONNECTS: LazyLock<Counter> = LazyLock::new(|| {
    register_counter!("robsond_sse_disconnects_total", "SSE stream terminations on /events")
        .expect("failed to register robsond_sse_disconnects_total")
});

/// RAII guard for a single SSE client connection on `/events`.
///
/// Increments [`SSE_CONNECTIONS`] on creation and, on drop, decrements it and
/// bumps [`SSE_DISCONNECTS`]. The guard lives inside the SSE stream's state
/// machine, so dropping it — whether the stream ends normally or the client
/// disconnects mid-await, dropping the response future — always releases the
/// connection slot.
pub(crate) struct SseConnectionGuard<'a> {
    connections: &'a Gauge,
    disconnects: &'a Counter,
}

impl SseConnectionGuard<'static> {
    /// Bind a guard to the global SSE metrics — used by the live `/events`
    /// handler.
    pub(crate) fn new() -> Self {
        SseConnectionGuard::instrumented(&*SSE_CONNECTIONS, &*SSE_DISCONNECTS)
    }
}

impl<'a> SseConnectionGuard<'a> {
    /// Bind a guard to explicit metrics. Exposed for unit tests so they can
    /// run in isolation against unregistered metrics instead of mutating the
    /// shared global registry.
    fn instrumented(connections: &'a Gauge, disconnects: &'a Counter) -> Self {
        connections.inc();
        Self { connections, disconnects }
    }
}

impl Drop for SseConnectionGuard<'_> {
    fn drop(&mut self) {
        self.connections.dec();
        self.disconnects.inc();
    }
}

/// Render all registered metrics in Prometheus exposition format.
pub fn render() -> String {
    prometheus::TextEncoder::new()
        .encode_to_string(&prometheus::default_registry().gather())
        .expect("failed to encode metrics")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The alert rules in rbx-infra match these exact names.
    #[test]
    fn budget_basis_metrics_keep_the_names_the_alert_rules_select() {
        LazyLock::force(&MONTHLY_BUDGET_BASIS_INVALID);
        LazyLock::force(&MONTHLY_BUDGET_BASIS_LAST_READ_TIMESTAMP_SECONDS);

        let rendered = render();
        assert!(rendered.contains("robsond_monthly_budget_basis_invalid"));
        assert!(rendered.contains("robsond_monthly_budget_basis_last_read_timestamp_seconds"));
    }

    #[test]
    fn the_journal_gauges_are_exported() {
        observe_order_journal(&robson_exec::order_journal::OrderJournal::unpersisted());

        let rendered = render();
        assert!(rendered.contains("robsond_journal_latch 0"), "{rendered}");
        assert!(rendered.contains("robsond_journal_retry_set_depth 0"), "{rendered}");
        assert!(rendered.contains("robsond_journal_non_reuse_ids_lost 0"), "{rendered}");
        assert!(rendered.contains("robsond_journal_statements "), "{rendered}");
        assert!(rendered.contains("robsond_journal_operation_statements_max "), "{rendered}");
        assert!(
            rendered.contains("robsond_journal_operations_over_statement_bound "),
            "{rendered}"
        );
    }

    #[test]
    fn the_suspended_stream_gauge_exports_both_kinds() {
        // Other tests' managers publish to the same gauge, so only the series
        // are asserted here, not their values.
        observe_streams_suspended(0, 0);

        let rendered = render();
        assert!(rendered.contains("robsond_streams_suspended{kind=\"position\"} "), "{rendered}");
        assert!(rendered.contains("robsond_streams_suspended{kind=\"system\"} "), "{rendered}");
    }

    #[test]
    fn detector_rearm_counter_exports_every_flow_from_startup() {
        init_detector_rearms_after_failure();

        let rendered = render();
        for flow in ["signal", "approval", "approval_expiry"] {
            assert!(
                rendered.contains(&format!(
                    "robsond_detector_rearms_after_failure_total{{flow=\"{flow}\"}}"
                )),
                "{flow}"
            );
        }
    }

    #[test]
    fn reconciliation_metric_family_is_registered() {
        let metrics = ReconciliationMetrics::global();
        metrics.scans.with_label_values(&["completed"]);
        metrics.scans.with_label_values(&["error"]);

        let rendered = render();
        assert!(rendered.contains("robsond_reconciliation_scans_total"));
        assert!(rendered.contains("robsond_reconciliation_scan_in_progress"));
        assert!(rendered.contains("robsond_reconciliation_last_attempt_timestamp_seconds"));
        assert!(rendered.contains("robsond_reconciliation_last_completed_timestamp_seconds"));
    }

    #[test]
    fn sse_connection_guard_increments_on_create_decrements_on_drop() {
        // Isolated, unregistered metrics so parallel tests touching the global
        // SSE counters cannot perturb these assertions.
        let connections = Gauge::new("test_sse_connections", "test").unwrap();
        let disconnects = Counter::new("test_sse_disconnects", "test").unwrap();

        assert_eq!(connections.get(), 0.0);
        assert_eq!(disconnects.get(), 0.0);

        {
            let _guard = SseConnectionGuard::instrumented(&connections, &disconnects);
            // While the guard is live, exactly one connection is open and no
            // disconnect has been recorded.
            assert_eq!(connections.get(), 1.0);
            assert_eq!(disconnects.get(), 0.0);
        }

        // Dropping the guard releases the slot and records the disconnect —
        // the property that makes client-disconnect accounting robust.
        assert_eq!(connections.get(), 0.0);
        assert_eq!(disconnects.get(), 1.0);
    }
}
