//! Owned migration adapter. Only the private embedded38 contract selects the
//! bounded path; tests may override it with a private synthetic fixture. The
//! exact embedded39 and embedded40 migrations use a delegated bounded path.
//! Other version-39 sources are refused outside this crate's tests, and every
//! other version-40 source is refused.
use std::{future::Future, pin::Pin, time::Duration};

use sqlx::{
    migrate::{AppliedMigration, Migrate, MigrateError, Migration, MigrationType},
    postgres::PgConnectOptions,
    ConnectOptions, PgConnection,
};
use tokio::time::timeout;

mod fixed52;
mod public_apply;
mod settings;
#[cfg(test)]
mod stages;
#[cfg(test)]
mod tests;

type MigrationFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, MigrateError>> + Send + 'a>>;
const VERSION: i64 = 20240101000038;
const VERSION39: i64 = 20240101000039;
const VERSION40: i64 = 20240101000040;
const APPLY_BOUND: Duration = Duration::from_secs(15);
const ACQUIRE_BOUND: Duration = Duration::from_secs(5);

/// Observable adapter phases, not SQLx's opaque inner wire phases.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Preflight,
    SetupStarted,
    SetupConfirmed,
    DelegatedApply,
    ApplyReturned,
    RestoreStarted,
    Restored,
    Retired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    ApplyNotStarted,
    MayHaveCommitted,
    AppliedButSessionUnrestored,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Failure {
    pub at: Phase,
    pub outcome: Outcome,
}

/// Bounded diagnostic state; no connection options or SQL values are retained.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Status {
    pub phase: Phase,
    pub failure: Option<Failure>,
}

/// Owns a dedicated raw connection. It is never returned to a pool on failure.
/// Version38 must exactly match the private embedded migration inventory.
pub struct BoundedMigration {
    connection: Option<PgConnection>,
    status: Status,
    #[cfg(test)]
    contract: Option<Migration>,
    #[cfg(test)]
    gate: stages::Slot,
}

fn refusal(message: &'static str) -> MigrateError {
    MigrateError::Source(Box::new(std::io::Error::other(message)))
}

fn interrupted(phase: Phase) -> Outcome {
    match phase {
        Phase::DelegatedApply => Outcome::MayHaveCommitted,
        Phase::ApplyReturned | Phase::RestoreStarted | Phase::Restored => {
            Outcome::AppliedButSessionUnrestored
        },
        _ => Outcome::ApplyNotStarted,
    }
}

struct OwnedApply<'a> {
    connection: Option<PgConnection>,
    status: &'a mut Status,
    #[cfg(test)]
    gate: &'a stages::Slot,
}

impl OwnedApply<'_> {
    fn connection(&mut self) -> &mut PgConnection {
        self.connection.as_mut().expect("owned until successful return")
    }

    fn phase(&mut self, phase: Phase) {
        self.status.phase = phase;
    }

    fn finish(mut self) -> PgConnection {
        self.connection.take().expect("owned until successful return")
    }
}

impl Drop for OwnedApply<'_> {
    fn drop(&mut self) {
        if self.connection.is_some() {
            let failure = Failure {
                at: self.status.phase,
                outcome: interrupted(self.status.phase),
            };
            self.status.failure = Some(failure);
            self.status.phase = Phase::Retired;
            // Dropping the borrowed SQLx future first queues any ROLLBACK. Raw
            // connection drop discards it; graceful close would flush hidden SQL.
            drop(self.connection.take());
            tracing::warn!(phase = ?failure.at, outcome = ?failure.outcome, "migration connection retired");
        }
    }
}

impl BoundedMigration {
    pub async fn connect(options: &PgConnectOptions) -> Result<Self, MigrateError> {
        let connection = timeout(ACQUIRE_BOUND, options.connect())
            .await
            .map_err(|_| refusal("migration connection deadline exceeded"))??;
        Ok(Self {
            connection: Some(connection),
            status: Status { phase: Phase::Preflight, failure: None },
            #[cfg(test)]
            contract: None,
            #[cfg(test)]
            gate: Default::default(),
        })
    }

    pub fn status(&self) -> Status {
        self.status
    }

    fn connection(&mut self) -> Result<&mut PgConnection, MigrateError> {
        self.connection
            .as_mut()
            .ok_or_else(|| refusal("migration connection is retired"))
    }

    fn take(&mut self) -> Result<OwnedApply<'_>, MigrateError> {
        let connection = self
            .connection
            .take()
            .ok_or_else(|| refusal("migration connection is retired"))?;
        self.status = Status { phase: Phase::Preflight, failure: None };
        Ok(OwnedApply {
            connection: Some(connection),
            status: &mut self.status,
            #[cfg(test)]
            gate: &self.gate,
        })
    }

    fn contract(&self) -> Option<&Migration> {
        #[cfg(test)]
        if let Some(contract) = self.contract.as_ref() {
            return Some(contract);
        }
        crate::MIGRATOR.iter().find(|migration| migration.version == VERSION)
    }

    async fn apply38(&mut self, migration: &Migration) -> Result<Duration, MigrateError> {
        let preflight = preflight(migration, self.contract());
        #[cfg(test)]
        let synthetic = self.contract.is_some();
        let mut owned = self.take()?;
        preflight?;
        #[cfg(not(test))]
        let chunks = fixed52::chunks(migration)?;
        #[cfg(test)]
        let chunks = if synthetic {
            None
        } else {
            Some(fixed52::chunks(migration)?)
        };
        let result = timeout(APPLY_BOUND, async {
            owned.phase(Phase::SetupStarted);
            #[cfg(test)]
            stages::checkpoint(owned.gate, stages::Stage::Install, stages::Edge::BeforeCall)
                .await?;
            let settings = settings::install(owned.connection()).await?;
            #[cfg(test)]
            stages::checkpoint(owned.gate, stages::Stage::Install, stages::Edge::AfterReturn)
                .await?;
            owned.phase(Phase::SetupConfirmed);
            owned.phase(Phase::DelegatedApply);
            #[cfg(not(test))]
            let elapsed = public_apply::apply(&mut owned, migration, chunks).await?;
            #[cfg(test)]
            let elapsed = match chunks {
                Some(chunks) => public_apply::apply(&mut owned, migration, chunks).await?,
                None => owned.connection().apply(migration).await?,
            };
            owned.phase(Phase::ApplyReturned);
            owned.phase(Phase::RestoreStarted);
            #[cfg(test)]
            stages::checkpoint(owned.gate, stages::Stage::Restore, stages::Edge::BeforeCall)
                .await?;
            settings.restore(owned.connection()).await?;
            #[cfg(test)]
            stages::checkpoint(owned.gate, stages::Stage::Restore, stages::Edge::AfterReturn)
                .await?;
            owned.phase(Phase::Restored);
            Ok::<_, MigrateError>(elapsed)
        })
        .await
        .map_err(|_| refusal("migration38 whole-apply deadline exceeded"))??;
        self.connection = Some(owned.finish());
        Ok(result)
    }

    /// Migration39's `ALTER TABLE` needs ACCESS EXCLUSIVE on `venue_orders`.
    /// Without the session bounds it would queue behind any holder and block
    /// every later reader and writer of the table. SQLx's own apply (BEGIN,
    /// the single original statement, metadata INSERT, COMMIT, elapsed UPDATE)
    /// runs unchanged on this same owned session between install and restore.
    async fn apply39(&mut self, migration: &Migration) -> Result<Duration, MigrateError> {
        self.apply_delegated(
            migration,
            preflight39(migration),
            "migration39 whole-apply deadline exceeded",
        )
        .await
    }

    async fn apply_delegated(
        &mut self,
        migration: &Migration,
        preflight: Result<(), MigrateError>,
        deadline_error: &'static str,
    ) -> Result<Duration, MigrateError> {
        let mut owned = self.take()?;
        preflight?;
        let result = timeout(APPLY_BOUND, async {
            owned.phase(Phase::SetupStarted);
            #[cfg(test)]
            stages::checkpoint(owned.gate, stages::Stage::Install, stages::Edge::BeforeCall)
                .await?;
            let settings = settings::install(owned.connection()).await?;
            #[cfg(test)]
            stages::checkpoint(owned.gate, stages::Stage::Install, stages::Edge::AfterReturn)
                .await?;
            owned.phase(Phase::SetupConfirmed);
            owned.phase(Phase::DelegatedApply);
            let elapsed = owned.connection().apply(migration).await?;
            owned.phase(Phase::ApplyReturned);
            owned.phase(Phase::RestoreStarted);
            #[cfg(test)]
            stages::checkpoint(owned.gate, stages::Stage::Restore, stages::Edge::BeforeCall)
                .await?;
            settings.restore(owned.connection()).await?;
            #[cfg(test)]
            stages::checkpoint(owned.gate, stages::Stage::Restore, stages::Edge::AfterReturn)
                .await?;
            owned.phase(Phase::Restored);
            Ok::<_, MigrateError>(elapsed)
        })
        .await
        .map_err(|_| refusal(deadline_error))??;
        self.connection = Some(owned.finish());
        Ok(result)
    }
}

fn contract39() -> Option<&'static Migration> {
    crate::MIGRATOR.iter().find(|migration| migration.version == VERSION39)
}

fn contract40() -> Option<&'static Migration> {
    crate::MIGRATOR.iter().find(|migration| migration.version == VERSION40)
}

/// Only this crate's tests may route a non-embedded version-39 source (the
/// acceptance suite's private ordinary witnesses) through SQLx's own path.
const ORDINARY39: bool = cfg!(test);

/// Version 39 is either the exact embedded39 contract (bounded path) or, in
/// production, refused before any SQL so it can never run unbounded.
fn route39(
    migration: &Migration,
    expected: Option<&Migration>,
    ordinary39: bool,
) -> Result<bool, MigrateError> {
    if selects39(migration, expected) {
        return Ok(true);
    }
    if migration.version == VERSION39 && !ordinary39 {
        return Err(refusal("migration39 contract mismatch"));
    }
    Ok(false)
}

/// Only the exact embedded39 identity selects the bounded39 path. The
/// production runner passes exactly that inventory.
fn selects39(migration: &Migration, expected: Option<&Migration>) -> bool {
    migration.version == VERSION39
        && expected.is_some_and(|expected| matches_embedded(migration, expected))
}

fn matches_embedded(migration: &Migration, expected: &Migration) -> bool {
    migration.version == expected.version
        && migration.description == expected.description
        && migration.sql == expected.sql
        && migration.checksum == expected.checksum
        && migration.migration_type == expected.migration_type
        && migration.no_tx == expected.no_tx
}

/// PG12 has no per-command statement timer inside a multi-command simple
/// Query, so the selected contract must stay one transactional statement.
/// Only the leading header of blank and `--` lines is skipped: no code
/// precedes it, so no quote can hide a separator there. Every later line,
/// comment or not, still counts.
fn preflight39(migration: &Migration) -> Result<(), MigrateError> {
    if migration.no_tx || migration.migration_type != MigrationType::Simple {
        return Err(refusal("migration39 must run inside one SQLx transaction"));
    }
    let sql = migration.sql.trim_end();
    let semicolons: usize = sql
        .lines()
        .skip_while(|line| {
            let line = line.trim_start();
            line.is_empty() || line.starts_with("--")
        })
        .map(|line| line.matches(';').count())
        .sum();
    if !sql.ends_with(';') || semicolons != 1 {
        return Err(refusal("migration39 must be one top-level statement"));
    }
    Ok(())
}

fn preflight40(migration: &Migration) -> Result<(), MigrateError> {
    if migration.no_tx || migration.migration_type != MigrationType::Simple {
        return Err(refusal("migration40 must run inside one SQLx transaction"));
    }
    // The first matching dollar delimiter ends the PostgreSQL DO body. No
    // SQL or comments may precede or follow the canonical wrapper.
    let body = migration
        .sql
        .trim_end()
        .strip_prefix("DO $migration40$\n")
        .and_then(|sql| sql.strip_suffix("\n$migration40$;"));
    if body.is_none_or(|body| body.is_empty() || body.contains("$migration40$")) {
        return Err(refusal("migration40 must be one top-level DO statement"));
    }
    Ok(())
}

fn preflight(migration: &Migration, expected: Option<&Migration>) -> Result<(), MigrateError> {
    let expected = expected.ok_or_else(|| refusal("no compiled migration38 contract"))?;
    if migration.version != VERSION
        || expected.version != VERSION
        || migration.description != expected.description
        || migration.no_tx
        || expected.no_tx
        || migration.sql != expected.sql
        || migration.checksum != expected.checksum
        || migration.migration_type != expected.migration_type
    {
        return Err(refusal("migration38 contract mismatch"));
    }
    let body = migration
        .sql
        .trim()
        .strip_prefix("DO $migration38$")
        .and_then(|sql| sql.strip_suffix("$migration38$;"))
        .ok_or_else(|| refusal("migration38 must be one canonical DO statement"))?;
    if body.contains("$migration38$") {
        return Err(refusal("migration38 contains an extra top-level statement"));
    }
    Ok(())
}

macro_rules! forward {
    ($name:ident, $output:ty) => {
        fn $name(&mut self) -> MigrationFuture<'_, $output> {
            Box::pin(async move { self.connection()?.$name().await })
        }
    };
}

impl Migrate for BoundedMigration {
    forward!(ensure_migrations_table, ());
    forward!(dirty_version, Option<i64>);
    forward!(list_applied_migrations, Vec<AppliedMigration>);
    forward!(unlock, ());

    fn lock(&mut self) -> MigrationFuture<'_, ()> {
        Box::pin(async move {
            let mut owned = self.take()?;
            timeout(ACQUIRE_BOUND, owned.connection().lock())
                .await
                .map_err(|_| refusal("migration advisory-lock deadline exceeded"))??;
            self.connection = Some(owned.finish());
            Ok(())
        })
    }

    fn apply<'e: 'm, 'm>(&'e mut self, migration: &'m Migration) -> MigrationFuture<'m, Duration> {
        Box::pin(async move {
            if migration.version == VERSION {
                self.apply38(migration).await
            } else if migration.version == VERSION40 {
                if !contract40().is_some_and(|expected| matches_embedded(migration, expected)) {
                    return Err(refusal("migration40 contract mismatch"));
                }
                self.apply_delegated(
                    migration,
                    preflight40(migration),
                    "migration40 whole-apply deadline exceeded",
                )
                .await
            } else if route39(migration, contract39(), ORDINARY39)? {
                self.apply39(migration).await
            } else {
                self.connection()?.apply(migration).await
            }
        })
    }

    fn revert<'e: 'm, 'm>(&'e mut self, migration: &'m Migration) -> MigrationFuture<'m, Duration> {
        Box::pin(async move { self.connection()?.revert(migration).await })
    }
}

// The native PostgreSQL acceptance suite, compiled only into this crate's
// tests with the postgres feature; never a production API.
#[cfg(all(test, feature = "postgres"))]
#[path = "../../robsond/tests/bounded_migration/mod.rs"]
mod public_pg;
