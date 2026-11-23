//! Bounded response restrictions. Values never replenish local capacity.
use std::{sync::LazyLock, time::Duration};

use prometheus::{register_int_gauge, IntGauge};
static LEVEL: LazyLock<IntGauge> = LazyLock::new(|| {
    register_int_gauge!(
        "robsond_spot_response_restriction_level",
        "Response restriction severity:0none1steady2protective_only_page3all_blocked"
    )
    .expect("register spot response restriction level")
});

use chrono::{DateTime, FixedOffset};
use reqwest::header::HeaderMap;
use robson_exec::{
    clock_records::{RawClockCapture, TICK_MAX},
    venue_clock::TickCapture,
};
use tokio::sync::Notify;
use uuid::Uuid;

use super::BudgetError;

/// The latest 418/429 a pause row must cover (PLAN §6.5): a fresh UUIDv7, its
/// status, the head capture's end tick (its clock domain), the checked
/// duration in seconds and an audit wall-clock end. Replaced only by one that
/// ends no earlier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PauseObservation {
    pub(crate) id: Uuid,
    pub(crate) status: i16,
    pub(crate) observed: TickCapture,
    pub(crate) seconds: u64,
    pub(crate) end: TickCapture,
    pub(crate) until: Option<DateTime<chrono::Utc>>,
}

/// Registers the restriction level at start, at 0 (PLAN §6.11).
pub(crate) fn register_level() {
    LazyLock::force(&LEVEL);
}

#[derive(Clone, Copy)]
struct Observation {
    severity: u8,
    end: TickCapture,
}
impl Observation {
    fn live(self, now: TickCapture) -> bool {
        now.elapsed_since(&self.end).is_err()
    }
    fn merge(&mut self, other: Self) {
        self.severity = self.severity.max(other.severity);
        if other.end.elapsed_since(&self.end).is_ok() {
            self.end = other.end;
        }
    }
}

#[derive(Default)]
pub(super) struct Heads {
    minutes: [Option<(i64, Observation)>; 3],
    fallback: Option<Observation>,
    orders: Option<Observation>,
    pause: Option<TickCapture>,
    unresolved: bool,
    pending: bool,
    observation: Option<PauseObservation>,
    /// Wakes the pause keeper at each new observation.
    pub(super) wake: std::sync::Arc<Notify>,
}

fn after(now: TickCapture, seconds: u64) -> Result<TickCapture, BudgetError> {
    let n = now
        .nanoseconds()
        .checked_add(Duration::from_secs(seconds).as_nanos())
        .ok_or(BudgetError::Invalid)?;
    if n > u128::try_from(TICK_MAX).map_err(|_| BudgetError::Invalid)? {
        return Err(BudgetError::Invalid);
    }
    TickCapture::new(
        now.domain(),
        i64::try_from(n / 1_000_000_000).map_err(|_| BudgetError::Invalid)?,
        i64::try_from(n % 1_000_000_000).map_err(|_| BudgetError::Invalid)?,
    )
    .map_err(|_| BudgetError::Invalid)
}
fn date(headers: &HeaderMap, name: &str) -> Option<DateTime<FixedOffset>> {
    DateTime::parse_from_rfc2822(headers.get(name)?.to_str().ok()?).ok()
}
fn severity(headers: &HeaderMap, name: &str, limit: u64) -> u8 {
    let Some(text) = headers.get(name).and_then(|value| value.to_str().ok()) else {
        return 0;
    };
    let value = match text.parse::<u64>() {
        Ok(value) => value,
        // Present malformed/overflowing usage is never evidence of headroom.
        Err(_) => return 3,
    };
    let ratio = u128::from(value) * 100;
    if ratio >= u128::from(limit) * 95 {
        3
    } else if ratio >= u128::from(limit) * 80 {
        2
    } else if ratio >= u128::from(limit) * 50 {
        1
    } else {
        0
    }
}
fn coalesce(slot: &mut Option<Observation>, value: Observation) {
    match slot {
        Some(current) => current.merge(value),
        None => *slot = Some(value),
    }
}

impl Heads {
    pub(super) fn observe(
        &mut self,
        status: u16,
        headers: &HeaderMap,
        head: Option<RawClockCapture>,
    ) {
        let observed_level = if matches!(status, 418 | 429) {
            3
        } else {
            severity(headers, "x-mbx-used-weight-1m", 6000)
                .max(severity(headers, "x-mbx-order-count-10s", 100))
                .max(severity(headers, "x-mbx-order-count-1d", 200000))
        };
        LEVEL.set(LEVEL.get().max(i64::from(observed_level)));
        if matches!(status, 418 | 429) {
            self.pending = true;
            // Every 418/429 wakes the keeper, a shorter one included.
            self.wake.notify_one();
        }
        let Some(head) = head else {
            if matches!(status, 418 | 429)
                || severity(headers, "x-mbx-used-weight-1m", 6000) > 0
                || severity(headers, "x-mbx-order-count-10s", 100) > 0
                || severity(headers, "x-mbx-order-count-1d", 200000) > 0
            {
                self.unresolved = true;
            }
            return;
        };
        let Ok(now) = head.end_tick() else {
            self.unresolved = true;
            return;
        };
        self.expire(now);
        let weight = severity(headers, "x-mbx-used-weight-1m", 6000);
        if weight > 0 {
            let Ok(end) = after(now, 120) else {
                self.unresolved = true;
                return;
            };
            let observation = Observation { severity: weight, end };
            // I1 supplies no validated C8 conversion to this observer. Even a
            // retained Date identity therefore receives the full120s, once.
            let minute = date(headers, "date")
                .filter(|date| {
                    head.wall_seconds
                        .checked_sub(date.timestamp())
                        .is_some_and(|age| (0..=120).contains(&age))
                })
                .map(|date| date.timestamp().div_euclid(60));
            match minute {
                Some(minute) => {
                    if let Some((_, slot)) =
                        self.minutes.iter_mut().flatten().find(|(id, _)| *id == minute)
                    {
                        slot.merge(observation);
                    } else if let Some(slot) = self.minutes.iter_mut().find(|slot| slot.is_none()) {
                        *slot = Some((minute, observation));
                    } else {
                        coalesce(&mut self.fallback, observation);
                    }
                },
                None => coalesce(&mut self.fallback, observation),
            }
        }
        let orders = severity(headers, "x-mbx-order-count-10s", 100).max(severity(
            headers,
            "x-mbx-order-count-1d",
            200000,
        ));
        if orders > 0 {
            match after(now, 10) {
                Ok(end) => coalesce(&mut self.orders, Observation { severity: orders, end }),
                Err(_) => self.unresolved = true,
            }
        }
        if matches!(status, 418 | 429) {
            let text = headers.get("retry-after").and_then(|value| value.to_str().ok());
            let seconds = match text {
                Some(text) if !text.is_empty() && text.bytes().all(|c| c.is_ascii_digit()) => {
                    text.parse::<u64>().ok()
                },
                Some(_) => date(headers, "retry-after")
                    .zip(date(headers, "date"))
                    .and_then(|(until, start)| until.timestamp().checked_sub(start.timestamp()))
                    .and_then(|seconds| u64::try_from(seconds).ok())
                    .and_then(|seconds| seconds.checked_add(1))
                    .map(|seconds| seconds.max(60)),
                None => Some(60),
            };
            match seconds.and_then(|seconds| Some((seconds, after(now, seconds).ok()?))) {
                Some((seconds, end)) => match self.pause {
                    Some(old) if end.elapsed_since(&old).is_err() => {},
                    _ => {
                        self.pause = Some(end);
                        let wall = head.wall_seconds.saturating_add_unsigned(seconds);
                        self.observation = Some(PauseObservation {
                            id: Uuid::now_v7(),
                            status: i16::try_from(status).unwrap_or(429),
                            observed: now,
                            seconds,
                            end,
                            until: DateTime::from_timestamp(wall, 0),
                        });
                    },
                },
                None => self.unresolved = true,
            }
        }
    }
    fn expire(&mut self, now: TickCapture) {
        for slot in &mut self.minutes {
            if slot.is_some_and(|(_, value)| !value.live(now)) {
                *slot = None;
            }
        }
        for slot in [&mut self.fallback, &mut self.orders] {
            if slot.is_some_and(|value| !value.live(now)) {
                *slot = None;
            }
        }
    }
    pub(super) fn permits_steady(&self, now: TickCapture) -> bool {
        let level = self
            .minutes
            .iter()
            .flatten()
            .map(|(_, value)| *value)
            .chain([self.fallback, self.orders].into_iter().flatten())
            .filter(|value| value.live(now))
            .map(|value| value.severity)
            .max()
            .unwrap_or(0);
        // A landed 418/429 still blocks every send, so still pages, to its end.
        let paused = self.pause.is_some_and(|end| now.elapsed_since(&end).is_err());
        LEVEL.set(if self.pending || self.unresolved || paused {
            3
        } else {
            i64::from(level)
        });
        !self.unresolved
            && !self.pending
            && self.pause.is_none_or(|end| now.elapsed_since(&end).is_ok())
            && !self
                .minutes
                .iter()
                .flatten()
                .any(|(_, value)| value.severity > 0 && value.live(now))
            && ![self.fallback, self.orders]
                .into_iter()
                .flatten()
                .any(|value| value.severity > 0 && value.live(now))
    }
    pub(super) fn pending(&self) -> bool {
        self.pending || self.unresolved
    }
    /// The observation the keeper must persist while a 418/429 is pending.
    pub(super) fn pending_observation(&self) -> Option<PauseObservation> {
        self.observation.filter(|_| self.pending)
    }
    /// A landed row covering `id` clears the pending pause only while `id` is
    /// still the latest observation; `unresolved` is never cleared.
    pub(super) fn acknowledge(&mut self, id: Uuid) {
        if self.observation.is_some_and(|observation| observation.id == id) {
            self.pending = false;
        }
    }
}
