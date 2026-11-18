//! Exact original38 representation, not a generic SQL parser.
use sqlx::migrate::{MigrateError, Migration, MigrationType};

use super::{refusal, VERSION};
mod manifest;
const ORIGINAL: &str = include_str!("../../../migrations/20240101000038_request_budget.sql");
const CHECKSUM: &[u8;48] = b"\xdb\xbe\x48\x61\xd2\xa8\xca\x80\xbb\x83\x81\x72\xc3\x22\xe9\xbb\x28\x4d\x35\x08\xfe\x72\xba\x1b\xe4\x91\x7a\x9f\x08\x22\x0c\x69\xe3\x7d\x3a\x26\xda\x85\x9a\xd3\xa0\x40\xc3\x4f\x66\x5d\x7a\xaf";
#[derive(Clone, Copy)]
struct Part {
    start: usize,
    end: usize,
    checksum: &'static [u8; 48],
}
pub(super) fn chunks(migration: &Migration) -> Result<[&str; 52], MigrateError> {
    if migration.version != VERSION
        || migration.no_tx
        || migration.migration_type != MigrationType::Simple
        || migration.description.as_ref() != "request budget"
        || migration.sql.as_ref() != ORIGINAL
        || migration.checksum.as_ref() != CHECKSUM
    {
        return Err(refusal("migration38 fixed identity mismatch"));
    }
    split(migration.sql.as_ref(), &manifest::PARTS)
}
fn split<'a>(sql: &'a str, parts: &[Part]) -> Result<[&'a str; 52], MigrateError> {
    if parts.len() != 52 {
        return Err(refusal("migration38 fixed statement count mismatch"));
    }
    let mut chunks = [""; 52];
    let mut previous = 0;
    for (index, part) in parts.iter().enumerate() {
        if part.start < previous || part.start >= part.end {
            return Err(refusal("migration38 fixed range mismatch"));
        }
        let chunk = sql
            .get(part.start..part.end)
            .ok_or_else(|| refusal("migration38 fixed UTF8/range mismatch"))?;
        // Public checksum computation only; this temporary is never executed
        // or supplied to a migrator. One48-byte checksum at a time, borrowed SQL.
        let canonical: &'static str = ORIGINAL
            .get(part.start..part.end)
            .ok_or_else(|| refusal("migration38 original range mismatch"))?;
        if chunk != canonical {
            return Err(refusal("migration38 original statement mismatch"));
        }
        let digest = Migration::new(
            VERSION,
            "checksum only".into(),
            MigrationType::Simple,
            canonical.into(),
            false,
        );
        if digest.checksum.as_ref() != part.checksum {
            return Err(refusal("migration38 fixed statement bytes mismatch"));
        }
        chunks[index] = chunk;
        previous = part.end;
    }
    Ok(chunks)
}
#[cfg(test)]
mod tests;
