//! Risk projection handlers

use crate::error::{ProjectionError, Result};
use crate::types::RiskCheckFailed;
use robson_eventlog::EventEnvelope;
use sqlx::PgPool;
use uuid::Uuid;

pub(crate) async fn handle_risk_check_failed(
    pool: &PgPool,
    envelope: &EventEnvelope,
) -> Result<()> {
    let payload: RiskCheckFailed = serde_json::from_value(envelope.payload.clone())
        .map_err(|e| ProjectionError::InvalidPayload {
            event_type: envelope.event_type.clone(),
            reason: e.to_string(),
        })?;

    // Generate or use existing risk_state_current record
    // For now, we'll create/update based on account_id + strategy_id

    let risk_id = Uuid::new_v4();

    sqlx::query(
        r#"
        INSERT INTO risk_state_current (
            risk_id, tenant_id, account_id, strategy_id,
            is_violated, violation_reason, violated_at,
            last_event_id, last_seq,
            calculated_at, updated_at
        ) VALUES (
            $1, $2, $3, $4,
            true, $5, $6,
            $7, $8,
            $9, $9
        )
        ON CONFLICT (uk_risk_account_strategy) DO UPDATE SET
            is_violated = true,
            violation_reason = EXCLUDED.violation_reason,
            violated_at = EXCLUDED.violated_at,
            last_event_id = EXCLUDED.last_event_id,
            last_seq = EXCLUDED.last_seq,
            updated_at = EXCLUDED.updated_at
        "#,
    )
    .bind(risk_id)
    .bind(payload.tenant_id)
    .bind(payload.account_id)
    .bind(payload.strategy_id)
    .bind(&payload.violation_reason)
    .bind(envelope.occurred_at)
    .bind(envelope.event_id)
    .bind(envelope.seq)
    .bind(envelope.occurred_at)
    .execute(pool)
    .await?;

    Ok(())
}
