//! Native history evidence and dormant prepared N5 selection. This path
//! cannot open entry or classify unresolved results as confirmed absence.

use std::{collections::BTreeMap, time::Duration};

use chrono::{DateTime, Utc};
use robson_store::native_n5::{Observation, ObservedRow, ReadError, MAX_ACTIVE};
use sha2::{Digest, Sha256};
use tokio::time::Instant;
use uuid::Uuid;

use super::{
    retained_n5::decode_retained, CostCandidate, CostIdentity, EvidenceIdentity,
    COST_VALIDATION_VERSION,
};
use crate::{
    clock_records::{RawClockCapture, TICK_MAX},
    venue::VenueId,
    venue_clock::{ClockDomain, ClockError, NativeClock, TickCapture},
};

pub const DAY_NS: u128 = 86_400_000_000_000;
pub const HANDOFF_NS: u128 = 60_000_000_000;

#[path = "native_n5_prepared.rs"]
mod prepared;
pub use prepared::{PreparedFeeOwner, PreparedFeeSelection};

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum HistoryError {
    #[error("persisted N5 history is invalid")]
    Invalid,
    #[error("native N5 history backend failed")]
    Backend,
    #[error("native N5 read did not complete")]
    Unpublished,
    #[error("native N5 history read expired")]
    Expired,
    #[error("native N5 clock domain is not comparable")]
    PrerequisiteRefusal,
}

impl From<ReadError> for HistoryError {
    fn from(error: ReadError) -> Self {
        match error {
            ReadError::Invalid => Self::Invalid,
            ReadError::Backend => Self::Backend,
            ReadError::Unpublished => Self::Unpublished,
            ReadError::Expired => Self::Expired,
        }
    }
}

#[derive(Clone, Debug)]
pub struct FrequencyKey {
    pub symbol: String,
    pub initialization_id: Uuid,
    pub recovery_generation: i64,
    pub registration_id: Uuid,
}

impl FrequencyKey {
    pub fn is_valid(&self) -> bool {
        !self.symbol.is_empty()
            && self.symbol.len() <= 64
            && !self.initialization_id.is_nil()
            && !self.registration_id.is_nil()
            && self.recovery_generation >= 0
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClaimIdentity {
    pub claim_id: Uuid,
    pub owner: Uuid,
    pub digest: [u8; 32],
    pub captured: TickCapture,
    pub audit_at: DateTime<Utc>,
}

#[derive(Clone, Copy, Debug)]
pub enum PriorEligibility {
    FirstUse,
    SameDomain,
    ContinuousObservation,
}

impl PriorEligibility {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::FirstUse => "first_use",
            Self::SameDomain => "same_domain",
            Self::ContinuousObservation => "continuous_observation",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum ClaimResult {
    Success,
    NoSend,
    TransportFailure,
    VenueRejected,
    InvalidPayload,
    PolicyMismatch,
    Unknown,
}

impl ClaimResult {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::NoSend => "no_send",
            Self::TransportFailure => "transport_failure",
            Self::VenueRejected => "venue_rejected",
            Self::InvalidPayload => "invalid_payload",
            Self::PolicyMismatch => "policy_mismatch",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Clone, Debug)]
pub struct PriorClaim {
    pub revision: i64,
    pub identity: ClaimIdentity,
    pub eligibility: PriorEligibility,
    pub handoff: Option<TickCapture>,
    pub result: Option<ClaimResult>,
}

pub fn valid_audit_at(audit_at: DateTime<Utc>) -> bool {
    let subsec = audit_at.timestamp_subsec_nanos();
    subsec < 1_000_000_000 && subsec % 1_000 == 0
}

pub fn valid_identity(identity: &ClaimIdentity) -> bool {
    !identity.claim_id.is_nil()
        && !identity.owner.is_nil()
        && identity.captured.nanoseconds() <= TICK_MAX as u128
        && valid_audit_at(identity.audit_at)
}

const NANOS_PER_SECOND: u128 = 1_000_000_000;
const CLAIM_CLOCK: [&str; 5] = [
    "claim_kind",
    "claim_boot",
    "claim_offset_s",
    "claim_offset_ns",
    "claim_tick",
];
const HANDOFF_CLOCK: [&str; 5] = [
    "handoff_kind",
    "handoff_boot",
    "handoff_offset_s",
    "handoff_offset_ns",
    "handoff_tick",
];
const SNAPSHOT_CLOCK: [&str; 5] = [
    "snapshot_kind",
    "snapshot_boot",
    "snapshot_offset_s",
    "snapshot_offset_ns",
    "snapshot_tick",
];

#[derive(Debug)]
struct Registry {
    root_id: i16,
    schema_version: i16,
    manifest_version: i16,
    manifest_revision: i64,
    initialized: bool,
    registered_count: i32,
    canonical_digest: Vec<u8>,
    initialization_id: Uuid,
    recovery_generation: i64,
    recovery_state: String,
}

impl Registry {
    fn from_observed(row: &ObservedRow) -> Result<Self, HistoryError> {
        Ok(Self {
            root_id: row.try_get("root_id")?,
            schema_version: row.try_get("schema_version")?,
            manifest_version: row.try_get("manifest_version")?,
            manifest_revision: row.try_get("manifest_revision")?,
            initialized: row.try_get("initialized")?,
            registered_count: row.try_get("registered_count")?,
            canonical_digest: row.try_get("canonical_digest")?,
            initialization_id: row.try_get("initialization_id")?,
            recovery_generation: row.try_get("recovery_generation")?,
            recovery_state: row.try_get("recovery_state")?,
        })
    }
}

#[derive(Debug)]
pub struct Registration {
    pub venue_id: String,
    pub symbol: String,
    pub descriptor: String,
    pub key_registry: i16,
    pub registration_id: Uuid,
    pub registered_revision: i64,
    pub registered_at: DateTime<Utc>,
    pub provenance: String,
}

impl Registration {
    fn from_observed(row: &ObservedRow) -> Result<Self, HistoryError> {
        Ok(Self {
            venue_id: row.try_get("venue_id")?,
            symbol: row.try_get("symbol")?,
            descriptor: row.try_get("descriptor")?,
            key_registry: row.try_get("key_registry")?,
            registration_id: row.try_get("registration_id")?,
            registered_revision: row.try_get("registered_revision")?,
            registered_at: row.try_get("registered_at")?,
            provenance: row.try_get("provenance")?,
        })
    }
    fn key(&self) -> (&str, &str, &str) {
        (&self.venue_id, &self.symbol, &self.descriptor)
    }
}

/// Fixture-specific encoding. This does not define a production manifest.
/// Hash sorted full registrations with a count, big-endian fixed-width
/// values, and binary length-framed UTF-8 strings.
pub fn fixture_manifest(registrations: &mut [Registration]) -> [u8; 32] {
    registrations.sort_unstable_by(|left, right| left.key().cmp(&right.key()));
    let mut hash = Sha256::new();
    hash.update(b"robson:test:n5-complete-manifest:v1\0");
    hash.update((registrations.len() as u32).to_be_bytes());
    for entry in registrations {
        for field in [
            &entry.venue_id,
            &entry.symbol,
            &entry.descriptor,
            &entry.provenance,
        ] {
            hash.update((field.len() as u32).to_be_bytes());
            hash.update(field.as_bytes());
        }
        hash.update(entry.key_registry.to_be_bytes());
        hash.update(entry.registration_id.as_bytes());
        hash.update(entry.registered_revision.to_be_bytes());
        hash.update(entry.registered_at.timestamp().to_be_bytes());
        hash.update(entry.registered_at.timestamp_subsec_nanos().to_be_bytes());
    }
    hash.finalize().into()
}

#[derive(Debug)]
pub struct LoadedKey {
    pub key: FrequencyKey,
    pub prior: Option<PriorClaim>,
    pub receipt: ReceiptState,
}

#[derive(Debug)]
pub enum ReceiptState {
    /// Selection and absence are both unproved, including for revision zero.
    Unproven,
    Stale,
    Current(ColdReceipt),
}

/// Descriptive verification of a persisted receipt, with no authority to size,
/// publish, or reuse the predecessor's handoff.
#[derive(Debug)]
pub struct ColdReceipt {
    pub original: RawClockCapture,
    candidate: CostCandidate,
    observed: TickCapture,
    pub payload_digest: [u8; 32],
    pub age: Duration,
}

/// Verified data from one completed bounded PostgreSQL read of the known
/// fixture manifest version. No caller can manufacture this from rows or an
/// already decanted receipt. Operational registration remains outside.
///
/// ```compile_fail
/// use robson_exec::{balance_costs::native_n5::VerifiedHistory, venue_clock::NativeClock};
/// let domain = NativeClock::initialize().unwrap().initial_capture().domain();
/// let _forged = VerifiedHistory { entries: Vec::new(), expected: Vec::new(), domain };
/// ```
pub struct VerifiedHistory {
    entries: Vec<LoadedKey>,
    expected: Vec<CostIdentity>,
    domain: ClockDomain,
}

impl VerifiedHistory {
    pub fn entries(&self) -> &[LoadedKey] {
        &self.entries
    }
}

impl ColdReceipt {
    pub fn evidence(&self) -> &EvidenceIdentity {
        self.candidate.evidence()
    }
}

/// Validate the manifest, active membership, and exact prior/current receipt
/// data from the same native snapshot. The passed clock must be the fixed
/// native domain for this owner; no durations are taken from a caller.
pub fn verify_history(
    observation: Observation,
    expected: &[CostIdentity],
    clock: &NativeClock,
    deadline: Instant,
) -> Result<VerifiedHistory, HistoryError> {
    let now = clock.capture().map_err(|_| HistoryError::PrerequisiteRefusal)?;
    let entries = verify_at(&observation, expected, now, deadline)?;
    Ok(VerifiedHistory {
        entries,
        expected: expected.to_vec(),
        domain: now.domain(),
    })
}

/// Diagnostic only, for boundary controls on the same physically loaded
/// rows. The test cannot turn a synthetic clock into a selection owner.
#[cfg(feature = "crash-points")]
pub fn fixture_describe_at(
    observation: &Observation,
    expected: &[CostIdentity],
    now: TickCapture,
    deadline: Instant,
) -> Result<Vec<LoadedKey>, HistoryError> {
    verify_at(observation, expected, now, deadline)
}

fn verify_at(
    observation: &Observation,
    expected: &[CostIdentity],
    now: TickCapture,
    deadline: Instant,
) -> Result<Vec<LoadedKey>, HistoryError> {
    if Instant::now() >= deadline {
        return Err(HistoryError::Expired);
    }
    if expected.is_empty()
        || expected.len() > MAX_ACTIVE
        || expected.iter().any(|entry| entry.venue().as_str() != VenueId::BINANCE_SPOT)
    {
        return Err(HistoryError::Invalid);
    }
    let mut requested: BTreeMap<_, _> =
        expected.iter().map(|entry| (entry.symbol().as_pair(), entry)).collect();
    if requested.len() != expected.len() {
        return Err(HistoryError::Invalid);
    }
    let rows = observation.rows();
    let root = Registry::from_observed(&rows[0])?;
    if root.root_id != 1
        || root.schema_version != 38
        || root.manifest_version != 1
        || root.manifest_revision <= 0
        || !root.initialized
        || root.registered_count as usize != rows.len()
        || root.initialization_id.is_nil()
        || root.recovery_generation < 0
        || root.recovery_state != "normal"
    {
        return Err(HistoryError::Invalid);
    }
    let mut registrations = Vec::with_capacity(rows.len());
    let mut keys = Vec::with_capacity(expected.len());
    for row in rows {
        let entry = Registration::from_observed(row)?;
        let revision: i64 = row.try_get("state_revision")?;
        if entry.key_registry != root.root_id
            || entry.registration_id.is_nil()
            || !(1..=root.manifest_revision).contains(&entry.registered_revision)
            || !valid_audit_at(entry.registered_at)
            || revision < 0
        {
            return Err(HistoryError::Invalid);
        }
        if row.try_get::<bool>("active")? {
            let identity = requested.remove(&entry.symbol).ok_or(HistoryError::Invalid)?;
            if entry.venue_id != identity.venue().as_str() || entry.descriptor != "commission" {
                return Err(HistoryError::Invalid);
            }
            let (prior, receipt) = state(row, revision, identity, now)?;
            keys.push(LoadedKey {
                key: FrequencyKey {
                    symbol: entry.symbol.clone(),
                    initialization_id: root.initialization_id,
                    recovery_generation: root.recovery_generation,
                    registration_id: entry.registration_id,
                },
                prior,
                receipt,
            });
        }
        registrations.push(entry);
    }
    let digest = fixture_manifest(&mut registrations);
    if !requested.is_empty()
        || digest.as_slice() != root.canonical_digest
        || registrations.windows(2).any(|pair| pair[0].key() == pair[1].key())
    {
        return Err(HistoryError::Invalid);
    }
    if Instant::now() >= deadline {
        return Err(HistoryError::Expired);
    }
    Ok(keys)
}

fn tick(domain: ClockDomain, value: u128) -> Result<TickCapture, HistoryError> {
    let seconds = i64::try_from(value / NANOS_PER_SECOND).map_err(|_| HistoryError::Invalid)?;
    TickCapture::new(domain, seconds, (value % NANOS_PER_SECOND) as i64)
        .map_err(|_| HistoryError::Invalid)
}

fn read_clock(
    row: &ObservedRow,
    [kind, boot, seconds, nanos, count]: [&str; 5],
) -> Result<TickCapture, HistoryError> {
    if row.try_get::<&str>(kind)? != "boottime" {
        return Err(HistoryError::Invalid);
    }
    let nanos = u32::try_from(row.try_get::<i32>(nanos)?).map_err(|_| HistoryError::Invalid)?;
    let domain = ClockDomain::new(row.try_get(boot)?, row.try_get(seconds)?, nanos)
        .map_err(|_| HistoryError::Invalid)?;
    let count = row.try_get::<&str>(count)?.parse().map_err(|_| HistoryError::Invalid)?;
    tick(domain, count)
}

fn state(
    row: &ObservedRow,
    revision: i64,
    account: &CostIdentity,
    now: TickCapture,
) -> Result<(Option<PriorClaim>, ReceiptState), HistoryError> {
    if row.try_get::<bool>("invalid_active_projection")? {
        return Err(HistoryError::Invalid);
    }
    let claim_fields = row.try_get::<i32>("claim_fields")?;
    let handoff_fields = row.try_get::<i32>("handoff_fields")?;
    let snapshot_fields = row.try_get::<i32>("snapshot_fields")?;
    let result = match row.try_get::<Option<&str>>("result_class")? {
        None => None,
        Some("success") => Some(ClaimResult::Success),
        Some("no_send") => Some(ClaimResult::NoSend),
        Some("transport_failure") => Some(ClaimResult::TransportFailure),
        Some("venue_rejected") => Some(ClaimResult::VenueRejected),
        Some("invalid_payload") => Some(ClaimResult::InvalidPayload),
        Some("policy_mismatch") => Some(ClaimResult::PolicyMismatch),
        Some("unknown") => Some(ClaimResult::Unknown),
        _ => return Err(HistoryError::Invalid),
    };
    if revision == 0 {
        return if (claim_fields, handoff_fields, snapshot_fields) == (0, 0, 0) && result.is_none() {
            Ok((None, ReceiptState::Unproven))
        } else {
            Err(HistoryError::Invalid)
        };
    }
    if claim_fields != 7 || !matches!(handoff_fields, 0 | 2) || !matches!(snapshot_fields, 0 | 12) {
        return Err(HistoryError::Invalid);
    }
    let eligibility = match row.try_get::<&str>("eligibility_kind")? {
        "first_use" => PriorEligibility::FirstUse,
        "same_domain" => PriorEligibility::SameDomain,
        "continuous_observation" => PriorEligibility::ContinuousObservation,
        _ => return Err(HistoryError::Invalid),
    };
    let identity = ClaimIdentity {
        claim_id: row.try_get("claim_id")?,
        owner: row.try_get("owner_process")?,
        digest: row
            .try_get::<&[u8]>("content_digest")?
            .try_into()
            .map_err(|_| HistoryError::Invalid)?,
        captured: read_clock(row, CLAIM_CLOCK)?,
        audit_at: row.try_get("claim_audit_at")?,
    };
    if !valid_identity(&identity) {
        return Err(HistoryError::Invalid);
    }
    let handoff = if handoff_fields == 2 {
        let at = read_clock(row, HANDOFF_CLOCK)?;
        let elapsed = at.elapsed_since(&identity.captured).map_err(|_| HistoryError::Invalid)?;
        if elapsed.as_nanos() > HANDOFF_NS || matches!(result, Some(ClaimResult::NoSend)) {
            return Err(HistoryError::Invalid);
        }
        Some(at)
    } else {
        None
    };
    let prior = PriorClaim {
        revision,
        identity,
        eligibility,
        handoff,
        result,
    };
    let snapshot_revision = row.try_get::<Option<i64>>("snapshot_revision")?;
    let receipt = match (result, snapshot_revision) {
        (Some(ClaimResult::Success), Some(value)) if value == revision => {
            current_receipt(row, account, &prior, now)?
        },
        (Some(ClaimResult::Success), _) => return Err(HistoryError::Invalid),
        (_, Some(value)) if !(1..revision).contains(&value) => return Err(HistoryError::Invalid),
        _ => ReceiptState::Unproven,
    };
    Ok((Some(prior), receipt))
}

fn current_receipt(
    row: &ObservedRow,
    account: &CostIdentity,
    prior: &PriorClaim,
    now: TickCapture,
) -> Result<ReceiptState, HistoryError> {
    let uid = row
        .try_get::<&str>("snapshot_uid")?
        .parse::<u64>()
        .map_err(|_| HistoryError::Invalid)?;
    let version = row.try_get::<i16>("snapshot_validation_version")?;
    if row.try_get::<Uuid>("snapshot_claim_id")? != prior.identity.claim_id
        || row.try_get::<Uuid>("snapshot_owner")? != prior.identity.owner
        || row.try_get::<Uuid>("snapshot_tenant")? != account.tenant()
        || row.try_get::<&str>("snapshot_scope")? != account.scope()
        || uid != account.uid()
        || u16::try_from(version).ok() != Some(COST_VALIDATION_VERSION)
    {
        return Err(HistoryError::Invalid);
    }
    let handoff = prior.handoff.ok_or(HistoryError::Invalid)?;
    let payload = row.try_get::<&[u8]>("snapshot_payload")?;
    let decoded = decode_retained(payload, account, handoff).ok_or(HistoryError::Invalid)?;
    if decoded.original.epoch != prior.identity.owner
        || decoded.observed != read_clock(row, SNAPSHOT_CLOCK)?
        || decoded.candidate.evidence().as_of() != row.try_get::<DateTime<Utc>>("snapshot_as_of")?
        || decoded.candidate.evidence().hash().as_slice()
            != row.try_get::<&[u8]>("snapshot_hash")?
    {
        return Err(HistoryError::Invalid);
    }
    let Some(age) = receipt_age(now, &decoded.original, decoded.observed)? else {
        return Ok(ReceiptState::Stale);
    };
    Ok(ReceiptState::Current(ColdReceipt {
        original: decoded.original,
        candidate: decoded.candidate,
        observed: decoded.observed,
        payload_digest: Sha256::digest(payload).into(),
        age,
    }))
}

fn receipt_age(
    now: TickCapture,
    original: &RawClockCapture,
    first: TickCapture,
) -> Result<Option<Duration>, HistoryError> {
    let age =
        receipt_elapsed(now, original, first).map_err(|_| HistoryError::PrerequisiteRefusal)?;
    if age.as_nanos() >= DAY_NS {
        return Ok(None);
    }
    Ok(Some(age))
}

fn receipt_elapsed(
    now: TickCapture,
    original: &RawClockCapture,
    first: TickCapture,
) -> Result<Duration, ClockError> {
    now.elapsed_since(&original.end_tick()?)?;
    now.elapsed_since(&first)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cold_receipt_refuses_at_exactly_day_and_before_original_bracket_end() {
        let domain = ClockDomain::new(Uuid::from_u128(1), -2, 345).unwrap();
        let raw = RawClockCapture {
            epoch: Uuid::from_u128(2),
            wall_seconds: 1_795_120_000,
            wall_nanoseconds: 123_456_789,
            domain,
            before_ns: 100,
            after_ns: 120,
        };
        let first = tick(domain, 100).unwrap();
        let calculate = |at| receipt_age(tick(domain, at).unwrap(), &raw, first);
        assert!(matches!(calculate(119), Err(HistoryError::PrerequisiteRefusal)));
        assert_eq!(calculate(120).unwrap(), Some(Duration::from_nanos(20)));
        assert_eq!(calculate(100 + DAY_NS - 1).unwrap().unwrap().as_nanos(), DAY_NS - 1);
        assert!(calculate(100 + DAY_NS).unwrap().is_none(), "exactly 24h must be stale");
        let foreign = ClockDomain::new(domain.boot(), -2, 346).unwrap();
        assert!(matches!(
            receipt_age(tick(foreign, 120).unwrap(), &raw, first),
            Err(HistoryError::PrerequisiteRefusal)
        ));
    }
}
