//! Bounded native source pages for a cold Spot generation.
//! One indexed statement per page on a caller-owned connection and snapshot.
//! The expected tenant is compared after LIMIT, so a foreign row stays visible.
use chrono::{DateTime, Utc};
use sqlx::PgConnection;
use uuid::Uuid;

pub(crate) const PAGE_LIMIT: usize = 8;

// $1 account_scope, $2 expected tenant; continuation $3 last client_order_id.
pub(crate) const INTENTS_FIRST: &str = "SELECT b.client_order_id,b.intent_id,b.tenant_id,b.tenant_id=$2 AS owner_matches,
       b.position_id,b.kind,b.venue_outcome,b.not_sent,b.venue_evidence_present,b.disposition_kind,b.created_at
    FROM (SELECT client_order_id,intent_id,tenant_id,position_id,kind,venue_outcome,not_sent,
                 venue_evidence IS NOT NULL AS venue_evidence_present,disposition_kind,created_at
            FROM public.order_intents
           WHERE venue_id='binance:spot' AND account_scope=$1
           ORDER BY client_order_id LIMIT 8) b
    ORDER BY b.client_order_id";
pub(crate) const INTENTS_NEXT: &str = "SELECT b.client_order_id,b.intent_id,b.tenant_id,b.tenant_id=$2 AS owner_matches,
       b.position_id,b.kind,b.venue_outcome,b.not_sent,b.venue_evidence_present,b.disposition_kind,b.created_at
    FROM (SELECT client_order_id,intent_id,tenant_id,position_id,kind,venue_outcome,not_sent,
                 venue_evidence IS NOT NULL AS venue_evidence_present,disposition_kind,created_at
            FROM public.order_intents
           WHERE venue_id='binance:spot' AND account_scope=$1 AND client_order_id>$3
           ORDER BY client_order_id LIMIT 8) b
    ORDER BY b.client_order_id";

// $1 account_scope, $2 expected tenant; continuation $3 last client_order_id.
// The literal venue matches the Spot partial unique index.
pub(crate) const ATTEMPTS_FIRST: &str = "WITH a AS MATERIALIZED (
        SELECT attempt_id,client_order_id,state,created_at,rejected_at
          FROM public.submit_attempts
         WHERE venue_id='binance:spot' AND account_scope=$1
         ORDER BY client_order_id LIMIT 8)
    SELECT a.client_order_id,a.attempt_id,a.state,a.created_at,a.rejected_at,
           i.intent_id AS parent_intent_id,i.tenant_id AS parent_tenant_id,
           COALESCE(i.tenant_id=$2,FALSE) AS owner_matches
      FROM a LEFT JOIN LATERAL (
           SELECT intent_id,tenant_id FROM public.order_intents i
            WHERE (i.venue_id,i.account_scope,i.client_order_id)=('binance:spot',$1,a.client_order_id)
            LIMIT 1) i ON true
     ORDER BY a.client_order_id";
pub(crate) const ATTEMPTS_NEXT: &str = "WITH a AS MATERIALIZED (
        SELECT attempt_id,client_order_id,state,created_at,rejected_at
          FROM public.submit_attempts
         WHERE venue_id='binance:spot' AND account_scope=$1 AND client_order_id>$3
         ORDER BY client_order_id LIMIT 8)
    SELECT a.client_order_id,a.attempt_id,a.state,a.created_at,a.rejected_at,
           i.intent_id AS parent_intent_id,i.tenant_id AS parent_tenant_id,
           COALESCE(i.tenant_id=$2,FALSE) AS owner_matches
      FROM a LEFT JOIN LATERAL (
           SELECT intent_id,tenant_id FROM public.order_intents i
            WHERE (i.venue_id,i.account_scope,i.client_order_id)=('binance:spot',$1,a.client_order_id)
            LIMIT 1) i ON true
     ORDER BY a.client_order_id";

// $1 account_scope, $2 expected tenant; continuation ($3 symbol, $4
// exchange_order_id).
pub(crate) const ORDERS_FIRST: &str =
    "SELECT b.symbol,b.exchange_order_id,b.tenant_id,b.tenant_id=$2 AS owner_matches,
       b.client_order_id,b.owner,b.intent_id,b.position_id,b.kind,b.side,b.status,b.phase,
       b.fills_complete
    FROM (SELECT symbol,exchange_order_id,tenant_id,client_order_id,owner,intent_id,position_id,
                 kind,side,status,phase,fills_complete
            FROM public.venue_orders
           WHERE venue_id='binance:spot' AND account_scope=$1
           ORDER BY symbol,exchange_order_id LIMIT 8) b
    ORDER BY b.symbol,b.exchange_order_id";
pub(crate) const ORDERS_NEXT: &str =
    "SELECT b.symbol,b.exchange_order_id,b.tenant_id,b.tenant_id=$2 AS owner_matches,
       b.client_order_id,b.owner,b.intent_id,b.position_id,b.kind,b.side,b.status,b.phase,
       b.fills_complete
    FROM (SELECT symbol,exchange_order_id,tenant_id,client_order_id,owner,intent_id,position_id,
                 kind,side,status,phase,fills_complete
            FROM public.venue_orders
           WHERE venue_id='binance:spot' AND account_scope=$1
             AND (symbol,exchange_order_id)>($3,$4)
           ORDER BY symbol,exchange_order_id LIMIT 8) b
    ORDER BY b.symbol,b.exchange_order_id";

/// A physical page. `full` may have another page; the end is a later short
/// page.
#[derive(Debug, Eq, PartialEq)]
pub(crate) struct SourcePage<T> {
    pub(crate) rows: Vec<T>,
    pub(crate) full: bool,
}

impl<T> SourcePage<T> {
    fn new(rows: Vec<T>) -> Self {
        let full = rows.len() == PAGE_LIMIT;
        Self { rows, full }
    }
}

#[derive(Debug, Eq, PartialEq, sqlx::FromRow)]
pub(crate) struct IntentSource {
    pub(crate) client_order_id: String,
    pub(crate) intent_id: Uuid,
    pub(crate) tenant_id: Uuid,
    pub(crate) owner_matches: bool,
    pub(crate) position_id: Option<Uuid>,
    pub(crate) kind: String,
    pub(crate) venue_outcome: Option<String>,
    pub(crate) not_sent: bool,
    pub(crate) venue_evidence_present: bool,
    pub(crate) disposition_kind: Option<String>,
    pub(crate) created_at: DateTime<Utc>,
}

#[derive(Debug, Eq, PartialEq, sqlx::FromRow)]
pub(crate) struct AttemptSource {
    pub(crate) client_order_id: String,
    pub(crate) attempt_id: Uuid,
    pub(crate) state: String,
    pub(crate) created_at: DateTime<Utc>,
    pub(crate) rejected_at: Option<DateTime<Utc>>,
    /// `None` is an unresolved parent, never an omitted attempt.
    pub(crate) parent_intent_id: Option<Uuid>,
    pub(crate) parent_tenant_id: Option<Uuid>,
    pub(crate) owner_matches: bool,
}

#[derive(Debug, Eq, PartialEq, sqlx::FromRow)]
pub(crate) struct OrderSource {
    pub(crate) symbol: String,
    pub(crate) exchange_order_id: String,
    pub(crate) tenant_id: Uuid,
    pub(crate) owner_matches: bool,
    pub(crate) client_order_id: String,
    pub(crate) owner: String,
    pub(crate) intent_id: Option<Uuid>,
    pub(crate) position_id: Option<Uuid>,
    pub(crate) kind: Option<String>,
    pub(crate) side: String,
    pub(crate) status: String,
    pub(crate) phase: String,
    pub(crate) fills_complete: bool,
}

/// `after` is the exact last client_order_id of the previous page, empty
/// strings included.
pub(crate) async fn read_intent_page(
    connection: &mut PgConnection,
    account_scope: &str,
    expected_tenant: Uuid,
    after: Option<&str>,
) -> Result<SourcePage<IntentSource>, sqlx::Error> {
    let query = match after {
        None => sqlx::query_as::<_, IntentSource>(INTENTS_FIRST)
            .bind(account_scope)
            .bind(expected_tenant),
        Some(last) => sqlx::query_as::<_, IntentSource>(INTENTS_NEXT)
            .bind(account_scope)
            .bind(expected_tenant)
            .bind(last),
    };
    Ok(SourcePage::new(query.fetch_all(connection).await?))
}

/// `after` is the exact last attempt client_order_id of the previous page.
pub(crate) async fn read_attempt_page(
    connection: &mut PgConnection,
    account_scope: &str,
    expected_tenant: Uuid,
    after: Option<&str>,
) -> Result<SourcePage<AttemptSource>, sqlx::Error> {
    let query = match after {
        None => sqlx::query_as::<_, AttemptSource>(ATTEMPTS_FIRST)
            .bind(account_scope)
            .bind(expected_tenant),
        Some(last) => sqlx::query_as::<_, AttemptSource>(ATTEMPTS_NEXT)
            .bind(account_scope)
            .bind(expected_tenant)
            .bind(last),
    };
    Ok(SourcePage::new(query.fetch_all(connection).await?))
}

/// `after` is the exact last (symbol, exchange_order_id) of the previous page.
pub(crate) async fn read_order_page(
    connection: &mut PgConnection,
    account_scope: &str,
    expected_tenant: Uuid,
    after: Option<(&str, &str)>,
) -> Result<SourcePage<OrderSource>, sqlx::Error> {
    let query = match after {
        None => sqlx::query_as::<_, OrderSource>(ORDERS_FIRST)
            .bind(account_scope)
            .bind(expected_tenant),
        Some((symbol, exchange_order_id)) => sqlx::query_as::<_, OrderSource>(ORDERS_NEXT)
            .bind(account_scope)
            .bind(expected_tenant)
            .bind(symbol)
            .bind(exchange_order_id),
    };
    Ok(SourcePage::new(query.fetch_all(connection).await?))
}

#[cfg(test)]
#[path = "cold_source_pg_tests.rs"]
mod pg_tests;
