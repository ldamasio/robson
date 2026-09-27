//! Prometheus metrics for robsond.
//!
//! Exposes counters and gauges for operational monitoring:
//!
//! - `robsond_cycles_total` — completed engine cycles (market tick processing)
//! - `robsond_orders_total` — exchange orders placed (entry + exit)
//! - `robsond_risk_denials_total` — risk gate rejections, labelled by check
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
//! - `robsond_budget_model_shadow_slots_delta` — dormant ADR-0051 NFS slots
//!   minus HWM slots
//! - `robsond_market_data_ws_failures_total` — failed WS attempts by endpoint
//! - `robsond_sse_connections` — currently connected SSE clients on `/events`
//! - `robsond_sse_events_total` — public SSE events sent, labelled by event
//!   type
//! - `robsond_sse_disconnects_total` — SSE stream terminations on `/events`

use std::sync::LazyLock;

use prometheus::{
    self, register_counter, register_counter_vec, register_gauge, register_gauge_vec, Counter,
    CounterVec, Gauge, GaugeVec,
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

/// Metric handles used by one reconciliation worker.
///
/// Production instances clone the registered global collectors. Tests inject
/// unregistered collectors so parallel workers cannot satisfy each other's
/// assertions or overwrite each other's gauges.
#[derive(Clone)]
pub(crate) struct ReconciliationMetrics {
    scans: CounterVec,
    scan_in_progress: Gauge,
    last_attempt_timestamp_seconds: Gauge,
    last_completed_timestamp_seconds: Gauge,
}

impl ReconciliationMetrics {
    pub(crate) fn global() -> Self {
        Self {
            scans: (*RECONCILIATION_SCANS).clone(),
            scan_in_progress: (*RECONCILIATION_SCAN_IN_PROGRESS).clone(),
            last_attempt_timestamp_seconds: (*RECONCILIATION_LAST_ATTEMPT_TIMESTAMP_SECONDS)
                .clone(),
            last_completed_timestamp_seconds: (*RECONCILIATION_LAST_COMPLETED_TIMESTAMP_SECONDS)
                .clone(),
        }
    }

    pub(crate) fn scan_started(&self, timestamp_seconds: i64) {
        self.scan_in_progress.set(1.0);
        self.last_attempt_timestamp_seconds.set(timestamp_seconds as f64);
    }

    pub(crate) fn scan_completed(&self, timestamp_seconds: i64) {
        self.scan_in_progress.set(0.0);
        self.scans.with_label_values(&["completed"]).inc();
        self.last_completed_timestamp_seconds.set(timestamp_seconds as f64);
    }

    pub(crate) fn scan_failed(&self) {
        self.scan_in_progress.set(0.0);
        self.scans.with_label_values(&["error"]).inc();
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

/// Dormant ADR-0051 shadow comparison: net-from-start slots minus HWM slots.
pub static BUDGET_MODEL_SHADOW_SLOTS_DELTA: LazyLock<Gauge> = LazyLock::new(|| {
    register_gauge!(
        "robsond_budget_model_shadow_slots_delta",
        "Net-from-start monthly budget slots minus HWM slots"
    )
    .expect("failed to register robsond_budget_model_shadow_slots_delta")
});

/// Market data mode per symbol (0 = WS, 1 = REST fallback) — ADR-0044.
pub static MARKET_DATA_MODE: LazyLock<GaugeVec> = LazyLock::new(|| {
    register_gauge_vec!(
        "robsond_market_data_mode",
        "Market data source mode per symbol (0=ws, 1=rest_fallback)",
        &["symbol"]
    )
    .expect("failed to register robsond_market_data_mode")
});

/// Seconds since the last WS tick per symbol — ADR-0044.
pub static MARKET_DATA_SILENT_SECONDS: LazyLock<GaugeVec> = LazyLock::new(|| {
    register_gauge_vec!(
        "robsond_market_data_silent_seconds",
        "Seconds since the last WebSocket tick per symbol",
        &["symbol"]
    )
    .expect("failed to register robsond_market_data_silent_seconds")
});

/// REST fallback price polls per symbol, by outcome — ADR-0044 request
/// budget telemetry.
pub static MARKET_DATA_FALLBACK_POLLS: LazyLock<CounterVec> = LazyLock::new(|| {
    register_counter_vec!(
        "robsond_market_data_fallback_polls_total",
        "REST fallback price polls by outcome",
        &["symbol", "outcome"] // outcome: ok, error
    )
    .expect("failed to register robsond_market_data_fallback_polls_total")
});

/// Failed WebSocket attempts per symbol, endpoint, and failure reason.
pub static MARKET_DATA_WS_FAILURES: LazyLock<CounterVec> = LazyLock::new(|| {
    register_counter_vec!(
        "robsond_market_data_ws_failures_total",
        "Failed market-data WebSocket attempts by endpoint and reason",
        &["symbol", "endpoint", "reason"]
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
