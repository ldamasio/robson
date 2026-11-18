//! Bounded nonnegative rationals. See docs/architecture/adr0056/8g-sizing.md.
use std::cmp::Ordering;

use num_bigint::BigUint;
use rust_decimal::Decimal;

pub(super) const MAX_BITS: u64 = 4096;
pub(super) const MAX_OPERATIONS: u16 = 128;
const MAX_EXPORT_SCALE: u32 = 28;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum MathError {
    Negative,
    ZeroDenominator,
    Unrepresentable,
    WorkLimit,
}

/// Not a qualified input, price, quantity or admission result.
#[derive(Clone, Debug)]
pub(super) struct Rational {
    numerator: BigUint,
    denominator: BigUint,
}

impl Rational {
    fn new(numerator: BigUint, denominator: BigUint) -> Result<Self, MathError> {
        if denominator == BigUint::from(0_u8) {
            return Err(MathError::ZeroDenominator);
        }
        if numerator.bits() > MAX_BITS || denominator.bits() > MAX_BITS {
            return Err(MathError::WorkLimit);
        }
        Ok(Self { numerator, denominator })
    }

    pub(super) fn decimal(value: Decimal) -> Result<Self, MathError> {
        if value < Decimal::ZERO {
            return Err(MathError::Negative);
        }
        // Decimal guarantees scale <=28 and a96-bit coefficient; negative zero
        // is mathematically zero. No parsing or rounded Decimal arithmetic.
        Self::new(
            BigUint::from(value.mantissa().unsigned_abs()),
            BigUint::from(10_u8).pow(value.scale()),
        )
    }
}

/// One bounded arithmetic frame. This is a work budget, never a risk limit.
/// The future expression graph must prove it fits before becoming a caller.
#[derive(Default)]
pub(super) struct Arithmetic {
    operations: u16,
}

impl Arithmetic {
    fn charge(&mut self) -> Result<(), MathError> {
        if self.operations == MAX_OPERATIONS {
            return Err(MathError::WorkLimit);
        }
        self.operations += 1;
        #[cfg(test)]
        trace("call", 1);
        Ok(())
    }

    pub(super) fn operations(&self) -> u16 {
        self.operations
    }

    pub(super) fn add(&mut self, a: &Rational, b: &Rational) -> Result<Rational, MathError> {
        self.charge()?;
        let left = product(&a.numerator, &b.denominator)?;
        let right = product(&b.numerator, &a.denominator)?;
        Rational::new(sum(&left, &right)?, product(&a.denominator, &b.denominator)?)
    }

    pub(super) fn sub(&mut self, a: &Rational, b: &Rational) -> Result<Rational, MathError> {
        self.charge()?;
        let left = product(&a.numerator, &b.denominator)?;
        let right = product(&b.numerator, &a.denominator)?;
        if left < right {
            return Err(MathError::Negative);
        }
        Rational::new(left - right, product(&a.denominator, &b.denominator)?)
    }

    pub(super) fn mul(&mut self, a: &Rational, b: &Rational) -> Result<Rational, MathError> {
        self.charge()?;
        Rational::new(
            product(&a.numerator, &b.numerator)?,
            product(&a.denominator, &b.denominator)?,
        )
    }

    pub(super) fn div(&mut self, a: &Rational, b: &Rational) -> Result<Rational, MathError> {
        self.charge()?;
        if b.numerator == BigUint::from(0_u8) {
            return Err(MathError::ZeroDenominator);
        }
        Rational::new(
            product(&a.numerator, &b.denominator)?,
            product(&a.denominator, &b.numerator)?,
        )
    }

    pub(super) fn compare(&mut self, a: &Rational, b: &Rational) -> Result<Ordering, MathError> {
        self.charge()?;
        Ok(product(&a.numerator, &b.denominator)?.cmp(&product(&b.numerator, &a.denominator)?))
    }

    pub(super) fn floor12(&mut self, value: &Rational) -> Result<Rational, MathError> {
        self.floor_places(value, 12)
    }

    pub(super) fn floor8(&mut self, value: &Rational) -> Result<Rational, MathError> {
        self.floor_places(value, 8)
    }

    fn floor_places(&mut self, value: &Rational, scale: u32) -> Result<Rational, MathError> {
        self.charge()?;
        // Only the two fixed wrappers call this; never a caller-supplied exponent.
        let factor = BigUint::from(10_u8).pow(scale);
        let scaled = product(&value.numerator, &factor)?;
        Rational::new(scaled / &value.denominator, factor)
    }

    pub(super) fn floor_step(
        &mut self,
        value: &Rational,
        step: &Rational,
    ) -> Result<Rational, MathError> {
        self.grid(value, step, false)
    }

    pub(super) fn ceil_step(
        &mut self,
        value: &Rational,
        step: &Rational,
    ) -> Result<Rational, MathError> {
        self.grid(value, step, true)
    }

    fn grid(
        &mut self,
        value: &Rational,
        step: &Rational,
        ceil: bool,
    ) -> Result<Rational, MathError> {
        self.charge()?;
        if step.numerator == BigUint::from(0_u8) {
            return Err(MathError::ZeroDenominator);
        }
        let numerator = product(&value.numerator, &step.denominator)?;
        let denominator = product(&value.denominator, &step.numerator)?;
        let (mut count, remainder) = quotient_remainder(&numerator, &denominator);
        if ceil && remainder != BigUint::from(0_u8) {
            count = sum(&count, &BigUint::from(1_u8))?;
        }
        Rational::new(product(&count, &step.numerator)?, step.denominator.clone())
    }

    /// Exact finite Decimal only: at most29 scale attempts, not a rounded
    /// divide. The export attempts are internal work, separate from the128
    /// frame calls.
    pub(super) fn decimal(&mut self, value: &Rational) -> Result<Decimal, MathError> {
        self.charge()?;
        let mut scaled = value.numerator.clone();
        for scale in 0..=MAX_EXPORT_SCALE {
            #[cfg(test)]
            trace("export_div", 1);
            let (coefficient, remainder) = quotient_remainder(&scaled, &value.denominator);
            if coefficient.bits() > 96 {
                // Later scales only increase the coefficient.
                return Err(MathError::Unrepresentable);
            }
            if remainder == BigUint::from(0_u8) {
                let words = coefficient.to_u32_digits(); // <=3 words, checked above
                return Ok(Decimal::from_parts(
                    words.first().copied().unwrap_or(0),
                    words.get(1).copied().unwrap_or(0),
                    words.get(2).copied().unwrap_or(0),
                    false,
                    scale,
                ));
            }
            if scale < MAX_EXPORT_SCALE {
                scaled = product(&scaled, &BigUint::from(10_u8))?;
            }
        }
        Err(MathError::Unrepresentable)
    }
}

fn product(a: &BigUint, b: &BigUint) -> Result<BigUint, MathError> {
    #[cfg(test)]
    trace("bits", a.bits() + b.bits());
    if a.bits() == 0 || b.bits() == 0 {
        return Ok(BigUint::from(0_u8));
    }
    // Sum-of-lengths bounds the product BEFORE allocating it. Conservative at
    // the implementation ceiling; the future formula proof must stay below it.
    if a.bits() + b.bits() > MAX_BITS {
        return Err(MathError::WorkLimit);
    }
    Ok(a * b)
}

fn sum(a: &BigUint, b: &BigUint) -> Result<BigUint, MathError> {
    #[cfg(test)]
    trace("bits", a.bits().max(b.bits()) + 1);
    if a.bits().max(b.bits()) + 1 > MAX_BITS {
        return Err(MathError::WorkLimit);
    }
    Ok(a + b)
}

fn quotient_remainder(numerator: &BigUint, denominator: &BigUint) -> (BigUint, BigUint) {
    // All callers proved denominator>0 and inputs<=4096bits. q*d<=n, so
    // reconstructing the remainder cannot grow beyond the numerator's bound.
    let quotient = numerator / denominator;
    let remainder = numerator - &quotient * denominator;
    (quotient, remainder)
}

#[cfg(test)]
#[path = "arithmetic_tests.rs"]
mod tests;

#[cfg(test)]
thread_local! { pub(super) static TRACE: std::cell::RefCell<Vec<(&'static str,u64)>> = const {std::cell::RefCell::new(Vec::new())}; }
#[cfg(test)]
pub(super) fn trace(kind: &'static str, value: u64) {
    TRACE.with_borrow_mut(|t| t.push((kind, value)));
}
