//! Bounded native source pages for a cold Spot generation.
//! One indexed statement per page on a caller-owned connection and snapshot.
//! The expected tenant is compared after LIMIT, so a foreign row stays visible.
use chrono::{DateTime, NaiveDate, Utc};
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

// $1 account_scope, $2 expected tenant; continuation $3 last position_id.
// Routes are materialized first; each then takes one nullable leg PK probe and
// one independent nullable current-admission probe, so neither can drop a
// route.
pub(crate) const ROUTES_FIRST: &str = "WITH r AS MATERIALIZED (
        SELECT position_id,tenant_id,venue_id,account_scope,routed_at
          FROM public.leg_routes
         WHERE venue_id='binance:spot' AND account_scope=$1
         ORDER BY position_id LIMIT 8)
    SELECT r.position_id,r.tenant_id,r.tenant_id=$2 AS owner_matches,r.venue_id,r.account_scope,r.routed_at,
           l.leg_position_id,l.leg_tenant_id,l.leg_venue_id,l.leg_account_scope,l.leg_symbol,
           l.leg_admission_intent_id,l.leg_decided_month,l.leg_recovery_state,l.leg_closed_at,
           a.admission_intent_id,a.admission_position_id,a.admission_tenant_id,a.admission_venue_id,
           a.admission_account_scope,a.admission_symbol,a.admission_decided_month
      FROM r LEFT JOIN LATERAL (
           SELECT position_id AS leg_position_id,tenant_id AS leg_tenant_id,venue_id AS leg_venue_id,
                  account_scope AS leg_account_scope,symbol AS leg_symbol,
                  admission_intent_id AS leg_admission_intent_id,decided_month AS leg_decided_month,
                  recovery_state AS leg_recovery_state,closed_at AS leg_closed_at
             FROM public.balance_legs WHERE position_id=r.position_id LIMIT 1) l ON true
      LEFT JOIN LATERAL (
           SELECT entry_intent_id AS admission_intent_id,position_id AS admission_position_id,
                  tenant_id AS admission_tenant_id,venue_id AS admission_venue_id,
                  account_scope AS admission_account_scope,symbol AS admission_symbol,
                  decided_month AS admission_decided_month
             FROM public.balance_leg_admissions
            WHERE position_id=r.position_id AND superseded_at IS NULL LIMIT 1) a ON true
     ORDER BY r.position_id";
pub(crate) const ROUTES_NEXT: &str = "WITH r AS MATERIALIZED (
        SELECT position_id,tenant_id,venue_id,account_scope,routed_at
          FROM public.leg_routes
         WHERE venue_id='binance:spot' AND account_scope=$1 AND position_id>$3::uuid
         ORDER BY position_id LIMIT 8)
    SELECT r.position_id,r.tenant_id,r.tenant_id=$2 AS owner_matches,r.venue_id,r.account_scope,r.routed_at,
           l.leg_position_id,l.leg_tenant_id,l.leg_venue_id,l.leg_account_scope,l.leg_symbol,
           l.leg_admission_intent_id,l.leg_decided_month,l.leg_recovery_state,l.leg_closed_at,
           a.admission_intent_id,a.admission_position_id,a.admission_tenant_id,a.admission_venue_id,
           a.admission_account_scope,a.admission_symbol,a.admission_decided_month
      FROM r LEFT JOIN LATERAL (
           SELECT position_id AS leg_position_id,tenant_id AS leg_tenant_id,venue_id AS leg_venue_id,
                  account_scope AS leg_account_scope,symbol AS leg_symbol,
                  admission_intent_id AS leg_admission_intent_id,decided_month AS leg_decided_month,
                  recovery_state AS leg_recovery_state,closed_at AS leg_closed_at
             FROM public.balance_legs WHERE position_id=r.position_id LIMIT 1) l ON true
      LEFT JOIN LATERAL (
           SELECT entry_intent_id AS admission_intent_id,position_id AS admission_position_id,
                  tenant_id AS admission_tenant_id,venue_id AS admission_venue_id,
                  account_scope AS admission_account_scope,symbol AS admission_symbol,
                  decided_month AS admission_decided_month
             FROM public.balance_leg_admissions
            WHERE position_id=r.position_id AND superseded_at IS NULL LIMIT 1) a ON true
     ORDER BY r.position_id";

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

/// One physical route with its nullable current leg and current admission.
/// Every route row is data; classification never removes it from the page.
#[derive(Debug, Eq, PartialEq, sqlx::FromRow)]
pub(crate) struct RouteSource {
    pub(crate) position_id: Uuid,
    pub(crate) tenant_id: Uuid,
    pub(crate) owner_matches: bool,
    pub(crate) venue_id: String,
    pub(crate) account_scope: String,
    pub(crate) routed_at: DateTime<Utc>,
    pub(crate) leg_position_id: Option<Uuid>,
    pub(crate) leg_tenant_id: Option<Uuid>,
    pub(crate) leg_venue_id: Option<String>,
    pub(crate) leg_account_scope: Option<String>,
    pub(crate) leg_symbol: Option<String>,
    pub(crate) leg_admission_intent_id: Option<Uuid>,
    pub(crate) leg_decided_month: Option<NaiveDate>,
    pub(crate) leg_recovery_state: Option<String>,
    pub(crate) leg_closed_at: Option<DateTime<Utc>>,
    pub(crate) admission_intent_id: Option<Uuid>,
    pub(crate) admission_position_id: Option<Uuid>,
    pub(crate) admission_tenant_id: Option<Uuid>,
    pub(crate) admission_venue_id: Option<String>,
    pub(crate) admission_account_scope: Option<String>,
    pub(crate) admission_symbol: Option<String>,
    pub(crate) admission_decided_month: Option<NaiveDate>,
}

/// A route's shape. Only `ArmedOnly` and `Leg` are eligible; a diagnostic
/// refuses the cold result while its row stays visible.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RouteClass {
    /// Neither leg nor current admission: arming only, no leg authority.
    ArmedOnly,
    /// A consistent current leg and admission; closed legs remain data.
    Leg {
        closed: bool,
    },
    Diagnostic(RouteDefect),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RouteDefect {
    ForeignOwner,
    LegWithoutAdmission,
    AdmissionWithoutLeg,
    LegRouteMismatch,
    AdmissionRouteMismatch,
    /// The leg points at an admission other than the current one.
    StaleLegPointer,
    /// Same admission identity but a different symbol or decided month.
    PairMismatch,
}

impl RouteSource {
    pub(crate) fn classify(&self) -> RouteClass {
        use RouteDefect::*;
        if !self.owner_matches {
            return RouteClass::Diagnostic(ForeignOwner);
        }
        let leg = self.leg_position_id.is_some();
        let admission = self.admission_intent_id.is_some();
        match (leg, admission) {
            (false, false) => return RouteClass::ArmedOnly,
            (true, false) => return RouteClass::Diagnostic(LegWithoutAdmission),
            (false, true) => return RouteClass::Diagnostic(AdmissionWithoutLeg),
            (true, true) => {},
        }
        let route = (
            Some(self.position_id),
            Some(self.tenant_id),
            Some(self.venue_id.as_str()),
            Some(self.account_scope.as_str()),
        );
        if (
            self.leg_position_id,
            self.leg_tenant_id,
            self.leg_venue_id.as_deref(),
            self.leg_account_scope.as_deref(),
        ) != route
        {
            return RouteClass::Diagnostic(LegRouteMismatch);
        }
        if (
            self.admission_position_id,
            self.admission_tenant_id,
            self.admission_venue_id.as_deref(),
            self.admission_account_scope.as_deref(),
        ) != route
        {
            return RouteClass::Diagnostic(AdmissionRouteMismatch);
        }
        if self.leg_admission_intent_id != self.admission_intent_id {
            return RouteClass::Diagnostic(StaleLegPointer);
        }
        if self.leg_symbol != self.admission_symbol
            || self.leg_decided_month != self.admission_decided_month
        {
            return RouteClass::Diagnostic(PairMismatch);
        }
        RouteClass::Leg { closed: self.leg_closed_at.is_some() }
    }

    pub(crate) fn eligible(&self) -> bool {
        !matches!(self.classify(), RouteClass::Diagnostic(_))
    }
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

/// `after` is the exact last route position_id of the previous page.
pub(crate) async fn read_route_page(
    connection: &mut PgConnection,
    account_scope: &str,
    expected_tenant: Uuid,
    after: Option<Uuid>,
) -> Result<SourcePage<RouteSource>, sqlx::Error> {
    let query = match after {
        None => sqlx::query_as::<_, RouteSource>(ROUTES_FIRST)
            .bind(account_scope)
            .bind(expected_tenant),
        Some(last) => sqlx::query_as::<_, RouteSource>(ROUTES_NEXT)
            .bind(account_scope)
            .bind(expected_tenant)
            .bind(last),
    };
    Ok(SourcePage::new(query.fetch_all(connection).await?))
}

#[cfg(test)]
#[path = "cold_source_pg_tests.rs"]
mod pg_tests;

#[cfg(test)]
#[path = "cold_route_pg_tests.rs"]
mod route_tests;
