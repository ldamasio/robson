//! Owned migration adapter. Only the private embedded38 contract selects the
//! bounded path; tests may override it with a private synthetic fixture. The
//! exact embedded39, embedded40 to embedded45, embedded46 to embedded50,
//! embedded51 to embedded54 and embedded55 to embedded56 migrations use a
//! delegated bounded path. Other version-39 sources are refused outside this
//! crate's tests, and every other source of a version from 40 to 56 is
//! refused.
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
const VERSION41: i64 = 20240101000041;
const VERSION42: i64 = 20240101000042;
const VERSION43: i64 = 20240101000043;
const VERSION44: i64 = 20240101000044;
const VERSION45: i64 = 20240101000045;
const VERSION46: i64 = 20240101000046;
const VERSION47: i64 = 20240101000047;
const VERSION48: i64 = 20240101000048;
const VERSION49: i64 = 20240101000049;
const VERSION50: i64 = 20240101000050;
const VERSION51: i64 = 20240101000051;
const VERSION52: i64 = 20240101000052;
const VERSION53: i64 = 20240101000053;
const VERSION54: i64 = 20240101000054;
const VERSION55: i64 = 20240101000055;
const VERSION56: i64 = 20240101000056;
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

fn contract41() -> Option<&'static Migration> {
    crate::MIGRATOR.iter().find(|migration| migration.version == VERSION41)
}

fn contract42() -> Option<&'static Migration> {
    crate::MIGRATOR.iter().find(|migration| migration.version == VERSION42)
}

fn contract43() -> Option<&'static Migration> {
    crate::MIGRATOR.iter().find(|migration| migration.version == VERSION43)
}

fn contract44() -> Option<&'static Migration> {
    crate::MIGRATOR.iter().find(|migration| migration.version == VERSION44)
}

fn contract45() -> Option<&'static Migration> {
    crate::MIGRATOR.iter().find(|migration| migration.version == VERSION45)
}

fn contract46() -> Option<&'static Migration> {
    crate::MIGRATOR.iter().find(|migration| migration.version == VERSION46)
}

fn contract47() -> Option<&'static Migration> {
    crate::MIGRATOR.iter().find(|migration| migration.version == VERSION47)
}

fn contract48() -> Option<&'static Migration> {
    crate::MIGRATOR.iter().find(|migration| migration.version == VERSION48)
}

fn contract49() -> Option<&'static Migration> {
    crate::MIGRATOR.iter().find(|migration| migration.version == VERSION49)
}

fn contract50() -> Option<&'static Migration> {
    crate::MIGRATOR.iter().find(|migration| migration.version == VERSION50)
}

fn contract51() -> Option<&'static Migration> {
    crate::MIGRATOR.iter().find(|migration| migration.version == VERSION51)
}

fn contract52() -> Option<&'static Migration> {
    crate::MIGRATOR.iter().find(|migration| migration.version == VERSION52)
}

fn contract53() -> Option<&'static Migration> {
    crate::MIGRATOR.iter().find(|migration| migration.version == VERSION53)
}

fn contract54() -> Option<&'static Migration> {
    crate::MIGRATOR.iter().find(|migration| migration.version == VERSION54)
}

fn contract55() -> Option<&'static Migration> {
    crate::MIGRATOR.iter().find(|migration| migration.version == VERSION55)
}

fn contract56() -> Option<&'static Migration> {
    crate::MIGRATOR.iter().find(|migration| migration.version == VERSION56)
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
    preflight_delegated_do(
        migration,
        "$migration40$",
        "migration40 must run inside one SQLx transaction",
        "migration40 must be one top-level DO statement",
    )
}

fn preflight41(migration: &Migration) -> Result<(), MigrateError> {
    preflight_delegated_do(
        migration,
        "$migration41$",
        "migration41 must run inside one SQLx transaction",
        "migration41 must be one top-level DO statement",
    )
}

fn preflight42(migration: &Migration) -> Result<(), MigrateError> {
    preflight_delegated_do(
        migration,
        "$migration42$",
        "migration42 must run inside one SQLx transaction",
        "migration42 must be one top-level DO statement",
    )
}

fn preflight43(migration: &Migration) -> Result<(), MigrateError> {
    preflight_delegated_do(
        migration,
        "$migration43$",
        "migration43 must run inside one SQLx transaction",
        "migration43 must be one top-level DO statement",
    )
}

fn preflight44(migration: &Migration) -> Result<(), MigrateError> {
    preflight_delegated_do(
        migration,
        "$migration44$",
        "migration44 must run inside one SQLx transaction",
        "migration44 must be one top-level DO statement",
    )
}

fn preflight45(migration: &Migration) -> Result<(), MigrateError> {
    preflight_delegated_do(
        migration,
        "$migration45$",
        "migration45 must run inside one SQLx transaction",
        "migration45 must be one top-level DO statement",
    )
}

fn preflight46(migration: &Migration) -> Result<(), MigrateError> {
    preflight_delegated_do(
        migration,
        "$migration46$",
        "migration46 must run inside one SQLx transaction",
        "migration46 must be one top-level DO statement",
    )
}

fn preflight47(migration: &Migration) -> Result<(), MigrateError> {
    preflight_delegated_do(
        migration,
        "$migration47$",
        "migration47 must run inside one SQLx transaction",
        "migration47 must be one top-level DO statement",
    )
}

fn preflight48(migration: &Migration) -> Result<(), MigrateError> {
    preflight_delegated_do(
        migration,
        "$migration48$",
        "migration48 must run inside one SQLx transaction",
        "migration48 must be one top-level DO statement",
    )
}

fn preflight49(migration: &Migration) -> Result<(), MigrateError> {
    preflight_delegated_do(
        migration,
        "$migration49$",
        "migration49 must run inside one SQLx transaction",
        "migration49 must be one top-level DO statement",
    )
}

fn preflight50(migration: &Migration) -> Result<(), MigrateError> {
    preflight_delegated_do(
        migration,
        "$migration50$",
        "migration50 must run inside one SQLx transaction",
        "migration50 must be one top-level DO statement",
    )
}

fn preflight51(migration: &Migration) -> Result<(), MigrateError> {
    preflight_delegated_do(
        migration,
        "$migration51$",
        "migration51 must run inside one SQLx transaction",
        "migration51 must be one top-level DO statement",
    )
}

fn preflight52(migration: &Migration) -> Result<(), MigrateError> {
    preflight_delegated_do(
        migration,
        "$migration52$",
        "migration52 must run inside one SQLx transaction",
        "migration52 must be one top-level DO statement",
    )
}

fn preflight53(migration: &Migration) -> Result<(), MigrateError> {
    preflight_delegated_do(
        migration,
        "$migration53$",
        "migration53 must run inside one SQLx transaction",
        "migration53 must be one top-level DO statement",
    )
}

fn preflight54(migration: &Migration) -> Result<(), MigrateError> {
    preflight_delegated_do(
        migration,
        "$migration54$",
        "migration54 must run inside one SQLx transaction",
        "migration54 must be one top-level DO statement",
    )
}

fn preflight55(migration: &Migration) -> Result<(), MigrateError> {
    preflight_delegated_do(
        migration,
        "$migration55$",
        "migration55 must run inside one SQLx transaction",
        "migration55 must be one top-level DO statement",
    )
}

fn preflight56(migration: &Migration) -> Result<(), MigrateError> {
    preflight_delegated_do(
        migration,
        "$migration56$",
        "migration56 must run inside one SQLx transaction",
        "migration56 must be one top-level DO statement",
    )
}

fn preflight_delegated_do(
    migration: &Migration,
    delimiter: &str,
    transaction_error: &'static str,
    shape_error: &'static str,
) -> Result<(), MigrateError> {
    if migration.no_tx || migration.migration_type != MigrationType::Simple {
        return Err(refusal(transaction_error));
    }
    // The first matching dollar delimiter ends the PostgreSQL DO body. No
    // SQL or comments may precede or follow the canonical wrapper.
    let prefix = format!("DO {delimiter}\n");
    let suffix = format!("\n{delimiter};");
    let body = migration
        .sql
        .trim_end()
        .strip_prefix(prefix.as_str())
        .and_then(|sql| sql.strip_suffix(suffix.as_str()));
    if body.is_none_or(|body| body.is_empty() || body.contains(delimiter)) {
        return Err(refusal(shape_error));
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
            } else if migration.version == VERSION41 {
                if !contract41().is_some_and(|expected| matches_embedded(migration, expected)) {
                    return Err(refusal("migration41 contract mismatch"));
                }
                self.apply_delegated(
                    migration,
                    preflight41(migration),
                    "migration41 whole-apply deadline exceeded",
                )
                .await
            } else if migration.version == VERSION42 {
                if !contract42().is_some_and(|expected| matches_embedded(migration, expected)) {
                    return Err(refusal("migration42 contract mismatch"));
                }
                self.apply_delegated(
                    migration,
                    preflight42(migration),
                    "migration42 whole-apply deadline exceeded",
                )
                .await
            } else if migration.version == VERSION43 {
                if !contract43().is_some_and(|expected| matches_embedded(migration, expected)) {
                    return Err(refusal("migration43 contract mismatch"));
                }
                self.apply_delegated(
                    migration,
                    preflight43(migration),
                    "migration43 whole-apply deadline exceeded",
                )
                .await
            } else if migration.version == VERSION44 {
                if !contract44().is_some_and(|expected| matches_embedded(migration, expected)) {
                    return Err(refusal("migration44 contract mismatch"));
                }
                self.apply_delegated(
                    migration,
                    preflight44(migration),
                    "migration44 whole-apply deadline exceeded",
                )
                .await
            } else if migration.version == VERSION45 {
                if !contract45().is_some_and(|expected| matches_embedded(migration, expected)) {
                    return Err(refusal("migration45 contract mismatch"));
                }
                self.apply_delegated(
                    migration,
                    preflight45(migration),
                    "migration45 whole-apply deadline exceeded",
                )
                .await
            } else if migration.version == VERSION46 {
                if !contract46().is_some_and(|expected| matches_embedded(migration, expected)) {
                    return Err(refusal("migration46 contract mismatch"));
                }
                self.apply_delegated(
                    migration,
                    preflight46(migration),
                    "migration46 whole-apply deadline exceeded",
                )
                .await
            } else if migration.version == VERSION47 {
                if !contract47().is_some_and(|expected| matches_embedded(migration, expected)) {
                    return Err(refusal("migration47 contract mismatch"));
                }
                self.apply_delegated(
                    migration,
                    preflight47(migration),
                    "migration47 whole-apply deadline exceeded",
                )
                .await
            } else if migration.version == VERSION48 {
                if !contract48().is_some_and(|expected| matches_embedded(migration, expected)) {
                    return Err(refusal("migration48 contract mismatch"));
                }
                self.apply_delegated(
                    migration,
                    preflight48(migration),
                    "migration48 whole-apply deadline exceeded",
                )
                .await
            } else if migration.version == VERSION49 {
                if !contract49().is_some_and(|expected| matches_embedded(migration, expected)) {
                    return Err(refusal("migration49 contract mismatch"));
                }
                self.apply_delegated(
                    migration,
                    preflight49(migration),
                    "migration49 whole-apply deadline exceeded",
                )
                .await
            } else if migration.version == VERSION50 {
                if !contract50().is_some_and(|expected| matches_embedded(migration, expected)) {
                    return Err(refusal("migration50 contract mismatch"));
                }
                self.apply_delegated(
                    migration,
                    preflight50(migration),
                    "migration50 whole-apply deadline exceeded",
                )
                .await
            } else if migration.version == VERSION51 {
                if !contract51().is_some_and(|expected| matches_embedded(migration, expected)) {
                    return Err(refusal("migration51 contract mismatch"));
                }
                self.apply_delegated(
                    migration,
                    preflight51(migration),
                    "migration51 whole-apply deadline exceeded",
                )
                .await
            } else if migration.version == VERSION52 {
                if !contract52().is_some_and(|expected| matches_embedded(migration, expected)) {
                    return Err(refusal("migration52 contract mismatch"));
                }
                self.apply_delegated(
                    migration,
                    preflight52(migration),
                    "migration52 whole-apply deadline exceeded",
                )
                .await
            } else if migration.version == VERSION53 {
                if !contract53().is_some_and(|expected| matches_embedded(migration, expected)) {
                    return Err(refusal("migration53 contract mismatch"));
                }
                self.apply_delegated(
                    migration,
                    preflight53(migration),
                    "migration53 whole-apply deadline exceeded",
                )
                .await
            } else if migration.version == VERSION54 {
                if !contract54().is_some_and(|expected| matches_embedded(migration, expected)) {
                    return Err(refusal("migration54 contract mismatch"));
                }
                self.apply_delegated(
                    migration,
                    preflight54(migration),
                    "migration54 whole-apply deadline exceeded",
                )
                .await
            } else if migration.version == VERSION55 {
                if !contract55().is_some_and(|expected| matches_embedded(migration, expected)) {
                    return Err(refusal("migration55 contract mismatch"));
                }
                self.apply_delegated(
                    migration,
                    preflight55(migration),
                    "migration55 whole-apply deadline exceeded",
                )
                .await
            } else if migration.version == VERSION56 {
                if !contract56().is_some_and(|expected| matches_embedded(migration, expected)) {
                    return Err(refusal("migration56 contract mismatch"));
                }
                self.apply_delegated(
                    migration,
                    preflight56(migration),
                    "migration56 whole-apply deadline exceeded",
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
