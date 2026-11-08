//! PostgreSQL side of the order journal (ADR-0056 step 7, PR J).
//!
//! Every write is one explicit transaction ending in `Transaction::commit`,
//! which goes out as a simple-query `COMMIT`. Nothing here commits implicitly.

use std::time::Duration;

use async_trait::async_trait;
use robson_exec::order_journal::{CommitFailure, JournalBackend, JournalBatch};
use sqlx::{pool::PoolConnection, postgres::PgPoolOptions, Connection, PgPool, Postgres};
use uuid::Uuid;

/// The journal's own pool, so order writes never queue behind the daemon's.
const JOURNAL_POOL_CONNECTIONS: u32 = 2;
/// Inside the 500 ms commit bound, so a pool with no connection to give ends
/// as a known failure: no statement was sent.
const JOURNAL_ACQUIRE_BOUND: Duration = Duration::from_millis(250);

/// A connection under an outer timeout, closed instead of returned to the pool
/// unless its work finished. A dropped future leaves a statement whose replies
/// may never arrive; returned to the pool it would wait on a ping, holding its
/// slot with no deadline, and two of those are the whole pool.
struct RetiringConnection {
    connection: PoolConnection<Postgres>,
    finished: bool,
}

impl Drop for RetiringConnection {
    fn drop(&mut self) {
        if !self.finished {
            self.connection.close_on_drop();
        }
    }
}

/// How long the whole startup check may take, probes and retries included.
const JOURNAL_READY_WAIT: Duration = Duration::from_secs(10);

pub struct PgJournalBackend {
    pool: PgPool,
    tenant_id: Uuid,
    ready_wait: Duration,
}

impl PgJournalBackend {
    /// Connects lazily, from the daemon pool's connection settings.
    pub fn new(daemon_pool: &PgPool, tenant_id: Uuid) -> Self {
        // One connection stays open: connecting can take longer than the acquire
        // bound, and an order should not pay for it.
        let pool = PgPoolOptions::new()
            .max_connections(JOURNAL_POOL_CONNECTIONS)
            .min_connections(1)
            .acquire_timeout(JOURNAL_ACQUIRE_BOUND)
            .connect_lazy_with((*daemon_pool.connect_options()).clone());
        Self {
            pool,
            tenant_id,
            ready_wait: JOURNAL_READY_WAIT,
        }
    }

    /// Change how long the startup check may take.
    pub fn with_ready_wait(mut self, ready_wait: Duration) -> Self {
        self.ready_wait = ready_wait;
        self
    }

    /// Probe until the journal answers or says its schema is missing. A probe
    /// that stalls is ended by the caller's deadline, which retires its
    /// connection.
    async fn probe_until_ready(&self) -> Result<(), String> {
        loop {
            let probe = async {
                let connection = self.pool.acquire().await?;
                let mut read = RetiringConnection { connection, finished: false };
                sqlx::query(
                    "SELECT 1 FROM order_intents, submit_attempts, journal_decisions LIMIT 0",
                )
                .execute(&mut *read.connection)
                .await?;
                read.finished = true;
                Ok::<_, sqlx::Error>(())
            };
            match probe.await {
                Ok(()) => return Ok(()),
                // The schema is missing or unreadable: waiting will not help.
                Err(sqlx::Error::Database(error)) => return Err(error.to_string()),
                Err(_) => tokio::time::sleep(Duration::from_millis(100)).await,
            }
        }
    }

    async fn write(
        &self,
        batch: &JournalBatch,
        tx: &mut sqlx::Transaction<'_, Postgres>,
    ) -> Result<(), CommitFailure> {
        let known = |error: sqlx::Error| CommitFailure::Known(error.to_string());
        let intent = &batch.intent;
        // The no-op update returns the row that owns the id, ours or not.
        let owner = sqlx::query_scalar::<_, Uuid>(
            r#"
            INSERT INTO order_intents (
                intent_id, tenant_id, venue_id, account_scope, client_order_id, position_id,
                kind, cancels_order_id, params, created_at
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
            ON CONFLICT (venue_id, account_scope, client_order_id)
            DO UPDATE SET client_order_id = order_intents.client_order_id
            RETURNING intent_id
            "#,
        )
        .bind(intent.intent_id)
        .bind(self.tenant_id)
        .bind(&intent.venue_id)
        .bind(&intent.account_scope)
        .bind(&intent.client_order_id)
        .bind(intent.position_id)
        .bind(intent.kind.as_str())
        .bind(&intent.cancels_order_id)
        .bind(&intent.params)
        .bind(intent.created_at)
        .fetch_one(&mut **tx)
        .await
        .map_err(known)?;
        if owner != intent.intent_id {
            return Err(CommitFailure::IdSpent);
        }

        if !batch.attempts.is_empty() {
            let ids: Vec<Uuid> = batch.attempts.iter().map(|a| a.attempt_id).collect();
            let created: Vec<_> = batch.attempts.iter().map(|a| a.created_at).collect();
            let rejected: Vec<_> = batch.attempts.iter().map(|a| a.rejected_at).collect();
            // Rule 1: `rejected` absorbs `started`, in whichever order they land.
            sqlx::query(
                r#"
                INSERT INTO submit_attempts (
                    attempt_id, venue_id, account_scope, client_order_id, state, created_at,
                    rejected_at
                )
                SELECT id, $2, $3, $4,
                       CASE WHEN rejected_at IS NULL THEN 'started' ELSE 'rejected' END,
                       created_at, rejected_at
                FROM UNNEST($1::uuid[], $5::timestamptz[], $6::timestamptz[])
                    AS incoming(id, created_at, rejected_at)
                ON CONFLICT (attempt_id) DO UPDATE SET
                    state = CASE WHEN submit_attempts.state = 'rejected'
                                   OR EXCLUDED.state = 'rejected'
                                 THEN 'rejected' ELSE 'started' END,
                    rejected_at = COALESCE(submit_attempts.rejected_at, EXCLUDED.rejected_at)
                "#,
            )
            .bind(&ids)
            .bind(&intent.venue_id)
            .bind(&intent.account_scope)
            .bind(&intent.client_order_id)
            .bind(&created)
            .bind(&rejected)
            .execute(&mut **tx)
            .await
            .map_err(known)?;
        }

        if let Some(outcome) = &batch.outcome {
            sqlx::query(
                r#"
                UPDATE order_intents SET
                    venue_outcome = COALESCE(venue_outcome, $2),
                    cancel_class = COALESCE(cancel_class, $3),
                    not_sent = not_sent OR $4,
                    venue_evidence = COALESCE(venue_evidence, $5),
                    uncertain_at = COALESCE(uncertain_at, $6)
                WHERE intent_id = $1
                "#,
            )
            .bind(intent.intent_id)
            .bind(outcome.venue_outcome.map(|o| o.as_str()))
            .bind(outcome.cancel_class)
            .bind(outcome.not_sent)
            .bind(&outcome.venue_evidence)
            .bind(outcome.uncertain_at)
            .execute(&mut **tx)
            .await
            .map_err(known)?;
        }
        Ok(())
    }
}

#[async_trait]
impl JournalBackend for PgJournalBackend {
    async fn commit(&self, batch: &JournalBatch) -> Result<(), CommitFailure> {
        let known = |e: sqlx::Error| CommitFailure::Known(e.to_string());
        let connection = self.pool.acquire().await.map_err(known)?;
        let mut write = RetiringConnection { connection, finished: false };
        let mut tx = write.connection.begin().await.map_err(known)?;
        self.write(batch, &mut tx).await?;
        // Sent and not answered: the row may exist.
        tx.commit().await.map_err(|e| CommitFailure::Uncertain(e.to_string()))?;
        write.finished = true;
        Ok(())
    }

    async fn check_ready(&self) -> Result<(), String> {
        match tokio::time::timeout(self.ready_wait, self.probe_until_ready()).await {
            Ok(result) => result,
            Err(_) => Err(format!("no answer within {} ms", self.ready_wait.as_millis())),
        }
    }

    async fn attempt_states(
        &self,
        venue_id: &str,
        account_scope: &str,
        client_order_id: &str,
    ) -> Result<Vec<bool>, String> {
        let connection = self.pool.acquire().await.map_err(|e| e.to_string())?;
        let mut read = RetiringConnection { connection, finished: false };
        let states = sqlx::query_scalar::<_, bool>(
            r#"
            SELECT state = 'rejected' FROM submit_attempts
            WHERE venue_id = $1 AND account_scope = $2 AND client_order_id = $3
            "#,
        )
        .bind(venue_id)
        .bind(account_scope)
        .bind(client_order_id)
        .fetch_all(&mut *read.connection)
        .await
        .map_err(|e| e.to_string())?;
        read.finished = true;
        Ok(states)
    }
}
