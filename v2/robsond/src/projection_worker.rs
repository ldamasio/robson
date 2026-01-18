//! Projection worker: polls event log and applies events to projections.
//!
//! Uses a simple cursor file to track last processed sequence number.

use crate::config::ProjectionConfig;
use crate::error::{DaemonError, DaemonResult};
use robson_eventlog::EventEnvelope;
use robson_projector::apply_event_to_projections;
use sqlx::PgPool;
use std::path::Path;
use tokio::time::{Duration, interval};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

/// Projection worker.
pub struct ProjectionWorker {
    pool: PgPool,
    config: ProjectionConfig,
    tenant_id: Uuid,
    cursor_file: String,
}

impl ProjectionWorker {
    /// Create a new projection worker.
    pub fn new(pool: PgPool, config: ProjectionConfig, tenant_id: Uuid) -> Self {
        let cursor_file =
            format!("/tmp/robson_projection_cursor_{}.txt", config.stream_key.replace(':', "_"));

        Self { pool, config, tenant_id, cursor_file }
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

    /// Load cursor from local file.
    async fn load_cursor(&self) -> DaemonResult<i64> {
        if Path::new(&self.cursor_file).exists() {
            let content = tokio::fs::read_to_string(&self.cursor_file)
                .await
                .map_err(|e| DaemonError::Config(format!("Failed to read cursor: {}", e)))?;

            let seq = content
                .trim()
                .parse::<i64>()
                .map_err(|_| DaemonError::Config(format!("Invalid cursor value: {}", content)))?;

            debug!(seq, "Loaded projection cursor from file");
            Ok(seq)
        } else {
            debug!("No cursor file found, starting from seq 0");
            Ok(0)
        }
    }

    /// Save cursor to local file.
    async fn save_cursor(&self, seq: i64) -> DaemonResult<()> {
        tokio::fs::write(&self.cursor_file, seq.to_string())
            .await
            .map_err(|e| DaemonError::Config(format!("Failed to write cursor: {}", e)))?;
        debug!(seq, "Saved projection cursor to file");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cursor_file_format() {
        let config = ProjectionConfig {
            database_url: None,
            stream_key: "robson:daemon".to_string(),
            poll_interval_ms: 100,
        };

        let cursor_file =
            format!("/tmp/robson_projection_cursor_{}.txt", config.stream_key.replace(':', "_"));

        assert_eq!(cursor_file, "/tmp/robson_projection_cursor_robson_daemon.txt");
    }
}
