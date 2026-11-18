use chrono::{DateTime, Utc};
use robson_exec::{clock_records::TICK_MAX, venue_clock::TickCapture};
use sqlx::{ConnectOptions, PgPool, Row};
use tokio::time::{timeout_at, Instant};
use uuid::Uuid;

use super::{completion, snapshot, BootstrapRows, BudgetError, Trace};
use crate::process_epoch::EpochStart;

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
        issue(trace, deadline)?;
        completion::run(&mut connection, sqlx::query(include_str!("sql/begin.sql")), false).await?;
        issue(trace, deadline)?;
        let settings =
            completion::one(&mut connection, sqlx::query(include_str!("sql/settings.sql"))).await?;
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
        let prior_clean =
            epoch.predecessor_epoch_id.map(|_| epoch.predecessor.unclean_reason().is_none());
        let inserted = completion::one(
            &mut connection,
            sqlx::query(include_str!("sql/epoch.sql"))
                .bind(epoch.epoch.epoch_id)
                .bind("binance:spot")
                .bind(domain.boot())
                .bind(seconds)
                .bind(i32::try_from(nanos).map_err(|_| BudgetError::Invalid)?)
                .bind(tick.to_string())
                .bind(epoch.predecessor_epoch_id)
                .bind(prior_clean)
                .bind(audit),
        )
        .await?;
        snapshot::check_epoch(&inserted, epoch, input, audit)?;
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
