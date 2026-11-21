//! Unqualified budget calculations, not reservations or request permission.
//! Supplied charges/counts do not prove completeness, durability or
//! currentness. The private local transition model is likewise unqualified;
//! no runtime caller, clock capture, I/O or authority producer is added.

mod charges;
mod limits;
#[allow(dead_code)] // Inert local model; runtime authority and ownership bridge remain absent.
mod local_model;
mod time_candidates;

pub use charges::{local_usage_candidates, ChargeCandidate, RequestCostCandidate, UsageFacts};
pub use limits::{
    additional_cost_facts, restart_cap_candidates, Class, EpochHistoryCandidate, LimitFacts,
    StartupCapsFacts, UnqualifiedDailyCount,
};
pub use local_model::bridge::{BudgetTransitions, PendingCharge, TransitionError};
pub use time_candidates::{
    attempt_age_candidate, daily_retention_candidate, quarantine_elapsed_candidate,
    AttemptAgeCandidate, DailyRetentionCandidate, QuarantineAnchorCandidate, QuarantineCandidate,
};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BudgetFactError {
    #[error("invalid numerical request cost")]
    InvalidCost,
    #[error("too many supplied local charges")]
    TooManyCharges,
    #[error("budget arithmetic overflow")]
    Overflow,
    #[error("epoch count omits the current process")]
    InvalidEpochs,
    #[error("quarantine anchor is missing")]
    MissingAnchor,
    #[error("quarantine identity mismatch")]
    IdentityMismatch,
    #[error(transparent)]
    Clock(#[from] crate::venue_clock::ClockError),
}

#[cfg(test)]
mod tests;
