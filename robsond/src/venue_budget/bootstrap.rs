use std::time::Duration;

use chrono::{DateTime, Utc};
use robson_exec::{
    clock_records::TICK_MAX,
    venue_clock::{NativeClock, TickCapture},
};
use sqlx::{ConnectOptions, PgConnection, PgPool, Row};
use tokio::time::{timeout_at, Instant};
use uuid::Uuid;

use super::{
    completion,
    pause_value::PauseValue,
    restore, snapshot,
    state::{NonTradingBudget, SharedBudget},
    BootstrapRows, BudgetError, Trace,
};
use crate::process_epoch::EpochStart;

/// The non-trading bootstrap's whole bound: its transaction and the pause
/// restoration together (PLAN §6.4, §7.2).
const NON_TRADING_BOUND: Duration = Duration::from_secs(5);

pub(super) fn issue(trace: &Trace, deadline: Instant) -> Result<(), BudgetError> {
    if Instant::now() >= deadline {
        return Err(BudgetError::Expired);
    }
    trace.issue();
    Ok(())
}

pub(crate) struct BootstrapInput {
    pub(super) audit: Uuid,
    pub(super) generation: i64,
    pub(super) start: TickCapture,
}

/// Missing actual C2/2b is never replaced with an initialized marker or bool.
pub(crate) fn production_input() -> Result<BootstrapInput, BudgetError> {
    Err(BudgetError::PrerequisiteRefusal)
}

/// B01 and B02: BEGIN, then the transaction-local bounds, checked as the
/// server returns them.
async fn begin_bounded(
    connection: &mut PgConnection,
    deadline: Instant,
    trace: &Trace,
) -> Result<(), BudgetError> {
    issue(trace, deadline)?;
    completion::run(connection, sqlx::query(include_str!("sql/begin.sql")), false).await?;
    issue(trace, deadline)?;
    let settings =
        completion::one(connection, sqlx::query(include_str!("sql/settings.sql"))).await?;
    if settings.len() != 3 {
        return Err(BudgetError::Invalid);
    }
    for (name, value) in [
        ("lock_timeout", "300ms"),
        ("statement_timeout", "1s"),
        ("idle_timeout", "1s"),
    ] {
        if settings.try_get::<String, _>(name).map_err(|_| BudgetError::Invalid)? != value {
            return Err(BudgetError::Invalid);
        }
    }
    Ok(())
}

pub(crate) async fn observe_bootstrap(
    pool: &PgPool,
    tenant: Uuid,
    epoch: &EpochStart,
    input: &BootstrapInput,
    deadline: Instant,
    trace: &Trace,
) -> Result<BootstrapRows, BudgetError> {
    let tick = i128::try_from(input.start.nanoseconds()).map_err(|_| BudgetError::Invalid)?;
    if !(0..=TICK_MAX).contains(&tick) || input.audit.is_nil() || input.generation < 0 {
        return Err(BudgetError::Invalid);
    }
    let audit = DateTime::from_timestamp_micros(Utc::now().timestamp_micros())
        .ok_or(BudgetError::Invalid)?;
    let domain = input.start.domain();
    let (seconds, nanos) = domain.offset();
    let options = (*pool.connect_options()).clone().options([
        ("statement_timeout", "1s"),
        ("idle_in_transaction_session_timeout", "1s"),
    ]);
    let work = async {
        // Owned raw connection: dropping this future never queues ROLLBACK or
        // returns a borrowing transaction to the shared J pool.
        if Instant::now() >= deadline {
            return Err(BudgetError::Expired);
        }
        let mut connection = options.connect().await.map_err(completion::classify)?;
        begin_bounded(&mut connection, deadline, trace).await?;
        issue(trace, deadline)?;
        let locked = completion::one(
            &mut connection,
            sqlx::query(include_str!("sql/lock.sql")).bind("binance:spot"),
        )
        .await?;
        snapshot::check_state(&locked, input)?;
        issue(trace, deadline)?;
        let row = completion::one(
            &mut connection,
            sqlx::query(include_str!("sql/snapshot.sql"))
                .bind("binance:spot")
                .bind(tenant)
                .bind(input.audit)
                .bind(input.generation)
                .bind(domain.boot())
                .bind(seconds)
                .bind(i32::try_from(nanos).map_err(|_| BudgetError::Invalid)?)
                .bind(tick.to_string()),
        )
        .await?;
        let rows = snapshot::decode(&row, input)?;
        issue(trace, deadline)?;
        insert_epoch(&mut connection, epoch, input.start, audit).await?;
        if Instant::now() >= deadline {
            return Err(BudgetError::Expired);
        }
        issue(trace, deadline)?;
        completion::run(&mut connection, sqlx::query(include_str!("sql/commit.sql")), false)
            .await?;
        if Instant::now() >= deadline {
            return Err(BudgetError::Expired);
        }
        Ok(rows)
    };
    timeout_at(deadline, work).await.map_err(|_| BudgetError::Expired)?
}

/// B05: this epoch's `budget_epochs` row (domain, start tick, predecessor
/// and its cleanliness), checked as returned.
async fn insert_epoch(
    connection: &mut PgConnection,
    epoch: &EpochStart,
    start: TickCapture,
    audit: DateTime<Utc>,
) -> Result<(), BudgetError> {
    let domain = start.domain();
    let (seconds, nanos) = domain.offset();
    let prior_clean =
        epoch.predecessor_epoch_id.map(|_| epoch.predecessor.unclean_reason().is_none());
    let inserted = completion::one(
        connection,
        sqlx::query(include_str!("sql/epoch.sql"))
            .bind(epoch.epoch.epoch_id)
            .bind("binance:spot")
            .bind(domain.boot())
            .bind(seconds)
            .bind(i32::try_from(nanos).map_err(|_| BudgetError::Invalid)?)
            .bind(start.nanoseconds().to_string())
            .bind(epoch.predecessor_epoch_id)
            .bind(prior_clean)
            .bind(audit),
    )
    .await?;
    snapshot::check_epoch(&inserted, epoch, start, audit)
}

/// What the non-trading bootstrap read: its start tick, captured after the
/// epoch's COMMIT, and the tenant's stored pause, bounded or absent.
pub(crate) struct NonTradingRows {
    pub(super) start: TickCapture,
    pub(super) pause: Option<PauseValue>,
}

/// The non-trading bootstrap (PLAN §6.4): one raw connection with startup
/// bounds 300 ms lock / 1 s statement / 1 s idle, one READ COMMITTED
/// transaction of five statements: BEGIN, the checked bounds, the tenant's
/// pause, this epoch's `budget_epochs` row, COMMIT. It never reads or writes
/// `budget_venue_state`, a source audit or a daily charge (C2/2b stays 8o's).
/// A stored `unresolved` pause refuses the start before anything is written.
pub(crate) async fn observe_non_trading(
    pool: &PgPool,
    tenant: Uuid,
    epoch: &EpochStart,
    clock: &NativeClock,
    deadline: Instant,
    trace: &Trace,
) -> Result<NonTradingRows, BudgetError> {
    // The returned row's start tick is checked against this capture.
    let start = clock.capture().map_err(|_| BudgetError::Invalid)?;
    let audit = DateTime::from_timestamp_micros(Utc::now().timestamp_micros())
        .ok_or(BudgetError::Invalid)?;
    let options = (*pool.connect_options()).clone().options([
        ("lock_timeout", "300ms"),
        ("statement_timeout", "1s"),
        ("idle_in_transaction_session_timeout", "1s"),
    ]);
    let work = async {
        if Instant::now() >= deadline {
            return Err(BudgetError::Expired);
        }
        let mut connection = options.connect().await.map_err(completion::classify)?;
        begin_bounded(&mut connection, deadline, trace).await?;
        issue(trace, deadline)?;
        let row = completion::one(
            &mut connection,
            sqlx::query(include_str!("sql/pause.sql")).bind(tenant).bind("binance:spot"),
        )
        .await?;
        let pause = PauseValue::decode(&row)?;
        if pause.as_ref().is_some_and(|pause| !pause.bounded) {
            return Err(BudgetError::Deferred);
        }
        issue(trace, deadline)?;
        insert_epoch(&mut connection, epoch, start, audit).await?;
        issue(trace, deadline)?;
        completion::run(&mut connection, sqlx::query(include_str!("sql/commit.sql")), false)
            .await?;
        if Instant::now() >= deadline {
            return Err(BudgetError::Expired);
        }
        Ok(NonTradingRows { start, pause })
    };
    timeout_at(deadline, work).await.map_err(|_| BudgetError::Expired)?
}

/// The non-trading start's budget, after the epoch's COMMIT and within one
/// 5 s bound: the bootstrap, then a stored pause restored in another clock
/// domain re-anchored to its full duration in this one. An unclean
/// predecessor (or none) holds every request until start + 120 s.
pub(crate) async fn non_trading(
    pool: &PgPool,
    tenant: Uuid,
    epoch: &EpochStart,
    clock: &NativeClock,
    deadline: Instant,
    trace: &Trace,
) -> Result<NonTradingBudget, BudgetError> {
    let deadline = deadline.min(Instant::now() + NON_TRADING_BOUND);
    let mut rows = observe_non_trading(pool, tenant, epoch, clock, deadline, trace).await?;
    restore::existing_pause(pool, tenant, rows.pause.as_mut(), clock, deadline, trace).await?;
    let unclean = epoch.predecessor.unclean_reason().is_some();
    SharedBudget::non_trading(rows.start, rows.pause, unclean)
}
