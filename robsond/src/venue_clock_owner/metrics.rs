//! Fixed-label I1 visibility. No identities or response payloads are labels.
use std::sync::LazyLock;

use prometheus::{
    register_counter_vec, register_gauge, register_int_gauge_vec, CounterVec, Gauge, IntGaugeVec,
};

static FAULTS: LazyLock<IntGaugeVec> = LazyLock::new(|| {
    register_int_gauge_vec!("robsond_spot_clock_fault", "Latched native/sample clock faults", &[
        "reason"
    ])
    .expect("register spot clock faults")
});
static EDGES: LazyLock<CounterVec> = LazyLock::new(|| {
    register_counter_vec!("robsond_spot_clock_faults_total", "Clock fault reason edges", &[
        "reason"
    ])
    .expect("register spot clock fault edges")
});
static UNCERTAINTY: LazyLock<Gauge> = LazyLock::new(|| {
    register_gauge!(
        "robsond_spot_clock_uncertainty_seconds",
        "Original sample exact doubled uncertainty, displayed in seconds"
    )
    .expect("register spot clock uncertainty")
});
pub(super) fn faults(mask: u8, new: u8) {
    for (bit, label) in [
        (1, "native"),
        (2, "wall_step"),
        (4, "no_valid_120s"),
        (8, "sample_invalid_or_stale"),
    ] {
        FAULTS.with_label_values(&[label]).set(i64::from(mask & bit != 0));
        if new & bit != 0 {
            EDGES.with_label_values(&[label]).inc();
        }
    }
}
pub(super) fn uncertainty(u2: i128) {
    UNCERTAINTY.set(u2 as f64 / 2_000_000_000.0);
}

static CYCLES: LazyLock<CounterVec> = LazyLock::new(|| {
    register_counter_vec!("robsond_spot_time_cycles_total", "TIME cycle outcomes", &["outcome"])
        .expect("register spot TIME cycle outcomes")
});
pub(super) fn cycle(error: Option<super::TimeError>) {
    use super::TimeError;
    let label = match error {
        None => "published",
        Some(TimeError::Budget(crate::venue_budget::BudgetError::Deferred)) => "deferred",
        Some(TimeError::Budget(_)) => "budget",
        Some(TimeError::Expired) => "expired",
        Some(TimeError::Closing) => "closing",
        Some(TimeError::Transport) => "transport",
        Some(TimeError::Native) => "native",
        Some(TimeError::Resource) => "resource",
        Some(TimeError::Payload) => "payload",
        Some(TimeError::Venue) => "venue",
        Some(TimeError::Storage(_)) => "storage",
    };
    CYCLES.with_label_values(&[label]).inc();
}
