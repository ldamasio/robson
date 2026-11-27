//! Event Appending with Optimistic Concurrency

use chrono::Utc;
use sqlx::{PgPool, Postgres, Row, Transaction};
use tracing::{debug, warn, Instrument as _};
use uuid::Uuid;

use crate::{
    idempotency::compute_idempotency_key,
    types::{ActorType, Event, EventEnvelope, EventLogError, Result},
};

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
/// Its first statement takes the cutover certificate's key shared without
/// waiting (8m PLAN-r4 §6.7): while completion holds or waits for it the
/// append rolls back with nothing written. [`append_event_tx`] takes no key:
/// its caller's transaction participates.
///
/// # Errors
/// - `ConcurrentModification` if expected_seq doesn't match
/// - `CertificateInProgress`, certain: nothing was written
/// - `Database` on SQL errors
pub async fn append_event(
    pool: &PgPool,
    stream_key: &str,
    expected_seq: Option<i64>,
    event: Event,
) -> Result<Uuid> {
    let mut tx = pool
        .begin()
        .instrument(
            tracing::info_span!(target: "robson_operation", "operation_io", io_kind = 2_u64),
        )
        .await?;
    let mut rollback_observation = RollbackObservation::new();
    if crate::certificate::try_participate(&mut tx).await?.is_none() {
        rollback_observation.disarm();
        // Nothing was written whether or not the rollback is answered.
        let _ = tx
            .rollback()
            .instrument(
                tracing::info_span!(target: "robson_operation", "operation_io", io_kind = 4_u64),
            )
            .await;
        return Err(EventLogError::CertificateInProgress);
    }
    let event_id = append_event_tx(&mut tx, stream_key, expected_seq, event).await?;
    tx.commit()
        .instrument(
            tracing::info_span!(target: "robson_operation", "operation_io", io_kind = 3_u64),
        )
        .await?;
    rollback_observation.disarm();
    Ok(event_id)
}

/// Append event within an existing transaction
///
/// Use this when you need to append event + update projections atomically.
/// The caller's transaction takes the certificate key first
/// ([`crate::certificate::participate`]).
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
    let idempotency_key =
        compute_idempotency_key(event.tenant_id, stream_key, event.command_id, &event.payload);

    // 4. Check global idempotency via event_idempotency table
    let idempotency_check = sqlx::query(
        r#"
        INSERT INTO event_idempotency (tenant_id, idempotency_key, event_id)
        VALUES ($1, $2, $3)
        ON CONFLICT (tenant_id, idempotency_key) DO NOTHING
        RETURNING event_id
        "#,
    )
    .bind(&event.tenant_id)
    .bind(&idempotency_key)
    .bind(&event_id)
    .fetch_optional(&mut **tx)
    .instrument(tracing::info_span!(target: "robson_operation", "operation_io", io_kind = 1_u64))
    .await;

    match idempotency_check {
        Ok(Some(returned_event_id)) => {
            // This is a new event - the INSERT succeeded
            debug!(
                event_id = %event_id,
                "Global idempotency check passed (new event)"
            );
        },
        Ok(None) => {
            // Duplicate detected - fetch the existing event_id
            let existing_event_id: Uuid = sqlx::query_scalar(
                "SELECT event_id FROM event_idempotency WHERE tenant_id = $1 AND idempotency_key = $2",
            )
            .bind(&event.tenant_id)
            .bind(&idempotency_key)
            .fetch_one(&mut **tx)
            .instrument(tracing::info_span!(target: "robson_operation", "operation_io", io_kind = 1_u64))
        .await?;

            warn!(
                existing_event_id = %existing_event_id,
                idempotency_key = %idempotency_key,
                "Idempotent event duplicate detected (global check)"
            );

            return Err(EventLogError::IdempotentDuplicate(existing_event_id));
        },
        Err(e) => {
            return Err(EventLogError::Database(e));
        },
    }

    // 5. Insert event into event_log (partitioned)
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
    .instrument(tracing::info_span!(target: "robson_operation", "operation_io", io_kind = 1_u64))
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
        },
        Err(e) => Err(EventLogError::Database(e)),
    }
}

/// Get next sequence number for stream with optional optimistic concurrency
/// check
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
        .instrument(
            tracing::info_span!(target: "robson_operation", "operation_io", io_kind = 1_u64),
        )
        .await?;

        match current_seq {
            Some(seq) if seq == exp_seq => Ok(seq + 1),
            Some(seq) => {
                Err(EventLogError::ConcurrentModification { expected: exp_seq, actual: seq })
            },
            None if exp_seq == 0 => Ok(1),
            None => Err(EventLogError::StreamNotFound(stream_key.to_string())),
        }
    } else {
        // Just get next sequence (no concurrency check)
        let next_seq: i64 = sqlx::query_scalar("SELECT next_seq($1, $2)")
            .bind(stream_key)
            .bind(tenant_id)
            .fetch_one(&mut **tx)
            .instrument(
                tracing::info_span!(target: "robson_operation", "operation_io", io_kind = 1_u64),
            )
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
        "UPDATE stream_state SET last_seq = $1, last_event_id = $2, updated_at = NOW() WHERE stream_key = $3",
    )
    .bind(seq)
    .bind(event_id)
    .bind(stream_key)
    .execute(&mut **tx)
    .instrument(tracing::info_span!(target: "robson_operation", "operation_io", io_kind = 1_u64))
        .await?;

    Ok(())
}


// The public SQLx Transaction Drop queues rollback while open. This only counts
// that application cleanup request; it never claims server acknowledgement.
pub(crate) struct RollbackObservation {
    origin: tracing::Span,
    armed: bool,
}
impl RollbackObservation {
    pub(crate) fn new() -> Self {
        Self {
            origin: tracing::Span::current(),
            armed: true,
        }
    }
    pub(crate) fn disarm(&mut self) {
        self.armed = false;
    }
}
impl Drop for RollbackObservation {
    fn drop(&mut self) {
        if self.armed {
            tracing::info_span!(target: "robson_operation", parent: &self.origin, "operation_io", io_kind = 4_u64).in_scope(|| {});
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    // TODO: Add integration tests with test database
    // - Test successful append
    // - Test idempotent duplicate
    // - Test concurrent modification
    // - Test sequence ordering
}
