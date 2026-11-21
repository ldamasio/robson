//! Monthly state projection handlers.
//!
//! Maintains the `monthly_state` materialized view used by runtime risk logic.
//!
//! INVARIANT: `monthly_state` is a ledger (historical accounting only).
//! It does NOT store live execution state — that lives in `positions_current`.
//!
//! INVARIANT: only an event that carries a `capital_base` creates a month row
//! here. The fill and close counters update an existing row and never create
//! one, because a missing row means the base is still unknown:
//! `load_monthly_state` falls back to configured capital until the reset writes
//! the month's base, and a zero would read as an invalid budget basis.

use std::convert::TryFrom;

use chrono::{DateTime, Datelike, Utc};
use robson_eventlog::EventEnvelope;
use rust_decimal::Decimal;
use sqlx::{
    postgres::{PgArguments, PgQueryResult},
    query::Query,
    Acquire, PgExecutor, PgPool, Postgres,
};
use tracing::Instrument as _;

use crate::{
    accounting::AccountingStep,
    error::{ProjectionError, Result},
    types::{CapitalBaseRecalibrated, EntryFilled, MonthBoundaryReset, PositionClosedDomain},
};

const INHERIT_BUDGET_MODEL: &str = r#"COALESCE(
    (SELECT monthly_budget_model
     FROM monthly_state
     WHERE (year, month) < ($1::SMALLINT, $2::SMALLINT)
     ORDER BY year DESC, month DESC
     LIMIT 1),
    'hwm_v1'
)"#;

/// One `monthly_state` statement: autocommit, as it always was, or on a
/// balance-account daemon (ADR-0056 step 8l-4) `BEGIN`, the daemon's
/// accounting step, the statement and `COMMIT`, so the accounting rows are
/// taken before `monthly_state` and a refused step writes nothing. `on` is
/// the pool, or (8p-2) a connection its caller owns and retires.
async fn write<'c>(
    on: impl Acquire<'c, Database = Postgres> + PgExecutor<'c>,
    accounting: Option<&dyn AccountingStep>,
    statement: Query<'_, Postgres, PgArguments>,
) -> Result<PgQueryResult> {
    let Some(step) = accounting else {
        return Ok(statement
            .execute(on)
            .instrument(
                tracing::info_span!(target: "robson_operation", "operation_io", io_kind = 1_u64),
            )
            .await?);
    };
    let mut tx = on
        .begin()
        .instrument(
            tracing::info_span!(target: "robson_operation", "operation_io", io_kind = 2_u64),
        )
        .await?;
    let mut rollback_observation = RollbackObservation::new();
    step.run(&mut tx).await.map_err(ProjectionError::Accounting)?;
    let written = statement
        .execute(&mut *tx)
        .instrument(
            tracing::info_span!(target: "robson_operation", "operation_io", io_kind = 1_u64),
        )
        .await?;
    tx.commit()
        .instrument(
            tracing::info_span!(target: "robson_operation", "operation_io", io_kind = 3_u64),
        )
        .await?;
    rollback_observation.disarm();
    Ok(written)
}

pub(crate) async fn handle_month_boundary_reset<'c>(
    on: impl Acquire<'c, Database = Postgres> + PgExecutor<'c>,
    envelope: &EventEnvelope,
    accounting: Option<&dyn AccountingStep>,
) -> Result<()> {
    let payload: MonthBoundaryReset =
        serde_json::from_value(envelope.payload.clone()).map_err(|e| {
            ProjectionError::InvalidPayload {
                event_type: envelope.event_type.clone(),
                reason: e.to_string(),
            }
        })?;

    let year = i16::try_from(payload.year).map_err(|_| ProjectionError::InvalidPayload {
        event_type: envelope.event_type.clone(),
        reason: format!("year out of range for SMALLINT: {}", payload.year),
    })?;
    let month = i16::try_from(payload.month).map_err(|_| ProjectionError::InvalidPayload {
        event_type: envelope.event_type.clone(),
        reason: format!("month out of range for SMALLINT: {}", payload.month),
    })?;

    let statement = format!(
        r#"
        INSERT INTO monthly_state (
            year, month, capital_base, carried_risk, realized_loss, trades_opened,
            month_peak_net, monthly_budget_model, boundary_reset_at, created_at
        )
        VALUES (
            $1, $2, $3, $4, 0, 0, 0,
            {INHERIT_BUDGET_MODEL},
            $5, $5
        )
        ON CONFLICT (year, month) DO UPDATE SET
            capital_base = EXCLUDED.capital_base,
            carried_risk = EXCLUDED.carried_risk,
            realized_loss = 0,
            trades_opened = 0,
            month_peak_net = 0,
            boundary_reset_at = EXCLUDED.boundary_reset_at
        "#,
    );
    write(
        on,
        accounting,
        sqlx::query(&statement)
            .bind(year)
            .bind(month)
            .bind(payload.capital_base)
            .bind(payload.carried_positions_risk)
            .bind(payload.timestamp),
    )
    .await?;

    Ok(())
}

pub(crate) async fn handle_capital_base_recalibrated(
    pool: &PgPool,
    envelope: &EventEnvelope,
    accounting: Option<&dyn AccountingStep>,
) -> Result<()> {
    let payload: CapitalBaseRecalibrated = serde_json::from_value(envelope.payload.clone())
        .map_err(|e| ProjectionError::InvalidPayload {
            event_type: envelope.event_type.clone(),
            reason: e.to_string(),
        })?;

    let year = i16::try_from(payload.year).map_err(|_| ProjectionError::InvalidPayload {
        event_type: envelope.event_type.clone(),
        reason: format!("year out of range for SMALLINT: {}", payload.year),
    })?;
    let month = i16::try_from(payload.month).map_err(|_| ProjectionError::InvalidPayload {
        event_type: envelope.event_type.clone(),
        reason: format!("month out of range for SMALLINT: {}", payload.month),
    })?;

    let statement = format!(
        r#"
        INSERT INTO monthly_state (
            year, month, capital_base, carried_risk, realized_loss, trades_opened,
            month_peak_net, monthly_budget_model, created_at
        )
        VALUES (
            $1, $2, $3, $4, 0, 0, 0,
            {INHERIT_BUDGET_MODEL},
            $5
        )
        ON CONFLICT (year, month) DO UPDATE SET
            capital_base = EXCLUDED.capital_base,
            carried_risk = EXCLUDED.carried_risk
        "#,
    );
    write(
        pool,
        accounting,
        sqlx::query(&statement)
            .bind(year)
            .bind(month)
            .bind(payload.new_capital_base)
            .bind(payload.carried_risk)
            .bind(payload.timestamp),
    )
    .await?;

    Ok(())
}

/// Add to the counters of the month's row, if it has one.
///
/// A fill or close before the row exists is not counted. The reset zeroes these
/// counters anyway, and neither the budget nor the API reads them: both charge
/// closes from `positions_current`. A replay from before the reset converges on
/// the same row, but a duplicate of one event after it counts twice.
///
/// A writer may be inserting the row right now, where the first UPDATE cannot
/// see it. Wait for it and look again, so the counter is not lost to a row that
/// commits a moment later. Only events before the month's first row take this
/// path. On a balance-account daemon each of the two transactions takes the
/// accounting step first (ADR-0056 step 8l-4).
async fn add_to_month_counters(
    pool: &PgPool,
    accounting: Option<&dyn AccountingStep>,
    occurred_at: DateTime<Utc>,
    trades_opened: i32,
    realized_loss: Decimal,
) -> Result<()> {
    const ADD: &str = "UPDATE monthly_state \
         SET trades_opened = trades_opened + $3, realized_loss = realized_loss + $4 \
         WHERE year = $1 AND month = $2";
    let year = occurred_at.year() as i16;
    let month = occurred_at.month() as i16;

    let updated = write(
        pool,
        accounting,
        sqlx::query(ADD).bind(year).bind(month).bind(trades_opened).bind(realized_loss),
    )
    .await?;
    if updated.rows_affected() > 0 {
        return Ok(());
    }

    let mut tx = pool
        .begin()
        .instrument(
            tracing::info_span!(target: "robson_operation", "operation_io", io_kind = 2_u64),
        )
        .await?;
    let mut rollback_observation = RollbackObservation::new();
    if let Some(step) = accounting {
        step.run(&mut tx).await.map_err(ProjectionError::Accounting)?;
    }
    sqlx::query("LOCK TABLE monthly_state IN SHARE ROW EXCLUSIVE MODE")
        .execute(&mut *tx)
        .instrument(
            tracing::info_span!(target: "robson_operation", "operation_io", io_kind = 1_u64),
        )
        .await?;
    sqlx::query(ADD)
        .bind(year)
        .bind(month)
        .bind(trades_opened)
        .bind(realized_loss)
        .execute(&mut *tx)
        .instrument(
            tracing::info_span!(target: "robson_operation", "operation_io", io_kind = 1_u64),
        )
        .await?;
    tx.commit()
        .instrument(
            tracing::info_span!(target: "robson_operation", "operation_io", io_kind = 3_u64),
        )
        .await?;
    rollback_observation.disarm();

    Ok(())
}

/// Increment `trades_opened` for the month when an entry is filled.
///
/// Extracts year/month from the event timestamp.
pub(crate) async fn handle_entry_filled_monthly(
    pool: &PgPool,
    envelope: &EventEnvelope,
    accounting: Option<&dyn AccountingStep>,
) -> Result<()> {
    let _payload: EntryFilled = serde_json::from_value(envelope.payload.clone()).map_err(|e| {
        ProjectionError::InvalidPayload {
            event_type: envelope.event_type.clone(),
            reason: e.to_string(),
        }
    })?;

    add_to_month_counters(pool, accounting, envelope.occurred_at, 1, Decimal::ZERO).await
}

/// Add realized loss from a closed position to `monthly_state`.
///
/// Only net losses (realized_pnl - total_fees < 0) are counted, matching
/// ADR-0024. Wins do NOT offset losses.
pub(crate) async fn handle_position_closed_monthly(
    pool: &PgPool,
    envelope: &EventEnvelope,
    accounting: Option<&dyn AccountingStep>,
) -> Result<()> {
    let payload: PositionClosedDomain =
        serde_json::from_value(envelope.payload.clone()).map_err(|e| {
            ProjectionError::InvalidPayload {
                event_type: envelope.event_type.clone(),
                reason: e.to_string(),
            }
        })?;

    // ADR-0024: only count losses. Wins do NOT offset losses.
    let net = payload.realized_pnl - payload.total_fees;
    if net >= Decimal::ZERO {
        return Ok(());
    }

    add_to_month_counters(pool, accounting, envelope.occurred_at, 0, net.abs()).await
}

// The public SQLx Transaction Drop queues rollback while open. This only counts
// that application cleanup request; it never claims server acknowledgement.
struct RollbackObservation {
    origin: tracing::Span,
    armed: bool,
}
impl RollbackObservation {
    fn new() -> Self {
        Self {
            origin: tracing::Span::current(),
            armed: true,
        }
    }
    fn disarm(&mut self) {
        self.armed = false;
    }
}
impl Drop for RollbackObservation {
    fn drop(&mut self) {
        if self.armed {
            tracing::info_span!(target: "robson_operation", parent: &self.origin, "operation_io", io_kind = 4_u64).in_scope(|| {});
        }
    }
}
