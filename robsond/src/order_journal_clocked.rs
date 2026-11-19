//! Private clocked Spot branch of the accounting-first balance journal (J2b).
//! One migration-38 `budget_insert_clocks` call replaces the five-field clock
//! insert for an explicitly supplied attempt capture. The receipt is
//! descriptive storage evidence: it confers no readiness, qualification or
//! permission to send.
use std::{
    collections::HashSet,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

use chrono::{DateTime, TimeDelta, TimeZone, Utc};
use robson_exec::{
    clock_records::{RawClockCapture, TICK_MAX},
    order_journal::{CommitFailure, IntentKind, JournalBatch},
    venue::VenueId,
    venue_clock::{ClockDomain, TickCapture},
    venue_ledger::{ClockEvidence, ClockVerdict},
};
use sqlx::{postgres::PgRow, Connection, Postgres, Row, Transaction};
use tokio::time::Instant;
use tracing::Instrument as _;
use uuid::Uuid;

use super::PgJournalBackend;
use crate::{ledger_connection::LedgerConnection, venue_clock_store::codec};

/// J's existing commit bound; a batch may only shorten it.
const CLOCKED_BOUND: Duration = Duration::from_millis(500);
/// M37 and J: one Spot attempt per client order id.
const MAX_ATTEMPTS: usize = 1;
/// Fields returned by `budget_insert_clocks`.
const RECEIPT_FIELDS: usize = 36;

const INSERT_CLOCKS: &str = "SELECT * FROM public.budget_insert_clocks(\
    $1::uuid[],$2,$3,$4,$5::timestamptz[],$6::uuid[],$7::bigint[],$8::bigint[],$9::text[],\
    $10::text[],$11::uuid[],$12::bigint[],$13::integer[],$14::numeric[],\
    $15::uuid[],$16::bigint[],$17::integer[],$18::numeric[],$19::numeric[],\
    $20::text[],$21::uuid[],$22::bigint[],$23::integer[])";

fn refused(why: &str) -> CommitFailure {
    CommitFailure::Known(format!("clocked journal refused: {why}"))
}

fn mismatch(why: &str) -> CommitFailure {
    CommitFailure::Known(format!("clock receipt mismatch: {why}"))
}

/// One attempt's frozen evidence, in the batch's attempt order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ClockedAttempt {
    pub(crate) attempt_id: Uuid,
    pub(crate) intent_id: Uuid,
    pub(crate) tenant_id: Uuid,
    pub(crate) venue_id: String,
    pub(crate) account_scope: String,
    pub(crate) client_order_id: String,
    pub(crate) kind: IntentKind,
    /// Original same-wall capture; its wall instant is the attempt's
    /// `created_at`.
    pub(crate) capture: RawClockCapture,
    pub(crate) sample: Option<ClockEvidence>,
    /// Independent pretransaction budget tick, never derived from `capture`.
    pub(crate) budget: TickCapture,
}

/// Supplied only by a private authorized owner. Its process epoch is the
/// enclosing caller's, which every capture must match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ClockedInput {
    pub(crate) process_epoch: Uuid,
    pub(crate) attempts: Vec<ClockedAttempt>,
}

/// Descriptive per-attempt storage receipt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ClockReceipt {
    pub(crate) attempt_id: Uuid,
    pub(crate) daily_units: i16,
    /// A prior retired row observed as it is; never enlarged or re-qualified.
    pub(crate) retired: bool,
}

/// Checked binds for the one helper call and the receipt each must return.
pub(super) struct Prepared {
    rows: Vec<Expected>,
}

struct Expected {
    attempt_id: Uuid,
    /// The original wall instant, bound as the source attempt was.
    wall: DateTime<Utc>,
    /// SQLx's microsecond projection of `wall`, as PostgreSQL stores it.
    stored: DateTime<Utc>,
    capture: RawClockCapture,
    sample: Option<ClockEvidence>,
    budget_domain: ClockDomain,
    budget_tick: i128,
}

/// SQLx 0.8.6 encodes `timestamptz` as whole microseconds since PostgreSQL's
/// epoch; this is the value a readback must equal.
fn postgres_microseconds(at: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let epoch = Utc.with_ymd_and_hms(2000, 1, 1, 0, 0, 0).single()?;
    let micros = at.signed_duration_since(epoch).num_microseconds()?;
    epoch.checked_add_signed(TimeDelta::microseconds(micros))
}

fn units(kind: IntentKind) -> i16 {
    i16::from(kind != IntentKind::Cancel)
}

fn verdict_name(verdict: ClockVerdict) -> &'static str {
    match verdict {
        ClockVerdict::Valid => "valid",
        ClockVerdict::Invalid => "invalid",
    }
}

impl PgJournalBackend {
    /// Match the supplied evidence to the batch before any SQL. Zero attempts
    /// prepare no helper call; two or more refuse.
    pub(super) fn prepare_clocked(
        &self,
        batch: &JournalBatch,
        input: &ClockedInput,
    ) -> Result<Prepared, CommitFailure> {
        let intent = &batch.intent;
        if batch.balance.is_none() {
            return Err(refused("not an accounting-first batch"));
        }
        if intent.venue_id != VenueId::BINANCE_SPOT {
            return Err(refused("not a Spot intent"));
        }
        if input.process_epoch.is_nil() || intent.intent_id.is_nil() || self.tenant_id.is_nil() {
            return Err(refused("nil identity"));
        }
        if batch.attempts.len() > MAX_ATTEMPTS {
            return Err(refused("more than one attempt for one client order id"));
        }
        if input.attempts.len() != batch.attempts.len() {
            return Err(refused("evidence does not cover exactly the batch's attempts"));
        }
        let mut seen = HashSet::new();
        let mut rows = Vec::with_capacity(batch.attempts.len());
        for (attempt, clocked) in batch.attempts.iter().zip(&input.attempts) {
            if attempt.attempt_id.is_nil()
                || attempt.attempt_id != clocked.attempt_id
                || !seen.insert(attempt.attempt_id)
            {
                return Err(refused("attempt identity"));
            }
            if (
                clocked.intent_id,
                clocked.tenant_id,
                clocked.venue_id.as_str(),
                clocked.account_scope.as_str(),
                clocked.client_order_id.as_str(),
                clocked.kind,
            ) != (
                intent.intent_id,
                self.tenant_id,
                intent.venue_id.as_str(),
                intent.account_scope.as_str(),
                intent.client_order_id.as_str(),
                intent.kind,
            ) {
                return Err(refused("foreign intent, tenant, venue, scope, client or kind"));
            }
            let capture = clocked.capture;
            if clocked.sample.as_ref().is_some_and(|sample| sample.sample_id.is_nil()) {
                return Err(refused("nil sample identity"));
            }
            if capture.epoch != input.process_epoch {
                return Err(refused("capture from another process epoch"));
            }
            capture.check().map_err(|_| refused("malformed capture"))?;
            let wall = DateTime::from_timestamp(capture.wall_seconds, capture.wall_nanoseconds)
                .ok_or_else(|| refused("unrepresentable capture wall"))?;
            if wall != attempt.created_at {
                return Err(refused("attempt time is not its captured wall instant"));
            }
            let stored =
                postgres_microseconds(wall).ok_or_else(|| refused("unencodable attempt time"))?;
            let budget_tick = i128::try_from(clocked.budget.nanoseconds())
                .ok()
                .filter(|tick| (0..=TICK_MAX).contains(tick))
                .ok_or_else(|| refused("budget tick out of range"))?;
            rows.push(Expected {
                attempt_id: attempt.attempt_id,
                wall,
                stored,
                capture,
                sample: clocked.sample.clone(),
                budget_domain: clocked.budget.domain(),
                budget_tick,
            });
        }
        Ok(Prepared { rows })
    }

    /// The one helper call at the five-field insert's place, and its receipt.
    pub(super) async fn insert_clocks(
        &self,
        batch: &JournalBatch,
        prepared: &Prepared,
        tx: &mut Transaction<'_, Postgres>,
    ) -> Result<Vec<ClockReceipt>, CommitFailure> {
        if prepared.rows.is_empty() {
            return Ok(Vec::new());
        }
        let n = prepared.rows.len();
        let encode = |_| refused("unencodable tick");
        let intent = &batch.intent;
        let mut budget_ticks = Vec::with_capacity(n);
        let mut before = Vec::with_capacity(n);
        let mut after = Vec::with_capacity(n);
        for row in &prepared.rows {
            budget_ticks.push(codec::decimal(row.budget_tick, 0, TICK_MAX).map_err(encode)?);
            before.push(codec::decimal(row.capture.before_ns, 0, TICK_MAX).map_err(encode)?);
            after.push(codec::decimal(row.capture.after_ns, 0, TICK_MAX).map_err(encode)?);
        }
        let wanted = &prepared.rows;
        let boottime = vec!["boottime".to_owned(); n];
        let rows = sqlx::query(INSERT_CLOCKS)
            .bind(column(wanted, |r| r.attempt_id))
            .bind(intent.intent_id)
            .bind(self.tenant_id)
            .bind(&intent.venue_id)
            .bind(column(wanted, |r| r.wall))
            .bind(column(wanted, |row| row.sample.as_ref().map(|e| e.sample_id)))
            .bind(column(wanted, |row| row.sample.as_ref().map(|e| e.wall_delta_ms)))
            .bind(column(wanted, |row| row.sample.as_ref().map(|e| e.mono_delta_ms)))
            .bind(column(wanted, |row| row.sample.as_ref().map(|e| verdict_name(e.verdict))))
            .bind(&boottime)
            .bind(column(wanted, |r| r.budget_domain.boot()))
            .bind(column(wanted, |r| r.budget_domain.offset().0))
            .bind(column(wanted, |r| r.budget_domain.offset().1 as i32))
            .bind(&budget_ticks)
            .bind(column(wanted, |r| r.capture.epoch))
            .bind(column(wanted, |r| r.capture.wall_seconds))
            .bind(column(wanted, |r| r.capture.wall_nanoseconds as i32))
            .bind(&before)
            .bind(&after)
            .bind(&boottime)
            .bind(column(wanted, |r| r.capture.domain.boot()))
            .bind(column(wanted, |r| r.capture.domain.offset().0))
            .bind(column(wanted, |r| r.capture.domain.offset().1 as i32))
            .fetch_all(&mut **tx)
            .instrument(
                tracing::info_span!(target: "robson_operation", "operation_io", io_kind = 1_u64),
            )
            .await
            .map_err(|error| CommitFailure::Known(error.to_string()))?;
        self.check_receipt(batch, prepared, &rows)
    }

    /// Every returned field against what was requested and the source rows.
    pub(super) fn check_receipt(
        &self,
        batch: &JournalBatch,
        prepared: &Prepared,
        rows: &[PgRow],
    ) -> Result<Vec<ClockReceipt>, CommitFailure> {
        let intent = &batch.intent;
        if rows.len() != prepared.rows.len() {
            return Err(mismatch("missing or extra rows"));
        }
        let mut seen = HashSet::new();
        let mut receipts = Vec::with_capacity(rows.len());
        for row in rows {
            if row.len() != RECEIPT_FIELDS {
                return Err(mismatch("field count"));
            }
            let id: Uuid = get(row, "attempt_id")?;
            let expected = prepared
                .rows
                .iter()
                .find(|e| e.attempt_id == id)
                .ok_or_else(|| mismatch("unrequested attempt"))?;
            if !seen.insert(id) {
                return Err(mismatch("repeated attempt"));
            }
            let identity: (Option<Uuid>, Option<Uuid>, Option<String>, Option<DateTime<Utc>>) = (
                get(row, "intent_id")?,
                get(row, "tenant_id")?,
                get(row, "venue_id")?,
                get(row, "attempt_created_at")?,
            );
            if identity
                != (
                    Some(intent.intent_id),
                    Some(self.tenant_id),
                    Some(intent.venue_id.clone()),
                    Some(expected.stored),
                )
            {
                return Err(mismatch("attempt identity or time"));
            }
            let sample: (Option<Uuid>, Option<i64>, Option<i64>, Option<String>) = (
                get(row, "clock_sample_id")?,
                get(row, "clock_wall_delta_ms")?,
                get(row, "clock_mono_delta_ms")?,
                get(row, "clock_verdict")?,
            );
            let frozen = expected.sample.as_ref();
            if (sample.0, sample.1, sample.2, sample.3.as_deref())
                != (
                    frozen.map(|e| e.sample_id),
                    frozen.map(|e| e.wall_delta_ms),
                    frozen.map(|e| e.mono_delta_ms),
                    frozen.map(|e| verdict_name(e.verdict)),
                )
            {
                return Err(mismatch("clock sample or verdict"));
            }
            let budget_domain = domain(row, "budget_kind", "budget_boot", "budget_offset")?;
            let budget_tick = codec::integer(row, "budget_created_tick", 0, TICK_MAX)
                .map_err(|_| mismatch("budget tick"))?;
            if (budget_domain, budget_tick) != (expected.budget_domain, expected.budget_tick) {
                return Err(mismatch("budget coordinates"));
            }
            // Server-generated and descriptive: present, never interpreted.
            if get::<Option<DateTime<Utc>>>(row, "budget_recorded_at")?.is_none() {
                return Err(mismatch("budget record time"));
            }
            let capture = RawClockCapture {
                epoch: get::<Option<Uuid>>(row, "capture_epoch")?
                    .ok_or_else(|| mismatch("capture epoch"))?,
                wall_seconds: get::<Option<i64>>(row, "capture_wall_seconds")?
                    .ok_or_else(|| mismatch("capture wall"))?,
                wall_nanoseconds: get::<Option<i32>>(row, "capture_wall_nanoseconds")?
                    .and_then(|n| u32::try_from(n).ok())
                    .ok_or_else(|| mismatch("capture wall"))?,
                domain: domain(row, "capture_kind", "capture_boot", "capture_offset")?,
                before_ns: codec::integer(row, "capture_tick_before", 0, TICK_MAX)
                    .map_err(|_| mismatch("capture tick"))?,
                after_ns: codec::integer(row, "capture_tick_after", 0, TICK_MAX)
                    .map_err(|_| mismatch("capture tick"))?,
            };
            if capture != expected.capture {
                return Err(mismatch("capture coordinates"));
            }
            let charge: (Option<String>, Option<String>, Option<String>, Option<i16>) = (
                get(row, "account_scope")?,
                get(row, "client_order_id")?,
                get(row, "kind")?,
                get(row, "daily_units")?,
            );
            if charge
                != (
                    Some(intent.account_scope.clone()),
                    Some(intent.client_order_id.clone()),
                    Some(intent.kind.as_str().to_owned()),
                    Some(units(intent.kind)),
                )
            {
                return Err(mismatch("charge route, client, kind or units"));
            }
            let retired = match (
                get::<Option<Uuid>>(row, "expiry_proof_id")?,
                get::<Option<DateTime<Utc>>>(row, "retired_at")?,
            ) {
                (None, None) => false,
                (Some(proof), Some(_)) if !proof.is_nil() => true,
                _ => return Err(mismatch("unpaired retirement")),
            };
            receipts.push(ClockReceipt {
                attempt_id: id,
                daily_units: units(intent.kind),
                retired,
            });
        }
        Ok(receipts)
    }

    /// Private clocked entry: J's accounting-first transaction with the clock
    /// row written by the one helper. One absolute deadline covers validation
    /// through COMMIT; a COMMIT without a timely answer is uncertain.
    pub(crate) async fn commit_clocked(
        &self,
        batch: &JournalBatch,
        input: &ClockedInput,
    ) -> Result<Vec<ClockReceipt>, CommitFailure> {
        let started = std::time::Instant::now();
        let Some(balance) = &batch.balance else {
            return Err(refused("not an accounting-first batch"));
        };
        let bound = balance.commit_bound.min(CLOCKED_BOUND);
        let deadline = Instant::from_std(started + bound);
        let committing = AtomicBool::new(false);
        let work = self.clocked_transaction(batch, input, bound, started, deadline, &committing);
        match tokio::time::timeout_at(deadline, work).await {
            Ok(result) => result,
            Err(_) if committing.load(Ordering::Acquire) => Err(CommitFailure::Uncertain(
                "clocked journal COMMIT unanswered at its deadline".to_owned(),
            )),
            Err(_) => {
                Err(CommitFailure::Known("clocked journal deadline before COMMIT".to_owned()))
            },
        }
    }

    async fn clocked_transaction(
        &self,
        batch: &JournalBatch,
        input: &ClockedInput,
        bound: Duration,
        started: std::time::Instant,
        deadline: Instant,
        committing: &AtomicBool,
    ) -> Result<Vec<ClockReceipt>, CommitFailure> {
        let known = |error: sqlx::Error| CommitFailure::Known(error.to_string());
        let prepared = self.prepare_clocked(batch, input)?;
        if Instant::now() >= deadline {
            return Err(refused("deadline before connection acquisition"));
        }
        let connection = self.pool.acquire().await.map_err(known)?;
        let mut write = LedgerConnection::new(connection);
        if Instant::now() >= deadline {
            return Err(refused("deadline before BEGIN"));
        }
        let mut tx = write
            .connection()
            .begin_with("BEGIN ISOLATION LEVEL READ COMMITTED")
            .instrument(
                tracing::info_span!(target: "robson_operation", "operation_io", io_kind = 2_u64),
            )
            .await
            .map_err(known)?;
        Box::pin(self.prepare_accounting_write(&batch.intent.venue_id, bound, &mut tx, started))
            .await?;
        let receipts = Box::pin(self.write_routed(batch, &mut tx, Some(&prepared))).await?;
        // Same-poll expiry: a completed receipt past the deadline never commits.
        if Instant::now() >= deadline {
            return Err(CommitFailure::Known("clocked journal deadline before COMMIT".to_owned()));
        }
        committing.store(true, Ordering::Release);
        tx.commit()
            .instrument(
                tracing::info_span!(target: "robson_operation", "operation_io", io_kind = 3_u64),
            )
            .await
            .map_err(|e| CommitFailure::Uncertain(e.to_string()))?;
        if Instant::now() >= deadline {
            return Err(CommitFailure::Uncertain(
                "clocked journal COMMIT answered late".to_owned(),
            ));
        }
        write.finished = true;
        Ok(receipts)
    }
}

/// One bound array, in the prepared order.
fn column<T>(rows: &[Expected], f: impl Fn(&Expected) -> T) -> Vec<T> {
    rows.iter().map(f).collect()
}

fn get<'r, T>(row: &'r PgRow, name: &str) -> Result<T, CommitFailure>
where
    T: sqlx::Decode<'r, Postgres> + sqlx::Type<Postgres>,
{
    row.try_get(name).map_err(|_| mismatch(name))
}

/// A whole BOOTTIME domain from its four returned fields.
fn domain(row: &PgRow, kind: &str, boot: &str, offset: &str) -> Result<ClockDomain, CommitFailure> {
    let fields: (Option<String>, Option<Uuid>, Option<i64>, Option<i32>) = (
        get(row, kind)?,
        get(row, boot)?,
        get(row, &format!("{offset}_seconds"))?,
        get(row, &format!("{offset}_nanoseconds"))?,
    );
    match fields {
        (Some(kind), Some(boot), Some(seconds), Some(nanos)) if kind == "boottime" => {
            ClockDomain::new(boot, seconds, u32::try_from(nanos).map_err(|_| mismatch("domain"))?)
                .map_err(|_| mismatch("domain"))
        },
        _ => Err(mismatch("domain")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stored_time_is_the_sqlx_microsecond_projection() {
        let wall = DateTime::from_timestamp(1_700_000_000, 123_456_789).unwrap();
        let stored = postgres_microseconds(wall).unwrap();
        assert_eq!(stored, DateTime::from_timestamp(1_700_000_000, 123_456_000).unwrap());
        assert_ne!(stored, wall, "the capture keeps its full nanoseconds");
    }

    #[test]
    fn a_cancel_is_charged_zero_units_and_every_other_kind_one() {
        assert_eq!(units(IntentKind::Cancel), 0);
        for kind in [
            IntentKind::Entry,
            IntentKind::Exit,
            IntentKind::Protection,
            IntentKind::SweepClose,
        ] {
            assert_eq!(units(kind), 1);
        }
    }
}
