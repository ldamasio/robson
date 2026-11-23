//! Concrete app-task retirement, before the existing journal final checks.
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::Mutex as AsyncMutex;

use super::*;

pub(crate) struct ClockClosing {
    budget: SharedBudget,
    state: Arc<Mutex<ClockState>>,
    cancel: CancellationToken,
    pub(super) worker: AsyncMutex<Option<JoinHandle<bool>>>,
    pub(super) watchdog: AsyncMutex<Option<JoinHandle<Result<(), TimeError>>>>,
    /// PS-2b's pause keeper of a non-trading owner.
    pub(super) keeper: AsyncMutex<Option<JoinHandle<bool>>>,
    failed: AtomicBool,
}

impl ClockOwner {
    pub(crate) fn spawn(mut self) -> Arc<ClockClosing> {
        let watchdog = tokio::spawn(super::watchdog::run(
            Arc::clone(&self.clock),
            self.epoch,
            Arc::clone(&self.state),
            self.cancel.clone(),
        ));
        let budget = self.budget.clone();
        let state = Arc::clone(&self.state);
        let cancel = self.cancel.clone();
        let keeper = self.keeper.take().map(|(pool, tenant)| {
            use crate::venue_budget::{pause_store, Trace};
            let clock = Arc::clone(&self.clock);
            let write = move |observed| {
                let (pool, clock) = (pool.clone(), Arc::clone(&clock));
                async move {
                    pause_store::persist(&pool, tenant, &observed, &clock, &Trace::default()).await
                }
            };
            tokio::spawn(pause_store::keep(budget.clone(), cancel.clone(), write))
        });
        let worker = tokio::spawn(self.run());
        // No await or publication occurs before both real handles are owned.
        Arc::new(ClockClosing {
            budget,
            state,
            cancel,
            worker: AsyncMutex::new(Some(worker)),
            watchdog: AsyncMutex::new(Some(watchdog)),
            keeper: AsyncMutex::new(keeper),
            failed: AtomicBool::new(false),
        })
    }

    async fn run(mut self) -> bool {
        let first = Instant::now() + self.first;
        let mut ticks = tokio::time::interval_at(first, Duration::from_secs(60));
        ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let requests = Arc::clone(&self.requests);
        while let Some(request) = next_cycle(&requests, &mut ticks, &self.cancel).await {
            let result = match &request {
                Some(request) => {
                    self.run_cycle_kind(CycleKind::OnDemand { deadline: request.deadline }).await
                },
                None => self.run_cycle().await,
            };
            metrics::cycle(result.as_ref().err().copied());
            metrics::bucket(self.budget.usage(|| self.clock.capture().ok()));
            if let Some(request) = request {
                request.answer(result);
            }
            // No subsequent cycle may overwrite a still-owned blocking job.
            // Cancellation cannot detach preparation or parser work.
            let retired = self.retire_jobs().await;
            if !retired {
                return false;
            }
            if matches!(result, Err(TimeError::Closing)) {
                break;
            }
        }
        // A pending pause is the join's to judge, after the keeper's join.
        self.retire_jobs().await
    }

    async fn retire_jobs(&mut self) -> bool {
        let preparation = jobs::retire(&mut self.preparation).await;
        let parser = jobs::retire(&mut self.parser).await;
        if !preparation || !parser {
            tracing::warn!(outcome = "resource", "TIME blocking-job retirement failed");
        }
        preparation && parser
    }
}

/// The sampler's next cycle, one at a time (PLAN §6.6): `None` once cancelled;
/// the minute's tick first, so requests never starve it; then a reader's
/// request, taken from its slot, which the caller answers after its cycle.
pub(super) async fn next_cycle(
    requests: &super::on_demand::Requests,
    ticks: &mut tokio::time::Interval,
    cancel: &CancellationToken,
) -> Option<Option<super::on_demand::Request>> {
    tokio::select! {
        biased;
        _ = cancel.cancelled() => None,
        _ = ticks.tick() => Some(None),
        request = requests.next() => Some(Some(request)),
    }
}

impl ClockClosing {
    /// Called synchronously BEFORE the journal gate changes to Closing.
    pub(crate) fn begin_closing(&self) {
        self.budget.begin_closing();
        match self.state.lock() {
            Ok(mut state) => {
                state.begin_closing();
            },
            Err(_) => self.failed.store(true, Ordering::Release),
        }
        self.cancel.cancel();
    }

    pub(crate) async fn join(&self, deadline: Instant) -> bool {
        let work = async {
            // The sampler, then PS-2b's pause keeper.
            for slot in [&self.worker, &self.keeper] {
                let mut task = slot.lock().await;
                if let Some(running) = task.as_mut() {
                    if !matches!(running.await, Ok(true)) {
                        self.failed.store(true, Ordering::Release);
                    }
                    *task = None;
                }
            }
            let mut watcher = self.watchdog.lock().await;
            if let Some(task) = watcher.as_mut() {
                if !matches!(task.await, Ok(Ok(()))) {
                    self.failed.store(true, Ordering::Release);
                }
                *watcher = None;
            }
            drop(watcher);
            !self.failed.load(Ordering::Acquire) && !self.budget.pending_pause()
        };
        // A timeout retains the actual join handles in their slots. It is
        // unresolved retirement, never evidence that the tasks disappeared.
        timeout_at(deadline, work).await == Ok(true)
    }
}
