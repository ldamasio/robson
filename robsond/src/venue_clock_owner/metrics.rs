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
    for read in READ_LABELS.split(' ') {
        for outcome in READ_OUTCOMES.split(' ') {
            READS.with_label_values(&[read, outcome]);
        }
    }
    evidence(None);
    LazyLock::force(&BOUND);
    bucket(Some((0, 0)));
    crate::venue_budget::register_restriction_level();
}

static BUCKET: LazyLock<IntGaugeVec> = LazyLock::new(|| {
    register_int_gauge_vec!("robsond_venue_bucket_used", "Rolling limiter bucket usage", &[
        "bucket", "class"
    ])
    .expect("register venue bucket usage")
});

/// The steady buckets' rolling usage (v16 §10): the minute's weight and the
/// 10 s orders, refreshed after each TIME cycle (and, with PS-5b, each read).
pub(super) fn bucket(usage: Option<(u64, u64)>) {
    set_bucket(&BUCKET, usage);
}

fn set_bucket(gauge: &IntGaugeVec, usage: Option<(u64, u64)>) {
    if let Some((weight, orders)) = usage {
        for (bucket, used) in [("weight_1m", weight), ("orders_10s", orders)] {
            let used = i64::try_from(used).unwrap_or(i64::MAX);
            gauge.with_label_values(&[bucket, "steady"]).set(used);
        }
    }
}

static READS: LazyLock<CounterVec> = LazyLock::new(|| {
    register_counter_vec!("robsond_spot_reads_total", "Budgeted Spot read outcomes", &[
        "read", "outcome"
    ])
    .expect("register spot read outcomes")
});

/// Every read's label (its `ReadKind`, as `read` writes it) and outcome.
const READ_LABELS: &str = "Account OperatedOpenOrders OpenOrderLists ExchangeInfo Kline Commission";
const READ_OUTCOMES: &str = "published deferred budget expired closing transport native resource \
                             payload venue storage busy dropped";

/// One budgeted read's outcome, by read (PS-5b): a TIME cycle's outcome
/// labels, `busy` (no slot) or `dropped` (its caller dropped it).
pub(super) fn read(read: impl std::fmt::Debug, outcome: &str) {
    READS.with_label_values(&[format!("{read:?}").as_str(), outcome]).inc();
}

pub(super) fn cycle(error: Option<super::TimeError>) {
    CYCLES.with_label_values(&[outcome(error.as_ref())]).inc();
}

pub(super) fn outcome(error: Option<&super::TimeError>) -> &'static str {
    use super::TimeError;
    match error {
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
        Some(TimeError::Busy) => "busy",
    }
}

#[cfg(test)]
mod tests {
    /// PS-5a: the bucket gauge shows the steady buckets' usage as given and
    /// keeps its last value when none is given. On a gauge of its own: the
    /// process's one is reset by every owner another test constructs.
    #[test]
    fn the_bucket_gauge_shows_the_steady_usage() {
        let opts = prometheus::Opts::new("bucket_used_under_test", "test");
        let gauge = prometheus::IntGaugeVec::new(opts, &["bucket", "class"]).unwrap();
        let used = |bucket: &str| gauge.with_label_values(&[bucket, "steady"]).get();
        super::set_bucket(&gauge, Some((26, 0)));
        assert_eq!((used("weight_1m"), used("orders_10s")), (26, 0));
        super::set_bucket(&gauge, None);
        assert_eq!(used("weight_1m"), 26);
        super::set_bucket(&gauge, Some((u64::MAX, 1)));
        assert_eq!((used("weight_1m"), used("orders_10s")), (i64::MAX, 1));
    }

    /// PS-5b: every read's series exists from the start: `READ_LABELS` names
    /// each `ReadKind` as `read` writes it, and `READ_OUTCOMES` holds every
    /// label `outcome` gives a read (each error, success) and `dropped`.
    #[test]
    fn every_read_series_is_registered_at_start() {
        use super::super::TimeError;
        use crate::{
            venue_budget::{state::ReadKind, BudgetError},
            venue_clock_store::StoreError,
        };
        let kinds = [
            ReadKind::Account,
            ReadKind::OperatedOpenOrders,
            ReadKind::OpenOrderLists,
            ReadKind::ExchangeInfo,
            ReadKind::Kline,
            ReadKind::Commission,
        ];
        let labels: Vec<_> = kinds.iter().map(|kind| format!("{kind:?}")).collect();
        assert_eq!(super::READ_LABELS.split(' ').collect::<Vec<_>>(), labels);
        let errors = [
            TimeError::Budget(BudgetError::Deferred),
            TimeError::Budget(BudgetError::Invalid),
            TimeError::Expired,
            TimeError::Closing,
            TimeError::Transport,
            TimeError::Storage(StoreError::Backend),
            TimeError::Native,
            TimeError::Resource,
            TimeError::Payload,
            TimeError::Venue,
            TimeError::Busy,
        ];
        let outcomes: Vec<_> = super::READ_OUTCOMES.split(' ').collect();
        for error in errors.iter().map(Some).chain([None]) {
            assert!(outcomes.contains(&super::outcome(error)), "{error:?}");
        }
        assert!(outcomes.contains(&"dropped"));
    }
}
