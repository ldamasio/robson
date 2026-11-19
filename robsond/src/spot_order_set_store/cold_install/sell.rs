//! Deterministic SELL side of one already validated, bounded cold Spot cut.
use std::collections::BTreeMap;

use uuid::Uuid;

use super::super::cold_source::{
    AttemptSource, IntentSource, OrderSource, RouteClass, RouteSource,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SellScope<'a> {
    Leg { symbol: &'a str, position: Uuid },
    Symbol(&'a str),
    Unbound,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct SellContribution<'a> {
    pub(super) scope: SellScope<'a>,
    // A present contribution is potential. Conflicts retain that hold.
    pub(super) conflict: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct SellIntent<'a> {
    pub(super) source_position: Option<Uuid>,
    pub(super) contribution: SellContribution<'a>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct SellCounts {
    pub(super) potential: i64,
    pub(super) conflict: i64,
}

#[derive(Debug, Default, Eq, PartialEq)]
pub(super) struct SellProjection<'a> {
    pub(super) intents: BTreeMap<Uuid, SellIntent<'a>>,
    pub(super) orders: BTreeMap<(&'a str, &'a str), SellContribution<'a>>,
    pub(super) legs: BTreeMap<(&'a str, Uuid), SellCounts>,
    pub(super) symbols: BTreeMap<&'a str, SellCounts>,
    pub(super) unbound: Option<SellCounts>,
}

#[derive(Debug, Eq, PartialEq)]
pub(in crate::spot_order_set_store) enum SellBuildRefusal {
    DuplicateSource,
    InvalidScopedSource,
    LeafBudget,
}

impl SellProjection<'_> {
    pub(super) fn leaf_rows(&self) -> usize {
        // Every map is built from the owner cut's <=63 rows per family.
        self.intents.len()
            + self.orders.len()
            + self.legs.len()
            + self.symbols.len()
            + usize::from(self.unbound.is_some())
    }
}

impl<'a> SellProjection<'a> {
    fn add(&mut self, contribution: SellContribution<'a>) -> Result<(), SellBuildRefusal> {
        let counts = match contribution.scope {
            SellScope::Leg { symbol, position } => self.legs.entry((symbol, position)).or_default(),
            SellScope::Symbol(symbol) => self.symbols.entry(symbol).or_default(),
            SellScope::Unbound => self.unbound.get_or_insert_with(SellCounts::default),
        };
        counts.potential = counts.potential.checked_add(1).ok_or(SellBuildRefusal::LeafBudget)?;
        counts.conflict = counts
            .conflict
            .checked_add(i64::from(contribution.conflict))
            .ok_or(SellBuildRefusal::LeafBudget)?;
        Ok(())
    }
}

fn native_sell_kind(journal: &IntentSource) -> Option<&'static str> {
    match journal.kind.as_str() {
        "protection" => Some("stop"),
        "exit" => Some("sell"),
        _ => None,
    }
}

fn exact_pair(journal: &IntentSource, order: &OrderSource, kind: &str) -> bool {
    order.intent_id == Some(journal.intent_id)
        && order.owner == "leg"
        && order.side == "SELL"
        && order.tenant_id == journal.tenant_id
        && order.position_id.is_some()
        && order.position_id == journal.position_id
        && order.client_order_id == journal.client_order_id
        && order.kind.as_deref() == Some(kind)
}

fn recognized_terminal(order: &OrderSource) -> bool {
    order.phase == "terminal"
        && matches!(
            order.status.as_str(),
            "FILLED" | "CANCELED" | "EXPIRED" | "EXPIRED_IN_MATCH" | "REJECTED"
        )
}

fn native_contribution<'a>(
    order: &'a OrderSource,
    journal: Option<&IntentSource>,
) -> Option<SellContribution<'a>> {
    let expected_kind = journal.and_then(native_sell_kind);
    if order.side != "SELL" && expected_kind.is_none() {
        return None;
    }
    let exact = journal.zip(expected_kind).is_some_and(|(j, kind)| exact_pair(j, order, kind));
    let contradicted =
        journal.is_some_and(|j| j.not_sent || j.venue_outcome.as_deref() == Some("rejected"));
    if recognized_terminal(order) && exact && !contradicted {
        return None;
    }
    let scope = match order.position_id {
        Some(position) => SellScope::Leg { symbol: &order.symbol, position },
        None => SellScope::Symbol(&order.symbol),
    };
    let conflict = order.phase == "terminal"
        || contradicted
        || ((order.position_id.is_some() || journal.is_some()) && !exact);
    Some(SellContribution { scope, conflict })
}

fn journal_contribution<'a>(
    journal: &'a IntentSource,
    order: Option<&OrderSource>,
    attempt: Option<&AttemptSource>,
    scope: SellScope<'a>,
) -> Option<SellContribution<'a>> {
    let kind = native_sell_kind(journal)?;
    if let Some(order) = order {
        // An exact pair belongs under the native key. A mismatched order
        // retains an independent journal hold even if J claims no send.
        return (!exact_pair(journal, order, kind))
            .then_some(SellContribution { scope, conflict: true });
    }
    let rejected = journal.venue_outcome.as_deref() == Some("rejected");
    let no_send =
        journal.not_sent && journal.venue_outcome.is_none() && !journal.venue_evidence_present;
    let definite_rejection = rejected
        && !journal.not_sent
        && !journal.venue_evidence_present
        && attempt.is_some_and(|a| {
            a.state == "rejected"
                && a.rejected_at.is_some()
                && a.parent_intent_id == Some(journal.intent_id)
                && a.parent_tenant_id == Some(journal.tenant_id)
        });
    (!no_send && !definite_rejection).then_some(SellContribution {
        scope,
        conflict: journal.not_sent || rejected,
    })
}

/// The source owner has already checked both directions of every native link,
/// including exact owners outside its paged prefix. Absence here means the
/// original identity was absent in that snapshot, or no native row names J.
pub(super) fn rebuild_sell<'a>(
    tenant: Uuid,
    account: &str,
    intents: &'a [IntentSource],
    attempts: &'a [AttemptSource],
    orders: &'a [OrderSource],
    routes: &'a [RouteSource],
    leaf_limit: usize,
) -> Result<SellProjection<'a>, SellBuildRefusal> {
    if [intents.len(), attempts.len(), orders.len(), routes.len()]
        .into_iter()
        .any(|n| n > 63)
    {
        return Err(SellBuildRefusal::LeafBudget);
    }
    if intents.iter().any(|j| !j.owner_matches || j.tenant_id != tenant)
        || attempts.iter().any(|a| {
            !a.owner_matches || a.parent_tenant_id != Some(tenant) || a.parent_intent_id.is_none()
        })
        || orders.iter().any(|o| !o.owner_matches || o.tenant_id != tenant)
        || routes.iter().any(|r| {
            !r.owner_matches
                || r.tenant_id != tenant
                || r.venue_id != "binance:spot"
                || r.account_scope != account
                || matches!(r.classify(), RouteClass::Diagnostic(_))
        })
    {
        return Err(SellBuildRefusal::InvalidScopedSource);
    }
    let journal_by_id: BTreeMap<_, _> = intents.iter().map(|j| (j.intent_id, j)).collect();
    let attempt_by_client: BTreeMap<_, _> =
        attempts.iter().map(|a| (a.client_order_id.as_str(), a)).collect();
    let order_by_journal: BTreeMap<_, _> =
        orders.iter().filter_map(|o| o.intent_id.map(|id| (id, o))).collect();
    let route_by_position: BTreeMap<_, _> = routes.iter().map(|r| (r.position_id, r)).collect();
    if journal_by_id.len() != intents.len()
        || attempt_by_client.len() != attempts.len()
        || route_by_position.len() != routes.len()
        || order_by_journal.len() != orders.iter().filter(|o| o.intent_id.is_some()).count()
    {
        return Err(SellBuildRefusal::DuplicateSource);
    }

    let mut projection = SellProjection::default();
    for order in orders {
        let intent = order.intent_id.and_then(|id| journal_by_id.get(&id)).copied();
        if let Some(contribution) = native_contribution(order, intent) {
            let key = (order.symbol.as_str(), order.exchange_order_id.as_str());
            if projection.orders.insert(key, contribution).is_some() {
                return Err(SellBuildRefusal::DuplicateSource);
            }
            projection.add(contribution)?;
        }
    }
    for journal in intents {
        let leg = journal
            .position_id
            .and_then(|position| route_by_position.get(&position))
            .filter(|route| matches!(route.classify(), RouteClass::Leg { .. }));
        let scope = match leg {
            Some(route) => SellScope::Leg {
                symbol: route.leg_symbol.as_deref().ok_or(SellBuildRefusal::InvalidScopedSource)?,
                position: route.position_id,
            },
            None => SellScope::Unbound,
        };
        let order = order_by_journal.get(&journal.intent_id).copied();
        let attempt = attempt_by_client.get(journal.client_order_id.as_str()).copied();
        if let Some(contribution) = journal_contribution(journal, order, attempt, scope) {
            projection.intents.insert(
                journal.intent_id,
                SellIntent {
                    source_position: journal.position_id,
                    contribution,
                },
            );
            projection.add(contribution)?;
        }
    }
    if projection.leaf_rows() > leaf_limit {
        return Err(SellBuildRefusal::LeafBudget);
    }
    Ok(projection)
}
