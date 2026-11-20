//! Balance-account ledger values (ADR-0056 step 8e). Persistence follows in
//! the Postgres ledger; these values neither admit nor submit an order.

use std::{fmt, str::FromStr};

use chrono::{DateTime, Utc};
use robson_domain::PositionId;
use rust_decimal::Decimal;
use thiserror::Error;
use uuid::Uuid;

use crate::{order_journal::IntentKind, venue::VenueId};

mod orders;
pub use orders::{
    contribution, merge_order, AdmissionTerms, LegPartition, OrderAnomaly, OrderKey, OrderLeg,
    OrderMerge, OrderWriteResult, StoredOrder,
};

mod trades;
pub use trades::{plan_trades, StoredTrade, TradePlan, TradeWriteResult};

/// The orders whose execution can belong to a governed leg.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegOrderKind {
    Entry,
    Stop,
    Sell,
}

impl LegOrderKind {
    pub fn from_intent(kind: IntentKind) -> Option<Self> {
        match kind {
            IntentKind::Entry => Some(Self::Entry),
            IntentKind::Protection => Some(Self::Stop),
            IntentKind::Exit => Some(Self::Sell),
            IntentKind::Cancel | IntentKind::SweepClose => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Entry => "entry",
            Self::Stop => "stop",
            Self::Sell => "sell",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OrderOwner {
    Leg {
        intent_id: Uuid,
        position_id: PositionId,
        kind: LegOrderKind,
    },
    Unauthored,
    Discovered,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderLink {
    pub venue: VenueId,
    pub account_scope: String,
    pub owner: OrderOwner,
    pub client_order_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClockVerdict {
    Valid,
    Invalid,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClockEvidence {
    pub sample_id: Uuid,
    pub wall_delta_ms: i64,
    pub mono_delta_ms: i64,
    pub verdict: ClockVerdict,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeenAt {
    /// Robson's wall clock, sampled after the venue answer arrived.
    pub at: DateTime<Utc>,
    pub clock: Option<ClockEvidence>,
    /// Original unqualified exact bracket; legacy rows remain absent.
    pub capture: Option<crate::clock_records::RawClockCapture>,
}

/// An unsigned venue integer id, printed without signs or leading zeroes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct VenueIntId(u64);

impl FromStr for VenueIntId {
    type Err = LedgerError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(LedgerError::InvalidEvidence("invalid venue integer id".to_string()));
        }
        value
            .parse::<u64>()
            .map(Self)
            .map_err(|_| LedgerError::InvalidEvidence("venue integer id exceeds u64".to_string()))
    }
}

impl fmt::Display for VenueIntId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Checked decimal arithmetic must also reject silent precision reduction.
pub fn exact_add(left: Decimal, right: Decimal) -> Result<Decimal, LedgerError> {
    exact_result(left.checked_add(right), left, right)
}

pub fn exact_sub(left: Decimal, right: Decimal) -> Result<Decimal, LedgerError> {
    exact_result(left.checked_sub(right), left, right)
}

// rust_decimal returns the other operand unaligned, at its own scale, when one
// operand is zero. That result is exact; only a nonzero pair can be rounded.
fn exact_result(
    value: Option<Decimal>,
    left: Decimal,
    right: Decimal,
) -> Result<Decimal, LedgerError> {
    let scale = left.scale().max(right.scale());
    let zero = left.is_zero() || right.is_zero();
    value
        .filter(|value| zero || value.scale() >= scale)
        .ok_or(LedgerError::Unrepresentable)
}

/// `Backend` proves nothing landed; `Uncertain` cannot make that promise.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum LedgerError {
    #[error("no accounting row")]
    NoAccountingRow,
    #[error("unknown order")]
    UnknownOrder,
    #[error("order identity conflict")]
    IdentityConflict,
    #[error("trade identity conflict")]
    TradeConflict,
    #[error("leg route conflict")]
    RouteConflict,
    #[error("invalid evidence: {0}")]
    InvalidEvidence(String),
    #[error("decimal result is not exactly representable")]
    Unrepresentable,
    #[error("ledger invariant: {0}")]
    Invariant(String),
    #[error("ledger backend failure, nothing landed: {0}")]
    Backend(String),
    #[error("ledger commit unanswered, it may have landed: {0}")]
    Uncertain(String),
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use rust_decimal_macros::dec;

    use super::*;

    #[test]
    fn exact_arithmetic_refuses_overflow_and_rounding() {
        let tiny = dec!(0.0000000000000000000000000001);
        for result in [
            exact_add(Decimal::MAX, Decimal::ONE),
            exact_sub(Decimal::MIN, Decimal::ONE),
            exact_add(Decimal::MAX, tiny),
            exact_sub(Decimal::MAX, tiny),
            exact_add(Decimal::MIN, -tiny),
        ] {
            assert_eq!(result, Err(LedgerError::Unrepresentable));
        }
        assert_eq!(exact_add(dec!(2.00), dec!(-0.125)), Ok(dec!(1.875)));
        assert_eq!(exact_sub(dec!(2.00), dec!(-0.125)), Ok(dec!(2.125)));
        // A zero operand comes back unaligned at the other's lower scale.
        let zero = dec!(0.00000000);
        assert_eq!(exact_add(dec!(0.0003), zero), Ok(dec!(0.0003)));
        assert_eq!(exact_add(zero, dec!(0.0003)), Ok(dec!(0.0003)));
        assert_eq!(exact_sub(dec!(0.25), zero), Ok(dec!(0.25)));
        assert_eq!(exact_sub(zero, dec!(0.25)), Ok(dec!(-0.25)));
        assert_eq!(exact_add(Decimal::MAX, zero), Ok(Decimal::MAX));
    }

    #[test]
    fn venue_ids_are_canonical() {
        for (raw, canonical) in [
            ("000123", "123"),
            ("0", "0"),
            ("000", "0"),
            ("18446744073709551615", "18446744073709551615"),
        ] {
            assert_eq!(raw.parse::<VenueIntId>().unwrap().to_string(), canonical);
        }
        for raw in [
            "",
            "-1",
            "+1",
            " 1",
            "1 ",
            "1.0",
            "1e2",
            "１２",
            "18446744073709551616",
        ] {
            assert!(raw.parse::<VenueIntId>().is_err(), "{raw:?}");
        }
    }

    #[test]
    fn only_leg_order_intents_have_a_leg_kind() {
        for (intent, expected) in [
            (IntentKind::Entry, Some(LegOrderKind::Entry)),
            (IntentKind::Protection, Some(LegOrderKind::Stop)),
            (IntentKind::Exit, Some(LegOrderKind::Sell)),
            (IntentKind::Cancel, None),
            (IntentKind::SweepClose, None),
        ] {
            assert_eq!(LegOrderKind::from_intent(intent), expected);
        }
    }

    proptest! {
        #[test]
        fn canonical_ids_round_trip(id: u64, zeroes in 0..16usize) {
            let raw = format!("{}{id}", "0".repeat(zeroes));
            prop_assert_eq!(raw.parse::<VenueIntId>().unwrap().to_string(), id.to_string());
        }

        #[test]
        fn exact_arithmetic_preserves_scaled_integer_results(a: i64, b: i64, scale in 0..=28u32) {
            let left = Decimal::from_i128_with_scale(i128::from(a), scale);
            let right = Decimal::from_i128_with_scale(i128::from(b), scale);
            prop_assert_eq!(exact_add(left, right), Ok(Decimal::from_i128_with_scale(i128::from(a) + i128::from(b), scale)));
            prop_assert_eq!(exact_sub(left, right), Ok(Decimal::from_i128_with_scale(i128::from(a) - i128::from(b), scale)));
        }

        #[test]
        fn a_zero_operand_of_any_scale_is_exact(a: i64, scale in 0..=28u32, zero_scale in 0..=28u32) {
            let value = Decimal::from_i128_with_scale(i128::from(a), scale);
            let zero = Decimal::from_i128_with_scale(0, zero_scale);
            prop_assert_eq!(exact_add(value, zero), Ok(value));
            prop_assert_eq!(exact_add(zero, value), Ok(value));
            prop_assert_eq!(exact_sub(value, zero), Ok(value));
            prop_assert_eq!(exact_sub(zero, value), Ok(-value));
        }
    }
}
