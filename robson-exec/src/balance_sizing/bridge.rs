use rust_decimal::Decimal as D;

use super::{
    arithmetic::{Arithmetic, MathError, Rational},
    inputs::*,
};
impl From<MathError> for SizingError {
    fn from(e: MathError) -> Self {
        match e {
            MathError::WorkLimit => Self::TechnicalWorkLimit,
            MathError::Unrepresentable => Self::Unrepresentable,
            _ => Self::Geometry,
        }
    }
}
pub(super) fn r(v: D) -> Result<Rational, SizingError> {
    Ok(Rational::decimal(v)?)
}
pub(super) fn native(a: &mut Arithmetic, v: &Rational) -> Result<D, SizingError> {
    Ok(a.decimal(v)?)
}
