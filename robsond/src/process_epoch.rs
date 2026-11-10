//! Process epochs (ADR-0056 step 7, PR S).
//!
//! One `process_epochs` row per daemon process, inserted with a strict commit
//! before the process sends any venue request, and marked by a stop that
//! passes its check. A start whose predecessor has no marker, or a marker
//! older than [`CLEAN_MARKER_VERSION`], is unclean. Only the fence PR writes
//! that version, so until then every start is unclean, and that is recorded
//! (log and gauge) and nothing more.

use std::time::Duration;

use sqlx::{PgPool, Row};
use tracing::{info, warn};
use uuid::Uuid;

use crate::error::{DaemonError, DaemonResult};

/// The marker a clean stop of this binary writes.
pub const MARKER_VERSION: i16 = 1;
/// The marker a predecessor needs for a start to be clean: the fence PR's.
pub const CLEAN_MARKER_VERSION: i16 = 2;
/// `epoch_seq` stays below this: it is the high half of the fence PR's
/// generation, a signed 64-bit value.
pub const EPOCH_SEQ_LIMIT: i64 = 1 << 31;
/// How long the insert may take before the start is refused.
const EPOCH_INSERT_BOUND: Duration = Duration::from_secs(10);

/// This process's epoch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Epoch {
    pub epoch_id: Uuid,
    pub epoch_seq: i64,
}

/// What the previous epoch of the same tenant left.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Predecessor {
    /// None recorded: the first start on this table, after a binary that wrote
    /// none, whatever that process did.
    None,
    /// It never marked a clean stop: it died, was killed, or stopped unclean.
    NoMarker { epoch_seq: i64 },
    /// It stopped clean and marked it at this version.
    Marked { epoch_seq: i64, marker_version: i16 },
}

impl Predecessor {
    /// Why a start after it is unclean; `None` when it is clean.
    pub fn unclean_reason(self) -> Option<&'static str> {
        match self {
            Self::None => Some("no_predecessor"),
            Self::NoMarker { .. } => Some("no_marker"),
            Self::Marked { marker_version, .. } if marker_version < CLEAN_MARKER_VERSION => {
                Some("marker_version")
            },
            Self::Marked { .. } => None,
        }
    }
}

/// The epoch inserted at this start and what it found before it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EpochStart {
    pub epoch: Epoch,
    pub predecessor: Predecessor,
}

/// Insert this process's epoch, strictly: it has landed when this returns, or
/// the start is refused. The predecessor is read in the same transaction.
///
/// # Errors
/// The database did not take the insert within its bound, or `epoch_seq`
/// reached [`EPOCH_SEQ_LIMIT`]; the row is then rolled back.
pub async fn start_epoch(
    pool: &PgPool,
    tenant_id: Uuid,
    binary_version: &str,
) -> DaemonResult<EpochStart> {
    match tokio::time::timeout(EPOCH_INSERT_BOUND, insert(pool, tenant_id, binary_version)).await {
        Ok(started) => started,
        Err(_) => Err(DaemonError::Config(format!(
            "process epoch not recorded within {} s; refusing to start",
            EPOCH_INSERT_BOUND.as_secs()
        ))),
    }
}

async fn insert(pool: &PgPool, tenant_id: Uuid, binary_version: &str) -> DaemonResult<EpochStart> {
    let mut tx = pool.begin().await?;
    let previous = sqlx::query(
        "SELECT epoch_seq, marker_version FROM process_epochs
         WHERE tenant_id = $1 ORDER BY epoch_seq DESC LIMIT 1",
    )
    .bind(tenant_id)
    .fetch_optional(&mut *tx)
    .await?;
    let predecessor = match previous {
        None => Predecessor::None,
        Some(row) => {
            let epoch_seq: i64 = row.get("epoch_seq");
            match row.get::<Option<i16>, _>("marker_version") {
                None => Predecessor::NoMarker { epoch_seq },
                Some(marker_version) => Predecessor::Marked { epoch_seq, marker_version },
            }
        },
    };
    let epoch_id = Uuid::now_v7();
    let epoch_seq: i64 = sqlx::query_scalar(
        "INSERT INTO process_epochs (epoch_id, tenant_id, binary_version)
         VALUES ($1, $2, $3) RETURNING epoch_seq",
    )
    .bind(epoch_id)
    .bind(tenant_id)
    .bind(binary_version)
    .fetch_one(&mut *tx)
    .await?;
    if epoch_seq >= EPOCH_SEQ_LIMIT {
        tx.rollback().await?;
        return Err(DaemonError::Config(format!(
            "process epoch sequence exhausted ({epoch_seq} >= 2^31); refusing to start"
        )));
    }
    tx.commit().await?;
    Ok(EpochStart {
        epoch: Epoch { epoch_id, epoch_seq },
        predecessor,
    })
}

/// Mark `epoch` as stopped clean. The caller bounds the wait.
///
/// # Errors
/// The update failed, or found no unmarked row for the epoch.
pub async fn mark_clean_stop(pool: &PgPool, epoch: Epoch) -> Result<(), String> {
    let marked = sqlx::query(
        "UPDATE process_epochs SET clean_stop_at = NOW(), marker_version = $2
         WHERE epoch_id = $1 AND clean_stop_at IS NULL",
    )
    .bind(epoch.epoch_id)
    .bind(MARKER_VERSION)
    .execute(pool)
    .await
    .map_err(|error| error.to_string())?;
    if marked.rows_affected() == 1 {
        Ok(())
    } else {
        Err(format!("epoch {} has no unmarked row", epoch.epoch_seq))
    }
}

/// Log and export whether this start is unclean. Recorded only: the fence PR
/// opens an incident on it.
pub fn record_start(start: &EpochStart) {
    let epoch_seq = start.epoch.epoch_seq;
    match (start.predecessor, start.predecessor.unclean_reason()) {
        (_, None) => info!(epoch_seq, "Process epoch started; predecessor stopped clean"),
        (Predecessor::Marked { epoch_seq: previous, marker_version }, Some(reason)) => {
            crate::metrics::UNCLEAN_START.with_label_values(&[reason]).set(1.0);
            info!(
                epoch_seq,
                previous,
                marker_version,
                "Process epoch started; predecessor stopped clean at a marker version that \
                 counts as unclean until the fence PR"
            );
        },
        (predecessor, Some(reason)) => {
            crate::metrics::UNCLEAN_START.with_label_values(&[reason]).set(1.0);
            warn!(epoch_seq, ?predecessor, reason, "Process epoch started after an unclean stop");
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_start_is_unclean_until_a_version_2_marker() {
        assert_eq!(Predecessor::None.unclean_reason(), Some("no_predecessor"));
        assert_eq!(Predecessor::NoMarker { epoch_seq: 7 }.unclean_reason(), Some("no_marker"));
        assert_eq!(
            Predecessor::Marked {
                epoch_seq: 7,
                marker_version: MARKER_VERSION
            }
            .unclean_reason(),
            Some("marker_version")
        );
        assert_eq!(
            Predecessor::Marked {
                epoch_seq: 7,
                marker_version: CLEAN_MARKER_VERSION
            }
            .unclean_reason(),
            None
        );
    }

    async fn epochs(pool: &PgPool, tenant_id: Uuid) -> Vec<(i64, bool, Option<i16>)> {
        sqlx::query_as(
            "SELECT epoch_seq, clean_stop_at IS NOT NULL, marker_version FROM process_epochs
             WHERE tenant_id = $1 ORDER BY epoch_seq",
        )
        .bind(tenant_id)
        .fetch_all(pool)
        .await
        .unwrap()
    }

    /// Each start inserts its epoch and reads the previous one of its tenant:
    /// none, unmarked, or marked at version 1, which is still unclean.
    #[sqlx::test(migrations = "../migrations")]
    #[ignore = "requires DATABASE_URL"]
    async fn each_start_records_its_epoch_and_finds_its_predecessor(pool: PgPool) {
        let tenant = Uuid::now_v7();
        let first = start_epoch(&pool, tenant, "test").await.unwrap();
        assert_eq!(first.predecessor, Predecessor::None);

        mark_clean_stop(&pool, first.epoch).await.unwrap();
        assert!(mark_clean_stop(&pool, first.epoch).await.is_err(), "a marker is written once");
        let second = start_epoch(&pool, tenant, "test").await.unwrap();
        assert_eq!(second.predecessor, Predecessor::Marked {
            epoch_seq: first.epoch.epoch_seq,
            marker_version: MARKER_VERSION,
        });
        assert_eq!(second.predecessor.unclean_reason(), Some("marker_version"));
        assert!(second.epoch.epoch_seq > first.epoch.epoch_seq);

        // The second never marked: the third finds an unclean predecessor.
        let third = start_epoch(&pool, tenant, "test").await.unwrap();
        assert_eq!(third.predecessor, Predecessor::NoMarker { epoch_seq: second.epoch.epoch_seq });

        // Another tenant's epochs are not this one's predecessors.
        let other = start_epoch(&pool, Uuid::now_v7(), "test").await.unwrap();
        assert_eq!(other.predecessor, Predecessor::None);

        assert_eq!(epochs(&pool, tenant).await, [
            (first.epoch.epoch_seq, true, Some(MARKER_VERSION)),
            (second.epoch.epoch_seq, false, None),
            (third.epoch.epoch_seq, false, None),
        ]);
    }

    /// `epoch_seq` stays below 2^31: the start that would reach it is refused
    /// and its row rolled back.
    #[sqlx::test(migrations = "../migrations")]
    #[ignore = "requires DATABASE_URL"]
    async fn a_start_is_refused_once_epoch_seq_would_reach_2_pow_31(pool: PgPool) {
        let tenant = Uuid::now_v7();
        sqlx::query("SELECT setval(pg_get_serial_sequence('process_epochs', 'epoch_seq'), $1)")
            .bind(EPOCH_SEQ_LIMIT - 2)
            .execute(&pool)
            .await
            .unwrap();
        let last = start_epoch(&pool, tenant, "test").await.unwrap();
        assert_eq!(last.epoch.epoch_seq, EPOCH_SEQ_LIMIT - 1);

        let refused = start_epoch(&pool, tenant, "test").await;
        assert!(matches!(refused, Err(DaemonError::Config(ref e)) if e.contains("exhausted")));
        assert_eq!(epochs(&pool, tenant).await.len(), 1, "the refused row is rolled back");
    }

    /// A database that refuses the insert refuses the start.
    #[sqlx::test(migrations = "../migrations")]
    #[ignore = "requires DATABASE_URL"]
    async fn a_start_is_refused_when_its_epoch_does_not_land(pool: PgPool) {
        refuse_epochs(&pool).await;
        assert!(start_epoch(&pool, Uuid::now_v7(), "test").await.is_err());
    }

    /// Make every epoch insert fail on the server.
    pub(crate) async fn refuse_epochs(pool: &PgPool) {
        sqlx::query(
            "CREATE OR REPLACE FUNCTION refuse_epochs() RETURNS trigger AS $$
             BEGIN RAISE EXCEPTION 'epochs refused by the test'; END $$ LANGUAGE plpgsql",
        )
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TRIGGER refuse_epochs BEFORE INSERT ON process_epochs
             FOR EACH ROW EXECUTE FUNCTION refuse_epochs()",
        )
        .execute(pool)
        .await
        .unwrap();
    }
}
