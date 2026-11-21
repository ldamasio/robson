//! One owned, read-only REPEATABLE READ snapshot over the four cold Spot
//! source families. The result is local source evidence only: it confers no
//! readiness, deployment barrier, restart permission, generation publication
//! or capital action. External barrier and restore provenance stay separate.
use sqlx::{Connection, PgPool, Row};
use tokio::time::{timeout_at, Instant};
use uuid::Uuid;

use super::{
    cold_owner::{read_anchor, AnchorDefect, EdgeDefect, ObservedAnchor, OwnerRequest},
    cold_source::{
        read_attempt_page, read_intent_page, read_order_page, read_route_page, AttemptSource, Edge,
        IntentSource, OrderSource, RouteClass, RouteDefect, RouteSource, PAGE_LIMIT,
    },
};
use crate::ledger_connection::LedgerConnection;

const CATALOG: &str = include_str!("cold_audit_catalog.sql");
const ACCOUNT: &str = "SELECT tenant_id,uid FROM public.venue_accounts
    WHERE (venue_id,account_scope)=('binance:spot',$1)";
// Local to the snapshot transaction; the measured 40ms page bound.
const LIMITS: &str = "SELECT set_config('lock_timeout','20ms',true),
    set_config('statement_timeout','40ms',true),
    set_config('idle_in_transaction_session_timeout','1s',true)";

/// Exact account and tenant the snapshot must prove.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ColdAuditScope<'a> {
    pub(crate) tenant_id: Uuid,
    pub(crate) account_scope: &'a str,
}

/// Approved finite physical-page budget per family. A family of `n` rows
/// needs `n / 8 + 1` pages: its terminal is always a later short page.
#[derive(Clone, Copy, Debug)]
pub(crate) struct PageBudget {
    pub(crate) intents: u32,
    pub(crate) attempts: u32,
    pub(crate) orders: u32,
    pub(crate) routes: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Family {
    Intents,
    Attempts,
    Orders,
    Routes,
}

/// A physical source key, taken only from a row a page actually returned.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum SourceKey {
    ClientOrder(String),
    Order {
        symbol: String,
        exchange_order_id: String,
    },
    Route(Uuid),
}

/// Bounded typed facts streamed one page at a time. Facts are provisional:
/// only a returned receipt says the traversal finished; any refusal voids
/// everything delivered in the run. A sink refusal is its own bound.
pub(crate) trait ColdSourceSink {
    fn intents(&mut self, rows: &[IntentSource]) -> Result<(), SinkFull>;
    fn attempts(&mut self, rows: &[AttemptSource]) -> Result<(), SinkFull>;
    fn orders(&mut self, rows: &[OrderSource]) -> Result<(), SinkFull>;
    fn routes(&mut self, rows: &[RouteSource]) -> Result<(), SinkFull>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SinkFull;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Stage {
    Acquire,
    Setup,
    Account,
    Catalog,
    Page(Family),
    Commit,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum RowDefect {
    ForeignOwner,
    /// An attempt with no intent at its exact Spot account key.
    MissingParent,
    Route(RouteDefect),
    /// A raw text exceeded `TEXT_LIMIT` bytes. The reported key is a clipped
    /// prefix for diagnosis only; it never became a cursor.
    OverWidth,
    /// The other side of a native J/order edge is in another tenant, venue
    /// or account, whether or not it lies in a scanned page.
    ForeignEdge,
}

#[derive(Debug)]
pub(crate) enum ColdAuditRefusal {
    /// Blank scope, nil tenant, zero page budget or an already passed deadline.
    InvalidRequest,
    Deadline(Stage),
    Database(Stage, sqlx::Error),
    AccountMissing,
    AccountTenant,
    AccountUid,
    /// The owner's anchor differs from its exact expectation.
    Anchor(AnchorDefect),
    /// The owner's two edge directions disagree within the captured cut.
    Unproven(EdgeDefect),
    Catalog(String),
    Row {
        family: Family,
        reason: RowDefect,
        key: SourceKey,
        last_verified: Option<SourceKey>,
    },
    PageBudget {
        family: Family,
        last_key: Option<SourceKey>,
    },
    Sink {
        family: Family,
        last_key: Option<SourceKey>,
    },
}

/// One family's terminal: `pages` statements, the last a short page, and the
/// final returned key. Counts alone never prove a full reconstruction.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct FamilyTerminal {
    pub(crate) family: Family,
    pub(crate) rows: u64,
    pub(crate) pages: u32,
    pub(crate) last_key: Option<SourceKey>,
}

/// Local source evidence from one committed read-only snapshot. Only this
/// module constructs it; it carries no readiness or product privilege.
#[derive(Debug, Eq, PartialEq)]
pub(crate) struct ColdSourceReceipt {
    pub(crate) tenant_id: Uuid,
    pub(crate) account_scope: String,
    pub(crate) families: [FamilyTerminal; 4],
    /// Exactly one transaction; BEGIN, SET TRANSACTION, limits, account,
    /// catalog, every page and COMMIT.
    pub(crate) statements: u64,
    _sealed: (),
}

/// Traverse the four Spot families inside one owned snapshot before
/// `deadline`. Timeout or caller cancellation drops the original guard
/// unfinished, which detaches and closes the backend: no queued cleanup and
/// no pool reuse.
pub(crate) async fn audit_cold_sources(
    pool: &PgPool,
    scope: ColdAuditScope<'_>,
    budget: PageBudget,
    deadline: Instant,
    sink: &mut impl ColdSourceSink,
) -> Result<ColdSourceReceipt, ColdAuditRefusal> {
    audit(pool, scope, budget, deadline, sink, None)
        .await
        .map(|(receipt, _)| receipt)
}

/// The one owned traversal. `owner` replaces the account read with the
/// owner's anchor read and adds edge refusals; statements stay the same.
pub(super) async fn audit(
    pool: &PgPool,
    scope: ColdAuditScope<'_>,
    budget: PageBudget,
    deadline: Instant,
    sink: &mut impl ColdSourceSink,
    owner: Option<&OwnerRequest<'_>>,
) -> Result<(ColdSourceReceipt, Option<ObservedAnchor>), ColdAuditRefusal> {
    if scope.tenant_id.is_nil()
        || scope.account_scope.trim().is_empty()
        || [
            budget.intents,
            budget.attempts,
            budget.orders,
            budget.routes,
        ]
        .contains(&0)
        || deadline <= Instant::now()
    {
        return Err(ColdAuditRefusal::InvalidRequest);
    }
    let mut stage = Stage::Acquire;
    match timeout_at(deadline, traverse(pool, scope, budget, sink, &mut stage, owner)).await {
        Ok(result) => result,
        Err(_) => Err(ColdAuditRefusal::Deadline(stage)),
    }
}

async fn traverse(
    pool: &PgPool,
    scope: ColdAuditScope<'_>,
    budget: PageBudget,
    sink: &mut impl ColdSourceSink,
    stage: &mut Stage,
    owner: Option<&OwnerRequest<'_>>,
) -> Result<(ColdSourceReceipt, Option<ObservedAnchor>), ColdAuditRefusal> {
    #[cfg(test)]
    tests::pause_at(tests::Boundary::BeforeAcquire).await;

    let connection = pool.acquire().await.map_err(db(Stage::Acquire))?;
    let mut owned = LedgerConnection::new(connection);
    *stage = Stage::Setup;
    let mut tx = owned.connection().begin().await.map_err(db(Stage::Setup))?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        .execute(&mut *tx)
        .await
        .map_err(db(Stage::Setup))?;
    sqlx::query(LIMITS).execute(&mut *tx).await.map_err(db(Stage::Setup))?;
    let mut statements = 3;

    // The first data read of the snapshot: the account alone, or the account
    // and the owner's anchor together.
    *stage = Stage::Account;
    let anchor = match owner {
        None => {
            let account = sqlx::query(ACCOUNT)
                .bind(scope.account_scope)
                .fetch_optional(&mut *tx)
                .await
                .map_err(db(Stage::Account))?
                .ok_or(ColdAuditRefusal::AccountMissing)?;
            if account.get::<Uuid, _>("tenant_id") != scope.tenant_id {
                return Err(ColdAuditRefusal::AccountTenant);
            }
            if account.get::<String, _>("uid").trim().is_empty() {
                return Err(ColdAuditRefusal::AccountUid);
            }
            None
        },
        Some(request) => Some(read_anchor(&mut *tx, request).await?),
    };
    statements += 1;
    let edges = owner.is_some();

    *stage = Stage::Catalog;
    let catalog = sqlx::query(CATALOG).fetch_all(&mut *tx).await.map_err(db(Stage::Catalog))?;
    statements += 1;
    for row in &catalog {
        if !row.get::<Option<bool>, _>("ok").unwrap_or(false) {
            return Err(ColdAuditRefusal::Catalog(row.get("label")));
        }
    }
    if catalog.len() != 17 {
        return Err(ColdAuditRefusal::Catalog("catalog row count".to_string()));
    }

    let (account_scope, tenant) = (scope.account_scope, scope.tenant_id);
    // Each cursor is the last key of the previous returned page, never input.
    *stage = Stage::Page(Family::Intents);
    let mut intents = Walk::new(Family::Intents, budget.intents);
    loop {
        let after = intents.cursor().map(client_order);
        let page = read_intent_page(&mut *tx, account_scope, tenant, after)
            .await
            .map_err(db(*stage))?;
        for row in &page.rows {
            let key = SourceKey::ClientOrder(row.client_order_id.clone());
            let defect = if row.wide {
                Some(RowDefect::OverWidth)
            } else if !row.owner_matches {
                Some(RowDefect::ForeignOwner)
            } else {
                (edges && row.reverse_edge(tenant, account_scope) == Edge::Foreign)
                    .then_some(RowDefect::ForeignEdge)
            };
            intents.check(key, defect)?;
        }
        intents.deliver(sink.intents(&page.rows))?;
        if intents.page(page.rows.len())? {
            break;
        }
    }

    #[cfg(test)]
    tests::pause_at(tests::Boundary::AfterIntents).await;

    *stage = Stage::Page(Family::Attempts);
    let mut attempts = Walk::new(Family::Attempts, budget.attempts);
    loop {
        let after = attempts.cursor().map(client_order);
        let page = read_attempt_page(&mut *tx, account_scope, tenant, after)
            .await
            .map_err(db(*stage))?;
        for row in &page.rows {
            let key = SourceKey::ClientOrder(row.client_order_id.clone());
            let defect = if row.wide {
                Some(RowDefect::OverWidth)
            } else if row.parent_intent_id.is_none() {
                Some(RowDefect::MissingParent)
            } else {
                (!row.owner_matches).then_some(RowDefect::ForeignOwner)
            };
            attempts.check(key, defect)?;
        }
        attempts.deliver(sink.attempts(&page.rows))?;
        if attempts.page(page.rows.len())? {
            break;
        }
    }

    #[cfg(test)]
    tests::pause_at(tests::Boundary::AfterAttempts).await;

    *stage = Stage::Page(Family::Orders);
    let mut orders = Walk::new(Family::Orders, budget.orders);
    loop {
        let after = match orders.cursor() {
            Some(SourceKey::Order { symbol, exchange_order_id }) => {
                Some((symbol.as_str(), exchange_order_id.as_str()))
            },
            _ => None,
        };
        let page = read_order_page(&mut *tx, account_scope, tenant, after)
            .await
            .map_err(db(*stage))?;
        for row in &page.rows {
            let key = SourceKey::Order {
                symbol: row.symbol.clone(),
                exchange_order_id: row.exchange_order_id.clone(),
            };
            // Unowned and uncertain orders, and orders naming an absent J,
            // stay data for reconstruction.
            let defect = if row.wide {
                Some(RowDefect::OverWidth)
            } else if !row.owner_matches {
                Some(RowDefect::ForeignOwner)
            } else {
                (edges && row.named_edge(tenant, account_scope) == Edge::Foreign)
                    .then_some(RowDefect::ForeignEdge)
            };
            orders.check(key, defect)?;
        }
        orders.deliver(sink.orders(&page.rows))?;
        if orders.page(page.rows.len())? {
            break;
        }
    }

    #[cfg(test)]
    tests::pause_at(tests::Boundary::AfterOrders).await;

    *stage = Stage::Page(Family::Routes);
    let mut routes = Walk::new(Family::Routes, budget.routes);
    loop {
        let after = match routes.cursor() {
            Some(SourceKey::Route(position)) => Some(*position),
            _ => None,
        };
        let page = read_route_page(&mut *tx, account_scope, tenant, after)
            .await
            .map_err(db(*stage))?;
        for row in &page.rows {
            let defect = if row.wide {
                Some(RowDefect::OverWidth)
            } else {
                match row.classify() {
                    RouteClass::Diagnostic(defect) => Some(RowDefect::Route(defect)),
                    // Armed-only and closed legs remain source data.
                    RouteClass::ArmedOnly | RouteClass::Leg { .. } => None,
                }
            };
            routes.check(SourceKey::Route(row.position_id), defect)?;
        }
        routes.deliver(sink.routes(&page.rows))?;
        if routes.page(page.rows.len())? {
            break;
        }
    }

    #[cfg(test)]
    tests::pause_at(tests::Boundary::AfterRoutes).await;

    *stage = Stage::Commit;
    tx.commit().await.map_err(db(Stage::Commit))?;
    owned.finished = true;
    let walks = [intents, attempts, orders, routes];
    statements += walks.iter().map(|walk| u64::from(walk.pages)).sum::<u64>() + 1;
    let receipt = ColdSourceReceipt {
        tenant_id: tenant,
        account_scope: account_scope.to_string(),
        families: walks.map(Walk::terminal),
        statements,
        _sealed: (),
    };
    Ok((receipt, anchor))
}

fn db(stage: Stage) -> impl FnOnce(sqlx::Error) -> ColdAuditRefusal {
    move |error| ColdAuditRefusal::Database(stage, error)
}

fn client_order(key: &SourceKey) -> &str {
    match key {
        SourceKey::ClientOrder(id) => id,
        _ => unreachable!("client-order families carry only client-order keys"),
    }
}

/// Per-family traversal state. `last` advances only over verified rows.
struct Walk {
    family: Family,
    budget: u32,
    pages: u32,
    rows: u64,
    last: Option<SourceKey>,
}

impl Walk {
    fn new(family: Family, budget: u32) -> Self {
        Self {
            family,
            budget,
            pages: 0,
            rows: 0,
            last: None,
        }
    }

    fn cursor(&self) -> Option<&SourceKey> {
        self.last.as_ref()
    }

    fn check(&mut self, key: SourceKey, defect: Option<RowDefect>) -> Result<(), ColdAuditRefusal> {
        if let Some(reason) = defect {
            return Err(ColdAuditRefusal::Row {
                family: self.family,
                reason,
                key,
                last_verified: self.last.take(),
            });
        }
        self.rows += 1;
        self.last = Some(key);
        Ok(())
    }

    fn deliver(&mut self, outcome: Result<(), SinkFull>) -> Result<(), ColdAuditRefusal> {
        outcome.map_err(|SinkFull| ColdAuditRefusal::Sink {
            family: self.family,
            last_key: self.last.clone(),
        })
    }

    /// True on the terminal short page (including the empty page after a
    /// full one). A full page with the budget spent refuses.
    fn page(&mut self, len: usize) -> Result<bool, ColdAuditRefusal> {
        self.pages += 1;
        if len < PAGE_LIMIT {
            return Ok(true);
        }
        if self.pages >= self.budget {
            return Err(ColdAuditRefusal::PageBudget {
                family: self.family,
                last_key: self.last.clone(),
            });
        }
        Ok(false)
    }

    fn terminal(self) -> FamilyTerminal {
        FamilyTerminal {
            family: self.family,
            rows: self.rows,
            pages: self.pages,
            last_key: self.last,
        }
    }
}

#[cfg(test)]
#[path = "cold_audit_pg_tests.rs"]
mod tests;
