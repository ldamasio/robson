//! Minimal data initialization for Robson v2.
//!
//! Seeds tenant/account/strategy rows for system startup.

use sqlx::PgPool;
use tracing::info;
use uuid::Uuid;

use super::Result;

/// Initialize minimal data for the system to start.
///
/// Creates tenant/account/strategy rows if they don't exist.
///
/// Idempotency contract (robson#231): the ArgoCD migrate hook runs
/// `robsond db init --tenant-id $PROJECTION_TENANT_ID` on **every** sync,
/// without an account id. The bootstrap identity is therefore the tenant:
/// when the tenant already owns a strategy, that strategy's account is the
/// bootstrap account and nothing is written. Only a tenant with no strategy
/// at all gets a new account, a new strategy and one `STRATEGY_CREATED`
/// event. Previously a fresh account id was minted per run, so every deploy
/// appended a strategy row and a creation event.
pub async fn init_minimal_data(
    pool: &PgPool,
    tenant_id: Option<Uuid>,
    account_id: Option<Uuid>,
) -> Result<(Uuid, Uuid)> {
    let tenant_id = tenant_id.unwrap_or_else(Uuid::now_v7);

    let mut tx = pool.begin().await?;

    // Resolve the bootstrap identity before minting anything. With an
    // explicit account the check is per (tenant, account); without one the
    // tenant's oldest strategy wins, which is the row the first run created.
    let existing: Option<(Uuid, Uuid)> = match account_id {
        Some(account_id) => {
            sqlx::query_as(
                r#"
            SELECT strategy_id, account_id FROM strategy_state_current
            WHERE tenant_id = $1 AND account_id = $2
            ORDER BY created_at ASC
            LIMIT 1
            "#,
            )
            .bind(tenant_id)
            .bind(account_id)
            .fetch_optional(&mut *tx)
            .await?
        },
        None => {
            sqlx::query_as(
                r#"
            SELECT strategy_id, account_id FROM strategy_state_current
            WHERE tenant_id = $1
            ORDER BY created_at ASC
            LIMIT 1
            "#,
            )
            .bind(tenant_id)
            .fetch_optional(&mut *tx)
            .await?
        },
    };

    if let Some((strategy_id, account_id)) = existing {
        info!(
            "Strategy {} already exists for tenant={}, account={}; nothing to seed",
            strategy_id, tenant_id, account_id
        );
        tx.commit().await?;
        return Ok((tenant_id, account_id));
    }

    let account_id = account_id.unwrap_or_else(Uuid::now_v7);

    // Insert minimal strategy_state_current row
    let strategy_id = Uuid::now_v7();
    let event_id = Uuid::now_v7();
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
    .bind(event_id)
    .execute(&mut *tx)
    .await?;

    // Insert into event_idempotency first (for global idempotency)
    let stream_key = format!("strategy:{}", strategy_id);
    let idempotency_key = format!("strategy-created-{}", strategy_id);

    let claimed = sqlx::query(
        r#"
        INSERT INTO event_idempotency (tenant_id, idempotency_key, event_id)
        VALUES ($1, $2, $3)
        ON CONFLICT (tenant_id, idempotency_key) DO NOTHING
        "#,
    )
    .bind(tenant_id)
    .bind(&idempotency_key)
    .bind(event_id)
    .execute(&mut *tx)
    .await?
    .rows_affected();

    // Defensive: the key is derived from a freshly minted strategy id, so a
    // conflict means a concurrent run or hand-edited data, never a normal
    // re-run (those return above). Never write a creation event without
    // owning its idempotency claim.
    if claimed == 0 {
        tx.commit().await?;
        info!("Idempotency key already claimed; STRATEGY_CREATED not re-emitted");
        return Ok((tenant_id, account_id));
    }

    // Then insert the event log entry
    sqlx::query(
        r#"
        INSERT INTO event_log (
            tenant_id, stream_key, seq, event_type, payload, payload_schema_version,
            occurred_at, ingested_at, idempotency_key, actor_type, actor_id,
            event_id
        ) VALUES (
            $1, $2, 1, 'STRATEGY_CREATED',
            $3::jsonb,
            1,
            NOW(), NOW(),
            $4,
            'System',
            'db-init',
            $5
        )
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
    .bind(&idempotency_key)
    .bind(event_id)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;

    info!("Minimal data initialized successfully");
    Ok((tenant_id, account_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn count(pool: &PgPool, sql: &str, tenant_id: Uuid) -> i64 {
        sqlx::query_scalar(sql).bind(tenant_id).fetch_one(pool).await.unwrap()
    }

    /// The ArgoCD hook shape: `--tenant-id` only, re-run on every sync.
    #[sqlx::test(migrations = "../migrations")]
    #[ignore = "Requires DATABASE_URL to be set"]
    async fn rerun_with_tenant_only_seeds_once(pool: PgPool) {
        let tenant_id = Uuid::now_v7();

        let (t1, a1) = init_minimal_data(&pool, Some(tenant_id), None).await.unwrap();
        let (t2, a2) = init_minimal_data(&pool, Some(tenant_id), None).await.unwrap();
        let (t3, a3) = init_minimal_data(&pool, Some(tenant_id), None).await.unwrap();

        assert_eq!((t1, t2, t3), (tenant_id, tenant_id, tenant_id));
        assert_eq!(a1, a2, "re-run must resolve the bootstrap account, not mint one");
        assert_eq!(a2, a3);

        let strategies = count(
            &pool,
            "SELECT COUNT(*) FROM strategy_state_current WHERE tenant_id = $1",
            tenant_id,
        )
        .await;
        let created_events = count(
            &pool,
            "SELECT COUNT(*) FROM event_log WHERE tenant_id = $1 AND event_type = 'STRATEGY_CREATED'",
            tenant_id,
        )
        .await;
        assert_eq!(strategies, 1, "one strategy per tenant after three runs");
        assert_eq!(created_events, 1, "one STRATEGY_CREATED per tenant after three runs");
    }

    /// An explicit account id keeps its own identity: a second account under
    /// the same tenant is a real creation, a repeat of the same pair is not.
    #[sqlx::test(migrations = "../migrations")]
    #[ignore = "Requires DATABASE_URL to be set"]
    async fn explicit_account_is_scoped_per_pair(pool: PgPool) {
        let tenant_id = Uuid::now_v7();
        let account_a = Uuid::now_v7();
        let account_b = Uuid::now_v7();

        init_minimal_data(&pool, Some(tenant_id), Some(account_a)).await.unwrap();
        init_minimal_data(&pool, Some(tenant_id), Some(account_a)).await.unwrap();
        init_minimal_data(&pool, Some(tenant_id), Some(account_b)).await.unwrap();
        // Tenant-only after explicit seeding resolves to the oldest account.
        let (_, resolved) = init_minimal_data(&pool, Some(tenant_id), None).await.unwrap();

        assert_eq!(resolved, account_a);
        let strategies = count(
            &pool,
            "SELECT COUNT(*) FROM strategy_state_current WHERE tenant_id = $1",
            tenant_id,
        )
        .await;
        let created_events = count(
            &pool,
            "SELECT COUNT(*) FROM event_log WHERE tenant_id = $1 AND event_type = 'STRATEGY_CREATED'",
            tenant_id,
        )
        .await;
        assert_eq!(strategies, 2);
        assert_eq!(created_events, 2);
    }
}
