//! `GET /api/v3/myTrades` paging (ADR-0056 step 8a; F16).
//!
//! Evidence only, never finality: a trade read can be cut short by the page
//! bound, and trades can lag their order. Line numbers are in `rest-api.md`
//! at `995f6eb3`.
//!
//! Every request names `fromId`, an inclusive lower bound on the trade id
//! (L4754). Without it Binance returns the most recent trades, so a scan whose
//! first page lacked it could hold only the newest of more trades than a page
//! and end with the older ones missing.
//! - **By order:** `symbol + orderId + fromId` (L4763), the first page from
//!   `fromId` 0 (weight 5 each).
//! - **From an id:** `symbol + fromId` (L4761), seeded by the caller with an
//!   explicit lower bound, such as the trade after the last one it recorded
//!   (weight 20 each). Any time filter is the caller's.
//!
//! Each page asks for [`MY_TRADES_PAGE_LIMIT`] trades and continues from the
//! highest id it saw plus one; only a short page ends the read. One read sends
//! at most [`MY_TRADES_MAX_PAGES`] pages, then returns a cursor.
//!
//! There is deliberately no time-window read (`startTime`/`endTime`). The
//! docs disagree on which trades its first page holds: the oldest of the
//! window by the general rule (L98-L102), the most recent by this endpoint's
//! (L4754). Under the second, a window with more trades than a page would end
//! with its oldest trades silently missing. Adding one needs authoritative
//! evidence of that first-page contract first.

use std::collections::{btree_map::Entry, BTreeMap};

use rust_decimal::Decimal;
use serde::Deserialize;

use super::rest::{parse_json, SpotRead, SpotReadError};

/// Trades asked for per page: Binance's maximum (L4747).
pub const MY_TRADES_PAGE_LIMIT: u16 = 1000;

/// Pages one trades read sends before it returns a cursor (design note v2
/// §3.1).
pub const MY_TRADES_MAX_PAGES: u32 = 5;

/// One account trade (L4765-L4783; `fromId` paging, L4754).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpotTrade {
    pub symbol: String,
    /// Trade id, unique within the symbol.
    pub id: u64,
    pub order_id: u64,
    pub order_list_id: i64,
    pub price: Decimal,
    pub qty: Decimal,
    pub quote_qty: Decimal,
    pub commission: Decimal,
    pub commission_asset: String,
    /// Trade time, ms since the epoch.
    pub time: i64,
    pub is_buyer: bool,
    pub is_maker: bool,
    pub is_best_match: bool,
}

/// Which trades a read asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpotTradesQuery {
    /// One order's trades with ids from `from_id`: 0 for the whole order, a
    /// cursor's value to continue.
    Order { order_id: u64, from_id: u64 },
    /// Every trade on the symbol with an id from `from_id` on: the caller's
    /// explicit lower bound, or a cursor's value to continue.
    FromId { from_id: u64 },
}

impl SpotTradesQuery {
    /// The read and parameters of this query's next page.
    pub(super) fn page_request(&self, pair: &str) -> (SpotRead, Vec<(&'static str, String)>) {
        let symbol = ("symbol", pair.to_string());
        let limit = ("limit", MY_TRADES_PAGE_LIMIT.to_string());
        match *self {
            Self::Order { order_id, from_id } => (SpotRead::MY_TRADES_BY_ORDER, vec![
                symbol,
                ("orderId", order_id.to_string()),
                ("fromId", from_id.to_string()),
                limit,
            ]),
            Self::FromId { from_id } => {
                (SpotRead::MY_TRADES, vec![symbol, ("fromId", from_id.to_string()), limit])
            },
        }
    }

    fn continued_from(self, next_id: u64) -> Self {
        match self {
            Self::Order { order_id, .. } => Self::Order { order_id, from_id: next_id },
            Self::FromId { .. } => Self::FromId { from_id: next_id },
        }
    }
}

/// What one trades read found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpotTradesRead {
    /// Distinct trades, ascending by id.
    pub trades: Vec<SpotTrade>,
    /// The query that continues this read when the page bound stopped it
    /// with a full last page; `None` when the venue had no more.
    pub next: Option<SpotTradesQuery>,
    /// Pages sent.
    pub pages: u32,
    /// Their documented weight.
    pub weight: u32,
}

/// One page of an order's trades, as one request established it (8c, design
/// §3.5.1): what the leg owner reads, one page per run, its cursor stored
/// durably by the caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpotTradePage {
    /// Distinct trades, ascending by id.
    pub trades: Vec<SpotTrade>,
    /// Where the next page starts, `highest id + 1`, only when this page was
    /// full; `None` when the venue had no more.
    pub next_from_id: Option<u64>,
}

/// One page answered to `orderId = order_id, fromId = from_id`. Each trade
/// is on `pair`, of the order, with an id at or above `from_id`; an id read
/// twice must carry the same record, and counts once. A full page's cursor
/// must exist and move past `from_id`. Anything else is
/// [`SpotReadError::Inconsistent`].
pub(super) fn parse_page(
    pair: &str,
    order_id: u64,
    from_id: u64,
    body: &str,
) -> Result<SpotTradePage, SpotReadError> {
    let page: Vec<SpotTrade> = parse_json("trades", body)?;
    let full = page.len() >= usize::from(MY_TRADES_PAGE_LIMIT);
    let mut trades = BTreeMap::new();
    for trade in page {
        let defect = if trade.symbol != pair {
            Some(format!("is on {}", trade.symbol))
        } else if trade.order_id != order_id {
            Some(format!("is of order {}", trade.order_id))
        } else if trade.id < from_id {
            Some(format!("is below fromId {from_id}"))
        } else {
            None
        };
        if let Some(defect) = defect {
            return Err(SpotReadError::Inconsistent(format!(
                "trade {} on a page of order {order_id} on {pair} {defect}",
                trade.id
            )));
        }
        match trades.entry(trade.id) {
            Entry::Vacant(slot) => {
                slot.insert(trade);
            },
            Entry::Occupied(seen) if *seen.get() == trade => {},
            Entry::Occupied(_) => {
                return Err(SpotReadError::Inconsistent(format!(
                    "trade {} on {pair} read twice with different records",
                    trade.id
                )));
            },
        }
    }
    let next_from_id = if full {
        let next = trades.keys().next_back().and_then(|highest| highest.checked_add(1));
        match next {
            Some(next) if next > from_id => Some(next),
            _ => {
                return Err(SpotReadError::Inconsistent(format!(
                    "a full page of order {order_id} on {pair} from {from_id} has no next cursor"
                )))
            },
        }
    } else {
        None
    };
    Ok(SpotTradePage {
        trades: trades.into_values().collect(),
        next_from_id,
    })
}

/// Trades collected across the pages of one read, deduplicated by id.
pub(super) struct SpotTradeSet {
    pair: String,
    trades: BTreeMap<u64, SpotTrade>,
}

impl SpotTradeSet {
    pub(super) fn new(pair: String) -> Self {
        Self { pair, trades: BTreeMap::new() }
    }

    /// Take one page answered to `query`; the query that continues it, or
    /// `None` when it was the last.
    ///
    /// A trade on another symbol or, by order, of another order is
    /// [`SpotReadError::Inconsistent`], and so is one id with two different
    /// records.
    pub(super) fn absorb(
        &mut self,
        query: SpotTradesQuery,
        body: &str,
    ) -> Result<Option<SpotTradesQuery>, SpotReadError> {
        let page: Vec<SpotTrade> = parse_json("trades", body)?;
        let full = page.len() >= usize::from(MY_TRADES_PAGE_LIMIT);
        let mut highest = None;
        for trade in page {
            if trade.symbol != self.pair {
                return Err(SpotReadError::Inconsistent(format!(
                    "trades for {} include trade {} on {}",
                    self.pair, trade.id, trade.symbol
                )));
            }
            highest = highest.max(Some(trade.id));
            if let SpotTradesQuery::Order { order_id, .. } = query {
                if trade.order_id != order_id {
                    return Err(SpotReadError::Inconsistent(format!(
                        "trades of order {order_id} include trade {} of order {}",
                        trade.id, trade.order_id
                    )));
                }
            }
            match self.trades.entry(trade.id) {
                Entry::Vacant(slot) => {
                    slot.insert(trade);
                },
                Entry::Occupied(seen) if *seen.get() == trade => {},
                Entry::Occupied(_) => {
                    return Err(SpotReadError::Inconsistent(format!(
                        "trade {} on {} read twice with different records",
                        trade.id, self.pair
                    )));
                },
            }
        }
        // Only a short page is the last: a full one may have more after it.
        if !full {
            return Ok(None);
        }
        let Some(highest) = highest else {
            return Ok(None);
        };
        let next_id = highest.checked_add(1).ok_or_else(|| {
            SpotReadError::Inconsistent(format!("trade id {highest} has no successor"))
        })?;
        Ok(Some(query.continued_from(next_id)))
    }

    pub(super) fn finish(
        self,
        next: Option<SpotTradesQuery>,
        pages: u32,
        weight: u32,
    ) -> SpotTradesRead {
        SpotTradesRead {
            trades: self.trades.into_values().collect(),
            next,
            pages,
            weight,
        }
    }
}
