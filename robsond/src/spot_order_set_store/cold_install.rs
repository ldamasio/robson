//! Local fixture owner for one atomic Spot generation installation.
use sqlx::{ConnectOptions, Connection, PgPool};
use tokio::time::{timeout_at, Instant};
use uuid::Uuid;

use super::{
    cold_audit::{ColdAuditRefusal, ColdAuditScope},
    cold_owner::{read_anchor, ColdOwnerCut, OwnerRequest, PriorGeneration, RestoreIdentity},
    JournalGeneration,
};
use crate::ledger_connection::LedgerConnection;

#[path = "cold_install/cancel.rs"]
mod cancel;
#[path = "cold_install/leaves.rs"]
mod leaves;
#[path = "cold_install/sell.rs"]
mod sell;

const LEAF_LIMIT: usize = 640;
const LIMITS: &str = "SELECT set_config('lock_timeout','20ms',true),
    set_config('statement_timeout','40ms',true),
    set_config('idle_in_transaction_session_timeout','1s',true)";
const LOCK: &str = "SELECT version FROM public.accounting_state
    WHERE (tenant_id,venue_id)=($1,'binance:spot') FOR UPDATE";
const INSTALL: &str = include_str!("cold_install/publish.sql");

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FencePhase {
    /// The supervised startup's checks, before its epoch and its cut.
    BeforeEpoch,
    BeforeCapture,
    BeforeRead,
    BeforePublish,
    AfterReadback,
    BeforeSourceAudit,
    AfterSourceAudit,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct FenceReceipt {
    pub(crate) restore: RestoreIdentity,
    pub(crate) owner: Uuid,
}

/// The disposable physical coordinator answers with the exact current
/// supervised operation and owner. The database anchor alone grants nothing.
pub(crate) trait ClosedWriterFence {
    async fn check(
        &mut self,
        restore: RestoreIdentity,
        phase: FencePhase,
        deadline: Instant,
    ) -> Result<FenceReceipt, FixtureFenceError>;
}

#[derive(Debug)]
pub(crate) struct FixtureFenceError;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Stage {
    Fence,
    Acquire,
    Setup,
    Lock,
    Anchor,
    Leaves,
    Publish,
    Commit,
    Readback,
}

#[derive(Debug)]
pub(crate) enum InstallRefusal {
    FixtureFence(Stage),
    Deadline(Stage),
    Sell(sell::SellBuildRefusal),
    Cancel(cancel::CancelRebuildError),
    Leaves(leaves::LeafError),
    Database(Stage, sqlx::Error),
    Anchor(ColdAuditRefusal),
    AnchorChanged,
    Publish,
    CommitUncertain(sqlx::Error),
    ReadbackChanged,
}

#[derive(Debug)]
pub(crate) struct Installed {
    pub(crate) generation: JournalGeneration,
    pub(crate) accounting_version: i64,
    pub(crate) restore: RestoreIdentity,
}

pub(crate) async fn install(
    pool: &PgPool,
    cut: &ColdOwnerCut,
    fence: &mut impl ClosedWriterFence,
    deadline: Instant,
) -> Result<Installed, InstallRefusal> {
    let mut stage = Stage::Fence;
    let result = timeout_at(deadline, install_owned(pool, cut, fence, deadline, &mut stage)).await;
    result.unwrap_or_else(|_| Err(InstallRefusal::Deadline(stage)))
}

async fn install_owned(
    pool: &PgPool,
    cut: &ColdOwnerCut,
    fence: &mut impl ClosedWriterFence,
    deadline: Instant,
    stage: &mut Stage,
) -> Result<Installed, InstallRefusal> {
    let receipt = fence
        .check(cut.restore, FencePhase::BeforeRead, deadline)
        .await
        .map_err(|_| InstallRefusal::FixtureFence(*stage))?;
    if receipt.restore != cut.restore || receipt.owner.is_nil() {
        return Err(InstallRefusal::FixtureFence(*stage));
    }
    let a = &cut.anchor;
    let request = OwnerRequest {
        scope: ColdAuditScope {
            tenant_id: a.tenant_id,
            account_scope: &a.account_scope,
        },
        expected_uid: &a.uid,
        epoch: a.process_epoch,
        accounting_version: a.accounting_version,
        restore: cut.restore,
    };
    let sell = sell::rebuild_sell(
        a.tenant_id,
        &a.account_scope,
        cut.intents(),
        cut.attempts(),
        cut.orders(),
        cut.routes(),
        LEAF_LIMIT,
    )
    .map_err(InstallRefusal::Sell)?;
    let cancel = cancel::rebuild_cancel(
        a.tenant_id,
        &a.account_scope,
        cut.intents(),
        cut.orders(),
        cut.routes(),
        LEAF_LIMIT - sell.leaf_rows(),
    )
    .map_err(InstallRefusal::Cancel)?;
    let leaves = leaves::PreparedLeaves::new(&sell, &cancel).map_err(InstallRefusal::Leaves)?;
    let generation = JournalGeneration {
        tenant_id: a.tenant_id,
        account_scope: a.account_scope.clone(),
        generation_id: Uuid::now_v7(),
        process_epoch: a.process_epoch,
        source_audit_id: Uuid::now_v7(),
    };
    let scope = leaves::LeafScope {
        tenant_id: a.tenant_id,
        account_scope: &a.account_scope,
        generation_id: generation.generation_id,
    };

    *stage = Stage::Acquire;
    let pg = pool.acquire().await.map_err(|e| InstallRefusal::Database(*stage, e))?;
    let mut owned = LedgerConnection::new(pg);
    *stage = Stage::Setup;
    let mut tx = owned
        .connection()
        .begin()
        .await
        .map_err(|e| InstallRefusal::Database(*stage, e))?;
    sqlx::query(LIMITS)
        .execute(&mut *tx)
        .await
        .map_err(|e| InstallRefusal::Database(*stage, e))?;
    *stage = Stage::Lock;
    let locked = sqlx::query_scalar::<_, i64>(LOCK)
        .bind(a.tenant_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|e| InstallRefusal::Database(*stage, e))?;
    if locked != Some(a.accounting_version) {
        return Err(InstallRefusal::AnchorChanged);
    }
    *stage = Stage::Anchor;
    if read_anchor(&mut tx, &request).await.map_err(InstallRefusal::Anchor)? != *a {
        return Err(InstallRefusal::AnchorChanged);
    }
    *stage = Stage::Leaves;
    leaves.insert(&mut tx, &scope).await.map_err(InstallRefusal::Leaves)?;
    *stage = Stage::Publish;
    if fence
        .check(cut.restore, FencePhase::BeforePublish, deadline)
        .await
        .map_err(|_| InstallRefusal::FixtureFence(*stage))?
        != receipt
    {
        return Err(InstallRefusal::FixtureFence(*stage));
    }
    let command = INSTALL.replace("@OBSERVED_LEAVES_MATCH@", &leaves::namespace_equality_sql());
    let old = a.prior.as_ref();
    let updated = sqlx::query_scalar::<_, Uuid>(&command)
        .bind(a.tenant_id)
        .bind(&a.account_scope)
        .bind(generation.generation_id)
        .bind(leaves.counts())
        .bind(leaves.payload())
        .bind(&a.uid)
        .bind(generation.process_epoch)
        .bind(a.accounting_version)
        .bind(generation.source_audit_id)
        .bind(old.map(|g| g.generation_id))
        .bind(old.and_then(|g| g.process_epoch))
        .bind(old.and_then(|g| g.source_audit_id))
        .bind(old.and_then(|g| g.accounting_version))
        .bind(old.map(|g| g.position_hints_complete))
        .fetch_optional(&mut *tx)
        .await
        .map_err(|e| InstallRefusal::Database(*stage, e))?;
    if updated != Some(generation.generation_id) {
        return Err(InstallRefusal::Publish);
    }
    *stage = Stage::Commit;
    tx.commit().await.map_err(InstallRefusal::CommitUncertain)?;
    owned.finished = true;

    *stage = Stage::Readback;
    let mut independent = pool
        .connect_options()
        .connect()
        .await
        .map_err(|e| InstallRefusal::Database(*stage, e))?;
    let read = read_anchor(&mut independent, &request).await.map_err(InstallRefusal::Anchor)?;
    let expected = PriorGeneration {
        generation_id: generation.generation_id,
        process_epoch: Some(generation.process_epoch),
        source_audit_id: Some(generation.source_audit_id),
        accounting_version: Some(a.accounting_version),
        position_hints_complete: true,
    };
    if read.prior != Some(expected) {
        return Err(InstallRefusal::ReadbackChanged);
    }
    leaves.verify(&mut independent, &scope).await.map_err(InstallRefusal::Leaves)?;
    if fence
        .check(cut.restore, FencePhase::AfterReadback, deadline)
        .await
        .map_err(|_| InstallRefusal::FixtureFence(*stage))?
        != receipt
    {
        return Err(InstallRefusal::FixtureFence(*stage));
    }
    Ok(Installed {
        generation,
        accounting_version: a.accounting_version,
        restore: cut.restore,
    })
}
