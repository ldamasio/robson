//! Monthly state projection handlers.
//!
//! Maintains the `monthly_state` materialized view used by runtime risk logic.
//!
//! INVARIANT: `monthly_state` is a ledger (historical accounting only).
//! It does NOT store live execution state — that lives in `positions_current`.

use std::convert::TryFrom;

use chrono::Datelike;
use robson_eventlog::EventEnvelope;
use sqlx::PgPool;

use crate::{
    error::{ProjectionError, Result},
    types::{EntryFilled, MonthBoundaryReset, PositionClosedDomain},
};

pub(crate) async fn handle_month_boundary_reset(
    pool: &PgPool,
    envelope: &EventEnvelope,
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

    sqlx::query(
        r#"
        INSERT INTO monthly_state (year, month, capital_base, carried_risk, realized_loss, trades_opened, created_at)
        VALUES ($1, $2, $3, $4, 0, 0, $5)
        ON CONFLICT (year, month) DO UPDATE SET
            capital_base = EXCLUDED.capital_base,
            carried_risk = EXCLUDED.carried_risk,
            realized_loss = 0,
            trades_opened = 0
        "#,
    )
    .bind(year)
    .bind(month)
    .bind(payload.capital_base)
    .bind(payload.carried_positions_risk)
    .bind(payload.timestamp)
    .execute(pool)
    .await?;

    Ok(())
}

/// Increment `trades_opened` for the month when an entry is filled.
///
/// Extracts year/month from the event timestamp. UPSERT ensures idempotent replay.
pub(crate) async fn handle_entry_filled_monthly(
    pool: &PgPool,
    envelope: &EventEnvelope,
) -> Result<()> {
    let payload: EntryFilled =
        serde_json::from_value(envelope.payload.clone()).map_err(|e| {
            ProjectionError::InvalidPayload {
                event_type: envelope.event_type.clone(),
                reason: e.to_string(),
            }
        })?;

    let year = envelope.occurred_at.year() as i16;
    let month = envelope.occurred_at.month() as i16;

    sqlx::query(
        r#"
        INSERT INTO monthly_state (year, month, capital_base, realized_loss, trades_opened, created_at)
        VALUES ($1, $2, 0, 0, 1, $3)
        ON CONFLICT (year, month) DO UPDATE SET
            trades_opened = monthly_state.trades_opened + 1
        "#,
    )
    .bind(year)
    .bind(month)
    .bind(envelope.occurred_at)
    .execute(pool)
    .await?;

    Ok(())
}

/// Add realized loss from a closed position to `monthly_state`.
///
/// Only net losses (realized_pnl - total_fees < 0) are counted, matching ADR-0024.
/// Wins do NOT offset losses for slot calculation.
pub(crate) async fn handle_position_closed_monthly(
    pool: &PgPool,
    envelope: &EventEnvelope,
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
    if net >= rust_decimal::Decimal::ZERO {
        return Ok(());
    }
    let loss = net.abs();

    let year = envelope.occurred_at.year() as i16;
    let month = envelope.occurred_at.month() as i16;

    sqlx::query(
        r#"
        INSERT INTO monthly_state (year, month, capital_base, realized_loss, trades_opened, created_at)
        VALUES ($1, $2, 0, $3, 0, $4)
        ON CONFLICT (year, month) DO UPDATE SET
            realized_loss = monthly_state.realized_loss + $3
        "#,
    )
    .bind(year)
    .bind(month)
    .bind(loss)
    .bind(envelope.occurred_at)
    .execute(pool)
    .await?;

    Ok(())
}
