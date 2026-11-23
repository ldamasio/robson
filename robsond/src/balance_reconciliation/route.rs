//! 8k-3c (PLAN-r2 §6.4, §6.5, §6.10): every scan's result takes exactly one
//! route. Only the scan's own qualified Clean reaches the cash writer, once; a
//! BTC conflict pages (a non-trading process, F3) or fences through [`apply`]
//! (a trading one, 8o); exact-lookup and strict-recovery findings are
//! withheld (PLAN-r2's `RecoveryRoute::Withheld`: 8k-5 and 8j bind the owner
//! later); everything else is counted. No route sells, closes, cancels, books
//! a residual or writes an anchor, flow or invalidation, and nothing retries
//! inside the scan after a writer refusal: the next scheduled scan observes
//! anew. Inert: no production caller until 8k-3d's task.
use std::sync::LazyLock;

use prometheus::{core::Collector, IntCounter, IntCounterVec, IntGauge, Opts, Registry};
use robson_exec::{gate::IncidentReason, venue_ledger::LedgerError};
use sqlx::postgres::PgConnectOptions;

use super::{
    apply, consequence, scan_non_trading, BaseVerdict, Consequence, IncompleteReason,
    InventoryReads, ScanOutcome, ScanScope, Scanned, UnqualifiedReason,
};
use crate::venue_ledger_store::cash::writer::{CashRefusal, CashWriter, Concluded};

/// Where a BTC conflict goes, fixed by the process's start kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConflictSink {
    /// A non-trading process pages and records nothing: the legacy reason
    /// CHECK refuses `venue_discrepancy` until 8h's 2b cut (8o), and no entry
    /// exists to fence.
    Page,
    /// A trading process records `venue_discrepancy` (8k-1 B's fence, R2).
    Fence,
}

/// The one route a scan took; its outcome label is the variant's name.
#[derive(Debug)]
pub(crate) enum Routed {
    Concluded(Concluded),
    /// The writer refused and wrote nothing.
    WriterRefused(CashRefusal),
    /// COMMIT sent and no answer: possibly committed; the next scan re-reads.
    Uncertain,
    /// A fence-and-page finding, paged or fenced by the sink; any recovery
    /// finding beside it is withheld.
    Conflict,
    /// Recovery findings only: counted and logged, nothing sent.
    RecoveryWithheld,
    /// Inside the interval only once open claims widen it.
    Held,
    Incomplete(IncompleteReason),
    Unqualified(UnqualifiedReason),
}

/// The balance scan's metrics (PLAN-r2 §6.10), finite labels: the registered
/// set in production ([`SCAN_METRICS`]), an unregistered one in a test.
pub(crate) struct ScanMetrics {
    pub(super) scans: IntCounterVec,
    pub(super) refusals: IntCounterVec,
    pub(super) attempts: IntCounter,
    pub(super) weight: IntCounter,
    pub(super) last_concluded: IntGauge,
    pub(super) retries: IntCounter,
    pub(super) conflict: IntGauge,
    pub(super) conflicts: IntCounterVec,
    pub(super) withheld: IntCounterVec,
}

/// The registered metrics, exported at 0 once forced (8k-3d, at start).
pub(crate) static SCAN_METRICS: LazyLock<ScanMetrics> =
    LazyLock::new(|| ScanMetrics::new(Some(prometheus::default_registry())));

impl ScanMetrics {
    /// Every series of at most one label exported at 0; registered when a
    /// registry is given.
    pub(crate) fn new(registry: Option<&Registry>) -> Self {
        let register = |metric: Box<dyn Collector>| {
            if let Some(registry) = registry {
                registry.register(metric).expect("balance scan metrics register once");
            }
        };
        let vec = |name: &str, help: &str, labels: &[&str], zero: &str| {
            let vec = IntCounterVec::new(Opts::new(name, help), labels).expect("valid metric");
            zero.split_whitespace().for_each(|value| drop(vec.with_label_values(&[value])));
            register(Box::new(vec.clone()));
            vec
        };
        let counter = |name: &str, help: &str| vec(name, help, &[], "").with_label_values(&[]);
        let gauge = |name: &str, help: &str| {
            let gauge = IntGauge::new(name, help).expect("valid metric");
            register(Box::new(gauge.clone()));
            gauge
        };
        let outcomes = "concluded writer_refused uncertain conflict recovery_withheld held \
                        incomplete unqualified";
        let found = "base_deficit base_excess locked_foreign operated_order_list";
        let kinds = "locked_short unlinked_operated_order needs_exact_evidence";
        let refused = ["stage", "reason"];
        Self {
            scans: vec("robsond_balance_scans_total", "Scans by route", &["outcome"], outcomes),
            refusals: vec("robsond_balance_scan_refusals_total", "Refusals", &refused, ""),
            attempts: counter("robsond_balance_scan_attempts_total", "Scan attempts"),
            weight: counter("robsond_balance_scan_weight_total", "Scan request weight"),
            last_concluded: gauge(
                "robsond_balance_scan_last_concluded_timestamp_seconds",
                "Unix time of the last cash conclusion",
            ),
            retries: counter("robsond_cash_observation_retries_total", "Observations retried"),
            conflict: gauge("robsond_spot_btc_conflict", "1 while a BTC conflict stands"),
            conflicts: vec("robsond_spot_btc_conflicts_total", "Conflicts", &["finding"], found),
            withheld: vec("robsond_balance_recovery_withheld_total", "Withheld", &["kind"], kinds),
        }
    }

    /// The scan's attempts and weight; the route's outcome and refusal; one
    /// retry per attempt the scan repeated and per publication refused for
    /// the next scan.
    fn observe(&self, scan: &ScanOutcome, routed: &Routed) {
        use CashRefusal as R;
        self.attempts.inc_by(u64::from(scan.attempts));
        self.weight.inc_by(u64::from(scan.cost.weight));
        let next = matches!(
            routed,
            Routed::WriterRefused(R::VersionMoved | R::Marks | R::Stale | R::Epoch)
        );
        self.retries
            .inc_by(u64::from(scan.attempts.saturating_sub(1)) + u64::from(next));
        self.scans.with_label_values(&[&label(routed)]).inc();
        let refusal = match routed {
            Routed::Concluded(_) => {
                self.last_concluded.set(chrono::Utc::now().timestamp());
                None
            },
            Routed::WriterRefused(refusal) => Some(("publish", label(refusal))),
            Routed::Incomplete(reason) => Some(("classify", label(reason))),
            Routed::Unqualified(reason) => Some(("qualify", label(reason))),
            _ => None,
        };
        if let Some((stage, reason)) = refusal {
            self.refusals.with_label_values(&[stage, &reason]).inc();
        }
    }
}

/// A variant's name as a snake_case label. Derived `Debug` writes the name
/// first, so a closed enum gives a closed label set whatever its fields hold.
fn label(variant: &impl std::fmt::Debug) -> String {
    let mut label = String::new();
    for c in format!("{variant:?}").chars().take_while(char::is_ascii_alphanumeric) {
        if c.is_ascii_uppercase() && !label.is_empty() {
            label.push('_');
        }
        label.push(c.to_ascii_lowercase());
    }
    label
}

/// One scheduled scan of the non-trading scope (8k-3d's task calls it): the
/// scan on the writer's own gate, then its one route. A non-trading process
/// only pages.
pub(crate) async fn scan_once(
    reads: &mut impl InventoryReads,
    options: &PgConnectOptions,
    scope: &ScanScope<'_>,
    writer: &CashWriter<'_>,
    metrics: &ScanMetrics,
) -> Option<Routed> {
    let scanned = scan_non_trading(reads, writer.gate, options, scope).await?;
    Some(route(scanned, writer, ConflictSink::Page, metrics).await)
}

/// The one route of one scan (PLAN-r2 §6.4). The verdict decides; the
/// writer's input is used only for a Clean verdict and only when it was
/// drawn on that same verdict, so nothing but a qualified Clean concludes. A
/// discrepancy takes its strongest route: a fence-and-page finding pages or
/// fences once, every recovery finding is withheld, holds change nothing.
pub(crate) async fn route(
    scanned: Scanned,
    writer: &CashWriter<'_>,
    sink: ConflictSink,
    metrics: &ScanMetrics,
) -> Routed {
    use UnqualifiedReason as U;
    let Scanned { outcome, verdict, cash } = scanned;
    let routed = match (&verdict, cash) {
        (BaseVerdict::Clean { .. }, Some(cash)) if cash.verdict == verdict => {
            match writer.conclude(&cash).await {
                Ok(done) => Routed::Concluded(done),
                Err(CashRefusal::Ledger(LedgerError::Uncertain(_))) => Routed::Uncertain,
                Err(refusal) => Routed::WriterRefused(refusal),
            }
        },
        (BaseVerdict::Clean { .. }, _) => Routed::Unqualified(U::Malformed),
        (BaseVerdict::Discrepancy { findings, .. }, _) => {
            let mut routed = Routed::Held;
            for finding in findings {
                let kind = label(finding);
                match consequence(finding) {
                    Consequence::FenceAndPage => {
                        metrics.conflicts.with_label_values(&[&kind]).inc();
                        tracing::error!(%kind, ?finding, ?sink, "BTC conflict");
                        routed = Routed::Conflict;
                    },
                    Consequence::ExactLookup | Consequence::StrictRecovery => {
                        metrics.withheld.with_label_values(&[&kind]).inc();
                        tracing::warn!(%kind, ?finding, "recovery withheld");
                        if !matches!(routed, Routed::Conflict) {
                            routed = Routed::RecoveryWithheld;
                        }
                    },
                    Consequence::Hold => {},
                }
            }
            if matches!(routed, Routed::Conflict) && sink == ConflictSink::Fence {
                let _ = apply(&verdict, writer.gate);
            }
            routed
        },
        (BaseVerdict::Incomplete { reason, .. }, _) => Routed::Incomplete(*reason),
        (BaseVerdict::Unqualified(reason), _) => Routed::Unqualified(*reason),
    };
    // The conflict gauge: a page stands until a later qualified Clean
    // reaches the writer; a fence follows its open incident, which a Clean
    // never clears (R2).
    let clean =
        matches!(routed, Routed::Concluded(_) | Routed::WriterRefused(_) | Routed::Uncertain);
    match (sink, &routed) {
        (_, Routed::Conflict) => metrics.conflict.set(1),
        (ConflictSink::Page, _) if clean => metrics.conflict.set(0),
        (ConflictSink::Page, _) => {},
        (ConflictSink::Fence, _) => {
            let open = writer.gate.open_incidents();
            let fenced = open.iter().any(|i| i.reason == IncidentReason::VenueDiscrepancy);
            metrics.conflict.set(i64::from(fenced));
        },
    }
    match &routed {
        Routed::Unqualified(reason @ (U::Scope | U::LedgerNotEmpty)) => {
            tracing::warn!(?reason, "balance scan refused: the scope does not hold")
        },
        Routed::Unqualified(reason) => tracing::debug!(?reason, "balance scan unqualified"),
        Routed::Conflict | Routed::RecoveryWithheld => {},
        Routed::Concluded(concluded) => tracing::info!(?concluded, "balance scan routed"),
        routed => tracing::info!(?routed, "balance scan routed"),
    }
    metrics.observe(&outcome, &routed);
    routed
}
