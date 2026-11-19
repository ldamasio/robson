//! Canonical v1 retained fee format from the owned original response.
//! Decoding can describe evidence. Only verified native history can make
//! that evidence available to prepared selection.
use std::{fmt, marker::PhantomData};

use chrono::{DateTime, Utc};
use robson_store::native_n5::MAX_PAYLOAD_BYTES;
use serde::{
    de::{value::MapAccessDeserializer, MapAccess, Visitor},
    Deserialize, Deserializer, Serialize,
};
use uuid::Uuid;

use super::{exact::ExactDecimal, CommissionComponents, CostCandidate, CostError, CostIdentity};
use crate::{
    clock_records::RawClockCapture,
    venue_clock::{ClockDomain, TickCapture},
};

pub const MAX_RESPONSE_BYTES: usize = 16_384;
const RECEIPT_FORMAT_VERSION: u16 = 1;
const NANOS_PER_SECOND: i128 = 1_000_000_000;
// PostgreSQL MIN_TIMESTAMP (-211813488000000000 μs from its 2000 epoch),
// expressed in Unix wall seconds.
// Chrono's upper limit is already earlier than PostgreSQL's.
const POSTGRES_MIN_UNIX_SECONDS: i64 = -210_866_803_200;

// Derive deserialization so repeated known keys fail at every nesting level.
// The wire types keep validated string spelling and skip unrelated fields.
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct FeeFields {
    symbol: String,
    #[serde(deserialize_with = "object")]
    standard_commission: Rates,
    #[serde(deserialize_with = "object")]
    special_commission: Rates,
    #[serde(deserialize_with = "object")]
    tax_commission: Rates,
    #[serde(deserialize_with = "object")]
    discount: Discount,
}

// Serde struct derives also accept ordered sequences. Require the named venue
// object shape at every level while retaining derived duplicate-key checks.
fn object<'de, D, T>(decoder: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    struct Object<T>(PhantomData<T>);

    impl<'de, T: Deserialize<'de>> Visitor<'de> for Object<T> {
        type Value = T;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("a commission object with named fields")
        }

        fn visit_map<M: MapAccess<'de>>(self, map: M) -> Result<T, M::Error> {
            T::deserialize(MapAccessDeserializer::new(map))
        }
    }

    decoder.deserialize_map(Object(PhantomData))
}

#[derive(Deserialize, Serialize)]
struct Rates {
    maker: String,
    taker: String,
    buyer: String,
    seller: String,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Discount {
    enabled_for_account: bool,
    enabled_for_symbol: bool,
    discount_asset: String,
    discount: String,
}

#[derive(Deserialize, Serialize)]
struct RetainedReceipt {
    format_version: u16,
    original: OriginalMark,
    commission: FeeFields,
}

#[derive(Deserialize, Serialize)]
struct OriginalMark {
    epoch: Uuid,
    boot: Uuid,
    offset_seconds: i64,
    offset_nanoseconds: u32,
    wall_seconds: i64,
    wall_nanoseconds: u32,
    // Decimal strings preserve exact ticks even in readers with narrow JSON integers.
    before_ns: String,
    after_ns: String,
}

impl From<RawClockCapture> for OriginalMark {
    fn from(raw: RawClockCapture) -> Self {
        let (offset_seconds, offset_nanoseconds) = raw.domain.offset();
        Self {
            epoch: raw.epoch,
            boot: raw.domain.boot(),
            offset_seconds,
            offset_nanoseconds,
            wall_seconds: raw.wall_seconds,
            wall_nanoseconds: raw.wall_nanoseconds,
            before_ns: raw.before_ns.to_string(),
            after_ns: raw.after_ns.to_string(),
        }
    }
}

fn response_time(
    raw: RawClockCapture,
    handoff: TickCapture,
) -> Option<(TickCapture, DateTime<Utc>)> {
    // This checks both endpoints, bracket order, epoch shape, and wall fraction.
    // The owner independently binds the epoch to the actual attempt.
    raw.check().ok()?;
    let observed = TickCapture::new(
        raw.domain,
        i64::try_from(raw.before_ns / NANOS_PER_SECOND).ok()?,
        i64::try_from(raw.before_ns % NANOS_PER_SECOND).ok()?,
    )
    .ok()?;
    observed.elapsed_since(&handoff).ok()?;
    if raw.wall_seconds < POSTGRES_MIN_UNIX_SECONDS {
        return None;
    }
    let as_of =
        DateTime::<Utc>::from_timestamp(raw.wall_seconds, raw.wall_nanoseconds / 1_000 * 1_000)?;
    Some((observed, as_of))
}

impl Rates {
    fn exact(&self) -> Result<[ExactDecimal; 4], CostError> {
        let exact = |value: &str| value.parse::<ExactDecimal>().map_err(|_| CostError::Rate);
        Ok([
            exact(&self.maker)?,
            exact(&self.taker)?,
            exact(&self.buyer)?,
            exact(&self.seller)?,
        ])
    }
}

impl FeeFields {
    fn candidate(
        &self,
        identity: CostIdentity,
        as_of: DateTime<Utc>,
    ) -> Result<CostCandidate, CostError> {
        if self.symbol != identity.symbol().as_pair() {
            return Err(CostError::Identity);
        }
        CostCandidate::commission(
            identity,
            as_of,
            CommissionComponents {
                standard: self.standard_commission.exact()?,
                tax: self.tax_commission.exact()?,
                special: self.special_commission.exact()?,
                enabled_for_account: self.discount.enabled_for_account,
                enabled_for_symbol: self.discount.enabled_for_symbol,
                discount: self.discount.discount.parse().map_err(|_| CostError::Rate)?,
                discount_asset: self.discount.discount_asset.clone(),
            },
        )
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum RetentionError {
    InvalidPayload,
    PolicyMismatch,
    Unknown,
}

pub struct EncodedCommission {
    pub observed: TickCapture,
    pub candidate: CostCandidate,
    pub payload: Vec<u8>,
}

/// Preserve only the bounded v1 fields and exact original capture.
pub fn encode_retained(
    body: &[u8],
    original: RawClockCapture,
    expected: &CostIdentity,
    handoff: TickCapture,
) -> Result<EncodedCommission, RetentionError> {
    if body.len() > MAX_RESPONSE_BYTES {
        return Err(RetentionError::InvalidPayload);
    }
    let (observed, as_of) = response_time(original, handoff).ok_or(RetentionError::Unknown)?;
    let mut decoder = serde_json::Deserializer::from_slice(body);
    let commission: FeeFields = object(&mut decoder).map_err(|_| RetentionError::InvalidPayload)?;
    decoder.end().map_err(|_| RetentionError::InvalidPayload)?;
    let candidate = commission.candidate(expected.clone(), as_of).map_err(|error| match error {
        CostError::FeePolicyMismatch => RetentionError::PolicyMismatch,
        _ => RetentionError::InvalidPayload,
    })?;
    let payload = serde_json::to_vec(&RetainedReceipt {
        format_version: RECEIPT_FORMAT_VERSION,
        original: original.into(),
        commission,
    })
    .map_err(|_| RetentionError::Unknown)?;
    if payload.len() > MAX_PAYLOAD_BYTES {
        return Err(RetentionError::InvalidPayload);
    }
    Ok(EncodedCommission { observed, candidate, payload })
}

/// Cold decode of one immutable retained receipt. The caller binds the
/// original epoch, domain, oldest tick, as-of, and hash to its own columns.
#[derive(Debug, Clone)]
pub struct DecodedCommission {
    pub original: RawClockCapture,
    pub observed: TickCapture,
    pub candidate: CostCandidate,
}

pub fn decode_retained(
    payload: &[u8],
    expected: &CostIdentity,
    handoff: TickCapture,
) -> Option<DecodedCommission> {
    if payload.is_empty() || payload.len() > MAX_PAYLOAD_BYTES {
        return None;
    }
    // Derived fields reject repeated keys; only the writer's exact compact output
    // round-trips, so foreign, reordered, escaped, or padded spellings fail here.
    let receipt: RetainedReceipt = serde_json::from_slice(payload).ok()?;
    if receipt.format_version != RECEIPT_FORMAT_VERSION
        || serde_json::to_vec(&receipt).ok()? != payload
    {
        return None;
    }
    let mark = &receipt.original;
    let tick = |text: &str| text.parse::<i128>().ok().filter(|n| n.to_string() == text);
    let original = RawClockCapture {
        epoch: mark.epoch,
        wall_seconds: mark.wall_seconds,
        wall_nanoseconds: mark.wall_nanoseconds,
        domain: ClockDomain::new(mark.boot, mark.offset_seconds, mark.offset_nanoseconds).ok()?,
        before_ns: tick(&mark.before_ns)?,
        after_ns: tick(&mark.after_ns)?,
    };
    let (observed, as_of) = response_time(original, handoff)?;
    let candidate = receipt.commission.candidate(expected.clone(), as_of).ok()?;
    Some(DecodedCommission { original, observed, candidate })
}
