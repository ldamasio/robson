use rust_decimal::Decimal;

use super::{partition_facts, FactError, FeeEvidence, PartitionFacts, RecordedPartition};
use crate::balance_sizing::{initial_executable, reduced_executable};

/// Persisted fee input for a first X. Neither variant derives a rate, converts
/// basis points or proves freshness/finality.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegFeeInput {
    /// Actual base commission B.
    FinalCommission(Decimal),
    /// Persisted dimensionless f_L in [0,1]; B̂=Q*f_L must be exact.
    ProvisionalBound { fraction: Decimal },
}

/// Untrusted recorded values. No prior executable value is checked here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InitialQuantityInput {
    pub gross: Decimal,
    pub lot_step: Decimal,
    pub consumed: Decimal,
    pub fee: LegFeeInput,
}

/// Untrusted recorded values; the lot step is the one persisted with prior X.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FinalFeeInput {
    pub gross: Decimal,
    pub base_fee: Decimal,
    pub prior_executable: Decimal,
    pub lot_step: Decimal,
    pub consumed: Decimal,
}

/// Exact B, X and partition over supplied values. Not a durable value, order
/// set, position authority, fee-finality proof or send grant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LegQuantity {
    base_fee: Decimal,
    executable: Decimal,
    facts: PartitionFacts,
}

impl LegQuantity {
    pub fn base_fee(&self) -> Decimal {
        self.base_fee
    }
    pub fn executable(&self) -> Decimal {
        self.executable
    }
    pub fn facts(&self) -> PartitionFacts {
        self.facts
    }
}

/// Candidate first X=floor_step(Q−B). Zero X is a valid calculation.
pub fn initial_quantity(input: InitialQuantityInput) -> Result<LegQuantity, FactError> {
    let fee_evidence = match input.fee {
        LegFeeInput::FinalCommission(_) => FeeEvidence::FinalCommission,
        LegFeeInput::ProvisionalBound { .. } => FeeEvidence::ProvisionalBound,
    };
    let (base_fee, executable) = initial_executable(input.gross, input.lot_step, input.fee)?;
    candidate(input.gross, base_fee, executable, input.consumed, fee_evidence)
}

/// Candidate X′=min(prior X, floor_step(Q−B)) for an exact final B.
pub fn final_fee_quantity(input: FinalFeeInput) -> Result<LegQuantity, FactError> {
    let executable =
        reduced_executable(input.gross, input.base_fee, input.prior_executable, input.lot_step)?;
    candidate(
        input.gross,
        input.base_fee,
        executable,
        input.consumed,
        FeeEvidence::FinalCommission,
    )
}

fn candidate(
    gross: Decimal,
    base_fee: Decimal,
    executable: Decimal,
    consumed: Decimal,
    fee_evidence: FeeEvidence,
) -> Result<LegQuantity, FactError> {
    // Consumed is never clamped: consumed>X stays an explicit deficit.
    let facts = partition_facts(RecordedPartition {
        gross,
        base_fee,
        owned: executable,
        consumed,
        fee_evidence,
    })?;
    Ok(LegQuantity { base_fee, executable, facts })
}

#[cfg(test)]
#[path = "quantity_tests.rs"]
mod tests;
