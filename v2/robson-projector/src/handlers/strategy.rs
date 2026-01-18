//! Strategy projection handlers

use crate::error::{ProjectionError, Result};
use crate::types::{StrategyDisabled, StrategyEnabled};
use robson_eventlog::EventEnvelope;
use sqlx::PgPool;

pub(crate) async fn handle_strategy_enabled(pool: &PgPool, envelope: &EventEnvelope) -> Result<()> {
    let payload: StrategyEnabled =
        serde_json::from_value(envelope.payload.clone()).map_err(|e| {
            ProjectionError::InvalidPayload {
                event_type: envelope.event_type.clone(),
                reason: e.to_string(),
            }
        })?;

    // Idempotency check
    let existing = sqlx::query_scalar::<_, i64>(
        "SELECT last_seq FROM strategy_state_current WHERE strategy_id = $1",
    )
    .bind(payload.strategy_id)
    .fetch_optional(pool)
    .await?;

    if let Some(seq) = existing {
        if seq >= envelope.seq {
            tracing::debug!("StrategyEnabled already applied: seq={}", seq);
            return Ok(());
        }
    }

    sqlx::query(
        r#"
        INSERT INTO strategy_state_current (
            strategy_id, tenant_id, account_id,
            strategy_name, strategy_type,
            detector_config, risk_config,
            is_enabled, enabled_at,
            last_event_id, last_seq,
            created_at, updated_at
        ) VALUES (
            $1, $2, $3,
            $4, $5,
            $6, $7,
            true, $8,
            $9, $10,
            $11, $11
        )
        ON CONFLICT (strategy_id) DO UPDATE SET
            is_enabled = true,
            enabled_at = EXCLUDED.enabled_at,
            disabled_at = NULL,
            disabled_reason = NULL,
            last_event_id = EXCLUDED.last_event_id,
            last_seq = EXCLUDED.last_seq,
            updated_at = EXCLUDED.updated_at
        "#,
    )
    .bind(payload.strategy_id)
    .bind(payload.tenant_id)
    .bind(payload.account_id)
    .bind(&payload.strategy_name)
    .bind(&payload.strategy_type)
    .bind(payload.detector_config)
    .bind(&payload.risk_config)
    .bind(envelope.occurred_at)
    .bind(envelope.event_id)
    .bind(envelope.seq)
    .bind(envelope.occurred_at)
    .execute(pool)
    .await?;

    Ok(())
}

pub(crate) async fn handle_strategy_disabled(
    pool: &PgPool,
    envelope: &EventEnvelope,
) -> Result<()> {
    let payload: StrategyDisabled =
        serde_json::from_value(envelope.payload.clone()).map_err(|e| {
            ProjectionError::InvalidPayload {
                event_type: envelope.event_type.clone(),
                reason: e.to_string(),
            }
        })?;

    sqlx::query(
        r#"
        UPDATE strategy_state_current
        SET
            is_enabled = false,
            disabled_at = $2,
            disabled_reason = $3,
            last_event_id = $4,
            last_seq = $5,
            updated_at = $6
        WHERE strategy_id = $1 AND last_seq < $5
        "#,
    )
    .bind(payload.strategy_id)
    .bind(envelope.occurred_at)
    .bind(payload.reason)
    .bind(envelope.event_id)
    .bind(envelope.seq)
    .bind(envelope.occurred_at)
    .execute(pool)
    .await?;

    Ok(())
}
