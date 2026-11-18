//! Checked B2 sample values; shared order/capture decoders stay unchanged.
use chrono::{DateTime, Utc};
use sqlx::{
    postgres::{PgRow, PgValueFormat},
    Row, ValueRef,
};
use uuid::Uuid;

use super::codec::{metadata as check_metadata, CodecError};

fn bytes<'r>(row: &'r PgRow, name: &str, oid: u32) -> Result<&'r [u8], CodecError> {
    let value = row.try_get_raw(name).map_err(|_| CodecError)?;
    if value.format() != PgValueFormat::Binary
        || value.type_info().oid().map(|oid| oid.0) != Some(oid)
    {
        return Err(CodecError);
    }
    value.as_bytes().map_err(|_| CodecError)
}

pub(super) fn text<'r>(row: &'r PgRow, name: &str, max: usize) -> Result<&'r str, CodecError> {
    let value = bytes(row, name, 25)?;
    if value.len() > max {
        return Err(CodecError);
    }
    std::str::from_utf8(value).map_err(|_| CodecError)
}

pub(super) fn i16(row: &PgRow, name: &str) -> Result<i16, CodecError> {
    Ok(i16::from_be_bytes(bytes(row, name, 21)?.try_into().map_err(|_| CodecError)?))
}

pub(super) fn i32(row: &PgRow, name: &str) -> Result<i32, CodecError> {
    Ok(i32::from_be_bytes(bytes(row, name, 23)?.try_into().map_err(|_| CodecError)?))
}

pub(super) fn i64(row: &PgRow, name: &str) -> Result<i64, CodecError> {
    Ok(i64::from_be_bytes(bytes(row, name, 20)?.try_into().map_err(|_| CodecError)?))
}

pub(super) fn uuid(row: &PgRow, name: &str) -> Result<Uuid, CodecError> {
    Uuid::from_slice(bytes(row, name, 2950)?).map_err(|_| CodecError)
}

pub(super) fn metadata(row: &PgRow, name: &str) -> Result<(), CodecError> {
    let scale = i32(row, &format!("{name}_scale"))?;
    let fits = match bytes(row, &format!("{name}_fits"), 16)? {
        [0] => false,
        [1] => true,
        _ => return Err(CodecError),
    };
    check_metadata(Some(scale), Some(fits))
}

fn parse_integer(value: &str, lo: i128, hi: i128) -> Result<i128, CodecError> {
    if value.len() > 30 {
        return Err(CodecError);
    }
    let n = value.parse::<i128>().map_err(|_| CodecError)?;
    if !(lo..=hi).contains(&n) {
        return Err(CodecError);
    }
    Ok(n)
}

pub(super) fn integer(row: &PgRow, name: &str, lo: i128, hi: i128) -> Result<i128, CodecError> {
    parse_integer(text(row, name, 30)?, lo, hi)
}

fn timestamp(micros: i64) -> Result<DateTime<Utc>, CodecError> {
    let unix_micros = micros.checked_add(946_684_800_000_000).ok_or(CodecError)?;
    DateTime::from_timestamp_micros(unix_micros).ok_or(CodecError)
}

pub(super) fn audit(row: &PgRow) -> Result<DateTime<Utc>, CodecError> {
    let micros =
        i64::from_be_bytes(bytes(row, "recorded_at", 1184)?.try_into().map_err(|_| CodecError)?);
    timestamp(micros)
}

#[cfg(test)]
#[path = "sample_values_tests.rs"]
mod tests;
