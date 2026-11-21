//! Concrete TIME owner. Its one production construction is a non-trading
//! Spot start's (PLAN §6.6, PS-3), on that start's non-trading budget; the
//! trading C2/final-source context stays unavailable.
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use robson_connectors::{binance::spot::PreparedSpotTime, BinanceRestClient, BinanceRestError};
use robson_exec::{
    clock_records::{RawClockCapture, RawClockSample, SampleArithmetic},
    venue::VenueId,
    venue_clock::{NativeClock, TickCapture},
};
use tokio::{
    sync::Semaphore,
    task::JoinHandle,
    time::{timeout_at, Instant},
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    process_epoch::EpochStart,
    venue_budget::{
        state::{NonTradingBudget, SharedBudget},
        BudgetError,
    },
    venue_clock_http::{self, TimeError},
    venue_clock_store::ClockSampleStore,
};

mod clock_state;
mod commission;
pub(crate) use commission::CommissionPreparation;
#[cfg(all(test, feature = "crash-points"))]
mod inventory;
mod jobs;
mod lifecycle;
mod metrics;
mod on_demand;
pub(crate) use on_demand::ClockHandle;
#[cfg(all(test, feature = "crash-points"))]
mod pg_tests;
#[cfg(all(test, feature = "crash-points"))]
pub(crate) use pg_tests::reconciliation::{
    AccountSeen, Hook, Listener, ReconciliationRig, ScanInputs, Scanned,
};
#[cfg(all(test, feature = "crash-points"))]
pub(crate) use pg_tests::restart_lookup::{GovernedRestartRead, RestartLookup};
#[cfg(all(test, feature = "crash-points"))]
pub(crate) use pg_tests::run_held_inventory_rehearsal;
mod watchdog;
pub(crate) use lifecycle::ClockClosing;

struct Published {
    id: Uuid,
    arithmetic: SampleArithmetic,
    u2: i128,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum ClockFault {
    Native        = 1,
    WallStep      = 2,
    NoValidSample = 4,
    StaleSample   = 8,
}

pub(crate) struct ClockState {
    generation: u64,
    closing: bool,
    published: Option<Published>,
    start: TickCapture,
    last_valid: Option<TickCapture>,
    faults: u8,
    active_faults: u8,
}


pub(crate) struct ClockOwner {
    clock: Arc<NativeClock>,
    client: Arc<BinanceRestClient>,
    store: ClockSampleStore,
    budget: SharedBudget,
    tenant: Uuid,
    epoch: Uuid,
    state: Arc<Mutex<ClockState>>,
    cancel: CancellationToken,
    preparation: Option<JoinHandle<Result<PreparedSpotTime, BinanceRestError>>>,
    parser:
        Option<JoinHandle<Result<(robson_exec::clock_records::RawClockCapture, i64), TimeError>>>,
    workers: Arc<Semaphore>,
    cycle: u64,
    last_opportunity: Option<TickCapture>,
    /// The latest on-demand cycle's handoff bound: at most its response.
    last_on_demand: Option<Instant>,
    /// Delay of the first scheduled cycle from the sampler's spawn.
    first: Duration,
    requests: Arc<on_demand::Requests>,
}

/// What a TIME cycle is for (PLAN §6.6): the minute's scheduled sample, or one
/// a reader asked for, bounded by its caller's absolute deadline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CycleKind {
    Scheduled,
    OnDemand { deadline: Instant },
}

fn after(tick: TickCapture, duration: Duration) -> Result<TickCapture, TimeError> {
    let n = tick.nanoseconds().checked_add(duration.as_nanos()).ok_or(TimeError::Native)?;
    TickCapture::new(
        tick.domain(),
        i64::try_from(n / 1_000_000_000).map_err(|_| TimeError::Native)?,
        i64::try_from(n % 1_000_000_000).map_err(|_| TimeError::Native)?,
    )
    .map_err(|_| TimeError::Native)
}

impl ClockOwner {
    /// Private controlled component constructor; SharedBudget derives from
    /// actual bootstrap rows, not an initialization/qualification boolean.
    pub(crate) fn controlled(
        clock: Arc<NativeClock>,
        client: Arc<BinanceRestClient>,
        store: ClockSampleStore,
        budget: SharedBudget,
        tenant: Uuid,
        epoch: Uuid,
    ) -> Result<Self, TimeError> {
        let start = budget.start().map_err(TimeError::Budget)?;
        Ok(Self {
            clock,
            client,
            store,
            budget,
            tenant,
            epoch,
            state: Arc::new(Mutex::new(ClockState {
                generation: 0,
                closing: false,
                published: None,
                start,
                last_valid: None,
                faults: 0,
                active_faults: 0,
            })),
            cancel: CancellationToken::new(),
            preparation: None,
            parser: None,
            workers: Arc::new(Semaphore::new(1)),
            cycle: 0,
            last_opportunity: None,
            last_on_demand: None,
            first: Duration::ZERO,
            requests: Arc::default(),
        })
    }

    /// The production owner of a non-trading Spot start (PLAN §6.6), built
    /// after its budget bootstrap: the store bound to the committed epoch, the
    /// budget's TIME handle, and the first scheduled cycle at start + 61 s
    /// (+ 121 s after an unclean start), the first venue request.
    pub(crate) fn non_trading(
        clock: Arc<NativeClock>,
        client: Arc<BinanceRestClient>,
        pool: &sqlx::PgPool,
        tenant: Uuid,
        start: &EpochStart,
        budget: &NonTradingBudget,
    ) -> Result<Self, TimeError> {
        metrics::register();
        let store = ClockSampleStore::from_committed_epoch(pool, tenant, start);
        let mut owner = Self::controlled(
            clock,
            client,
            store,
            budget.time_owner(),
            tenant,
            start.epoch.epoch_id,
        )?;
        let unclean = start.predecessor.unclean_reason().is_some();
        let now = owner.clock.capture().map_err(|_| TimeError::Native)?;
        owner.first = first_delay(budget.start().map_err(TimeError::Budget)?, now, unclean)?;
        Ok(owner)
    }

    /// A clonable handle for readers: on-demand samples and clock evidence.
    pub(crate) fn handle(&self) -> ClockHandle {
        ClockHandle {
            clock: Arc::clone(&self.clock),
            epoch: self.epoch,
            state: Arc::clone(&self.state),
            cancel: self.cancel.clone(),
            requests: Arc::clone(&self.requests),
        }
    }

    pub(crate) async fn run_cycle(&mut self) -> Result<Uuid, TimeError> {
        self.run_cycle_kind(CycleKind::Scheduled).await
    }

    pub(crate) async fn run_cycle_kind(&mut self, kind: CycleKind) -> Result<Uuid, TimeError> {
        if self.preparation.is_some() || self.parser.is_some() {
            return Err(TimeError::Resource);
        }
        if self.cancel.is_cancelled() {
            return Err(TimeError::Closing);
        }
        let (admitted, generation) = {
            let mut state = self.state.lock().map_err(|_| TimeError::Native)?;
            if state.closing {
                return Err(TimeError::Closing);
            }
            let capture = state.capture_native(&self.clock, self.epoch)?;
            (capture.end_tick().map_err(|_| TimeError::Native)?, state.generation)
        };
        if matches!(kind, CycleKind::Scheduled)
            && self.last_opportunity.is_some_and(|last| {
                admitted.elapsed_since(&last).map_or(true, |age| age < Duration::from_secs(60))
            })
        {
            return Err(TimeError::Budget(BudgetError::Deferred));
        }
        // Only a scheduled cycle keeps the 60 s opportunity spacing; an
        // on-demand one is refused, before any reservation, once its caller's
        // deadline has passed or within 5 s of the previous on-demand handoff
        // (`last_on_demand`: monotonic, never earlier than that handoff).
        let limit = match kind {
            CycleKind::Scheduled => None,
            CycleKind::OnDemand { deadline } if Instant::now() >= deadline => {
                return Err(TimeError::Expired);
            },
            CycleKind::OnDemand { .. }
                if self
                    .last_on_demand
                    .is_some_and(|last| last.elapsed() < Duration::from_secs(5)) =>
            {
                return Err(TimeError::Budget(BudgetError::Deferred));
            },
            CycleKind::OnDemand { deadline } => Some(deadline),
        };
        // A paused/refused opportunity is skipped, never retried in the same
        // minute. The run loop also skips missed ticks rather than replaying.
        if limit.is_none() {
            self.last_opportunity = Some(admitted);
        }
        let http_deadline = bounded(limit, Instant::now() + Duration::from_secs(10));
        // `now` under the budget lock (PS-5): readers reserve beside this
        // cycle. `admitted` stays for the spacing rule and the persistence
        // bound (the cycle's remaining 60 s), not for the reservation.
        let clock = &self.clock;
        let pending = self
            .budget
            .reserve_now(1, || {
                let now = clock.capture().map_err(|_| BudgetError::Invalid)?;
                Ok((now, after(now, Duration::from_secs(10)).map_err(|_| BudgetError::Invalid)?))
            })
            .map_err(TimeError::Budget)?;
        if limit.is_some() {
            self.last_on_demand = Some(Instant::now());
        }
        self.cycle = self.cycle.checked_add(1).ok_or(TimeError::Resource)?;
        // The immutable send mark includes synchronous preparation time.
        // Capture after reservation and BEFORE spawning preparation.
        let send = {
            let mut state = self.state.lock().map_err(|_| TimeError::Native)?;
            let capture = state.capture_native(&self.clock, self.epoch)?;
            if state.closing || state.generation != generation {
                return Err(TimeError::Expired);
            }
            capture
        };
        let permit =
            Arc::clone(&self.workers).try_acquire_owned().map_err(|_| TimeError::Resource)?;
        let client = Arc::clone(&self.client);
        self.preparation = Some(tokio::task::spawn_blocking(move || {
            let _permit = permit;
            client.prepare_spot_time()
        }));
        let prepared = jobs::complete(&mut self.preparation, http_deadline, &self.cancel, |_| {
            TimeError::Transport
        })
        .await?;
        {
            let mut state = self.state.lock().map_err(|_| TimeError::Native)?;
            state.capture_native(&self.clock, self.epoch)?;
            if state.closing || state.generation != generation || Instant::now() >= http_deadline {
                return Err(TimeError::Expired);
            }
        }
        let body = venue_clock_http::receive(
            prepared,
            &self.budget,
            pending,
            &self.clock,
            self.epoch,
            http_deadline,
            &self.cancel,
        )
        .await;
        if limit.is_some() {
            // Whatever the answer, a handoff it had is no later than now.
            self.last_on_demand = Some(Instant::now());
        }
        let body = body?;
        {
            let mut state = self.state.lock().map_err(|_| TimeError::Native)?;
            state.capture_native(&self.clock, self.epoch)?;
            if state.closing || state.generation != generation {
                return Err(TimeError::Expired);
            }
        }
        let permit =
            Arc::clone(&self.workers).try_acquire_owned().map_err(|_| TimeError::Resource)?;
        self.parser = Some(tokio::task::spawn_blocking(move || {
            let _permit = permit;
            body.decode()
        }));
        let (receive, server_ms) =
            jobs::complete(&mut self.parser, http_deadline, &self.cancel, |error| error).await?;
        // No response/candidate is queued or exposed for a later cycle.
        let id = Uuid::now_v7();
        let raw = RawClockSample {
            id,
            tenant: self.tenant,
            venue: VenueId::new("binance:spot"),
            send,
            receive,
            server_ms,
        };
        let arithmetic = raw.arithmetic().map_err(|_| TimeError::Native)?;
        let numbers = raw.numbers().map_err(|_| TimeError::Native)?;
        metrics::uncertainty(numbers.u2);
        let original_valid = numbers.failures == 0;
        let now = {
            let mut state = self.state.lock().map_err(|_| TimeError::Native)?;
            state
                .capture_native(&self.clock, self.epoch)?
                .end_tick()
                .map_err(|_| TimeError::Native)?
        };
        let now_ns = i128::try_from(now.nanoseconds()).map_err(|_| TimeError::Native)?;
        let remaining_age = 60_000_000_000i128
            .checked_sub(now_ns.checked_sub(receive.before_ns).ok_or(TimeError::Native)?)
            .ok_or(TimeError::Expired)?;
        let remaining_cycle = Duration::from_secs(60)
            .checked_sub(now.elapsed_since(&admitted).map_err(|_| TimeError::Native)?)
            .ok_or(TimeError::Expired)?;
        let remaining_age =
            Duration::from_nanos(u64::try_from(remaining_age).map_err(|_| TimeError::Expired)?);
        let persist_for = Duration::from_secs(2).min(remaining_age).min(remaining_cycle);
        let deadline = bounded(limit, Instant::now() + persist_for);
        // Every poll/resumption observes actual native state. Cancellation
        // drops this one owned persistence future, never creates a revisit.
        let mut persist = Box::pin(self.store.persist(self.cycle, raw, Some(deadline)));
        let result = std::future::poll_fn(|cx| {
            use std::task::Poll;
            if self.cancel.is_cancelled() {
                return Poll::Ready(Err(TimeError::Closing));
            }
            let mut state = match self.state.lock() {
                Ok(state) => state,
                Err(_) => return Poll::Ready(Err(TimeError::Native)),
            };
            let capture = match state.capture_native(&self.clock, self.epoch) {
                Ok(capture) => capture,
                Err(error) => return Poll::Ready(Err(error)),
            };
            if state.closing {
                return Poll::Ready(Err(TimeError::Closing));
            }
            if state.generation != generation
                || Instant::now() >= deadline
                || !capture
                    .after_ns
                    .checked_sub(receive.before_ns)
                    .is_some_and(|age| (0..=60_000_000_000).contains(&age))
            {
                return Poll::Ready(Err(TimeError::Expired));
            }
            if receive.wall_agrees(&capture) != Ok(true) {
                return Poll::Ready(Err(TimeError::Native));
            }
            drop(state);
            std::future::Future::poll(persist.as_mut(), cx)
                .map(|result| result.map_err(TimeError::Storage))
        })
        .await;
        drop(persist);
        let _positive = result?;
        let mut state = self.state.lock().map_err(|_| TimeError::Native)?;
        if !original_valid {
            state.invalidate(ClockFault::StaleSample);
        }
        // Fresh capture is taken AFTER acquiring the decision lock.
        let capture = state.capture_native(&self.clock, self.epoch)?;
        if !original_valid
            || Instant::now() >= deadline
            || !arithmetic.fits_capture(&capture).map_err(|_| TimeError::Native)?
        {
            return Err(TimeError::Expired);
        }
        let received = receive.end_tick().map_err(|_| TimeError::Native)?;
        state.publish(generation, Published { id, arithmetic, u2: numbers.u2 }, received)
    }
}

/// A cycle's bound `at`, or its on-demand caller's deadline when earlier:
/// both the HTTP request and the persistence end by the caller's deadline.
fn bounded(limit: Option<Instant>, at: Instant) -> Instant {
    limit.map_or(at, |deadline| at.min(deadline))
}

/// How long after spawn the first scheduled cycle comes: start + 61 s, or
/// start + 121 s after an unclean start (PLAN §6.6), never before the
/// budget's grant window (60 s) or unclean barrier (120 s) ends.
fn first_delay(start: TickCapture, now: TickCapture, unclean: bool) -> Result<Duration, TimeError> {
    let grant = Duration::from_secs(if unclean { 121 } else { 61 });
    Ok(grant.saturating_sub(now.elapsed_since(&start).map_err(|_| TimeError::Native)?))
}

#[cfg(all(
    test,
    target_os = "linux",
    target_env = "gnu",
    target_pointer_width = "64",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
mod state_tests;

// PS-3: the request slot, publication CAS and caller deadline (no I/O), and
// the owner on PG16 with a loopback TIME venue (crash-points harness URL).
#[cfg(all(test, feature = "crash-points"))]
mod ps3_pg_tests;
#[cfg(all(test, target_os = "linux"))]
mod ps3_tests;
