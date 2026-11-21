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

pub(crate) struct PendingCommission(PendingTime);

/// Test-only C8 inventory reservation, bound to the one closed read whose
/// fixed weight it charged.
#[cfg(all(test, feature = "crash-points"))]
pub(crate) struct PendingAccount(PendingTime, robson_connectors::inventory_loopback::InventoryRead);

#[cfg(all(test, feature = "crash-points"))]
pub(crate) struct PendingOwnedOrder(PendingTime);

#[cfg(all(test, feature = "crash-points"))]
impl PendingAccount {
    pub(crate) fn read(&self) -> robson_connectors::inventory_loopback::InventoryRead {
        self.1
    }
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
        self.reserve_steady(&mut state, now, deadline, 1)
    }

    pub(crate) fn try_reserve_commission(
        &self,
        now: TickCapture,
        deadline: TickCapture,
    ) -> Result<PendingCommission, BudgetError> {
        let mut state = self.0.try_lock().map_err(|_| BudgetError::Deferred)?;
        self.reserve_steady(&mut state, now, deadline, 20).map(PendingCommission)
    }

    fn reserve_steady(
        &self,
        state: &mut State,
        now: TickCapture,
        deadline: TickCapture,
        weight: u32,
    ) -> Result<PendingTime, BudgetError> {
        Self::check(&state, now)?;
        let daily = UnqualifiedDailyCount(state.daily);
        let cost = RequestCostCandidate::new(weight, 0).map_err(|_| BudgetError::Invalid)?;
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

    /// The caller already holds the clock gate. This method never acquires
    /// that gate or waits while holding the budget gate.
    pub(crate) fn poll_commission_first(
        &self,
        pending: &mut PendingCommission,
        clock: &NativeClock,
        epoch: uuid::Uuid,
        admit_clock: impl FnOnce() -> Result<TickCapture, BudgetError>,
        poll: impl FnOnce() -> std::task::Poll<reqwest::Result<reqwest::Response>>,
    ) -> Result<(TickCapture, std::task::Poll<reqwest::Result<reqwest::Response>>), BudgetError>
    {
        let admit = || admit_clock().map(|h| (h, ()));
        self.poll_reserved_first(&mut pending.0, clock, epoch, admit, poll)
            .map(|(now, (), result)| (now, result))
    }

    /// The shared reserved first poll: owner check, budget gate, the caller's
    /// clock admission, budget check, charge handoff, poll and head
    /// observation, in that order and under one budget guard. The admission
    /// mark is returned as produced, never reconstructed.
    fn poll_reserved_first<M>(
        &self,
        pending: &mut PendingTime,
        clock: &NativeClock,
        epoch: uuid::Uuid,
        admit_clock: impl FnOnce() -> Result<(TickCapture, M), BudgetError>,
        poll: impl FnOnce() -> std::task::Poll<reqwest::Result<reqwest::Response>>,
    ) -> Result<(TickCapture, M, std::task::Poll<reqwest::Result<reqwest::Response>>), BudgetError>
    {
        if !Arc::ptr_eq(&self.0, &pending.owner.0) {
            return Err(BudgetError::Invalid);
        }
        let mut state = self.0.try_lock().map_err(|_| BudgetError::Deferred)?;
        let (now, mark) = admit_clock()?;
        Self::check(&state, now)?;
        let next = state.handoffs.checked_add(1).ok_or(BudgetError::Invalid)?;
        let charge = pending.charge.take().ok_or(BudgetError::Invalid)?;
        if let Err((error, charge)) = state.model.handoff(charge, now) {
            let _ = state.model.cancel(charge);
            return Err(transition(error));
        }
        state.handoffs = next;
        let result = poll();
        Self::observe_head(&mut state, &result, clock, epoch);
        Ok((now, mark, result))
    }

    /// Test-only C8 ACCOUNT reservation: weight 20, the same Steady class,
    /// non-blocking gate and checks as commission, under its own type.
    #[cfg(all(test, feature = "crash-points"))]
    pub(crate) fn try_reserve_account(
        &self,
        now: TickCapture,
        deadline: TickCapture,
    ) -> Result<PendingAccount, BudgetError> {
        use robson_connectors::inventory_loopback::InventoryRead;
        self.try_reserve_inventory(InventoryRead::Account, now, deadline)
    }

    /// Test-only C8 inventory reservation: the closed read's fixed weight
    /// (20/80/6), never a caller-chosen one, same Steady checks.
    #[cfg(all(test, feature = "crash-points"))]
    pub(crate) fn try_reserve_inventory(
        &self,
        read: robson_connectors::inventory_loopback::InventoryRead,
        now: TickCapture,
        deadline: TickCapture,
    ) -> Result<PendingAccount, BudgetError> {
        let mut state = self.0.try_lock().map_err(|_| BudgetError::Deferred)?;
        self.reserve_steady(&mut state, now, deadline, read.weight())
            .map(|pending| PendingAccount(pending, read))
    }

    #[cfg(all(test, feature = "crash-points"))]
    pub(crate) fn poll_account_first<M>(
        &self,
        pending: &mut PendingAccount,
        clock: &NativeClock,
        epoch: uuid::Uuid,
        admit_clock: impl FnOnce() -> Result<(TickCapture, M), BudgetError>,
        poll: impl FnOnce() -> std::task::Poll<reqwest::Result<reqwest::Response>>,
    ) -> Result<(TickCapture, M, std::task::Poll<reqwest::Result<reqwest::Response>>), BudgetError>
    {
        self.poll_reserved_first(&mut pending.0, clock, epoch, admit_clock, poll)
    }

    #[cfg(all(test, feature = "crash-points"))]
    pub(crate) fn try_reserve_owned_order(
        &self,
        now: TickCapture,
        deadline: TickCapture,
    ) -> Result<PendingOwnedOrder, BudgetError> {
        let mut state = self.0.try_lock().map_err(|_| BudgetError::Deferred)?;
        self.reserve_steady(
            &mut state,
            now,
            deadline,
            robson_connectors::inventory_loopback::OWNED_ORDER_WEIGHT,
        )
        .map(PendingOwnedOrder)
    }

    #[cfg(all(test, feature = "crash-points"))]
    pub(crate) fn poll_owned_order_first<M>(
        &self,
        pending: &mut PendingOwnedOrder,
        clock: &NativeClock,
        epoch: uuid::Uuid,
        admit_clock: impl FnOnce() -> Result<(TickCapture, M), BudgetError>,
        poll: impl FnOnce() -> std::task::Poll<reqwest::Result<reqwest::Response>>,
    ) -> Result<(TickCapture, M, std::task::Poll<reqwest::Result<reqwest::Response>>), BudgetError>
    {
        self.poll_reserved_first(&mut pending.0, clock, epoch, admit_clock, poll)
    }

    fn observe_head(
        state: &mut State,
        result: &std::task::Poll<reqwest::Result<reqwest::Response>>,
        clock: &NativeClock,
        epoch: uuid::Uuid,
    ) {
        if let std::task::Poll::Ready(Ok(response)) = result {
            state.heads.observe(
                response.status().as_u16(),
                response.headers(),
                clock.wall_bracket(epoch).ok(),
            );
        }
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
        Self::observe_head(&mut state, &result, clock, epoch);
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
