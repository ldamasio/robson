//! Position projection handlers
//!
//! INVARIANT: POSITION_OPENED requires technical_stop_price and
//! technical_stop_distance. INVARIANT: trailing_stop_price is initially derived
//! from technical_stop_distance. Domain position_armed may not have technical
//! stop data yet; the real detector stop is captured from entry_signal_received
//! and enforced before entry_order_accepted transitions the position.

use robson_eventlog::EventEnvelope;
use rust_decimal::Decimal;
use sqlx::PgPool;

use crate::{
    error::{ProjectionError, Result},
    types::{
        EntryExecutionRejected, EntryOrderAccepted, EntryOrderFailed, EntryOrderPlaced,
        EntryOrderRequested, EntrySignalReceived, ExitFilled, ExitOrderPlaced, PositionArmed,
        PositionClosed, PositionClosedDomain, PositionDisarmed, PositionOpened,
        TechnicalStopDistancePayload,
    },
};

pub(crate) async fn handle_position_opened(pool: &PgPool, envelope: &EventEnvelope) -> Result<()> {
    let payload: PositionOpened =
        serde_json::from_value(envelope.payload.clone()).map_err(|e| {
            ProjectionError::InvalidPayload {
                event_type: envelope.event_type.clone(),
                reason: e.to_string(),
            }
        })?;

    // INVARIANT CHECK: technical_stop_price and distance must be present
    // This is the Golden Rule - stop comes from technical analysis, not arbitrary
    // %.
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

fn technical_stop_fields(
    tech_stop_distance: Option<TechnicalStopDistancePayload>,
) -> (Option<Decimal>, Option<Decimal>) {
    match tech_stop_distance {
        Some(tech) if !tech.initial_stop.is_zero() && !tech.distance.is_zero() => {
            (Some(tech.initial_stop), Some(tech.distance))
        },
        _ => (None, None),
    }
}

fn signal_stop_distance(entry_price: Decimal, stop_loss: Decimal) -> Decimal {
    if entry_price >= stop_loss {
        entry_price - stop_loss
    } else {
        stop_loss - entry_price
    }
}

fn validate_entry_accepted_technical_stop(
    technical_stop_price: Option<Decimal>,
    technical_stop_distance: Option<Decimal>,
) -> Result<()> {
    let stop = technical_stop_price.ok_or_else(|| ProjectionError::InvariantViolated {
        event_type: "entry_order_accepted".to_string(),
        reason: "technical_stop_price is required before entry_order_accepted".to_string(),
    })?;

    if stop.is_zero() {
        return Err(ProjectionError::InvariantViolated {
            event_type: "entry_order_accepted".to_string(),
            reason: "technical_stop_price must be non-zero".to_string(),
        });
    }

    let distance = technical_stop_distance.ok_or_else(|| ProjectionError::InvariantViolated {
        event_type: "entry_order_accepted".to_string(),
        reason: "technical_stop_distance is required before entry_order_accepted".to_string(),
    })?;

    if distance.is_zero() {
        return Err(ProjectionError::InvariantViolated {
            event_type: "entry_order_accepted".to_string(),
            reason: "technical_stop_distance must be non-zero".to_string(),
        });
    }

    Ok(())
}

/// Handle legacy entry_order_placed (audit-only, does NOT transition to entering).
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

    let Some((last_seq,)) = sqlx::query_as::<_, (i64,)>(
        r#"
            SELECT last_seq
            FROM positions_current
            WHERE position_id = $1
            "#,
    )
    .bind(payload.position_id)
    .fetch_optional(pool)
    .await?
    else {
        return Err(ProjectionError::InvariantViolated {
            event_type: "entry_order_placed".to_string(),
            reason: "position must exist before entry_order_placed (legacy)".to_string(),
        });
    };

    if last_seq >= envelope.seq {
        tracing::debug!("entry_order_placed already applied: seq={}", last_seq);
        return Ok(());
    }

    // Legacy: audit-only. Record entry data but do NOT set state = 'entering'.
    // Do not set entry_order_id here: positions_current.entry_order_id has a
    // foreign key to orders_current, and legacy domain events do not create an
    // orders_current row.
    sqlx::query(
        r#"
        UPDATE positions_current
        SET
            entry_price = $2,
            entry_quantity = $3,
            current_quantity = $3,
            entry_signal_id = $4,
            last_event_id = $5,
            last_seq = $6,
            updated_at = $7
        WHERE position_id = $1 AND last_seq < $6
        "#,
    )
    .bind(payload.position_id)
    .bind(payload.expected_price)
    .bind(payload.quantity)
    .bind(payload.signal_id)
    .bind(envelope.event_id)
    .bind(envelope.seq)
    .bind(envelope.occurred_at)
    .execute(pool)
    .await?;

    Ok(())
}

/// Handle entry_order_requested (audit-only, does NOT transition to entering).
pub(crate) async fn handle_entry_order_requested(
    pool: &PgPool,
    envelope: &EventEnvelope,
) -> Result<()> {
    let payload: EntryOrderRequested =
        serde_json::from_value(envelope.payload.clone()).map_err(|e| {
            ProjectionError::InvalidPayload {
                event_type: envelope.event_type.clone(),
                reason: e.to_string(),
            }
        })?;

    let Some((last_seq,)) = sqlx::query_as::<_, (i64,)>(
        r#"
            SELECT last_seq
            FROM positions_current
            WHERE position_id = $1
            "#,
    )
    .bind(payload.position_id)
    .fetch_optional(pool)
    .await?
    else {
        return Err(ProjectionError::InvariantViolated {
            event_type: "entry_order_requested".to_string(),
            reason: "position must exist before entry_order_requested".to_string(),
        });
    };

    if last_seq >= envelope.seq {
        tracing::debug!("entry_order_requested already applied: seq={}", last_seq);
        return Ok(());
    }

    let mut tx = pool.begin().await?;

    // EntryOrderRequested is the projection home for the local order row.
    // Create it before setting positions_current.entry_order_id so the FK is valid.
    sqlx::query(
        r#"
        INSERT INTO orders_current (
            order_id, tenant_id, account_id, position_id,
            client_order_id, symbol, side, order_type,
            quantity, price, stop_price,
            status, filled_quantity, total_fee,
            last_event_id, last_seq,
            created_at, updated_at
        )
        SELECT
            $2, tenant_id, account_id, position_id,
            $3, symbol,
            CASE side WHEN 'long' THEN 'buy' ELSE 'sell' END,
            'market',
            $4, NULL, NULL,
            'pending', 0, 0,
            $5, $6,
            $7, $7
        FROM positions_current
        WHERE position_id = $1
        ON CONFLICT (order_id) DO UPDATE SET
            client_order_id = EXCLUDED.client_order_id,
            quantity = EXCLUDED.quantity,
            last_event_id = EXCLUDED.last_event_id,
            last_seq = EXCLUDED.last_seq,
            updated_at = EXCLUDED.updated_at
        WHERE orders_current.last_seq < EXCLUDED.last_seq
        "#,
    )
    .bind(payload.position_id)
    .bind(payload.order_id)
    .bind(&payload.client_order_id)
    .bind(payload.quantity)
    .bind(envelope.event_id)
    .bind(envelope.seq)
    .bind(envelope.occurred_at)
    .execute(&mut *tx)
    .await?;

    // Audit-only for position state: record entry data, do NOT set state = 'entering'.
    sqlx::query(
        r#"
        UPDATE positions_current
        SET
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
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(())
}

/// Handle entry_order_accepted (transitions to entering).
pub(crate) async fn handle_entry_order_accepted(
    pool: &PgPool,
    envelope: &EventEnvelope,
) -> Result<()> {
    let payload: EntryOrderAccepted =
        serde_json::from_value(envelope.payload.clone()).map_err(|e| {
            ProjectionError::InvalidPayload {
                event_type: envelope.event_type.clone(),
                reason: e.to_string(),
            }
        })?;

    let Some((last_seq, technical_stop_price, technical_stop_distance)) =
        sqlx::query_as::<_, (i64, Option<Decimal>, Option<Decimal>)>(
            r#"
            SELECT last_seq, technical_stop_price, technical_stop_distance
            FROM positions_current
            WHERE position_id = $1
            "#,
        )
        .bind(payload.position_id)
        .fetch_optional(pool)
        .await?
    else {
        return Err(ProjectionError::InvariantViolated {
            event_type: "entry_order_accepted".to_string(),
            reason: "position must exist before entry_order_accepted".to_string(),
        });
    };

    if last_seq >= envelope.seq {
        tracing::debug!("entry_order_accepted already applied: seq={}", last_seq);
        return Ok(());
    }

    validate_entry_accepted_technical_stop(technical_stop_price, technical_stop_distance)?;

    let mut tx = pool.begin().await?;

    // Upsert the order row so positions_current.entry_order_id remains a valid FK
    // and accepted-but-not-filled entries can be reconstructed after restart.
    sqlx::query(
        r#"
        INSERT INTO orders_current (
            order_id, tenant_id, account_id, position_id,
            exchange_order_id, client_order_id, symbol, side, order_type,
            quantity, price, stop_price,
            status, filled_quantity, total_fee,
            last_event_id, last_seq,
            created_at, updated_at
        )
        SELECT
            $2, tenant_id, account_id, position_id,
            $3, $4, symbol,
            CASE side WHEN 'long' THEN 'buy' ELSE 'sell' END,
            'market',
            $5, NULL, NULL,
            'acknowledged', 0, 0,
            $6, $7,
            $8, $8
        FROM positions_current
        WHERE position_id = $1
        ON CONFLICT (order_id) DO UPDATE SET
            exchange_order_id = EXCLUDED.exchange_order_id,
            client_order_id = EXCLUDED.client_order_id,
            quantity = EXCLUDED.quantity,
            status = 'acknowledged',
            last_event_id = EXCLUDED.last_event_id,
            last_seq = EXCLUDED.last_seq,
            updated_at = EXCLUDED.updated_at
        WHERE orders_current.last_seq < EXCLUDED.last_seq
        "#,
    )
    .bind(payload.position_id)
    .bind(payload.order_id)
    .bind(&payload.exchange_order_id)
    .bind(&payload.client_order_id)
    .bind(payload.quantity)
    .bind(envelope.event_id)
    .bind(envelope.seq)
    .bind(envelope.occurred_at)
    .execute(&mut *tx)
    .await?;

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
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(())
}

/// Handle entry_order_failed (audit-only, no state transition).
pub(crate) async fn handle_entry_order_failed(
    pool: &PgPool,
    envelope: &EventEnvelope,
) -> Result<()> {
    let payload: EntryOrderFailed =
        serde_json::from_value(envelope.payload.clone()).map_err(|e| {
            ProjectionError::InvalidPayload {
                event_type: envelope.event_type.clone(),
                reason: e.to_string(),
            }
        })?;

    let Some((last_seq,)) = sqlx::query_as::<_, (i64,)>(
        r#"
            SELECT last_seq
            FROM positions_current
            WHERE position_id = $1
            "#,
    )
    .bind(payload.position_id)
    .fetch_optional(pool)
    .await?
    else {
        return Err(ProjectionError::InvariantViolated {
            event_type: "entry_order_failed".to_string(),
            reason: "position must exist before entry_order_failed".to_string(),
        });
    };

    if last_seq >= envelope.seq {
        tracing::debug!("entry_order_failed already applied: seq={}", last_seq);
        return Ok(());
    }

    let mut tx = pool.begin().await?;

    sqlx::query(
        r#"
        UPDATE orders_current
        SET
            status = 'rejected',
            last_event_id = $2,
            last_seq = $3,
            updated_at = $4
        WHERE order_id = $1 AND last_seq < $3
        "#,
    )
    .bind(payload.order_id)
    .bind(envelope.event_id)
    .bind(envelope.seq)
    .bind(envelope.occurred_at)
    .execute(&mut *tx)
    .await?;

    // Audit-only for position state: update last_event_id/seq, no state change.
    sqlx::query(
        r#"
        UPDATE positions_current
        SET
            last_event_id = $2,
            last_seq = $3,
            updated_at = $4
        WHERE position_id = $1 AND last_seq < $3
        "#,
    )
    .bind(payload.position_id)
    .bind(envelope.event_id)
    .bind(envelope.seq)
    .bind(envelope.occurred_at)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(())
}

/// Handle entry_execution_rejected (recoverable manual-review state).
pub(crate) async fn handle_entry_execution_rejected(
    pool: &PgPool,
    envelope: &EventEnvelope,
) -> Result<()> {
    let payload: EntryExecutionRejected = serde_json::from_value(envelope.payload.clone())
        .map_err(|e| ProjectionError::InvalidPayload {
            event_type: envelope.event_type.clone(),
            reason: e.to_string(),
        })?;

    let Some((last_seq,)) = sqlx::query_as::<_, (i64,)>(
        r#"
            SELECT last_seq
            FROM positions_current
            WHERE position_id = $1
            "#,
    )
    .bind(payload.position_id)
    .fetch_optional(pool)
    .await?
    else {
        return Err(ProjectionError::InvariantViolated {
            event_type: "entry_execution_rejected".to_string(),
            reason: "position must exist before entry_execution_rejected".to_string(),
        });
    };

    if last_seq >= envelope.seq {
        tracing::debug!("entry_execution_rejected already applied: seq={}", last_seq);
        return Ok(());
    }

    let mut tx = pool.begin().await?;

    sqlx::query(
        r#"
        UPDATE orders_current
        SET
            status = 'rejected',
            last_event_id = $2,
            last_seq = $3,
            updated_at = $4
        WHERE order_id = $1 AND last_seq < $3
        "#,
    )
    .bind(payload.order_id)
    .bind(envelope.event_id)
    .bind(envelope.seq)
    .bind(envelope.occurred_at)
    .execute(&mut *tx)
    .await?;

    sqlx::query(
        r#"
        UPDATE positions_current
        SET
            state = 'error',
            last_event_id = $2,
            last_seq = $3,
            updated_at = $4
        WHERE position_id = $1 AND last_seq < $3
        "#,
    )
    .bind(payload.position_id)
    .bind(envelope.event_id)
    .bind(envelope.seq)
    .bind(envelope.occurred_at)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
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
/// This event carries the detector-derived technical stop. It does not change
/// position state; entry_order_accepted performs the Entering transition after
/// verifying these fields are present and non-zero.
pub(crate) async fn handle_entry_signal_received(
    pool: &PgPool,
    envelope: &EventEnvelope,
) -> Result<()> {
    let payload: EntrySignalReceived =
        serde_json::from_value(envelope.payload.clone()).map_err(|e| {
            ProjectionError::InvalidPayload {
                event_type: envelope.event_type.clone(),
                reason: e.to_string(),
            }
        })?;

    let distance = signal_stop_distance(payload.entry_price, payload.stop_loss);

    sqlx::query(
        r#"
        UPDATE positions_current
        SET
            technical_stop_price = $2,
            technical_stop_distance = $3,
            last_event_id = $4,
            last_seq = $5,
            updated_at = $6
        WHERE position_id = $1 AND last_seq < $5
        "#,
    )
    .bind(payload.position_id)
    .bind(payload.stop_loss)
    .bind(distance)
    .bind(envelope.event_id)
    .bind(envelope.seq)
    .bind(envelope.occurred_at)
    .execute(pool)
    .await?;

    tracing::debug!(
        position_id = %payload.position_id,
        signal_id = %payload.signal_id,
        stop_loss = %payload.stop_loss,
        distance = %distance,
        "entry_signal_received stored detector technical stop"
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

    // Update trailing_stop_price + favorable_extreme (trigger_price is the new
    // extreme)
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

    let mut tx = pool.begin().await?;

    // Upsert the exit order into orders_current first so the FK
    // positions_current.exit_order_id → orders_current is satisfied.
    sqlx::query(
        r#"
        INSERT INTO orders_current (
            order_id, tenant_id, account_id, position_id,
            client_order_id, symbol, side, order_type,
            quantity, price, stop_price,
            status, filled_quantity, total_fee,
            last_event_id, last_seq,
            created_at, updated_at
        )
        SELECT
            $2, tenant_id, account_id, position_id,
            $2::text, symbol,
            CASE side WHEN 'long' THEN 'sell' ELSE 'buy' END,
            'market',
            $3, $4, NULL,
            'pending', 0, 0,
            $5, $6,
            $7, $7
        FROM positions_current
        WHERE position_id = $1
        ON CONFLICT (order_id) DO UPDATE SET
            last_event_id = EXCLUDED.last_event_id,
            last_seq = EXCLUDED.last_seq,
            updated_at = EXCLUDED.updated_at
        WHERE orders_current.last_seq < EXCLUDED.last_seq
        "#,
    )
    .bind(payload.position_id)
    .bind(payload.order_id)
    .bind(payload.quantity)
    .bind(payload.expected_price)
    .bind(envelope.event_id)
    .bind(envelope.seq)
    .bind(envelope.occurred_at)
    .execute(&mut *tx)
    .await?;

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
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(())
}

// =============================================================================
// Domain position lifecycle handlers (snake_case event types from executor)
// =============================================================================

/// Handle position_armed (robson-domain::Event::PositionArmed)
///
/// Creates the initial 'armed' row in positions_current. The detector has not
/// produced an entry price or technical stop yet, so technical stop columns may
/// be NULL at this phase.
pub(crate) async fn handle_position_armed(pool: &PgPool, envelope: &EventEnvelope) -> Result<()> {
    let payload: PositionArmed = serde_json::from_value(envelope.payload.clone()).map_err(|e| {
        ProjectionError::InvalidPayload {
            event_type: envelope.event_type.clone(),
            reason: e.to_string(),
        }
    })?;

    let (technical_stop_price, technical_stop_distance) =
        technical_stop_fields(payload.tech_stop_distance);
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
    .bind(technical_stop_price)
    .bind(technical_stop_distance)
    .bind(envelope.event_id)
    .bind(envelope.seq)
    .bind(payload.timestamp)
    .execute(pool)
    .await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use rust_decimal_macros::dec;

    use super::*;

    #[test]
    fn position_armed_treats_missing_or_zero_technical_stop_as_unknown() {
        assert_eq!(technical_stop_fields(None), (None, None));

        let zero_stop = TechnicalStopDistancePayload {
            distance: dec!(1),
            distance_pct: dec!(100),
            initial_stop: Decimal::ZERO,
        };
        assert_eq!(technical_stop_fields(Some(zero_stop)), (None, None));
    }

    #[test]
    fn entry_order_accepted_requires_non_zero_detector_technical_stop() {
        assert!(validate_entry_accepted_technical_stop(None, Some(dec!(1500))).is_err());
        assert!(
            validate_entry_accepted_technical_stop(Some(Decimal::ZERO), Some(dec!(1500))).is_err()
        );
        assert!(validate_entry_accepted_technical_stop(Some(dec!(93500)), None).is_err());
        assert!(
            validate_entry_accepted_technical_stop(Some(dec!(93500)), Some(Decimal::ZERO)).is_err()
        );
        assert!(validate_entry_accepted_technical_stop(Some(dec!(93500)), Some(dec!(1500))).is_ok());
    }
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
pub(crate) async fn handle_exit_filled(pool: &PgPool, envelope: &EventEnvelope) -> Result<()> {
    let payload: ExitFilled = serde_json::from_value(envelope.payload.clone()).map_err(|e| {
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
/// Transitions the position to 'closed' and records exit_price, realized_pnl,
/// total_fees.
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
