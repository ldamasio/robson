//! The cutover report (ADR-0056 step 8m-3, 8m PLAN-r4 §6.2): checks C1 to
//! C10 with C9a-d, the partition check, C9c and SD14, over evidence read
//! fresh: the database part in one REPEATABLE READ, READ ONLY transaction
//! ([`read_database`], whose statement returns its verdicts with every value
//! they compare), the venue part from the Spot venue's ports, the USD-M
//! account through a [`UsdmReadObserver`] (C6, reads only) and the process's
//! gate, journal and stream memory ([`RuntimeFacts`]). A read that failed fails
//! its check with its reason, so the report is never green on missing evidence.
//! The hash covers every verdict and decision value, never a timestamp, a
//! reason's text or the certificate's generation. Nothing here writes, and no
//! production path calls it before 8m-4 and 8m-6.
use std::{collections::BTreeMap, time::Duration};

use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, Utc};
use robson_connectors::{BinanceRestClient, BinanceRestError};
use robson_domain::Symbol;
use robson_exec::venue::{ExecutionCostEnvelope, FeeAssetPolicy, InventoryObservation};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::{types::Json, PgPool};
use uuid::Uuid;

/// The last event types that leave a position stream terminal (C9b): those
/// whose projection ends its row closed, cancelled or in error.
pub(crate) const TERMINAL: [&str; 4] = [
    "POSITION_CLOSED",
    "position_closed",
    "position_disarmed",
    "entry_execution_rejected",
];
/// The database checks the statement returns, with what fails each.
const DATABASE_CHECKS: [(&str, &str); 7] = [
    (
        "c7",
        "an open incident or stream suspension, boundary B, the latch, or the epochs",
    ),
    ("c8", "an unfinished funding saga"),
    ("c9a", "a position event in the month, or its partition missing or detached"),
    (
        "c9b",
        "an open position row, or a position stream without a row and not terminal",
    ),
    ("c9c", "no landed reset, a month peak, or a base other than the latest reset's"),
    ("c9d", "a position filled or closed in the month"),
    ("sd14", "a budget model other than net-from-start"),
];

/// The database part as of one snapshot: its verdicts, the values they
/// compare, the month, C10's previous basis (the month's capital base), the
/// open incidents' reasons and the venue's run, if any.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub(crate) struct DatabaseFacts {
    pub month: NaiveDate,
    pub previous: Option<Decimal>,
    pub checks: BTreeMap<String, bool>,
    pub values: BTreeMap<String, Value>,
    /// The open incidents' reasons, shown in C7's detail and never hashed.
    pub incident_reasons: Option<Vec<String>>,
    pub run: Option<Value>,
}

/// BEGIN (REPEATABLE READ, READ ONLY), the ledger's bounds, the one read and
/// COMMIT: four statements, no write.
pub(crate) async fn read_database(
    pool: &PgPool,
    tenant: Uuid,
    venue: &str,
    epoch: Uuid,
) -> Result<DatabaseFacts, sqlx::Error> {
    let mut tx = pool.begin_with("BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY").await?;
    sqlx::query(crate::balance_accounting::BOUNDS).execute(&mut *tx).await?;
    let facts: Json<DatabaseFacts> = sqlx::query_scalar(include_str!("cutover_report.sql"))
        .bind(tenant)
        .bind(venue)
        .bind(epoch)
        .bind(&TERMINAL[..])
        .fetch_one(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(facts.0)
}

/// C6's read of Robson's own USD-M account through the key the Spot daemon
/// is configured with (operator question Q1): its open positions, open orders
/// and open algo orders, counted. It only reads; a refused read fails C6 and
/// names a missing permission as such.
#[async_trait]
pub(crate) trait UsdmReadObserver: Send + Sync {
    async fn open(&self) -> Result<[usize; 3], String>;
}

#[async_trait]
impl UsdmReadObserver for BinanceRestClient {
    async fn open(&self) -> Result<[usize; 3], String> {
        self.open_book_counts().await.map_err(usdm_refusal)
    }
}

/// C6's reason for a refused USD-M read: a key without the read permission
/// (`-2015`, `-2014`, HTTP 401 or 403, with or without Binance's JSON body) is
/// named as such.
pub(crate) fn usdm_refusal(error: BinanceRestError) -> String {
    let denied = match &error {
        BinanceRestError::ApiError { code, http_status, .. } => {
            matches!(code, -2015 | -2014) || matches!(http_status, 401 | 403)
        },
        // A refusal without Binance's JSON body keeps its status only in the
        // text (`RestAnswer::into_body`: "HTTP 401 Unauthorized: ...").
        BinanceRestError::RequestFailed(t) => {
            t.starts_with("HTTP 401") || t.starts_with("HTTP 403")
        },
        _ => false,
    };
    if denied {
        return format!("usdm_permission_missing: {error}");
    }
    format!("usdm_read_failed: {error}")
}

/// The venue part, each read as it ended.
#[derive(Debug, Clone)]
pub(crate) struct VenueFacts {
    /// `account` (C1's account, C4, C10, the uid).
    pub account: Result<InventoryObservation, String>,
    /// The symbol's `orderTypes` (C1).
    pub order_types: Result<Vec<String>, String>,
    /// The symbol's cost envelope, selected under 8g/8h's rules (C2).
    pub costs: Result<ExecutionCostEnvelope, String>,
    /// The symbol's rules, read fresh and parsed (C3).
    pub rules: Result<(), String>,
    /// The clock bound u of a sample of this epoch with valid evidence (C5);
    /// invalid or missing evidence is the reason there is none.
    pub clock: Result<Duration, String>,
    /// The USD-M account's [open positions, open orders, open algo orders]
    /// (C6).
    pub usdm: Result<[usize; 3], String>,
}

/// C7's process memory: the gate's open incidents, the journal's latch and
/// the suspended streams; clean is the default.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct RuntimeFacts {
    pub incidents: usize,
    pub latch: bool,
    pub suspended: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct Verdict {
    pub pass: bool,
    pub detail: String,
}

/// The decision values: what the certificate certifies and every value a
/// check compares.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct Decision {
    pub uid: Option<String>,
    pub month: NaiveDate,
    pub quote_total: Option<Decimal>,
    pub previous: Option<Decimal>,
    pub basis: Option<Decimal>,
    pub values: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct Report {
    pub checks: BTreeMap<&'static str, Verdict>,
    pub decision: Decision,
    pub hash: String,
    pub as_of: DateTime<Utc>,
}

impl Report {
    pub(crate) fn passed(&self) -> bool {
        self.checks.values().all(|verdict| verdict.pass)
    }
}

/// Every check over its evidence, for `symbol` (BTCUSDT): pure.
pub(crate) fn evaluate(
    db: &DatabaseFacts,
    venue: &VenueFacts,
    runtime: RuntimeFacts,
    symbol: &Symbol,
    as_of: DateTime<Utc>,
) -> Report {
    let mut checks = BTreeMap::new();
    let mut values = db.values.clone();
    let mut check = |id: &'static str, outcome: Result<bool, &String>, why: &str| {
        let (pass, detail) = match outcome {
            Ok(pass) => (pass, if pass { "ok" } else { why }.to_string()),
            Err(error) => (false, error.clone()),
        };
        checks.insert(id, Verdict { pass, detail });
    };
    let account = venue.account.as_ref();
    let total = |asset: &str| {
        account.ok().map(|a| {
            let held = a.holdings.iter().filter(|(name, _)| name == asset);
            held.fold((Decimal::ZERO, Decimal::ZERO), |(f, l), (_, h)| (f + h.free, l + h.locked))
        })
    };
    let c1 = account.and_then(|a| {
        venue.order_types.as_ref().map(|types| {
            a.account_type == "SPOT"
                && a.can_trade
                && a.permissions.iter().any(|p| p == "SPOT")
                && ["MARKET", "STOP_LOSS"].iter().all(|t| types.iter().any(|x| x == t))
        })
    });
    check("c1", c1, "not a SPOT account that can trade, or MARKET or STOP_LOSS unlisted");
    let c2 = venue.costs.as_ref().map(|c| c.fee_asset_policy == FeeAssetPolicy::Base);
    check("c2", c2, "commission not charged in the base asset");
    check("c3", venue.rules.as_ref().map(|_| true), "");
    // C4 (SD7): BTC neither held nor locked (a foreign order may lock it), no
    // open order on the symbol; the account reports order lists by count only.
    // Its values: [[BTC free, BTC locked], symbol orders, order lists].
    let c4 = account.map(|a| {
        let orders = a.open_orders.iter().filter(|o| o.symbol == *symbol).count();
        let (free, locked) = total(symbol.base()).unwrap_or_default();
        let held = [free.to_string(), locked.to_string()];
        values.insert("c4".into(), json!([held, orders, a.open_order_lists]));
        free.is_zero() && locked.is_zero() && orders == 0 && a.open_order_lists == 0
    });
    check("c4", c4, "BTC held or locked, an open BTCUSDT order, or an open order list");
    let c5 = venue.clock.as_ref().map(|u| *u <= Duration::from_secs(1));
    check("c5", c5, "clock bound u above 1 s");
    let c6 = venue.usdm.as_ref().map(|open| {
        values.insert("c6_usdm".into(), json!(open));
        *open == [0; 3]
    });
    check("c6", c6, "a USD-M position, open order or open algo order");
    values.insert("c7_memory".into(), json!([runtime.incidents, runtime.latch, runtime.suspended]));
    for (id, why) in DATABASE_CHECKS {
        let mut pass = db.checks.get(id).copied().unwrap_or(false);
        let mut why = why.to_string();
        if id == "c7" {
            pass &= runtime == RuntimeFacts::default();
            if let Some(reasons) = &db.incident_reasons {
                why = format!("{why}; open incidents: {}", reasons.join(", "));
            }
        }
        check(id, Ok(pass), &why);
    }
    // C10: the basis lands in monthly_state.capital_base (NUMERIC(20,8))
    // unrounded; the quote keeps the ledger's precision.
    let quote_total = total(symbol.quote()).map(|(free, locked)| free + locked);
    let basis = quote_total.zip(db.previous).map(|(q, previous)| q.min(previous));
    let exact = |b: Decimal| b.normalize().scale() <= 8 && b.abs() < Decimal::from(10_i64.pow(12));
    let c10 = account.map(|_| basis.is_some_and(exact));
    check("c10", c10, "no previous basis, or a basis not exact at NUMERIC(20,8)");
    let decision = Decision {
        uid: account.ok().map(|a| a.uid.clone()),
        month: db.month,
        quote_total,
        previous: db.previous,
        basis,
        values,
    };
    let verdicts: BTreeMap<&str, bool> = checks.iter().map(|(id, v)| (*id, v.pass)).collect();
    let canonical = serde_json::to_vec(&(verdicts, &decision)).expect("a report serializes");
    let hash = format!("{:x}", Sha256::digest(canonical));
    Report { checks, decision, hash, as_of }
}

#[cfg(test)]
#[path = "cutover_report_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "cutover_report_pg_tests.rs"]
mod pg_tests;
