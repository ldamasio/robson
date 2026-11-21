//! The 8k-2 pre statement: one bounded autocommit read of the ledger rows the
//! base check needs. Rows are decoded one by one into native Decimal; no SQL
//! aggregate of an amount is decoded. 8k-3a adds the non-trading scope's
//! pre and post statement under the same connection owner, [`fetch`].
use chrono::{DateTime, Utc};
use robson_exec::venue::VenueId;
use rust_decimal::Decimal;
use sqlx::{
    postgres::{PgArguments, PgConnectOptions, PgRow},
    query::Query,
    ConnectOptions, Connection, FromRow, Postgres, Row,
};
use tokio::time::{timeout_at, Instant};
use uuid::Uuid;

use super::{DetectorScope, ScanScope};

/// Ends `LIMIT 65` (`PRE_ROW_CAP + 1`): the server returns at most 65 rows.
const PRE: &str = include_str!("btc_partition.sql");
const NON_TRADING: &str = include_str!("non_trading.sql");

/// Explicit cap on returned rows, applied before any decode. A fixture bound
/// for the inert pilot detector, not production policy.
pub(crate) const PRE_ROW_CAP: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PreRefusal {
    /// No row: scope, UID, generation or epoch is not current.
    Scope,
    Cap,
    Malformed,
    /// The non-trading scope's Spot ledger holds a row.
    LedgerNotEmpty,
    Timeout,
    Sql,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct PreSnapshot {
    pub(crate) version: i64,
    pub(crate) closed_legs: i64,
    pub(crate) legs: Vec<LegRow>,
    pub(crate) orders: Vec<OrderRow>,
    pub(crate) intents: Vec<IntentRow>,
}

/// One open leg of the operated pair. The four partition members are
/// recorded together or not at all (M36), never zero-filled here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LegRow {
    pub(crate) position_id: Uuid,
    pub(crate) admission_intent_id: Uuid,
    pub(crate) gross: Option<Decimal>,
    pub(crate) base_fee: Option<Decimal>,
    pub(crate) base_fee_final: bool,
    pub(crate) executable: Option<Decimal>,
    pub(crate) consumed: Decimal,
    pub(crate) recovery_state: Option<String>,
    pub(crate) entry_orders: i64,
    /// Every entry order of the leg names its current admission intent.
    pub(crate) entry_linked: bool,
    /// The current admission's requested quantity; absent if superseded.
    pub(crate) requested: Option<Decimal>,
}

/// A Robson stop or sell of an open leg whose phase is live or unknown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OrderRow {
    pub(crate) position_id: Uuid,
    pub(crate) exchange_order_id: String,
    pub(crate) kind: String,
    pub(crate) phase: String,
    pub(crate) orig_qty: Decimal,
    pub(crate) executed: Decimal,
    pub(crate) intent_id: Option<Uuid>,
}

/// The order row an intent names through `venue_orders.intent_id`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LinkedOrder {
    pub(crate) owner: String,
    pub(crate) kind: Option<String>,
    pub(crate) position_id: Option<Uuid>,
    pub(crate) tenant_id: Uuid,
    pub(crate) venue_id: String,
    pub(crate) account_scope: String,
    pub(crate) client_order_id: String,
    pub(crate) phase: String,
    pub(crate) executed: Decimal,
    pub(crate) exchange_order_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IntentRow {
    pub(crate) intent_id: Uuid,
    pub(crate) kind: String,
    pub(crate) position_id: Option<Uuid>,
    pub(crate) not_sent: bool,
    pub(crate) venue_outcome: Option<String>,
    pub(crate) disposition_kind: Option<String>,
    pub(crate) uncertain_at: Option<DateTime<Utc>>,
    pub(crate) client_order_id: String,
    /// The open leg whose current admission this entry intent is.
    pub(crate) admission_of: Option<Uuid>,
    /// For a cancel only: its exact target is proven terminal.
    pub(crate) target_terminal_proof: Option<bool>,
    pub(crate) linked: Option<LinkedOrder>,
}

/// One statement on a fresh connection under the caller's deadline; the
/// connection is closed before returning, so nothing is held across HTTP
/// ([`fetch`], its owner since 8k-3a).
pub(crate) async fn read_pre(
    options: &PgConnectOptions,
    scope: DetectorScope<'_>,
    deadline: Instant,
) -> Result<PreSnapshot, PreRefusal> {
    if !scope.expected.is_valid() || scope.uid.is_empty() || scope.operated.is_empty() {
        return Err(PreRefusal::Scope);
    }
    let e = scope.expected;
    let query = sqlx::query(PRE)
        .bind(e.tenant_id)
        .bind(e.venue.as_str())
        .bind(e.account_scope)
        .bind(e.generation_id)
        .bind(e.process_epoch)
        .bind(e.source_audit_id)
        .bind(scope.uid)
        .bind(scope.operated);
    let rows = fetch(options, query, deadline).await?;
    if rows.is_empty() {
        return Err(PreRefusal::Scope);
    }
    decode(&rows)
}

/// The one connection owner of the pre and post statements: one statement
/// on a fresh connection under the caller's deadline; the connection is
/// closed before returning, so nothing is held across HTTP. The statements
/// return at most `PRE_ROW_CAP + 1` rows; more than the cap is refused here,
/// before any row is decoded.
async fn fetch(
    options: &PgConnectOptions,
    query: Query<'_, Postgres, PgArguments>,
    deadline: Instant,
) -> Result<Vec<PgRow>, PreRefusal> {
    let rows = timeout_at(deadline, async {
        let mut connection = options.connect().await?;
        let rows = query.fetch_all(&mut connection).await?;
        connection.close().await?;
        Ok::<_, sqlx::Error>(rows)
    })
    .await
    .map_err(|_| PreRefusal::Timeout)?
    .map_err(|_| PreRefusal::Sql)?;
    if rows.len() > PRE_ROW_CAP {
        return Err(PreRefusal::Cap);
    }
    Ok(rows)
}

/// The non-trading scope's pre (`expected` None) and post (`Some(v₀)`)
/// statement (PLAN-r2 §6.1): the version when the scope is bound to its UID,
/// the epoch is the tenant's live one and the scope's Spot ledger is empty.
/// A malformed scope is refused before any connection: another venue, a nil
/// tenant or process epoch, a blank account scope, UID or operated pair
/// (its generation and source-audit ids are nil by design and not read).
pub(super) async fn read_non_trading(
    options: &PgConnectOptions,
    scope: &ScanScope<'_>,
    expected: Option<i64>,
    deadline: Instant,
) -> Result<i64, PreRefusal> {
    let (d, e) = (scope.detector, scope.detector.expected);
    let blank = [e.account_scope.trim(), d.uid, d.operated].contains(&"");
    let nil = e.tenant_id.is_nil() || e.process_epoch.is_nil();
    if nil || blank || e.venue.as_str() != VenueId::BINANCE_SPOT {
        return Err(PreRefusal::Scope);
    }
    let query = sqlx::query(NON_TRADING)
        .bind(e.tenant_id)
        .bind(e.venue.as_str())
        .bind(e.account_scope)
        .bind(e.process_epoch)
        .bind(d.uid)
        .bind(expected);
    let rows = fetch(options, query, deadline).await?;
    let row = rows.first().ok_or(PreRefusal::Scope)?;
    // version, uid_bound, epoch_live, ledger_empty.
    match <(i64, bool, bool, bool)>::from_row(row).map_err(|_| PreRefusal::Malformed)? {
        (version, true, true, true) => Ok(version),
        (_, true, true, false) => Err(PreRefusal::LedgerNotEmpty),
        _ => Err(PreRefusal::Scope),
    }
}

pub(super) fn decode(rows: &[PgRow]) -> Result<PreSnapshot, PreRefusal> {
    let bad = |_| PreRefusal::Malformed;
    let (header, rest) = rows.split_first().ok_or(PreRefusal::Scope)?;
    if header.try_get::<&str, _>("kind").map_err(bad)? != "header" {
        return Err(PreRefusal::Malformed);
    }
    let version: i64 = header.try_get("version").map_err(bad)?;
    let mut snapshot = PreSnapshot {
        version,
        closed_legs: header.try_get("closed_legs").map_err(bad)?,
        legs: Vec::new(),
        orders: Vec::new(),
        intents: Vec::new(),
    };
    for row in rest {
        if row.try_get::<i64, _>("version").map_err(bad)? != version {
            return Err(PreRefusal::Malformed);
        }
        match row.try_get::<&str, _>("kind").map_err(bad)? {
            "leg" => snapshot.legs.push(LegRow {
                position_id: row.try_get("position_id").map_err(bad)?,
                admission_intent_id: row.try_get("admission_intent_id").map_err(bad)?,
                gross: row.try_get("gross").map_err(bad)?,
                base_fee: row.try_get("base_fee").map_err(bad)?,
                base_fee_final: row.try_get("base_final").map_err(bad)?,
                executable: row.try_get("executable").map_err(bad)?,
                consumed: row.try_get("consumed").map_err(bad)?,
                recovery_state: row.try_get("recovery_state").map_err(bad)?,
                entry_orders: row.try_get("entry_orders").map_err(bad)?,
                entry_linked: row.try_get("entry_linked").map_err(bad)?,
                requested: row.try_get("requested").map_err(bad)?,
            }),
            "order" => snapshot.orders.push(OrderRow {
                position_id: row.try_get("position_id").map_err(bad)?,
                exchange_order_id: row.try_get("order_id").map_err(bad)?,
                kind: row.try_get("order_kind").map_err(bad)?,
                phase: row.try_get("order_phase").map_err(bad)?,
                orig_qty: row.try_get("orig_qty").map_err(bad)?,
                executed: row.try_get("executed").map_err(bad)?,
                intent_id: row.try_get("intent_id").map_err(bad)?,
            }),
            "intent" => snapshot.intents.push(intent(row)?),
            _ => return Err(PreRefusal::Malformed),
        }
    }
    Ok(snapshot)
}

fn intent(row: &PgRow) -> Result<IntentRow, PreRefusal> {
    let bad = |_| PreRefusal::Malformed;
    let owner: Option<String> = row.try_get("o_owner").map_err(bad)?;
    let linked = match owner {
        None => None,
        Some(owner) => Some(LinkedOrder {
            owner,
            kind: row.try_get("o_kind").map_err(bad)?,
            position_id: row.try_get("o_position").map_err(bad)?,
            tenant_id: row.try_get("o_tenant").map_err(bad)?,
            venue_id: row.try_get("o_venue").map_err(bad)?,
            account_scope: row.try_get("o_scope").map_err(bad)?,
            client_order_id: row.try_get("o_client").map_err(bad)?,
            phase: row.try_get("o_phase").map_err(bad)?,
            executed: row.try_get("o_executed").map_err(bad)?,
            exchange_order_id: row.try_get("o_order_id").map_err(bad)?,
        }),
    };
    Ok(IntentRow {
        intent_id: row.try_get("intent_id").map_err(bad)?,
        kind: row.try_get("intent_kind").map_err(bad)?,
        position_id: row.try_get("position_id").map_err(bad)?,
        not_sent: row.try_get("not_sent").map_err(bad)?,
        venue_outcome: row.try_get("venue_outcome").map_err(bad)?,
        disposition_kind: row.try_get("disposition_kind").map_err(bad)?,
        uncertain_at: row.try_get("uncertain_at").map_err(bad)?,
        client_order_id: row.try_get("client_order_id").map_err(bad)?,
        admission_of: row.try_get("admission_of").map_err(bad)?,
        target_terminal_proof: row.try_get("target_terminal_proof").map_err(bad)?,
        linked,
    })
}
