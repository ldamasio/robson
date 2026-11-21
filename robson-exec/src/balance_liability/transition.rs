use std::cmp::Ordering;

use rust_decimal::Decimal;

use super::FactError;

/// Request vocabulary only. No progress, completion or row merge is implied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransitionKind {
    Replace,
    Reprotect,
    Flatten,
    Exit,
}

impl TransitionKind {
    /// Equal rank intentionally does not choose Replace versus Reprotect.
    pub fn compare_strength(self, other: Self) -> Ordering {
        self.rank().cmp(&other.rank())
    }

    fn rank(self) -> u8 {
        match self {
            Self::Replace | Self::Reprotect => 0,
            Self::Flatten => 1,
            Self::Exit => 2,
        }
    }
}

/// Join supplied positive triggers. This does not validate chart provenance or
/// authorize a durable target update, cancellation or replacement.
pub fn join_target(current: Decimal, requested: Decimal) -> Result<Decimal, FactError> {
    if current <= Decimal::ZERO || requested <= Decimal::ZERO {
        return Err(FactError::InvalidTarget);
    }
    Ok(current.max(requested))
}
