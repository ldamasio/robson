//! Public SQLx transaction plus the two narrowly owned metadata operations.
//! See README.md for pinned upstream provenance/license and upgrade
//! obligations.
use std::time::{Duration, Instant};

use sqlx::{
    migrate::{MigrateError, Migration},
    Connection,
};

#[cfg(test)]
use super::stages::{self, Edge, Stage};
use super::OwnedApply;
const INSERT: &str = r#"
    INSERT INTO _sqlx_migrations ( version, description, success, checksum, execution_time )
    VALUES ( $1, $2, TRUE, $3, -1 )
                "#;
const ELAPSED: &str = r#"
    UPDATE _sqlx_migrations
    SET execution_time = $1
    WHERE version = $2
                "#;

#[cfg(test)]
pub(super) fn metadata_insert_sql_for_tests() -> &'static str {
    INSERT
}

pub(super) async fn apply(
    owned: &mut OwnedApply<'_>,
    migration: &Migration,
    chunks: [&str; 52],
) -> Result<Duration, MigrateError> {
    let connection = owned.connection.as_mut().expect("owned until successful return");
    #[cfg(test)]
    let gate = owned.gate;
    macro_rules! point {
        ($stage:expr,$edge:expr) => {
            #[cfg(test)]
            {
                stages::checkpoint(gate, $stage, $edge).await?;
            }
        };
    }
    let started = Instant::now();
    point!(Stage::Begin, Edge::BeforeCall);
    let mut transaction = connection.begin().await?;
    point!(Stage::Begin, Edge::AfterReturn);
    for (index, chunk) in chunks.into_iter().enumerate() {
        #[cfg(test)]
        let number =
            u8::try_from(index + 1).map_err(|_| super::refusal("migration stage index"))?;
        #[cfg(not(test))]
        let _ = index;
        point!(Stage::Ddl(number), Edge::BeforeCall);
        sqlx::query(chunk)
            .persistent(false)
            .execute(&mut *transaction)
            .await
            .map_err(|error| MigrateError::ExecuteMigration(error, migration.version))?;
        point!(Stage::Ddl(number), Edge::AfterReturn);
    }
    point!(Stage::Metadata, Edge::BeforeCall);
    sqlx::query(INSERT)
        .persistent(false)
        .bind(migration.version)
        .bind(&*migration.description)
        .bind(&*migration.checksum)
        .execute(&mut *transaction)
        .await?;
    point!(Stage::Metadata, Edge::AfterReturn);
    point!(Stage::Commit, Edge::BeforeCall);
    transaction.commit().await?;
    let elapsed = started.elapsed();
    point!(Stage::Commit, Edge::AfterReturn);
    point!(Stage::Elapsed, Edge::BeforeCall);
    // Preserve pinned SQLx's elapsed metadata representation; the absolute
    // 200ms owner bound is independent of this debugging/lineage field.
    #[allow(clippy::cast_possible_truncation)]
    let nanos = elapsed.as_nanos() as i64;
    sqlx::query(ELAPSED)
        .persistent(false)
        .bind(nanos)
        .bind(migration.version)
        .execute(connection)
        .await?;
    point!(Stage::Elapsed, Edge::AfterReturn);
    Ok(elapsed)
}
