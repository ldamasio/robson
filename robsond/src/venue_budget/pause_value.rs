use robson_exec::venue_clock::{ClockDomain, TickCapture};
use sqlx::postgres::PgRow;
use uuid::Uuid;

use super::{
    snapshot::{get, integer},
    BudgetError,
};

pub(super) struct PauseValue {
    pub(super) observation: Uuid,
    pub(super) duration: Option<i128>,
    pub(super) original: Option<(ClockDomain, i128)>,
    pub(super) restored: Option<(ClockDomain, i128, i128)>,
    pub(super) bounded: bool,
    pub(super) _observed: (i16, Option<i32>, Option<String>, String),
}

fn domain(row: &PgRow, prefix: &str) -> Result<Option<ClockDomain>, BudgetError> {
    let present: bool = get(row, &format!("{prefix}_present"))?;
    let kind: Option<String> = get(row, &format!("{prefix}_kind"))?;
    let boot: Option<Uuid> = get(row, &format!("{prefix}_boot"))?;
    let seconds: Option<i64> = get(row, &format!("{prefix}_offset_seconds"))?;
    let nanos: Option<i32> = get(row, &format!("{prefix}_offset_ns"))?;
    if !present && kind.is_none() && boot.is_none() && seconds.is_none() && nanos.is_none() {
        return Ok(None);
    }
    if !present || kind.as_deref() != Some("boottime") {
        return Err(BudgetError::Invalid);
    }
    ClockDomain::new(
        boot.ok_or(BudgetError::Invalid)?,
        seconds.ok_or(BudgetError::Invalid)?,
        u32::try_from(nanos.ok_or(BudgetError::Invalid)?).map_err(|_| BudgetError::Invalid)?,
    )
    .map(Some)
    .map_err(|_| BudgetError::Invalid)
}

fn optional(row: &PgRow, text: &str, fit: &str) -> Result<Option<i128>, BudgetError> {
    match (get::<Option<String>>(row, text)?, get::<bool>(row, fit)?) {
        (None, false) => Ok(None),
        (Some(value), true) => integer(&value, true).map(Some),
        _ => Err(BudgetError::Invalid),
    }
}

impl PauseValue {
    pub(super) fn decode(row: &PgRow) -> Result<Option<Self>, BudgetError> {
        let present: bool = get(row, "pause_present")?;
        let status: Option<i16> = get(row, "pause_status")?;
        let code: Option<i32> = get(row, "pause_code")?;
        let state: Option<String> = get(row, "pause_state")?;
        let observation: Option<Uuid> = get(row, "pause_observation_id")?;
        let original = domain(row, "pause_original")?;
        let restored = domain(row, "pause_restoration")?;
        let observed = optional(row, "pause_observed_tick_text", "pause_observed_tick_fits")?;
        let duration = optional(row, "pause_duration_text", "pause_duration_fits")?;
        let anchor = optional(row, "pause_anchor_text", "pause_anchor_fits")?;
        let deadline = optional(row, "pause_deadline_text", "pause_deadline_fits")?;
        let until: Option<String> = get(row, "pause_until_audit")?;
        let recorded: Option<String> = get(row, "pause_recorded_audit")?;
        if !present {
            if status.is_none()
                && code.is_none()
                && state.is_none()
                && observation.is_none()
                && original.is_none()
                && restored.is_none()
                && observed.is_none()
                && duration.is_none()
                && anchor.is_none()
                && deadline.is_none()
                && until.is_none()
                && recorded.is_none()
            {
                return Ok(None);
            }
            return Err(BudgetError::Invalid);
        }
        let original = match (original, observed, duration) {
            (None, None, None) => None,
            (Some(domain), Some(tick), Some(_)) => Some((domain, tick)),
            _ => return Err(BudgetError::Invalid),
        };
        let restored = match (restored, anchor, deadline) {
            (None, None, None) => None,
            (Some(domain), Some(start), Some(end))
                if duration.is_some_and(|n| start.checked_add(n) == Some(end)) =>
            {
                Some((domain, start, end))
            },
            _ => return Err(BudgetError::Invalid),
        };
        let bounded = match state.as_deref() {
            Some("bounded") if original.is_some() && restored.is_some() => true,
            Some("unresolved") => false,
            _ => return Err(BudgetError::Invalid),
        };
        let status = status.ok_or(BudgetError::Invalid)?;
        let observation = observation.ok_or(BudgetError::Invalid)?;
        if !matches!(status, 418 | 429) || observation.is_nil() {
            return Err(BudgetError::Invalid);
        }
        Ok(Some(Self {
            observation,
            duration,
            original,
            restored,
            bounded,
            _observed: (status, code, until, recorded.ok_or(BudgetError::Invalid)?),
        }))
    }

    pub(super) fn allows(&self, now: TickCapture) -> Result<bool, BudgetError> {
        if !self.bounded {
            return Ok(false);
        }
        let (domain, _, deadline) = self.restored.ok_or(BudgetError::Invalid)?;
        if domain != now.domain() {
            return Ok(false);
        }
        Ok(i128::try_from(now.nanoseconds()).map_err(|_| BudgetError::Invalid)? >= deadline)
    }
}
