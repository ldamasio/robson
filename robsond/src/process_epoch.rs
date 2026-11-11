//! Process epochs (ADR-0056 step 7, PR S), and the unclean start of the fence
//! PR (F).
//!
//! One `process_epochs` row per daemon process, inserted with a strict commit
//! before the process sends any venue request, and marked by a stop that
//! passes its check. A start whose predecessor has no marker, or a marker
//! older than [`CLEAN_MARKER_VERSION`], is unclean. In the epoch's own
//! transaction an unclean start stamps every intent of earlier processes with
//! no outcome and no stamp at its generation, `epoch_seq << 32 | 1`, and opens
//! or counts the `unclean_start` incident; the same transaction reads every
//! incident no release covered. The daemon seeds the fence from that before
//! any venue request (design §3).

use std::time::Duration;

use chrono::{DateTime, Utc};
use robson_exec::gate::{Incident, IncidentReason};
use sqlx::{PgPool, Row};
use tracing::{info, warn};
use uuid::Uuid;

use crate::error::{DaemonError, DaemonResult};

/// The marker a clean stop of this binary writes: the fence PR's, which a
/// stop writes only with no incident open.
pub const MARKER_VERSION: i16 = 2;
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EpochStart {
    pub epoch: Epoch,
    pub predecessor: Predecessor,
    /// The generation this start recorded its `unclean_start` occurrence at,
    /// when it is unclean.
    pub unclean_generation: Option<i64>,
    /// Intents of earlier processes stamped at that generation.
    pub stamped_intents: u64,
    /// Every incident on record no release covered, this start's included.
    pub open_incidents: Vec<Incident>,
}

/// An epoch's generation: its high half.
pub fn generation_of(epoch_seq: i64, n: u32) -> i64 {
    (epoch_seq << 32) | i64::from(n)
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
    // An unclean start is an occurrence, the first of this epoch, recorded
    // here so it is durable before any venue request.
    let unclean_generation = predecessor.unclean_reason().map(|_| generation_of(epoch_seq, 1));
    let mut stamped_intents = 0;
    if let Some(generation) = unclean_generation {
        stamped_intents = sqlx::query(
            "UPDATE order_intents SET uncertain_generation = $2, \
                 uncertain_at = COALESCE(uncertain_at, NOW()) \
             WHERE tenant_id = $1 AND venue_outcome IS NULL AND NOT not_sent \
               AND uncertain_generation IS NULL",
        )
        .bind(tenant_id)
        .bind(generation)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        sqlx::query(
            "INSERT INTO recovery_incidents (incident_id, tenant_id, reason, first_at, last_at, \
                 count, first_generation, last_generation) \
             VALUES ($1, $2, 'unclean_start', NOW(), NOW(), 1, $3, $3) \
             ON CONFLICT (tenant_id, reason) WHERE released_at IS NULL DO UPDATE SET \
                 last_at = EXCLUDED.last_at, count = recovery_incidents.count + 1, \
                 last_generation = \
                     GREATEST(recovery_incidents.last_generation, EXCLUDED.last_generation)",
        )
        .bind(Uuid::now_v7())
        .bind(tenant_id)
        .bind(generation)
        .execute(&mut *tx)
        .await?;
    }
    let open_incidents = sqlx::query_as::<_, IncidentRecord>(
        "SELECT reason, first_at, last_at, count, first_generation, last_generation \
         FROM recovery_incidents WHERE tenant_id = $1 AND released_at IS NULL ORDER BY reason",
    )
    .bind(tenant_id)
    .fetch_all(&mut *tx)
    .await?
    .into_iter()
    .map(IncidentRecord::incident)
    .collect::<DaemonResult<Vec<_>>>()?;
    tx.commit().await?;
    Ok(EpochStart {
        epoch: Epoch { epoch_id, epoch_seq },
        predecessor,
        unclean_generation,
        stamped_intents,
        open_incidents,
    })
}

#[derive(sqlx::FromRow)]
struct IncidentRecord {
    reason: String,
    first_at: DateTime<Utc>,
    last_at: DateTime<Utc>,
    count: i64,
    first_generation: i64,
    last_generation: i64,
}

impl IncidentRecord {
    /// A reason this binary does not know, a newer binary's, refuses the
    /// start: an open incident is never ignored.
    fn incident(self) -> DaemonResult<Incident> {
        let reason = IncidentReason::parse(&self.reason).ok_or_else(|| {
            DaemonError::Config(format!(
                "an open incident has a reason this binary does not know ({}); refusing to start",
                self.reason
            ))
        })?;
        Ok(Incident {
            reason,
            first_at: self.first_at,
            last_at: self.last_at,
            count: self.count,
            first_generation: self.first_generation,
            last_generation: self.last_generation,
        })
    }
}

/// Mark `epoch` as stopped clean. The caller bounds the wait. The marker is
/// never written while an incident of the epoch's tenant is open on record,
/// whatever this process's memory says (ADR-0056 step 7, PR F): the stop's
/// own check reads memory, and this one reads the record in the same
/// statement.
///
/// # Errors
/// The update failed, or found no unmarked row for the epoch, or an incident
/// is open on record.
pub async fn mark_clean_stop(pool: &PgPool, epoch: Epoch) -> Result<(), String> {
    let marked = sqlx::query(
        "UPDATE process_epochs SET clean_stop_at = NOW(), marker_version = $2
         WHERE epoch_id = $1 AND clean_stop_at IS NULL
           AND NOT EXISTS (
               SELECT 1 FROM recovery_incidents
               WHERE recovery_incidents.tenant_id = process_epochs.tenant_id
                 AND recovery_incidents.released_at IS NULL
           )",
    )
    .bind(epoch.epoch_id)
    .bind(MARKER_VERSION)
    .execute(pool)
    .await
    .map_err(|error| error.to_string())?;
    if marked.rows_affected() == 1 {
        Ok(())
    } else {
        Err(format!(
            "epoch {} has no unmarked row, or an incident is open on record",
            epoch.epoch_seq
        ))
    }
}

/// Log and export whether this start is unclean, and what it opened.
pub fn record_start(start: &EpochStart) {
    let epoch_seq = start.epoch.epoch_seq;
    let open = start.open_incidents.len();
    let stamped = start.stamped_intents;
    match (start.predecessor, start.predecessor.unclean_reason()) {
        (_, None) => info!(
            epoch_seq,
            open_incidents = open,
            "Process epoch started; predecessor stopped clean"
        ),
        (Predecessor::Marked { epoch_seq: previous, marker_version }, Some(reason)) => {
            crate::metrics::UNCLEAN_START.with_label_values(&[reason]).set(1.0);
            warn!(
                epoch_seq,
                previous,
                marker_version,
                stamped,
                "Process epoch started; predecessor stopped clean at a marker version from \
                 before the fence, so this start is unclean: incident opened, admissions fenced"
            );
        },
        (predecessor, Some(reason)) => {
            crate::metrics::UNCLEAN_START.with_label_values(&[reason]).set(1.0);
            warn!(
                epoch_seq,
                ?predecessor,
                reason,
                stamped,
                "Process epoch started after an unclean stop: incident opened, admissions fenced"
            );
        },
    }
    if open > 0 {
        warn!(
            epoch_seq,
            open_incidents = open,
            "Admissions are fenced until an operator's release (POST /recovery/release)"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_start_is_unclean_until_a_version_2_marker() {
        assert_eq!(Predecessor::None.unclean_reason(), Some("no_predecessor"));
        assert_eq!(Predecessor::NoMarker { epoch_seq: 7 }.unclean_reason(), Some("no_marker"));
        // What S and P write.
        assert_eq!(
            Predecessor::Marked { epoch_seq: 7, marker_version: 1 }.unclean_reason(),
            Some("marker_version")
        );
        assert_eq!(MARKER_VERSION, CLEAN_MARKER_VERSION, "this binary writes the clean version");
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
    /// none, unmarked, or marked; a version 2 marker is clean.
    #[sqlx::test(migrations = "../migrations")]
    #[ignore = "requires DATABASE_URL"]
    async fn each_start_records_its_epoch_and_finds_its_predecessor(pool: PgPool) {
        let tenant = Uuid::now_v7();
        let first = start_epoch(&pool, tenant, "test").await.unwrap();
        assert_eq!(first.predecessor, Predecessor::None);

        release_all(&pool, tenant).await;
        mark_clean_stop(&pool, first.epoch).await.unwrap();
        assert!(mark_clean_stop(&pool, first.epoch).await.is_err(), "a marker is written once");
        let second = start_epoch(&pool, tenant, "test").await.unwrap();
        assert_eq!(second.predecessor, Predecessor::Marked {
            epoch_seq: first.epoch.epoch_seq,
            marker_version: MARKER_VERSION,
        });
        assert_eq!(second.predecessor.unclean_reason(), None);
        assert_eq!(second.unclean_generation, None);
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

    /// The intent rows an unclean start stamps: those with no outcome and no
    /// stamp. Returns (client id, uncertain generation, uncertain_at set).
    async fn stamps(pool: &PgPool, tenant_id: Uuid) -> Vec<(String, Option<i64>, bool)> {
        sqlx::query_as(
            "SELECT client_order_id, uncertain_generation, uncertain_at IS NOT NULL \
             FROM order_intents WHERE tenant_id = $1 ORDER BY client_order_id",
        )
        .bind(tenant_id)
        .fetch_all(pool)
        .await
        .unwrap()
    }

    async fn intent(
        pool: &PgPool,
        tenant_id: Uuid,
        id: &str,
        outcome: Option<&str>,
        stamp: Option<i64>,
    ) {
        sqlx::query(
            "INSERT INTO order_intents (intent_id, tenant_id, venue_id, account_scope, \
                 client_order_id, kind, params, venue_outcome, uncertain_generation, \
                 uncertain_at, created_at) \
             VALUES ($1, $2, 'test:venue', 'operated', $3, 'exit', '{}', $4, $5, \
                 CASE WHEN $5::BIGINT IS NULL THEN NULL ELSE NOW() END, NOW())",
        )
        .bind(Uuid::now_v7())
        .bind(tenant_id)
        .bind(id)
        .bind(outcome)
        .bind(stamp)
        .execute(pool)
        .await
        .unwrap();
    }

    /// Design §2.7: an unclean start, in its epoch's transaction, stamps the
    /// intents of earlier processes with no outcome and no stamp at its own
    /// generation (`epoch_seq << 32 | 1`), `uncertain_at` included, and opens
    /// `unclean_start`; another unclean start counts on the same open
    /// incident. A row with an outcome, or stamped already, is left alone, and
    /// another tenant's rows too.
    #[sqlx::test(migrations = "../migrations")]
    #[ignore = "requires DATABASE_URL"]
    async fn an_unclean_start_stamps_the_dead_intents_and_opens_its_incident(pool: PgPool) {
        let tenant = Uuid::now_v7();
        let other = Uuid::now_v7();
        intent(&pool, tenant, "dead", None, None).await;
        intent(&pool, tenant, "done", Some("success"), None).await;
        intent(&pool, tenant, "stamped", None, Some(5)).await;
        intent(&pool, other, "theirs", None, None).await;

        let start = start_epoch(&pool, tenant, "test").await.unwrap();
        let generation = generation_of(start.epoch.epoch_seq, 1);
        assert_eq!(start.unclean_generation, Some(generation));
        assert_eq!(start.stamped_intents, 1);
        assert_eq!(stamps(&pool, tenant).await, [
            ("dead".to_string(), Some(generation), true),
            ("done".to_string(), None, false),
            ("stamped".to_string(), Some(5), true),
        ]);
        assert_eq!(stamps(&pool, other).await, [("theirs".to_string(), None, false)]);
        assert_eq!(start.open_incidents.len(), 1);
        let opened = &start.open_incidents[0];
        assert_eq!(opened.reason, IncidentReason::UncleanStart);
        assert_eq!((opened.count, opened.first_generation), (1, generation));

        // No marker: the next start is unclean too, on the same incident.
        let next = start_epoch(&pool, tenant, "test").await.unwrap();
        let counted = &next.open_incidents[0];
        assert_eq!(next.open_incidents.len(), 1);
        assert_eq!(counted.count, 2);
        assert_eq!(counted.first_generation, generation);
        assert_eq!(counted.last_generation, generation_of(next.epoch.epoch_seq, 1));
    }

    /// A start after a version 2 marker is clean: it stamps nothing and opens
    /// nothing, but still loads an incident open on record, here one written
    /// after the marker.
    #[sqlx::test(migrations = "../migrations")]
    #[ignore = "requires DATABASE_URL"]
    async fn a_clean_start_opens_nothing_and_loads_what_is_still_open(pool: PgPool) {
        let tenant = Uuid::now_v7();
        let first = start_epoch(&pool, tenant, "test").await.unwrap();
        release_all(&pool, tenant).await;
        mark_clean_stop(&pool, first.epoch).await.unwrap();
        intent(&pool, tenant, "dead", None, None).await;
        sqlx::query(
            "INSERT INTO recovery_incidents (incident_id, tenant_id, reason, first_at, last_at, \
                 count, first_generation, last_generation) \
             VALUES ($1, $2, 'uncertain_outcome', NOW(), NOW(), 1, 7, 7)",
        )
        .bind(Uuid::now_v7())
        .bind(tenant)
        .execute(&pool)
        .await
        .unwrap();

        let clean = start_epoch(&pool, tenant, "test").await.unwrap();
        assert_eq!((clean.unclean_generation, clean.stamped_intents), (None, 0));
        assert_eq!(stamps(&pool, tenant).await, [("dead".to_string(), None, false)]);
        assert_eq!(clean.open_incidents.len(), 1);
        assert_eq!(clean.open_incidents[0].reason, IncidentReason::UncertainOutcome);
        assert_eq!(clean.open_incidents[0].count, 1);
    }

    /// Release every open incident of `tenant`, as a release's transaction
    /// does.
    async fn release_all(pool: &PgPool, tenant_id: Uuid) {
        let decision = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO journal_decisions (decision_id, kind, actor, auth_kind, note, payload) \
             VALUES ($1, 'release', 'test', 'verified', 'released', '{}')",
        )
        .bind(decision)
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            "UPDATE recovery_incidents SET released_at = NOW(), decision_id = $2 \
             WHERE tenant_id = $1 AND released_at IS NULL",
        )
        .bind(tenant_id)
        .bind(decision)
        .execute(pool)
        .await
        .unwrap();
    }

    /// An open incident of a reason this binary does not know refuses the
    /// start, and the epoch is not recorded.
    #[sqlx::test(migrations = "../migrations")]
    #[ignore = "requires DATABASE_URL"]
    async fn an_open_incident_of_an_unknown_reason_refuses_the_start(pool: PgPool) {
        let tenant = Uuid::now_v7();
        sqlx::query(
            "ALTER TABLE recovery_incidents DROP CONSTRAINT recovery_incidents_reason_check",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO recovery_incidents (incident_id, tenant_id, reason, first_at, last_at, \
                 count, first_generation, last_generation) \
             VALUES ($1, $2, 'from_the_future', NOW(), NOW(), 1, 1, 1)",
        )
        .bind(Uuid::now_v7())
        .bind(tenant)
        .execute(&pool)
        .await
        .unwrap();
        let refused = start_epoch(&pool, tenant, "test").await;
        assert!(matches!(refused, Err(DaemonError::Config(ref e)) if e.contains("does not know")));
        assert!(epochs(&pool, tenant).await.is_empty());
    }

    /// Review round 1: the marker is not written while an incident is open on
    /// record, even when nothing in memory says so; once it is released, it is.
    #[sqlx::test(migrations = "../migrations")]
    #[ignore = "requires DATABASE_URL"]
    async fn no_marker_is_written_over_an_open_incident_on_record(pool: PgPool) {
        let tenant = Uuid::now_v7();
        let start = start_epoch(&pool, tenant, "test").await.unwrap();
        assert_eq!(start.open_incidents.len(), 1, "no predecessor: unclean_start");

        let refused = mark_clean_stop(&pool, start.epoch).await;
        assert!(matches!(&refused, Err(e) if e.contains("incident")), "{refused:?}");
        assert_eq!(epochs(&pool, tenant).await, [(start.epoch.epoch_seq, false, None)]);

        release_all(&pool, tenant).await;
        mark_clean_stop(&pool, start.epoch).await.unwrap();
        assert_eq!(epochs(&pool, tenant).await, [(
            start.epoch.epoch_seq,
            true,
            Some(MARKER_VERSION)
        )]);
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
