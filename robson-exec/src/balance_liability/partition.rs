use rust_decimal::Decimal;

use super::FactError;
use crate::venue_ledger::{exact_add, exact_sub};

/// Describes supplied B; neither variant proves fee completeness or freshness.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeeEvidence {
    ProvisionalBound,
    FinalCommission,
}

/// Untrusted recorded values. X is supplied, never chosen or rounded here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecordedPartition {
    pub gross: Decimal,
    pub base_fee: Decimal,
    pub owned: Decimal,
    pub consumed: Decimal,
    pub fee_evidence: FeeEvidence,
}

/// Arithmetic vocabulary only: in a deficit G does not exist.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Remainder {
    Quantity(Decimal),
    Deficit { excess: Decimal },
}

/// Exact facts about supplied values, not durable or settlement authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PartitionFacts {
    net: Decimal,
    dust: Decimal,
    remainder: Remainder,
    fee_evidence: FeeEvidence,
}

impl PartitionFacts {
    pub fn net(&self) -> Decimal {
        self.net
    }
    pub fn dust(&self) -> Decimal {
        self.dust
    }
    pub fn remainder(&self) -> Remainder {
        self.remainder
    }
    pub fn fee_evidence(&self) -> FeeEvidence {
        self.fee_evidence
    }
}

/// Derive D and G/deficit using at most five exact additions/subtractions.
/// A final-fee input still cannot initialize, increase or reduce durable X.
pub fn partition_facts(input: RecordedPartition) -> Result<PartitionFacts, FactError> {
    let RecordedPartition {
        gross,
        base_fee,
        owned,
        consumed,
        fee_evidence,
    } = input;
    if gross < Decimal::ZERO
        || base_fee < Decimal::ZERO
        || owned < Decimal::ZERO
        || consumed < Decimal::ZERO
        || base_fee > gross
    {
        return Err(FactError::InvalidQuantity);
    }
    let net = exact_sub(gross, base_fee).map_err(|_| FactError::Unrepresentable)?;
    if owned > net {
        return Err(FactError::InvalidQuantity);
    }
    let dust = exact_sub(net, owned).map_err(|_| FactError::Unrepresentable)?;
    let fee_and_dust = exact_add(base_fee, dust).map_err(|_| FactError::Unrepresentable)?;
    if exact_add(fee_and_dust, owned).map_err(|_| FactError::Unrepresentable)? != gross {
        return Err(FactError::InvalidQuantity);
    }
    let remainder = if consumed <= owned {
        Remainder::Quantity(exact_sub(owned, consumed).map_err(|_| FactError::Unrepresentable)?)
    } else {
        Remainder::Deficit {
            excess: exact_sub(consumed, owned).map_err(|_| FactError::Unrepresentable)?,
        }
    };
    Ok(PartitionFacts { net, dust, remainder, fee_evidence })
}
