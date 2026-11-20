//! Immutable balance-account fee evidence and selection. No I/O or send grant.

use chrono::{DateTime, Utc};
use num_bigint::BigUint;
use robson_domain::Symbol;
use rust_decimal::Decimal;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::{age::CostAgeAuthority, exact::ExactDecimal, AgeError};
use crate::venue::{ExecutionCostEnvelope, FeeAssetPolicy, VenueId};

pub const COST_VALIDATION_VERSION: u16 = 1;
const API_SOURCE: &str = "binance:spot /api/v3/account/commission";
const OVERRIDE_SOURCE: &str = "ROBSON_COSTS_OVERRIDE:v1";

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CostError {
    #[error("invalid bounded cost identity or provenance")]
    Identity,
    #[error("invalid cost configuration")]
    Config,
    #[error("incompatible commission asset policy")]
    FeePolicyMismatch,
    #[error("negative or out-of-policy commission rate")]
    Rate,
    #[error("cost arithmetic is not exactly representable")]
    Unrepresentable,
    #[error("cost evidence is missing")]
    Missing,
    #[error("cost evidence is stale")]
    Stale,
    #[error(transparent)]
    Age(#[from] AgeError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CostIdentity {
    tenant: Uuid,
    venue: VenueId,
    scope: String,
    uid: u64,
    symbol: Symbol,
}

impl CostIdentity {
    pub fn new(
        tenant: Uuid,
        venue: VenueId,
        scope: String,
        uid: u64,
        symbol: Symbol,
    ) -> Result<Self, CostError> {
        if tenant.is_nil()
            || !bounded(venue.as_str(), 64)
            || !bounded(&scope, 128)
            || !bounded(symbol.base(), 32)
            || !bounded(symbol.quote(), 32)
        {
            return Err(CostError::Identity);
        }
        Ok(Self { tenant, venue, scope, uid, symbol })
    }
    pub fn tenant(&self) -> Uuid {
        self.tenant
    }
    pub fn venue(&self) -> &VenueId {
        &self.venue
    }
    pub fn scope(&self) -> &str {
        &self.scope
    }
    pub fn uid(&self) -> u64 {
        self.uid
    }
    pub fn symbol(&self) -> &Symbol {
        &self.symbol
    }
}

pub(super) fn bounded(value: &str, maximum: usize) -> bool {
    !value.trim().is_empty() && value.len() <= maximum && !value.chars().any(char::is_control)
}

/// Exact original strings survive the connector boundary. Array order is
/// maker, taker, buyer, seller; three kinds are always present, never
/// discounted.
#[derive(Debug, Clone)]
pub struct CommissionComponents {
    pub standard: [ExactDecimal; 4],
    pub tax: [ExactDecimal; 4],
    pub special: [ExactDecimal; 4],
    pub enabled_for_account: bool,
    pub enabled_for_symbol: bool,
    pub discount: ExactDecimal,
    pub discount_asset: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CostSource {
    Venue,
    DatedOverride,
}

/// Identity supplied to the sealed authority, including original timestamp,
/// validation version and hash of the complete validated source evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvidenceIdentity {
    account: CostIdentity,
    as_of: DateTime<Utc>,
    source: CostSource,
    hash: [u8; 32],
}
impl EvidenceIdentity {
    pub fn account(&self) -> &CostIdentity {
        &self.account
    }
    pub fn as_of(&self) -> DateTime<Utc> {
        self.as_of
    }
    pub fn source(&self) -> CostSource {
        self.source
    }
    pub fn hash(&self) -> &[u8; 32] {
        &self.hash
    }
    pub fn validation_version(&self) -> u16 {
        COST_VALIDATION_VERSION
    }
}

/// Validated values, not a qualified receipt. Selection still needs the sealed
/// authority, and selected terms still confer no permission to journal or send.
#[derive(Debug, Clone)]
pub struct CostCandidate {
    evidence: EvidenceIdentity,
    maker_bps: Decimal,
    taker_bps: Decimal,
    maker_fraction: Decimal,
    taker_fraction: Decimal,
    note: String,
}

impl CostCandidate {
    pub fn commission(
        identity: CostIdentity,
        as_of: DateTime<Utc>,
        input: CommissionComponents,
    ) -> Result<Self, CostError> {
        if identity.venue.as_str() != VenueId::BINANCE_SPOT || !bounded(&input.discount_asset, 32) {
            return Err(CostError::Identity);
        }
        let kinds = [input.standard, input.tax, input.special];
        if kinds.iter().flatten().any(|v| v.value() < Decimal::ZERO)
            || input.discount.value() < Decimal::ZERO
        {
            return Err(CostError::Rate);
        }
        if input.enabled_for_account && input.enabled_for_symbol {
            return Err(CostError::FeePolicyMismatch);
        }
        let rate = |kind: usize| -> Result<Decimal, CostError> {
            let side = |side: usize| {
                sum_bps(kinds.iter().flat_map(|rates| [rates[kind].value(), rates[side].value()]))
            };
            Ok(side(2)?.max(side(3)?))
        };
        let maker = rate(0)?;
        let taker = rate(1)?;
        validate_rates(maker, taker)?;
        let mut hash = canonical(&identity, as_of, CostSource::Venue);
        field(&mut hash, "fractions");
        for value in kinds.iter().flatten() {
            field(&mut hash, &value.value().normalize().to_string());
        }
        field(
            &mut hash,
            if input.enabled_for_account {
                "true"
            } else {
                "false"
            },
        );
        field(
            &mut hash,
            if input.enabled_for_symbol {
                "true"
            } else {
                "false"
            },
        );
        field(&mut hash, &input.discount.value().normalize().to_string());
        field(&mut hash, &input.discount_asset);
        Self::finish(identity, as_of, CostSource::Venue, maker, taker, String::new(), hash)
    }

    pub(super) fn dated(
        identity: CostIdentity,
        as_of: DateTime<Utc>,
        maker: Decimal,
        taker: Decimal,
        note: String,
    ) -> Result<Self, CostError> {
        validate_rates(maker, taker)?;
        if !bounded(&note, 256) {
            return Err(CostError::Config);
        }
        let mut hash = canonical(&identity, as_of, CostSource::DatedOverride);
        field(&mut hash, "bps");
        field(&mut hash, &maker.normalize().to_string());
        field(&mut hash, &taker.normalize().to_string());
        field(&mut hash, &note);
        Self::finish(identity, as_of, CostSource::DatedOverride, maker, taker, note, hash)
    }
    fn finish(
        identity: CostIdentity,
        as_of: DateTime<Utc>,
        source: CostSource,
        maker_bps: Decimal,
        taker_bps: Decimal,
        note: String,
        hash: Sha256,
    ) -> Result<Self, CostError> {
        let maker_fraction = exact_fraction(maker_bps)?;
        let taker_fraction = exact_fraction(taker_bps)?;
        Ok(Self {
            evidence: EvidenceIdentity {
                account: identity,
                as_of,
                source,
                hash: hash.finalize().into(),
            },
            maker_bps,
            taker_bps,
            maker_fraction,
            taker_fraction,
            note,
        })
    }
    pub fn evidence(&self) -> &EvidenceIdentity {
        &self.evidence
    }
    pub fn maker_bps(&self) -> Decimal {
        self.maker_bps
    }
    pub fn taker_bps(&self) -> Decimal {
        self.taker_bps
    }
}

fn validate_rates(maker: Decimal, taker: Decimal) -> Result<(), CostError> {
    if maker < Decimal::ZERO || taker < Decimal::ZERO || taker >= Decimal::from(100) {
        Err(CostError::Rate)
    } else {
        Ok(())
    }
}

// Six native96-bit values aligned to scale28 and multiplied by10,000 require
// fewer than 208 bits. No input-dependent precision or unbounded iteration.
fn sum_bps(values: impl Iterator<Item = Decimal>) -> Result<Decimal, CostError> {
    let mut sum = BigUint::from(0_u8);
    for value in values {
        let coefficient = BigUint::from(value.mantissa().unsigned_abs());
        sum += coefficient * BigUint::from(10_u8).pow(28 - value.scale());
    }
    sum *= BigUint::from(10_000_u32);
    native_decimal(sum, 28)
}

fn exact_fraction(bps: Decimal) -> Result<Decimal, CostError> {
    native_decimal(BigUint::from(bps.mantissa().unsigned_abs()), bps.scale() + 4)
}

fn native_decimal(mut coefficient: BigUint, mut scale: u32) -> Result<Decimal, CostError> {
    let ten = BigUint::from(10_u8);
    while scale > 0 && &coefficient % &ten == BigUint::from(0_u8) {
        coefficient /= &ten;
        scale -= 1;
    }
    let digits = coefficient.to_u32_digits();
    if scale > 28 || digits.len() > 3 {
        return Err(CostError::Unrepresentable);
    }
    Ok(Decimal::from_parts(
        *digits.first().unwrap_or(&0),
        *digits.get(1).unwrap_or(&0),
        *digits.get(2).unwrap_or(&0),
        false,
        scale,
    ))
}

fn canonical(identity: &CostIdentity, as_of: DateTime<Utc>, source: CostSource) -> Sha256 {
    let mut hash = Sha256::new();
    field(&mut hash, "robson:balance-cost-evidence:v1");
    field(&mut hash, &COST_VALIDATION_VERSION.to_string());
    field(&mut hash, &identity.tenant.to_string());
    field(&mut hash, identity.venue.as_str());
    field(&mut hash, &identity.scope);
    field(&mut hash, &identity.uid.to_string());
    field(&mut hash, identity.symbol.base());
    field(&mut hash, identity.symbol.quote());
    field(&mut hash, &as_of.to_rfc3339());
    field(&mut hash, source_name(source));
    field(&mut hash, "Base");
    hash
}
fn field(hash: &mut Sha256, value: &str) {
    hash.update((value.len() as u32).to_be_bytes());
    hash.update(value.as_bytes());
}
fn source_name(source: CostSource) -> &'static str {
    match source {
        CostSource::Venue => API_SOURCE,
        CostSource::DatedOverride => OVERRIDE_SOURCE,
    }
}

#[derive(Debug, Clone)]
pub struct CostTerms {
    candidate: CostCandidate,
    gap_bps: Decimal,
    hash: [u8; 32],
}
impl CostTerms {
    pub fn evidence(&self) -> &EvidenceIdentity {
        self.candidate.evidence()
    }
    pub fn maker_bps(&self) -> Decimal {
        self.candidate.maker_bps
    }
    pub fn taker_bps(&self) -> Decimal {
        self.candidate.taker_bps
    }
    pub fn maker_fraction(&self) -> Decimal {
        self.candidate.maker_fraction
    }
    pub fn taker_fraction(&self) -> Decimal {
        self.candidate.taker_fraction
    }
    pub fn gap_bps(&self) -> Decimal {
        self.gap_bps
    }
    pub fn note(&self) -> &str {
        &self.candidate.note
    }
    pub fn hash(&self) -> &[u8; 32] {
        &self.hash
    }
    /// Source and original note survive the existing AdmissionRow source field.
    pub fn source_provenance(&self) -> String {
        match self.evidence().source {
            CostSource::Venue => API_SOURCE.to_string(),
            CostSource::DatedOverride => format!("{OVERRIDE_SOURCE}:{}", self.candidate.note),
        }
    }
    pub fn envelope(&self) -> ExecutionCostEnvelope {
        ExecutionCostEnvelope {
            maker_bps: self.maker_bps(),
            taker_bps: self.taker_bps(),
            gap_bps: self.gap_bps,
            fee_asset_policy: FeeAssetPolicy::Base,
            funding: None,
            as_of: self.evidence().as_of,
            source: self.source_provenance(),
        }
    }
}

/// Transport absence must be positively confirmed by the authority. A failed or
/// unknown storage result is Unresolved, never an excuse to use an override.
pub enum VenueCostEvidence<'a> {
    Available(&'a CostCandidate),
    Unavailable,
    Invalid(CostError),
    Unresolved,
}

/// Preserve invalid override configuration as an outcome so a valid fresh venue
/// still wins. Only an actually needed override can cause its configuration
/// error.
pub enum DatedOverrideEvidence<'a> {
    Absent,
    Available(&'a CostCandidate),
    Invalid(CostError),
}

pub fn select_costs(
    authority: &impl CostAgeAuthority,
    identity: &CostIdentity,
    venue: VenueCostEvidence<'_>,
    dated_override: DatedOverrideEvidence<'_>,
    gap: ExactDecimal,
) -> Result<CostTerms, CostError> {
    let gap = gap.value();
    if gap < Decimal::ZERO || gap > Decimal::from(100) {
        return Err(CostError::Config);
    }
    let fresh = |candidate: &CostCandidate, source| -> Result<bool, CostError> {
        if candidate.evidence.account != *identity || candidate.evidence.source != source {
            return Err(CostError::Identity);
        }
        Ok(authority.age(candidate.evidence())?.fresh()?)
    };
    let (selected, missing) = match venue {
        VenueCostEvidence::Invalid(error) => return Err(error),
        VenueCostEvidence::Unresolved => return Err(AgeError::Unresolved.into()),
        VenueCostEvidence::Unavailable => {
            authority.confirm_absence(identity)?;
            (None, CostError::Missing)
        },
        VenueCostEvidence::Available(candidate) => {
            (fresh(candidate, CostSource::Venue)?.then_some(candidate), CostError::Stale)
        },
    };
    let selected = if let Some(candidate) = selected {
        candidate
    } else {
        let candidate = match dated_override {
            DatedOverrideEvidence::Absent => return Err(missing),
            DatedOverrideEvidence::Invalid(error) => return Err(error),
            DatedOverrideEvidence::Available(candidate) => candidate,
        };
        if !fresh(candidate, CostSource::DatedOverride)? {
            return Err(CostError::Stale);
        }
        candidate
    };
    let mut hash = Sha256::new();
    field(&mut hash, "robson:balance-cost-terms:v1");
    hash.update(selected.evidence.hash);
    field(&mut hash, "ROBSON_STOP_GAP_BPS:bps");
    field(&mut hash, &gap.normalize().to_string());
    Ok(CostTerms {
        candidate: selected.clone(),
        gap_bps: gap,
        hash: hash.finalize().into(),
    })
}
