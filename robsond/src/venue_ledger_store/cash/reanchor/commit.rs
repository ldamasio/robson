//! 8l-5b (PLAN-r4 §6.12, §6.5, §6.6, §6.10; operator decisions A and Q2 = R):
//! the re-anchor's one transaction. It takes the accounting rows by the bump,
//! which must land on `v₀ + 1`, advances the order-set generation, reads its
//! context, checks the gate's marks again under its mutex, and writes the new
//! anchor, the re-anchor row, its flows, the checkpoint and the discrepancy's
//! closure in one statement: seven statements in all (six with no installed
//! generation). Any refusal returns unfinished, so the guard retires the
//! connection and the server rolls everything back.
//!
//! An attested re-anchor keeps `E(t)` continuous: `Δ` against `E_old`, the
//! minimum over the open discrepancy's clean sightings, Δ(O1), Δ(O2) and 0,
//! `W = −min Δ` dated at the later of the effective dating copy and the old
//! anchor's `as_of`, `C = Δ(O2) + W` dated at O1's end, so `C − W = Δ`. A
//! recovery re-anchor (Q2 = R) replaces an anchor whose own hull late
//! evidence made dirty: no flow, no amounts; the months from the latest
//! clean-hull anchor's through its own are invalid at read time.
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        LazyLock,
    },
    time::Duration,
};

use chrono::{DateTime, Datelike, NaiveDate, Utc};
use robson_exec::venue_ledger::{exact_sub, LedgerError};
use rust_decimal::Decimal;
use sqlx::Connection;
use uuid::Uuid;

use super::{CashReanchor, Evidence, ReanchorRefusal, Reanchored, TOLERANCE};
use crate::{
    balance_accounting::{AccountingRefusal, BOUNDS},
    ledger_connection::LedgerConnection,
    venue_ledger_store::{
        cash::{expected_at, writer::OpenDiscrepancy, DISCREPANCY},
        commit_error,
        settlement::{asset_columns, SETTLEMENT_BARRIER},
    },
};

#[cfg(test)]
mod pg_tests;

/// The context read (`context.sql`): the barrier, `E_old` at both cuts and
/// the open discrepancy embedded.
static CONTEXT: LazyLock<String> = LazyLock::new(|| {
    include_str!("context.sql")
        .replace("{barrier}", SETTLEMENT_BARRIER)
        .replace("{expected_o1}", &expected_at("$8"))
        .replace("{expected_o2}", &expected_at("$9"))
        .replace("{discrepancy}", DISCREPANCY)
});
const WRITE: &str = include_str!("write.sql");

const CLIENT_BOUND: Duration = Duration::from_secs(2);
/// C8's 60 s from the observation to publication (v16:460-462), from before
/// O1's capture.
const FRESH: Duration = Duration::from_secs(60);

/// What the context read (`context.sql` documents each column).
#[derive(Debug, Clone, sqlx::FromRow)]
pub(crate) struct Context {
    pub(crate) unsettled: i64,
    pub(crate) venue_known: bool,
    pub(crate) at_rest: bool,
    pub(crate) anchor_id: Option<Uuid>,
    pub(crate) anchor_as_of: Option<DateTime<Utc>>,
    pub(crate) anchor_dirty: bool,
    pub(crate) anchor_o2: Option<Uuid>,
    pub(crate) impaired: bool,
    pub(crate) expected_o1: Option<Decimal>,
    pub(crate) expected_o2: Option<Decimal>,
    pub(crate) discrepancy_id: Option<Uuid>,
    pub(crate) missing: Option<bool>,
    pub(crate) clean_min: Option<Decimal>,
    pub(crate) clean_max: Option<Decimal>,
    pub(crate) dirty_seqs: Option<Vec<i32>>,
    pub(crate) copy_cut: Option<DateTime<Utc>>,
    pub(crate) copy_start: Option<DateTime<Utc>>,
    pub(crate) copy_end: Option<DateTime<Utc>>,
    pub(crate) checkpoint_cut: Option<DateTime<Utc>>,
    pub(crate) checkpoint_start: Option<DateTime<Utc>>,
    pub(crate) checkpoint_end: Option<DateTime<Utc>>,
    pub(crate) clean_as_of: Option<DateTime<Utc>>,
    pub(crate) clean_start: Option<DateTime<Utc>>,
    pub(crate) clean_end: Option<DateTime<Utc>>,
    pub(crate) recovery_anchor: Option<Uuid>,
    pub(crate) hull_dirty: bool,
    pub(crate) epoch_current: bool,
}

/// A copy of an observation's dating: cut, interval start, interval end.
type CheckpointCopy = (DateTime<Utc>, DateTime<Utc>, DateTime<Utc>);

fn copy(
    cut: Option<DateTime<Utc>>,
    start: Option<DateTime<Utc>>,
    end: Option<DateTime<Utc>>,
) -> Option<CheckpointCopy> {
    Some((cut?, start?, end?))
}

/// What the write statement writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Plan {
    /// Operator Q2 = R: no flow, no dating copy, no amounts.
    pub(crate) recovery: bool,
    pub(crate) old_anchor: Uuid,
    /// The effective dating copy (attested only).
    pub(crate) dating: Option<CheckpointCopy>,
    /// Δ(O2), min Δ, W and C (attested only).
    pub(crate) amounts: Option<[Decimal; 4]>,
    /// W's date: the later of the dating cut and the old anchor's `as_of`.
    pub(crate) withdrawal_at: Option<DateTime<Utc>>,
    pub(crate) resolved: Option<Uuid>,
    /// The resolved discrepancy's sightings already dirty at this commit.
    pub(crate) excluded: Vec<i32>,
    pub(crate) voided: Option<Uuid>,
    /// Whether the flows are written: with a surviving open discrepancy,
    /// or |Δ(O1)| or |Δ(O2)| above 0.01 (v16:551-552).
    pub(crate) flows: bool,
}

/// The context's refusals and the write's plan from O1's and O2's totals.
pub(crate) fn plan(c: &Context, totals: [Decimal; 2]) -> Result<Plan, ReanchorRefusal> {
    use ReanchorRefusal as R;
    let refused = [
        (!c.venue_known, R::Unavailable),
        (c.unsettled != 0 || !c.at_rest, R::NotAtRest),
        (!c.epoch_current, R::Epoch),
        (c.hull_dirty, R::DirtyHull),
    ];
    if let Some((_, refusal)) = refused.into_iter().find(|(refused, _)| *refused) {
        return Err(refusal);
    }
    let (anchor, as_of) = c.anchor_id.zip(c.anchor_as_of).ok_or(R::NoAnchor)?;
    if c.anchor_o2 != Some(anchor) {
        return Err(R::AnchorOverlap);
    }
    // A discrepancy with no surviving clean sighting of its direction is
    // void by late evidence and counts as none open (PLAN-r4 §6.10).
    let open = c.discrepancy_id.map(|id| OpenDiscrepancy {
        id,
        missing: c.missing == Some(true),
        last_seq: 0,
        clean_min: c.clean_min,
        clean_max: c.clean_max,
    });
    let surviving = open.filter(OpenDiscrepancy::surviving);
    let resolved = surviving.map(|open| open.id);
    let base = Plan {
        recovery: false,
        old_anchor: anchor,
        dating: None,
        amounts: None,
        withdrawal_at: None,
        resolved,
        excluded: resolved.and(c.dirty_seqs.clone()).unwrap_or_default(),
        voided: open.filter(|open| !open.surviving()).map(|open| open.id),
        flows: false,
    };
    if c.impaired {
        // Q2 = R recovers only E's own anchor with a dirty hull (kind (A));
        // the old anchor is the latest clean-hull one, else the first.
        let old_anchor = c.recovery_anchor.ok_or(R::NoAnchor)?;
        return match c.anchor_dirty {
            true => Ok(Plan { recovery: true, old_anchor, ..base }),
            false => Err(R::ImpairedChain),
        };
    }
    let (e1, e2) = c.expected_o1.zip(c.expected_o2).ok_or(R::Undefined)?;
    // Exact: an overflow or a rounded difference refuses (review r1).
    let first = exact_sub(totals[0], e1).map_err(|_| R::Undefined)?;
    let delta = exact_sub(totals[1], e2).map_err(|_| R::Undefined)?;
    let floor = surviving.and_then(|open| open.clean_min).unwrap_or(Decimal::ZERO);
    let min = floor.min(first).min(delta).min(Decimal::ZERO);
    let contribution = exact_sub(delta, min).map_err(|_| R::Undefined)?;
    let dating = match surviving {
        Some(_) => copy(c.copy_cut, c.copy_start, c.copy_end),
        None => copy(c.checkpoint_cut, c.checkpoint_start, c.checkpoint_end),
    };
    let dating = dating
        .or(copy(c.clean_as_of, c.clean_start, c.clean_end))
        .ok_or(R::ImpairedChain)?;
    Ok(Plan {
        dating: Some(dating),
        amounts: Some([delta, min, -min, contribution]),
        withdrawal_at: Some(dating.0.max(as_of)),
        flows: surviving.is_some() || first.abs() > TOLERANCE || delta.abs() > TOLERANCE,
        ..base
    })
}

/// A database or bound failure: nothing committed; the cause is logged.
fn unavailable<E: std::fmt::Display>(error: E) -> ReanchorRefusal {
    tracing::warn!(%error, "cash re-anchor: the database refused or failed");
    ReanchorRefusal::Unavailable
}

impl CashReanchor {
    /// The transaction under the 2 s client bound: a timeout before COMMIT
    /// is a refusal, one after it was sent is `uncertain` (a retry re-reads
    /// and, the first having committed, sees Δ ≈ 0 and writes no flow).
    pub(super) async fn commit(
        &self,
        subject: &str,
        reason: &str,
        evidence: &Evidence,
    ) -> Result<Reanchored, ReanchorRefusal> {
        let committing = AtomicBool::new(false);
        let work = self.transact(subject, reason, evidence, &committing);
        tokio::time::timeout(CLIENT_BOUND, work).await.unwrap_or_else(|_| {
            Err(match committing.load(Ordering::Relaxed) {
                true => ReanchorRefusal::Uncertain,
                false => ReanchorRefusal::Unavailable,
            })
        })
    }

    async fn transact(
        &self,
        subject: &str,
        reason: &str,
        evidence: &Evidence,
        committing: &AtomicBool,
    ) -> Result<Reanchored, ReanchorRefusal> {
        let (o1, o2, accounting) = (&evidence.o1, &evidence.o2, &self.accounting);
        let [symbols, bases, quotes] = asset_columns(&[self.pair]).map_err(unavailable)?;
        let mut held = LedgerConnection::new(self.pool.acquire().await.map_err(unavailable)?);
        let mut tx = held.connection().begin().await.map_err(unavailable)?;
        sqlx::query(BOUNDS).execute(&mut *tx).await.map_err(unavailable)?;
        // 8m-2a: `participate()` goes here, before the row lock (8m PLAN-r4 §6.7).
        let version = accounting.advance(&mut tx).await.map_err(|refusal| match refusal {
            AccountingRefusal::Database(_) => ReanchorRefusal::Unavailable,
            _ => ReanchorRefusal::StaleGeneration,
        })?;
        if version != evidence.version.checked_add(1) {
            return Err(ReanchorRefusal::VersionMoved);
        }
        let cut = o2.observed.cut.date_naive();
        let month = NaiveDate::from_ymd_opt(cut.year(), cut.month(), 1)
            .ok_or(ReanchorRefusal::Unavailable)?;
        let context: Context = sqlx::query_as(&CONTEXT)
            .bind(accounting.tenant_id)
            .bind(accounting.venue.as_str())
            .bind(month)
            .bind(Utc::now())
            .bind(&symbols)
            .bind(&bases)
            .bind(&quotes)
            .bind(o1.observed.cut)
            .bind(o2.observed.cut)
            .bind(o2.observed.end)
            .bind(o2.reading.epoch_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(unavailable)?;
        let plan = plan(&context, [o1.total, o2.total])?;
        // PLAN-r4 §6.4 step 4, holding the rows, under the gate mutex: a
        // memory invalidator is ordered before (refused here) or after.
        let marks = self.gate.invalidity_marks();
        if marks.generation != evidence.marks.generation
            || marks.pending.blocks_publication()
            || evidence.marks.pending.blocks_publication()
        {
            return Err(ReanchorRefusal::Marks);
        }
        if o1.reading.epoch_id != self.epoch || o2.reading.epoch_id != self.epoch {
            return Err(ReanchorRefusal::Epoch);
        }
        if evidence.requested.elapsed() > FRESH {
            return Err(ReanchorRefusal::Stale);
        }
        let (reanchor_id, mode) = (
            Uuid::now_v7(),
            if plan.recovery {
                "recovery"
            } else {
                "attested"
            },
        );
        let [delta, min_delta, withdrawal, contribution] =
            plan.amounts.map_or([None; 4], |a| a.map(Some));
        let dating = plan.dating;
        let (closed, flows): (i64, i64) = sqlx::query_as(WRITE)
            .bind(accounting.tenant_id)
            .bind(accounting.venue.as_str())
            .bind(Uuid::now_v7())
            .bind(o2.total)
            .bind(o2.observed.cut)
            .bind(o2.observed.send_at)
            .bind(o2.observed.receive_at)
            .bind(o2.reading.u_ns)
            .bind(o2.reading.sample_id)
            .bind(o2.reading.epoch_id)
            .bind(o2.observed.end)
            .bind(o1.observed.cut)
            .bind(reanchor_id)
            .bind(subject)
            .bind(reason)
            .bind(o1.observed.send_at)
            .bind(o1.observed.receive_at)
            .bind(o1.reading.u_ns)
            .bind(o1.reading.sample_id)
            .bind(o1.reading.epoch_id)
            .bind(o1.observed.end)
            .bind(o1.total)
            .bind(mode)
            .bind(evidence.version)
            .bind(plan.old_anchor)
            .bind(dating.map(|d| d.0))
            .bind(dating.map(|d| d.1))
            .bind(dating.map(|d| d.2))
            .bind(delta)
            .bind(min_delta)
            .bind(withdrawal)
            .bind(contribution)
            .bind(plan.resolved)
            .bind(&plan.excluded)
            .bind(plan.voided)
            .bind(plan.withdrawal_at)
            .bind(plan.flows)
            .fetch_one(&mut *tx)
            .await
            .map_err(unavailable)?;
        // The statement wrote exactly what the plan says, or nothing commits.
        let written =
            |amount: Option<Decimal>| plan.flows && amount.is_some_and(|a| a > Decimal::ZERO);
        let expected = (written(withdrawal), written(contribution));
        let closes = i64::from(plan.resolved.is_some() || plan.voided.is_some());
        if (closed, flows) != (closes, i64::from(expected.0) + i64::from(expected.1)) {
            return Err(unavailable(format!("the write closed {closed} and wrote {flows} flows")));
        }
        // 8p: its reopen statement goes here (PLAN-r4 §6.12).
        robson_exec::crash_point!(REANCHOR_BEFORE_COMMIT);
        committing.store(true, Ordering::Relaxed);
        tx.commit().await.map_err(|error| match commit_error(error) {
            LedgerError::Uncertain(_) => ReanchorRefusal::Uncertain,
            _ => ReanchorRefusal::Unavailable,
        })?;
        held.finished = true;
        let flowed = [("withdrawal", expected.0), ("contribution", expected.1)];
        let direction = ["none", "withdrawal", "contribution", "both"]
            [usize::from(expected.0) + 2 * usize::from(expected.1)];
        crate::metrics::observe_cash_reanchor(if plan.recovery { "recovery" } else { direction });
        flowed
            .iter()
            .filter(|f| f.1)
            .for_each(|f| crate::metrics::observe_external_flow(f.0));
        tracing::info!(%reanchor_id, mode, subject, reason, version = evidence.version + 1, ?delta,
            ?withdrawal, ?contribution, flows, "cash re-anchor committed");
        Ok(Reanchored {
            reanchor_id,
            mode,
            version: evidence.version + 1,
            delta,
            withdrawal,
            contribution,
            flows: u64::try_from(flows).unwrap_or_default(),
        })
    }
}
