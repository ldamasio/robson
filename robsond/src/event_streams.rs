//! The event-log streams the position manager writes: per-stream
//! serialization and suspension (ADR-0056 step 7, PR P).
//!
//! `persist_event_to_log` is the one production writer to `position:{id}` and
//! `system:month_boundary:YYYY-MM` streams. It holds the stream's async lock
//! through the suspension check, the append, the projection apply and, on a
//! failure, marking the stream suspended, so a writer queued behind a failure
//! sees the suspension before it appends. A lock entry lives only while it is
//! held or awaited.
//!
//! A suspended stream refuses appends for the life of the process: its
//! projection then lags its log by exactly the suffix that did not apply. The
//! suspension is recorded durably through the journal's retry set, which sets
//! the latch, and is loaded back at the next start. A position stream is then
//! repaired ([`repair_position_stream`]); a system stream stays suspended until
//! step 8, and its month's accounting is invalid, whether that month is current
//! or history and whatever a release does (design §2.5): the invalid-basis path
//! reads [`EventStreams::month_suspended`], held here, so it adds no query.
//!
//! Lock order (design §2.10): the manager lock, a stream's lock, the gate
//! mutex, the journal's memory mutex. The two registries here are sync leaves
//! as well: never held across an await, nor while another lock is taken.
//!
//! A build without PostgreSQL persists nothing, so nothing in it suspends.
#![cfg_attr(not(feature = "postgres"), allow(dead_code))]

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, MutexGuard, OnceLock},
};

use chrono::{NaiveDate, Utc};
use robson_domain::PositionId;
use robson_exec::order_journal::StreamSuspensionRow;
use tokio::sync::OwnedMutexGuard;
use uuid::Uuid;

/// Prefix of a position's stream.
pub(crate) const POSITION_STREAM_PREFIX: &str = "position:";
/// Prefix of a month's system stream.
pub(crate) const MONTH_STREAM_PREFIX: &str = "system:month_boundary:";

/// What a managed stream key names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ManagedStream {
    Position(PositionId),
    Month { year: i32, month: u32 },
}

impl ManagedStream {
    pub(crate) fn parse(stream_key: &str) -> Option<Self> {
        if let Some(id) = stream_key.strip_prefix(POSITION_STREAM_PREFIX) {
            return Uuid::parse_str(id).ok().map(Self::Position);
        }
        let (year, month) = stream_key.strip_prefix(MONTH_STREAM_PREFIX)?.split_once('-')?;
        let (year, month) = (year.parse().ok()?, month.parse().ok()?);
        NaiveDate::from_ymd_opt(year, month, 1)?;
        Some(Self::Month { year, month })
    }

    pub(crate) fn kind(self) -> &'static str {
        match self {
            Self::Position(_) => "position",
            Self::Month { .. } => "system",
        }
    }

    fn first_day(self) -> Option<NaiveDate> {
        match self {
            Self::Position(_) => None,
            Self::Month { year, month } => NaiveDate::from_ymd_opt(year, month, 1),
        }
    }
}

/// A stream that refuses appends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Suspended {
    pub(crate) suspension_id: Uuid,
    pub(crate) stream: ManagedStream,
    /// The last seq a startup repair applied (position streams).
    pub(crate) repaired_through_seq: Option<i64>,
}

struct StreamLock {
    mutex: Arc<tokio::sync::Mutex<()>>,
    /// Holders and waiters: the entry goes when the last of them does.
    users: usize,
}

/// The stream locks and the suspended streams of one position manager.
#[derive(Default)]
pub(crate) struct EventStreams {
    locks: Mutex<HashMap<String, StreamLock>>,
    suspended: Mutex<HashMap<String, Suspended>>,
    epoch: OnceLock<Uuid>,
}

/// A stream's lock, held until this drops. Dropped while still waiting, it
/// leaves nothing behind either.
pub(crate) struct StreamGuard<'a> {
    streams: &'a EventStreams,
    key: String,
    held: Option<OwnedMutexGuard<()>>,
}

impl Drop for StreamGuard<'_> {
    fn drop(&mut self) {
        self.held.take();
        let mut locks = self.streams.lock_table();
        if let Some(entry) = locks.get_mut(&self.key) {
            entry.users -= 1;
            if entry.users == 0 {
                locks.remove(&self.key);
            }
        }
    }
}

impl EventStreams {
    fn lock_table(&self) -> MutexGuard<'_, HashMap<String, StreamLock>> {
        self.locks.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn suspended_table(&self) -> MutexGuard<'_, HashMap<String, Suspended>> {
        self.suspended.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Wait for `stream_key`'s lock.
    pub(crate) async fn lock(&self, stream_key: &str) -> StreamGuard<'_> {
        let mutex = {
            let mut locks = self.lock_table();
            let entry = locks
                .entry(stream_key.to_string())
                .or_insert_with(|| StreamLock { mutex: Arc::default(), users: 0 });
            entry.users += 1;
            Arc::clone(&entry.mutex)
        };
        let mut guard = StreamGuard {
            streams: self,
            key: stream_key.to_string(),
            held: None,
        };
        guard.held = Some(mutex.lock_owned().await);
        guard
    }

    /// Stream locks held or awaited now.
    #[cfg(test)]
    pub(crate) fn lock_entries(&self) -> usize {
        self.lock_table().len()
    }

    /// Holders and waiters of `stream_key`'s lock: a waiter counts from the
    /// moment it registers, before it waits.
    #[cfg(test)]
    pub(crate) fn lock_users(&self, stream_key: &str) -> usize {
        self.lock_table().get(stream_key).map_or(0, |entry| entry.users)
    }

    /// The process epoch suspensions are recorded under.
    pub(crate) fn set_epoch(&self, epoch_id: Uuid) {
        let _ = self.epoch.set(epoch_id);
    }

    pub(crate) fn is_suspended(&self, stream_key: &str) -> bool {
        self.suspended_table().contains_key(stream_key)
    }

    /// Suspend `stream_key`, under its lock. The row to record durably, or
    /// `None` when it already was suspended or is no managed stream.
    pub(crate) fn suspend(
        &self,
        stream_key: &str,
        failed_seq: Option<i64>,
        error: &str,
    ) -> Option<StreamSuspensionRow> {
        let stream = ManagedStream::parse(stream_key)?;
        let suspension_id = Uuid::now_v7();
        {
            let mut suspended = self.suspended_table();
            if suspended.contains_key(stream_key) {
                return None;
            }
            suspended.insert(stream_key.to_string(), Suspended {
                suspension_id,
                stream,
                repaired_through_seq: None,
            });
            Self::publish(&suspended);
        }
        Some(StreamSuspensionRow {
            suspension_id,
            stream_key: stream_key.to_string(),
            stream_kind: stream.kind(),
            failed_seq,
            month: stream.first_day(),
            error: error.to_string(),
            epoch_id: self.epoch.get().copied(),
            suspended_at: Utc::now(),
        })
    }

    /// The suspensions on record, loaded at start.
    pub(crate) fn restore(&self, loaded: impl IntoIterator<Item = (String, Suspended)>) {
        let mut suspended = self.suspended_table();
        suspended.extend(loaded);
        Self::publish(&suspended);
    }

    /// A repaired stream takes appends again.
    pub(crate) fn clear(&self, stream_key: &str) {
        let mut suspended = self.suspended_table();
        suspended.remove(stream_key);
        Self::publish(&suspended);
    }

    /// A repair's checkpoint, kept so a later attempt in this process starts
    /// past it.
    pub(crate) fn note_repaired_through(&self, stream_key: &str, seq: i64) {
        if let Some(entry) = self.suspended_table().get_mut(stream_key) {
            entry.repaired_through_seq = Some(seq);
        }
    }

    pub(crate) fn position_suspended(&self, position_id: PositionId) -> bool {
        self.suspended_table()
            .values()
            .any(|entry| entry.stream == ManagedStream::Position(position_id))
    }

    /// Whether the system stream of this month is suspended: one loaded at
    /// start, which nothing clears before step 8, or one suspended since.
    pub(crate) fn month_suspended(&self, year: i32, month: u32) -> bool {
        self.suspended_table()
            .values()
            .any(|entry| entry.stream == ManagedStream::Month { year, month })
    }

    /// The suspended position streams, in key order.
    pub(crate) fn suspended_positions(&self) -> Vec<(String, Suspended)> {
        let mut streams: Vec<_> = self
            .suspended_table()
            .iter()
            .filter(|(_, entry)| matches!(entry.stream, ManagedStream::Position(_)))
            .map(|(key, entry)| (key.clone(), entry.clone()))
            .collect();
        streams.sort_by(|a, b| a.0.cmp(&b.0));
        streams
    }

    fn publish(suspended: &HashMap<String, Suspended>) {
        let positions = suspended
            .values()
            .filter(|entry| matches!(entry.stream, ManagedStream::Position(_)))
            .count();
        crate::metrics::observe_streams_suspended(positions, suspended.len() - positions);
    }
}

/// The suspensions of `tenant_id` no repair cleared.
#[cfg(feature = "postgres")]
pub(crate) async fn load_open_suspensions(
    pool: &sqlx::PgPool,
    tenant_id: Uuid,
) -> Result<Vec<(String, Suspended)>, sqlx::Error> {
    let rows = sqlx::query_as::<_, (Uuid, String, Option<i64>)>(
        "SELECT suspension_id, stream_key, repaired_through_seq FROM stream_suspensions \
         WHERE tenant_id = $1 AND cleared_at IS NULL ORDER BY suspended_at",
    )
    .bind(tenant_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(suspension_id, stream_key, repaired_through_seq)| {
            let stream = ManagedStream::parse(&stream_key)?;
            Some((stream_key, Suspended {
                suspension_id,
                stream,
                repaired_through_seq,
            }))
        })
        .collect())
}

/// Events a startup repair reads at once, in each pass (design §2.5).
#[cfg(feature = "postgres")]
pub(crate) const REPAIR_PAGE: i64 = 500;

/// How the startup repair of one suspended position stream ended.
#[cfg(feature = "postgres")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RepairOutcome {
    /// Its suffix applied in full; the stream takes appends again.
    Cleared { applied: u64 },
    /// An event type in its suffix has no guarded position handler: nothing
    /// was applied, and it stays suspended.
    Refused { seq: i64, event_type: String },
    /// An apply or a read failed partway; it stays suspended, its checkpoint
    /// where the last whole page left it.
    Failed { error: String },
}

/// Repair one suspended position stream (ADR-0056 step 7, PR P, design §2.5),
/// holding its lock throughout. Two passes, each 500 events a page:
///
/// 1. **Validate** the whole suffix past `positions_current.last_seq`, keeping
///    only the cursor: every event type must apply through a handler guarded by
///    the leg's `last_seq`, or be audit-only. Any other type means no repair at
///    all.
/// 2. **Apply** it in seq order, from past the later of that `last_seq` and the
///    checkpoint a previous attempt left, writing `repaired_through_seq` after
///    each page; then clear the suspension.
///
/// Any failure leaves the stream suspended, recorded in `repair_error`, and its
/// leg's snapshot invalid through the latent-failure path. The monthly
/// counters `EntryFilled` and `PositionClosed` add to are not idempotent: an
/// event re-applied whose position part was already applied adds again. They
/// drive neither the budget nor the API.
#[cfg(feature = "postgres")]
pub(crate) async fn repair_position_stream(
    pool: &sqlx::PgPool,
    tenant_id: Uuid,
    streams: &EventStreams,
    stream_key: &str,
    suspended: &Suspended,
) -> RepairOutcome {
    let _stream = streams.lock(stream_key).await;
    let outcome = match repair_under_lock(pool, tenant_id, streams, stream_key, suspended).await {
        Ok(outcome) => outcome,
        Err(error) => RepairOutcome::Failed { error: error.to_string() },
    };
    let recorded = match &outcome {
        RepairOutcome::Cleared { .. } => {
            sqlx::query(
                "UPDATE stream_suspensions SET cleared_at = NOW(), repair_error = NULL \
                 WHERE suspension_id = $1",
            )
            .bind(suspended.suspension_id)
            .execute(pool)
            .await
        },
        RepairOutcome::Refused { seq, event_type } => {
            record_repair_error(
                pool,
                suspended.suspension_id,
                &format!("seq {seq}: {event_type} has no guarded position handler"),
            )
            .await
        },
        RepairOutcome::Failed { error } => {
            record_repair_error(pool, suspended.suspension_id, error).await
        },
    };
    match (outcome, recorded) {
        (RepairOutcome::Cleared { applied }, Ok(_)) => {
            streams.clear(stream_key);
            RepairOutcome::Cleared { applied }
        },
        // Applied, but not cleared on record: it stays suspended, and the next
        // start validates and applies a suffix that is a no-op by then.
        (RepairOutcome::Cleared { .. }, Err(error)) => RepairOutcome::Failed {
            error: format!("clearing the suspension: {error}"),
        },
        (outcome, _) => outcome,
    }
}

#[cfg(feature = "postgres")]
async fn record_repair_error(
    pool: &sqlx::PgPool,
    suspension_id: Uuid,
    error: &str,
) -> Result<sqlx::postgres::PgQueryResult, sqlx::Error> {
    sqlx::query("UPDATE stream_suspensions SET repair_error = $2 WHERE suspension_id = $1")
        .bind(suspension_id)
        .bind(error)
        .execute(pool)
        .await
}

#[cfg(feature = "postgres")]
async fn repair_under_lock(
    pool: &sqlx::PgPool,
    tenant_id: Uuid,
    streams: &EventStreams,
    stream_key: &str,
    suspended: &Suspended,
) -> Result<RepairOutcome, sqlx::Error> {
    let ManagedStream::Position(position_id) = suspended.stream else {
        return Ok(RepairOutcome::Refused {
            seq: 0,
            event_type: "a system stream".to_string(),
        });
    };
    let projected = sqlx::query_scalar::<_, i64>(
        "SELECT last_seq FROM positions_current WHERE position_id = $1",
    )
    .bind(position_id)
    .fetch_optional(pool)
    .await?
    .unwrap_or(0);

    // 1. Validate, holding only the cursor.
    let mut cursor = projected;
    loop {
        let page = sqlx::query_as::<_, (i64, String)>(
            "SELECT seq, event_type FROM event_log \
             WHERE tenant_id = $1 AND stream_key = $2 AND seq > $3 ORDER BY seq LIMIT $4",
        )
        .bind(tenant_id)
        .bind(stream_key)
        .bind(cursor)
        .bind(REPAIR_PAGE)
        .fetch_all(pool)
        .await?;
        if let Some((seq, event_type)) = page
            .iter()
            .find(|(_, event_type)| !robson_projector::is_repairable_position_event(event_type))
        {
            return Ok(RepairOutcome::Refused {
                seq: *seq,
                event_type: event_type.clone(),
            });
        }
        match page.last() {
            Some((seq, _)) if page.len() as i64 == REPAIR_PAGE => cursor = *seq,
            _ => break,
        }
    }

    // 2. Apply, past what an interrupted attempt already did.
    let mut cursor = projected.max(suspended.repaired_through_seq.unwrap_or(0));
    let mut applied = 0;
    loop {
        let page = sqlx::query_as::<_, robson_eventlog::EventEnvelope>(
            "SELECT event_id, tenant_id, stream_key, seq, event_type, payload, \
             payload_schema_version, occurred_at, ingested_at, idempotency_key, \
             trace_id, causation_id, command_id, workflow_id, \
             actor_type, actor_id, prev_hash, hash \
             FROM event_log \
             WHERE tenant_id = $1 AND stream_key = $2 AND seq > $3 ORDER BY seq LIMIT $4",
        )
        .bind(tenant_id)
        .bind(stream_key)
        .bind(cursor)
        .bind(REPAIR_PAGE)
        .fetch_all(pool)
        .await?;
        let Some(last) = page.last().map(|envelope| envelope.seq) else {
            break;
        };
        for envelope in &page {
            if let Err(error) = robson_projector::apply_event_to_projections(pool, envelope).await {
                return Ok(RepairOutcome::Failed {
                    error: format!("seq {}: {}: {error}", envelope.seq, envelope.event_type),
                });
            }
            applied += 1;
        }
        sqlx::query(
            "UPDATE stream_suspensions SET repaired_through_seq = $2 WHERE suspension_id = $1",
        )
        .bind(suspended.suspension_id)
        .bind(last)
        .execute(pool)
        .await?;
        streams.note_repaired_through(stream_key, last);
        robson_exec::crash_point!(REPAIR_AFTER_PAGE);
        cursor = last;
        if (page.len() as i64) < REPAIR_PAGE {
            break;
        }
    }
    Ok(RepairOutcome::Cleared { applied })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_managed_stream_key_names_its_position_or_its_month() {
        let id = Uuid::now_v7();
        assert_eq!(
            ManagedStream::parse(&format!("position:{id}")),
            Some(ManagedStream::Position(id))
        );
        assert_eq!(
            ManagedStream::parse("system:month_boundary:2026-11"),
            Some(ManagedStream::Month { year: 2026, month: 11 })
        );
        for other in [
            "position:x",
            "system:month_boundary:2026-13",
            "robson:daemon",
            "funding:x",
        ] {
            assert_eq!(ManagedStream::parse(other), None, "{other}");
        }
    }

    #[test]
    fn a_stream_is_suspended_once_and_its_row_says_what_it_is() {
        let streams = EventStreams::default();
        let epoch = Uuid::now_v7();
        streams.set_epoch(epoch);
        let row = streams.suspend("system:month_boundary:2026-11", None, "apply failed").unwrap();
        assert_eq!(row.stream_kind, "system");
        assert_eq!(row.month, NaiveDate::from_ymd_opt(2026, 11, 1));
        assert_eq!(row.epoch_id, Some(epoch));
        assert!(streams.suspend("system:month_boundary:2026-11", Some(3), "again").is_none());
        assert!(streams.month_suspended(2026, 11));
        assert!(!streams.month_suspended(2026, 12));

        let id = Uuid::now_v7();
        let row = streams.suspend(&format!("position:{id}"), Some(7), "append failed").unwrap();
        assert_eq!((row.stream_kind, row.failed_seq, row.month), ("position", Some(7), None));
        assert!(streams.position_suspended(id));
        assert!(!streams.position_suspended(Uuid::now_v7()));
        assert_eq!(streams.suspended_positions().len(), 1);
        streams.clear(&format!("position:{id}"));
        assert!(!streams.position_suspended(id));
    }

    /// A lock entry goes when its last holder or waiter does, a waiter dropped
    /// mid-wait included.
    #[tokio::test]
    async fn idle_stream_locks_are_dropped() {
        let streams = EventStreams::default();
        let held = streams.lock("position:a").await;
        assert_eq!(streams.lock_entries(), 1);
        let waiting =
            tokio::time::timeout(std::time::Duration::from_millis(20), streams.lock("position:a"))
                .await;
        assert!(waiting.is_err(), "the lock is held");
        drop(held);
        assert_eq!(streams.lock_entries(), 0);

        let a = streams.lock("position:a").await;
        let b = streams.lock("position:b").await;
        assert_eq!(streams.lock_entries(), 2);
        drop((a, b));
        assert_eq!(streams.lock_entries(), 0);
    }
}
