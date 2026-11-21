//! Exact Spot leg quantity over the private rational frame (ADR-0056 8i-2).
//! See docs/architecture/adr0056/8i-liability-facts.md. The frame stays
//! private; only these two calculations are crate-visible.
use std::cmp::Ordering;

use rust_decimal::Decimal;

use super::arithmetic::{Arithmetic, MathError, Rational};
use crate::balance_liability::{FactError, LegFeeInput};

impl From<MathError> for FactError {
    fn from(e: MathError) -> Self {
        match e {
            // At most five native imports and six calls stay far below the
            // frame limits; an unexpected limit still refuses, never rounds.
            MathError::Unrepresentable | MathError::WorkLimit => Self::Unrepresentable,
            MathError::Negative | MathError::ZeroDenominator => Self::InvalidQuantity,
        }
    }
}

/// First (B, X) with X = floor_step(Q − B). Provisional B̂ = Q × f_L is exact
/// and must export exactly. At most three imports and five calls.
pub(crate) fn initial_executable(
    gross: Decimal,
    lot_step: Decimal,
    fee: LegFeeInput,
) -> Result<(Decimal, Decimal), FactError> {
    if lot_step <= Decimal::ZERO {
        return Err(FactError::InvalidQuantity);
    }
    let mut math = Arithmetic::default();
    let q = Rational::decimal(gross)?;
    let step = Rational::decimal(lot_step)?;
    let (base_fee, fee) = match fee {
        LegFeeInput::FinalCommission(base_fee) => (base_fee, Rational::decimal(base_fee)?),
        LegFeeInput::ProvisionalBound { fraction } => {
            if fraction > Decimal::ONE {
                return Err(FactError::InvalidQuantity);
            }
            let bound = math.mul(&q, &Rational::decimal(fraction)?)?;
            (math.decimal(&bound)?, bound)
        },
    };
    // Negative Q − B is B > Q.
    let net = math.sub(&q, &fee)?;
    let executable = math.floor_step(&net, &step)?;
    Ok((base_fee, math.decimal(&executable)?))
}

/// X′ = min(prior X, floor_step(Q − B)). Selection precedes export, so a
/// larger unselected floor cannot refuse. Four imports, at most six calls.
pub(crate) fn reduced_executable(
    gross: Decimal,
    base_fee: Decimal,
    prior_executable: Decimal,
    lot_step: Decimal,
) -> Result<Decimal, FactError> {
    if lot_step <= Decimal::ZERO || base_fee > gross || prior_executable > gross {
        return Err(FactError::InvalidQuantity);
    }
    let mut math = Arithmetic::default();
    let q = Rational::decimal(gross)?;
    let fee = Rational::decimal(base_fee)?;
    let prior = Rational::decimal(prior_executable)?;
    let step = Rational::decimal(lot_step)?;
    let prior_floor = math.floor_step(&prior, &step)?;
    if math.compare(&prior_floor, &prior)? != Ordering::Equal {
        return Err(FactError::InvalidQuantity);
    }
    let net = math.sub(&q, &fee)?;
    let floor = math.floor_step(&net, &step)?;
    match math.compare(&floor, &prior)? {
        Ordering::Less => Ok(math.decimal(&floor)?),
        // X never rises: a lower final fee leaves X and adds the excess to D.
        Ordering::Equal | Ordering::Greater => Ok(prior_executable),
    }
}

#[cfg(test)]
#[path = "leg_quantity_tests.rs"]
mod tests;
