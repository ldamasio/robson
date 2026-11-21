//! ADR-0056 step 8p-2: a balance-account daemon owns its month boundaries
//! (8p PLAN-r3 §6.2; the zero-basis PLAN's R1). Its startup, before anything
//! else, and each boundary poll write a `pending` snapshot row for every
//! boundary B after its certified activation basis's `as_of`, up to now.
//! Before any certificate, a month with neither a landed reset nor an
//! activation row gets 8p's own reset at basis 0, under R2's guard and the
//! month's command id. The futures-wallet reset never runs on such a daemon;
//! completion and publication are 8p-5's and 8p-6's.
//!
//! Each transaction here, the zero reset's apply included, runs within
//! [`DEADLINE`] on a connection it owns; a late or failed one is dropped and
//! its connection retired raw. The daemon's main loop is the only caller
//! besides the startup, so the tick is single flight.
use std::{future::Future, time::Duration};

use chrono::{DateTime, Datelike as _, Utc};
use robson_eventlog::{append::append_event_tx, ActorType, EventEnvelope, EventLogError};
use robson_exec::order_journal::OrderJournal;
use robson_projector::apply::apply_month_boundary_reset_on;
use rust_decimal::Decimal;
use sha2::{Digest as _, Sha256};
use sqlx::{Connection as _, PgExecutor, PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::{
    balance_accounting::{BalanceAccounting, BOUNDS},
    event_streams::{EventStreams, MONTH_STREAM_PREFIX},
    ledger_connection::LedgerConnection,
};

/// Why a boundary step wrote nothing (but after an unanswered `COMMIT`,
/// which may have landed, PLAN-r3 §7.1); the next poll retries.
pub(crate) type Failure = Box<dyn std::error::Error + Send + Sync>;

/// The client deadline of each transaction (PLAN-r3 §7.2).
const DEADLINE: Duration = Duration::from_secs(2);

/// A pending row for every boundary in (`as_of`, $3] of the venue's certified
/// activation basis: the first is the month after the basis's own.
const PENDING: &str = "INSERT INTO public.boundary_snapshots (tenant_id, venue_id, month, as_of, \
    state, revision, reopen_count, pending_since) SELECT a.tenant_id, a.venue_id, b::date, \
    b AT TIME ZONE 'UTC', 'pending', 0, 0, $3 FROM public.activation_bases a, generate_series(\
    a.month + INTERVAL '1 month', $3 AT TIME ZONE 'UTC', INTERVAL '1 month') b \
    WHERE (a.tenant_id, a.venue_id, a.state) = ($1, $2, 'certified') \
    ON CONFLICT DO NOTHING RETURNING month";

/// Month $3 (year $4, month $5): 8p's zero reset is due, since no reset
/// landed in it (R2) and the venue has neither an activation row of it nor a
/// certified one (§6.2); its reset landed; its snapshot is owed (a certified
/// basis of an earlier month) and not complete.
const VIEW: &str = "WITH m AS (SELECT EXISTS (SELECT 1 FROM public.monthly_state \
    WHERE (year, month) = ($4, $5) AND boundary_reset_at IS NOT NULL) AS landed) \
    SELECT NOT m.landed AND NOT EXISTS (SELECT 1 FROM public.activation_bases \
    WHERE (tenant_id, venue_id) = ($1, $2) AND (month = $3 OR state = 'certified')), m.landed, \
    EXISTS (SELECT 1 FROM public.activation_bases WHERE (tenant_id, venue_id, state) = \
    ($1, $2, 'certified') AND month < $3) AND NOT EXISTS (SELECT 1 FROM public.boundary_snapshots \
    WHERE (tenant_id, venue_id, month, state) = ($1, $2, $3, 'complete')) FROM m";

/// The stored event, as the append path fetches it before its apply.
const ENVELOPE: &str = "SELECT event_id, tenant_id, stream_key, seq, event_type, payload, \
    payload_schema_version, occurred_at, ingested_at, idempotency_key, trace_id, causation_id, \
    command_id, workflow_id, actor_type, actor_id, prev_hash, hash FROM event_log WHERE event_id = $1";

/// v16's command id `name` as a UUID: the first 16 bytes of its SHA-256, with
/// the RFC 4122 variant bits set (PLAN-r3 §6.6). 8p-6's recalibrations and
/// 8m's cutover map theirs the same way.
pub(crate) fn command_id(name: &str) -> Uuid {
    let mut bytes = [0; 16];
    bytes.copy_from_slice(&Sha256::digest(name.as_bytes())[..16]);
    uuid::Builder::from_bytes(bytes)
        .with_variant(uuid::Variant::RFC4122)
        .into_uuid()
}

/// The command id of `venue`'s reset of a month, v16's
/// `month_reset:binance_spot:YYYY-MM` on Binance Spot.
pub(crate) fn reset_command_id(venue: &str, year: i32, month: u32) -> Uuid {
    command_id(&format!("month_reset:{}:{year:04}-{month:02}", venue.replace(':', "_")))
}

/// `BEGIN` and the ledger's bounds, the head of every transaction that
/// writes here. 8m-2a's shared certificate key goes right after the bounds
/// (8m PLAN-r4 §6.7), where `advance` then finds it taken.
async fn begin(held: &mut LedgerConnection) -> Result<Transaction<'_, Postgres>, sqlx::Error> {
    let mut tx = held.connection().begin().await?;
    sqlx::query(BOUNDS).execute(&mut *tx).await?;
    Ok(tx)
}

async fn bounded<T, E: Into<Failure>>(
    work: impl Future<Output = Result<T, E>>,
) -> Result<T, Failure> {
    tokio::time::timeout(DEADLINE, work).await?.map_err(Into::into)
}

/// The current month, one read-only statement on `executor` (the pool for the
/// poll, the append's transaction for the zero reset's guard): whether 8p's
/// zero reset is due, whether its reset landed, and whether its snapshot is
/// owed and not complete, in that order.
pub(crate) async fn read_month(
    executor: impl PgExecutor<'_>,
    accounting: &BalanceAccounting,
    now: DateTime<Utc>,
) -> Result<(bool, bool, bool), Failure> {
    let read = sqlx::query_as::<_, (bool, bool, bool)>(VIEW)
        .bind(accounting.tenant_id)
        .bind(accounting.venue.as_str())
        .bind(now.date_naive().with_day(1).expect("every month has a first day"))
        .bind(now.year() as i16)
        .bind(now.month() as i16)
        .fetch_one(executor);
    bounded(read).await
}

/// The pending rows of every boundary after the certified activation basis's
/// `as_of`, up to `now`, in one transaction: `BEGIN`, the bounds, `advance`
/// (the bump, and the CAS when a generation is installed), the insert, then
/// `COMMIT`, or `ROLLBACK` when nothing is new, so no version moves without a
/// row (§6.2, §7.2). How many it wrote.
pub(crate) async fn write_pending(
    pool: &PgPool,
    accounting: &BalanceAccounting,
    now: DateTime<Utc>,
) -> Result<usize, Failure> {
    bounded(async {
        let mut held = LedgerConnection::new(pool.acquire().await?);
        let mut tx = begin(&mut held).await?;
        accounting.advance(&mut tx).await?;
        let written = sqlx::query(PENDING)
            .bind(accounting.tenant_id)
            .bind(accounting.venue.as_str())
            .bind(now)
            .fetch_all(&mut *tx)
            .await?
            .len();
        if written == 0 {
            tx.rollback().await?;
        } else {
            tx.commit().await?;
            robson_exec::crash_point!(BOUNDARY_AFTER_PENDING);
        }
        held.finished = true;
        Ok::<_, Failure>(written)
    })
    .await
}

/// 8p's reset of `now`'s month at basis 0 and carried risk 0, if the month's
/// view still finds it due in its append's own transaction (§6.2): under the
/// month's stream lock, never on a suspended stream; `BEGIN`, the bounds, the
/// guard, the append with the month's command id, `COMMIT`. An event already
/// stored under that id, whose apply was lost, is not appended again but
/// applied. The apply fetches the event and runs 8l-4b's accounting step and
/// write (the projector's owned-connection entry, not PLAN-r3 §6.6's pooled
/// apply) on one connection within the client deadline, so a late one, its
/// `COMMIT` unanswered included, retires that connection raw; any failure
/// suspends the stream and records it. Whether it applied one.
pub(crate) async fn zero_reset(
    pool: &PgPool,
    accounting: &BalanceAccounting,
    streams: &EventStreams,
    journal: &OrderJournal,
    now: DateTime<Utc>,
) -> Result<bool, Failure> {
    let (year, month) = (now.year(), now.month());
    let key = format!("{MONTH_STREAM_PREFIX}{year:04}-{month:02}");
    let _stream = streams.lock(&key).await;
    if streams.is_suspended(&key) {
        return Err(format!("{key} is suspended").into());
    }
    let reset = robson_domain::Event::MonthBoundaryReset {
        capital_base: Decimal::ZERO,
        carried_positions_risk: Decimal::ZERO,
        month,
        year,
        timestamp: now,
    };
    let payload = serde_json::to_value(&reset)?;
    let stored =
        robson_eventlog::Event::new(accounting.tenant_id, &key, reset.event_type(), payload)
            .with_actor(ActorType::Daemon, Some("robsond".to_string()))
            .with_command_id(reset_command_id(accounting.venue.as_str(), year, month));
    let appended = bounded(async {
        let mut held = LedgerConnection::new(pool.acquire().await?);
        let mut tx = begin(&mut held).await?;
        let appended = match read_month(&mut *tx, accounting, now).await?.0 {
            false => None,
            true => match append_event_tx(&mut tx, &key, None, stored).await {
                Ok(event_id) => Some((event_id, true)),
                Err(EventLogError::IdempotentDuplicate(event_id)) => Some((event_id, false)),
                Err(error) => return Err(error.into()),
            },
        };
        match appended {
            Some((_, true)) => tx.commit().await?,
            _ => tx.rollback().await?,
        }
        held.finished = true;
        Ok::<_, Failure>(appended.map(|(event_id, _)| event_id))
    })
    .await?;
    let Some(event_id) = appended else {
        return Ok(false);
    };
    let mut failed_seq = None;
    let apply = async {
        let mut held = LedgerConnection::new(pool.acquire().await?);
        let fetch = sqlx::query_as::<_, EventEnvelope>(ENVELOPE).bind(event_id);
        let envelope = fetch.fetch_one(held.connection()).await?;
        failed_seq = Some(envelope.seq);
        apply_month_boundary_reset_on(held.connection(), &envelope, accounting).await?;
        held.finished = true;
        Ok::<_, Failure>(())
    };
    let applied = bounded(apply).await;
    if let Err(error) = applied {
        let message = format!("applying the month's reset {event_id}: {error}");
        if let Some(suspension) = streams.suspend(&key, failed_seq, &message) {
            journal.record_suspension(suspension);
        }
        return Err(message.into());
    }
    Ok(true)
}
