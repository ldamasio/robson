//! Balance-account cost inputs and pure immutable terms. This module adds no
//! worker, clock implementation, store, production caller or send permission.

pub(crate) mod age;
mod config;
pub mod exact;
mod terms;

pub use age::{AgeError, AgeInterval, CostAgeAuthority};
pub use config::{parse_dated_override, CostPolicy, MAX_OVERRIDE_BYTES};
pub use terms::{
    select_costs, CommissionComponents, CostCandidate, CostError, CostIdentity, CostSource,
    CostTerms, DatedOverrideEvidence, EvidenceIdentity, VenueCostEvidence, COST_VALIDATION_VERSION,
};

#[cfg(test)]
mod tests;
