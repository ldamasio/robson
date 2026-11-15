use std::cmp::Ordering;

use rust_decimal::Decimal as D;

use super::{
    arithmetic::{Arithmetic, Rational},
    bridge::{native, r},
    geometry,
    inputs::*,
};
struct Prepared {
    lot: LotLimits,
    af: D,
    l: D,
    t: Rational,
    u: Rational,
    cap: Rational,
    physical: Rational,
    s0: Rational,
    r0: Rational,
}
fn prepare(i: &SizingInputs) -> Result<Prepared, SizingError> {
    #[cfg(test)]
    super::arithmetic::trace("prepare", 0);
    let config = geometry::validate(i)?;
    let mut a = Arithmetic::default();
    let lot = geometry::lot(&mut a, i)?;
    let trigger = geometry::geometry(&mut a, i, &config)?;
    let tenk = r(D::from(10000))?;
    let one = r(D::ONE)?;
    let h = a.sub(&tenk, &r(config.margin_headroom_bps())?)?;
    let h = a.div(&h, &tenk)?;
    let cap = a.div(&r(i.basis.amount)?, &r(D::from(100))?)?;
    let gap = a.div(&r(i.terms.gap_bps())?, &tenk)?;
    let factor = a.sub(&one, &gap)?;
    let af = a.mul(&trigger, &factor)?;
    let af = native(&mut a, &af)?;
    let distance = a.sub(&r(i.price.value)?, &r(af)?)?;
    let prices = a.add(&r(i.price.value)?, &r(af)?)?;
    let fees = a.mul(&r(i.terms.taker_fraction())?, &prices)?;
    let l = a.add(&distance, &fees)?;
    let l = native(&mut a, &l)?;
    if af <= D::ZERO || l <= D::ZERO {
        return Err(SizingError::Geometry);
    }
    let t = a.sub(&one, &r(i.terms.taker_fraction())?)?;
    let margin = a.div(&r(i.policy.margin_bps())?, &tenk)?;
    let u = a.sub(&one, &margin)?;
    let risk = a.div(&cap, &r(l)?)?;
    let risk = a.floor12(&risk)?;
    let physical = a.mul(&r(i.basis.amount.min(i.cash.free))?, &h)?;
    let qty = a.div(&physical, &r(i.price.value)?)?;
    let qty = a.floor8(&qty)?;
    let min = if a.compare(&risk, &qty)? == Ordering::Greater {
        qty
    } else {
        risk
    };
    let s0 = a.floor_step(&min, &r(lot.step)?)?;
    let reserve = a.mul(&r(af)?, &u)?;
    let reserve = a.div(&r(i.rules.notional.min)?, &reserve)?;
    let r0 = a.ceil_step(&reserve, &r(lot.step)?)?;
    debug_assert!(a.operations() <= 57);
    Ok(Prepared { lot, af, l, t, u, cap, physical, s0, r0 })
}
fn deny(reason: CandidateRefusal) -> SizingError {
    SizingError::Exhausted { reason, attempts: 0 }
}
fn candidate(
    i: &SizingInputs,
    p: &Prepared,
    k: u8,
    terminal: &mut bool,
) -> Result<SizedAmounts, SizingError> {
    #[cfg(test)]
    super::arithmetic::trace("candidate", u64::from(k));
    let mut a = Arithmetic::default();
    let step = r(p.lot.step)?;
    let decrement = a.mul(&r(D::from(k))?, &step)?;
    if a.compare(&p.s0, &decrement)? != Ordering::Greater {
        *terminal = true;
        return Err(deny(CandidateRefusal::Quantity));
    }
    let s = a.sub(&p.s0, &decrement)?;
    let s = native(&mut a, &s)?;
    if s < p.lot.min {
        *terminal = true;
        return Err(deny(CandidateRefusal::Quantity));
    }
    if s > p.lot.max {
        return Err(deny(CandidateRefusal::Quantity));
    }
    let sr = r(s)?;
    let q = a.div(&sr, &p.t)?;
    let q = a.ceil_step(&q, &step)?;
    let q = native(&mut a, &q)?;
    if q < p.lot.min || q > p.lot.max {
        return Err(deny(CandidateRefusal::Quantity));
    }
    let qr = r(q)?;
    let rh = if a.compare(&sr, &p.r0)? == Ordering::Greater {
        &p.r0
    } else {
        &sr
    };
    let rh = native(&mut a, rh)?;
    let price = r(i.price.value)?;
    let af = r(p.af)?;
    let minimum = r(i.rules.notional.min)?;
    let sl = a.mul(&sr, &r(p.l)?)?;
    let dust = a.sub(&qr, &sr)?;
    let dust = a.mul(&dust, &price)?;
    let rem = a.mul(&r(rh)?, &af)?;
    let rem = a.mul(&rem, &p.t)?;
    let charge = a.add(&sl, &dust)?;
    let charge = a.add(&charge, &rem)?;
    if a.compare(&charge, &p.cap)? == Ordering::Greater {
        return Err(deny(CandidateRefusal::ChargeCap));
    }
    let gross = a.mul(&qr, &price)?;
    if a.compare(&gross, &p.physical)? == Ordering::Greater {
        return Err(deny(CandidateRefusal::PhysicalCap));
    }
    if let Some(cap) = i.policy.max_entry_notional() {
        if a.compare(&gross, &r(cap)?)? == Ordering::Greater {
            return Err(deny(CandidateRefusal::OperatorCap));
        }
    }
    let entry = a.mul(&gross, &p.u)?;
    if a.compare(&entry, &minimum)? == Ordering::Less {
        return Err(deny(CandidateRefusal::EntryMinimum));
    }
    let stop = a.mul(&sr, &af)?;
    let stop = a.mul(&stop, &p.u)?;
    if a.compare(&stop, &minimum)? == Ordering::Less {
        return Err(deny(CandidateRefusal::ProtectionMinimum));
    }
    let n = i.rules.notional;
    if n.apply_min || n.apply_max {
        let v = r(i.filter_price.as_ref().ok_or(SizingError::FilterPrice)?.value)?;
        let maximum = r(n.max)?;
        for qty in [&qr, &sr] {
            let value = a.mul(qty, &v)?;
            if (n.apply_min && a.compare(&value, &minimum)? == Ordering::Less)
                || (n.apply_max && a.compare(&value, &maximum)? == Ordering::Greater)
            {
                return Err(deny(CandidateRefusal::Filter));
            }
        }
    }
    for qty in [&sr, &qr] {
        let down = a.floor_step(qty, &step)?;
        if a.compare(&down, qty)? != Ordering::Equal {
            return Err(SizingError::Rules);
        }
    }
    let charge = native(&mut a, &charge)?;
    let gross = native(&mut a, &gross)?;
    debug_assert!(a.operations() <= 37);
    Ok(SizedAmounts {
        protection: s,
        gross_entry: q,
        adverse_fill: p.af,
        loss_per_unit: p.l,
        remainder: rh,
        charge,
        gross_notional: gross,
    })
}
/// Evaluate raw values only. Current receipt/basis/filter qualification and
/// serialized admission are later owners; this result cannot authorize a send.
pub fn calculate_balance_candidate(i: &SizingInputs) -> Result<SizingCalculation, SizingError> {
    let p = prepare(i)?;
    let mut reason = CandidateRefusal::Quantity;
    for k in 0..4 {
        let mut terminal = false;
        match candidate(i, &p, k, &mut terminal) {
            Ok(amounts) => {
                return Ok(SizingCalculation {
                    selected_lot_step: p.lot.step,
                    amounts,
                    inputs: i.clone(),
                    attempts: k + 1,
                })
            },
            Err(SizingError::Unrepresentable) => reason = CandidateRefusal::Unrepresentable,
            Err(SizingError::Exhausted { reason: r, .. }) => reason = r,
            Err(e) => return Err(e),
        }
        if terminal {
            return Err(SizingError::Exhausted { reason, attempts: k + 1 });
        }
    }
    Err(SizingError::Exhausted { reason, attempts: 4 })
}
#[cfg(test)]
#[path = "sizing_tests.rs"]
mod tests;
