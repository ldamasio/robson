//! PostgreSQL side of the order journal (ADR-0056 step 7, PR J), with the
//! fence PR's incidents and release.
//!
//! Every write is one explicit transaction ending in `Transaction::commit`,
//! which goes out as a simple-query `COMMIT`. Nothing here commits implicitly.

use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use robson_exec::{
    gate::Incident,
    order_journal::{
        CommitFailure, JournalBackend, JournalBatch, ReleaseCounts, ReleaseRecord,
        StreamSuspensionRow,
    },
};
use rust_decimal::Decimal;
use serde_json::Value;
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
    /// The daemon's pool, for a release: an operator's transaction never
    /// takes one of the journal's two connections from an order.
    daemon_pool: PgPool,
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
            daemon_pool: daemon_pool.clone(),
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
                    "SELECT 1 FROM order_intents, submit_attempts, journal_decisions, \
                     stream_suspensions, recovery_incidents LIMIT 0",
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
                    uncertain_at = COALESCE(uncertain_at, $6),
                    uncertain_generation = COALESCE(uncertain_generation, $7)
                WHERE intent_id = $1
                "#,
            )
            .bind(intent.intent_id)
            .bind(outcome.venue_outcome.map(|o| o.as_str()))
            .bind(outcome.cancel_class)
            .bind(outcome.not_sent)
            .bind(&outcome.venue_evidence)
            .bind(outcome.uncertain_at)
            .bind(outcome.uncertain_generation)
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

    async fn record_suspension(
        &self,
        suspension: &StreamSuspensionRow,
    ) -> Result<(), CommitFailure> {
        let known = |e: sqlx::Error| CommitFailure::Known(e.to_string());
        let connection = self.pool.acquire().await.map_err(known)?;
        let mut write = RetiringConnection { connection, finished: false };
        let mut tx = write.connection.begin().await.map_err(known)?;
        // An open suspension of the stream, or this one replayed, stands.
        sqlx::query(
            r#"
            INSERT INTO stream_suspensions (
                suspension_id, tenant_id, stream_key, stream_kind, failed_seq, month, error,
                epoch_id, suspended_at
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
            ON CONFLICT DO NOTHING
            "#,
        )
        .bind(suspension.suspension_id)
        .bind(self.tenant_id)
        .bind(&suspension.stream_key)
        .bind(suspension.stream_kind)
        .bind(suspension.failed_seq)
        .bind(suspension.month)
        .bind(&suspension.error)
        .bind(suspension.epoch_id)
        .bind(suspension.suspended_at)
        .execute(&mut *tx)
        .await
        .map_err(known)?;
        tx.commit().await.map_err(|e| CommitFailure::Uncertain(e.to_string()))?;
        write.finished = true;
        Ok(())
    }

    async fn upsert_incident(&self, incident: &Incident) -> Result<(), CommitFailure> {
        let known = |e: sqlx::Error| CommitFailure::Known(e.to_string());
        let connection = self.pool.acquire().await.map_err(known)?;
        let mut write = RetiringConnection { connection, finished: false };
        let mut tx = write.connection.begin().await.map_err(known)?;
        // Every value only grows, so a replay or a reordering converges. A
        // released row does not conflict: this opens a new one.
        sqlx::query(
            r#"
            INSERT INTO recovery_incidents (
                incident_id, tenant_id, reason, first_at, last_at, count, first_generation,
                last_generation
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            ON CONFLICT (tenant_id, reason) WHERE released_at IS NULL DO UPDATE SET
                first_at = LEAST(recovery_incidents.first_at, EXCLUDED.first_at),
                last_at = GREATEST(recovery_incidents.last_at, EXCLUDED.last_at),
                count = GREATEST(recovery_incidents.count, EXCLUDED.count),
                first_generation =
                    LEAST(recovery_incidents.first_generation, EXCLUDED.first_generation),
                last_generation =
                    GREATEST(recovery_incidents.last_generation, EXCLUDED.last_generation)
            "#,
        )
        .bind(Uuid::now_v7())
        .bind(self.tenant_id)
        .bind(incident.reason.as_str())
        .bind(incident.first_at)
        .bind(incident.last_at)
        .bind(incident.count)
        .bind(incident.first_generation)
        .bind(incident.last_generation)
        .execute(&mut *tx)
        .await
        .map_err(known)?;
        tx.commit().await.map_err(|e| CommitFailure::Uncertain(e.to_string()))?;
        write.finished = true;
        Ok(())
    }

    /// One transaction of five statements, `BEGIN` and `COMMIT` included
    /// (design §2.8): tombstone, release, decision. The decision comes last,
    /// with what the two before it did; the incidents' reference to it is
    /// checked at the commit.
    async fn release(&self, release: &ReleaseRecord) -> Result<ReleaseCounts, String> {
        let error = |e: sqlx::Error| e.to_string();
        // A connection whose release did not finish (the caller's bound
        // dropped it, or an error cut it) is closed, not handed back: returned
        // to the pool it would wait on a ping with no deadline, holding a
        // daemon-pool slot (J's rule for the journal's own pool).
        let connection = self.daemon_pool.acquire().await.map_err(error)?;
        let mut held = RetiringConnection { connection, finished: false };
        let mut tx = held.connection.begin().await.map_err(error)?;
        // A row with no `uncertain_at` is never tombstoned, nor one with an
        // outcome, proven unsent, or already disposed of.
        let tombstoned = sqlx::query(
            r#"
            UPDATE order_intents SET
                disposition_kind = 'accepted_risk',
                disposition_treatment = $3,
                risk_accepted_at = $4,
                risk_accepted_by = $5,
                risk_accepted_auth = $6,
                risk_accepted_note = $7
            WHERE tenant_id = $1
              AND uncertain_generation IS NOT NULL
              AND uncertain_generation <= $2
              AND uncertain_at IS NOT NULL
              AND venue_outcome IS NULL
              AND NOT not_sent
              AND disposition_kind IS NULL
            "#,
        )
        .bind(self.tenant_id)
        .bind(release.generation)
        .bind(release.treatment)
        .bind(release.decided_at)
        .bind(&release.actor)
        .bind(release.auth_kind)
        .bind(&release.note)
        .execute(&mut *tx)
        .await
        .map_err(error)?
        .rows_affected();
        let released = sqlx::query(
            r#"
            UPDATE recovery_incidents SET released_at = $3, decision_id = $4
            WHERE tenant_id = $1 AND released_at IS NULL AND last_generation <= $2
            "#,
        )
        .bind(self.tenant_id)
        .bind(release.generation)
        .bind(release.decided_at)
        .bind(release.decision_id)
        .execute(&mut *tx)
        .await
        .map_err(error)?
        .rows_affected();
        let payload = serde_json::json!({
            "generation": release.generation,
            "intents_tombstoned": tombstoned,
            "incidents_released": released,
        });
        sqlx::query(
            r#"
            INSERT INTO journal_decisions (
                decision_id, kind, intent_id, actor, auth_kind, note, treatment, evidence,
                payload, decided_at
            ) VALUES ($1, 'release', NULL, $2, $3, $4, $5, NULL, $6, $7)
            "#,
        )
        .bind(release.decision_id)
        .bind(&release.actor)
        .bind(release.auth_kind)
        .bind(&release.note)
        .bind(release.treatment)
        .bind(&payload)
        .bind(release.decided_at)
        .execute(&mut *tx)
        .await
        .map_err(error)?;
        tx.commit().await.map_err(error)?;
        held.finished = true;
        Ok(ReleaseCounts {
            intents_tombstoned: tombstoned,
            incidents_released: released,
        })
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

/// What an operator recorded about an outstanding intent.
#[derive(Debug, Clone)]
pub struct Disposition {
    pub intent_id: Uuid,
    /// `Some` is terminal evidence naming `order_ref`; `None` is accepted risk.
    pub terminal_evidence: Option<TerminalEvidence>,
    pub treatment: &'static str,
    pub note: String,
    pub actor: String,
    pub auth_kind: &'static str,
}

#[derive(Debug, Clone)]
pub struct TerminalEvidence {
    pub order_ref: String,
    pub filled_qty: Decimal,
    pub document: Value,
}

#[derive(Debug, thiserror::Error)]
pub enum DispositionError {
    #[error("no such intent")]
    NotFound,
    #[error("intent has a definite outcome or was never sent; there is nothing to dispose of")]
    NotOutstanding,
    #[error("the evidence names another order")]
    EvidenceForAnotherOrder,
    #[error("the evidence reports more than the order asked for")]
    ImpossibleQuantity,
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DispositionReceipt {
    pub decision_id: Uuid,
    /// Intents still blocking rollback boundary B.
    pub boundary_b_blockers: i64,
}

/// The quantity the intent asked for, when it recorded one.
fn ordered_quantity(params: &Value) -> Option<Decimal> {
    match params.get("quantity")? {
        Value::String(quantity) => quantity.parse().ok(),
        Value::Number(quantity) => quantity.to_string().parse().ok(),
        _ => None,
    }
}

/// Record a disposition in one transaction: the intent's disposition and an
/// append-only decision row. The latest disposition stands; the decisions keep
/// them all, so a retry after a lost acknowledgement adds a second decision and
/// changes nothing else. It writes no venue outcome and no event, and it does
/// not check the attached record against the venue: the record is what the
/// named operator attached.
pub async fn record_disposition(
    pool: &PgPool,
    disposition: &Disposition,
) -> Result<DispositionReceipt, DispositionError> {
    let mut tx = pool.begin().await?;
    let row = sqlx::query_as::<_, (String, Option<String>, bool, bool, Value)>(
        r#"
        SELECT client_order_id, cancels_order_id, venue_outcome IS NOT NULL, not_sent, params
        FROM order_intents WHERE intent_id = $1 FOR UPDATE
        "#,
    )
    .bind(disposition.intent_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((client_order_id, cancels_order_id, has_outcome, not_sent, params)) = row else {
        return Err(DispositionError::NotFound);
    };
    if has_outcome || not_sent {
        return Err(DispositionError::NotOutstanding);
    }
    if let Some(evidence) = &disposition.terminal_evidence {
        // A cancellation's own id is local and never reached the venue: its
        // evidence is about the order it cancelled.
        if evidence.order_ref != cancels_order_id.unwrap_or(client_order_id) {
            return Err(DispositionError::EvidenceForAnotherOrder);
        }
        if ordered_quantity(&params).is_some_and(|ordered| evidence.filled_qty > ordered) {
            return Err(DispositionError::ImpossibleQuantity);
        }
    }

    let now = Utc::now();
    let (kind, document, accepted_at, accepted_by, accepted_auth, accepted_note) =
        match &disposition.terminal_evidence {
            Some(evidence) => {
                ("terminal_evidence", Some(&evidence.document), None, None, None, None)
            },
            None => (
                "accepted_risk",
                None,
                Some(now),
                Some(disposition.actor.as_str()),
                Some(disposition.auth_kind),
                Some(disposition.note.as_str()),
            ),
        };
    sqlx::query(
        r#"
        UPDATE order_intents SET
            disposition_kind = $2,
            disposition_treatment = $3,
            disposition_evidence = $4,
            risk_accepted_at = $5,
            risk_accepted_by = $6,
            risk_accepted_auth = $7,
            risk_accepted_note = $8
        WHERE intent_id = $1
        "#,
    )
    .bind(disposition.intent_id)
    .bind(kind)
    .bind(disposition.treatment)
    .bind(document)
    .bind(accepted_at)
    .bind(accepted_by)
    .bind(accepted_auth)
    .bind(accepted_note)
    .execute(&mut *tx)
    .await?;

    let decision_id = Uuid::now_v7();
    let payload = serde_json::json!({ "disposition_kind": kind });
    sqlx::query(
        r#"
        INSERT INTO journal_decisions (
            decision_id, kind, intent_id, actor, auth_kind, note, treatment, evidence, payload,
            decided_at
        ) VALUES ($1, 'disposition', $2, $3, $4, $5, $6, $7, $8, $9)
        "#,
    )
    .bind(decision_id)
    .bind(disposition.intent_id)
    .bind(&disposition.actor)
    .bind(disposition.auth_kind)
    .bind(&disposition.note)
    .bind(disposition.treatment)
    .bind(document)
    .bind(&payload)
    .bind(now)
    .execute(&mut *tx)
    .await?;

    let boundary_b_blockers = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*)::BIGINT FROM order_intents_blocking_rollback",
    )
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(DispositionReceipt { decision_id, boundary_b_blockers })
}
