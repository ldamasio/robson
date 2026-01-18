//! Test helper functions for database seeding.

use chrono::{DateTime, Utc};
use serde_json::json;
use sqlx::{PgPool, Postgres, Row, Transaction};
use uuid::Uuid;

use crate::Result;
use robson_eventlog::{ActorType, EventEnvelope};

/// Options for appending events to the event log.
pub struct AppendEventOptions {
    /// Event type (e.g., "BALANCE_SAMPLED", "POSITION_OPENED")
    pub event_type: String,
    /// Stream key (e.g., "account:123")
    pub stream_key: String,
    /// Event payload as JSON value
    pub payload: serde_json::Value,
    /// Sequence number (auto-increments if None)
    pub seq: Option<i64>,
    /// When the event occurred (defaults to now)
    pub occurred_at: Option<DateTime<Utc>>,
    /// Actor type (defaults to CLI)
    pub actor_type: ActorType,
    /// Actor ID (defaults to "test-user")
    pub actor_id: Option<String>,
}

/// Seed a minimal tenant and account for testing.
///
/// Returns (tenant_id, account_id) tuple.
/// Uses INSERT ... ON CONFLICT for idempotency.
pub async fn seed_tenant_account(_pool: &PgPool) -> Result<(Uuid, Uuid)> {
    let tenant_id = Uuid::now_v7();
    let account_id = Uuid::now_v7();

    // For now, we just return generated IDs.
    // The actual tables for tenant/account don't exist yet in the schema.
    // Tests use these IDs when inserting events.

    Ok((tenant_id, account_id))
}

/// Append an event to the event log for testing.
///
/// Automatically handles idempotency_key generation and sequencing.
pub async fn append_event(
    pool: &PgPool,
    tenant_id: Uuid,
    options: AppendEventOptions,
) -> Result<EventEnvelope> {
    let mut tx = pool.begin().await?;

    let AppendEventOptions {
        event_type,
        stream_key,
        payload,
        seq,
        occurred_at,
        actor_type,
        actor_id,
    } = options;

    // Get next sequence number if not provided
    let seq = if let Some(s) = seq {
        s
    } else {
        let result = sqlx::query_scalar::<_, i64>(
            "SELECT COALESCE(MAX(seq), 0) + 1 FROM event_log WHERE stream_key = $1",
        )
        .bind(&stream_key)
        .fetch_one(&mut *tx)
        .await?;

        result
    };

    let occurred_at = occurred_at.unwrap_or_else(Utc::now);
    let idempotency_key = format!("{}:{}:{}", stream_key, seq, event_type);
    let event_id = Uuid::now_v7();
    let actor_id = actor_id.unwrap_or_else(|| "test-user".to_string());

    // Insert into event_idempotency first (for global idempotency)
    sqlx::query(
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
    .await?;

    // Then insert the event log entry
    sqlx::query(
        r#"
        INSERT INTO event_log (
            tenant_id, stream_key, seq, event_type, payload, payload_schema_version,
            occurred_at, ingested_at, idempotency_key, actor_type, actor_id,
            event_id
        ) VALUES (
            $1, $2, $3, $4, $5, 1,
            $6, NOW(), $7, $8, $9,
            $10
        )
        "#,
    )
    .bind(tenant_id)
    .bind(&stream_key)
    .bind(seq)
    .bind(&event_type)
    .bind(sqlx::types::Json(payload))
    .bind(occurred_at)
    .bind(&idempotency_key)
    .bind(actor_type as ActorType)
    .bind(&actor_id)
    .bind(event_id)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;

    // Fetch and return the created event
    let row = sqlx::query(
        r#"
        SELECT event_id, tenant_id, stream_key, seq, event_type, payload,
               payload_schema_version, occurred_at, ingested_at, idempotency_key,
               trace_id, causation_id, command_id, workflow_id,
               actor_type, actor_id, prev_hash, hash
        FROM event_log
        WHERE event_id = $1
        "#,
    )
    .bind(event_id)
    .fetch_one(pool)
    .await?;

    let actor_type_str: Option<String> = row.try_get("actor_type")?;
    let actor_type = match actor_type_str.as_deref() {
        Some("CLI") => Some(ActorType::CLI),
        Some("Daemon") => Some(ActorType::Daemon),
        Some("System") => Some(ActorType::System),
        Some("Exchange") => Some(ActorType::Exchange),
        _ => None,
    };

    Ok(EventEnvelope {
        event_id: row.try_get("event_id")?,
        tenant_id: row.try_get("tenant_id")?,
        stream_key: row.try_get("stream_key")?,
        seq: row.try_get("seq")?,
        event_type: row.try_get("event_type")?,
        payload: row.try_get("payload")?,
        payload_schema_version: row.try_get("payload_schema_version")?,
        occurred_at: row.try_get("occurred_at")?,
        ingested_at: row.try_get("ingested_at")?,
        idempotency_key: row.try_get("idempotency_key")?,
        trace_id: row.try_get("trace_id")?,
        causation_id: row.try_get("causation_id")?,
        command_id: row.try_get("command_id")?,
        workflow_id: row.try_get("workflow_id")?,
        actor_type,
        actor_id: row.try_get("actor_id")?,
        prev_hash: row.try_get("prev_hash")?,
        hash: row.try_get("hash")?,
    })
}

/// Seed a valid BALANCE_SAMPLED event for testing.
///
/// Convenience function that creates a balance event with sensible defaults.
pub async fn seed_balance_sampled_event(
    pool: &PgPool,
    tenant_id: Uuid,
    account_id: Uuid,
    asset: &str,
    free: &str,
    locked: &str,
) -> Result<EventEnvelope> {
    append_event(
        pool,
        tenant_id,
        AppendEventOptions {
            event_type: "BALANCE_SAMPLED".to_string(),
            stream_key: format!("account:{}", account_id),
            payload: json!({
                "balance_id": Uuid::now_v7(),
                "tenant_id": tenant_id,
                "account_id": account_id,
                "asset": asset,
                "free": free,
                "locked": locked,
                "sampled_at": Utc::now()
            }),
            seq: None,
            occurred_at: None,
            actor_type: ActorType::CLI,
            actor_id: Some("test-user".to_string()),
        },
    )
    .await
}

/// Seed a POSITION_OPENED event with configurable technical_stop_distance.
///
/// Use distance=0 to test invariant failures.
pub async fn seed_position_opened_event(
    pool: &PgPool,
    tenant_id: Uuid,
    account_id: Uuid,
    symbol: &str,
    entry_price: &str,
    quantity: &str,
    technical_stop_price: &str,
    technical_stop_distance: &str,
) -> Result<EventEnvelope> {
    append_event(
        pool,
        tenant_id,
        AppendEventOptions {
            event_type: "POSITION_OPENED".to_string(),
            stream_key: format!("position:{}", Uuid::now_v7()),
            payload: json!({
                "position_id": Uuid::now_v7(),
                "tenant_id": tenant_id,
                "account_id": account_id,
                "symbol": symbol,
                "side": "long",
                "entry_price": entry_price,
                "entry_quantity": quantity,
                "technical_stop_price": technical_stop_price,
                "technical_stop_distance": technical_stop_distance,
                "entry_filled_at": Utc::now()
            }),
            seq: None,
            occurred_at: None,
            actor_type: ActorType::CLI,
            actor_id: Some("test-user".to_string()),
        },
    )
    .await
}
