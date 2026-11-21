//! The separately counted existing-pause restoration operation; never an
//! extra hidden bootstrap query or a new observed-pause durability writer.
use chrono::{DateTime, Utc};
use robson_exec::{clock_records::TICK_MAX, venue_clock::NativeClock};
use sqlx::{ConnectOptions, PgPool, Row};
use tokio::time::{timeout_at, Instant};
use uuid::Uuid;

use super::{
    bootstrap::issue,
    completion,
    snapshot::{get, integer},
    BootstrapRows, BudgetError, Trace,
};

pub(crate) async fn existing_pause(
    pool: &PgPool,
    tenant: Uuid,
    rows: &mut BootstrapRows,
    clock: &NativeClock,
    deadline: Instant,
    trace: &Trace,
) -> Result<(), BudgetError> {
    let Some(pause) = rows.pause.as_mut() else {
        return Ok(());
    };
    let now = clock.capture().map_err(|_| BudgetError::Invalid)?;
    if !pause.bounded {
        return Err(BudgetError::Deferred);
    }
    if pause.restored.is_some_and(|(domain, _, _)| domain == now.domain()) {
        return Ok(());
    }
    let anchor = i128::try_from(now.nanoseconds()).map_err(|_| BudgetError::Invalid)?;
    let duration = pause.duration.ok_or(BudgetError::Invalid)?;
    let end = anchor
        .checked_add(duration)
        .filter(|n| *n <= TICK_MAX)
        .ok_or(BudgetError::Invalid)?;
    let at = DateTime::from_timestamp_micros(Utc::now().timestamp_micros())
        .ok_or(BudgetError::Invalid)?;
    let new_domain = now.domain();
    let (seconds, nanos) = new_domain.offset();
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
        issue(trace, deadline)?;
        let row = completion::one(
            &mut connection,
            sqlx::query(include_str!("sql/restore.sql"))
                .bind(tenant)
                .bind("binance:spot")
                .bind(pause.observation)
                .bind(new_domain.boot())
                .bind(seconds)
                .bind(i32::try_from(nanos).map_err(|_| BudgetError::Invalid)?)
                .bind(anchor.to_string())
                .bind(at),
        )
        .await?;
        let (original_domain, original_tick) = pause.original.ok_or(BudgetError::Invalid)?;
        let (original_seconds, original_nanos) = original_domain.offset();
        if row.len() != 20
            || get::<Uuid>(&row, "tenant_id")? != tenant
            || get::<String>(&row, "venue_id")? != "binance:spot"
            || get::<i16>(&row, "status")? != pause._observed.0
            || get::<Option<i32>>(&row, "code")? != pause._observed.1
            || get::<String>(&row, "pause_state")? != "bounded"
            || get::<Uuid>(&row, "observation_id")? != pause.observation
            || get::<String>(&row, "original_kind")? != "boottime"
            || get::<Uuid>(&row, "original_boot")? != original_domain.boot()
            || get::<i64>(&row, "original_offset_seconds")? != original_seconds
            || get::<i32>(&row, "original_offset_ns")?
                != i32::try_from(original_nanos).map_err(|_| BudgetError::Invalid)?
            || integer(&get::<String>(&row, "observed_tick_text")?, true)? != original_tick
            || integer(&get::<String>(&row, "duration_text")?, true)? != duration
            || get::<String>(&row, "restoration_kind")? != "boottime"
            || get::<Uuid>(&row, "restoration_boot")? != new_domain.boot()
            || get::<i64>(&row, "restoration_offset_seconds")? != seconds
            || get::<i32>(&row, "restoration_offset_ns")?
                != i32::try_from(nanos).map_err(|_| BudgetError::Invalid)?
            || integer(&get::<String>(&row, "restoration_anchor_text")?, true)? != anchor
            || integer(&get::<String>(&row, "deadline_tick_text")?, true)? != end
            || get::<Option<String>>(&row, "until_audit")? != pause._observed.2
            || get::<i64>(&row, "recorded_at_us")? != at.timestamp_micros()
        {
            return Err(BudgetError::Invalid);
        }
        if Instant::now() >= deadline {
            return Err(BudgetError::Expired);
        }
        Ok(())
    };
    timeout_at(deadline, work).await.map_err(|_| BudgetError::Expired)??;
    pause.restored = Some((new_domain, anchor, end));
    Ok(())
}
