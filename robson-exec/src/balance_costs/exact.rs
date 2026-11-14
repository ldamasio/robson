//! Exact, bounded decimal ingress for balance-account costs and spot inputs.
//! This deliberately does not change serde's global Decimal behavior.

use std::{fmt, str::FromStr};

use rust_decimal::Decimal;
use serde::{de, Deserialize, Deserializer};

/// Largest accepted decimal text token, before any arithmetic/allocation here.
pub const MAX_DECIMAL_TEXT_BYTES: usize = 128;
const MAX_COEFFICIENT: u128 = (1_u128 << 96) - 1;

/// A decimal whose original string was representable without rounding.
/// Numeric JSON tokens, exponents and a discarded nonzero digit are refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExactDecimal(Decimal);

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("expected a bounded, exactly representable plain decimal string")]
pub struct ExactDecimalError;

impl ExactDecimal {
    pub fn value(self) -> Decimal {
        self.0
    }
}

impl FromStr for ExactDecimal {
    type Err = ExactDecimalError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        if text.is_empty() || text.len() > MAX_DECIMAL_TEXT_BYTES {
            return Err(ExactDecimalError);
        }
        let (negative, digits) = match text.strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, text),
        };
        let (whole, fraction) = match digits.split_once('.') {
            Some((whole, fraction)) if !fraction.is_empty() => (whole, fraction),
            Some(_) => return Err(ExactDecimalError),
            None => (digits, ""),
        };
        if whole.is_empty()
            || !whole.bytes().all(|b| b.is_ascii_digit())
            || !fraction.bytes().all(|b| b.is_ascii_digit())
        {
            return Err(ExactDecimalError);
        }
        let fraction = fraction.trim_end_matches('0');
        if fraction.len() > 28 {
            return Err(ExactDecimalError);
        }
        let mut coefficient = 0_u128;
        for byte in whole.bytes().chain(fraction.bytes()) {
            coefficient = coefficient
                .checked_mul(10)
                .and_then(|n| n.checked_add(u128::from(byte - b'0')))
                .filter(|n| *n <= MAX_COEFFICIENT)
                .ok_or(ExactDecimalError)?;
        }
        Ok(Self(Decimal::from_parts(
            coefficient as u32,
            (coefficient >> 32) as u32,
            (coefficient >> 64) as u32,
            negative,
            fraction.len() as u32,
        )))
    }
}

impl<'de> Deserialize<'de> for ExactDecimal {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl de::Visitor<'_> for Visitor {
            type Value = ExactDecimal;
            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an exact plain decimal string")
            }
            fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
                value.parse().map_err(E::custom)
            }
        }
        deserializer.deserialize_str(Visitor)
    }
}

/// Spot amounts/rules/rates: reject negative original values before conversion
/// could erase their sign. A mathematical negative zero is just zero.
pub fn nonnegative<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Decimal, D::Error> {
    let value = ExactDecimal::deserialize(deserializer)?.value();
    if value < Decimal::ZERO {
        return Err(de::Error::custom("negative spot decimal"));
    }
    Ok(value)
}

#[cfg(test)]
mod tests;
