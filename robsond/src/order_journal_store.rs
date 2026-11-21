//! PostgreSQL side of the order journal (ADR-0056 step 7, PR J), with the
//! fence PR's incidents and release.
//!
//! Every write is one explicit transaction ending in `Transaction::commit`,
//! which goes out as a simple-query `COMMIT`. Nothing here commits implicitly.

use std::time::{Duration, Instant};

#[path = "order_journal_balance.rs"]
mod balance;
#[allow(
    dead_code,
    reason = "private clocked Spot route awaiting the native capture and readiness owner"
)]
#[path = "order_journal_clocked.rs"]
mod clocked;
#[cfg(test)]
#[path = "order_journal_clocked_pg_tests.rs"]
mod clocked_pg_tests;
#[cfg(test)]
#[path = "order_journal_projected_pg_tests.rs"]
mod projected_pg_tests;
#[path = "order_journal_readiness.rs"]
mod readiness;
#[allow(
    dead_code,
    reason = "local atomic storage primitive awaiting bounded Spot owner"
)]
#[path = "order_journal_transition.rs"]
mod transition;

use async_trait::async_trait;
use chrono::Utc;
use robson_exec::{
    gate::Incident,
    order_journal::{
        CommitFailure, IntentKind, JournalBackend, JournalBatch, ReleaseCounts, ReleaseRecord,
        StreamSuspensionRow, COMMIT_BOUND, RELEASE_BOUND,
    },
    venue::VenueId,
};
use rust_decimal::Decimal;
use serde_json::Value;
use sqlx::{pool::PoolConnection, postgres::PgPoolOptions, Connection, PgPool, Postgres, Row};
use tracing::Instrument as _;
use uuid::Uuid;

use crate::spot_order_set_store::{
    maintain_cancel_outcome, maintain_entry_admission, maintain_unbound_journal,
    spot_startup::SpotReady, JournalGeneration, JournalPrior,
};

/// The journal's own pool, so order writes never queue behind the daemon's.
const JOURNAL_POOL_CONNECTIONS: u32 = 2;
/// Inside the 500 ms commit bound, so a pool with no connection to give ends
/// as a known failure: no statement was sent.
const JOURNAL_ACQUIRE_BOUND: Duration = Duration::from_millis(250);

/// A connection under an outer timeout, closed instead of returned to the pool
/// unless its work finished. A dropped future leaves a statement whose replies
/// may never arrive; returned to the pool it would wait on a ping, holding its
/// slot with no deadline, and two of those are the whole pool.
pub(crate) struct RetiringConnection {
    pub(crate) connection: PoolConnection<Postgres>,
    pub(crate) finished: bool,
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
    /// Bound incidents, suspensions and releases as a balance account's.
    balance_account: bool,
    /// Private opt-in journal maintenance; `None` keeps the legacy statements.
    order_set_projection: Option<JournalGeneration>,
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
            balance_account: false,
            order_set_projection: None,
        }
    }

    /// Run this backend's incident, suspension and release transactions as a
    /// balance account's (ADR-0056 step 8): server bounds first, raw
    /// retirement. Set it with the journal's own `with_balance_account`.
    pub fn with_balance_account(mut self, on: bool) -> Self {
        self.balance_account = on;
        self
    }

    /// The journal's own pool, so the daemon wiring's proof can read its
    /// session settings (ADR-0056 step 8).
    #[doc(hidden)]
    pub fn session_pool(&self) -> &PgPool {
        &self.pool
    }

    /// The supervised Spot backend: orders and a release both on the admitted
    /// writer, bound to its installed generation. The installer's pool never
    /// reaches it.
    #[allow(dead_code, reason = "no production supervisor yet")]
    pub(crate) fn supervised(ready: &SpotReady) -> Self {
        Self {
            pool: ready.writer().clone(),
            daemon_pool: ready.writer().clone(),
            tenant_id: ready.generation().tenant_id,
            ready_wait: JOURNAL_READY_WAIT,
            // Only the Spot balance account is supervised: its incidents,
            // suspensions and releases run under the server bounds.
            balance_account: true,
            order_set_projection: Some(ready.generation().clone()),
        }
    }

    /// Change how long the startup check may take.
    pub fn with_ready_wait(mut self, ready_wait: Duration) -> Self {
        self.ready_wait = ready_wait;
        self
    }

    /// Opt one exact owner into journal maintenance. No daemon, operator or
    /// release path calls this.
    #[allow(dead_code, reason = "private isolated integration only")]
    pub(crate) fn with_order_set_projection(mut self, generation: JournalGeneration) -> Self {
        self.order_set_projection = Some(generation);
        self
    }

    /// The opted-in binding, refused before any SQL unless the batch is one
    /// supported shape in the bound tenant and account scope, accounting-first:
    /// a Spot Protection/Exit intent or the outcome of one previously persisted
    /// grouped Cancel with no attempts, both without admission, or an Entry
    /// with its own admission and no outcome.
    fn projection_for(
        &self,
        batch: &JournalBatch,
    ) -> Result<Option<&JournalGeneration>, CommitFailure> {
        let Some(generation) = &self.order_set_projection else {
            return Ok(None);
        };
        let refuse =
            |why: &str| Err(CommitFailure::Known(format!("order-set journal refused: {why}")));
        let intent = &batch.intent;
        if !generation.is_valid()
            || generation.tenant_id != self.tenant_id
            || intent.intent_id.is_nil()
        {
            return refuse("invalid binding");
        }
        if intent.venue_id != VenueId::BINANCE_SPOT {
            return refuse("not a Spot intent");
        }
        if intent.account_scope != generation.account_scope {
            return refuse("another account scope");
        }
        let Some(balance) = &batch.balance else {
            return refuse("not an accounting-first batch");
        };
        if balance.admission.is_some() && intent.kind != IntentKind::Entry {
            return refuse("admission is not supported");
        }
        match intent.kind {
            // A linked Entry outcome can change a CANCEL mask through
            // J.not_sent: separate maintenance coverage, refused here.
            IntentKind::Entry => {
                let Some(admission) = &balance.admission else {
                    return refuse("entry without an admission");
                };
                if batch.outcome.is_some() {
                    return refuse("entry outcome is not supported");
                }
                if admission.position_id.is_nil()
                    || intent.position_id != Some(admission.position_id)
                    || admission.symbol.trim().is_empty()
                {
                    return refuse("entry admission is not the intent's leg");
                }
            },
            IntentKind::Protection | IntentKind::Exit => {},
            // Never a first cancel: that has its own covered transaction.
            IntentKind::Cancel => {
                if batch.outcome.is_none() || !batch.attempts.is_empty() {
                    return refuse("cancel without exactly an outcome");
                }
                if intent.position_id.is_none_or(|position| position.is_nil()) {
                    return refuse("cancel without a position");
                }
                let canonical = intent.cancels_order_id.as_deref().is_some_and(|target| {
                    target
                        .parse::<robson_exec::venue_ledger::VenueIntId>()
                        .is_ok_and(|parsed| parsed.to_string() == target)
                });
                if !canonical {
                    return refuse("cancel without a canonical target");
                }
            },
            _ => return refuse("unsupported intent kind"),
        }
        Ok(Some(generation))
    }

    /// The one maintenance statement after the original source effects, in
    /// the same transaction and under the original deadline.
    #[allow(clippy::too_many_arguments)]
    async fn maintain_order_set(
        &self,
        generation: &JournalGeneration,
        batch: &JournalBatch,
        version: i64,
        prior: JournalPrior,
        tx: &mut sqlx::Transaction<'_, Postgres>,
        started: Instant,
        bound: Duration,
    ) -> Result<(), CommitFailure> {
        let maintained = match (&batch.intent.kind, prior) {
            (
                IntentKind::Protection | IntentKind::Exit,
                JournalPrior::Sell { potential, conflict },
            ) => {
                maintain_unbound_journal(
                    tx,
                    generation,
                    version,
                    &batch.intent,
                    potential,
                    conflict,
                )
                .await
            },
            (IntentKind::Cancel, JournalPrior::Cancel { pending, conflict }) => {
                maintain_cancel_outcome(tx, generation, version, &batch.intent, pending, conflict)
                    .await
            },
            (IntentKind::Entry, JournalPrior::Entry { birth }) => {
                let Some(admission) = batch.balance.as_ref().and_then(|b| b.admission.as_ref())
                else {
                    return Err(CommitFailure::Known(
                        "order-set journal refused: entry without an admission".to_owned(),
                    ));
                };
                maintain_entry_admission(
                    tx,
                    generation,
                    version,
                    &batch.intent,
                    &admission.symbol,
                    birth,
                )
                .await
            },
            _ => {
                return Err(CommitFailure::Known(
                    "order-set journal refused: unsupported route".to_owned(),
                ))
            },
        }
        .map_err(|e| CommitFailure::Known(e.to_string()))?;
        if maintained != Some(version) {
            return Err(CommitFailure::Known(
                "order-set journal refused: stale generation or nonmatching source".to_owned(),
            ));
        }
        if started.elapsed() >= bound {
            return Err(CommitFailure::Known(
                "order-set journal deadline before COMMIT".to_owned(),
            ));
        }
        Ok(())
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
                .instrument(tracing::info_span!(target: "robson_operation", "operation_io", io_kind = 1_u64))
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
    ) -> Result<JournalPrior, CommitFailure> {
        self.write_routed(batch, tx, None).await.map(|(_, prior)| prior)
    }

    /// The journal's statements. A prepared clocked route writes its clock row
    /// with the one migration-38 helper instead of the five-field insert.
    async fn write_routed(
        &self,
        batch: &JournalBatch,
        tx: &mut sqlx::Transaction<'_, Postgres>,
        clock: Option<&clocked::Prepared>,
    ) -> Result<(Vec<clocked::ClockReceipt>, JournalPrior), CommitFailure> {
        let known = |error: sqlx::Error| CommitFailure::Known(error.to_string());
        let intent = &batch.intent;
        let projected = self.order_set_projection.is_some();
        let projected_cancel = projected && intent.kind == IntentKind::Cancel;
        let projected_entry = projected && intent.kind == IntentKind::Entry;
        // The no-op update returns the row that owns the id, ours or not.
        let ordinary_intent = r#"
            INSERT INTO order_intents (
                intent_id, tenant_id, venue_id, account_scope, client_order_id, position_id,
                kind, cancels_order_id, params, created_at
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
            ON CONFLICT (venue_id, account_scope, client_order_id)
            DO UPDATE SET client_order_id = order_intents.client_order_id
            RETURNING intent_id
            "#;
        let projected_intent;
        let intent_sql = if projected_cancel {
            // Exact prior J identity and contribution; no row is a fresh intent.
            let before = include_str!("spot_order_set_store/journal_cancel_before_write.sql");
            projected_intent = format!(
                "{before}, written AS ({ordinary_intent})
                 SELECT written.intent_id, prior_cancel.intent_id IS NOT NULL AS prior_found,
                        coalesce(prior_cancel.pending, false) AS prior_pending,
                        coalesce(prior_cancel.conflict, false) AS prior_conflict
                 FROM written LEFT JOIN prior_cancel ON true"
            );
            projected_intent.as_str()
        } else if projected_entry {
            // The leg, admission and native edge before this write and the
            // admission below.
            let before = include_str!("spot_order_set_store/journal_entry_before_write.sql");
            projected_intent = format!(
                "{before}, written AS ({ordinary_intent})
                 SELECT written.intent_id, prior_entry.prior_found, prior_entry.prior_leg,
                        prior_entry.prior_admission, prior_entry.prior_admitted,
                        prior_entry.prior_native
                 FROM written CROSS JOIN prior_entry"
            );
            projected_intent.as_str()
        } else if projected {
            let before = include_str!("spot_order_set_store/journal_before_write.sql");
            projected_intent = format!(
                "{before}, written AS ({ordinary_intent})
                 SELECT written.intent_id, prior_member.intent_id IS NOT NULL AS prior_potential,
                        coalesce(prior_member.conflict, false) AS prior_conflict
                 FROM written LEFT JOIN prior_member ON true"
            );
            projected_intent.as_str()
        } else {
            ordinary_intent
        };
        let written = sqlx::query(intent_sql)
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
            .instrument(
                tracing::info_span!(target: "robson_operation", "operation_io", io_kind = 1_u64),
            )
            .await
            .map_err(known)?;
        if written.try_get::<Uuid, _>("intent_id").map_err(known)? != intent.intent_id {
            return Err(CommitFailure::IdSpent);
        }
        let prior = if projected_cancel {
            if !written.try_get::<bool, _>("prior_found").map_err(known)? {
                return Err(CommitFailure::Known(
                    "order-set journal refused: no persisted cancel intent".to_owned(),
                ));
            }
            JournalPrior::Cancel {
                pending: written.try_get("prior_pending").map_err(known)?,
                conflict: written.try_get("prior_conflict").map_err(known)?,
            }
        } else if projected_entry {
            let found: bool = written.try_get("prior_found").map_err(known)?;
            let leg: bool = written.try_get("prior_leg").map_err(known)?;
            let admission: Option<Uuid> = written.try_get("prior_admission").map_err(known)?;
            let admitted: bool = written.try_get("prior_admitted").map_err(known)?;
            let native: bool = written.try_get("prior_native").map_err(known)?;
            // A fresh birth, or the same current admission replayed. Any
            // supersession, foreign leg or native edge is held for repair.
            let birth = !found && !leg && !admitted;
            let replay = found && admitted && admission == Some(intent.intent_id);
            if native || !(birth || replay) {
                return Err(CommitFailure::Known(
                    "order-set journal refused: entry is neither a fresh birth nor its replay"
                        .to_owned(),
                ));
            }
            JournalPrior::Entry { birth }
        } else if projected {
            JournalPrior::Sell {
                potential: written.try_get("prior_potential").map_err(known)?,
                conflict: written.try_get("prior_conflict").map_err(known)?,
            }
        } else {
            JournalPrior::Sell { potential: false, conflict: false }
        };

        if !batch.attempts.is_empty() {
            let ids: Vec<Uuid> = batch.attempts.iter().map(|a| a.attempt_id).collect();
            let created: Vec<_> = batch.attempts.iter().map(|a| a.created_at).collect();
            let rejected: Vec<_> = batch.attempts.iter().map(|a| a.rejected_at).collect();
            // Rule 1: `rejected` absorbs `started`, in whichever order they land.
            let ordinary_attempts = r#"
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
                "#;
            let projected_attempts;
            let attempt_sql = if projected {
                projected_attempts = format!(
                    "{ordinary_attempts}
                     WHERE (submit_attempts.venue_id, submit_attempts.account_scope,
                            submit_attempts.client_order_id)
                         = (excluded.venue_id, excluded.account_scope, excluded.client_order_id)"
                );
                projected_attempts.as_str()
            } else {
                ordinary_attempts
            };
            let attempts = sqlx::query(attempt_sql)
            .bind(&ids)
            .bind(&intent.venue_id)
            .bind(&intent.account_scope)
            .bind(&intent.client_order_id)
            .bind(&created)
            .bind(&rejected)
            .execute(&mut **tx)
            .instrument(
                tracing::info_span!(target: "robson_operation", "operation_io", io_kind = 1_u64),
            )
            .await
            .map_err(|error| {
                // Only the spot one-attempt constraint proves that this id
                // was spent. Other unique violations keep their known-error
                // classification; USD-M never matches this partial index.
                if let sqlx::Error::Database(database) = &error {
                    if database.code().as_deref() == Some("23505")
                        && database.constraint()
                            == Some("idx_submit_attempts_one_per_spot_client_id")
                    {
                        return CommitFailure::IdSpent;
                    }
                }
                known(error)
            })?;
            if projected && attempts.rows_affected() != ids.len() as u64 {
                return Err(CommitFailure::Known(
                    "order-set journal refused: submit attempt belongs to another intent"
                        .to_owned(),
                ));
            }
        }

        if let Some(outcome) = &batch.outcome {
            let affected = sqlx::query(
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
            .instrument(
                tracing::info_span!(target: "robson_operation", "operation_io", io_kind = 1_u64),
            )
            .await
            .map_err(known)?;
            if projected_cancel && affected.rows_affected() != 1 {
                return Err(CommitFailure::Known(
                    "order-set journal refused: cancel outcome did not affect one row".to_owned(),
                ));
            }
        }
        let receipts = if batch.balance.is_some() {
            Box::pin(self.write_balance(batch, tx, clock)).await?
        } else {
            Vec::new()
        };
        Ok((receipts, prior))
    }
}

#[async_trait]
impl JournalBackend for PgJournalBackend {
    async fn commit(&self, batch: &JournalBatch) -> Result<(), CommitFailure> {
        let started = Instant::now();
        let projection = self.projection_for(batch)?;
        if let Some(balance) = &batch.balance {
            return Box::pin(self.commit_balance(batch, started, balance.commit_bound, projection))
                .await;
        }
        if batch.intent.venue_id == VenueId::BINANCE_SPOT {
            return Err(CommitFailure::Known(
                "Spot journal mutations require accounting-first storage".to_owned(),
            ));
        }
        let known = |e: sqlx::Error| CommitFailure::Known(e.to_string());
        let connection = self.pool.acquire().await.map_err(known)?;
        let mut write = RetiringConnection { connection, finished: false };
        let mut tx = write
            .connection
            .begin()
            .instrument(
                tracing::info_span!(target: "robson_operation", "operation_io", io_kind = 2_u64),
            )
            .await
            .map_err(known)?;
        let mut rollback_observation = RollbackObservation::new();
        self.write(batch, &mut tx).await?;
        // Sent and not answered: the row may exist.
        tx.commit()
            .instrument(
                tracing::info_span!(target: "robson_operation", "operation_io", io_kind = 3_u64),
            )
            .await
            .map_err(|e| CommitFailure::Uncertain(e.to_string()))?;
        rollback_observation.disarm();
        write.finished = true;
        Ok(())
    }

    async fn record_suspension(
        &self,
        suspension: &StreamSuspensionRow,
    ) -> Result<(), CommitFailure> {
        let started = Instant::now();
        let known = |e: sqlx::Error| CommitFailure::Known(e.to_string());
        let connection = self.pool.acquire().await.map_err(known)?;
        let mut write = self.hold(connection);
        let mut tx = write
            .connection()
            .begin()
            .instrument(
                tracing::info_span!(target: "robson_operation", "operation_io", io_kind = 2_u64),
            )
            .await
            .map_err(known)?;
        let mut rollback_observation = self.rollback_observation();
        self.bound_auxiliary(&mut tx, COMMIT_BOUND, started)
            .await
            .map_err(CommitFailure::Known)?;
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
        .instrument(
            tracing::info_span!(target: "robson_operation", "operation_io", io_kind = 1_u64),
        )
        .await
        .map_err(known)?;
        tx.commit()
            .instrument(
                tracing::info_span!(target: "robson_operation", "operation_io", io_kind = 3_u64),
            )
            .await
            .map_err(|e| CommitFailure::Uncertain(e.to_string()))?;
        rollback_observation.disarm();
        write.finish();
        Ok(())
    }

    async fn upsert_incident(&self, incident: &Incident) -> Result<(), CommitFailure> {
        let started = Instant::now();
        let known = |e: sqlx::Error| CommitFailure::Known(e.to_string());
        let connection = self.pool.acquire().await.map_err(known)?;
        let mut write = self.hold(connection);
        let mut tx = write
            .connection()
            .begin()
            .instrument(
                tracing::info_span!(target: "robson_operation", "operation_io", io_kind = 2_u64),
            )
            .await
            .map_err(known)?;
        let mut rollback_observation = self.rollback_observation();
        self.bound_auxiliary(&mut tx, COMMIT_BOUND, started)
            .await
            .map_err(CommitFailure::Known)?;
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
        .instrument(
            tracing::info_span!(target: "robson_operation", "operation_io", io_kind = 1_u64),
        )
        .await
        .map_err(known)?;
        tx.commit()
            .instrument(
                tracing::info_span!(target: "robson_operation", "operation_io", io_kind = 3_u64),
            )
            .await
            .map_err(|e| CommitFailure::Uncertain(e.to_string()))?;
        rollback_observation.disarm();
        write.finish();
        Ok(())
    }

    /// One transaction of five statements, `BEGIN` and `COMMIT` included
    /// (design §2.8): tombstone, release, decision. The decision comes last,
    /// with what the two before it did; the incidents' reference to it is
    /// checked at the commit. A balance account's has six: its bounds first.
    async fn release(&self, release: &ReleaseRecord) -> Result<ReleaseCounts, String> {
        let started = Instant::now();
        let error = |e: sqlx::Error| e.to_string();
        // A connection whose release did not finish (the caller's bound
        // dropped it, or an error cut it) is closed, not handed back: returned
        // to the pool it would wait on a ping with no deadline, holding a
        // daemon-pool slot (J's rule for the journal's own pool).
        let connection = self.daemon_pool.acquire().await.map_err(error)?;
        let mut held = self.hold(connection);
        let mut tx = held
            .connection()
            .begin()
            .instrument(
                tracing::info_span!(target: "robson_operation", "operation_io", io_kind = 2_u64),
            )
            .await
            .map_err(error)?;
        let mut rollback_observation = self.rollback_observation();
        self.bound_auxiliary(&mut tx, RELEASE_BOUND, started).await?;
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
        .instrument(
            tracing::info_span!(target: "robson_operation", "operation_io", io_kind = 1_u64),
        )
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
        .instrument(
            tracing::info_span!(target: "robson_operation", "operation_io", io_kind = 1_u64),
        )
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
        .instrument(
            tracing::info_span!(target: "robson_operation", "operation_io", io_kind = 1_u64),
        )
        .await
        .map_err(error)?;
        tx.commit()
            .instrument(
                tracing::info_span!(target: "robson_operation", "operation_io", io_kind = 3_u64),
            )
            .await
            .map_err(error)?;
        rollback_observation.disarm();
        held.finish();
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

    async fn check_balance_schema(&self, venue: &str, one_attempt: bool) -> Result<(), String> {
        self.probe_balance_schema(venue, one_attempt).await
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
        .instrument(
            tracing::info_span!(target: "robson_operation", "operation_io", io_kind = 1_u64),
        )
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
    let mut tx = pool
        .begin()
        .instrument(
            tracing::info_span!(target: "robson_operation", "operation_io", io_kind = 2_u64),
        )
        .await?;
    let mut rollback_observation = RollbackObservation::new();
    let row = sqlx::query_as::<_, (String, Option<String>, bool, bool, Value)>(
        r#"
        SELECT client_order_id, cancels_order_id, venue_outcome IS NOT NULL, not_sent, params
        FROM order_intents WHERE intent_id = $1 FOR UPDATE
        "#,
    )
    .bind(disposition.intent_id)
    .fetch_optional(&mut *tx)
    .instrument(tracing::info_span!(target: "robson_operation", "operation_io", io_kind = 1_u64))
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
    .instrument(tracing::info_span!(target: "robson_operation", "operation_io", io_kind = 1_u64))
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
    .instrument(tracing::info_span!(target: "robson_operation", "operation_io", io_kind = 1_u64))
    .await?;

    let boundary_b_blockers = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*)::BIGINT FROM order_intents_blocking_rollback",
    )
    .fetch_one(&mut *tx)
    .instrument(tracing::info_span!(target: "robson_operation", "operation_io", io_kind = 1_u64))
    .await?;
    tx.commit()
        .instrument(
            tracing::info_span!(target: "robson_operation", "operation_io", io_kind = 3_u64),
        )
        .await?;
    rollback_observation.disarm();
    Ok(DispositionReceipt { decision_id, boundary_b_blockers })
}

// The public SQLx Transaction Drop queues rollback while open. This only counts
// that application cleanup request; it never claims server acknowledgement.
struct RollbackObservation {
    origin: tracing::Span,
    armed: bool,
}
impl RollbackObservation {
    fn new() -> Self {
        Self {
            origin: tracing::Span::current(),
            armed: true,
        }
    }
    fn disarm(&mut self) {
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
