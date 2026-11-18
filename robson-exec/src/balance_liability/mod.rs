//! Unqualified balance-liability facts (ADR-0056 step 8i-1).
//!
//! These calculations neither establish complete/current evidence nor authorize
//! protection, settlement, a transition, or a send. No runtime caller uses
//! them. Initial X, its quantization/reduction, complete O(L), and R1/R3 are
//! later work.
//!
//! Successful fact values cannot be constructed without validation:
//! ```compile_fail
//! use robson_exec::balance_liability::PartitionFacts;
//! let forged = PartitionFacts {};
//! ```

mod order_facts;
mod partition;
mod transition;

pub use order_facts::{order_facts, OrderFacts};
pub use partition::{partition_facts, FeeEvidence, PartitionFacts, RecordedPartition, Remainder};
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
