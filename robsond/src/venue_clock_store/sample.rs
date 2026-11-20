//! Same-statement sample metadata and full original-record decoding.
use chrono::{DateTime, Utc};
use robson_exec::{
    clock_records::{
        RawClockCapture, RawClockSample, SampleNumbers, ALGORITHM_VERSION, THETA_MAX, THETA_MIN,
        TICK_MAX, U2_MAX,
    },
    venue::VenueId,
    venue_clock::ClockDomain,
};
use sqlx::{postgres::PgRow, Row};
use uuid::Uuid;

use super::{codec::CodecError, sample_values as value};

pub(super) const FIELDS: &str = "s.sample_id,
    s.tenant_id,
    s.venue_id,
    s.process_epoch,
    s.send_wall_seconds,
    s.send_wall_nanoseconds,
    s.send_tick_before::TEXT AS send_tick_before,
    s.send_tick_after::TEXT AS send_tick_after,
    s.receive_wall_seconds,
    s.receive_wall_nanoseconds,
    s.receive_tick_before::TEXT AS receive_tick_before,
    s.receive_tick_after::TEXT AS receive_tick_after,
    s.server_ms,
    s.theta2_ns::TEXT AS theta2_ns,
    s.r_low_ns::TEXT AS r_low_ns,
    s.r_high_ns::TEXT AS r_high_ns,
    s.u2_ns::TEXT AS u2_ns,
    s.numeric_verdict,
    s.numeric_failures,
    s.algorithm_version,
    s.recorded_at,
    (s.clock_domain).kind AS clock_kind,
    (s.clock_domain).boot_uuid AS clock_boot,
    (s.clock_domain).offset_seconds AS clock_seconds,
    (s.clock_domain).offset_nanoseconds AS clock_nanos,
    scale(s.send_tick_before) AS send_tick_before_scale,
    budget_integer_fits(s.send_tick_before,0,9223372036854775807999999999) AS send_tick_before_fits,
    scale(s.send_tick_after) AS send_tick_after_scale,
    budget_integer_fits(s.send_tick_after,0,9223372036854775807999999999) AS send_tick_after_fits,
    scale(s.receive_tick_before) AS receive_tick_before_scale,
    budget_integer_fits(s.receive_tick_before,0,9223372036854775807999999999) AS receive_tick_before_fits,
    scale(s.receive_tick_after) AS receive_tick_after_scale,
    budget_integer_fits(s.receive_tick_after,0,9223372036854775807999999999) AS receive_tick_after_fits,
    scale(s.theta2_ns) AS theta2_ns_scale,
    budget_integer_fits(s.theta2_ns,-18446744073709551615999999998,18465190817783261167614000000) AS theta2_ns_fits,
    scale(s.r_low_ns) AS r_low_ns_scale,
    budget_integer_fits(s.r_low_ns,0,9223372036854775807999999999) AS r_low_ns_fits,
    scale(s.r_high_ns) AS r_high_ns_scale,
    budget_integer_fits(s.r_high_ns,0,9223372036854775807999999999) AS r_high_ns_fits,
    scale(s.u2_ns) AS u2_ns_scale,
    budget_integer_fits(s.u2_ns,0,27688562854638036975813999999) AS u2_ns_fits";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Record {
    pub raw: RawClockSample,
    pub numbers: SampleNumbers,
    pub audit: DateTime<Utc>,
}
impl Record {
    pub(super) fn freeze(raw: RawClockSample) -> Result<Self, CodecError> {
        let micros = raw
            .receive
            .wall_seconds
            .checked_mul(1_000_000)
            .and_then(|n| n.checked_add(i64::from(raw.receive.wall_nanoseconds / 1000)))
            .ok_or(CodecError)?;
        let audit = DateTime::from_timestamp_micros(micros).ok_or(CodecError)?;
        let numbers = raw.numbers().map_err(|_| CodecError)?;
        Ok(Self { raw, numbers, audit })
    }
}

pub(super) fn decode(row: &PgRow) -> Result<Record, CodecError> {
    if row.len() != 41 {
        return Err(CodecError);
    }
    // Check original NUMERIC metadata before interpreting any projected text.
    for name in [
        "send_tick_before",
        "send_tick_after",
        "receive_tick_before",
        "receive_tick_after",
        "theta2_ns",
        "r_low_ns",
        "r_high_ns",
        "u2_ns",
    ] {
        value::metadata(row, name)?;
    }
    if value::i16(row, "algorithm_version")? != ALGORITHM_VERSION
        || value::text(row, "clock_kind", "boottime".len())? != "boottime"
    {
        return Err(CodecError);
    }
    let epoch: Uuid = value::uuid(row, "process_epoch")?;
    let domain = ClockDomain::new(
        value::uuid(row, "clock_boot")?,
        value::i64(row, "clock_seconds")?,
        u32::try_from(value::i32(row, "clock_nanos")?).map_err(|_| CodecError)?,
    )
    .map_err(|_| CodecError)?;
    let mark = |prefix: &str| -> Result<RawClockCapture, CodecError> {
        Ok(RawClockCapture {
            epoch,
            domain,
            wall_seconds: value::i64(row, &format!("{prefix}_wall_seconds"))?,
            wall_nanoseconds: u32::try_from(value::i32(
                row,
                &format!("{prefix}_wall_nanoseconds"),
            )?)
            .map_err(|_| CodecError)?,
            before_ns: value::integer(row, &format!("{prefix}_tick_before"), 0, TICK_MAX)?,
            after_ns: value::integer(row, &format!("{prefix}_tick_after"), 0, TICK_MAX)?,
        })
    };
    let raw = RawClockSample {
        id: value::uuid(row, "sample_id")?,
        tenant: value::uuid(row, "tenant_id")?,
        venue: VenueId::new(value::text(row, "venue_id", VenueId::BINANCE_SPOT.len())?),
        send: mark("send")?,
        receive: mark("receive")?,
        server_ms: value::i64(row, "server_ms")?,
    };
    let numbers = SampleNumbers {
        theta2: value::integer(row, "theta2_ns", THETA_MIN, THETA_MAX)?,
        r_low: value::integer(row, "r_low_ns", 0, TICK_MAX)?,
        r_high: value::integer(row, "r_high_ns", 0, TICK_MAX)?,
        u2: value::integer(row, "u2_ns", 0, U2_MAX)?,
        failures: value::i16(row, "numeric_failures")?,
    };
    let verdict = value::text(row, "numeric_verdict", "invalid".len())?;
    if raw.numbers().map_err(|_| CodecError)? != numbers
        || verdict
            != if numbers.failures == 0 {
                "valid"
            } else {
                "invalid"
            }
    {
        return Err(CodecError);
    }
    Ok(Record { raw, numbers, audit: value::audit(row)? })
}
