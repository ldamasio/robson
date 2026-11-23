//! The settlement barrier and month invalidity of one balance-account venue
//! (ADR-0056 step 8l-2; v16 §3.7.3; PLAN-r4 §6.3), with the
//! `accounting_invalidations` reader. Inert: no production caller until the
//! cash writer (8l-3), the balance snapshot (8l-8) and eligibility (8n).
//! Protection never reads it.
//!
//! One read-only statement ([`SETTLEMENT_BARRIER`]) on the caller's
//! connection: it can sit inside a row-holding transaction or a REPEATABLE
//! READ snapshot whose bounds the caller set, and it takes no row lock. Its
//! cost is linear in the venue's legs, orders, fills and intents, through
//! their indexes (measured in `pg_tests` at PLAN-r4 §7.2's cardinality). The
//! in-memory side of the barrier (a stream suspended in memory, evidence not
//! yet committed) is the gate's invalidity generation and pending read.
use chrono::{DateTime, Datelike, NaiveDate, Utc};
use robson_exec::{venue::VenueId, venue_ledger::LedgerError};
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use super::backend;

/// The statement; `settlement.sql` documents its parameters and rules.
pub const SETTLEMENT_BARRIER: &str = include_str!("settlement.sql");

/// The commission asset a symbol's fills carry under fee policy `base`
/// (SD2): a buy pays in `base`, a sell in `quote`. From the venue's rules;
/// the ledger stores neither.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommissionAssets<'a> {
    pub symbol: &'a str,
    pub base: &'a str,
    pub quote: &'a str,
}

#[derive(Debug, Clone, Copy)]
pub struct SettlementQuery<'a> {
    pub tenant_id: Uuid,
    pub venue: &'a VenueId,
    /// The first day of the UTC month asked about, current or historical.
    pub month: NaiveDate,
    /// Robson's wall clock at the read: where a life not yet seen terminal
    /// ends.
    pub now: DateTime<Utc>,
    /// Every symbol the venue's legs trade; a fill of an unlisted one is
    /// unexpected.
    pub assets: &'a [CommissionAssets<'a>],
}

/// Unsettled legs, in total and by condition; one leg can hold several.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct UnsettledLegs {
    pub total: u64,
    pub unknown_order: u64,
    pub unrowed_intent: u64,
    pub incomplete_fills: u64,
    pub provisional_fee: u64,
    pub unexpected_asset: u64,
    pub suspended: u64,
    pub broken_identity: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SettlementBarrier {
    /// The barrier now, whatever the month: every unsettled leg of the venue.
    pub unsettled: UnsettledLegs,
    /// An unsettled order's, intent's or leg's widened life meets the month.
    pub life_meets_month: bool,
    /// `accounting_invalidations` rows of the month.
    pub invalidations: u64,
    /// The month starts after `now`: nothing proves it yet.
    pub not_begun: bool,
}

impl SettlementBarrier {
    pub fn settled(&self) -> bool {
        self.unsettled.total == 0
    }

    /// The month's accounting is invalid for admission, MonthlyHalt, the
    /// late revalidation, capacity and the month view (v16 §3.7.3).
    pub fn month_invalid(&self) -> bool {
        self.life_meets_month || self.invalidations > 0 || self.not_begun
    }
}

/// Read the barrier and the month's validity in one statement. Any error is
/// no answer: the caller treats the month as invalid and the venue as
/// unsettled.
pub async fn read_settlement(
    connection: &mut PgConnection,
    query: &SettlementQuery<'_>,
) -> Result<SettlementBarrier, LedgerError> {
    let [symbols, bases, quotes] = assets(query)?;
    let row = sqlx::query(SETTLEMENT_BARRIER)
        .bind(query.tenant_id)
        .bind(query.venue.as_str())
        .bind(query.month)
        .bind(query.now)
        .bind(&symbols)
        .bind(&bases)
        .bind(&quotes)
        .fetch_one(connection)
        .await
        .map_err(backend)?;
    // An unknown venue has no ledger to be settled by.
    if !row.try_get::<bool, _>("venue_known").map_err(backend)? {
        return Err(LedgerError::NoAccountingRow);
    }
    let count = |column: &str| {
        let value: i64 = row.try_get(column).map_err(backend)?;
        u64::try_from(value).map_err(|_| LedgerError::Invariant(format!("negative {column}")))
    };
    let first = query.month.and_hms_opt(0, 0, 0).map(|midnight| midnight.and_utc());
    let barrier = SettlementBarrier {
        unsettled: UnsettledLegs {
            total: count("unsettled")?,
            unknown_order: count("unknown_order")?,
            unrowed_intent: count("unrowed_intent")?,
            incomplete_fills: count("incomplete_fills")?,
            provisional_fee: count("provisional_fee")?,
            unexpected_asset: count("unexpected_asset")?,
            suspended: count("suspended")?,
            broken_identity: count("broken_identity")?,
        },
        life_meets_month: row.try_get("life_meets_month").map_err(backend)?,
        invalidations: count("invalidations")?,
        not_begun: first.is_none_or(|first| query.now < first),
    };
    crate::metrics::observe_leg_settlement_pending(barrier.unsettled.total);
    Ok(barrier)
}

/// The expected assets as three parallel arrays, refused before any SQL
/// unless the month is named by its first day and each symbol appears once
/// with no blank field.
fn assets<'a>(query: &SettlementQuery<'a>) -> Result<[Vec<&'a str>; 3], LedgerError> {
    if query.month.day() != 1 {
        return Err(LedgerError::InvalidEvidence("a month is named by its first day".into()));
    }
    let mut columns: [Vec<&str>; 3] = Default::default();
    for assets in query.assets {
        let fields = [assets.symbol, assets.base, assets.quote];
        if fields.iter().any(|text| text.trim().is_empty()) || columns[0].contains(&assets.symbol) {
            return Err(LedgerError::InvalidEvidence(
                "commission assets need one complete entry per symbol".into(),
            ));
        }
        for (column, field) in columns.iter_mut().zip(fields) {
            column.push(field);
        }
    }
    Ok(columns)
}

#[cfg(test)]
mod pg_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_misnamed_month_or_ambiguous_assets_are_refused_before_sql() {
        fn query<'a>(day: u32, assets: &'a [CommissionAssets<'a>]) -> SettlementQuery<'a> {
            static VENUE: std::sync::LazyLock<VenueId> =
                std::sync::LazyLock::new(|| VenueId::new(VenueId::BINANCE_SPOT));
            let month = NaiveDate::from_ymd_opt(2026, 11, day).unwrap();
            SettlementQuery {
                tenant_id: Uuid::from_u128(1),
                venue: &VENUE,
                month,
                now: Utc::now(),
                assets,
            }
        }
        let pair = CommissionAssets {
            symbol: "BTCUSDT",
            base: "BTC",
            quote: "USDT",
        };
        let blank = CommissionAssets { quote: " ", ..pair };
        assert_eq!(assets(&query(1, &[pair])), Ok([vec!["BTCUSDT"], vec!["BTC"], vec!["USDT"]]));
        assert_eq!(
            assets(&query(1, &[])),
            Ok(Default::default()),
            "nothing expected: all unexpected"
        );
        for refused in [
            query(2, &[pair]),
            query(1, &[pair, pair]),
            query(1, &[blank]),
        ] {
            assert!(matches!(assets(&refused), Err(LedgerError::InvalidEvidence(_))));
        }
    }
}
