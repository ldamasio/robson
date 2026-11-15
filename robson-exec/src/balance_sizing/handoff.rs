//! Copy unqualified calculation data into J's existing admission payload.
use chrono::NaiveDate;
use robson_domain::{PositionId, Symbol};

use super::SizingCalculation;
use crate::order_journal::AdmissionRow;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum HandoffError {
    #[error("candidate position must be non-nil")]
    Position,
    #[error("candidate symbol differs from the calculation")]
    Symbol,
    #[error("candidate month differs from the calculation")]
    Month,
}

/// Unqualified data only: no currentness, capacity, reservation or send grant.
/// J/backend still own intent/venue/scope/tenant/time. The returned public row
/// is mutable; later8i must compare its exact payload and requested Q under
/// real qualification. Neither mapping success nor identity equality does so.
/// No new arithmetic, I/O or clock read; hash encoding visits exactly32 bytes.
pub fn candidate_admission(
    calculation: &SizingCalculation,
    position_id: PositionId,
    symbol: &Symbol,
    decided_month: NaiveDate,
) -> Result<AdmissionRow, HandoffError> {
    let inputs = calculation.inputs();
    if position_id.is_nil() {
        return Err(HandoffError::Position);
    }
    if symbol != inputs.binding.account.symbol() {
        return Err(HandoffError::Symbol);
    }
    if decided_month != inputs.basis.month {
        return Err(HandoffError::Month);
    }
    let amounts = calculation.amounts();
    let terms = &inputs.terms;
    let mut hash = String::with_capacity(64);
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for byte in terms.hash() {
        hash.push(char::from(HEX[usize::from(byte >> 4)]));
        hash.push(char::from(HEX[usize::from(byte & 15)]));
    }
    Ok(AdmissionRow {
        position_id,
        symbol: symbol.as_pair(),
        decided_month,
        taker_bps: terms.taker_bps(),
        gap_bps: terms.gap_bps(),
        notional_margin_bps: inputs.policy.margin_bps(),
        min_notional: inputs.rules.notional.min,
        lot_step: calculation.selected_lot_step(),
        fee_asset_policy: "base".into(),
        envelope_source: terms.source_provenance(),
        envelope_as_of: terms.evidence().as_of(),
        envelope_hash: hash,
        planned_qty: amounts.protection,
        requested_qty: amounts.gross_entry,
        reference_price: inputs.price.value,
        remainder_reserve: amounts.remainder,
        admission_charge: amounts.charge,
        initial_trigger: inputs.chart.plan.trigger.as_decimal(),
    })
}

#[cfg(test)]
#[path = "handoff_tests.rs"]
mod tests;
