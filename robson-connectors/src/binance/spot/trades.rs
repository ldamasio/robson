//! `GET /api/v3/myTrades` paging (ADR-0056 step 8a; F16).
//!
//! Evidence only, never finality: a trade read can be cut short by the page
//! bound, and trades can lag their order. Line numbers are in `rest-api.md`
//! at `995f6eb3`.
//!
//! - **By order:** every page is `symbol + orderId + fromId` (L4763), the first
//!   from `fromId` 0 (weight 5 each). Without `fromId` Binance returns the most
//!   recent trades (L4754), so an order with more fills than a page would lose
//!   its oldest ones.
//! - **By window:** the first page is `symbol + startTime + endTime`, which
//!   returns the oldest trades of the window: "With both, behaves like
//!   `startTime`", which "returns oldest items from `startTime`" (L98-L102).
//!   That general rule is read as unchanged by the endpoint's "Otherwise most
//!   recent trades are returned" (L4754), which then covers a request with
//!   neither `fromId` nor a time [NV]. `fromId` cannot be combined with a time
//!   (L4756-L4763), so later pages are `symbol + fromId` and stop at the first
//!   trade past `endTime` (weight 20 each). This takes trade ids to rise with
//!   time within a symbol [R].
//!
//! Each page asks for [`MY_TRADES_PAGE_LIMIT`] trades and continues from the
//! highest id it saw plus one; a short page ends the read. One read sends at
//! most [`MY_TRADES_MAX_PAGES`] pages, then returns a cursor.

use std::collections::{btree_map::Entry, BTreeMap};

use rust_decimal::Decimal;
use serde::Deserialize;

use super::rest::{parse_json, SpotRead, SpotReadError};

/// Trades asked for per page: Binance's maximum (L4747).
pub const MY_TRADES_PAGE_LIMIT: u16 = 1000;

/// Pages one trades read sends before it returns a cursor (design note v2
/// §3.1).
pub const MY_TRADES_MAX_PAGES: u32 = 5;

/// Longest window one read may span: "The time between `startTime` and
/// `endTime` can't be longer than 24 hours." (L4755).
pub const MY_TRADES_MAX_WINDOW_MS: i64 = 24 * 60 * 60 * 1000;

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
    /// Trades with `start_ms <= time <= end_ms`, a window of at most
    /// [`MY_TRADES_MAX_WINDOW_MS`]; `from_id` continues an earlier read.
    Window {
        start_ms: i64,
        end_ms: i64,
        from_id: Option<u64>,
    },
}

impl SpotTradesQuery {
    /// Refuse, before sending, a window Binance would refuse.
    pub fn validate(&self) -> Result<(), SpotReadError> {
        if let Self::Window { start_ms, end_ms, .. } = *self {
            if start_ms < 0 || end_ms < start_ms || end_ms - start_ms > MY_TRADES_MAX_WINDOW_MS {
                return Err(SpotReadError::InvalidQuery(format!(
                    "trade window {start_ms}..={end_ms} ms is not within 0..=24 h"
                )));
            }
        }
        Ok(())
    }

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
            Self::Window { start_ms, end_ms, from_id: None } => (SpotRead::MY_TRADES, vec![
                symbol,
                ("startTime", start_ms.to_string()),
                ("endTime", end_ms.to_string()),
                limit,
            ]),
            Self::Window { from_id: Some(from_id), .. } => {
                (SpotRead::MY_TRADES, vec![symbol, ("fromId", from_id.to_string()), limit])
            },
        }
    }

    fn continued_from(self, next_id: u64) -> Self {
        match self {
            Self::Order { order_id, .. } => Self::Order { order_id, from_id: next_id },
            Self::Window { start_ms, end_ms, .. } => {
                Self::Window { start_ms, end_ms, from_id: Some(next_id) }
            },
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
    /// records. A trade outside the window is not this read's.
    pub(super) fn absorb(
        &mut self,
        query: SpotTradesQuery,
        body: &str,
    ) -> Result<Option<SpotTradesQuery>, SpotReadError> {
        let page: Vec<SpotTrade> = parse_json("trades", body)?;
        let full = page.len() >= usize::from(MY_TRADES_PAGE_LIMIT);
        let mut past_window = false;
        let mut highest = None;
        for trade in page {
            if trade.symbol != self.pair {
                return Err(SpotReadError::Inconsistent(format!(
                    "trades for {} include trade {} on {}",
                    self.pair, trade.id, trade.symbol
                )));
            }
            highest = highest.max(Some(trade.id));
            match query {
                SpotTradesQuery::Order { order_id, .. } if trade.order_id != order_id => {
                    return Err(SpotReadError::Inconsistent(format!(
                        "trades of order {order_id} include trade {} of order {}",
                        trade.id, trade.order_id
                    )));
                },
                SpotTradesQuery::Window { start_ms, end_ms, .. } => {
                    if trade.time > end_ms {
                        past_window = true;
                        continue;
                    }
                    if trade.time < start_ms {
                        continue;
                    }
                },
                SpotTradesQuery::Order { .. } => {},
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
        // A short page, or one that ran past the window, was the last.
        if !full || past_window {
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
