//! Minimal data initialization for Robson v2.
//!
//! Seeds tenant/account/strategy rows for system startup.

use sqlx::{PgPool, Row};
use tracing::info;
use uuid::Uuid;

use super::Result;

/// Initialize minimal data for the system to start.
///
/// Creates tenant/account/strategy rows if they don't exist.
/// Uses INSERT ... ON CONFLICT DO NOTHING for idempotency.
pub async fn init_minimal_data(
    pool: &PgPool,
    tenant_id: Option<Uuid>,
    account_id: Option<Uuid>,
) -> Result<(Uuid, Uuid)> {
    let tenant_id = tenant_id.unwrap_or_else(Uuid::now_v7);
    let account_id = account_id.unwrap_or_else(Uuid::now_v7);

    let mut tx = pool.begin().await?;

    // Check if strategy already exists for this tenant/account
    let existing = sqlx::query(
        r#"
        SELECT strategy_id FROM strategy_state_current
        WHERE tenant_id = $1 AND account_id = $2
        LIMIT 1
        "#,
    )
    .bind(tenant_id)
    .bind(account_id)
    .fetch_optional(&mut *tx)
    .await?;

    if existing.is_some() {
        info!("Strategy already exists for tenant={}, account={}", tenant_id, account_id);
        tx.commit().await?;
        return Ok((tenant_id, account_id));
    }

    // Insert minimal strategy_state_current row
    let strategy_id = Uuid::now_v7();
    info!(
        "Creating minimal strategy: id={}, tenant={}, account={}",
        strategy_id, tenant_id, account_id
    );

    sqlx::query(
        r#"
        INSERT INTO strategy_state_current (
            strategy_id, tenant_id, account_id,
            strategy_name, strategy_type, risk_config,
            is_enabled, total_signals, total_positions, open_positions,
            total_pnl, win_rate,
            last_event_id, last_seq,
            created_at, updated_at
        ) VALUES (
            $1, $2, $3,
            'Default Strategy', 'manual',
            '{"max_exposure": 10000, "daily_loss_limit": 100}'::jsonb,
            false, 0, 0, 0,
            0, 0,
            $4, 0,
            NOW(), NOW()
        )
        ON CONFLICT (strategy_id) DO NOTHING
        "#,
    )
    .bind(strategy_id)
    .bind(tenant_id)
    .bind(account_id)
    .bind(Uuid::now_v7()) // dummy event_id
    .execute(&mut *tx)
    .await?;

    // Insert corresponding event for audit trail
    let stream_key = format!("strategy:{}", strategy_id);
    sqlx::query(
        r#"
        INSERT INTO event_log (
            tenant_id, stream_key, seq, event_type, payload, payload_schema_version,
            occurred_at, ingested_at, idempotency_key, actor_type, actor_id
        ) VALUES (
            $1, $2, 1, 'STRATEGY_CREATED',
            $3::jsonb,
            1,
            NOW(), NOW(),
            $4,
            'System',
            'db-init'
        )
        ON CONFLICT (idempotency_key, ingested_at) DO NOTHING
        "#,
    )
    .bind(tenant_id)
    .bind(&stream_key)
    .bind(serde_json::json!({
        "strategy_id": strategy_id,
        "tenant_id": tenant_id,
        "account_id": account_id,
        "name": "Default Strategy",
        "type": "manual",
        "risk_config": {"max_exposure": 10000, "daily_loss_limit": 100}
    }))
    .bind(format!("strategy-created-{}", strategy_id))
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;

    info!("Minimal data initialized successfully");
    Ok((tenant_id, account_id))
}
