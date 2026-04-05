//! Position projection handlers
//!
//! INVARIANT: POSITION_OPENED requires technical_stop_price and technical_stop_distance.
//! INVARIANT: trailing_stop_price is initially derived from technical_stop_distance.
//! INVARIANT: position_armed requires tech_stop_distance.initial_stop (same semantic).

use crate::error::{ProjectionError, Result};
use crate::types::{
    EntryOrderPlaced, EntrySignalReceived, ExitFilled, ExitOrderPlaced, PositionArmed,
    PositionClosed, PositionClosedDomain, PositionDisarmed, PositionOpened,
};
use robson_eventlog::EventEnvelope;
use sqlx::PgPool;

pub(crate) async fn handle_position_opened(pool: &PgPool, envelope: &EventEnvelope) -> Result<()> {
    let payload: PositionOpened =
        serde_json::from_value(envelope.payload.clone()).map_err(|e| {
            ProjectionError::InvalidPayload {
                event_type: envelope.event_type.clone(),
                reason: e.to_string(),
            }
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
        "SELECT last_seq FROM positions_current WHERE position_id = $1",
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

pub(crate) async fn handle_position_closed(pool: &PgPool, envelope: &EventEnvelope) -> Result<()> {
    let payload: PositionClosed =
        serde_json::from_value(envelope.payload.clone()).map_err(|e| {
            ProjectionError::InvalidPayload {
                event_type: envelope.event_type.clone(),
                reason: e.to_string(),
            }
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

pub(crate) async fn handle_entry_order_placed(
    pool: &PgPool,
    envelope: &EventEnvelope,
) -> Result<()> {
    let payload: EntryOrderPlaced =
        serde_json::from_value(envelope.payload.clone()).map_err(|e| {
            ProjectionError::InvalidPayload {
                event_type: envelope.event_type.clone(),
                reason: e.to_string(),
            }
        })?;

    sqlx::query(
        r#"
        UPDATE positions_current
        SET
            state = 'entering',
            entry_price = $2,
            entry_quantity = $3,
            current_quantity = $3,
            entry_order_id = $4,
            entry_signal_id = $5,
            last_event_id = $6,
            last_seq = $7,
            updated_at = $8
        WHERE position_id = $1 AND last_seq < $7
        "#,
    )
    .bind(payload.position_id)
    .bind(payload.expected_price)
    .bind(payload.quantity)
    .bind(payload.order_id)
    .bind(payload.signal_id)
    .bind(envelope.event_id)
    .bind(envelope.seq)
    .bind(envelope.occurred_at)
    .execute(pool)
    .await?;

    Ok(())
}

pub(crate) async fn handle_entry_filled(pool: &PgPool, envelope: &EventEnvelope) -> Result<()> {
    use crate::types::EntryFilled;

    let payload: EntryFilled = serde_json::from_value(envelope.payload.clone()).map_err(|e| {
        ProjectionError::InvalidPayload {
            event_type: envelope.event_type.clone(),
            reason: e.to_string(),
        }
    })?;

    // Initialize favorable_extreme = fill_price, trailing_stop_price = initial_stop
    sqlx::query(
        r#"
        UPDATE positions_current
        SET
            state = 'active',
            entry_price = $2,
            entry_filled_at = $3,
            trailing_stop_price = $4,
            favorable_extreme = $2,
            extreme_at = $3,
            last_event_id = $5,
            last_seq = $6,
            updated_at = $7
        WHERE position_id = $1 AND last_seq < $6
        "#,
    )
    .bind(payload.position_id)
    .bind(payload.fill_price)
    .bind(payload.timestamp)
    .bind(payload.initial_stop)
    .bind(envelope.event_id)
    .bind(envelope.seq)
    .bind(envelope.occurred_at)
    .execute(pool)
    .await?;

    Ok(())
}

/// Handle entry_signal_received (robson-domain::Event::EntrySignalReceived)
///
/// This is an audit event emitted when a detector signal is received.
/// It does NOT change position state - the state transition to Entering
/// is done by entry_order_placed. We simply acknowledge the event was
/// processed successfully (it's already persisted in event_log by the
/// write path).
pub(crate) async fn handle_entry_signal_received(
    _pool: &PgPool,
    envelope: &EventEnvelope,
) -> Result<()> {
    let _payload: EntrySignalReceived =
        serde_json::from_value(envelope.payload.clone()).map_err(|e| ProjectionError::InvalidPayload {
            event_type: envelope.event_type.clone(),
            reason: e.to_string(),
        })?;

    // Audit event only - no projection update needed.
    // The event is already in event_log for replay/audit purposes.
    tracing::debug!(
        position_id = %_payload.position_id,
        signal_id = %_payload.signal_id,
        "entry_signal_received processed (audit only)"
    );

    Ok(())
}

pub(crate) async fn handle_trailing_stop_updated(
    pool: &PgPool,
    envelope: &EventEnvelope,
) -> Result<()> {
    use crate::types::TrailingStopUpdated;

    let payload: TrailingStopUpdated =
        serde_json::from_value(envelope.payload.clone()).map_err(|e| {
            ProjectionError::InvalidPayload {
                event_type: envelope.event_type.clone(),
                reason: e.to_string(),
            }
        })?;

    // Update trailing_stop_price + favorable_extreme (trigger_price is the new extreme)
    sqlx::query(
        r#"
        UPDATE positions_current
        SET
            trailing_stop_price = $2,
            favorable_extreme = $3,
            extreme_at = $4,
            last_event_id = $5,
            last_seq = $6,
            updated_at = $7
        WHERE position_id = $1 AND last_seq < $6
        "#,
    )
    .bind(payload.position_id)
    .bind(payload.new_stop)
    .bind(payload.trigger_price)
    .bind(payload.timestamp)
    .bind(envelope.event_id)
    .bind(envelope.seq)
    .bind(envelope.occurred_at)
    .execute(pool)
    .await?;

    Ok(())
}

pub(crate) async fn handle_exit_triggered(pool: &PgPool, envelope: &EventEnvelope) -> Result<()> {
    use crate::types::ExitTriggered;

    let payload: ExitTriggered = serde_json::from_value(envelope.payload.clone()).map_err(|e| {
        ProjectionError::InvalidPayload {
            event_type: envelope.event_type.clone(),
            reason: e.to_string(),
        }
    })?;

    // Mark state as exiting
    sqlx::query(
        r#"
        UPDATE positions_current
        SET
            state = 'exiting',
            exit_reason = $2,
            last_event_id = $3,
            last_seq = $4,
            updated_at = $5
        WHERE position_id = $1 AND last_seq < $4
        "#,
    )
    .bind(payload.position_id)
    .bind(&payload.reason)
    .bind(envelope.event_id)
    .bind(envelope.seq)
    .bind(envelope.occurred_at)
    .execute(pool)
    .await?;

    Ok(())
}

pub(crate) async fn handle_exit_order_placed(
    pool: &PgPool,
    envelope: &EventEnvelope,
) -> Result<()> {
    let payload: ExitOrderPlaced =
        serde_json::from_value(envelope.payload.clone()).map_err(|e| {
            ProjectionError::InvalidPayload {
                event_type: envelope.event_type.clone(),
                reason: e.to_string(),
            }
        })?;

    sqlx::query(
        r#"
        UPDATE positions_current
        SET
            state = 'exiting',
            exit_order_id = $2,
            exit_reason = $3,
            last_event_id = $4,
            last_seq = $5,
            updated_at = $6
        WHERE position_id = $1 AND last_seq < $5
        "#,
    )
    .bind(payload.position_id)
    .bind(payload.order_id)
    .bind(&payload.exit_reason)
    .bind(envelope.event_id)
    .bind(envelope.seq)
    .bind(envelope.occurred_at)
    .execute(pool)
    .await?;

    Ok(())
}

// =============================================================================
// Domain position lifecycle handlers (snake_case event types from executor)
// =============================================================================

/// Handle position_armed (robson-domain::Event::PositionArmed)
///
/// Creates the initial 'armed' row in positions_current.
/// technical_stop_price is derived from tech_stop_distance.initial_stop.
/// Enforces the same technical stop invariants as POSITION_OPENED.
pub(crate) async fn handle_position_armed(
    pool: &PgPool,
    envelope: &EventEnvelope,
) -> Result<()> {
    let payload: PositionArmed =
        serde_json::from_value(envelope.payload.clone()).map_err(|e| {
            ProjectionError::InvalidPayload {
                event_type: envelope.event_type.clone(),
                reason: e.to_string(),
            }
        })?;

    // INVARIANT: tech_stop_distance must be present and non-zero (Golden Rule)
    let tech = payload.tech_stop_distance.ok_or_else(|| ProjectionError::InvariantViolated {
        event_type: "position_armed".to_string(),
        reason: "tech_stop_distance is required for position_armed".to_string(),
    })?;

    if tech.distance.is_zero() {
        return Err(ProjectionError::InvariantViolated {
            event_type: "position_armed".to_string(),
            reason: "tech_stop_distance.distance must be non-zero".to_string(),
        });
    }
    if tech.initial_stop.is_zero() {
        return Err(ProjectionError::InvariantViolated {
            event_type: "position_armed".to_string(),
            reason: "tech_stop_distance.initial_stop must be non-zero".to_string(),
        });
    }

    let symbol = payload.symbol.as_pair();
    let side = payload.side.to_lowercase();

    // Idempotency check
    let existing = sqlx::query_scalar::<_, i64>(
        "SELECT last_seq FROM positions_current WHERE position_id = $1",
    )
    .bind(payload.position_id)
    .fetch_optional(pool)
    .await?;

    if let Some(seq) = existing {
        if seq >= envelope.seq {
            tracing::debug!("position_armed already applied: seq={}", seq);
            return Ok(());
        }
    }

    sqlx::query(
        r#"
        INSERT INTO positions_current (
            position_id, tenant_id, account_id,
            symbol, side,
            technical_stop_price, technical_stop_distance,
            trailing_stop_price,
            current_quantity,
            state,
            last_event_id, last_seq,
            created_at, updated_at
        ) VALUES (
            $1, $2, $3,
            $4, $5,
            $6, $7,
            $6,
            0,
            'armed',
            $8, $9,
            $10, $10
        )
        ON CONFLICT (position_id) DO NOTHING
        "#,
    )
    .bind(payload.position_id)
    .bind(envelope.tenant_id)
    .bind(payload.account_id)
    .bind(&symbol)
    .bind(&side)
    .bind(tech.initial_stop)
    .bind(tech.distance)
    .bind(envelope.event_id)
    .bind(envelope.seq)
    .bind(payload.timestamp)
    .execute(pool)
    .await?;

    Ok(())
}

/// Handle position_disarmed (robson-domain::Event::PositionDisarmed)
///
/// Transitions the position from 'armed' to 'closed'.
/// Only valid from 'armed' state (no order was ever placed).
pub(crate) async fn handle_position_disarmed(
    pool: &PgPool,
    envelope: &EventEnvelope,
) -> Result<()> {
    let payload: PositionDisarmed =
        serde_json::from_value(envelope.payload.clone()).map_err(|e| {
            ProjectionError::InvalidPayload {
                event_type: envelope.event_type.clone(),
                reason: e.to_string(),
            }
        })?;

    sqlx::query(
        r#"
        UPDATE positions_current
        SET
            state = 'closed',
            exit_reason = $2,
            closed_at = $3,
            last_event_id = $4,
            last_seq = $5,
            updated_at = $6
        WHERE position_id = $1 AND last_seq < $5
        "#,
    )
    .bind(payload.position_id)
    .bind("DisarmedByUser")
    .bind(payload.timestamp)
    .bind(envelope.event_id)
    .bind(envelope.seq)
    .bind(envelope.occurred_at)
    .execute(pool)
    .await?;

    Ok(())
}

/// Handle exit_filled (robson-domain::Event::ExitFilled)
///
/// Records the confirmed exit fill. Does not close the position — that is done
/// by the subsequent position_closed event which carries final P&L.
/// Updates current_quantity to 0 and records the fill timestamp.
pub(crate) async fn handle_exit_filled(
    pool: &PgPool,
    envelope: &EventEnvelope,
) -> Result<()> {
    let payload: ExitFilled =
        serde_json::from_value(envelope.payload.clone()).map_err(|e| {
            ProjectionError::InvalidPayload {
                event_type: envelope.event_type.clone(),
                reason: e.to_string(),
            }
        })?;

    sqlx::query(
        r#"
        UPDATE positions_current
        SET
            current_quantity = 0,
            current_price = $2,
            total_fees = COALESCE(total_fees, 0) + $3,
            last_event_id = $4,
            last_seq = $5,
            updated_at = $6
        WHERE position_id = $1 AND last_seq < $5
        "#,
    )
    .bind(payload.position_id)
    .bind(payload.fill_price)
    .bind(payload.fee)
    .bind(envelope.event_id)
    .bind(envelope.seq)
    .bind(envelope.occurred_at)
    .execute(pool)
    .await?;

    Ok(())
}

/// Handle position_closed (robson-domain::Event::PositionClosed, lowercase)
///
/// Final closure event with realized P&L.
/// Transitions the position to 'closed' and records exit_price, realized_pnl, total_fees.
pub(crate) async fn handle_position_closed_domain(
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

    sqlx::query(
        r#"
        UPDATE positions_current
        SET
            state = 'closed',
            current_price = $2,
            realized_pnl = $3,
            total_fees = $4,
            exit_reason = $5,
            closed_at = $6,
            last_event_id = $7,
            last_seq = $8,
            updated_at = $9
        WHERE position_id = $1 AND last_seq < $8
        "#,
    )
    .bind(payload.position_id)
    .bind(payload.exit_price)
    .bind(payload.realized_pnl)
    .bind(payload.total_fees)
    .bind(&payload.exit_reason)
    .bind(payload.timestamp)
    .bind(envelope.event_id)
    .bind(envelope.seq)
    .bind(envelope.occurred_at)
    .execute(pool)
    .await?;

    Ok(())
}
