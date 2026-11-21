//! Descriptive transition access for the actual application owner. These
//! values never certify restoration, pause state, durability or send authority.
use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransitionError {
    Invalid,
    Expired,
    Deferred,
}

fn error(value: ModelError) -> TransitionError {
    match value {
        ModelError::Deadline | ModelError::Attempt => TransitionError::Expired,
        ModelError::Limits | ModelError::Capacity => TransitionError::Deferred,
        _ => TransitionError::Invalid,
    }
}

pub struct PendingCharge(ModelReference);

pub struct BudgetTransitions(LocalModel);
impl BudgetTransitions {
    /// I1 has no actual comparable-history producer; persisted status and
    /// filtered arrays cannot replace UnknownPriorHistory.
    pub fn unknown_history(start: TickCapture) -> Result<Self, TransitionError> {
        LocalModel::new(start, EpochHistoryCandidate::UnknownPriorHistory)
            .map(Self)
            .map_err(error)
    }

    pub fn reserve_normal(
        &mut self,
        now: TickCapture,
        cost: RequestCostCandidate,
        class: Class,
        deadline: TickCapture,
        daily: UnqualifiedDailyCount,
    ) -> Result<PendingCharge, TransitionError> {
        self.0
            .insert_pending_candidate(now, PendingCandidate {
                weight: cost.weight,
                orders: cost.orders,
                class,
                tier: Tier::Normal,
                deadline,
                attempt: None,
                daily,
            })
            .map(PendingCharge)
            .map_err(error)
    }

    /// The rolling usage at `now`, pending charges included: descriptive,
    /// for the bucket gauge (PS-5). Like every transition, `now` must not be
    /// older than the last observed.
    pub fn usage(&mut self, now: TickCapture) -> Result<UsageFacts, TransitionError> {
        self.0.usage_at(now).map_err(error)
    }

    pub fn cancel(&mut self, pending: PendingCharge) -> Result<(), TransitionError> {
        self.0.cancel_pending_candidate(pending.0).map(|_| ()).map_err(error)
    }

    pub fn handoff(
        &mut self,
        pending: PendingCharge,
        now: TickCapture,
    ) -> Result<(), (TransitionError, PendingCharge)> {
        self.0
            .record_handoff_candidate(pending.0, now)
            .map_err(|(failure, reference)| (error(failure), PendingCharge(reference)))
    }
}
