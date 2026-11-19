//! Prepare a result from the original bounded commission response and EOF capture.

use std::{fmt, marker::PhantomData};

use chrono::{DateTime, Utc};
use robson_connectors::binance::spot::SpotCommission;
use robson_exec::{
    balance_costs::{CostError, CostIdentity},
    clock_records::RawClockCapture,
    venue_clock::TickCapture,
};
use serde::{
    de::{value::MapAccessDeserializer, MapAccess, Visitor},
    Deserialize, Deserializer, Serialize,
};
use uuid::Uuid;

use super::{
    result::{PreparedSnapshot, MAX_PAYLOAD_BYTES},
    ClaimResult,
};

pub(super) const MAX_RESPONSE_BYTES: usize = 16_384;
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

#[derive(Serialize)]
struct RetainedReceipt {
    format_version: u16,
    original: OriginalMark,
    commission: FeeFields,
}

#[derive(Serialize)]
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

pub(super) fn prepare_response(
    body: &[u8],
    original: RawClockCapture,
    expected: &CostIdentity,
    handoff: TickCapture,
) -> Result<PreparedSnapshot, ClaimResult> {
    if body.len() > MAX_RESPONSE_BYTES {
        return Err(ClaimResult::InvalidPayload);
    }
    let (observed, as_of) = response_time(original, handoff).ok_or(ClaimResult::Unknown)?;
    // The same bounded bytes feed the existing exact fee parser and the retained
    // whitelist. The caller's verified account remains the only account source.
    let mut decoder = serde_json::Deserializer::from_slice(body);
    let commission = object(&mut decoder).map_err(|_| ClaimResult::InvalidPayload)?;
    decoder.end().map_err(|_| ClaimResult::InvalidPayload)?;
    let parsed: SpotCommission =
        serde_json::from_slice(body).map_err(|_| ClaimResult::InvalidPayload)?;
    let candidate =
        parsed.cost_candidate(expected.clone(), as_of).map_err(|error| match error {
            CostError::FeePolicyMismatch => ClaimResult::PolicyMismatch,
            _ => ClaimResult::InvalidPayload,
        })?;
    let payload = serde_json::to_vec(&RetainedReceipt {
        format_version: RECEIPT_FORMAT_VERSION,
        original: original.into(),
        commission,
    })
    .map_err(|_| ClaimResult::Unknown)?;
    if payload.len() > MAX_PAYLOAD_BYTES {
        return Err(ClaimResult::InvalidPayload);
    }
    Ok(PreparedSnapshot {
        evidence: candidate.evidence().clone(),
        observed,
        payload,
    })
}

#[cfg(test)]
#[path = "response_tests.rs"]
mod tests;
