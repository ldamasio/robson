//! The cash re-anchor (ADR-0056 step 8l-5; v16 §C8; PLAN-r4 §6.12, operator
//! decision A): the operator's only way to bring an external movement into
//! the ledger. `POST /venue/cash-reanchor` runs [`CashReanchor::run`]: one
//! flight at a time, two agreeing ACCOUNT observations O1 and O2 at least 5 s
//! apart through 8k-3a's capture (two statements on fresh connections), then
//! one transaction holding the accounting rows (8l-5b).
//!
//! 8l-5a: the identity, the single flight, the at-rest and timing refusals
//! before the transaction, and O1/O2; 8l-5b ([`commit`]): the transaction,
//! its flows and continuity, and the recovery of an impaired anchor (operator
//! decision Q2 = R). Inert: no production composition. The venue side is a
//! seam ([`ReanchorVenue`]) that PS-5b's ACCOUNT and TIME read path fills,
//! and the daemon adds the service to the router at the 5a composition;
//! without it the endpoint answers 503.
use std::{sync::Arc, time::Duration};

use chrono::{DateTime, Datelike, TimeDelta, Utc};
use robson_exec::gate::{InvalidityMarks, OperationGate};
use rust_decimal::Decimal;
use serde::Serialize;
use sqlx::PgPool;
use tokio::time::Instant;
use uuid::Uuid;

use super::writer::Observed;
use crate::{
    balance_accounting::BalanceAccounting,
    balance_reconciliation::{
        classify_account, AccountReading, AccountReceipt, AttemptOutcome, Capture, Classification,
        ClockOutcome, DetectorScope, ScanScope, UnqualifiedReason,
    },
    spot_order_set_store::ExpectedGeneration,
    venue_ledger_store::settlement::CommissionAssets,
};

/// O2 is sent at least 5 s after O1's receipt (v16 §C8), checked on the
/// receipts' monotonic ticks.
const SPACING_NS: i128 = 5_000_000_000;
/// O1 and O2 agree within this, and so does the transaction's Δ test.
pub(crate) const TOLERANCE: Decimal = rust_decimal_macros::dec!(0.01);
/// No UTC month boundary within this of [O1's cut, O2's end].
const BOUNDARY_MARGIN: TimeDelta = TimeDelta::hours(1);
/// Each capture statement's deadline: connect, statement and close.
const CAPTURE_BOUND: Duration = Duration::from_secs(2);
/// Each ACCOUNT receipt's bound: a venue that never answers refuses `venue`
/// and frees the flight (review r1).
const ACCOUNT_BOUND: Duration = Duration::from_secs(5);

/// The venue side of one observation, filled by PS-5b's read path at the 5a
/// composition: one ACCOUNT request (weight 20) and its TIME request (weight
/// 1) through 8h's limiter, so the re-anchor costs 42.
#[async_trait::async_trait]
pub(crate) trait ReanchorVenue: Send + Sync {
    /// The receipt, or `None` when the venue gave none.
    async fn account(&self) -> Option<AccountReceipt>;
    /// The receipt's clock evidence judged again now, after the post
    /// statement: it can only refuse, never validate retroactively.
    fn revalidate(&self, receipt: &AccountReceipt) -> bool;
}

/// Why a re-anchor was refused; nothing was written. The endpoint answers
/// 409 with it as `{"error": …, "detail": …}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "error", content = "detail", rename_all = "snake_case")]
pub(crate) enum ReanchorRefusal {
    /// Another re-anchor of this daemon is running.
    Busy,
    /// 8m's read-only mode (a UID mismatch): no re-anchor on this account.
    ReadOnly,
    /// 8k-3a's capture refused: the scope, a non-empty Spot ledger (not at
    /// rest), a version, generation or pending change from before O1 to
    /// after O2, the clock, or the database.
    Unqualified(UnqualifiedReason),
    /// The venue gave no ACCOUNT receipt.
    Venue,
    /// The ACCOUNT-only base or locked-base check is not clean.
    Base,
    /// The body has no single well-formed quote-asset row.
    Quote,
    /// O2 was sent less than 5 s after O1's receipt (monotonic).
    Spacing,
    /// O1's and O2's totals differ by more than 0.01.
    Disagree,
    /// A UTC month boundary lies within 1 h of [O1's cut, O2's end].
    NearBoundary,
    /// The transaction's bump did not land on `v₀ + 1`.
    VersionMoved,
    /// The installed order-set generation refused its CAS (PLAN-r4 §6.5).
    StaleGeneration,
    /// An unsettled leg, an open transition, an unknown or live non-stop
    /// order, or an uncertain intent.
    NotAtRest,
    /// The observations are not of the tenant's live epoch or this process.
    Epoch,
    /// A booked fill lies in [O1's cut, O2's end].
    DirtyHull,
    /// No observed anchor precedes O1 (the cutover's is 8m's).
    NoAnchor,
    /// An anchor lies between O1's and O2's cuts.
    AnchorOverlap,
    /// `E_old` is impaired but not by its own anchor's dirty hull.
    ImpairedChain,
    /// `E_old` has no value (an unexpected commission asset or a sum that
    /// does not fit), or an amount is not exactly representable.
    Undefined,
    /// The invalidity generation moved, or evidence is pending.
    Marks,
    /// Publication more than 60 s after O1's capture began.
    Stale,
    /// COMMIT was sent and no answer came: a retry re-reads.
    Uncertain,
    /// The database refused or failed; nothing committed.
    Unavailable,
}

/// One of O1 and O2: its reading, the stored microsecond times and the
/// quote asset's free + locked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Observation {
    pub(crate) reading: AccountReading,
    pub(crate) observed: Observed,
    pub(crate) total: Decimal,
}

/// What qualified O1 and O2 hand the transaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Evidence {
    pub(crate) o1: Observation,
    pub(crate) o2: Observation,
    /// `v₀`, read before O1.
    pub(crate) version: i64,
    /// The gate's invalidity marks, read before O1.
    pub(crate) marks: InvalidityMarks,
    /// Taken before O1's capture: the freshness floor.
    pub(crate) requested: Instant,
}

/// A committed re-anchor, as the endpoint answers it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Reanchored {
    pub(crate) reanchor_id: Uuid,
    /// `attested`, or `recovery` for an impaired anchor (operator Q2 = R).
    pub(crate) mode: &'static str,
    pub(crate) version: i64,
    pub(crate) delta: Option<Decimal>,
    pub(crate) withdrawal: Option<Decimal>,
    pub(crate) contribution: Option<Decimal>,
    /// Attested flows written.
    pub(crate) flows: u64,
}

/// One balance-account daemon's re-anchor service.
pub(crate) struct CashReanchor {
    pub(crate) pool: PgPool,
    pub(crate) accounting: BalanceAccounting,
    pub(crate) gate: Arc<OperationGate>,
    /// This process's epoch: both observations and the publication are its.
    pub(crate) epoch: Uuid,
    /// The venue UID the account scope is bound to.
    pub(crate) uid: String,
    /// The operated pair and its commission assets.
    pub(crate) pair: CommissionAssets<'static>,
    pub(crate) venue: Arc<dyn ReanchorVenue>,
    /// 8m's read-only mode, until 8m-6 exports it.
    pub(crate) read_only: bool,
    /// The single flight.
    pub(crate) flight: tokio::sync::Mutex<()>,
}

impl CashReanchor {
    /// One re-anchor for `subject` (the verified identity) and `reason`, or
    /// a refusal with nothing written.
    pub(crate) async fn run(
        &self,
        subject: &str,
        reason: &str,
    ) -> Result<Reanchored, ReanchorRefusal> {
        // The guard is held for the whole arm.
        let result = match self.flight.try_lock() {
            Err(_) => Err(ReanchorRefusal::Busy),
            Ok(_flight) if self.read_only => Err(ReanchorRefusal::ReadOnly),
            Ok(_flight) => match self.observe().await {
                Ok(evidence) => self.commit(subject, reason, &evidence).await,
                Err(refusal) => Err(refusal),
            },
        };
        if let Err(refusal) = &result {
            let outcome = match refusal {
                ReanchorRefusal::Uncertain => "uncertain",
                _ => "refused",
            };
            crate::metrics::observe_cash_reanchor(outcome);
            tracing::warn!(?refusal, subject, reason, "cash re-anchor refused");
        }
        result
    }

    /// The process's non-trading scope (8k-3a): no order-set generation, so
    /// the scope's Spot ledger must be empty, which is at rest.
    fn scope(&self) -> ScanScope<'_> {
        let accounting = &self.accounting;
        ScanScope {
            detector: DetectorScope {
                expected: ExpectedGeneration {
                    tenant_id: accounting.tenant_id,
                    venue: &accounting.venue,
                    account_scope: &accounting.account_scope,
                    generation_id: Uuid::nil(),
                    process_epoch: self.epoch,
                    source_audit_id: Uuid::nil(),
                },
                uid: &self.uid,
                operated: self.pair.symbol,
                base_asset: self.pair.base,
            },
            quote_asset: self.pair.quote,
        }
    }

    /// O1 and O2 under one capture: the marks and `v₀` before O1, the post
    /// with `v₀`, both receipts' clocks judged again and the marks after O2.
    pub(crate) async fn observe(&self) -> Result<Evidence, ReanchorRefusal> {
        let (scope, options) = (self.scope(), self.pool.connect_options());
        let requested = Instant::now();
        let capture = Capture::before(&self.gate, &options, &scope, requested + CAPTURE_BOUND)
            .await
            .map_err(ReanchorRefusal::Unqualified)?;
        let (first, o1) = self.read(&scope, &capture).await?;
        robson_exec::crash_point!(REANCHOR_AFTER_O1);
        tokio::time::sleep(Duration::from_secs(5)).await;
        let (second, o2) = self.read(&scope, &capture).await?;
        let gap = second.send.before_ns.checked_sub(first.receive.after_ns);
        if first.receive.domain != second.send.domain || gap.is_none_or(|gap| gap < SPACING_NS) {
            return Err(ReanchorRefusal::Spacing);
        }
        let clock = || match self.venue.revalidate(&first) && self.venue.revalidate(&second) {
            true => ClockOutcome::Valid(o2.reading.interval),
            false => ClockOutcome::Invalid,
        };
        let deadline = Instant::now() + CAPTURE_BOUND;
        if let AttemptOutcome::Unqualified(reason) =
            capture.still(&self.gate, &options, &scope, deadline, clock).await
        {
            return Err(ReanchorRefusal::Unqualified(reason));
        }
        if (o1.total - o2.total).abs() > TOLERANCE {
            return Err(ReanchorRefusal::Disagree);
        }
        if near_boundary(o1.observed.cut, o2.observed.end) {
            return Err(ReanchorRefusal::NearBoundary);
        }
        let (version, marks) = (capture.pre.version, capture.marks);
        Ok(Evidence { o1, o2, version, marks, requested })
    }

    /// One observation: the receipt, its reading against the scope (UID,
    /// sample, this epoch, interval), the ACCOUNT-only classification and
    /// the quote total.
    async fn read(
        &self,
        scope: &ScanScope<'_>,
        capture: &Capture,
    ) -> Result<(AccountReceipt, Observation), ReanchorRefusal> {
        let receipt = tokio::time::timeout(ACCOUNT_BOUND, self.venue.account()).await;
        let receipt = receipt.ok().flatten().ok_or(ReanchorRefusal::Venue)?;
        let reading =
            AccountReading::from_receipt(&receipt, scope).map_err(ReanchorRefusal::Unqualified)?;
        if !matches!(classify_account(scope, &capture.pre, &receipt.body), Classification::Clean) {
            return Err(ReanchorRefusal::Base);
        }
        let total = reading.quote_total.ok_or(ReanchorRefusal::Quote)?;
        let observed = Observed::new(reading.interval, reading.u_ns)
            .ok_or(ReanchorRefusal::Unqualified(UnqualifiedReason::Malformed))?;
        Ok((receipt, Observation { reading, observed, total }))
    }
}

/// Whether a UTC month boundary lies within 1 h of `[cut, end]`, both ends
/// included: the instant 1 h and a microsecond before `cut` (every stored
/// time is whole microseconds) is in another month than 1 h after `end`. An
/// unrepresentable edge is near (review r2): a refusal, never a panic.
pub(crate) fn near_boundary(cut: DateTime<Utc>, end: DateTime<Utc>) -> bool {
    let before = cut.checked_sub_signed(BOUNDARY_MARGIN + TimeDelta::microseconds(1));
    let after = end.checked_add_signed(BOUNDARY_MARGIN);
    let month = |t: DateTime<Utc>| (t.year(), t.month());
    before.zip(after).is_none_or(|(before, after)| month(before) != month(after))
}

mod commit;
#[cfg(test)]
mod pg_tests;
