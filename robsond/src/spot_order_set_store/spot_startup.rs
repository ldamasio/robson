//! Supervised Spot startup. One trusted in-process supervisor holds one
//! restored target closed from before the epoch through admission; each stage
//! owns the next, and only the last yields the writer the daemon serves on.
//! No environment switch, raw id, lock or installed marker grants anything.
//!
//! Package 4 compiles only [`SpotReady`] in production, and nothing there
//! constructs one: the supervisor, the epoch, the capture, the install, the
//! source audit and the admission that produce it run only in tests, like
//! the cold installer and the cold owner's capture they drive.
#[cfg(test)]
use std::{future::Future, time::Duration};

#[cfg(test)]
use robson_exec::venue::VenueId;
#[cfg(test)]
use sqlx::PgConnection;
use sqlx::PgPool;
#[cfg(test)]
use tokio::time::{timeout, timeout_at, Instant};
#[cfg(test)]
use uuid::Uuid;

use super::JournalGeneration;
#[cfg(test)]
use super::{
    cold_audit::{ColdAuditRefusal, ColdAuditScope},
    cold_install::{
        install, ClosedWriterFence, FencePhase, FenceReceipt, FixtureFenceError, InstallRefusal,
    },
    cold_owner::{capture, ColdOwnerCut, OwnerRequest, RestoreIdentity},
    observe_generation, ExpectedGeneration,
};
use crate::process_epoch::EpochStart;
#[cfg(test)]
use crate::{error::DaemonError, ledger_connection::LedgerConnection};

#[cfg(test)]
/// The installer's own session and cluster, read once before the epoch.
const INSTALLER: &str = "SELECT current_user::text, session_user::text, current_database()::text,
    inet_server_addr()::text, inet_server_port(), pg_postmaster_start_time()::text,
    c.system_identifier::text, current_setting('data_directory')
    FROM pg_control_system() c
    WHERE EXISTS (SELECT 1 FROM public.venue_accounts
                  WHERE (venue_id, account_scope)=('binance:spot',$2) AND tenant_id=$1)";
#[cfg(test)]
/// The admitted writer's session: what a restricted login can read.
const WRITER: &str = "SELECT current_user::text, session_user::text, current_database()::text,
    inet_server_addr()::text, inet_server_port(), pg_postmaster_start_time()::text";
#[cfg(test)]
/// A refused writer pool is closed within this, whatever is left of the
/// deadline.
const WRITER_CLOSE_BOUND: Duration = Duration::from_millis(250);

#[cfg(test)]
/// The restored target the supervisor verified and holds closed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RestoredTarget {
    pub(crate) restore: RestoreIdentity,
    pub(crate) database: String,
    pub(crate) system_identifier: String,
    pub(crate) data_directory: String,
    /// `pg_postmaster_start_time()::text` in a UTC session.
    pub(crate) postmaster_started: String,
}

#[cfg(test)]
/// What the supervisor is asked to admit, after install and readback.
pub(crate) struct Admission<'a> {
    pub(crate) generation: &'a JournalGeneration,
    pub(crate) login: &'a str,
}

#[cfg(test)]
/// The trusted supervisor of one exclusive restore operation. While it lives
/// it denies both writer logins except as admitted here and tracks the
/// previous backend's retirement and the target's incarnation. Every answer
/// is cooperative and bounded by the deadline it is given.
pub(crate) trait SpotSupervisor: ClosedWriterFence {
    fn target(&self) -> &RestoredTarget;
    /// The successor login the trusted spec names, pinned before admission.
    fn successor_login(&self) -> &str;
    /// An authenticated pool for `admission.login` to the same fenced
    /// endpoint. Nothing is written on it before it is verified here.
    async fn admit(
        &mut self,
        admission: &Admission<'_>,
        deadline: Instant,
    ) -> Result<PgPool, FixtureFenceError>;
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum StartupStage {
    BeforeEpoch,
    Epoch,
    BeforeCapture,
    Capture,
    Install,
    SourceAudit,
    Admit,
}

#[cfg(test)]
#[derive(Debug)]
pub(crate) enum StartupRefusal {
    Deadline(StartupStage),
    Fence(StartupStage),
    /// Another database, cluster, incarnation, endpoint or binding.
    Target(StartupStage),
    Database(StartupStage, sqlx::Error),
    Epoch(DaemonError),
    Capture(ColdAuditRefusal),
    Install(InstallRefusal),
    SourceAudit(crate::venue_budget::BudgetError),
    /// The successor login is the installer's, absent, or not the session.
    Login,
    /// The installed generation is missing or stale for the writer.
    Generation,
}

#[cfg(test)]
#[derive(Debug, Eq, PartialEq)]
struct Session {
    login: String,
    session_login: String,
    database: String,
    address: Option<String>,
    port: Option<i32>,
    postmaster_started: String,
}

#[cfg(test)]
type WriterRow = (String, String, String, Option<String>, Option<i32>, String);
#[cfg(test)]
type InstallerRow = (String, String, String, Option<String>, Option<i32>, String, String, String);

#[cfg(test)]
impl From<WriterRow> for Session {
    fn from(row: WriterRow) -> Self {
        let (login, session_login, database, address, port, postmaster_started) = row;
        Self {
            login,
            session_login,
            database,
            address,
            port,
            postmaster_started,
        }
    }
}

#[cfg(test)]
async fn bounded<T>(
    stage: StartupStage,
    deadline: Instant,
    work: impl Future<Output = T>,
) -> Result<T, StartupRefusal> {
    timeout_at(deadline, work).await.map_err(|_| StartupRefusal::Deadline(stage))
}

#[cfg(test)]
/// One owner, operation and target from before the epoch on.
struct Held<S> {
    supervisor: S,
    target: RestoredTarget,
    receipt: FenceReceipt,
    installer: Session,
    pool: PgPool,
    tenant_id: Uuid,
    account_scope: String,
    deadline: Instant,
}

#[cfg(test)]
impl<S: SpotSupervisor> Held<S> {
    async fn check(
        &mut self,
        phase: FencePhase,
        stage: StartupStage,
    ) -> Result<(), StartupRefusal> {
        let answer = self.supervisor.check(self.target.restore, phase, self.deadline);
        let receipt = bounded(stage, self.deadline, answer).await?;
        if receipt.map_err(|_| StartupRefusal::Fence(stage))? != self.receipt {
            return Err(StartupRefusal::Fence(stage));
        }
        Ok(())
    }
}

#[cfg(test)]
/// The supervisor's fence holding the receipt it gave before the epoch: the
/// installer's own phases refuse any other owner, operation or target.
struct Continuous<'a, S> {
    supervisor: &'a mut S,
    receipt: FenceReceipt,
}

#[cfg(test)]
impl<S: ClosedWriterFence> ClosedWriterFence for Continuous<'_, S> {
    async fn check(
        &mut self,
        restore: RestoreIdentity,
        phase: FencePhase,
        deadline: Instant,
    ) -> Result<FenceReceipt, FixtureFenceError> {
        let receipt = self.supervisor.check(restore, phase, deadline).await?;
        if receipt != self.receipt {
            return Err(FixtureFenceError);
        }
        Ok(receipt)
    }
}

#[cfg(test)]
/// The fence and target checked against the installer's actual connection,
/// then exactly one strict epoch. A refusal before the insert writes nothing.
pub(crate) async fn record_epoch<S: SpotSupervisor>(
    pool: PgPool,
    mut supervisor: S,
    tenant_id: Uuid,
    account_scope: &str,
    binary_version: &str,
    deadline: Instant,
) -> Result<Epoched<S>, StartupRefusal> {
    let stage = StartupStage::BeforeEpoch;
    let target = supervisor.target().clone();
    if tenant_id.is_nil()
        || account_scope != robson_exec::order_journal::DEFAULT_ACCOUNT_SCOPE
        || target.restore.operation.is_nil()
        || target.restore.target.is_nil()
    {
        return Err(StartupRefusal::Target(stage));
    }
    let answer = supervisor.check(target.restore, FencePhase::BeforeEpoch, deadline);
    let receipt = bounded(stage, deadline, answer)
        .await?
        .map_err(|_| StartupRefusal::Fence(stage))?;
    if receipt.restore != target.restore || receipt.owner.is_nil() {
        return Err(StartupRefusal::Fence(stage));
    }
    let (installer, cluster) =
        bounded(stage, deadline, read_installer(&pool, tenant_id, account_scope)).await??;
    if installer.login != installer.session_login
        || installer.database != target.database
        || installer.postmaster_started != target.postmaster_started
        || cluster != (target.system_identifier.clone(), target.data_directory.clone())
    {
        return Err(StartupRefusal::Target(stage));
    }
    let start = bounded(
        StartupStage::Epoch,
        deadline,
        crate::process_epoch::start_epoch(&pool, tenant_id, binary_version),
    )
    .await?
    .map_err(StartupRefusal::Epoch)?;
    Ok(Epoched {
        held: Held {
            supervisor,
            target,
            receipt,
            installer,
            pool,
            tenant_id,
            account_scope: account_scope.to_owned(),
            deadline,
        },
        start,
    })
}

#[cfg(test)]
async fn read_installer(
    pool: &PgPool,
    tenant_id: Uuid,
    account_scope: &str,
) -> Result<(Session, (String, String)), StartupRefusal> {
    let database = |e| StartupRefusal::Database(StartupStage::BeforeEpoch, e);
    let mut owned = LedgerConnection::new(pool.acquire().await.map_err(database)?);
    let (login, session_login, db, address, port, started, system, directory) =
        sqlx::query_as::<_, InstallerRow>(INSTALLER)
            .bind(tenant_id)
            .bind(account_scope)
            .fetch_one(owned.connection())
            .await
            .map_err(database)?;
    owned.finished = true;
    let installer = Session::from((login, session_login, db, address, port, started));
    Ok((installer, (system, directory)))
}

#[cfg(test)]
/// The one fresh epoch, under the held fence.
pub(crate) struct Epoched<S> {
    held: Held<S>,
    start: EpochStart,
}

#[cfg(test)]
impl<S: SpotSupervisor> Epoched<S> {
    pub(crate) fn epoch(&self) -> &EpochStart {
        &self.start
    }

    pub(crate) fn supervisor(&mut self) -> &mut S {
        &mut self.held.supervisor
    }

    /// The anchored cold cut at this epoch, after the same fence again.
    pub(crate) async fn capture(
        mut self,
        expected_uid: &str,
        accounting_version: i64,
    ) -> Result<Captured<S>, StartupRefusal> {
        self.held.check(FencePhase::BeforeCapture, StartupStage::BeforeCapture).await?;
        let held = &self.held;
        let request = OwnerRequest {
            scope: ColdAuditScope {
                tenant_id: held.tenant_id,
                account_scope: &held.account_scope,
            },
            expected_uid,
            epoch: self.start.epoch.epoch_id,
            accounting_version,
            restore: held.target.restore,
        };
        let cut = bounded(
            StartupStage::Capture,
            held.deadline,
            capture(&held.pool, &request, held.deadline),
        )
        .await?
        .map_err(StartupRefusal::Capture)?;
        Ok(Captured { held: self.held, start: self.start, cut })
    }
}

#[cfg(test)]
/// The cut, still under the held fence.
pub(crate) struct Captured<S> {
    held: Held<S>,
    start: EpochStart,
    cut: ColdOwnerCut,
}

#[cfg(test)]
impl<S: SpotSupervisor> Captured<S> {
    pub(crate) fn epoch(&self) -> &EpochStart {
        &self.start
    }

    pub(crate) fn cut(&self) -> &ColdOwnerCut {
        &self.cut
    }

    pub(crate) fn supervisor(&mut self) -> &mut S {
        &mut self.held.supervisor
    }

    /// The existing installer and its three phases, each held to the receipt
    /// given before the epoch.
    pub(crate) async fn install(mut self) -> Result<InstalledSpot<S>, StartupRefusal> {
        let held = &mut self.held;
        let mut fence = Continuous {
            supervisor: &mut held.supervisor,
            receipt: held.receipt,
        };
        let installed = install(&held.pool, &self.cut, &mut fence, held.deadline)
            .await
            .map_err(StartupRefusal::Install)?;
        let g = &installed.generation;
        if installed.restore != held.target.restore
            || g.tenant_id != held.tenant_id
            || g.account_scope != held.account_scope
            || g.process_epoch != self.start.epoch.epoch_id
        {
            return Err(StartupRefusal::Target(StartupStage::Install));
        }
        Ok(InstalledSpot {
            held: self.held,
            start: self.start,
            generation: installed.generation,
            accounting_version: installed.accounting_version,
        })
    }
}

#[cfg(test)]
/// The installed and independently read back generation.
pub(crate) struct InstalledSpot<S> {
    held: Held<S>,
    start: EpochStart,
    generation: JournalGeneration,
    accounting_version: i64,
}

#[cfg(test)]
impl<S: SpotSupervisor> InstalledSpot<S> {
    pub(crate) fn epoch(&self) -> &EpochStart {
        &self.start
    }

    pub(crate) fn generation(&self) -> &JournalGeneration {
        &self.generation
    }

    pub(crate) fn supervisor(&mut self) -> &mut S {
        &mut self.held.supervisor
    }

    /// Preflight only. The same writer closure covers both sides of the
    /// descriptive audit. It confers no C2/2b or venue authority.
    pub(crate) async fn audit_source_for_preflight(
        &mut self,
    ) -> Result<crate::venue_budget::source_audit::DescriptiveAudit, StartupRefusal> {
        use crate::venue_budget::source_audit::describe_existing_source;

        let stage = StartupStage::SourceAudit;
        self.held.check(FencePhase::BeforeSourceAudit, stage).await?;
        let audit = describe_existing_source(&self.held.pool, self.held.deadline)
            .await
            .map_err(StartupRefusal::SourceAudit)?;
        self.held.check(FencePhase::AfterSourceAudit, stage).await?;
        Ok(audit)
    }

    /// The pinned successor, authenticated on one bounded connection: its
    /// session on the installer's database, endpoint and incarnation, then the
    /// installed generation current for it. A refused pool is closed. The
    /// supervisor comes back to the caller, still holding the operation.
    pub(crate) async fn admit(self) -> Result<(SpotReady, S), StartupRefusal> {
        let Self {
            mut held,
            start,
            generation,
            accounting_version,
        } = self;
        let stage = StartupStage::Admit;
        let login = held.supervisor.successor_login().to_owned();
        if login.is_empty()
            || login == held.installer.login
            || login == held.installer.session_login
        {
            return Err(StartupRefusal::Login);
        }
        let admission = Admission { generation: &generation, login: &login };
        let answer = held.supervisor.admit(&admission, held.deadline);
        let writer = bounded(stage, held.deadline, answer)
            .await?
            .map_err(|_| StartupRefusal::Fence(stage))?;
        let verified = bounded(
            stage,
            held.deadline,
            authenticate(&writer, &held.installer, &login, &generation, accounting_version),
        )
        .await
        .and_then(|verified| verified);
        if let Err(refusal) = verified {
            let _ = timeout(WRITER_CLOSE_BOUND, writer.close()).await;
            return Err(refusal);
        }
        Ok((SpotReady { start, generation, writer }, held.supervisor))
    }
}

#[cfg(test)]
async fn authenticate(
    writer: &PgPool,
    installer: &Session,
    login: &str,
    generation: &JournalGeneration,
    accounting_version: i64,
) -> Result<(), StartupRefusal> {
    let database = |e| StartupRefusal::Database(StartupStage::Admit, e);
    let mut owned = LedgerConnection::new(writer.acquire().await.map_err(database)?);
    let connection: &mut PgConnection = owned.connection();
    let actual = Session::from(
        sqlx::query_as::<_, WriterRow>(WRITER)
            .fetch_one(&mut *connection)
            .await
            .map_err(database)?,
    );
    if actual.login != login || actual.session_login != login {
        return Err(StartupRefusal::Login);
    }
    if actual.database != installer.database
        || actual.address != installer.address
        || actual.port != installer.port
        || actual.postmaster_started != installer.postmaster_started
    {
        return Err(StartupRefusal::Target(StartupStage::Admit));
    }
    let venue = VenueId::new(VenueId::BINANCE_SPOT);
    let expected = ExpectedGeneration {
        tenant_id: generation.tenant_id,
        venue: &venue,
        account_scope: &generation.account_scope,
        generation_id: generation.generation_id,
        process_epoch: generation.process_epoch,
        source_audit_id: generation.source_audit_id,
    };
    let observed = observe_generation(connection, expected).await.map_err(database)?;
    if observed
        .is_none_or(|o| !o.position_hints_complete || o.accounting_version != accounting_version)
    {
        return Err(StartupRefusal::Generation);
    }
    owned.finished = true;
    Ok(())
}

/// A supervised startup's result: its one fresh epoch, the installed
/// generation and the admitted writer. The only way that writer reaches a
/// daemon.
pub(crate) struct SpotReady {
    start: EpochStart,
    generation: JournalGeneration,
    writer: PgPool,
}

impl SpotReady {
    pub(crate) fn epoch(&self) -> &EpochStart {
        &self.start
    }

    pub(crate) fn generation(&self) -> &JournalGeneration {
        &self.generation
    }

    pub(crate) fn writer(&self) -> &PgPool {
        &self.writer
    }
}

