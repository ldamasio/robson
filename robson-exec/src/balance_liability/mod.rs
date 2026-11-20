//! Unqualified balance-liability facts (ADR-0056 step 8i-1).
//!
//! These calculations neither establish complete/current evidence nor authorize
//! protection, settlement, a transition, or a send. No runtime caller uses
//! them. Exact initial X and its final-fee reduction (8i-2) are candidates
//! only. The venue ledger records the initial candidate once at a terminal
//! entry (EP-1) and the final-fee reduction once its fills corroborate it
//! (EP-2); complete O(L) and R1/R3 are later work.
//!
//! Successful fact values cannot be constructed without validation:
//! ```compile_fail
//! use robson_exec::balance_liability::PartitionFacts;
//! let forged = PartitionFacts {};
//! ```
//!
//! A quantity candidate cannot be forged from a valid partition:
//! ```compile_fail
//! use robson_exec::balance_liability::*;
//! use rust_decimal::Decimal;
//! let facts = partition_facts(RecordedPartition {
//!     gross: Decimal::TEN,
//!     base_fee: Decimal::ZERO,
//!     owned: Decimal::TEN,
//!     consumed: Decimal::ZERO,
//!     fee_evidence: FeeEvidence::FinalCommission,
//! })
//! .unwrap();
//! let forged = LegQuantity { base_fee: Decimal::ZERO, executable: Decimal::TEN, facts };
//! ```

mod order_facts;
mod partition;
mod quantity;
mod transition;

pub use order_facts::{order_facts, OrderFacts};
pub use partition::{partition_facts, FeeEvidence, PartitionFacts, RecordedPartition, Remainder};
pub use quantity::{
    final_fee_quantity, initial_quantity, FinalFeeInput, InitialQuantityInput, LegFeeInput,
    LegQuantity,
};
pub use transition::{join_target, TransitionKind};

/// Bounded local refusal, without copying input payloads into an error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum FactError {
    #[error("invalid recorded quantity partition or order quantities")]
    InvalidQuantity,
    #[error("order does not match the expected governed identity")]
    IdentityMismatch,
    #[error("target trigger must be positive")]
    InvalidTarget,
    #[error("fact is not exactly representable")]
    Unrepresentable,
}

#[cfg(test)]
mod tests;
