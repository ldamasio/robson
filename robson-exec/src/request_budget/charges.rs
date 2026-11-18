use std::time::Duration;

use super::BudgetFactError;
use crate::venue_clock::TickCapture;

/// Numerical domain only; does not authenticate an endpoint's actual cost.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestCostCandidate {
    pub(super) weight: u32,
    pub(super) orders: u32,
}

impl RequestCostCandidate {
    pub fn new(weight: u32, orders: u32) -> Result<Self, BudgetFactError> {
        if weight == 0 || weight > 1200 || orders > 1 {
            return Err(BudgetFactError::InvalidCost);
        }
        Ok(Self { weight, orders })
    }
}

/// Descriptive input, including an unqualified alleged handoff capture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChargeCandidate {
    Pending(RequestCostCandidate),
    Sent {
        cost: RequestCostCandidate,
        handoff: TickCapture,
    },
}

/// Sum of supplied entries only, never a complete owned reservation snapshot.
/// ```compile_fail
/// use robson_exec::request_budget::UsageFacts;
/// let forged = UsageFacts { weight: 0, short_orders: 0 };
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UsageFacts {
    pub(super) weight: u64,
    pub(super) short_orders: u64,
}

impl UsageFacts {
    pub fn weight(&self) -> u64 {
        self.weight
    }
    pub fn short_orders(&self) -> u64 {
        self.short_orders
    }
}

/// O(n), at most 1200 records; duplicates count twice and no input is retained.
pub fn local_usage_candidates(
    charges: &[ChargeCandidate],
    now: &TickCapture,
) -> Result<UsageFacts, BudgetFactError> {
    if charges.len() > 1200 {
        return Err(BudgetFactError::TooManyCharges);
    }
    let mut usage = UsageFacts { weight: 0, short_orders: 0 };
    for charge in charges {
        let (cost, weight_live, orders_live) = match charge {
            ChargeCandidate::Pending(cost) => (cost, true, true),
            ChargeCandidate::Sent { cost, handoff } => {
                let age = now.elapsed_since(handoff)?;
                (cost, age < Duration::from_secs(60), age < Duration::from_secs(10))
            },
        };
        usage.weight = usage
            .weight
            .checked_add(if weight_live { cost.weight as u64 } else { 0 })
            .ok_or(BudgetFactError::Overflow)?;
        usage.short_orders = usage
            .short_orders
            .checked_add(if orders_live { cost.orders as u64 } else { 0 })
            .ok_or(BudgetFactError::Overflow)?;
    }
    Ok(usage)
}
