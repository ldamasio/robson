//! Balance projection handlers

use crate::error::{ProjectionError, Result};
use crate::types::BalanceSampled;
use robson_eventlog::EventEnvelope;
use sqlx::PgPool;

pub(crate) async fn handle_balance_sampled(
    pool: &PgPool,
    envelope: &EventEnvelope,
) -> Result<()> {
    let payload: BalanceSampled = serde_json::from_value(envelope.payload.clone())
        .map_err(|e| ProjectionError::InvalidPayload {
            event_type: envelope.event_type.clone(),
            reason: e.to_string(),
        })?;

    // Idempotency check via seq
    let existing = sqlx::query_scalar::<_, i64>(
        "SELECT last_seq FROM balances_current WHERE balance_id = $1"
    )
    .bind(payload.balance_id)
    .fetch_optional(pool)
    .await?;

    if let Some(seq) = existing {
        if seq >= envelope.seq {
            tracing::debug!("BalanceSampled already applied: seq={}", seq);
            return Ok(());
        }
    }

    sqlx::query(
        r#"
        INSERT INTO balances_current (
            balance_id, tenant_id, account_id, asset,
            free, locked,
            last_event_id, last_seq,
            sampled_at, updated_at
        ) VALUES (
            $1, $2, $3, $4,
            $5, $6,
            $7, $8,
            $9, $10
        )
        ON CONFLICT (balance_id) DO UPDATE SET
            free = EXCLUDED.free,
            locked = EXCLUDED.locked,
            last_event_id = EXCLUDED.last_event_id,
            last_seq = EXCLUDED.last_seq,
            sampled_at = EXCLUDED.sampled_at,
            updated_at = EXCLUDED.updated_at
        "#,
    )
    .bind(payload.balance_id)
    .bind(payload.tenant_id)
    .bind(payload.account_id)
    .bind(&payload.asset)
    .bind(payload.free)
    .bind(payload.locked)
    .bind(envelope.event_id)
    .bind(envelope.seq)
    .bind(payload.sampled_at)
    .bind(envelope.occurred_at)
    .execute(pool)
    .await?;

    Ok(())
}
