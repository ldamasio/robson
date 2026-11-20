//! The actual application owns reservation lifetimes; numerical transitions
//! alone never supply the still-missing production startup proof.
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use robson_exec::{
    request_budget::{
        BudgetTransitions, Class, PendingCharge, RequestCostCandidate, TransitionError,
        UnqualifiedDailyCount,
    },
    venue_clock::{NativeClock, TickCapture},
};

use super::{pause_value::PauseValue, BootstrapRows, BudgetError};

fn transition(error: TransitionError) -> BudgetError {
    match error {
        TransitionError::Invalid => BudgetError::Invalid,
        TransitionError::Expired => BudgetError::Expired,
        TransitionError::Deferred => BudgetError::Deferred,
    }
}

struct State {
    model: BudgetTransitions,
    start: TickCapture,
    unclean: bool,
    daily: u64,
    pause: Option<PauseValue>,
    protective_waiters: usize,
    handoffs: u64,
    closing: bool,
    heads: super::heads::Heads,
}

#[derive(Clone)]
pub(crate) struct SharedBudget(Arc<Mutex<State>>);

pub(crate) struct PendingTime {
    owner: SharedBudget,
    charge: Option<PendingCharge>,
}

impl Drop for PendingTime {
    fn drop(&mut self) {
        if let Some(charge) = self.charge.take() {
            if let Ok(mut state) = self.owner.0.lock() {
                let _ = state.model.cancel(charge);
            }
        }
    }
}

pub(crate) struct ProtectiveWaiter(SharedBudget);
impl Drop for ProtectiveWaiter {
    fn drop(&mut self) {
        if let Ok(mut state) = self.0 .0.lock() {
            state.protective_waiters = state.protective_waiters.saturating_sub(1);
        }
    }
}

impl SharedBudget {
    /// Registration describes a real caller waiting for protection; it grants
    /// no request/J authorization. The later operated-client factory must use
    /// this same handle, never an independently created TIME limiter.
    pub(crate) fn protective_waiter(&self) -> Result<ProtectiveWaiter, BudgetError> {
        let mut state = self.0.lock().map_err(|_| BudgetError::Invalid)?;
        if state.closing {
            return Err(BudgetError::Closing);
        }
        state.protective_waiters =
            state.protective_waiters.checked_add(1).ok_or(BudgetError::Invalid)?;
        Ok(ProtectiveWaiter(self.clone()))
    }

    pub(crate) fn handoffs(&self) -> Result<u64, BudgetError> {
        self.0.lock().map(|state| state.handoffs).map_err(|_| BudgetError::Invalid)
    }
    pub(crate) fn from_rows(rows: BootstrapRows, unclean: bool) -> Result<Self, BudgetError> {
        let model = BudgetTransitions::unknown_history(rows.start).map_err(transition)?;
        let daily = u64::try_from(rows.active.len()).map_err(|_| BudgetError::Invalid)?;
        Ok(Self(Arc::new(Mutex::new(State {
            model,
            start: rows.start,
            unclean,
            daily,
            pause: rows.pause,
            protective_waiters: 0,
            handoffs: 0,
            closing: false,
            heads: super::heads::Heads::default(),
        }))))
    }

    pub(crate) fn start(&self) -> Result<TickCapture, BudgetError> {
        self.0.lock().map(|state| state.start).map_err(|_| BudgetError::Invalid)
    }

    pub(crate) fn try_reserve_time(
        &self,
        now: TickCapture,
        deadline: TickCapture,
    ) -> Result<PendingTime, BudgetError> {
        let mut state = self.0.lock().map_err(|_| BudgetError::Invalid)?;
        Self::check(&state, now)?;
        let daily = UnqualifiedDailyCount(state.daily);
        let cost = RequestCostCandidate::new(1, 0).map_err(|_| BudgetError::Invalid)?;
        let charge = state
            .model
            .reserve_normal(now, cost, Class::Steady, deadline, daily)
            .map_err(transition)?;
        Ok(PendingTime {
            owner: self.clone(),
            charge: Some(charge),
        })
    }

    fn check(state: &State, now: TickCapture) -> Result<(), BudgetError> {
        if state.closing {
            return Err(BudgetError::Closing);
        }
        let elapsed = now.elapsed_since(&state.start).map_err(|_| BudgetError::Invalid)?;
        if state.protective_waiters > 0
            || !state.heads.permits_steady(now)
            || (state.unclean && elapsed < Duration::from_secs(120))
            || state.pause.as_ref().is_some_and(|pause| pause.allows(now) != Ok(true))
        {
            return Err(BudgetError::Deferred);
        }
        Ok(())
    }

    /// The send poll and actual head observation share the same short lock
    /// as first-poll admission. No head can be observed and then race a new
    /// first poll before its restrictions are installed.
    pub(crate) fn poll_send(
        &self,
        pending: &mut Option<PendingTime>,
        clock: &NativeClock,
        epoch: uuid::Uuid,
        poll: impl FnOnce() -> std::task::Poll<reqwest::Result<reqwest::Response>>,
    ) -> Result<std::task::Poll<reqwest::Result<reqwest::Response>>, BudgetError> {
        if pending.as_ref().is_some_and(|pending| !Arc::ptr_eq(&self.0, &pending.owner.0)) {
            return Err(BudgetError::Invalid);
        }
        let mut state = self.0.lock().map_err(|_| BudgetError::Invalid)?;
        if let Some(mut pending) = pending.take() {
            let charge = pending.charge.take().ok_or(BudgetError::Invalid)?;
            let now = match clock.capture() {
                Ok(now) => now,
                Err(_) => {
                    let _ = state.model.cancel(charge);
                    return Err(BudgetError::Invalid);
                },
            };
            if let Err(error) = Self::check(&state, now) {
                let _ = state.model.cancel(charge);
                return Err(error);
            }
            let Some(next) = state.handoffs.checked_add(1) else {
                let _ = state.model.cancel(charge);
                return Err(BudgetError::Invalid);
            };
            if let Err((error, charge)) = state.model.handoff(charge, now) {
                let _ = state.model.cancel(charge);
                return Err(transition(error));
            }
            state.handoffs = next;
        }
        let result = poll();
        if let std::task::Poll::Ready(Ok(response)) = &result {
            state.heads.observe(
                response.status().as_u16(),
                response.headers(),
                clock.wall_bracket(epoch).ok(),
            );
        }
        Ok(result)
    }

    pub(crate) fn begin_closing(&self) {
        if let Ok(mut state) = self.0.lock() {
            state.closing = true;
        }
    }

    pub(crate) fn pending_pause(&self) -> bool {
        self.0.lock().map_or(true, |state| state.heads.pending())
    }
}

#[cfg(test)]
#[path = "state_tests.rs"]
mod tests;
