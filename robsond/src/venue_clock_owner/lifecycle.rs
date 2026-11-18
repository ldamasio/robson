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
    failed: AtomicBool,
}

impl ClockOwner {
    pub(crate) fn spawn(self) -> Arc<ClockClosing> {
        let watchdog = tokio::spawn(super::watchdog::run(
            Arc::clone(&self.clock),
            self.epoch,
            Arc::clone(&self.state),
            self.cancel.clone(),
        ));
        let budget = self.budget.clone();
        let state = Arc::clone(&self.state);
        let cancel = self.cancel.clone();
        let worker = tokio::spawn(self.run());
        // No await or publication occurs before both real handles are owned.
        Arc::new(ClockClosing {
            budget,
            state,
            cancel,
            worker: AsyncMutex::new(Some(worker)),
            watchdog: AsyncMutex::new(Some(watchdog)),
            failed: AtomicBool::new(false),
        })
    }

    async fn run(mut self) -> bool {
        let mut ticks = tokio::time::interval(Duration::from_secs(60));
        ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                biased;
                _ = self.cancel.cancelled() => break,
                _ = ticks.tick() => {},
            }
            let result = self.run_cycle().await;
            metrics::cycle(result.as_ref().err().copied());
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
        self.retire_jobs().await && !self.budget.pending_pause()
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
            let mut worker = self.worker.lock().await;
            if let Some(task) = worker.as_mut() {
                if !matches!(task.await, Ok(true)) {
                    self.failed.store(true, Ordering::Release);
                }
                *worker = None;
            }
            drop(worker);
            let mut watcher = self.watchdog.lock().await;
            if let Some(task) = watcher.as_mut() {
                if !matches!(task.await, Ok(Ok(()))) {
                    self.failed.store(true, Ordering::Release);
                }
                *watcher = None;
            }
            !self.failed.load(Ordering::Acquire) && !self.budget.pending_pause()
        };
        // A timeout retains the actual join handles in their slots. It is
        // unresolved retirement, never evidence that the tasks disappeared.
        timeout_at(deadline, work).await == Ok(true)
    }
}
