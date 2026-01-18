//! Event Appending with Optimistic Concurrency

use crate::idempotency::compute_idempotency_key;
use crate::types::{ActorType, Event, EventEnvelope, EventLogError, Result};
use chrono::Utc;
use sqlx::{PgPool, Postgres, Transaction};
use tracing::{debug, warn};
use uuid::Uuid;

/// Append event to log with optimistic concurrency
///
/// # Arguments
/// * `pool` - Database connection pool
/// * `stream_key` - Stream key (e.g., "position:uuid")
/// * `expected_seq` - Expected sequence number (None = no check)
/// * `event` - Event to append
///
/// # Returns
/// Event ID of appended event (or existing event if idempotent duplicate)
///
/// # Errors
/// - `ConcurrentModification` if expected_seq doesn't match
/// - `Database` on SQL errors
pub async fn append_event(
    pool: &PgPool,
    stream_key: &str,
    expected_seq: Option<i64>,
    event: Event,
) -> Result<Uuid> {
    let mut tx = pool.begin().await?;
    let event_id = append_event_tx(&mut tx, stream_key, expected_seq, event).await?;
    tx.commit().await?;
    Ok(event_id)
}

/// Append event within an existing transaction
///
/// Use this when you need to append event + update projections atomically.
pub async fn append_event_tx(
    tx: &mut Transaction<'_, Postgres>,
    stream_key: &str,
    expected_seq: Option<i64>,
    event: Event,
) -> Result<Uuid> {
    // 1. Get next sequence with lock
    let next_seq = get_next_seq(tx, stream_key, &event.tenant_id, expected_seq).await?;

    // 2. Generate event ID
    let event_id = Uuid::new_v4();

    // 3. Compute idempotency key
    let idempotency_key = compute_idempotency_key(
        event.tenant_id,
        stream_key,
        event.command_id,
        &event.payload,
    );

    // 4. Insert event
    let result = sqlx::query(
        r#"
        INSERT INTO event_log (
            event_id, tenant_id, stream_key, seq, event_type, payload,
            occurred_at, idempotency_key, trace_id, causation_id, command_id,
            workflow_id, actor_type, actor_id, payload_schema_version
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)
        "#,
    )
    .bind(&event_id)
    .bind(&event.tenant_id)
    .bind(stream_key)
    .bind(next_seq)
    .bind(&event.event_type)
    .bind(&event.payload)
    .bind(&event.occurred_at)
    .bind(&idempotency_key)
    .bind(&event.trace_id)
    .bind(&event.causation_id)
    .bind(&event.command_id)
    .bind(&event.workflow_id)
    .bind(event.actor_type.as_ref().map(|a| a.as_str()))
    .bind(&event.actor_id)
    .bind(event.payload_schema_version)
    .execute(&mut **tx)
    .await;

    match result {
        Ok(_) => {
            // Update stream state
            update_stream_state(tx, stream_key, next_seq, event_id).await?;

            debug!(
                event_id = %event_id,
                stream_key = %stream_key,
                seq = %next_seq,
                event_type = %event.event_type,
                "Event appended"
            );

            Ok(event_id)
        }
        Err(sqlx::Error::Database(db_err)) if is_unique_violation(db_err.as_ref()) => {
            // Idempotent duplicate - return existing event ID
            let existing_event_id: Uuid = sqlx::query_scalar(
                "SELECT event_id FROM event_log WHERE idempotency_key = $1",
            )
            .bind(&idempotency_key)
            .fetch_one(&mut **tx)
            .await?;

            warn!(
                existing_event_id = %existing_event_id,
                idempotency_key = %idempotency_key,
                "Idempotent event duplicate detected"
            );

            Err(EventLogError::IdempotentDuplicate(existing_event_id))
        }
        Err(e) => Err(EventLogError::Database(e)),
    }
}

/// Get next sequence number for stream with optional optimistic concurrency check
async fn get_next_seq(
    tx: &mut Transaction<'_, Postgres>,
    stream_key: &str,
    tenant_id: &Uuid,
    expected_seq: Option<i64>,
) -> Result<i64> {
    if let Some(exp_seq) = expected_seq {
        // Optimistic concurrency: verify expected sequence
        let current_seq: Option<i64> = sqlx::query_scalar(
            "SELECT last_seq FROM stream_state WHERE stream_key = $1 FOR UPDATE",
        )
        .bind(stream_key)
        .fetch_optional(&mut **tx)
        .await?;

        match current_seq {
            Some(seq) if seq == exp_seq => Ok(seq + 1),
            Some(seq) => Err(EventLogError::ConcurrentModification {
                expected: exp_seq,
                actual: seq,
            }),
            None if exp_seq == 0 => Ok(1),
            None => Err(EventLogError::StreamNotFound(stream_key.to_string())),
        }
    } else {
        // Just get next sequence (no concurrency check)
        let next_seq: i64 = sqlx::query_scalar("SELECT next_seq($1, $2)")
            .bind(stream_key)
            .bind(tenant_id)
            .fetch_one(&mut **tx)
            .await?;

        Ok(next_seq)
    }
}

/// Update stream state after successful event append
async fn update_stream_state(
    tx: &mut Transaction<'_, Postgres>,
    stream_key: &str,
    seq: i64,
    event_id: Uuid,
) -> Result<()> {
    sqlx::query(
        "UPDATE stream_state SET last_seq = $1, last_event_id = $2, updated_at = NOW() WHERE stream_key = $3"
    )
    .bind(seq)
    .bind(event_id)
    .bind(stream_key)
    .execute(&mut **tx)
    .await?;

    Ok(())
}

/// Check if database error is a unique constraint violation
fn is_unique_violation(db_err: &dyn sqlx::error::DatabaseError) -> bool {
    db_err.code() == Some(std::borrow::Cow::Borrowed("23505"))
}

// TODO: Implement batch append for performance
// pub async fn append_events_batch(
//     pool: &PgPool,
//     stream_key: &str,
//     events: Vec<Event>,
// ) -> Result<Vec<Uuid>> {
//     // Batch insert for better performance
//     todo!("Implement batch event append")
// }

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // TODO: Add integration tests with test database
    // - Test successful append
    // - Test idempotent duplicate
    // - Test concurrent modification
    // - Test sequence ordering
}
