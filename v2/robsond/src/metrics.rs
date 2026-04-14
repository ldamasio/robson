//! Prometheus metrics for robsond.
//!
//! Exposes counters and gauges for operational monitoring:
//!
//! - `robsond_cycles_total` — completed engine cycles (market tick processing)
//! - `robsond_orders_total` — exchange orders placed (entry + exit)
//! - `robsond_risk_denials_total` — risk gate rejections, labelled by check
//! - `robsond_position_pnl` — realized PnL per closed position
//! - `robsond_active_positions` — currently open position count
//! - `robsond_monthly_halt_active` — MonthlyHalt circuit breaker (0 or 1)

use std::sync::LazyLock;

use prometheus::{
    self, register_counter_vec, register_gauge, register_gauge_vec, CounterVec, Gauge, GaugeVec,
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
    register_gauge_vec!(
        "robsond_position_pnl",
        "Realized PnL for closed positions",
        &["position_id"]
    )
    .expect("failed to register robsond_position_pnl")
});

/// Currently open position count.
pub static ACTIVE_POSITIONS: LazyLock<Gauge> = LazyLock::new(|| {
    register_gauge!(
        "robsond_active_positions",
        "Number of currently open positions"
    )
    .expect("failed to register robsond_active_positions")
});

/// MonthlyHalt circuit breaker state (0 = normal, 1 = halted).
pub static MONTHLY_HALT_ACTIVE: LazyLock<Gauge> = LazyLock::new(|| {
    register_gauge!(
        "robsond_monthly_halt_active",
        "MonthlyHalt circuit breaker (0=normal, 1=halted)"
    )
    .expect("failed to register robsond_monthly_halt_active")
});

/// Render all registered metrics in Prometheus exposition format.
pub fn render() -> String {
    prometheus::TextEncoder::new()
        .encode_to_string(&prometheus::default_registry().gather())
        .expect("failed to encode metrics")
}
