//! The cash writer of one balance-account venue (ADR-0056 step 8l-3b; v16
//! §C8; PLAN-r4 §6.4-§6.6): one conclusion from one qualified attempt, in one
//! transaction holding the accounting rows. No production caller until 8k-3.
//!
//! A conclusion is evidence, never accounting: it writes the checkpoint
//! pointer, discrepancies and their sightings only, never an anchor or a flow
//! (8m's cutover and 8l-5's re-anchor do), and `E(t)` reads none of them.
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        LazyLock,
    },
    time::{Duration, Instant},
};

use chrono::{DateTime, Datelike, NaiveDate, Utc};
use robson_exec::{
    gate::{InvalidityMarks, OperationGate},
    venue_ledger::LedgerError,
};
use rust_decimal::Decimal;
use sqlx::{postgres::PgRow, Connection, PgPool, Row};
use uuid::Uuid;

use super::{
    super::{
        backend, commit_error,
        settlement::{asset_columns, CommissionAssets, SETTLEMENT_BARRIER},
        LedgerConnection,
    },
    expected_at, flag, DISCREPANCY,
};
use crate::{
    balance_accounting::{AccountingRefusal, BalanceAccounting, ACCOUNTING_ROWS, BOUNDS},
    balance_reconciliation::{BaseVerdict, ObservationInterval},
};

#[cfg(test)]
mod pg_tests;

/// The writer's one context read (`context.sql`): the barrier, `E(cut)`, the
/// open discrepancy and its own reads.
static CONTEXT: LazyLock<String> = LazyLock::new(|| {
    include_str!("context.sql")
        .replace("{barrier}", SETTLEMENT_BARRIER)
        .replace("{expected}", &expected_at("$8"))
        .replace("{discrepancy}", DISCREPANCY)
});

/// The pointer moves only forward; its writers keep it monotone (PLAN-r4
/// §6.11.2).
const CHECKPOINT: &str = "INSERT INTO public.cash_checkpoints AS k (tenant_id, venue_id, cut, send_at, receive_at, u_ns, sample_id, epoch_id, interval_start, interval_end, source)
    VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $3, $9, 'scan')
    ON CONFLICT (tenant_id, venue_id) DO UPDATE SET (cut, send_at, receive_at, u_ns, sample_id, epoch_id, interval_start, interval_end, source, anchor_id)
        = (EXCLUDED.cut, EXCLUDED.send_at, EXCLUDED.receive_at, EXCLUDED.u_ns, EXCLUDED.sample_id, EXCLUDED.epoch_id, EXCLUDED.interval_start, EXCLUDED.interval_end, 'scan', NULL)
    WHERE k.cut < EXCLUDED.cut";
/// A new discrepancy and its first sighting share one statement.
const OPEN: &str = "WITH d AS (INSERT INTO public.cash_discrepancies (discrepancy_id, tenant_id, venue_id, direction, state, first_send_at, first_receive_at, first_u_ns, first_sample_id, first_epoch_id, first_interval_start, first_interval_end, first_delta, checkpoint_cut, checkpoint_interval_start, checkpoint_interval_end, opened_version, last_seen_at, min_delta)
        VALUES ($10, $1, $2, $11, 'open', $4, $5, $6, $7, $8, $3, $9, $12, $13, $14, $15, $16, $5, $12) RETURNING discrepancy_id)
    INSERT INTO public.cash_sightings (discrepancy_id, seq, tenant_id, venue_id, send_at, receive_at, u_ns, sample_id, epoch_id, interval_start, interval_end, delta)
    SELECT discrepancy_id, 1, $1, $2, $4, $5, $6, $7, $8, $3, $9, $12 FROM d";
/// A sighting moves `last_seen_at` (its receipt) forward and the running
/// minimum down, with no bump (R11-4).
const SIGHT: &str = "WITH s AS (INSERT INTO public.cash_sightings (discrepancy_id, seq, tenant_id, venue_id, send_at, receive_at, u_ns, sample_id, epoch_id, interval_start, interval_end, delta)
        VALUES ($10, $11, $1, $2, $4, $5, $6, $7, $8, $3, $9, $12) RETURNING discrepancy_id)
    UPDATE public.cash_discrepancies d SET last_seen_at = greatest(d.last_seen_at, $5), min_delta = least(d.min_delta, $12)
    FROM s WHERE d.discrepancy_id = s.discrepancy_id AND d.state = 'open'";
const VOID: &str = "UPDATE public.cash_discrepancies SET state = 'void', void_reason = $2, closed_at = now() WHERE discrepancy_id = $1 AND state = 'open'";

const CLIENT_BOUND: Duration = Duration::from_secs(2);
/// C8's 60 s between the ACCOUNT receipt and publication (v16:460-462).
const FRESH: Duration = Duration::from_secs(60);
const TOLERANCE: Decimal = rust_decimal_macros::dec!(0.01);

/// What 8k-3 hands the writer (PLAN-r4 §6.4): a scan's verdict, which must be
/// a qualified `Clean`; `v₀` and the gate's invalidity marks, read before its
/// pre statement; and the ACCOUNT observation the verdict was drawn from.
#[derive(Debug, Clone)]
pub(crate) struct CashAttempt {
    pub(crate) verdict: BaseVerdict,
    pub(crate) version: i64,
    pub(crate) marks: InvalidityMarks,
    /// USDT free + locked in the same ACCOUNT body.
    pub(crate) quote_total: Decimal,
    /// u of the receipt's sample: the interval is [send − u, receive + u].
    pub(crate) u_ns: i64,
    pub(crate) sample_id: Uuid,
    pub(crate) epoch_id: Uuid,
    /// The ACCOUNT response's arrival on the monotonic clock.
    pub(crate) received: Instant,
}

/// An observation as stored (PLAN-r4 §4.3): 8k-2's nanosecond interval
/// converted once, the cut and the send floored to the microsecond, the
/// receipt and the end ceiled, so the stored interval only widens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Observed {
    pub(crate) cut: DateTime<Utc>,
    pub(crate) send_at: DateTime<Utc>,
    pub(crate) receive_at: DateTime<Utc>,
    pub(crate) end: DateTime<Utc>,
}

impl Observed {
    pub(crate) fn new(interval: ObservationInterval, u_ns: i64) -> Option<Self> {
        let micros = |ns: i128, up: bool| {
            let micros = ns.div_euclid(1_000) + i128::from(up && ns.rem_euclid(1_000) != 0);
            DateTime::from_timestamp_micros(i64::try_from(micros).ok()?)
        };
        let send = interval.start_ns.checked_add(i128::from(u_ns))?;
        let receive = interval.end_ns.checked_sub(i128::from(u_ns))?;
        if u_ns < 0 || interval.cut_ns != interval.start_ns || send > receive {
            return None;
        }
        Some(Self {
            cut: micros(interval.start_ns, false)?,
            send_at: micros(send, false)?,
            receive_at: micros(receive, true)?,
            end: micros(interval.end_ns, true)?,
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum CashRefusal {
    #[error("only a qualified Clean verdict concludes cash")]
    NotClean,
    #[error("read-only mode: no cash conclusion on this account")]
    ReadOnly,
    #[error("the observation's interval is malformed")]
    Malformed,
    #[error("the accounting version moved since pre")]
    VersionMoved,
    #[error("a leg is unsettled")]
    Unsettled,
    #[error("a transition is open")]
    Transition,
    #[error("the observation is not of the live process epoch")]
    Epoch,
    #[error("a booked fill lies inside the observation's interval")]
    DirtyObservation,
    #[error("E(cut) is undefined")]
    Undefined,
    #[error("the invalidity generation moved or evidence is pending")]
    Marks,
    #[error("publication more than 60 s after the receipt")]
    Stale,
    #[error(transparent)]
    Accounting(#[from] AccountingRefusal),
    #[error(transparent)]
    Ledger(#[from] LedgerError),
}

/// One write of a conclusion, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Write {
    /// Void the open discrepancy with this reason.
    Void(Uuid, &'static str),
    /// Open a discrepancy with its first sighting; true for missing cash.
    Open(bool),
    /// Sighting `seq` of the open discrepancy.
    Sight(Uuid, i32),
    /// The checkpoint, when this cut is later than the stored one.
    Checkpoint,
}

/// The open discrepancy as the context read it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OpenDiscrepancy {
    pub(crate) id: Uuid,
    pub(crate) missing: bool,
    pub(crate) last_seq: i32,
    pub(crate) clean_min: Option<Decimal>,
    pub(crate) clean_max: Option<Decimal>,
}

impl OpenDiscrepancy {
    /// A clean sighting of its direction survives (PLAN-r4 §6.10); without
    /// one it is void by late evidence.
    pub(crate) fn surviving(&self) -> bool {
        match self.missing {
            true => self.clean_min.is_some_and(|min| min < -TOLERANCE),
            false => self.clean_max.is_some_and(|max| max > TOLERANCE),
        }
    }
}

/// Step 5's branch table (PLAN-r4 §6.4): whether the version moves, then at
/// most two writes. An open discrepancy with no surviving clean sighting of
/// its direction is void by late evidence and the observation is judged as
/// with none open. Missing cash never writes a checkpoint, a clean
/// observation never voids it, and only an insert opens one, with the bump.
pub(crate) fn plan(open: Option<OpenDiscrepancy>, delta: Decimal) -> (bool, Vec<Write>) {
    let (late, open) = match open {
        Some(o) if !o.surviving() => (Some(Write::Void(o.id, "late_evidence")), None),
        open => (None, open),
    };
    let clean = delta.abs() <= TOLERANCE;
    let (bump, writes) = match open {
        None if clean => (false, vec![Write::Checkpoint]),
        None => (true, vec![Write::Open(delta < Decimal::ZERO)]),
        Some(o) if !o.missing && clean => {
            (true, vec![Write::Void(o.id, "clean_observation"), Write::Checkpoint])
        },
        Some(o) if !o.missing && delta < Decimal::ZERO => (true, vec![
            Write::Void(o.id, "superseded_by_missing"),
            Write::Open(true),
        ]),
        Some(o) if clean => (false, vec![Write::Checkpoint, Write::Sight(o.id, o.last_seq + 1)]),
        Some(o) => (false, vec![Write::Sight(o.id, o.last_seq + 1)]),
    };
    match late {
        Some(void) => (true, [vec![void], writes].concat()),
        None => (bump, writes),
    }
}

/// A published conclusion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Concluded {
    pub(crate) delta: Decimal,
    /// The version after the bump; `None` when nothing bumped.
    pub(crate) bumped: Option<i64>,
    pub(crate) writes: Vec<Write>,
}

/// A checkpoint copy: cut, interval start, interval end.
type CheckpointCopy = (DateTime<Utc>, DateTime<Utc>, DateTime<Utc>);

/// The context's refusals (step 3) and what the branch needs: `E(cut)`, the
/// open discrepancy and the checkpoint copy a new one takes.
fn admit(
    row: &PgRow,
) -> Result<(Decimal, Option<OpenDiscrepancy>, Option<CheckpointCopy>), CashRefusal> {
    if !flag(row, "venue_known")? {
        return Err(LedgerError::NoAccountingRow.into());
    }
    let unsettled: i64 = row.try_get("unsettled").map_err(backend)?;
    crate::metrics::observe_leg_settlement_pending(u64::try_from(unsettled).unwrap_or(u64::MAX));
    let refusal = [
        (unsettled != 0, CashRefusal::Unsettled),
        (!flag(row, "at_rest")?, CashRefusal::Transition),
        (!flag(row, "epoch_current")?, CashRefusal::Epoch),
        (flag(row, "observation_dirty")?, CashRefusal::DirtyObservation),
    ];
    if let Some((_, refusal)) = refusal.into_iter().find(|(refused, _)| *refused) {
        return Err(refusal);
    }
    let expected: Option<Decimal> = row.try_get("expected").map_err(backend)?;
    let open = match row.try_get::<Option<Uuid>, _>("discrepancy_id").map_err(backend)? {
        None => None,
        Some(id) => Some(OpenDiscrepancy {
            id,
            missing: row.try_get::<&str, _>("direction").map_err(backend)? == "missing",
            last_seq: row.try_get::<Option<i32>, _>("last_seq").map_err(backend)?.unwrap_or(0),
            clean_min: row.try_get("clean_min").map_err(backend)?,
            clean_max: row.try_get("clean_max").map_err(backend)?,
        }),
    };
    let copy = match (
        row.try_get("copy_cut").map_err(backend)?,
        row.try_get("copy_start").map_err(backend)?,
        row.try_get("copy_end").map_err(backend)?,
    ) {
        (Some(cut), Some(start), Some(end)) => Some((cut, start, end)),
        _ => None,
    };
    Ok((expected.ok_or(CashRefusal::Undefined)?, open, copy))
}

/// The cash writer of one balance-account daemon (PLAN-r4 §6.4), on the
/// daemon's accounting context, gate and process epoch.
pub(crate) struct CashWriter<'a> {
    pub(crate) pool: &'a PgPool,
    pub(crate) accounting: &'a BalanceAccounting,
    pub(crate) gate: &'a OperationGate,
    /// This process's epoch: an attempt of another is never published.
    pub(crate) epoch: Uuid,
    /// The venue rules' commission assets, as 8l-2's barrier takes them.
    pub(crate) assets: &'a [CommissionAssets<'a>],
    /// 8m's read-only mode (a uid mismatch): no conclusion on that account.
    pub(crate) read_only: bool,
}

impl CashWriter<'_> {
    /// One conclusion from one attempt, or a refusal with nothing written.
    /// At most nine statements: BEGIN, the bounds, the row lock, the context
    /// read, the bump and the generation CAS when the branch bumps, at most
    /// two writes, COMMIT. A refusal returns unfinished, so the guard retires
    /// the connection and the server rolls everything back; a lost COMMIT
    /// answer is `Uncertain`, and the next scan re-reads.
    pub(crate) async fn conclude(&self, attempt: &CashAttempt) -> Result<Concluded, CashRefusal> {
        let committing = AtomicBool::new(false);
        let result = tokio::time::timeout(CLIENT_BOUND, self.publish(attempt, &committing))
            .await
            .unwrap_or_else(|_| {
                let reason = "cash conclusion exceeded 2000 ms".to_string();
                Err(match committing.load(Ordering::Relaxed) {
                    true => LedgerError::Uncertain(reason).into(),
                    false => LedgerError::Backend(reason).into(),
                })
            });
        let outcome = match &result {
            Ok(done) if done.delta.abs() <= TOLERANCE => "clean",
            Ok(done) if done.delta < Decimal::ZERO => "missing",
            Ok(_) => "extra",
            Err(CashRefusal::Ledger(LedgerError::Uncertain(_))) => "uncertain",
            Err(_) => "refused",
        };
        crate::metrics::observe_cash_conclusion(outcome);
        result
    }

    async fn publish(
        &self,
        attempt: &CashAttempt,
        committing: &AtomicBool,
    ) -> Result<Concluded, CashRefusal> {
        let BaseVerdict::Clean { interval } = attempt.verdict else {
            return Err(CashRefusal::NotClean);
        };
        if self.read_only {
            return Err(CashRefusal::ReadOnly);
        }
        let observed = Observed::new(interval, attempt.u_ns).ok_or(CashRefusal::Malformed)?;
        let [symbols, bases, quotes] = asset_columns(self.assets)?;
        let accounting = self.accounting;
        let mut held = LedgerConnection::new(self.pool.acquire().await.map_err(backend)?);
        let mut tx = held.connection().begin().await.map_err(backend)?;
        sqlx::query(BOUNDS).execute(&mut *tx).await.map_err(backend)?;
        // 8m-2a: `participate()` goes here, before the row lock (8m PLAN-r4 §6.7).
        let locked: Vec<(String, i64)> = sqlx::query_as(ACCOUNTING_ROWS)
            .bind(accounting.tenant_id)
            .fetch_all(&mut *tx)
            .await
            .map_err(backend)?;
        let version = locked.iter().find(|(venue, _)| venue == accounting.venue.as_str());
        if version.ok_or(LedgerError::NoAccountingRow)?.1 != attempt.version {
            return Err(CashRefusal::VersionMoved);
        }
        let cut = observed.cut.date_naive();
        let month =
            NaiveDate::from_ymd_opt(cut.year(), cut.month(), 1).ok_or(CashRefusal::Malformed)?;
        let row = sqlx::query(&CONTEXT)
            .bind(accounting.tenant_id)
            .bind(accounting.venue.as_str())
            .bind(month)
            .bind(Utc::now())
            .bind(&symbols)
            .bind(&bases)
            .bind(&quotes)
            .bind(observed.cut)
            .bind(observed.end)
            .bind(attempt.epoch_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(backend)?;
        robson_exec::crash_point!(CASH_AFTER_CONTEXT);
        let (expected, open, copy) = admit(&row)?;
        // Step 4, holding the row and under the gate mutex: a memory
        // invalidator is ordered before (refused here) or after (its booking
        // waits for this row; a fill inside the interval makes it dirty).
        let marks = self.gate.invalidity_marks();
        if marks.generation != attempt.marks.generation
            || marks.pending.blocks_publication()
            || attempt.marks.pending.blocks_publication()
        {
            return Err(CashRefusal::Marks);
        }
        if attempt.epoch_id != self.epoch {
            return Err(CashRefusal::Epoch);
        }
        if attempt.received.elapsed() > FRESH {
            return Err(CashRefusal::Stale);
        }
        let delta =
            attempt.quote_total.checked_sub(expected).ok_or(LedgerError::Unrepresentable)?;
        let (bump, writes) = plan(open, delta);
        let bumped = match bump {
            true => Some(accounting.advance(&mut tx).await?.ok_or(LedgerError::NoAccountingRow)?),
            false => None,
        };
        for write in &writes {
            let observation = |sql: &'static str| {
                sqlx::query(sql)
                    .bind(accounting.tenant_id)
                    .bind(accounting.venue.as_str())
                    .bind(observed.cut)
                    .bind(observed.send_at)
                    .bind(observed.receive_at)
                    .bind(attempt.u_ns)
                    .bind(attempt.sample_id)
                    .bind(attempt.epoch_id)
                    .bind(observed.end)
            };
            let landed = match *write {
                Write::Checkpoint => observation(CHECKPOINT).execute(&mut *tx).await.map(|_| true),
                Write::Void(id, reason) => sqlx::query(VOID)
                    .bind(id)
                    .bind(reason)
                    .execute(&mut *tx)
                    .await
                    .map(|done| done.rows_affected() == 1),
                Write::Sight(id, seq) => observation(SIGHT)
                    .bind(id)
                    .bind(seq)
                    .bind(delta)
                    .execute(&mut *tx)
                    .await
                    .map(|done| done.rows_affected() == 1),
                Write::Open(missing) => {
                    let (cut, start, end) = copy.ok_or(CashRefusal::Undefined)?;
                    observation(OPEN)
                        .bind(Uuid::now_v7())
                        .bind(if missing { "missing" } else { "extra" })
                        .bind(delta)
                        .bind(cut)
                        .bind(start)
                        .bind(end)
                        .bind(bumped)
                        .execute(&mut *tx)
                        .await
                        .map(|done| done.rows_affected() == 1)
                },
            };
            if !landed.map_err(backend)? {
                return Err(LedgerError::Invariant("the open discrepancy moved".into()).into());
            }
        }
        robson_exec::crash_point!(CASH_BEFORE_COMMIT);
        committing.store(true, Ordering::Relaxed);
        tx.commit().await.map_err(commit_error)?;
        held.finished = true;
        Ok(Concluded { delta, bumped, writes })
    }
}
