use std::cmp::Ordering;

use chrono::Datelike;
use robson_domain::{RiskConfig, Side, StopPolicy};
use rust_decimal::Decimal as D;

use super::{
    arithmetic::{Arithmetic, Rational},
    bridge::{native, r},
    inputs::*,
};
use crate::ports::CandleInterval;

pub(super) fn validate(i: &SizingInputs) -> Result<RiskConfig, SizingError> {
    let observations = [
        &i.basis.observation,
        &i.cash.observation,
        &i.price.observation,
        &i.chart.observation,
        &i.rules.observation,
    ];
    if i.binding.preparation_id.is_nil()
        || observations.iter().any(|o| o.binding != i.binding || o.observation_id.is_nil())
        || i.terms.evidence().account() != &i.binding.account
        || i.cash.quote_asset != i.binding.account.symbol().quote()
    {
        return Err(SizingError::Identity);
    }
    let bounds = i.risk.stop_distance_bounds();
    if i.basis.amount <= D::ZERO
        || i.basis.month.day() != 1
        || i.price.value <= D::ZERO
        || i.cash.free < D::ZERO
        || i.cash.locked < D::ZERO
        || i.policy.gap().value() != i.terms.gap_bps()
        || i.terms.taker_fraction() < D::ZERO
        || i.terms.taker_fraction() >= D::new(1, 2)
        || !(D::ZERO..=D::from(100)).contains(&i.terms.gap_bps())
        || !(D::ZERO..=D::from(100)).contains(&i.risk.stop_buffer_bps())
        || !(D::ZERO..=D::from(1000)).contains(&i.risk.margin_headroom_bps())
        || !(D::ZERO..D::from(10000)).contains(&i.policy.margin_bps())
        || i.policy.max_entry_notional().is_some_and(|v| v <= D::ZERO)
        || bounds.min_bps() <= D::ZERO
        || bounds.min_bps() >= bounds.max_bps()
        || bounds.max_bps() > D::from(10000)
    {
        return Err(SizingError::Config);
    }
    let rules = &i.rules;
    let p = rules.price;
    let n = rules.notional;
    if !rules.trading
        || !rules.market
        || !rules.stop_loss
        || p.tick <= D::ZERO
        || p.min < D::ZERO
        || p.max < D::ZERO
        || (p.max > D::ZERO && p.max < p.min)
        || n.min <= D::ZERO
        || n.max < n.min
        || [Some(rules.lot), rules.market_lot]
            .into_iter()
            .flatten()
            .any(|l| l.min < D::ZERO || l.max <= D::ZERO || l.max < l.min || l.step < D::ZERO)
    {
        return Err(SizingError::Rules);
    }
    if n.apply_min || n.apply_max {
        let v = i.filter_price.as_ref().ok_or(SizingError::FilterPrice)?;
        if v.value <= D::ZERO
            || v.observation.binding != i.binding
            || v.observation.observation_id.is_nil()
            || v.rules_generation != rules.observation.source_generation
            || match v.source {
                FilterPriceSource::VenueReference => false,
                FilterPriceSource::Average { minutes } => {
                    minutes == 0 || minutes != n.average_minutes
                },
                FilterPriceSource::LastPrice => n.average_minutes != 0,
            }
        {
            return Err(SizingError::FilterPrice);
        }
    }
    i.risk
        .with_capital(i.basis.amount)
        .and_then(|r| r.with_execution_costs(i.terms.taker_fraction(), i.terms.gap_bps()))
        .map_err(|_| SizingError::Config)
}

pub(super) fn grid(a: &mut Arithmetic, value: D, step: D) -> Result<bool, SizingError> {
    let value = r(value)?;
    let step = r(step)?;
    let down = a.floor_step(&value, &step)?;
    Ok(a.compare(&down, &value)? == Ordering::Equal)
}
pub(super) fn lot(a: &mut Arithmetic, i: &SizingInputs) -> Result<LotLimits, SizingError> {
    let mut lot = i.rules.lot;
    if let Some(m) = i.rules.market_lot {
        let (small, big) = (lot.step.min(m.step), lot.step.max(m.step));
        if small > D::ZERO && !grid(a, big, small)? {
            return Err(SizingError::Rules);
        }
        lot = LotLimits {
            min: lot.min.max(m.min),
            max: lot.max.min(m.max),
            step: big,
        };
    }
    if lot.step <= D::ZERO || lot.min > lot.max {
        return Err(SizingError::Rules);
    }
    Ok(lot)
}
pub(super) fn geometry(
    a: &mut Arithmetic,
    i: &SizingInputs,
    config: &RiskConfig,
) -> Result<Rational, SizingError> {
    let plan = i.chart.plan;
    let p = i.price.value;
    let chart = i.chart.support;
    let basis = i.chart.guard.map_or(chart, |g| g.min(chart));
    if plan.side != Side::Long
        || i.chart.interval != CandleInterval::FifteenMinutes
        || chart <= D::ZERO
        || chart >= p
        || basis <= D::ZERO
        || basis >= p
        || i.chart.guard.is_some_and(|g| g <= D::ZERO)
        || plan.basis.as_decimal() != basis
        || plan.guard_bound != (basis < chart)
    {
        return Err(SizingError::Geometry);
    }
    if !grid(a, i.rules.price.min, i.rules.price.tick)? {
        return Err(SizingError::Rules);
    }
    let pr = r(p)?;
    let br = r(basis)?;
    let dc = if plan.policy == StopPolicy::ExecutableSpan {
        let value = a.sub(&pr, &br)?;
        Some(native(a, &value)?)
    } else {
        None
    };
    let raw = a.mul(&br, &r(config.stop_buffer_bps())?)?;
    let raw = a.div(&raw, &r(D::from(10000))?)?;
    let buffer = if plan.policy == StopPolicy::ExecutableSpan {
        let cap = a.mul(
            &r(dc.ok_or(SizingError::Geometry)?)?,
            &r(robson_domain::executable_stop::BUFFER_CAP_RATIO)?,
        )?;
        if a.compare(&raw, &cap)? == Ordering::Greater {
            cap
        } else {
            raw
        }
    } else {
        raw
    };
    let buffer = native(a, &buffer)?;
    let raw_trigger = a.sub(&br, &r(buffer)?)?;
    let trigger = if plan.policy == StopPolicy::ExecutableSpan {
        a.floor_step(&raw_trigger, &r(i.rules.price.tick)?)?
    } else {
        raw_trigger
    };
    let trigger = native(a, &trigger)?;
    let expected = if plan.policy == StopPolicy::ExecutableSpan {
        let span = a.sub(&pr, &r(trigger)?)?;
        let span = native(a, &span)?;
        (dc, Some(span), Some(i.rules.price.tick))
    } else {
        (None, None, None)
    };
    if (plan.cap_basis_distance, plan.executable_span, plan.tick_size) != expected
        || plan.effective_buffer != buffer
        || plan.trigger.as_decimal() != trigger
        || trigger <= D::ZERO
        || trigger >= p
        || trigger < i.rules.price.min
        || (i.rules.price.max > D::ZERO && trigger > i.rules.price.max)
        || !grid(a, trigger, i.rules.price.tick)?
    {
        return Err(SizingError::Geometry);
    }
    let bounds = config.stop_distance_bounds();
    let lower = a.mul(&r(bounds.min_bps())?, &pr)?;
    let upper = a.mul(&r(bounds.max_bps())?, &pr)?;
    for level in [chart, basis, trigger] {
        let delta = a.sub(&pr, &r(level)?)?;
        let distance = a.mul(&delta, &r(D::from(10000))?)?;
        if a.compare(&distance, &lower)? == Ordering::Less
            || a.compare(&distance, &upper)? == Ordering::Greater
        {
            return Err(SizingError::Geometry);
        }
    }
    r(trigger)
}
