//! Gate the original bounded commission envelope through the venue parser.

use chrono::{DateTime, Utc};
use robson_connectors::binance::spot::SpotCommission;
pub(super) use robson_exec::balance_costs::retained_n5::MAX_RESPONSE_BYTES;
use robson_exec::{
    balance_costs::{
        retained_n5::{encode_retained, RetentionError},
        CostError, CostIdentity, EvidenceIdentity,
    },
    clock_records::RawClockCapture,
    venue_clock::TickCapture,
};

use super::{result::PreparedSnapshot, ClaimResult};

pub(super) fn prepare_response(
    body: &[u8],
    original: RawClockCapture,
    expected: &CostIdentity,
    handoff: TickCapture,
) -> Result<PreparedSnapshot, ClaimResult> {
    let encoded =
        encode_retained(body, original, expected, handoff).map_err(|error| match error {
            RetentionError::InvalidPayload => ClaimResult::InvalidPayload,
            RetentionError::PolicyMismatch => ClaimResult::PolicyMismatch,
            RetentionError::Unknown => ClaimResult::Unknown,
        })?;
    // Keep the original venue parser on the writer side, and require its
    // evidence to agree with the pure retained reader before publishing.
    let evidence = encoded.candidate.evidence();
    if &connector_evidence(body, expected, evidence.as_of())? != evidence {
        return Err(ClaimResult::InvalidPayload);
    }
    Ok(PreparedSnapshot {
        evidence: evidence.clone(),
        observed: encoded.observed,
        payload: encoded.payload,
    })
}

fn connector_evidence(
    body: &[u8],
    expected: &CostIdentity,
    as_of: DateTime<Utc>,
) -> Result<EvidenceIdentity, ClaimResult> {
    let parsed: SpotCommission =
        serde_json::from_slice(body).map_err(|_| ClaimResult::InvalidPayload)?;
    parsed
        .cost_candidate(expected.clone(), as_of)
        .map(|candidate| candidate.evidence().clone())
        .map_err(|error| match error {
            CostError::FeePolicyMismatch => ClaimResult::PolicyMismatch,
            _ => ClaimResult::InvalidPayload,
        })
}

#[cfg(test)]
#[path = "response_tests.rs"]
mod tests;
