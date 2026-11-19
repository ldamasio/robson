//! Original signing provenance and synchronized commission first-poll gate.

use std::task::Poll;

use robson_connectors::binance::spot::PreparedSpotCommission;
use robson_exec::balance_costs::CostIdentity;

use crate::venue_budget::state::PendingCommission;

use super::*;

const SECOND_NS: i128 = 1_000_000_000;
const MILLIS_NS: i128 = 1_000_000;
const HANDOFF_LIMIT: Duration = Duration::from_secs(60);

pub(crate) type CommissionPreparation =
    JoinHandle<Result<PreparedSpotCommission, BinanceRestError>>;

pub(crate) struct CommissionSigningTime {
    pub(super) state: Arc<Mutex<ClockState>>,
    generation: u64,
    sample: Uuid,
    capture: RawClockCapture,
    timestamp_ms: i64,
}

impl CommissionSigningTime {
    pub(crate) fn timestamp_ms(&self) -> i64 {
        self.timestamp_ms
    }
}

fn wall_ns(raw: &RawClockCapture) -> Option<i128> {
    i128::from(raw.wall_seconds)
        .checked_mul(SECOND_NS)?
        .checked_add(i128::from(raw.wall_nanoseconds))
}

fn window_allows(timestamp_ms: i64, wall: i128, u2: i128) -> Option<bool> {
    if !(0..=2 * SECOND_NS).contains(&u2) {
        return None;
    }
    let error = u2.checked_add(1)?.checked_div(2)?.checked_add(50 * MILLIS_NS)?;
    let signed = i128::from(timestamp_ms).checked_mul(MILLIS_NS)?;
    let oldest_server = wall.checked_sub(error)?;
    let newest_server = wall.checked_add(error)?;
    Some(
        signed < oldest_server.checked_add(SECOND_NS)?
            && newest_server.checked_sub(signed)? <= 5 * SECOND_NS,
    )
}

impl ClockOwner {
    pub(crate) fn commission_epoch(&self) -> Uuid {
        self.epoch
    }

    pub(crate) fn is_commission_cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }

    pub(crate) async fn commission_cancelled(&self) {
        self.cancel.cancelled().await;
    }

    pub(crate) fn commission_capture(&self) -> Result<RawClockCapture, TimeError> {
        let mut state =
            self.state.try_lock().map_err(|_| TimeError::Budget(BudgetError::Deferred))?;
        if state.closing || self.cancel.is_cancelled() {
            return Err(TimeError::Closing);
        }
        state.capture_native(&self.clock, self.epoch)
    }

    pub(crate) fn reserve_commission(
        &self,
        deadline: Instant,
    ) -> Result<PendingCommission, TimeError> {
        let now = self.clock.capture().map_err(|_| TimeError::Native)?;
        let remaining =
            deadline.checked_duration_since(Instant::now()).ok_or(TimeError::Expired)?;
        self.budget
            .try_reserve_commission(now, after(now, remaining)?)
            .map_err(TimeError::Budget)
    }

    pub(crate) async fn prepare_commission(
        &self,
        account: &CostIdentity,
        verified_client: &Arc<BinanceRestClient>,
        slot: &mut Option<CommissionPreparation>,
        deadline: Instant,
    ) -> Result<PreparedSpotCommission, TimeError> {
        if slot.is_some()
            || !Arc::ptr_eq(&self.client, verified_client)
            || account.tenant() != self.tenant
            || account.venue().as_str() != "binance:spot"
        {
            return Err(TimeError::Resource);
        }
        if self.cancel.is_cancelled() {
            return Err(TimeError::Closing);
        }
        if Instant::now() >= deadline {
            return Err(TimeError::Expired);
        }
        let permit =
            Arc::clone(&self.workers).try_acquire_owned().map_err(|_| TimeError::Resource)?;
        let client = Arc::clone(&self.client);
        let symbol = account.symbol().clone();
        *slot = Some(tokio::task::spawn_blocking(move || {
            let _permit = permit;
            client.prepare_spot_commission(&symbol)
        }));
        jobs::complete(slot, deadline, &self.cancel, |_| TimeError::Transport).await
    }

    pub(crate) async fn retire_commission(slot: &mut Option<CommissionPreparation>) -> bool {
        jobs::retire(slot).await
    }

    pub(crate) fn commission_signing_time(&self) -> Result<CommissionSigningTime, TimeError> {
        let mut state =
            self.state.try_lock().map_err(|_| TimeError::Budget(BudgetError::Deferred))?;
        if state.closing || self.cancel.is_cancelled() {
            return Err(TimeError::Closing);
        }
        let capture = state.capture_native(&self.clock, self.epoch)?;
        let sample = state.published.as_ref().ok_or(TimeError::Native)?;
        if sample.arithmetic.fits_capture(&capture) != Ok(true) {
            return Err(TimeError::Native);
        }
        let ms = wall_ns(&capture).ok_or(TimeError::Native)?.div_euclid(MILLIS_NS);
        let timestamp_ms = i64::try_from(ms).map_err(|_| TimeError::Native)?;
        if timestamp_ms <= 0 {
            return Err(TimeError::Native);
        }
        Ok(CommissionSigningTime {
            state: Arc::clone(&self.state),
            generation: state.generation,
            sample: sample.id,
            capture,
            timestamp_ms,
        })
    }

    pub(crate) fn poll_commission_first(
        &self,
        signed: &CommissionSigningTime,
        pending: &mut PendingCommission,
        claim: TickCapture,
        deadline: Instant,
        poll: impl FnOnce() -> Poll<reqwest::Result<reqwest::Response>>,
    ) -> Result<(TickCapture, Poll<reqwest::Result<reqwest::Response>>), BudgetError> {
        if !Arc::ptr_eq(&self.state, &signed.state) {
            return Err(BudgetError::Invalid);
        }
        // Clock state first. Pause and heads use the second guard independently.
        let mut state = self.state.try_lock().map_err(|_| BudgetError::Deferred)?;
        self.budget.poll_commission_first(
            pending,
            &self.clock,
            self.epoch,
            || self.admit_signed(&mut state, signed, claim, deadline).map(|(h, _)| h),
            poll,
        )
    }

    /// The original signed first-poll clock admission, run by the budget core
    /// under the held clock guard. Returns the end tick and the actual native
    /// poll mark it was judged on.
    pub(super) fn admit_signed(
        &self,
        state: &mut ClockState,
        signed: &CommissionSigningTime,
        claim: TickCapture,
        deadline: Instant,
    ) -> Result<(TickCapture, RawClockCapture), BudgetError> {
        if state.closing || self.cancel.is_cancelled() {
            return Err(BudgetError::Closing);
        }
        if Instant::now() >= deadline {
            return Err(BudgetError::Expired);
        }
        if state.generation != signed.generation || signed.capture.epoch != self.epoch {
            return Err(BudgetError::Deferred);
        }
        let poll_mark =
            state.capture_native(&self.clock, self.epoch).map_err(|_| BudgetError::Invalid)?;
        let sample = state
            .published
            .as_ref()
            .filter(|s| s.id == signed.sample)
            .ok_or(BudgetError::Deferred)?;
        let h = poll_mark.end_tick().map_err(|_| BudgetError::Invalid)?;
        let elapsed = h.elapsed_since(&claim).map_err(|_| BudgetError::Invalid)?;
        if elapsed > HANDOFF_LIMIT || Instant::now() >= deadline {
            return Err(BudgetError::Expired);
        }
        if sample.arithmetic.fits_capture(&poll_mark) != Ok(true)
            || signed.capture.wall_agrees(&poll_mark) != Ok(true)
            || wall_ns(&poll_mark)
                .and_then(|wall| window_allows(signed.timestamp_ms, wall, sample.u2))
                != Some(true)
        {
            return Err(BudgetError::Deferred);
        }
        Ok((h, poll_mark))
    }

    pub(crate) fn poll_commission_again(
        &self,
        poll: impl FnOnce() -> Poll<reqwest::Result<reqwest::Response>>,
    ) -> Result<Poll<reqwest::Result<reqwest::Response>>, BudgetError> {
        self.budget.poll_send(&mut None, &self.clock, self.epoch, poll)
    }
}

#[cfg(test)]
mod tests {
    use super::window_allows;

    #[test]
    fn signed_window_edges_are_strict_future_and_inclusive_past() {
        let timestamp = 1_800_000_000_000_i64;
        let wall = i128::from(timestamp) * 1_000_000;
        let maximum = 2_000_000_000;
        assert_eq!(window_allows(timestamp, wall + 50_000_000, maximum), Some(false));
        assert_eq!(window_allows(timestamp, wall + 50_000_001, maximum), Some(true));
        assert_eq!(window_allows(timestamp, wall + 3_950_000_000, maximum), Some(true));
        assert_eq!(window_allows(timestamp, wall + 3_950_000_001, maximum), Some(false));
        assert_eq!(window_allows(timestamp, wall, maximum + 1), None);
    }
}
