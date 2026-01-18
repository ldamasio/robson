//! Order projection handlers
//!
//! INVARIANT: All handlers are idempotent - safe for replay.
//! UPSERT is used and sequence numbers are checked.

use crate::error::{ProjectionError, Result};
use crate::types::{FillReceived, OrderAcked, OrderCanceled, OrderRejected, OrderSubmitted};
use chrono::Utc;
use robson_eventlog::EventEnvelope;
use sqlx::PgPool;
use uuid::Uuid;

pub(crate) async fn handle_order_submitted(pool: &PgPool, envelope: &EventEnvelope) -> Result<()> {
    let payload: OrderSubmitted =
        serde_json::from_value(envelope.payload.clone()).map_err(|e| {
            ProjectionError::InvalidPayload {
                event_type: envelope.event_type.clone(),
                reason: e.to_string(),
            }
        })?;

    let mut tx = pool.begin().await?;

    // Check if already applied (idempotency via seq check)
    let existing =
        sqlx::query_scalar::<_, i64>("SELECT last_seq FROM orders_current WHERE order_id = $1")
            .bind(payload.order_id)
            .fetch_optional(&mut *tx)
            .await?;

    if let Some(seq) = existing {
        if seq >= envelope.seq {
            tracing::debug!("OrderSubmitted already applied: seq={}", seq);
            return Ok(());
        }
    }

    sqlx::query(
        r#"
        INSERT INTO orders_current (
            order_id, tenant_id, account_id, position_id,
            client_order_id, symbol, side, order_type,
            quantity, price, stop_price,
            status, filled_quantity, total_fee,
            last_event_id, last_seq,
            created_at, updated_at
        ) VALUES (
            $1, $2, $3, $4,
            $5, $6, $7, $8,
            $9, $10, $11,
            'pending', 0, 0,
            $12, $13,
            $14, $14
        )
        ON CONFLICT (order_id) DO UPDATE SET
            status = EXCLUDED.status,
            updated_at = EXCLUDED.updated_at
        "#,
    )
    .bind(payload.order_id)
    .bind(payload.tenant_id)
    .bind(payload.account_id)
    .bind(payload.position_id)
    .bind(&payload.client_order_id)
    .bind(&payload.symbol)
    .bind(&payload.side)
    .bind(&payload.order_type)
    .bind(payload.quantity)
    .bind(payload.price)
    .bind(payload.stop_price)
    .bind(envelope.event_id)
    .bind(envelope.seq)
    .bind(envelope.occurred_at)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(())
}

pub(crate) async fn handle_order_acked(pool: &PgPool, envelope: &EventEnvelope) -> Result<()> {
    let payload: OrderAcked = serde_json::from_value(envelope.payload.clone()).map_err(|e| {
        ProjectionError::InvalidPayload {
            event_type: envelope.event_type.clone(),
            reason: e.to_string(),
        }
    })?;

    sqlx::query(
        r#"
        UPDATE orders_current
        SET
            exchange_order_id = $2,
            status = 'acknowledged',
            last_event_id = $3,
            last_seq = $4,
            updated_at = $5
        WHERE order_id = $1 AND last_seq < $4
        "#,
    )
    .bind(payload.order_id)
    .bind(&payload.exchange_order_id)
    .bind(envelope.event_id)
    .bind(envelope.seq)
    .bind(envelope.occurred_at)
    .execute(pool)
    .await?;

    Ok(())
}

pub(crate) async fn handle_order_rejected(pool: &PgPool, envelope: &EventEnvelope) -> Result<()> {
    let payload: OrderRejected = serde_json::from_value(envelope.payload.clone()).map_err(|e| {
        ProjectionError::InvalidPayload {
            event_type: envelope.event_type.clone(),
            reason: e.to_string(),
        }
    })?;

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
    .execute(pool)
    .await?;

    Ok(())
}

pub(crate) async fn handle_order_canceled(pool: &PgPool, envelope: &EventEnvelope) -> Result<()> {
    let payload: OrderCanceled = serde_json::from_value(envelope.payload.clone()).map_err(|e| {
        ProjectionError::InvalidPayload {
            event_type: envelope.event_type.clone(),
            reason: e.to_string(),
        }
    })?;

    sqlx::query(
        r#"
        UPDATE orders_current
        SET
            status = 'canceled',
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
    .execute(pool)
    .await?;

    Ok(())
}

pub(crate) async fn handle_fill_received(pool: &PgPool, envelope: &EventEnvelope) -> Result<()> {
    let payload: FillReceived = serde_json::from_value(envelope.payload.clone()).map_err(|e| {
        ProjectionError::InvalidPayload {
            event_type: envelope.event_type.clone(),
            reason: e.to_string(),
        }
    })?;

    let mut tx = pool.begin().await?;

    // Idempotency check for fill
    let fill_exists = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM fills WHERE exchange_trade_id = $1 AND tenant_id = $2)",
    )
    .bind(&payload.exchange_trade_id)
    .bind(payload.tenant_id)
    .fetch_one(&mut *tx)
    .await?;

    if fill_exists {
        tracing::debug!("Fill already processed: {}", payload.exchange_trade_id);
        tx.commit().await?;
        return Ok(());
    }

    // Record the fill
    sqlx::query(
        r#"
        INSERT INTO fills (
            fill_id, tenant_id, account_id, order_id,
            exchange_order_id, exchange_trade_id,
            symbol, side, fill_price, fill_quantity,
            fee, fee_asset, is_maker,
            filled_at, ingested_at
        ) VALUES (
            $1, $2, $3, $4,
            $5, $6,
            $7, $8, $9, $10,
            $11, $12, $13,
            $14, $15
        )
        "#,
    )
    .bind(Uuid::new_v4()) // fill_id
    .bind(payload.tenant_id)
    .bind(payload.account_id)
    .bind(payload.order_id)
    .bind(&payload.exchange_order_id)
    .bind(&payload.exchange_trade_id)
    .bind(&payload.symbol)
    .bind(&payload.side)
    .bind(payload.fill_price)
    .bind(payload.fill_quantity)
    .bind(payload.fee)
    .bind(&payload.fee_asset)
    .bind(payload.is_maker)
    .bind(payload.filled_at)
    .bind(Utc::now())
    .execute(&mut *tx)
    .await?;

    // Update order aggregate (safe on replay due to idempotency check above)
    sqlx::query(
        r#"
        UPDATE orders_current
        SET
            filled_quantity = filled_quantity + $2,
            total_fee = total_fee + $3,
            fee_asset = $4,
            last_event_id = $5,
            last_seq = $6,
            updated_at = $7
        WHERE order_id = $1 AND last_seq < $6
        "#,
    )
    .bind(payload.order_id)
    .bind(payload.fill_quantity)
    .bind(payload.fee)
    .bind(&payload.fee_asset)
    .bind(envelope.event_id)
    .bind(envelope.seq)
    .bind(envelope.occurred_at)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(())
}
