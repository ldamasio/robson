//! Balance-account ledger persistence (ADR-0056 step 8e). No production caller
//! selects this store yet. Each call uses the daemon pool, a two-second outer
//! bound and connection retirement on any unfinished work.
use std::{
    future::Future,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    time::Duration,
};

use chrono::{DateTime, Utc};
use robson_domain::{MonthlyBudgetModel, PositionId};
use robson_exec::{venue::VenueId, venue_ledger::LedgerError};
use sqlx::{Connection, PgPool, Postgres, Row, Transaction};
use uuid::Uuid;

use crate::order_journal_store::RetiringConnection;

const WRITE_BOUND: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountingRow {
    pub tenant_id: Uuid,
    pub venue_id: VenueId,
    pub version: i64,
    pub budget_model: MonthlyBudgetModel,
    pub created_at: DateTime<Utc>,
}

pub struct PgVenueLedger {
    pool: PgPool,
    tenant_id: Uuid,
    statements: AtomicU64,
}

impl PgVenueLedger {
    pub fn new(daemon_pool: &PgPool, tenant_id: Uuid) -> Self {
        Self {
            pool: daemon_pool.clone(),
            tenant_id,
            statements: AtomicU64::new(0),
        }
    }

    pub fn statements_total(&self) -> u64 {
        self.statements.load(Ordering::Relaxed)
    }

    fn statement(&self) {
        self.statements.fetch_add(1, Ordering::Relaxed);
    }

    async fn timeouts(&self, tx: &mut Transaction<'_, Postgres>) -> Result<(), LedgerError> {
        self.statement();
        sqlx::query("SELECT set_config('lock_timeout', '300ms', true), set_config('statement_timeout', '1s', true)")
            .execute(&mut **tx).await.map_err(backend)?;
        Ok(())
    }

    async fn bounded<T>(
        &self,
        work: impl Future<Output = Result<T, LedgerError>>,
        committing: &AtomicBool,
    ) -> Result<T, LedgerError> {
        tokio::time::timeout(WRITE_BOUND, work).await.map_err(|_| {
            let reason = "venue ledger exceeded 2000 ms".to_string();
            if committing.load(Ordering::Relaxed) {
                LedgerError::Uncertain(reason)
            } else {
                LedgerError::Backend(reason)
            }
        })?
    }

    /// Cutover preparation only (8m): an existing model is returned unchanged,
    /// so the caller can detect a mismatch. Existence is not certification.
    pub async fn ensure_accounting_row(
        &self,
        venue: &VenueId,
        model: MonthlyBudgetModel,
    ) -> Result<AccountingRow, LedgerError> {
        let committing = AtomicBool::new(false);
        self.bounded(async {
            let connection = self.pool.acquire().await.map_err(backend)?;
            let mut write = RetiringConnection { connection, finished: false };
            self.statement();
            let mut tx = write.connection.begin().await.map_err(backend)?;
            self.timeouts(&mut tx).await?;
            self.statement();
            sqlx::query("INSERT INTO accounting_state (tenant_id, venue_id, budget_model) VALUES ($1,$2,$3) ON CONFLICT DO NOTHING")
                .bind(self.tenant_id).bind(venue.as_str()).bind(model.as_str()).execute(&mut *tx).await.map_err(backend)?;
            self.statement();
            let row = sqlx::query("SELECT version, budget_model, created_at FROM accounting_state WHERE tenant_id = $1 AND venue_id = $2")
                .bind(self.tenant_id).bind(venue.as_str()).fetch_one(&mut *tx).await.map_err(backend)?;
            let stored_model = match row.get::<&str, _>("budget_model") {
                "hwm_v1" => MonthlyBudgetModel::HwmV1,
                "net_from_start_non_expanding_v1" => MonthlyBudgetModel::NetFromStartNonExpandingV1,
                other => return Err(LedgerError::Invariant(format!("unknown accounting budget model {other}"))),
            };
            let stored = AccountingRow { tenant_id: self.tenant_id, venue_id: venue.clone(), version: row.get("version"),
                budget_model: stored_model, created_at: row.get("created_at") };
            self.statement();
            committing.store(true, Ordering::Relaxed);
            tx.commit().await.map_err(commit_error)?;
            write.finished = true;
            Ok(stored)
        }, &committing).await
    }

    /// A route's first timestamp is retained on replay. Reusing its position
    /// for another tenant, venue or scope is a conflict, never an overwrite.
    pub async fn insert_route(
        &self,
        position_id: PositionId,
        venue: &VenueId,
        scope: &str,
        at: DateTime<Utc>,
    ) -> Result<(), LedgerError> {
        let committing = AtomicBool::new(false);
        self.bounded(async {
            let connection = self.pool.acquire().await.map_err(backend)?;
            let mut write = RetiringConnection { connection, finished: false };
            self.statement();
            let mut tx = write.connection.begin().await.map_err(backend)?;
            self.timeouts(&mut tx).await?;
            self.statement();
            sqlx::query("INSERT INTO leg_routes (position_id, tenant_id, venue_id, account_scope, routed_at) VALUES ($1,$2,$3,$4,$5) ON CONFLICT DO NOTHING")
                .bind(position_id).bind(self.tenant_id).bind(venue.as_str()).bind(scope).bind(at)
                .execute(&mut *tx).await.map_err(backend)?;
            self.statement();
            let row = sqlx::query("SELECT tenant_id, venue_id, account_scope FROM leg_routes WHERE position_id = $1")
                .bind(position_id).fetch_one(&mut *tx).await.map_err(backend)?;
            if row.get::<Uuid, _>("tenant_id") != self.tenant_id || row.get::<&str, _>("venue_id") != venue.as_str()
                || row.get::<&str, _>("account_scope") != scope {
                return Err(LedgerError::RouteConflict);
            }
            self.statement();
            committing.store(true, Ordering::Relaxed);
            tx.commit().await.map_err(commit_error)?;
            write.finished = true;
            Ok(())
        }, &committing).await
    }
}

fn backend(error: sqlx::Error) -> LedgerError {
    if let sqlx::Error::Database(database) = &error {
        if database.code().as_deref() == Some("23514") {
            return LedgerError::Invariant(database.message().to_string());
        }
    }
    LedgerError::Backend(error.to_string())
}

/// A server ErrorResponse to COMMIT proves rollback; a transport/protocol
/// failure supplies no answer and cannot establish whether it landed.
fn commit_error(error: sqlx::Error) -> LedgerError {
    if matches!(error, sqlx::Error::Database(_)) {
        LedgerError::Backend(error.to_string())
    } else {
        LedgerError::Uncertain(error.to_string())
    }
}
