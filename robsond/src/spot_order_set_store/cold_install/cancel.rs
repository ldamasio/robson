//! Pure CANCEL reconstruction for the bounded, validated cold Spot source cut.
//! The audit owns account binding and verification of both named-edge directions.

use std::collections::{BTreeMap, BTreeSet};

use uuid::Uuid;

use super::super::cold_source::{IntentSource, OrderSource, RouteClass, RouteSource};

const SPOT: &str = "binance:spot";
const SOURCE_ROWS_PER_FAMILY: usize = 63;
const GENERATION_LEAF_LIMIT: usize = 640;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) struct CancelLegKey<'a> {
    pub(super) symbol: &'a str,
    pub(super) position_id: Uuid,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) struct CancelGroupKey<'a> {
    pub(super) leg: CancelLegKey<'a>,
    pub(super) target_id: &'a str,
}

#[derive(Debug, Eq, PartialEq)]
pub(super) struct CancelMember<'a> {
    pub(super) source_position: Option<Uuid>,
    pub(super) leg: Option<CancelLegKey<'a>>,
    pub(super) target_id: Option<&'a str>,
    pub(super) pending: bool,
    pub(super) conflict: bool,
}

#[derive(Debug, Default, Eq, PartialEq)]
pub(super) struct CancelGroup {
    pub(super) pending_count: i64,
    pub(super) conflict_count: i64,
    pub(super) terminal_mask: bool,
}

#[derive(Debug, Default, Eq, PartialEq)]
pub(super) struct CancelCounts {
    pub(super) active_count: i64,
    pub(super) conflict_count: i64,
}

impl CancelCounts {
    fn add(&mut self, active: i64, conflict: i64) -> Result<(), CancelRebuildError> {
        add_counts(&mut self.active_count, &mut self.conflict_count, active, conflict)
    }
}

#[derive(Debug, Default, Eq, PartialEq)]
pub(super) struct CancelLeaves<'a> {
    pub(super) members: BTreeMap<Uuid, CancelMember<'a>>,
    pub(super) groups: BTreeMap<CancelGroupKey<'a>, CancelGroup>,
    pub(super) legs: BTreeMap<CancelLegKey<'a>, CancelCounts>,
    pub(super) unbound: Option<CancelCounts>,
    pub(super) ungroupable: BTreeMap<CancelLegKey<'a>, CancelCounts>,
    pub(super) leaf_count: usize,
}

#[derive(Debug, Eq, PartialEq)]
pub(in crate::spot_order_set_store) enum CancelRebuildError {
    SourceRowLimit,
    ForeignSource,
    DuplicateSource(&'static str),
    InvalidRoute,
    InvalidCanonicalTarget,
    CountOverflow,
    LeafLimit,
}

fn add_counts(
    pending: &mut i64,
    conflicts: &mut i64,
    add_pending: i64,
    add_conflicts: i64,
) -> Result<(), CancelRebuildError> {
    let next_pending = pending.checked_add(add_pending).ok_or(CancelRebuildError::CountOverflow)?;
    let next_conflicts =
        conflicts.checked_add(add_conflicts).ok_or(CancelRebuildError::CountOverflow)?;
    *pending = next_pending;
    *conflicts = next_conflicts;
    Ok(())
}

fn terminal_group<'a>(
    order: &'a OrderSource,
    journal: &BTreeMap<Uuid, &'a IntentSource>,
    route_symbols: &BTreeMap<Uuid, Option<&'a str>>,
) -> Option<CancelGroupKey<'a>> {
    if order.owner != "leg"
        || order.phase != "terminal"
        || !matches!(
            order.status.as_str(),
            "FILLED" | "CANCELED" | "EXPIRED" | "EXPIRED_IN_MATCH" | "REJECTED"
        )
    {
        return None;
    }
    let position_id = order.position_id?;
    let symbol = route_symbols.get(&position_id).copied().flatten()?;
    let intent = journal.get(&order.intent_id?)?;
    let expected_kind = match intent.kind.as_str() {
        "entry" => "entry",
        "protection" => "stop",
        "exit" => "sell",
        _ => return None,
    };
    if order.symbol != symbol
        || intent.tenant_id != order.tenant_id
        || intent.position_id != Some(position_id)
        || intent.client_order_id != order.client_order_id
        || intent.not_sent
        || order.kind.as_deref() != Some(expected_kind)
    {
        return None;
    }
    Some(CancelGroupKey {
        leg: CancelLegKey { symbol, position_id },
        target_id: &order.exchange_order_id,
    })
}

/// Rebuild five sparse leaf sets for one validated tenant, Spot venue and account.
/// `remaining_leaf_rows` is the shared ten-table budget minus the other leaves.
pub(super) fn rebuild_cancel<'a>(
    tenant_id: Uuid,
    account_scope: &str,
    intents: &'a [IntentSource],
    orders: &'a [OrderSource],
    routes: &'a [RouteSource],
    remaining_leaf_rows: usize,
) -> Result<CancelLeaves<'a>, CancelRebuildError> {
    if [intents.len(), orders.len(), routes.len()]
        .into_iter()
        .any(|count| count > SOURCE_ROWS_PER_FAMILY)
    {
        return Err(CancelRebuildError::SourceRowLimit);
    }

    let mut journal = BTreeMap::new();
    let mut client_ids = BTreeSet::new();
    for intent in intents {
        if !intent.owner_matches || intent.tenant_id != tenant_id {
            return Err(CancelRebuildError::ForeignSource);
        }
        if journal.insert(intent.intent_id, intent).is_some()
            || !client_ids.insert(&intent.client_order_id)
        {
            return Err(CancelRebuildError::DuplicateSource("journal identity"));
        }
    }

    let mut route_symbols = BTreeMap::new();
    for route in routes {
        if !route.owner_matches
            || route.tenant_id != tenant_id
            || route.venue_id != SPOT
            || route.account_scope != account_scope
        {
            return Err(CancelRebuildError::ForeignSource);
        }
        let symbol = match route.classify() {
            RouteClass::Leg { .. } => {
                Some(route.leg_symbol.as_deref().ok_or(CancelRebuildError::InvalidRoute)?)
            },
            RouteClass::ArmedOnly => None,
            RouteClass::Diagnostic(_) => return Err(CancelRebuildError::InvalidRoute),
        };
        if route_symbols.insert(route.position_id, symbol).is_some() {
            return Err(CancelRebuildError::DuplicateSource("route position"));
        }
    }

    let mut leaves = CancelLeaves::default();
    let mut native_keys = BTreeSet::new();
    let mut named_intents = BTreeSet::new();
    for order in orders {
        if !order.owner_matches || order.tenant_id != tenant_id {
            return Err(CancelRebuildError::ForeignSource);
        }
        if !native_keys.insert((&order.symbol, &order.exchange_order_id))
            || order.intent_id.is_some_and(|id| !named_intents.insert(id))
        {
            return Err(CancelRebuildError::DuplicateSource("native identity"));
        }
        if let Some(key) = terminal_group(order, &journal, &route_symbols) {
            leaves.groups.entry(key).or_default().terminal_mask = true;
        }
    }

    for intent in journal.values().filter(|intent| intent.kind == "cancel") {
        let leg = intent.position_id.and_then(|position_id| {
            route_symbols
                .get(&position_id)
                .copied()
                .flatten()
                .map(|symbol| CancelLegKey { symbol, position_id })
        });
        let target_id = intent.canonical_cancel_target.as_deref();
        if target_id.is_some() && target_id != intent.cancels_order_id.as_deref() {
            return Err(CancelRebuildError::InvalidCanonicalTarget);
        }
        let conflict = named_intents.contains(&intent.intent_id)
            || (intent.not_sent
                && (intent.venue_outcome.is_some() || intent.venue_evidence_present));
        let pending = !intent.not_sent || conflict;
        leaves.members.insert(
            intent.intent_id,
            CancelMember {
                source_position: intent.position_id,
                leg,
                target_id,
                pending,
                conflict,
            },
        );

        match (leg, target_id) {
            (Some(leg), Some(target_id)) => {
                // Even an inactive named member needs a materialized group for the writer.
                let group = leaves.groups.entry(CancelGroupKey { leg, target_id }).or_default();
                add_counts(
                    &mut group.pending_count,
                    &mut group.conflict_count,
                    pending as i64,
                    conflict as i64,
                )?;
            },
            (Some(leg), None) if pending || conflict => {
                leaves
                    .ungroupable
                    .entry(leg)
                    .or_default()
                    .add(pending as i64, conflict as i64)?;
            },
            (None, _) if pending || conflict => {
                leaves
                    .unbound
                    .get_or_insert_with(CancelCounts::default)
                    .add(pending as i64, conflict as i64)?;
            },
            _ => {},
        }
    }

    for (key, group) in &leaves.groups {
        let active = if group.terminal_mask {
            0
        } else {
            group.pending_count
        };
        if active != 0 || group.conflict_count != 0 {
            leaves.legs.entry(key.leg).or_default().add(active, group.conflict_count)?;
        }
    }

    leaves.leaf_count = [
        leaves.members.len(),
        leaves.groups.len(),
        leaves.legs.len(),
        leaves.unbound.is_some() as usize,
        leaves.ungroupable.len(),
    ]
    .into_iter()
    .try_fold(0usize, usize::checked_add)
    .ok_or(CancelRebuildError::LeafLimit)?;
    if leaves.leaf_count > remaining_leaf_rows.min(GENERATION_LEAF_LIMIT) {
        return Err(CancelRebuildError::LeafLimit);
    }
    Ok(leaves)
}
