//! Projection worker: polls event log and applies events to projections.
//!
//! Uses a PostgreSQL checkpoint row to track last processed sequence number.

use robson_eventlog::EventEnvelope;
use robson_projector::apply_event_to_projections;
use sqlx::PgPool;
use tokio::time::{interval, Duration};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::{config::ProjectionConfig, error::DaemonResult};

const PROJECTION_NAME: &str = "robson-projector";

/// Projection worker.
pub struct ProjectionWorker {
    pool: PgPool,
    config: ProjectionConfig,
    tenant_id: Uuid,
}

impl ProjectionWorker {
    /// Create a new projection worker.
    pub fn new(pool: PgPool, config: ProjectionConfig, tenant_id: Uuid) -> Self {
        Self { pool, config, tenant_id }
    }

    /// Run the projection worker loop.
    ///
    /// Returns when shutdown is signaled via cancellation token.
    pub async fn run(self, shutdown: tokio_util::sync::CancellationToken) -> DaemonResult<()> {
        info!(
            stream_key = %self.config.stream_key,
            poll_interval_ms = self.config.poll_interval_ms,
            "Projection worker started"
        );

        // Load initial cursor
        let mut last_seq = self.load_cursor().await?;

        let mut ticker = interval(Duration::from_millis(self.config.poll_interval_ms));
        ticker.tick().await; // First tick is immediate

        loop {
            tokio::select! {
                _ = shutdown.cancelled() => {
                    info!("Projection worker shutdown requested");
                    break;
                }
                _ = ticker.tick() => {
                    match self.poll_and_apply(&mut last_seq).await {
                        Ok(count) if count > 0 => {
                            debug!(count, "Applied events to projections");
                        }
                        Err(e) => {
                            error!(error = %e, "Projection error (will retry)");
                        }
                        _ => {}
                    }
                }
            }
        }

        info!("Projection worker stopped");
        Ok(())
    }

    /// Poll for new events and apply them.
    async fn poll_and_apply(&self, last_seq: &mut i64) -> DaemonResult<usize> {
        let events = sqlx::query_as::<_, EventEnvelope>(
            r#"
            SELECT event_id, tenant_id, stream_key, seq,
                   event_type, payload, payload_schema_version,
                   occurred_at, ingested_at, idempotency_key,
                   trace_id, causation_id, command_id, workflow_id,
                   actor_type, actor_id,
                   prev_hash, hash
            FROM event_log
            WHERE tenant_id = $1
              AND stream_key = $2
              AND seq > $3
            ORDER BY seq ASC
            LIMIT 1000
            "#,
        )
        .bind(self.tenant_id)
        .bind(&self.config.stream_key)
        .bind(*last_seq)
        .fetch_all(&self.pool)
        .await?;

        if events.is_empty() {
            return Ok(0);
        }

        let mut applied = 0;
        for event in &events {
            match apply_event_to_projections(&self.pool, event).await {
                Ok(()) => {
                    info!(
                        seq = event.seq,
                        event_type = %event.event_type,
                        "Applied event to projections"
                    );
                    *last_seq = event.seq;
                    applied += 1;
                },
                Err(e) => {
                    // Log error but don't advance cursor
                    // This event will be retried on next poll
                    warn!(
                        event_id = %event.event_id,
                        seq = event.seq,
                        event_type = %event.event_type,
                        error = %e,
                        "Failed to apply event (will retry)"
                    );
                    // Stop processing this batch on first error
                    // to maintain ordering
                    break;
                },
            }
        }

        if applied > 0 {
            self.save_cursor(*last_seq).await?;
        }

        Ok(applied)
    }

    /// Load cursor from PostgreSQL checkpoint row.
    async fn load_cursor(&self) -> DaemonResult<i64> {
        let seq = sqlx::query_scalar::<_, i64>(
            r#"
            SELECT last_seq
            FROM projection_checkpoints
            WHERE projection_name = $1
              AND tenant_id = $2
              AND stream_key = $3
            "#,
        )
        .bind(PROJECTION_NAME)
        .bind(self.tenant_id)
        .bind(&self.config.stream_key)
        .fetch_optional(&self.pool)
        .await?;

        match seq {
            Some(seq) => {
                debug!(seq, "Loaded projection cursor from database checkpoint");
                Ok(seq)
            },
            None => {
                debug!("No projection checkpoint found, starting from seq 0");
                Ok(0)
            },
        }
    }

    /// Save cursor to PostgreSQL checkpoint row.
    async fn save_cursor(&self, seq: i64) -> DaemonResult<()> {
        sqlx::query(
            r#"
            INSERT INTO projection_checkpoints (
                projection_name, tenant_id, stream_key, last_seq, updated_at
            ) VALUES ($1, $2, $3, $4, NOW())
            ON CONFLICT (projection_name, tenant_id, stream_key) DO UPDATE SET
                last_seq = EXCLUDED.last_seq,
                updated_at = EXCLUDED.updated_at
            "#,
        )
        .bind(PROJECTION_NAME)
        .bind(self.tenant_id)
        .bind(&self.config.stream_key)
        .bind(seq)
        .execute(&self.pool)
        .await?;

        debug!(seq, "Saved projection cursor to database checkpoint");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "postgres")]
    use chrono::{TimeZone, Utc};
    #[cfg(feature = "postgres")]
    use robson_domain::{Price, Side, Symbol};
    #[cfg(feature = "postgres")]
    use robson_eventlog::{append_event, ActorType, Event, QUERY_STATE_CHANGED_EVENT_TYPE};
    #[cfg(feature = "postgres")]
    use rust_decimal_macros::dec;
    #[cfg(feature = "postgres")]
    use uuid::Uuid;

    use super::*;
    #[cfg(feature = "postgres")]
    use crate::query::{ActorKind, ExecutionQuery, QueryKind, QueryOutcome, QueryState};
    #[cfg(feature = "postgres")]
    use crate::query_engine::QueryStateChangedEvent;

    #[test]
    fn test_projection_name_constant() {
        assert_eq!(PROJECTION_NAME, "robson-projector");
    }

    #[cfg(feature = "postgres")]
    fn ts(hour: u32, minute: u32, second: u32) -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 4, 5, hour, minute, second).single().unwrap()
    }

    #[cfg(feature = "postgres")]
    fn base_query(
        query_id: Uuid,
        position_id: Uuid,
        started_at: chrono::DateTime<Utc>,
    ) -> ExecutionQuery {
        let mut query = ExecutionQuery::new(
            QueryKind::ProcessSignal {
                signal_id: Uuid::from_u128(0xA1),
                symbol: Symbol::from_pair("BTCUSDT").unwrap(),
                side: Side::Long,
                entry_price: Price::new(dec!(95000)).unwrap(),
                stop_loss: Price::new(dec!(93500)).unwrap(),
            },
            ActorKind::Detector,
        );
        query.id = query_id;
        query.position_id = Some(position_id);
        query.started_at = started_at;
        query.finished_at = None;
        query.outcome = None;
        query.approval = None;
        query
    }

    #[cfg(feature = "postgres")]
    async fn append_query_event(
        pool: &PgPool,
        tenant_id: Uuid,
        stream_key: &str,
        query: &ExecutionQuery,
        transition_cause: &str,
        occurred_at: chrono::DateTime<Utc>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let payload = QueryStateChangedEvent::from_query(query, transition_cause);
        let mut event = Event::new(
            tenant_id,
            stream_key,
            QUERY_STATE_CHANGED_EVENT_TYPE,
            serde_json::to_value(payload)?,
        )
        .with_actor(ActorType::Daemon, Some("projection-worker-test".to_string()));
        event.occurred_at = occurred_at;
        append_event(pool, stream_key, None, event).await?;
        Ok(())
    }

    #[cfg(feature = "postgres")]
    #[sqlx::test(migrations = "../migrations")]
    #[ignore = "Requires DATABASE_URL to be set"]
    async fn test_projection_worker_persists_checkpoint_across_restart(pool: PgPool) {
        let tenant_id = Uuid::from_u128(0x100);
        let stream_key = "robson:daemon:phase4:checkpoint";
        let config = ProjectionConfig {
            database_url: None,
            tenant_id: Some(tenant_id),
            stream_key: stream_key.to_string(),
            poll_interval_ms: 10,
        };

        let query_id = Uuid::from_u128(0x101);
        let position_id = Uuid::from_u128(0x102);

        let mut processing_query = base_query(query_id, position_id, ts(10, 0, 0));
        processing_query.transition(QueryState::Processing).unwrap();

        append_query_event(
            &pool,
            tenant_id,
            stream_key,
            &processing_query,
            "processing",
            ts(10, 0, 1),
        )
        .await
        .unwrap();

        let worker = ProjectionWorker::new(pool.clone(), config.clone(), tenant_id);
        let mut cursor = worker.load_cursor().await.unwrap();
        assert_eq!(cursor, 0);

        let applied = worker.poll_and_apply(&mut cursor).await.unwrap();
        assert_eq!(applied, 1);
        assert_eq!(cursor, 1);

        let checkpoint: i64 = sqlx::query_scalar(
            r#"
            SELECT last_seq
            FROM projection_checkpoints
            WHERE projection_name = $1
              AND tenant_id = $2
              AND stream_key = $3
            "#,
        )
        .bind(PROJECTION_NAME)
        .bind(tenant_id)
        .bind(stream_key)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(checkpoint, 1);

        let mut completed_query = processing_query.clone();
        completed_query.transition(QueryState::Acting).unwrap();
        completed_query
            .complete(QueryOutcome::ActionsExecuted { actions_count: 1 })
            .unwrap();
        completed_query.finished_at = Some(ts(10, 0, 3));

        append_query_event(
            &pool,
            tenant_id,
            stream_key,
            &completed_query,
            "completed",
            ts(10, 0, 4),
        )
        .await
        .unwrap();

        let restarted_worker = ProjectionWorker::new(pool.clone(), config, tenant_id);
        let mut restarted_cursor = restarted_worker.load_cursor().await.unwrap();
        assert_eq!(restarted_cursor, 1);

        let applied_after_restart =
            restarted_worker.poll_and_apply(&mut restarted_cursor).await.unwrap();
        assert_eq!(applied_after_restart, 1);
        assert_eq!(restarted_cursor, 2);

        let final_state: String = sqlx::query_scalar(
            r#"
            SELECT state
            FROM queries_current
            WHERE query_id = $1
            "#,
        )
        .bind(query_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(final_state, "Completed");
    }
}
