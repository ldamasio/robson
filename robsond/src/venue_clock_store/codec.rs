//! Built-in scalar SQL representation; raw values confer no qualification.
use robson_exec::{
    clock_records::{RawClockCapture, TICK_MAX},
    venue_clock::ClockDomain,
};
use rust_decimal::Decimal;
use sqlx::{
    postgres::{PgArguments, PgRow},
    query::Query,
    Postgres, Row, ValueRef,
};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("malformed exact clock representation")]
pub(crate) struct CodecError;

pub(crate) use super::order_columns::ORDER_COLUMNS;

pub(crate) fn decimal(n: i128, lo: i128, hi: i128) -> Result<Decimal, CodecError> {
    if !(lo..=hi).contains(&n) {
        return Err(CodecError);
    }
    Decimal::try_from_i128_with_scale(n, 0).map_err(|_| CodecError)
}

pub(super) fn metadata(scale: Option<i32>, fits: Option<bool>) -> Result<(), CodecError> {
    if scale != Some(0) || fits != Some(true) {
        return Err(CodecError);
    }
    Ok(())
}

pub(crate) fn integer(row: &PgRow, name: &str, lo: i128, hi: i128) -> Result<i128, CodecError> {
    // Read original server metadata BEFORE Decimal normalization/rescaling.
    metadata(
        row.try_get(format!("{name}_scale").as_str()).map_err(|_| CodecError)?,
        row.try_get(format!("{name}_fits").as_str()).map_err(|_| CodecError)?,
    )?;
    let n: Decimal = row.try_get(name).map_err(|_| CodecError)?;
    if n.scale() != 0 || !(lo..=hi).contains(&n.mantissa()) {
        return Err(CodecError);
    }
    Ok(n.mantissa())
}

pub(crate) fn capture(row: &PgRow, prefix: &str) -> Result<Option<RawClockCapture>, CodecError> {
    let names = [
        format!("{prefix}_capture_epoch"),
        format!("{prefix}_wall_seconds"),
        format!("{prefix}_wall_nanoseconds"),
        format!("{prefix}_tick_before"),
        format!("{prefix}_tick_after"),
    ];
    let present: bool = row
        .try_get(format!("{prefix}_domain_present").as_str())
        .map_err(|_| CodecError)?;
    let nulls = names
        .iter()
        .map(|n| row.try_get_raw(n.as_str()).map(|v| v.is_null()).map_err(|_| CodecError))
        .collect::<Result<Vec<_>, _>>()?;
    let kind: Option<String> =
        row.try_get(format!("{prefix}_capture_kind").as_str()).map_err(|_| CodecError)?;
    let boot: Option<Uuid> =
        row.try_get(format!("{prefix}_capture_boot").as_str()).map_err(|_| CodecError)?;
    let seconds: Option<i64> = row
        .try_get(format!("{prefix}_capture_offset_seconds").as_str())
        .map_err(|_| CodecError)?;
    let nanos: Option<i32> = row
        .try_get(format!("{prefix}_capture_offset_nanoseconds").as_str())
        .map_err(|_| CodecError)?;
    if !present
        && nulls.iter().all(|n| *n)
        && kind.is_none()
        && boot.is_none()
        && seconds.is_none()
        && nanos.is_none()
    {
        return Ok(None);
    }
    if !present || nulls.iter().any(|n| *n) || kind.as_deref() != Some("boottime") {
        return Err(CodecError);
    }
    // Validate BOTH metadata pairs before decoding either exact NUMERIC.
    for name in [&names[3], &names[4]] {
        metadata(
            row.try_get(format!("{name}_scale").as_str()).map_err(|_| CodecError)?,
            row.try_get(format!("{name}_fits").as_str()).map_err(|_| CodecError)?,
        )?;
    }
    let raw = RawClockCapture {
        epoch: row.try_get(names[0].as_str()).map_err(|_| CodecError)?,
        wall_seconds: row.try_get(names[1].as_str()).map_err(|_| CodecError)?,
        wall_nanoseconds: u32::try_from(
            row.try_get::<i32, _>(names[2].as_str()).map_err(|_| CodecError)?,
        )
        .map_err(|_| CodecError)?,
        domain: ClockDomain::new(
            boot.ok_or(CodecError)?,
            seconds.ok_or(CodecError)?,
            u32::try_from(nanos.ok_or(CodecError)?).map_err(|_| CodecError)?,
        )
        .map_err(|_| CodecError)?,
        before_ns: integer(row, &names[3], 0, TICK_MAX)?,
        after_ns: integer(row, &names[4], 0, TICK_MAX)?,
    };
    raw.check().map_err(|_| CodecError)?;
    Ok(Some(raw))
}

#[derive(Clone, Copy)]
pub(crate) struct CaptureBind {
    epoch: Option<Uuid>,
    seconds: Option<i64>,
    nanos: Option<i32>,
    before: Option<Decimal>,
    after: Option<Decimal>,
    kind: Option<&'static str>,
    boot: Option<Uuid>,
    offset_seconds: Option<i64>,
    offset_nanos: Option<i32>,
}
impl CaptureBind {
    pub(crate) fn new(raw: Option<&RawClockCapture>) -> Result<Self, CodecError> {
        let Some(r) = raw else {
            return Ok(Self {
                epoch: None,
                seconds: None,
                nanos: None,
                before: None,
                after: None,
                kind: None,
                boot: None,
                offset_seconds: None,
                offset_nanos: None,
            });
        };
        r.check().map_err(|_| CodecError)?;
        let (seconds, nanos) = r.domain.offset();
        Ok(Self {
            epoch: Some(r.epoch),
            seconds: Some(r.wall_seconds),
            nanos: Some(r.wall_nanoseconds as i32),
            before: Some(decimal(r.before_ns, 0, TICK_MAX)?),
            after: Some(decimal(r.after_ns, 0, TICK_MAX)?),
            kind: Some("boottime"),
            boot: Some(r.domain.boot()),
            offset_seconds: Some(seconds),
            offset_nanos: Some(nanos as i32),
        })
    }
    pub(crate) fn bind<'q>(
        self,
        q: Query<'q, Postgres, PgArguments>,
    ) -> Query<'q, Postgres, PgArguments> {
        q.bind(self.epoch)
            .bind(self.seconds)
            .bind(self.nanos)
            .bind(self.before)
            .bind(self.after)
            .bind(self.kind)
            .bind(self.boot)
            .bind(self.offset_seconds)
            .bind(self.offset_nanos)
    }
}
