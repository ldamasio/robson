//! Typed source cut for the private cold-generation owner. The external
//! restore identity is correlation data. It cannot authorize publication.
use std::{collections::BTreeMap, mem::size_of};

use sqlx::{PgConnection, PgPool};
use tokio::time::Instant;
use uuid::Uuid;

use super::{
    cold_audit::{
        audit, ColdAuditRefusal, ColdAuditScope, ColdSourceReceipt, ColdSourceSink, PageBudget,
        SinkFull, Stage,
    },
    cold_source::{AttemptSource, Edge, IntentSource, OrderSource, RouteSource, TEXT_LIMIT},
};

const ANCHOR: &str = include_str!("cold_owner_anchor.sql");
const PAGES: u32 = 8;
const ROWS: usize = 63;
const BYTES: usize = 256 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RestoreIdentity {
    pub(crate) operation: Uuid,
    pub(crate) target: Uuid,
}

pub(crate) struct OwnerRequest<'a> {
    pub(crate) scope: ColdAuditScope<'a>,
    pub(crate) expected_uid: &'a str,
    pub(crate) epoch: Uuid,
    pub(crate) accounting_version: i64,
    pub(crate) restore: RestoreIdentity,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct PriorGeneration {
    pub(super) generation_id: Uuid,
    pub(super) process_epoch: Option<Uuid>,
    pub(super) source_audit_id: Option<Uuid>,
    pub(super) accounting_version: Option<i64>,
    pub(super) position_hints_complete: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ObservedAnchor {
    pub(super) tenant_id: Uuid,
    pub(super) account_scope: String,
    pub(super) uid: String,
    pub(super) process_epoch: Uuid,
    pub(super) accounting_version: i64,
    pub(super) prior: Option<PriorGeneration>,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum AnchorDefect {
    Epoch,
    AccountingVersion,
    PriorGenerationShape,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum EdgeDefect {
    Intent(Uuid),
    Order {
        symbol: String,
        exchange_order_id: String,
    },
}

#[derive(sqlx::FromRow)]
struct AnchorRow {
    tenant_id: Uuid,
    uid_wide: bool,
    uid: String,
    accounting_version: Option<i64>,
    latest_epoch: Option<Uuid>,
    latest_epoch_live: bool,
    prior_generation_id: Option<Uuid>,
    prior_process_epoch: Option<Uuid>,
    prior_source_audit_id: Option<Uuid>,
    prior_accounting_version: Option<i64>,
    prior_position_hints_complete: Option<bool>,
}

pub(super) async fn read_anchor(
    connection: &mut PgConnection,
    request: &OwnerRequest<'_>,
) -> Result<ObservedAnchor, ColdAuditRefusal> {
    let row = sqlx::query_as::<_, AnchorRow>(ANCHOR)
        .bind(request.scope.account_scope)
        .fetch_optional(connection)
        .await
        .map_err(|e| ColdAuditRefusal::Database(Stage::Account, e))?
        .ok_or(ColdAuditRefusal::AccountMissing)?;
    if row.tenant_id != request.scope.tenant_id {
        return Err(ColdAuditRefusal::AccountTenant);
    }
    if row.uid_wide || row.uid.trim().is_empty() || row.uid != request.expected_uid {
        return Err(ColdAuditRefusal::AccountUid);
    }
    if row.latest_epoch != Some(request.epoch) || !row.latest_epoch_live {
        return Err(ColdAuditRefusal::Anchor(AnchorDefect::Epoch));
    }
    if row.accounting_version != Some(request.accounting_version) {
        return Err(ColdAuditRefusal::Anchor(AnchorDefect::AccountingVersion));
    }
    let prior = match (row.prior_generation_id, row.prior_position_hints_complete) {
        (None, None) => None,
        (Some(generation_id), Some(position_hints_complete)) => Some(PriorGeneration {
            generation_id,
            process_epoch: row.prior_process_epoch,
            source_audit_id: row.prior_source_audit_id,
            accounting_version: row.prior_accounting_version,
            position_hints_complete,
        }),
        _ => return Err(ColdAuditRefusal::Anchor(AnchorDefect::PriorGenerationShape)),
    };
    Ok(ObservedAnchor {
        tenant_id: row.tenant_id,
        account_scope: request.scope.account_scope.to_owned(),
        uid: row.uid,
        process_epoch: request.epoch,
        accounting_version: request.accounting_version,
        prior,
    })
}

#[derive(Debug)]
pub(crate) struct ColdOwnerCut {
    pub(super) anchor: ObservedAnchor,
    pub(crate) receipt: ColdSourceReceipt,
    pub(super) restore: RestoreIdentity,
    rows: CapturedRows,
}

impl ColdOwnerCut {
    pub(crate) fn intents(&self) -> &[IntentSource] {
        &self.rows.intents
    }
    pub(crate) fn attempts(&self) -> &[AttemptSource] {
        &self.rows.attempts
    }
    pub(crate) fn orders(&self) -> &[OrderSource] {
        &self.rows.orders
    }
    pub(crate) fn routes(&self) -> &[RouteSource] {
        &self.rows.routes
    }
}

pub(crate) async fn capture(
    pool: &PgPool,
    request: &OwnerRequest<'_>,
    deadline: Instant,
) -> Result<ColdOwnerCut, ColdAuditRefusal> {
    if request.expected_uid.len() > TEXT_LIMIT
        || request.expected_uid.trim().is_empty()
        || request.scope.account_scope.len() > TEXT_LIMIT
        || request.epoch.is_nil()
        || request.accounting_version < 0
        || request.restore.operation.is_nil()
        || request.restore.target.is_nil()
    {
        return Err(ColdAuditRefusal::InvalidRequest);
    }
    let budget = PageBudget {
        intents: PAGES,
        attempts: PAGES,
        orders: PAGES,
        routes: PAGES,
    };
    let mut rows = CapturedRows::new();
    let (receipt, anchor) =
        audit(pool, request.scope, budget, deadline, &mut rows, Some(request)).await?;
    let anchor = anchor.ok_or(ColdAuditRefusal::InvalidRequest)?;
    rows.validate_edges(&anchor).map_err(ColdAuditRefusal::Unproven)?;
    if Instant::now() >= deadline {
        return Err(ColdAuditRefusal::Deadline(Stage::Commit));
    }
    Ok(ColdOwnerCut {
        anchor,
        receipt,
        restore: request.restore,
        rows,
    })
}

#[derive(Debug)]
struct CapturedRows {
    intents: Vec<IntentSource>,
    attempts: Vec<AttemptSource>,
    orders: Vec<OrderSource>,
    routes: Vec<RouteSource>,
    bytes: usize,
}

impl CapturedRows {
    fn new() -> Self {
        Self {
            intents: Vec::with_capacity(ROWS),
            attempts: Vec::with_capacity(ROWS),
            orders: Vec::with_capacity(ROWS),
            routes: Vec::with_capacity(ROWS),
            bytes: size_of::<Self>()
                + ROWS
                    * (size_of::<IntentSource>()
                        + size_of::<AttemptSource>()
                        + size_of::<OrderSource>()
                        + size_of::<RouteSource>()),
        }
    }

    fn validate_edges(&self, anchor: &ObservedAnchor) -> Result<(), EdgeDefect> {
        let journals: BTreeMap<_, _> = self.intents.iter().map(|r| (r.intent_id, r)).collect();
        let orders_by_journal: BTreeMap<_, _> =
            self.orders.iter().filter_map(|r| r.intent_id.map(|id| (id, r))).collect();
        for j in &self.intents {
            let named = orders_by_journal.get(&j.intent_id);
            let exact = match j.reverse_edge(anchor.tenant_id, &anchor.account_scope) {
                Edge::InScope => named.is_some_and(|o| {
                    j.order_symbol.as_deref() == Some(o.symbol.as_str())
                        && j.order_exchange_order_id.as_deref()
                            == Some(o.exchange_order_id.as_str())
                }),
                Edge::Unlinked => named.is_none(),
                Edge::AbsentNamed | Edge::Foreign => false,
            };
            if !exact {
                return Err(EdgeDefect::Intent(j.intent_id));
            }
        }
        for o in &self.orders {
            let journal = o.intent_id.and_then(|id| journals.get(&id));
            let exact = match o.named_edge(anchor.tenant_id, &anchor.account_scope) {
                Edge::InScope => journal.is_some_and(|j| {
                    o.named_client_order_id.as_deref() == Some(j.client_order_id.as_str())
                }),
                Edge::AbsentNamed | Edge::Unlinked => journal.is_none(),
                Edge::Foreign => false,
            };
            if !exact {
                return Err(EdgeDefect::Order {
                    symbol: o.symbol.clone(),
                    exchange_order_id: o.exchange_order_id.clone(),
                });
            }
        }
        Ok(())
    }
}

fn text_len(values: &[Option<&str>]) -> usize {
    values.iter().flatten().map(|text| text.len()).sum()
}

fn append<T: Clone>(
    retained: &mut Vec<T>,
    bytes: &mut usize,
    incoming: &[T],
    text_bytes: impl Fn(&T) -> usize,
) -> Result<(), SinkFull> {
    if retained.len() + incoming.len() > ROWS {
        return Err(SinkFull);
    }
    let charged = incoming.iter().map(text_bytes).sum::<usize>();
    let next = bytes.checked_add(charged).filter(|n| *n <= BYTES).ok_or(SinkFull)?;
    retained.extend_from_slice(incoming);
    *bytes = next;
    Ok(())
}

impl ColdSourceSink for CapturedRows {
    fn intents(&mut self, rows: &[IntentSource]) -> Result<(), SinkFull> {
        append(&mut self.intents, &mut self.bytes, rows, |r| {
            text_len(&[
                Some(&r.client_order_id),
                Some(&r.kind),
                r.cancels_order_id.as_deref(),
                r.canonical_cancel_target.as_deref(),
                r.venue_outcome.as_deref(),
                r.disposition_kind.as_deref(),
                r.order_venue_id.as_deref(),
                r.order_account_scope.as_deref(),
                r.order_symbol.as_deref(),
                r.order_exchange_order_id.as_deref(),
            ])
        })
    }
    fn attempts(&mut self, rows: &[AttemptSource]) -> Result<(), SinkFull> {
        append(&mut self.attempts, &mut self.bytes, rows, |r| {
            r.client_order_id.len() + r.state.len()
        })
    }
    fn orders(&mut self, rows: &[OrderSource]) -> Result<(), SinkFull> {
        append(&mut self.orders, &mut self.bytes, rows, |r| {
            text_len(&[
                Some(&r.symbol),
                Some(&r.exchange_order_id),
                Some(&r.client_order_id),
                Some(&r.owner),
                r.kind.as_deref(),
                Some(&r.side),
                Some(&r.status),
                Some(&r.phase),
                r.named_venue_id.as_deref(),
                r.named_account_scope.as_deref(),
                r.named_client_order_id.as_deref(),
            ])
        })
    }
    fn routes(&mut self, rows: &[RouteSource]) -> Result<(), SinkFull> {
        append(&mut self.routes, &mut self.bytes, rows, |r| {
            text_len(&[
                Some(&r.venue_id),
                Some(&r.account_scope),
                r.leg_venue_id.as_deref(),
                r.leg_account_scope.as_deref(),
                r.leg_symbol.as_deref(),
                r.leg_recovery_state.as_deref(),
                r.admission_venue_id.as_deref(),
                r.admission_account_scope.as_deref(),
                r.admission_symbol.as_deref(),
            ])
        })
    }
}
