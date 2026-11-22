//! 8k-3b (PLAN-r2 §6.2, §6.3): the production attempt and scan of the
//! non-trading scope over a narrow inventory port, promoted from 8k-2's
//! test-only composition (F1). Nothing is held across the triple: the pre and
//! post statements run on connections closed before they return, and only
//! the last attempt's bodies survive it. Inert: no production caller yet
//! (8k-3d's task and PS-5b's adapter).
use std::time::Duration;

use robson_connectors::binance::spot::{SpotOpenOrderList, SpotOrderRecord};
use robson_exec::gate::OperationGate;
use sqlx::postgres::PgConnectOptions;
use tokio::time::Instant;

use super::{
    cash_attempt, classify, conclude, AccountReading, AccountReceipt, Attempt, AttemptOutcome,
    Attribution, BaseVerdict, Capture, ClockOutcome, Listing, OperatedList, OperatedOrder,
    Progress, Scan, ScanOutcome, ScanScope, UnqualifiedReason,
};
use crate::venue_ledger_store::cash::writer::CashAttempt;

/// One attempt's absolute deadline: PS's `inventory` default, never reset.
pub(crate) const ATTEMPT_DEADLINE: Duration = Duration::from_secs(15);

/// ACCOUNT, the operated pair's open orders and every open order list, read
/// under one sample and clock generation (PS-5b's `inventory`; the rig's in
/// tests).
#[derive(Debug, Clone)]
pub(crate) struct Triple {
    pub(crate) account: AccountReceipt,
    pub(crate) orders: Vec<SpotOrderRecord>,
    pub(crate) lists: Vec<SpotOpenOrderList>,
}

/// Why a triple failed. `Deferred`: refused before any request was handed
/// off (a busy read slot, a budget deferral, an active pause), so nothing was
/// sent or charged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReadFailure {
    Deferred,
    Clock,
    Venue,
    Timeout,
    Malformed,
}

/// The inventory port (PLAN-r2 §6.2).
pub(crate) trait InventoryReads {
    /// One triple by `deadline`: the requests handed off to the wire (a
    /// failing one included) and the triple or why it failed.
    async fn triple(&mut self, deadline: Instant) -> (u8, Result<Triple, ReadFailure>);

    /// The EOF receipt rule applied again to the ACCOUNT receipt, after the
    /// post: it only refuses, never validates retroactively.
    fn still_valid(&self, account: &AccountReceipt) -> bool;
}

/// What a scan concludes: its attempts and actual cost, the verdict to
/// route, and the writer's input only for a qualified Clean with a
/// well-formed quote total (then `verdict` is that Clean).
#[derive(Debug)]
pub(crate) struct Scanned {
    pub(crate) outcome: ScanOutcome,
    pub(crate) verdict: BaseVerdict,
    pub(crate) cash: Option<CashAttempt>,
}

/// The bodies and capture one attempt drew on, kept only until the next.
struct Drawn {
    capture: Capture,
    reading: AccountReading,
    triple: Triple,
    received: std::time::Instant,
}

/// At most [`super::MAX_ATTEMPTS`] attempts, each on a fresh deadline, the
/// counters never reset; a permanent refusal (`Deferred` included) ends the
/// scan. Only the last attempt is classified, and only when it qualified.
pub(crate) async fn scan_non_trading(
    reads: &mut impl InventoryReads,
    gate: &OperationGate,
    options: &PgConnectOptions,
    scope: &ScanScope<'_>,
) -> Option<Scanned> {
    let mut scan = Scan::default();
    let mut drawn = None;
    loop {
        drop(drawn.take());
        let deadline = Instant::now() + ATTEMPT_DEADLINE;
        let (attempt, attempted) = attempt(&mut *reads, gate, options, scope, deadline).await;
        drawn = attempted;
        if !scan.record(attempt) {
            break;
        }
    }
    let outcome = scan.finish()?;
    let classification = match (outcome.outcome, &drawn) {
        (AttemptOutcome::Qualified(_), Some(d)) => {
            let body = &d.triple.account.body;
            Some(classify(scope.detector, &d.capture.pre, body, &listing(scope, &d.triple)))
        },
        _ => None,
    };
    let verdict = conclude(&outcome, classification);
    let (verdict, cash) = match drawn {
        Some(d) => match cash_attempt(verdict, &d.capture, &d.reading, d.received) {
            Ok(cash) => (cash.verdict.clone(), Some(cash)),
            Err(verdict) => (verdict, None),
        },
        None => (verdict, None),
    };
    Some(Scanned { outcome, verdict, cash })
}

/// C8 steps 1-9 (PLAN-r2 §6.2): marks and pre, the freshness floor before
/// the composite, the triple, the ACCOUNT reading, the post, the receipt's
/// revalidation, the marks again and `qualify`.
async fn attempt(
    reads: &mut impl InventoryReads,
    gate: &OperationGate,
    options: &PgConnectOptions,
    scope: &ScanScope<'_>,
    deadline: Instant,
) -> (Attempt, Option<Drawn>) {
    let mut progress = Progress { pre: true, ..Progress::default() };
    let refused = |progress, reason| {
        (
            Attempt {
                progress,
                outcome: AttemptOutcome::Unqualified(reason),
            },
            None,
        )
    };
    let capture = match Capture::before(gate, options, scope, deadline).await {
        Ok(capture) => capture,
        Err(reason) => return refused(progress, reason),
    };
    let received = std::time::Instant::now();
    let (sent, triple) = reads.triple(deadline).await;
    progress.responses = sent;
    let triple = match triple {
        Ok(triple) => triple,
        Err(failure) => {
            let reason = match failure {
                ReadFailure::Deferred if sent == 0 => UnqualifiedReason::Deferred,
                ReadFailure::Deferred | ReadFailure::Venue => UnqualifiedReason::Venue,
                ReadFailure::Clock => UnqualifiedReason::Clock,
                ReadFailure::Timeout => UnqualifiedReason::Timeout,
                ReadFailure::Malformed => UnqualifiedReason::Malformed,
            };
            return refused(progress, reason);
        },
    };
    let reading = match AccountReading::from_receipt(&triple.account, scope) {
        Ok(reading) => reading,
        Err(reason) => return refused(progress, reason),
    };
    progress.post = true;
    let revalidate = || match reads.still_valid(&triple.account) {
        true => ClockOutcome::Valid(reading.interval),
        false => ClockOutcome::Invalid,
    };
    let outcome = capture.still(gate, options, scope, deadline, revalidate).await;
    let drawn = Drawn { capture, reading, triple, received };
    (Attempt { progress, outcome }, Some(drawn))
}

/// The non-trading operated subset: orders and lists of exactly the
/// operated pair, every order `Absent` (the post proved the scope's ledger
/// empty, so no order row exists); the rest only counted.
fn listing(scope: &ScanScope<'_>, triple: &Triple) -> Listing {
    let operated = scope.detector.operated;
    let orders = triple.orders.iter().filter(|o| o.symbol == operated);
    let lists = triple.lists.iter().filter(|l| l.symbol == operated);
    let orders: Vec<_> = orders
        .map(|o| OperatedOrder {
            symbol: o.symbol.clone(),
            order_id: o.order_id,
            attribution: Attribution::Absent,
        })
        .collect();
    let lists: Vec<_> = lists.map(|l| OperatedList { order_list_id: l.order_list_id }).collect();
    Listing {
        unrelated_orders: triple.orders.len() - orders.len(),
        unrelated_lists: triple.lists.len() - lists.len(),
        orders,
        lists,
    }
}
