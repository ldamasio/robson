//! ADR-0056 step 8p-4: the leg facts of month K's boundary B (8p PLAN-r3
//! §6.4), the input of 8p-3's as-of-B calculus. One read-only statement
//! ([`LEG_FACTS`]) on the caller's connection: 8p-5 reads it inside its
//! completion transaction, once 8l-2's barrier found the venue settled. No
//! production caller until 8p-5.
use chrono::{Datelike as _, NaiveDate, NaiveTime};
use robson_exec::{boundary_carry::CandidateLeg, venue::VenueId, venue_ledger::LedgerError};
use sqlx::{types::Json, PgConnection};
use uuid::Uuid;

use super::backend;

/// The statement; `leg_facts.sql` documents its parameters and rules.
pub const LEG_FACTS: &str = include_str!("leg_facts.sql");

/// The candidate legs of month K (`month`, its first day) on the venue, by
/// position, with every fact 8p-3 asks for: Robson times with their stored
/// verdicts, every trade of each listed order and every cancel intent of the
/// leg. A key 8p-3's types do not know refuses the read; the owner's lookup
/// and the venue's order times are never read from it. Any error is no
/// answer: K stays pending.
pub async fn read_leg_facts(
    connection: &mut PgConnection,
    tenant_id: Uuid,
    venue: &VenueId,
    month: NaiveDate,
) -> Result<Vec<CandidateLeg>, LedgerError> {
    if month.day() != 1 {
        return Err(LedgerError::InvalidEvidence("a month is named by its first day".into()));
    }
    let legs: Vec<Json<CandidateLeg>> = sqlx::query_scalar(LEG_FACTS)
        .bind(tenant_id)
        .bind(venue.as_str())
        .bind(month.and_time(NaiveTime::MIN).and_utc())
        .fetch_all(connection)
        .await
        .map_err(backend)?;
    Ok(legs.into_iter().map(|Json(leg)| leg).collect())
}

#[cfg(test)]
mod pg_tests;

#[cfg(test)]
mod tests;
