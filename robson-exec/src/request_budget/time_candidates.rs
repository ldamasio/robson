use std::time::Duration;

use uuid::Uuid;

use super::BudgetFactError;
use crate::venue_clock::TickCapture;

/// Numerical guard only; does not confirm an actual handoff.
/// ```compile_fail
/// use robson_exec::request_budget::AttemptAgeCandidate;
/// let forged = AttemptAgeCandidate { within_guard: true };
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AttemptAgeCandidate {
    within_guard: bool,
}
impl AttemptAgeCandidate {
    pub fn within_guard(&self) -> bool {
        self.within_guard
    }
}

pub fn attempt_age_candidate(
    created: &TickCapture,
    handoff: &TickCapture,
) -> Result<AttemptAgeCandidate, BudgetFactError> {
    Ok(AttemptAgeCandidate {
        within_guard: handoff.elapsed_since(created)? <= Duration::from_secs(60),
    })
}

/// Numerical threshold only, never a durable expiry proof or row retirement.
/// ```compile_fail
/// use robson_exec::request_budget::DailyRetentionCandidate;
/// let forged = DailyRetentionCandidate { threshold_elapsed: true };
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DailyRetentionCandidate {
    threshold_elapsed: bool,
}
impl DailyRetentionCandidate {
    pub fn threshold_elapsed(&self) -> bool {
        self.threshold_elapsed
    }
}

pub fn daily_retention_candidate(
    created: &TickCapture,
    now: &TickCapture,
) -> Result<DailyRetentionCandidate, BudgetFactError> {
    Ok(DailyRetentionCandidate {
        threshold_elapsed: now.elapsed_since(created)? >= Duration::from_secs(86460),
    })
}

/// Matching UUIDs do not authenticate a cohort, continuity or persisted anchor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuarantineAnchorCandidate {
    pub cohort: Uuid,
    pub recovery: Uuid,
    pub observed: TickCapture,
}

/// Numerical threshold only, with no continuity or durable-anchor authority.
/// ```compile_fail
/// use robson_exec::request_budget::QuarantineCandidate;
/// let forged = QuarantineCandidate { threshold_elapsed: true };
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuarantineCandidate {
    threshold_elapsed: bool,
}
impl QuarantineCandidate {
    pub fn threshold_elapsed(&self) -> bool {
        self.threshold_elapsed
    }
}

pub fn quarantine_elapsed_candidate(
    anchor: Option<&QuarantineAnchorCandidate>,
    cohort: Uuid,
    recovery: Uuid,
    now: &TickCapture,
) -> Result<QuarantineCandidate, BudgetFactError> {
    let anchor = anchor.ok_or(BudgetFactError::MissingAnchor)?;
    if cohort.is_nil()
        || recovery.is_nil()
        || anchor.cohort != cohort
        || anchor.recovery != recovery
    {
        return Err(BudgetFactError::IdentityMismatch);
    }
    Ok(QuarantineCandidate {
        threshold_elapsed: now.elapsed_since(&anchor.observed)? >= Duration::from_secs(86460),
    })
}
