//! Pure order evidence merge. No journal, database or venue call happens here.

use chrono::{DateTime, NaiveDate, Utc};
use robson_domain::{OrderSide, PositionId};
use rust_decimal::Decimal;
use uuid::Uuid;

use super::{
    exact_add, exact_sub, LedgerError, LegOrderKind, OrderLink, OrderOwner, SeenAt, VenueIntId,
};
use crate::venue::{OrderPhase, ProtectionKind, VenueId, VenueOrderRecord};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct OrderKey {
    pub venue: VenueId,
    pub account_scope: String,
    pub symbol: String,
    pub exchange_order_id: VenueIntId,
}

impl OrderKey {
    pub fn from_record(record: &VenueOrderRecord, link: &OrderLink) -> Result<Self, LedgerError> {
        Ok(Self {
            venue: link.venue.clone(),
            account_scope: link.account_scope.clone(),
            symbol: record.symbol.clone(),
            exchange_order_id: record.exchange_order_id.parse()?,
        })
    }
}

/// The immutable first observations and mutable evidence of a durable order.
/// Tenant isolation is checked by the store before constructing this value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredOrder {
    pub key: OrderKey,
    pub owner: OrderOwner,
    pub client_order_id: String,
    pub last_client_order_id: String,
    pub side: OrderSide,
    pub order_type: String,
    pub orig_qty: Decimal,
    pub stop_price: Option<Decimal>,
    pub status: String,
    pub phase: OrderPhase,
    pub executed_cum: Decimal,
    pub cumulative_quote: Option<Decimal>,
    pub fills_qty: Decimal,
    pub trades_cursor: i64,
    pub last_checked_at: DateTime<Utc>,
    pub live_seen: SeenAt,
    pub terminal_seen: Option<SeenAt>,
    pub expiry_reason: Option<String>,
    pub venue_time_ms: Option<i64>,
    pub venue_update_time_ms: Option<i64>,
    pub venue_transact_time_ms: Option<i64>,
    pub venue_working_time_ms: Option<i64>,
}

impl StoredOrder {
    pub fn terminal(&self) -> bool {
        self.phase == OrderPhase::Terminal
    }

    pub fn fills_complete(&self) -> bool {
        self.fills_qty == self.executed_cum
    }
}

/// The incoming (or already linked) leg, read together with the order. The
/// store verifies tenant identity before passing it to the pure merge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderLeg {
    pub position_id: PositionId,
    pub venue: VenueId,
    pub account_scope: String,
    pub symbol: String,
    pub admission_intent_id: Uuid,
    pub decided_month: NaiveDate,
    pub cohort_month: Option<NaiveDate>,
    pub consumed_cum: Decimal,
    pub closed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderAnomaly {
    ExecutionRegressed,
    TerminalReadAsLive,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderWriteResult {
    pub changed: bool,
    pub anomalies: Vec<OrderAnomaly>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderMerge {
    pub order: StoredOrder,
    pub changed: bool,
    pub consumed_delta: Decimal,
    pub set_cohort: bool,
    pub anomalies: Vec<OrderAnomaly>,
}

/// Only a stop or sell owned by a leg consumes that leg's inventory.
pub fn contribution(order: &StoredOrder) -> Decimal {
    match order.owner {
        OrderOwner::Leg {
            kind: LegOrderKind::Stop | LegOrderKind::Sell,
            ..
        } => order.executed_cum,
        _ => Decimal::ZERO,
    }
}

/// Normalize before insertion AND comparison. A venue's zero placeholder is
/// not a trigger, but only known non-trigger types receive that interpretation.
fn stop_price(record: &VenueOrderRecord) -> Result<Option<Decimal>, LedgerError> {
    match record.stop_price {
        Some(value)
            if value.is_zero()
                && record.protection_kind.is_none()
                && matches!(record.order_type.as_str(), "MARKET" | "LIMIT" | "LIMIT_MAKER") =>
        {
            Ok(None)
        },
        Some(value) if value <= Decimal::ZERO => {
            Err(LedgerError::InvalidEvidence("non-positive trigger".into()))
        },
        value => Ok(value),
    }
}

fn validate_leg(
    order: &StoredOrder,
    leg: Option<&OrderLeg>,
    adopting: bool,
) -> Result<(), LedgerError> {
    let OrderOwner::Leg { intent_id, position_id, kind } = order.owner else {
        return Ok(());
    };
    let leg = leg.ok_or(LedgerError::IdentityConflict)?;
    if leg.position_id != position_id
        || leg.venue != order.key.venue
        || leg.account_scope != order.key.account_scope
        || leg.symbol != order.key.symbol
        || (adopting && (kind == LegOrderKind::Entry || leg.closed_at.is_some()))
    {
        return Err(LedgerError::IdentityConflict);
    }
    let shape = match kind {
        LegOrderKind::Entry => {
            order.side == OrderSide::Buy
                && order.order_type == "MARKET"
                && intent_id == leg.admission_intent_id
        },
        LegOrderKind::Stop => {
            order.side == OrderSide::Sell
                && order.order_type == "STOP_LOSS"
                && order.stop_price.is_some()
        },
        LegOrderKind::Sell => order.side == OrderSide::Sell && order.order_type == "MARKET",
    };
    if !shape {
        return Err(LedgerError::IdentityConflict);
    }
    Ok(())
}

/// Merge typed venue evidence without weakening any first-seen or terminal
/// field. The store repeats this decision under its accounting/order locks.
/// Audit fields may advance on an anomalous live observation; financial and
/// terminal facts cannot regress. A replay with no new evidence keeps rotation
/// and the accounting version unchanged, even when SeenAt is later.
pub fn merge_order(
    stored: Option<&StoredOrder>,
    record: &VenueOrderRecord,
    link: &OrderLink,
    seen: &SeenAt,
    leg: Option<&OrderLeg>,
) -> Result<OrderMerge, LedgerError> {
    if record.orig_qty <= Decimal::ZERO
        || !record.quantities_consistent()
        || record.cumulative_quote_qty.is_some_and(|q| q < Decimal::ZERO)
        || record.symbol.is_empty()
        || record.status.is_empty()
    {
        return Err(LedgerError::InvalidEvidence("invalid order quantities or identity".into()));
    }
    let key = OrderKey::from_record(record, link)?;
    let stop_price = stop_price(record)?;
    let mut anomalies = Vec::with_capacity(2);
    let mut adopting = false;
    let mut order = if let Some(old) = stored {
        if old.key != key
            || old.side != record.side
            || old.order_type != record.order_type
            || old.orig_qty != record.orig_qty
            || old.stop_price != stop_price
        {
            return Err(LedgerError::IdentityConflict);
        }
        let mut order = old.clone();
        match (&old.owner, &link.owner) {
            (OrderOwner::Leg { .. }, OrderOwner::Leg { .. }) if old.owner != link.owner => {
                return Err(LedgerError::IdentityConflict)
            },
            (OrderOwner::Leg { .. }, _) => {}, // A non-leg link supplies no new ownership
            // information.
            (_, OrderOwner::Leg { .. }) => {
                order.owner = link.owner.clone();
                adopting = true;
            },
            _ if old.owner != link.owner => return Err(LedgerError::IdentityConflict),
            _ => {},
        }
        if record.phase == OrderPhase::Terminal && record.executed_qty < old.executed_cum {
            return Err(LedgerError::TradeConflict);
        }
        let execution_regressed = record.executed_qty < old.executed_cum;
        if execution_regressed {
            anomalies.push(OrderAnomaly::ExecutionRegressed);
        }
        if old.terminal() {
            if record.phase != OrderPhase::Terminal {
                anomalies.push(OrderAnomaly::TerminalReadAsLive);
            } else {
                if record.status != old.status {
                    return Err(LedgerError::IdentityConflict);
                }
                if record.executed_qty != old.executed_cum
                    || matches!((old.cumulative_quote,record.cumulative_quote_qty),(Some(a),Some(b)) if a != b)
                {
                    return Err(LedgerError::TradeConflict);
                }
            }
            // Terminal execution, status, quote, expiry and clock evidence are
            // the first terminal tuple, even if later evidence fills an
            // unknown.
        } else {
            if !execution_regressed {
                order.executed_cum = record.executed_qty;
                order.cumulative_quote = record.cumulative_quote_qty.or(old.cumulative_quote);
                order.phase = record.phase;
                order.status = record.status.clone();
                if record.phase == OrderPhase::Terminal {
                    order.terminal_seen = Some(seen.clone());
                }
            }
            order.expiry_reason =
                record.expiry_reason.clone().or_else(|| old.expiry_reason.clone());
        }
        order.last_client_order_id = record.client_order_id.clone();
        order.venue_time_ms = record.time_ms.or(old.venue_time_ms);
        order.venue_update_time_ms = record.update_time_ms.or(old.venue_update_time_ms);
        order.venue_transact_time_ms = record.transact_time_ms.or(old.venue_transact_time_ms);
        order.venue_working_time_ms = record.working_time_ms.or(old.venue_working_time_ms);
        order
    } else {
        if link.client_order_id != record.client_order_id
            && record.orig_client_order_id.as_deref() != Some(link.client_order_id.as_str())
        {
            return Err(LedgerError::IdentityConflict);
        }
        StoredOrder {
            key,
            owner: link.owner.clone(),
            client_order_id: link.client_order_id.clone(),
            last_client_order_id: record.client_order_id.clone(),
            side: record.side,
            order_type: record.order_type.clone(),
            orig_qty: record.orig_qty,
            stop_price,
            status: record.status.clone(),
            phase: record.phase,
            executed_cum: record.executed_qty,
            cumulative_quote: record.cumulative_quote_qty,
            fills_qty: Decimal::ZERO,
            trades_cursor: 0,
            last_checked_at: seen.at,
            live_seen: seen.clone(),
            terminal_seen: (record.phase == OrderPhase::Terminal).then(|| seen.clone()),
            expiry_reason: record.expiry_reason.clone(),
            venue_time_ms: record.time_ms,
            venue_update_time_ms: record.update_time_ms,
            venue_transact_time_ms: record.transact_time_ms,
            venue_working_time_ms: record.working_time_ms,
        }
    };
    validate_leg(&order, leg, adopting)?;
    if matches!(order.owner, OrderOwner::Leg { kind: LegOrderKind::Stop, .. })
        && record.protection_kind != Some(ProtectionKind::MarketOnTrigger)
    {
        return Err(LedgerError::IdentityConflict);
    }
    let consumed_delta =
        exact_sub(contribution(&order), stored.map_or(Decimal::ZERO, contribution))?;
    if consumed_delta < Decimal::ZERO {
        return Err(LedgerError::Invariant("negative order contribution".into()));
    }
    if consumed_delta > Decimal::ZERO {
        exact_add(leg.ok_or(LedgerError::IdentityConflict)?.consumed_cum, consumed_delta)?;
    }
    let set_cohort = matches!(order.owner, OrderOwner::Leg { kind: LegOrderKind::Entry, .. })
        && leg.is_some_and(|leg| leg.cohort_month.is_none());
    let changed = stored != Some(&order) || set_cohort;
    if changed {
        order.last_checked_at = stored.map_or(seen.at, |old| old.last_checked_at.max(seen.at));
    }
    Ok(OrderMerge {
        order,
        changed,
        consumed_delta,
        set_cohort,
        anomalies,
    })
}

#[cfg(test)]
mod tests;
