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
//! - `robsond_monthly_halt_active` — MonthlyHalt circuit breaker (0 or 1)
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

/// MonthlyHalt circuit breaker state (0 = normal, 1 = halted).
pub static MONTHLY_HALT_ACTIVE: LazyLock<Gauge> = LazyLock::new(|| {
    register_gauge!(
        "robsond_monthly_halt_active",
        "MonthlyHalt circuit breaker (0=normal, 1=halted)"
    )
    .expect("failed to register robsond_monthly_halt_active")
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
