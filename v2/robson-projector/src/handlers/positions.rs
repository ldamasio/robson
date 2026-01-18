//! Position projection handlers
//!
//! INVARIANT: POSITION_OPENED requires technical_stop_price and technical_stop_distance.
//! INVARIANT: trailing_stop_price is initially derived from technical_stop_distance.

use crate::error::{ProjectionError, Result};
use crate::types::{PositionClosed, PositionOpened};
use robson_eventlog::EventEnvelope;
use sqlx::PgPool;

pub(crate) async fn handle_position_opened(
    pool: &PgPool,
    envelope: &EventEnvelope,
) -> Result<()> {
    let payload: PositionOpened = serde_json::from_value(envelope.payload.clone())
        .map_err(|e| ProjectionError::InvalidPayload {
            event_type: envelope.event_type.clone(),
            reason: e.to_string(),
        })?;

    // INVARIANT CHECK: technical_stop_price and distance must be present
    // This is the Golden Rule - stop comes from technical analysis, not arbitrary %.
    if payload.technical_stop_price.is_zero() {
        return Err(ProjectionError::InvariantViolated {
            event_type: "POSITION_OPENED".to_string(),
            reason: "technical_stop_price must be non-zero".to_string(),
        });
    }

    if payload.technical_stop_distance.is_zero() {
        return Err(ProjectionError::InvariantViolated {
            event_type: "POSITION_OPENED".to_string(),
            reason: "technical_stop_distance must be non-zero".to_string(),
        });
    }

    // INVARIANT: trailing_stop_price is initially anchored to technical_stop_price
    let initial_trailing_stop = payload.technical_stop_price;

    // Idempotency check
    let existing = sqlx::query_scalar::<_, i64>(
        "SELECT last_seq FROM positions_current WHERE position_id = $1"
    )
    .bind(payload.position_id)
    .fetch_optional(pool)
    .await?;

    if let Some(seq) = existing {
        if seq >= envelope.seq {
            tracing::debug!("PositionOpened already applied: seq={}", seq);
            return Ok(());
        }
    }

    sqlx::query(
        r#"
        INSERT INTO positions_current (
            position_id, tenant_id, account_id, strategy_id,
            symbol, side,
            entry_price, entry_quantity, entry_filled_at,
            technical_stop_price, technical_stop_distance,
            trailing_stop_price,
            current_quantity,
            state,
            entry_order_id, stop_loss_order_id,
            last_event_id, last_seq,
            created_at, updated_at
        ) VALUES (
            $1, $2, $3, $4,
            $5, $6,
            $7, $8, $9,
            $10, $11,
            $12,
            $13,
            'armed',
            $14, $15,
            $16, $17,
            $18, $18
        )
        ON CONFLICT (position_id) DO NOTHING
        "#,
    )
    .bind(payload.position_id)
    .bind(payload.tenant_id)
    .bind(payload.account_id)
    .bind(payload.strategy_id)
    .bind(&payload.symbol)
    .bind(&payload.side)
    .bind(payload.entry_price)
    .bind(payload.entry_quantity)
    .bind(payload.entry_filled_at)
    .bind(payload.technical_stop_price)
    .bind(payload.technical_stop_distance)
    .bind(initial_trailing_stop)
    .bind(payload.entry_quantity.unwrap_or_else(|| 0.into()))
    .bind(payload.entry_order_id)
    .bind(payload.stop_loss_order_id)
    .bind(envelope.event_id)
    .bind(envelope.seq)
    .bind(envelope.occurred_at)
    .execute(pool)
    .await?;

    Ok(())
}

pub(crate) async fn handle_position_closed(
    pool: &PgPool,
    envelope: &EventEnvelope,
) -> Result<()> {
    let payload: PositionClosed = serde_json::from_value(envelope.payload.clone())
        .map_err(|e| ProjectionError::InvalidPayload {
            event_type: envelope.event_type.clone(),
            reason: e.to_string(),
        })?;

    sqlx::query(
        r#"
        UPDATE positions_current
        SET
            state = 'closed',
            exit_order_id = $2,
            closed_at = $3,
            last_event_id = $4,
            last_seq = $5,
            updated_at = $6
        WHERE position_id = $1 AND last_seq < $5
        "#,
    )
    .bind(payload.position_id)
    .bind(payload.exit_order_id)
    .bind(payload.closed_at)
    .bind(envelope.event_id)
    .bind(envelope.seq)
    .bind(envelope.occurred_at)
    .execute(pool)
    .await?;

    Ok(())
}
