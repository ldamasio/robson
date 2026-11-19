//! Read and check dormant N5 history from one bounded registry snapshot.
use std::{collections::BTreeMap, time::Duration};

use chrono::{DateTime, Utc};
use robson_exec::{
    balance_costs::{CostIdentity, EvidenceIdentity, COST_VALIDATION_VERSION},
    clock_records::RawClockCapture,
    venue::VenueId,
    venue_clock::{ClockDomain, NativeClock, TickCapture},
};
use robson_store::native_n5::{self, Observation, ObservedRow, ReadError, MAX_ACTIVE};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use tokio::time::Instant;
use uuid::Uuid;

use super::{
    response::decode_retained, valid_audit_at, valid_identity, BudgetError, ClaimIdentity,
    ClaimResult, FrequencyKey, PriorClaim, PriorEligibility, Trace, DAY_NS, HANDOFF_NS,
    STORE_LIMIT,
};

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
    fn from_observed(row: &ObservedRow) -> Result<Self, BudgetError> {
        Ok(Self {
            root_id: read(row, "root_id")?,
            schema_version: read(row, "schema_version")?,
            manifest_version: read(row, "manifest_version")?,
            manifest_revision: read(row, "manifest_revision")?,
            initialized: read(row, "initialized")?,
            registered_count: read(row, "registered_count")?,
            canonical_digest: read(row, "canonical_digest")?,
            initialization_id: read(row, "initialization_id")?,
            recovery_generation: read(row, "recovery_generation")?,
            recovery_state: read(row, "recovery_state")?,
        })
    }
}

#[derive(Debug)]
pub(super) struct Registration {
    pub(super) venue_id: String,
    pub(super) symbol: String,
    pub(super) descriptor: String,
    pub(super) key_registry: i16,
    pub(super) registration_id: Uuid,
    pub(super) registered_revision: i64,
    pub(super) registered_at: DateTime<Utc>,
    pub(super) provenance: String,
}

impl Registration {
    fn from_observed(row: &ObservedRow) -> Result<Self, BudgetError> {
        Ok(Self {
            venue_id: read(row, "venue_id")?,
            symbol: read(row, "symbol")?,
            descriptor: read(row, "descriptor")?,
            key_registry: read(row, "key_registry")?,
            registration_id: read(row, "registration_id")?,
            registered_revision: read(row, "registered_revision")?,
            registered_at: read(row, "registered_at")?,
            provenance: read(row, "provenance")?,
        })
    }
    fn key(&self) -> (&str, &str, &str) {
        (&self.venue_id, &self.symbol, &self.descriptor)
    }
}

/// Fixture-specific encoding. This does not define a production manifest.
/// Hash sorted full registrations with a count, big-endian fixed-width
/// values, and binary length-framed UTF-8 strings.
pub(super) fn fixture_manifest(registrations: &mut [Registration]) -> [u8; 32] {
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
pub(super) struct LoadedKey {
    pub(super) key: FrequencyKey,
    pub(super) prior: Option<PriorClaim>,
    pub(super) receipt: ReceiptState,
}

#[derive(Debug)]
pub(super) enum ReceiptState {
    /// Selection and absence are both unproved, including for revision zero.
    Unproven,
    Stale,
    Current(ColdReceipt),
}

/// Descriptive verification of a persisted receipt, with no authority to size,
/// publish, or reuse the predecessor's handoff.
#[derive(Debug)]
pub(super) struct ColdReceipt {
    pub(super) original: RawClockCapture,
    pub(super) evidence: EvidenceIdentity,
    pub(super) payload_digest: [u8; 32],
    pub(super) age: Duration,
}

pub(super) async fn load(
    pool: &PgPool,
    expected: &[CostIdentity],
    clock: &NativeClock,
    deadline: Instant,
    trace: &Trace,
) -> Result<Vec<LoadedKey>, BudgetError> {
    if expected.is_empty()
        || expected.len() > MAX_ACTIVE
        || expected.iter().any(|entry| entry.venue().as_str() != VenueId::BINANCE_SPOT)
    {
        return Err(BudgetError::Invalid);
    }
    let mut requested: BTreeMap<_, _> =
        expected.iter().map(|entry| (entry.symbol().as_pair(), entry)).collect();
    if requested.len() != expected.len() {
        return Err(BudgetError::Invalid);
    }
    let deadline = deadline.min(Instant::now() + STORE_LIMIT);
    let symbols: Vec<_> = requested.keys().cloned().collect();
    let observation = read_rows(pool, &symbols, deadline, trace).await?;
    let rows = observation.rows();
    let now = clock.capture().map_err(|_| BudgetError::PrerequisiteRefusal)?;
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
        return Err(BudgetError::Invalid);
    }
    let mut registrations = Vec::with_capacity(rows.len());
    let mut keys = Vec::with_capacity(expected.len());
    for row in rows {
        let entry = Registration::from_observed(row)?;
        let revision: i64 = read(row, "state_revision")?;
        if entry.key_registry != root.root_id
            || entry.registration_id.is_nil()
            || !(1..=root.manifest_revision).contains(&entry.registered_revision)
            || !valid_audit_at(entry.registered_at)
            || revision < 0
        {
            return Err(BudgetError::Invalid);
        }
        if read::<bool>(row, "active")? {
            let identity = requested.remove(&entry.symbol).ok_or(BudgetError::Invalid)?;
            if entry.venue_id != identity.venue().as_str() || entry.descriptor != "commission" {
                return Err(BudgetError::Invalid);
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
        return Err(BudgetError::Invalid);
    }
    if Instant::now() >= deadline {
        return Err(BudgetError::Expired);
    }
    Ok(keys)
}

fn read<'r, T>(row: &'r ObservedRow, column: &str) -> Result<T, BudgetError>
where
    T: sqlx::Decode<'r, sqlx::Postgres> + sqlx::Type<sqlx::Postgres>,
{
    row.try_get(column).map_err(map_read_error)
}

fn tick(domain: ClockDomain, value: u128) -> Result<TickCapture, BudgetError> {
    let seconds = i64::try_from(value / NANOS_PER_SECOND).map_err(|_| BudgetError::Invalid)?;
    TickCapture::new(domain, seconds, (value % NANOS_PER_SECOND) as i64)
        .map_err(|_| BudgetError::Invalid)
}

fn read_clock(
    row: &ObservedRow,
    [kind, boot, seconds, nanos, count]: [&str; 5],
) -> Result<TickCapture, BudgetError> {
    if read::<&str>(row, kind)? != "boottime" {
        return Err(BudgetError::Invalid);
    }
    let nanos = u32::try_from(read::<i32>(row, nanos)?).map_err(|_| BudgetError::Invalid)?;
    let domain = ClockDomain::new(read(row, boot)?, read(row, seconds)?, nanos)
        .map_err(|_| BudgetError::Invalid)?;
    let count = read::<&str>(row, count)?.parse().map_err(|_| BudgetError::Invalid)?;
    tick(domain, count)
}

fn state(
    row: &ObservedRow,
    revision: i64,
    account: &CostIdentity,
    now: TickCapture,
) -> Result<(Option<PriorClaim>, ReceiptState), BudgetError> {
    if read::<bool>(row, "invalid_active_projection")? {
        return Err(BudgetError::Invalid);
    }
    let claim_fields = read::<i32>(row, "claim_fields")?;
    let handoff_fields = read::<i32>(row, "handoff_fields")?;
    let snapshot_fields = read::<i32>(row, "snapshot_fields")?;
    let result = match read::<Option<&str>>(row, "result_class")? {
        None => None,
        Some("success") => Some(ClaimResult::Success),
        Some("no_send") => Some(ClaimResult::NoSend),
        Some("transport_failure") => Some(ClaimResult::TransportFailure),
        Some("venue_rejected") => Some(ClaimResult::VenueRejected),
        Some("invalid_payload") => Some(ClaimResult::InvalidPayload),
        Some("policy_mismatch") => Some(ClaimResult::PolicyMismatch),
        Some("unknown") => Some(ClaimResult::Unknown),
        _ => return Err(BudgetError::Invalid),
    };
    if revision == 0 {
        return if (claim_fields, handoff_fields, snapshot_fields) == (0, 0, 0) && result.is_none() {
            Ok((None, ReceiptState::Unproven))
        } else {
            Err(BudgetError::Invalid)
        };
    }
    if claim_fields != 7 || !matches!(handoff_fields, 0 | 2) || !matches!(snapshot_fields, 0 | 12) {
        return Err(BudgetError::Invalid);
    }
    let eligibility = match read::<&str>(row, "eligibility_kind")? {
        "first_use" => PriorEligibility::FirstUse,
        "same_domain" => PriorEligibility::SameDomain,
        "continuous_observation" => PriorEligibility::ContinuousObservation,
        _ => return Err(BudgetError::Invalid),
    };
    let identity = ClaimIdentity {
        claim_id: read(row, "claim_id")?,
        owner: read(row, "owner_process")?,
        digest: read::<&[u8]>(row, "content_digest")?
            .try_into()
            .map_err(|_| BudgetError::Invalid)?,
        captured: read_clock(row, CLAIM_CLOCK)?,
        audit_at: read(row, "claim_audit_at")?,
    };
    if !valid_identity(&identity) {
        return Err(BudgetError::Invalid);
    }
    let handoff = if handoff_fields == 2 {
        let at = read_clock(row, HANDOFF_CLOCK)?;
        let elapsed = at.elapsed_since(&identity.captured).map_err(|_| BudgetError::Invalid)?;
        if elapsed.as_nanos() > HANDOFF_NS || matches!(result, Some(ClaimResult::NoSend)) {
            return Err(BudgetError::Invalid);
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
    let snapshot_revision = read::<Option<i64>>(row, "snapshot_revision")?;
    let receipt = match (result, snapshot_revision) {
        (Some(ClaimResult::Success), Some(value)) if value == revision => {
            current_receipt(row, account, &prior, now)?
        },
        (Some(ClaimResult::Success), _) => return Err(BudgetError::Invalid),
        (_, Some(value)) if !(1..revision).contains(&value) => return Err(BudgetError::Invalid),
        _ => ReceiptState::Unproven,
    };
    Ok((Some(prior), receipt))
}

fn current_receipt(
    row: &ObservedRow,
    account: &CostIdentity,
    prior: &PriorClaim,
    now: TickCapture,
) -> Result<ReceiptState, BudgetError> {
    let uid = read::<&str>(row, "snapshot_uid")?
        .parse::<u64>()
        .map_err(|_| BudgetError::Invalid)?;
    let version = read::<i16>(row, "snapshot_validation_version")?;
    if read::<Uuid>(row, "snapshot_claim_id")? != prior.identity.claim_id
        || read::<Uuid>(row, "snapshot_owner")? != prior.identity.owner
        || read::<Uuid>(row, "snapshot_tenant")? != account.tenant()
        || read::<&str>(row, "snapshot_scope")? != account.scope()
        || uid != account.uid()
        || u16::try_from(version).ok() != Some(COST_VALIDATION_VERSION)
    {
        return Err(BudgetError::Invalid);
    }
    let handoff = prior.handoff.ok_or(BudgetError::Invalid)?;
    let payload = read::<&[u8]>(row, "snapshot_payload")?;
    let decoded = decode_retained(payload, account, handoff).ok_or(BudgetError::Invalid)?;
    if decoded.original.epoch != prior.identity.owner
        || decoded.observed != read_clock(row, SNAPSHOT_CLOCK)?
        || decoded.evidence.as_of() != read::<DateTime<Utc>>(row, "snapshot_as_of")?
        || decoded.evidence.hash().as_slice() != read::<&[u8]>(row, "snapshot_hash")?
    {
        return Err(BudgetError::Invalid);
    }
    let Some(age) = receipt_age(now, &decoded.original, decoded.observed)? else {
        return Ok(ReceiptState::Stale);
    };
    Ok(ReceiptState::Current(ColdReceipt {
        original: decoded.original,
        evidence: decoded.evidence,
        payload_digest: Sha256::digest(payload).into(),
        age,
    }))
}

fn receipt_age(
    now: TickCapture,
    original: &RawClockCapture,
    first: TickCapture,
) -> Result<Option<Duration>, BudgetError> {
    now.elapsed_since(&original.end_tick().map_err(|_| BudgetError::Invalid)?)
        .map_err(|_| BudgetError::PrerequisiteRefusal)?;
    let age = now.elapsed_since(&first).map_err(|_| BudgetError::PrerequisiteRefusal)?;
    if age.as_nanos() >= DAY_NS {
        return Ok(None);
    }
    Ok(Some(age))
}

fn map_read_error(error: ReadError) -> BudgetError {
    match error {
        ReadError::Invalid => BudgetError::Invalid,
        ReadError::Backend => BudgetError::Backend,
        ReadError::Unpublished => BudgetError::Unpublished,
        ReadError::Expired => BudgetError::Expired,
    }
}

pub(super) async fn read_rows(
    pool: &PgPool,
    symbols: &[String],
    deadline: Instant,
    trace: &Trace,
) -> Result<Observation, BudgetError> {
    native_n5::read(pool, symbols, deadline, &trace.0).await.map_err(map_read_error)
}

#[cfg(all(test, feature = "crash-points"))]
#[path = "cold_load_tests.rs"]
mod tests;
#[cfg(all(test, feature = "crash-points"))]
pub(super) use tests::verify_real_receipt_controls;
