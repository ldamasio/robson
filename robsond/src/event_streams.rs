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
//! repaired; a system stream stays suspended until step 8, and while its month
//! is the current one admissions stay closed (the fence PR replaces that rule
//! with its accounting contract and release).
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

use chrono::{Datelike, NaiveDate, Utc};
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

    pub(crate) fn position_suspended(&self, position_id: PositionId) -> bool {
        self.suspended_table()
            .values()
            .any(|entry| entry.stream == ManagedStream::Position(position_id))
    }

    /// Whether the system stream of this month is suspended.
    pub(crate) fn month_suspended(&self, year: i32, month: u32) -> bool {
        self.suspended_table()
            .values()
            .any(|entry| entry.stream == ManagedStream::Month { year, month })
    }

    /// Whether the current UTC month's system stream is suspended.
    pub(crate) fn current_month_suspended(&self) -> Option<(i32, u32)> {
        let now = Utc::now();
        self.month_suspended(now.year(), now.month()).then(|| (now.year(), now.month()))
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
