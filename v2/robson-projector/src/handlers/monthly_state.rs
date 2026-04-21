//! Monthly state projection handlers.
//!
//! Maintains the `monthly_state` materialized view used by runtime risk logic
//! to recover the current month's capital base.

use std::convert::TryFrom;

use robson_eventlog::EventEnvelope;
use sqlx::PgPool;

use crate::{
    error::{ProjectionError, Result},
    types::MonthBoundaryReset,
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
        INSERT INTO monthly_state (year, month, capital_base, carried_risk, created_at)
        VALUES ($1, $2, $3, $4, $5)
        ON CONFLICT (year, month) DO UPDATE SET
            capital_base = EXCLUDED.capital_base,
            carried_risk = EXCLUDED.carried_risk
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
