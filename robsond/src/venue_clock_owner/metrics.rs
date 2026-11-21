//! Fixed-label I1 visibility. No identities or response payloads are labels.
use std::sync::LazyLock;

use prometheus::{
    register_counter_vec, register_gauge, register_int_gauge, register_int_gauge_vec, CounterVec,
    Gauge, IntGauge, IntGaugeVec,
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
        // Every edge counter exists from the first call (PS-3: at start).
        let edges = EDGES.with_label_values(&[label]);
        if new & bit != 0 {
            edges.inc();
        }
    }
}
// A Prometheus gauge value in seconds, not a financial amount.
#[allow(clippy::disallowed_types)]
pub(super) fn uncertainty(u2: i128) {
    UNCERTAINTY.set(u2 as f64 / 2_000_000_000.0);
}

static CYCLES: LazyLock<CounterVec> = LazyLock::new(|| {
    register_counter_vec!("robsond_spot_time_cycles_total", "TIME cycle outcomes", &["outcome"])
        .expect("register spot TIME cycle outcomes")
});
// v16's clock evidence (§C8 alerts): a published sample (0/1) and its bound
// u = u2 / 2 in seconds, a Prometheus value, not a financial amount.
static EVIDENCE: LazyLock<IntGauge> = LazyLock::new(|| {
    register_int_gauge!("robsond_clock_evidence_valid", "Published valid clock sample (0/1)")
        .expect("register clock evidence")
});
static BOUND: LazyLock<Gauge> = LazyLock::new(|| {
    register_gauge!("robsond_venue_clock_bound_seconds", "Published sample's bound u in seconds")
        .expect("register clock bound")
});
#[allow(clippy::disallowed_types)]
pub(super) fn evidence(u2: Option<i128>) {
    EVIDENCE.set(i64::from(u2.is_some()));
    BOUND.set(u2.map_or(0.0, |u2| u2 as f64 / 2_000_000_000.0));
}

/// Every clock metric exists from the start, zero-valued, so its absence is
/// visible (PLAN §6.11); the restriction level is the budget's.
pub(super) fn register() {
    faults(0, 0);
    LazyLock::force(&UNCERTAINTY);
    let outcomes = "published deferred budget expired closing transport native resource payload";
    for outcome in outcomes.split(' ').chain(["venue", "storage"]) {
        CYCLES.with_label_values(&[outcome]);
    }
    evidence(None);
    LazyLock::force(&BOUND);
    crate::venue_budget::register_restriction_level();
    crate::venue_budget::pause_store::register();
}

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
