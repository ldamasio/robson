//! Floors (v16 §C4), carried excess and the month partition (§C7), in exact
//! decimal arithmetic. A floor rounds down to the finest amount the ledger
//! holds (a `Decimal`: scale at most 28, a 96-bit coefficient); every other
//! result is exact or refused.

use std::ops::Range;

use chrono::{DateTime, Utc};
use num_bigint::{BigInt, BigUint, Sign};
use rust_decimal::Decimal;

use super::{ensure, ledger_orders, nonneg, sub, Hold, LegOrder, LegTerms};
use crate::venue_ledger::LegOrderKind;

/// The floor of a remainder protected by a stop, at a mark.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Floor {
    /// AF = t × (1 − gap).
    pub adverse_fill: Decimal,
    /// p = min(AF, P × (1 − gap)).
    pub price: Decimal,
    /// F = max(0, G − R̂) × p × (1 − f), rounded down.
    pub quote: Decimal,
}

/// F_B of a remainder `g` = G_B protected at trigger `t` = t_B, at `mark` =
/// P_B; or F_now of the current remainder and stop at a supplied mark. gap =
/// `gap_bps` / 10⁴, f = `taker_bps` / 10⁴. A trigger must be positive (M36).
pub fn floor_at(g: Decimal, t: Decimal, mark: Decimal, terms: &LegTerms) -> Result<Floor, Hold> {
    if t <= Decimal::ZERO {
        return Err(Hold::Invalid);
    }
    let gap = Exact::complement_bps(terms.gap_bps)?;
    let adverse_fill = Exact::of(t)?.times(&gap).exact()?;
    let marked = Exact::of(mark)?.times(&gap).exact()?;
    let price = adverse_fill.min(marked);
    let held = sub(nonneg(g)?, nonneg(terms.remainder_reserve)?)?.max(Decimal::ZERO);
    let fee = Exact::complement_bps(terms.taker_bps)?;
    let quote = Exact::of(held)?.times(&Exact::of(price)?).times(&fee).floor()?;
    Ok(Floor { adverse_fill, price, quote })
}

/// Δcash of a leg's trades by venue time in `month`, half-open: for month K
/// `B_K..B_{K+1}`. A sell adds q × p less its quote fee, a buy (an entry in
/// flight at B) subtracts q × p; a write-off adds nothing. Orders the ledger
/// would not hold (M36) hold.
pub fn delta_cash(orders: &[LegOrder], month: Range<DateTime<Utc>>) -> Result<Decimal, Hold> {
    ensure(ledger_orders(orders))?;
    let mut cash = Vec::new();
    for order in orders {
        for fill in order.fills.iter().filter(|f| month.contains(&f.trade_time)) {
            let quote = product(fill.qty, fill.price)?;
            cash.push(match order.kind {
                LegOrderKind::Entry => -quote,
                LegOrderKind::Stop | LegOrderKind::Sell => sub(quote, fill.commission)?,
            });
        }
    }
    sum(cash)
}

/// CE_lat, latent risk of K while the leg is open in K: max(0, F_B − Δcash −
/// F_now), F_now being [`floor_at`] of its current remainder and stop (0
/// without one). With F_now = F_B(K + 1), the K + 1 floor of a leg still open
/// at B_{K+1}, it is K's prior-month excess, kept on K + 1's leg row.
pub fn latent_excess(f_b: Decimal, cash: Decimal, f_now: Decimal) -> Result<Decimal, Hold> {
    Ok(sum([f_b, -cash, -f_now])?.max(Decimal::ZERO))
}

/// CE_real, consumption of K dated at the close, once the leg's close settles
/// in K: max(0, F_B − Δcash). A result at or above its floor gives 0 (SD15).
pub fn realized_excess(f_b: Decimal, cash: Decimal) -> Result<Decimal, Hold> {
    Ok(sub(f_b, cash)?.max(Decimal::ZERO))
}

/// N_K = cohort net of K (Σ E1–E4 dated in K of legs with cohort K, 8l-6's
/// reader) − Σ CE_real dated in K of the legs carried into K (each ≥ 0).
pub fn month_net(cohort_net: Decimal, realized: &[Decimal]) -> Result<Decimal, Hold> {
    let realized = realized.iter().map(|ce| nonneg(*ce).map(|ce| -ce));
    sum(realized.chain([Ok(cohort_net)]).collect::<Result<Vec<_>, _>>()?)
}

/// Σ values, exact and independent of their order: summed as integers at the
/// finest scale, converted once (refused unless a `Decimal` holds it).
pub(super) fn sum(values: impl IntoIterator<Item = Decimal>) -> Result<Decimal, Hold> {
    let values: Vec<Decimal> = values.into_iter().collect();
    let scale = values.iter().map(Decimal::scale).max().unwrap_or(0);
    let scaled = |v: &Decimal| BigInt::from(v.mantissa()) * BigInt::from(pow10(scale - v.scale()));
    let total: BigInt = values.iter().map(scaled).sum();
    let mut value = Exact(total.magnitude().clone(), scale).exact()?;
    value.set_sign_negative(total.sign() == Sign::Minus);
    Ok(value)
}

/// `left × right`, exact or refused.
pub(super) fn product(left: Decimal, right: Decimal) -> Result<Decimal, Hold> {
    Exact::of(left)?.times(&Exact::of(right)?).exact()
}

/// `Exact(n, scale)`: the nonnegative exact decimal n / 10^scale, before any
/// rounding.
struct Exact(BigUint, u32);

impl Exact {
    fn of(value: Decimal) -> Result<Self, Hold> {
        if value < Decimal::ZERO {
            return Err(Hold::Invalid);
        }
        Ok(Self(BigUint::from(value.mantissa().unsigned_abs()), value.scale()))
    }

    /// 1 − bps / 10⁴, for 0 ≤ bps < 10⁴.
    fn complement_bps(bps: Decimal) -> Result<Self, Hold> {
        let Self(n, scale) = Self::of(bps)?;
        let one = pow10(scale + 4);
        (n < one).then(|| Self(one - n, scale + 4)).ok_or(Hold::Invalid)
    }

    fn times(&self, other: &Self) -> Self {
        Self(&self.0 * &other.0, self.1 + other.1)
    }

    /// The largest `Decimal` not above this value: its floor at the finest
    /// scale whose coefficient fits.
    fn floor(&self) -> Result<Decimal, Hold> {
        (0..=self.1.min(28))
            .rev()
            .find_map(|scale| {
                let coefficient = &self.0 / pow10(self.1 - scale);
                let coefficient = i128::try_from(u128::try_from(&coefficient).ok()?).ok()?;
                Decimal::try_from_i128_with_scale(coefficient, scale).ok()
            })
            .ok_or(Hold::Invalid)
    }

    /// This value as a `Decimal`, refused unless exactly representable.
    fn exact(&self) -> Result<Decimal, Hold> {
        let value = self.floor()?;
        let Self(n, scale) = Self::of(value)?;
        (n * pow10(self.1 - scale) == self.0).then_some(value).ok_or(Hold::Invalid)
    }
}

fn pow10(exponent: u32) -> BigUint {
    BigUint::from(10_u8).pow(exponent)
}
