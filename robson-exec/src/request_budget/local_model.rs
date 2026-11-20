//! Private numerical transitions, never runtime reservations or send grants.
pub mod bridge;
use std::{
    slice,
    sync::{Arc, Weak},
};

use super::*;
use crate::venue_clock::TickCapture;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tier {
    Normal,
    Expanded,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
enum ModelError {
    #[error("local model time is invalid")]
    InvalidTime,
    #[error("local candidate deadline expired")]
    Deadline,
    #[error("local candidate attempt is stale")]
    Attempt,
    #[error("local numerical limits exceeded")]
    Limits,
    #[error("local model capacity or sequence exhausted")]
    Capacity,
    #[error("reference does not name a pending record in this model")]
    Reference,
    #[error(transparent)]
    Facts(#[from] BudgetFactError),
}

// No Clone/Copy/Deserialize: only a local identity, not a usable reservation.
#[derive(Debug)]
struct ModelReference {
    owner: Weak<()>,
    id: u64,
}

#[derive(Clone, Copy)]
struct Record {
    id: u64,
    cost: RequestCostCandidate,
    class: Class,
    tier: Tier,
    acquired: TickCapture,
    deadline: TickCapture,
    attempt: Option<TickCapture>,
    charge: ChargeCandidate,
}

struct PendingCandidate {
    weight: u32,
    orders: u32,
    class: Class,
    tier: Tier,
    deadline: TickCapture,
    attempt: Option<TickCapture>,
    daily: UnqualifiedDailyCount,
}

struct LocalModel {
    marker: Arc<()>,
    records: Vec<Record>,
    sequence: u64,
    start: TickCapture,
    observed: TickCapture,
    history: EpochHistoryCandidate,
    invalid_time: bool,
    #[cfg(test)]
    visits: usize,
}

impl LocalModel {
    fn new(start: TickCapture, history: EpochHistoryCandidate) -> Result<Self, ModelError> {
        restart_cap_candidates(&start, &start, history)?;
        Ok(Self {
            marker: Arc::new(()),
            records: Vec::with_capacity(1200),
            sequence: 0,
            start,
            observed: start,
            history,
            invalid_time: false,
            #[cfg(test)]
            visits: 0,
        })
    }

    fn visit(&mut self) {
        #[cfg(test)]
        {
            self.visits += 1;
        }
    }

    // Advance on every validated observation, including a subsequent refusal.
    fn usage_at(&mut self, now: TickCapture) -> Result<UsageFacts, ModelError> {
        #[cfg(test)]
        {
            self.visits = 0;
        }
        if self.invalid_time {
            return Err(ModelError::InvalidTime);
        }
        if now.elapsed_since(&self.observed).is_err() {
            self.invalid_time = true;
            return Err(ModelError::InvalidTime);
        }
        self.observed = now;
        let mut usage = UsageFacts { weight: 0, short_orders: 0 };
        let mut index = 0;
        while index < self.records.len() {
            self.visit();
            let record = &self.records[index];
            let facts = local_usage_candidates(slice::from_ref(&record.charge), &now)?;
            if matches!(record.charge, ChargeCandidate::Sent { .. })
                && facts.weight() == 0
                && facts.short_orders() == 0
            {
                self.records.swap_remove(index);
            } else {
                usage.weight =
                    usage.weight.checked_add(facts.weight()).ok_or(BudgetFactError::Overflow)?;
                usage.short_orders = usage
                    .short_orders
                    .checked_add(facts.short_orders())
                    .ok_or(BudgetFactError::Overflow)?;
                index += 1;
            }
        }
        Ok(usage)
    }

    fn insert_pending_candidate(
        &mut self,
        now: TickCapture,
        input: PendingCandidate,
    ) -> Result<ModelReference, ModelError> {
        let usage = self.usage_at(now)?;
        let cost = RequestCostCandidate::new(input.weight, input.orders)?;
        let duration = input.deadline.elapsed_since(&now).map_err(BudgetFactError::from)?;
        if duration.is_zero() {
            return Err(ModelError::Deadline);
        }
        if let Some(attempt) = input.attempt {
            now.elapsed_since(&attempt).map_err(BudgetFactError::from)?;
        }
        let startup = restart_cap_candidates(&self.start, &now, self.history)?;
        let fit = additional_cost_facts(usage, input.daily, cost, input.class, startup)?;
        let fits = match input.tier {
            Tier::Normal => fit.fits_normal_arithmetic(),
            Tier::Expanded => fit.fits_expanded_arithmetic() == Some(true),
        };
        if !fits {
            return Err(ModelError::Limits);
        }
        if self.records.len() == 1200 {
            return Err(ModelError::Capacity);
        }
        let id = self.sequence.checked_add(1).ok_or(ModelError::Capacity)?;
        self.records.push(Record {
            id,
            cost,
            class: input.class,
            tier: input.tier,
            acquired: now,
            deadline: input.deadline,
            attempt: input.attempt,
            charge: ChargeCandidate::Pending(cost),
        });
        self.sequence = id;
        Ok(ModelReference { owner: Arc::downgrade(&self.marker), id })
    }

    fn index(&mut self, reference: &ModelReference) -> Result<usize, ModelError> {
        if reference.owner.as_ptr() != Arc::as_ptr(&self.marker) {
            return Err(ModelError::Reference);
        }
        for index in 0..self.records.len() {
            self.visit();
            if self.records[index].id == reference.id {
                return Ok(index);
            }
        }
        Err(ModelError::Reference)
    }

    fn cancel_pending_candidate(
        &mut self,
        reference: ModelReference,
    ) -> Result<RequestCostCandidate, ModelError> {
        #[cfg(test)]
        {
            self.visits = 0;
        }
        let index = self.index(&reference)?;
        if !matches!(self.records[index].charge, ChargeCandidate::Pending(_)) {
            return Err(ModelError::Reference);
        }
        Ok(self.records.swap_remove(index).cost)
    }

    fn record_handoff_candidate(
        &mut self,
        reference: ModelReference,
        h: TickCapture,
    ) -> Result<(), (ModelError, ModelReference)> {
        let mut transition = || -> Result<usize, ModelError> {
            self.usage_at(h)?;
            let index = self.index(&reference)?;
            let record = &self.records[index];
            if !matches!(record.charge, ChargeCandidate::Pending(_)) {
                return Err(ModelError::Reference);
            }
            h.elapsed_since(&record.acquired).map_err(BudgetFactError::from)?;
            let remaining = record.deadline.elapsed_since(&h).map_err(|_| ModelError::Deadline)?;
            if remaining.is_zero() {
                return Err(ModelError::Deadline);
            }
            if let Some(created) = record.attempt {
                if !attempt_age_candidate(&created, &h)?.within_guard() {
                    return Err(ModelError::Attempt);
                }
            }
            Ok(index)
        };
        match transition() {
            Ok(index) => {
                self.records[index].charge = ChargeCandidate::Sent {
                    cost: self.records[index].cost,
                    handoff: h,
                };
                Ok(())
            },
            Err(error) => Err((error, reference)),
        }
    }
}

#[cfg(test)]
#[path = "local_model_tests.rs"]
mod tests;
