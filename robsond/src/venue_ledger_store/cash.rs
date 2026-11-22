//! The cash ledger of one balance-account venue (ADR-0056 step 8l-3; v16
//! §C1, §C8; PLAN-r4 §6.4, §6.6, §6.10): `E(t)` with its read-time
//! impairment rule and the cash validity predicates (8l-3a), and the cash
//! writer ([`writer`], 8l-3b). Inert: no production caller until 8k-3 hands
//! the writer its qualified attempts and 8l-8, 8p and 8n read the
//! predicates.
//!
//! A discrepancy is evidence, never accounting: `E(t)` reads observed
//! anchors, leg-owned fills and attested flows only, and nothing here writes
//! (8m's cutover and 8l-5's re-anchor write anchors and flows). Late evidence
//! is derived at read time from immutable rows (`expected.sql`,
//! `discrepancy.sql`): nothing is marked, so a restart reads the same answer.
use std::sync::LazyLock;

use chrono::{DateTime, Utc};
use robson_exec::{venue::VenueId, venue_ledger::LedgerError};
use rust_decimal::Decimal;
use sqlx::{postgres::PgRow, PgConnection, Row};
use uuid::Uuid;

use super::{
    backend,
    settlement::{asset_columns, CommissionAssets},
};

#[cfg(test)]
mod pg_tests;
// 8l-5's re-anchor: composed at 5a with PS-5b's ACCOUNT reads.
#[cfg_attr(not(test), allow(dead_code, reason = "8l-5: composed at 5a"))]
pub(crate) mod reanchor;
#[cfg_attr(
    not(test),
    allow(
        dead_code,
        reason = "8l-3b cash writer: no production caller until 8k-3"
    )
)]
pub(crate) mod writer;

const EXPECTED: &str = include_str!("cash/expected.sql");
const DISCREPANCY: &str = include_str!("cash/discrepancy.sql");

/// `E(t)` as a sub-select, `t` being the caller's parameter.
fn expected_at(t: &str) -> String {
    EXPECTED.replace("{t}", t)
}

/// `E(t)` and the cash validity predicates in one statement (`state.sql`),
/// for 8l-8's, 8p's and 8n's own transactions: `$1` tenant, `$2` venue, `$3`
/// t, `$4` now, `$5..$7` the expected commission assets.
pub static CASH_STATE: LazyLock<String> = LazyLock::new(|| {
    include_str!("cash/state.sql")
        .replace("{expected_t}", &expected_at("$3"))
        .replace("{expected_now}", &expected_at("$4"))
        .replace("{discrepancy}", DISCREPANCY)
});

/// What a reader asks: `E` and V6 at `t`, impairment and the fence at `now`.
#[derive(Debug, Clone, Copy)]
pub struct CashQuery<'a> {
    pub tenant_id: Uuid,
    pub venue: &'a VenueId,
    pub t: DateTime<Utc>,
    pub now: DateTime<Utc>,
    /// The venue rules' commission assets, as 8l-2's barrier takes them.
    pub assets: &'a [CommissionAssets<'a>],
}

/// `E(t)` and the observed anchor it rests on (8p's `quote_B` at t = B).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExpectedQuote {
    /// The anchor's id and `as_of`; `None` when no observed anchor precedes t.
    pub anchor: Option<(Uuid, DateTime<Utc>)>,
    /// t lies in an impaired span (PLAN-r4 §6.10).
    pub impaired: bool,
    /// `None` when impaired, with no anchor, with a counted fill in an
    /// unexpected commission asset, or with a sum that does not fit.
    pub value: Option<Decimal>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CashState {
    pub expected: ExpectedQuote,
    /// `E(now)` is impaired: the current month is invalid.
    pub impaired_now: bool,
    /// V5's clause: an open missing discrepancy with a clean sighting below
    /// −0.01.
    pub open_missing: bool,
    /// V6: such a discrepancy whose stored checkpoint cut is before t.
    pub missing_before_t: bool,
    /// An attested withdrawal attributed to now's UTC month.
    pub withdrawal_fence: bool,
    /// The open discrepancy's minimum Δ over its clean sightings.
    pub effective_minimum: Option<Decimal>,
}

/// One read-only statement on the caller's connection or transaction; any
/// error is no answer, which the caller reads as invalid.
pub async fn read_cash_state(
    connection: &mut PgConnection,
    query: &CashQuery<'_>,
) -> Result<CashState, LedgerError> {
    let [symbols, bases, quotes] = asset_columns(query.assets)?;
    let row = sqlx::query(&CASH_STATE)
        .bind(query.tenant_id)
        .bind(query.venue.as_str())
        .bind(query.t)
        .bind(query.now)
        .bind(&symbols)
        .bind(&bases)
        .bind(&quotes)
        .fetch_one(connection)
        .await
        .map_err(backend)?;
    let anchor_id: Option<Uuid> = row.try_get("anchor_id").map_err(backend)?;
    let anchor_as_of: Option<DateTime<Utc>> = row.try_get("anchor_as_of").map_err(backend)?;
    let state = CashState {
        expected: ExpectedQuote {
            anchor: anchor_id.zip(anchor_as_of),
            impaired: flag(&row, "impaired")?,
            value: row.try_get("expected").map_err(backend)?,
        },
        impaired_now: flag(&row, "impaired_now")?,
        open_missing: flag(&row, "open_missing")?,
        missing_before_t: flag(&row, "missing_before")?,
        withdrawal_fence: flag(&row, "withdrawal_fence")?,
        effective_minimum: row.try_get("effective_minimum").map_err(backend)?,
    };
    crate::metrics::observe_cash_impaired(state.impaired_now);
    Ok(state)
}

fn flag(row: &PgRow, column: &str) -> Result<bool, LedgerError> {
    row.try_get(column).map_err(backend)
}
