//! Query audit projection handlers

use robson_eventlog::EventEnvelope;
use sqlx::PgPool;

use crate::{
    error::{ProjectionError, Result},
    types::QueryStateChanged,
};

pub(crate) async fn handle_query_state_changed(
    pool: &PgPool,
    envelope: &EventEnvelope,
) -> Result<()> {
    let payload: QueryStateChanged =
        serde_json::from_value(envelope.payload.clone()).map_err(|e| {
            ProjectionError::InvalidPayload {
                event_type: envelope.event_type.clone(),
                reason: e.to_string(),
            }
        })?;

    sqlx::query(
        r#"
        INSERT INTO queries_current (
            query_id, tenant_id, stream_key, position_id,
            state, started_at, finished_at, snapshot,
            last_event_id, last_seq, updated_at
        ) VALUES (
            $1, $2, $3, $4,
            $5, $6, $7, $8,
            $9, $10, $11
        )
        ON CONFLICT (query_id) DO UPDATE SET
            tenant_id = EXCLUDED.tenant_id,
            stream_key = EXCLUDED.stream_key,
            position_id = EXCLUDED.position_id,
            state = EXCLUDED.state,
            started_at = EXCLUDED.started_at,
            finished_at = EXCLUDED.finished_at,
            snapshot = EXCLUDED.snapshot,
            last_event_id = EXCLUDED.last_event_id,
            last_seq = EXCLUDED.last_seq,
            updated_at = EXCLUDED.updated_at
        WHERE queries_current.last_seq < EXCLUDED.last_seq
        "#,
    )
    .bind(payload.query_id)
    .bind(envelope.tenant_id)
    .bind(&envelope.stream_key)
    .bind(payload.position_id)
    .bind(&payload.state)
    .bind(payload.started_at)
    .bind(payload.finished_at)
    .bind(payload.snapshot)
    .bind(envelope.event_id)
    .bind(envelope.seq)
    .bind(envelope.occurred_at)
    .execute(pool)
    .await?;

    Ok(())
}
