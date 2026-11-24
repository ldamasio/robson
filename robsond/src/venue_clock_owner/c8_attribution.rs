#![cfg(all(test, feature = "crash-points"))]
//! Fixture-only C8 operated-order attribution witness (8k-2 precursor).
//!
//! Typed identity facts only: no amount, cash, pending count, recovery
//! action or clean/ready classification. A refused snapshot yields nothing.

use robson_connectors::binance::spot::{SpotOpenOrderList, SpotOrderRecord};
use rust_decimal::Decimal;
use sqlx::{
    postgres::{PgConnectOptions, PgRow},
    ConnectOptions, Connection, Row,
};
use tokio::time::{timeout_at, Instant};
use uuid::Uuid;

use crate::spot_order_set_store::ExpectedGeneration;

/// Native fixture cap, applied before any SQL. Not production policy.
pub(super) const FIXTURE_KEY_CAP: usize = 16;

const SNAPSHOT: &str = include_str!("c8_attribution.sql");

#[derive(Clone, Copy)]
pub(super) struct Scope<'a> {
    pub(super) expected: ExpectedGeneration<'a>,
    pub(super) uid: &'a str,
    pub(super) version: i64,
    /// The exact raw operated pair; nothing else is parsed as an asset.
    pub(super) operated: &'a str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Refusal {
    /// Scope, UID, generation, epoch, audit or version moved or absent.
    Scope,
    /// More operated keys than the fixture cap; no SQL issued.
    Cap,
    /// An operated list without its own client ID.
    ListIdentity,
    /// The returned batch did not echo the exact requested keys.
    Malformed,
    Timeout,
    Sql,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Divergence {
    Tenant,
    Client,
    Side,
    Phase,
    /// A `leg` row whose J is missing, has contradictory delivery evidence, or
    /// disagrees on tenant/venue/scope/client/position/kind, or whose
    /// ledger type disagrees with the wire.
    Journal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Fact {
    /// No durable row under the exact unique key.
    Absent,
    /// Durable, exact identity, recorded as discovered: never authorship.
    Discovered,
    Unauthored,
    /// Exact ledger + J identity under the bound tenant and scope.
    Ledger {
        intent_id: Uuid,
    },
    Contradiction(Divergence),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct OrderFact {
    pub(super) symbol: String,
    pub(super) order_id: u64,
    pub(super) client_order_id: String,
    pub(super) side: String,
    pub(super) order_list_id: i64,
    pub(super) fact: Fact,
    pub(super) durable_executed: Option<Decimal>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ListFact {
    pub(super) order_list_id: i64,
    pub(super) list_client_order_id: String,
    pub(super) members: Vec<(u64, String)>,
}

/// Operated subset of one typed triple; unrelated rows are only counted.
pub(super) struct Operated<'a> {
    pub(super) orders: Vec<&'a SpotOrderRecord>,
    pub(super) lists: Vec<ListFact>,
    pub(super) unrelated_orders: usize,
    pub(super) unrelated_lists: usize,
}

pub(super) fn select<'a>(
    scope: Scope<'_>,
    orders: &'a [SpotOrderRecord],
    lists: &[SpotOpenOrderList],
) -> Result<Operated<'a>, Refusal> {
    let mine: Vec<_> = orders.iter().filter(|o| o.symbol == scope.operated).collect();
    if mine.len() > FIXTURE_KEY_CAP {
        return Err(Refusal::Cap);
    }
    let mut operated = Vec::new();
    for list in lists.iter().filter(|l| l.symbol == scope.operated) {
        let client = list.list_client_order_id.clone().ok_or(Refusal::ListIdentity)?;
        operated.push(ListFact {
            order_list_id: list.order_list_id,
            list_client_order_id: client,
            members: list.orders.iter().map(|m| (m.order_id, m.client_order_id.clone())).collect(),
        });
    }
    Ok(Operated {
        unrelated_orders: orders.len() - mine.len(),
        unrelated_lists: lists.len() - operated.len(),
        orders: mine,
        lists: operated,
    })
}

/// One statement on a fresh read-only connection with server and client
/// deadlines.
pub(super) async fn snapshot(
    options: &PgConnectOptions,
    scope: Scope<'_>,
    orders: &[&SpotOrderRecord],
    deadline: Instant,
) -> Result<Vec<PgRow>, Refusal> {
    if orders.len() > FIXTURE_KEY_CAP {
        return Err(Refusal::Cap);
    }
    if !scope.expected.is_valid() || scope.uid.is_empty() {
        return Err(Refusal::Scope);
    }
    let symbols: Vec<&str> = orders.iter().map(|o| o.symbol.as_str()).collect();
    let ids: Vec<String> = orders.iter().map(|o| o.order_id.to_string()).collect();
    let e = scope.expected;
    let rows = timeout_at(deadline, async {
        let mut connection = options.connect().await?;
        let rows = sqlx::query(SNAPSHOT)
            .bind(e.tenant_id)
            .bind(e.venue.as_str())
            .bind(e.account_scope)
            .bind(e.generation_id)
            .bind(e.process_epoch)
            .bind(e.source_audit_id)
            .bind(scope.uid)
            .bind(scope.version)
            .bind(&symbols)
            .bind(&ids)
            .fetch_all(&mut connection)
            .await?;
        connection.close().await?;
        Ok::<_, sqlx::Error>(rows)
    })
    .await
    .map_err(|_| Refusal::Timeout)?
    .map_err(|_| Refusal::Sql)?;
    if rows.is_empty() {
        return Err(Refusal::Scope);
    }
    if rows.len() != orders.len().max(1) {
        return Err(Refusal::Malformed);
    }
    for row in &rows {
        if row.try_get::<i64, _>("accounting_version").map_err(|_| Refusal::Malformed)?
            != scope.version
        {
            return Err(Refusal::Scope);
        }
    }
    Ok(rows)
}

/// Judges each exact key after the unique probe: tenant, client and side
/// separately, then J scope, so a contradiction never reads as absence.
pub(super) fn attribute(
    scope: Scope<'_>,
    orders: &[&SpotOrderRecord],
    rows: &[PgRow],
) -> Result<Vec<OrderFact>, Refusal> {
    let bad = |_| Refusal::Malformed;
    if orders.is_empty() {
        return match rows {
            [row] if row.try_get::<Option<i64>, _>("ord").map_err(bad)?.is_none() => Ok(Vec::new()),
            _ => Err(Refusal::Malformed),
        };
    }
    if rows.len() != orders.len() {
        return Err(Refusal::Malformed);
    }
    let e = scope.expected;
    let mut facts = Vec::with_capacity(orders.len());
    for (i, (order, row)) in orders.iter().zip(rows).enumerate() {
        let ord: Option<i64> = row.try_get("ord").map_err(bad)?;
        let symbol: Option<String> = row.try_get("symbol").map_err(bad)?;
        let id: Option<String> = row.try_get("order_id").map_err(bad)?;
        if ord != Some(i as i64 + 1)
            || symbol.as_deref() != Some(order.symbol.as_str())
            || id != Some(order.order_id.to_string())
        {
            return Err(Refusal::Malformed);
        }
        let tenant: Option<Uuid> = row.try_get("o_tenant").map_err(bad)?;
        let client: Option<String> = row.try_get("o_client").map_err(bad)?;
        let side: Option<String> = row.try_get("o_side").map_err(bad)?;
        let owner: Option<String> = row.try_get("o_owner").map_err(bad)?;
        let phase: Option<String> = row.try_get("o_phase").map_err(bad)?;
        let fact = match (
            tenant,
            client.as_deref(),
            side.as_deref(),
            phase.as_deref(),
            owner.as_deref(),
        ) {
            (None, None, None, None, None) => Fact::Absent,
            (Some(t), _, _, _, _) if t != e.tenant_id => Fact::Contradiction(Divergence::Tenant),
            (Some(_), Some(c), _, _, _) if c != order.client_order_id => {
                Fact::Contradiction(Divergence::Client)
            },
            (Some(_), Some(_), Some(s), _, _) if s != order.side => {
                Fact::Contradiction(Divergence::Side)
            },
            (Some(_), Some(_), Some(_), Some(phase), _) if phase != "live" => {
                Fact::Contradiction(Divergence::Phase)
            },
            (Some(_), Some(_), Some(_), Some("live"), Some(owner)) => match owner {
                "discovered" => Fact::Discovered,
                "unauthored" => Fact::Unauthored,
                "leg" => journal(row, scope, order)?,
                _ => return Err(Refusal::Malformed),
            },
            _ => return Err(Refusal::Malformed),
        };
        facts.push(OrderFact {
            symbol: order.symbol.clone(),
            order_id: order.order_id,
            client_order_id: order.client_order_id.clone(),
            side: order.side.clone(),
            order_list_id: order.order_list_id,
            fact,
            durable_executed: row.try_get("o_executed").map_err(bad)?,
        });
    }
    Ok(facts)
}

fn journal(row: &PgRow, scope: Scope<'_>, order: &SpotOrderRecord) -> Result<Fact, Refusal> {
    let bad = |_| Refusal::Malformed;
    let linked: Option<Uuid> = row.try_get("o_intent").map_err(bad)?;
    let intent: Option<Uuid> = row.try_get("j_intent").map_err(bad)?;
    let tenant: Option<Uuid> = row.try_get("j_tenant").map_err(bad)?;
    let venue: Option<String> = row.try_get("j_venue").map_err(bad)?;
    let account: Option<String> = row.try_get("j_scope").map_err(bad)?;
    let client: Option<String> = row.try_get("j_client").map_err(bad)?;
    let o_position: Option<Uuid> = row.try_get("o_position").map_err(bad)?;
    let j_position: Option<Uuid> = row.try_get("j_position").map_err(bad)?;
    let o_kind: Option<String> = row.try_get("o_kind").map_err(bad)?;
    let j_kind: Option<String> = row.try_get("j_kind").map_err(bad)?;
    let o_type: Option<String> = row.try_get("o_type").map_err(bad)?;
    let not_sent: Option<bool> = row.try_get("j_not_sent").map_err(bad)?;
    let outcome: Option<String> = row.try_get("j_outcome").map_err(bad)?;
    // J kind against the ledger leg kind; the ledger type against the wire.
    let kinds = matches!(
        (j_kind.as_deref(), o_kind.as_deref()),
        (Some("entry"), Some("entry"))
            | (Some("protection"), Some("stop"))
            | (Some("exit"), Some("sell"))
    );
    let e = scope.expected;
    Ok(match (linked, intent) {
        (Some(linked), Some(intent))
            if linked == intent
                && tenant == Some(e.tenant_id)
                && venue.as_deref() == Some(e.venue.as_str())
                && account.as_deref() == Some(e.account_scope)
                && client.as_deref() == Some(order.client_order_id.as_str())
                && o_position.is_some()
                && o_position == j_position
                && kinds
                && o_type.as_deref() == Some(order.order_type.as_str())
                && not_sent == Some(false)
                && matches!(outcome.as_deref(), None | Some("success")) =>
        {
            Fact::Ledger { intent_id: intent }
        },
        _ => Fact::Contradiction(Divergence::Journal),
    })
}

/// The 8k-2 detector's operated listing from this witness's subset and keyed
/// facts. Any contradiction stays a contradiction; nothing is dropped.
pub(super) fn listing(
    operated: Operated<'_>,
    facts: Vec<OrderFact>,
) -> crate::balance_reconciliation::Listing {
    use crate::balance_reconciliation::{Attribution, Listing, OperatedList, OperatedOrder};
    Listing {
        orders: facts
            .into_iter()
            .map(|fact| OperatedOrder {
                symbol: fact.symbol,
                order_id: fact.order_id,
                attribution: match fact.fact {
                    Fact::Absent => Attribution::Absent,
                    Fact::Discovered => Attribution::Discovered,
                    Fact::Unauthored => Attribution::Unauthored,
                    Fact::Ledger { intent_id } => Attribution::Ledger { intent_id },
                    Fact::Contradiction(_) => Attribution::Contradiction,
                },
            })
            .collect(),
        lists: operated
            .lists
            .iter()
            .map(|list| OperatedList { order_list_id: list.order_list_id })
            .collect(),
        unrelated_orders: operated.unrelated_orders,
        unrelated_lists: operated.unrelated_lists,
    }
}
