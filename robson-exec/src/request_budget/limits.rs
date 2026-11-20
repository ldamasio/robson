use std::time::Duration;

use super::{BudgetFactError, RequestCostCandidate, UsageFacts};
use crate::venue_clock::TickCapture;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    Steady,
    Protective,
}

/// Does not prove a complete current union of attempts/pending/uncertain work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnqualifiedDailyCount(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EpochHistoryCandidate {
    Comparable { k60: u64, k10: u64 },
    UnknownPriorHistory,
}

/// Additional startup ceilings only, not readiness or a clean-start verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StartupCapsFacts {
    weight: Option<u64>,
    short_orders: Option<u64>,
}

impl StartupCapsFacts {
    pub fn weight(&self) -> Option<u64> {
        self.weight
    }
    pub fn short_orders(&self) -> Option<u64> {
        self.short_orders
    }
}

pub fn restart_cap_candidates(
    start: &TickCapture,
    now: &TickCapture,
    history: EpochHistoryCandidate,
) -> Result<StartupCapsFacts, BudgetFactError> {
    let elapsed = now.elapsed_since(start)?;
    let (weight, orders) = match history {
        EpochHistoryCandidate::UnknownPriorHistory => (0, 0),
        EpochHistoryCandidate::Comparable { k60, k10 } => {
            if k60 == 0 || k10 == 0 {
                return Err(BudgetFactError::InvalidEpochs);
            }
            (
                if k60 >= 9 { 0 } else { 200 >> (k60 - 1) },
                if k10 >= 3 { 0 } else { 2 >> (k10 - 1) },
            )
        },
    };
    Ok(StartupCapsFacts {
        weight: (elapsed < Duration::from_secs(60)).then_some(weight),
        short_orders: (elapsed < Duration::from_secs(10)).then_some(orders),
    })
}

/// Numerical fit is never a grant. Expanded fit needs the later emergency
/// protocol; every fit still lacks daily completeness, pause and J authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LimitFacts {
    usage: UsageFacts,
    daily: u64,
    normal: bool,
    expanded: Option<bool>,
}

impl LimitFacts {
    pub fn usage(&self) -> UsageFacts {
        self.usage
    }
    pub fn daily(&self) -> u64 {
        self.daily
    }
    pub fn fits_normal_arithmetic(&self) -> bool {
        self.normal
    }
    pub fn fits_expanded_arithmetic(&self) -> Option<bool> {
        self.expanded
    }
}

pub fn additional_cost_facts(
    local: UsageFacts,
    daily: UnqualifiedDailyCount,
    cost: RequestCostCandidate,
    class: Class,
    startup: StartupCapsFacts,
) -> Result<LimitFacts, BudgetFactError> {
    let usage = UsageFacts {
        weight: local.weight.checked_add(cost.weight as u64).ok_or(BudgetFactError::Overflow)?,
        short_orders: local
            .short_orders
            .checked_add(cost.orders as u64)
            .ok_or(BudgetFactError::Overflow)?,
    };
    let daily = daily.0.checked_add(cost.orders as u64).ok_or(BudgetFactError::Overflow)?;
    let fits = |weight, orders| {
        daily <= 10000
            && usage.weight <= weight
            && usage.short_orders <= orders
            && startup.weight.is_none_or(|cap| usage.weight <= cap)
            && startup.short_orders.is_none_or(|cap| usage.short_orders <= cap)
    };
    Ok(LimitFacts {
        usage,
        daily,
        normal: fits(if class == Class::Steady { 400 } else { 600 }, 10),
        expanded: (class == Class::Protective).then(|| fits(1200, 20)),
    })
}
