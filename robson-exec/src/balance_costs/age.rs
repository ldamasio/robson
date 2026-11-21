//! Receipt-age boundary. The dormant 8h native proof has a private
//! implementation. No wall clock, process-start timestamp or caller-supplied
//! duration can qualify a receipt through this public API.

use std::time::Duration;

use super::terms::{CostIdentity, EvidenceIdentity};

pub(crate) mod sealed {
    pub trait Sealed {}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AgeError {
    #[error("cost receipt age authority is unavailable")]
    Unknown,
    #[error("cost evidence is future dated")]
    Future,
    #[error("cost receipt storage result is unresolved")]
    Unresolved,
    #[error("cost receipt identity does not match")]
    Identity,
}

/// A nonnegative original-age interval from a positively persisted, identity-
/// bound receipt. No public constructor or deserializer exists. The later 8h
/// implementation must validate domain, storage and original timestamp first.
///
/// ```compile_fail
/// use std::time::Duration;
/// use robson_exec::balance_costs::AgeInterval;
/// let forged = AgeInterval { minimum: Duration::ZERO, maximum: Duration::ZERO };
/// ```
#[derive(Debug, Clone, Copy)]
pub struct AgeInterval {
    pub(crate) minimum: Duration,
    pub(crate) maximum: Duration,
}

impl AgeInterval {
    pub(crate) fn fresh(self) -> Result<bool, AgeError> {
        if self.minimum > self.maximum {
            return Err(AgeError::Unknown);
        }
        Ok(self.maximum < Duration::from_secs(24 * 60 * 60))
    }
}

/// Synchronous selection-time assessment; no cached proof supplied by a caller.
/// Implementing the private supertrait is reserved to receipt authorities in
/// this crate. The dormant native positive-evidence path never confirms
/// absence.
///
/// ```compile_fail
/// use robson_exec::balance_costs::{AgeError, AgeInterval, CostAgeAuthority, CostIdentity, EvidenceIdentity};
/// struct Untrusted;
/// impl CostAgeAuthority for Untrusted {
///     fn age(&self, _: &EvidenceIdentity) -> Result<AgeInterval, AgeError> { panic!() }
///     fn confirm_absence(&self, _: &CostIdentity) -> Result<(), AgeError> { Ok(()) }
/// }
/// ```
#[allow(private_bounds)]
pub trait CostAgeAuthority: sealed::Sealed {
    fn age(&self, evidence: &EvidenceIdentity) -> Result<AgeInterval, AgeError>;
    fn confirm_absence(&self, identity: &CostIdentity) -> Result<(), AgeError>;
}
