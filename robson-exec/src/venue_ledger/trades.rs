//! Pure bounded trade-page planning. Existing fills are supplied by their
//! venue/scope/symbol trade identity, including fills assigned to another
//! order.
use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use uuid::Uuid;

use super::{contribution, exact_add, exact_sub, LedgerError, OrderKey, StoredOrder, VenueIntId};
use crate::{ports::TradeFill, venue::TradePage};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredTrade {
    pub order: OrderKey,
    pub tenant_id: Uuid,
    pub trade_id: VenueIntId,
    pub price: Decimal,
    pub qty: Decimal,
    pub quote_qty: Decimal,
    pub commission: Decimal,
    pub commission_asset: String,
    pub trade_time: DateTime<Utc>,
    /// Local observation time is audit metadata, never economic identity.
    pub recorded_at: DateTime<Utc>,
}

impl StoredTrade {
    fn agrees(&self, incoming: &Self) -> bool {
        self.order == incoming.order
            && self.tenant_id == incoming.tenant_id
            && self.trade_id == incoming.trade_id
            && self.price == incoming.price
            && self.qty == incoming.qty
            && self.quote_qty == incoming.quote_qty
            && self.commission == incoming.commission
            && self.commission_asset == incoming.commission_asset
            && self.trade_time == incoming.trade_time
    }

    fn incoming(
        order: &OrderKey,
        tenant_id: Uuid,
        fill: &TradeFill,
        recorded_at: DateTime<Utc>,
    ) -> Result<Self, LedgerError> {
        if fill.price <= Decimal::ZERO
            || fill.qty <= Decimal::ZERO
            || fill.quote_qty < Decimal::ZERO
            || fill.commission < Decimal::ZERO
            || fill.commission_asset.is_empty()
        {
            return Err(LedgerError::InvalidEvidence(
                "invalid trade amounts or commission asset".into(),
            ));
        }
        Ok(Self {
            order: order.clone(),
            tenant_id,
            trade_id: fill.trade_id.parse()?,
            price: fill.price,
            qty: fill.qty,
            quote_qty: fill.quote_qty,
            commission: fill.commission,
            commission_asset: fill.commission_asset.clone(),
            trade_time: fill
                .time
                .ok_or_else(|| LedgerError::InvalidEvidence("trade has no venue time".into()))?,
            recorded_at,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TradeWriteResult {
    pub changed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TradePlan {
    pub order: StoredOrder,
    pub new_fills: Vec<StoredTrade>,
    pub consumed_delta: Decimal,
    pub changed: bool,
}

/// At most one page (1,000 trades) and one corresponding existing-fill set.
/// The store checks tenant ownership of the order before calling this function.
/// Rechecking under the accounting/order lock uses this identical planner.
pub fn plan_trades(
    stored: &StoredOrder,
    tenant_id: Uuid,
    page: &TradePage,
    existing: &[StoredTrade],
    recorded_at: DateTime<Utc>,
) -> Result<TradePlan, LedgerError> {
    if page.trades.len() > 1_000 || existing.len() > 1_000 {
        return Err(LedgerError::InvalidEvidence("trade page exceeds 1000 rows".into()));
    }
    let mut landed = HashMap::with_capacity(existing.len());
    for fill in existing {
        if fill.order.venue != stored.key.venue
            || fill.order.account_scope != stored.key.account_scope
            || fill.order.symbol != stored.key.symbol
            || landed.insert(fill.trade_id, fill).is_some()
        {
            return Err(LedgerError::Invariant("unexpected existing trade identity".into()));
        }
    }
    let mut ids = HashSet::with_capacity(page.trades.len());
    let mut new_fills = Vec::with_capacity(page.trades.len());
    let mut max_id: Option<u64> = None;
    let mut added = Decimal::ZERO;
    for fill in &page.trades {
        let incoming = StoredTrade::incoming(&stored.key, tenant_id, fill, recorded_at)?;
        if !ids.insert(incoming.trade_id) {
            return Err(LedgerError::InvalidEvidence("duplicate trade id within page".into()));
        }
        max_id = Some(max_id.map_or(incoming.trade_id.0, |id| id.max(incoming.trade_id.0)));
        match landed.get(&incoming.trade_id) {
            Some(previous) if previous.agrees(&incoming) => (),
            Some(_) => return Err(LedgerError::TradeConflict),
            None => {
                added = exact_add(added, incoming.qty)?;
                new_fills.push(incoming);
            },
        }
    }
    let next = match page.next_from_id {
        Some(next) => Some(next),
        None => max_id
            .map(|id| {
                id.checked_add(1).ok_or_else(|| {
                    LedgerError::InvalidEvidence("trade cursor overflows u64".into())
                })
            })
            .transpose()?,
    };
    let next = next
        .map(i64::try_from)
        .transpose()
        .map_err(|_| LedgerError::InvalidEvidence("trade cursor exceeds i64".into()))?;
    let mut order = stored.clone();
    order.trades_cursor = order.trades_cursor.max(next.unwrap_or(order.trades_cursor));
    order.fills_qty = exact_add(order.fills_qty, added)?;
    if order.fills_qty > order.orig_qty
        || (order.terminal() && order.fills_qty > order.executed_cum)
    {
        return Err(LedgerError::TradeConflict);
    }
    if !order.terminal() {
        order.executed_cum = order.executed_cum.max(order.fills_qty);
    }
    let changed = !new_fills.is_empty() || order.trades_cursor != stored.trades_cursor;
    if changed {
        order.last_checked_at = order.last_checked_at.max(recorded_at);
    }
    let consumed_delta = exact_sub(contribution(&order), contribution(stored))?;
    if consumed_delta < Decimal::ZERO {
        return Err(LedgerError::Invariant("trade contribution decreased".into()));
    }
    Ok(TradePlan {
        order,
        new_fills,
        consumed_delta,
        changed,
    })
}

#[cfg(test)]
mod tests;
