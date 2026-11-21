use std::collections::HashSet;

use chrono::{DateTime, Utc};
use robson_exec::{
    clock_records::TICK_MAX,
    venue_clock::{ClockDomain, TickCapture},
};
use sqlx::{postgres::PgRow, Decode, Postgres, Row, Type};
use uuid::Uuid;

use super::{bootstrap::BootstrapInput, pause_value::PauseValue, BudgetError};
use crate::process_epoch::EpochStart;

pub(super) fn get<T>(row: &PgRow, name: &str) -> Result<T, BudgetError>
where
    for<'r> T: Decode<'r, Postgres> + Type<Postgres>,
{
    row.try_get(name).map_err(|_| BudgetError::Invalid)
}

pub(super) fn integer(text: &str, fits: bool) -> Result<i128, BudgetError> {
    if !fits {
        return Err(BudgetError::Invalid);
    }
    let number = text.parse::<i128>().map_err(|_| BudgetError::Invalid)?;
    if !(0..=TICK_MAX).contains(&number) {
        return Err(BudgetError::Invalid);
    }
    Ok(number)
}

pub(super) fn capture(domain: ClockDomain, tick: i128) -> Result<TickCapture, BudgetError> {
    TickCapture::new(
        domain,
        i64::try_from(tick / 1_000_000_000).map_err(|_| BudgetError::Invalid)?,
        i64::try_from(tick % 1_000_000_000).map_err(|_| BudgetError::Invalid)?,
    )
    .map_err(|_| BudgetError::Invalid)
}

type ObservedState = (Uuid, Uuid, i64, i64, String, Option<Uuid>, Option<Uuid>, i16, i16, String);

pub(crate) struct BootstrapRows {
    pub(super) start: TickCapture,
    pub(super) active: Vec<(Uuid, Option<TickCapture>)>,
    pub(crate) pause: Option<PauseValue>,
    // Retain the observed state; none of these descriptive fields grants
    // current-domain comparability or production readiness.
    _state: ObservedState,
}

pub(super) fn check_state(row: &PgRow, input: &BootstrapInput) -> Result<(), BudgetError> {
    if get::<String>(row, "venue_id")? != "binance:spot"
        || get::<String>(row, "initialization_state")? != "complete"
        || get::<Option<Uuid>>(row, "initialization_id")?.is_none_or(|id| id.is_nil())
        || get::<Option<Uuid>>(row, "audit_id")? != Some(input.audit)
        || get::<i64>(row, "quiescence_generation")? != input.generation
        || get::<i16>(row, "schema_version")? != 38
        || get::<i16>(row, "proof_version")? != 1
        || get::<i64>(row, "recovery_generation")? < 0
        || !(0..=9).contains(&get::<i16>(row, "epoch_count_60")?)
        || !(0..=3).contains(&get::<i16>(row, "epoch_count_10")?)
    {
        return Err(BudgetError::Invalid);
    }
    match get::<String>(row, "recovery_status")?.as_str() {
        "unknown" | "comparable" | "quarantined" => Ok(()),
        _ => Err(BudgetError::Invalid),
    }
}

fn epochs(row: &PgRow, group: &str, now: i128) -> Result<Vec<Uuid>, BudgetError> {
    let ids: Vec<Uuid> = get(row, &format!("{group}_ids"))?;
    let ticks: Vec<String> = get(row, &format!("{group}_tick_text"))?;
    let fits: Vec<bool> = get(row, &format!("{group}_tick_fits"))?;
    let bound = if group == "epoch60" { 9 } else { 3 };
    if ids.len() > bound
        || ids.len() != ticks.len()
        || ids.len() != fits.len()
        || ids.iter().copied().collect::<HashSet<_>>().len() != ids.len()
    {
        return Err(BudgetError::Invalid);
    }
    for (tick, fit) in ticks.iter().zip(fits) {
        if integer(tick, fit)? > now {
            return Err(BudgetError::Invalid);
        }
    }
    Ok(ids)
}

pub(super) fn decode(row: &PgRow, input: &BootstrapInput) -> Result<BootstrapRows, BudgetError> {
    if row.len() != 59
        || !get::<bool>(row, "state_ok")?
        || !get::<bool>(row, "audit_ok")?
        || get::<bool>(row, "future_same_domain_epoch")?
        || !(0..=1_000_000).contains(&get::<i64>(row, "audit_examined_count")?)
    {
        return Err(BudgetError::Invalid);
    }
    check_state(row, input)?;
    let count = get::<i64>(row, "active_count")?;
    if !(0..=10000).contains(&count) {
        return Err(BudgetError::Invalid);
    }
    let ids: Vec<Uuid> = get(row, "active_ids")?;
    let present: Vec<bool> = get(row, "active_domain_present")?;
    let kinds: Vec<Option<String>> = get(row, "active_kind")?;
    let boots: Vec<Option<Uuid>> = get(row, "active_boot")?;
    let seconds: Vec<Option<i64>> = get(row, "active_offset_seconds")?;
    let nanos: Vec<Option<i32>> = get(row, "active_offset_ns")?;
    let ticks: Vec<Option<String>> = get(row, "active_tick_text")?;
    let fits: Vec<bool> = get(row, "active_tick_fits")?;
    let n = usize::try_from(count).map_err(|_| BudgetError::Invalid)?;
    if [
        ids.len(),
        present.len(),
        kinds.len(),
        boots.len(),
        seconds.len(),
        nanos.len(),
        ticks.len(),
        fits.len(),
    ]
    .iter()
    .any(|len| *len != n)
        || ids.iter().copied().collect::<HashSet<_>>().len() != n
    {
        return Err(BudgetError::Invalid);
    }
    let mut active = Vec::with_capacity(n);
    for index in 0..n {
        let raw_absent = kinds[index].is_none()
            && boots[index].is_none()
            && seconds[index].is_none()
            && nanos[index].is_none()
            && ticks[index].is_none();
        let tick = if !present[index] && raw_absent && !fits[index] {
            None
        } else {
            if !present[index] || kinds[index].as_deref() != Some("boottime") {
                return Err(BudgetError::Invalid);
            }
            let domain = ClockDomain::new(
                boots[index].ok_or(BudgetError::Invalid)?,
                seconds[index].ok_or(BudgetError::Invalid)?,
                u32::try_from(nanos[index].ok_or(BudgetError::Invalid)?)
                    .map_err(|_| BudgetError::Invalid)?,
            )
            .map_err(|_| BudgetError::Invalid)?;
            Some(capture(
                domain,
                integer(ticks[index].as_deref().ok_or(BudgetError::Invalid)?, fits[index])?,
            )?)
        };
        active.push((ids[index], tick));
    }
    let now = i128::try_from(input.start.nanoseconds()).map_err(|_| BudgetError::Invalid)?;
    let sixty = epochs(row, "epoch60", now)?;
    let ten = epochs(row, "epoch10", now)?;
    if ten.iter().any(|id| !sixty.contains(id)) {
        return Err(BudgetError::Invalid);
    }
    Ok(BootstrapRows {
        start: input.start,
        active,
        pause: PauseValue::decode(row)?,
        _state: (
            get::<Option<Uuid>>(row, "initialization_id")?.ok_or(BudgetError::Invalid)?,
            input.audit,
            input.generation,
            get(row, "recovery_generation")?,
            get(row, "recovery_status")?,
            get(row, "cohort_id")?,
            get(row, "source_high_water")?,
            get(row, "epoch_count_60")?,
            get(row, "epoch_count_10")?,
            get(row, "state_recorded_at")?,
        ),
    })
}

pub(super) fn check_epoch(
    row: &PgRow,
    epoch: &EpochStart,
    start: TickCapture,
    audit: DateTime<Utc>,
) -> Result<(), BudgetError> {
    let domain = start.domain();
    let (seconds, nanos) = domain.offset();
    let clean = epoch.predecessor_epoch_id.map(|_| epoch.predecessor.unclean_reason().is_none());
    if row.len() != 11
        || get::<Uuid>(row, "process_epoch")? != epoch.epoch.epoch_id
        || get::<String>(row, "venue_id")? != "binance:spot"
        || get::<String>(row, "domain_kind")? != "boottime"
        || get::<Uuid>(row, "domain_boot")? != domain.boot()
        || get::<i64>(row, "domain_offset_seconds")? != seconds
        || get::<i32>(row, "domain_offset_ns")?
            != i32::try_from(nanos).map_err(|_| BudgetError::Invalid)?
        || integer(&get::<String>(row, "start_tick_text")?, get(row, "start_tick_fits")?)?
            != i128::try_from(start.nanoseconds()).map_err(|_| BudgetError::Invalid)?
        || get::<Option<Uuid>>(row, "predecessor_epoch")? != epoch.predecessor_epoch_id
        || get::<Option<bool>>(row, "predecessor_clean")? != clean
        || get::<i64>(row, "recorded_at_us")? != audit.timestamp_micros()
    {
        Err(BudgetError::Invalid)
    } else {
        Ok(())
    }
}
