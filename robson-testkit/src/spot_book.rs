//! One spot balance account and its market, in memory (ADR-0056 step 8, 8d).
//!
//! [`SpotBook`] keeps an account's per-asset `free` and `locked` balances,
//! MARKET orders that execute at once at the symbol's current price, and SELL
//! `STOP_LOSS` orders that lock their base until they execute, are cancelled
//! or expire. It acts only on commands: what a venue is asked (place, cancel,
//! look up, list) and what the market or the venue does on its own (cross a
//! stop, expire it, delay cancels, hide an order or its trades, archive it).
//! No clock decides anything: times come from a counter from a fixed epoch,
//! so the same commands always give the same book.
//!
//! Answers follow Binance spot's documentation at `995f6eb3` of
//! `binance/binance-spot-api-docs`; each one names its source. Rules kept:
//! - **Commissions** (`faqs/commission_faq.md` L141-L151): the fee is charged
//!   on the received amount, so a BUY pays in the base asset (`quantity ×
//!   rate`) and a SELL in the quote asset (`notional × rate`). A scripted
//!   [`Script::Commission`] charges one fill in another asset instead, when
//!   that asset's `free` covers it, and falls back to the default otherwise.
//! - **Terminal precedence:** an order's executed quantity only grows, never
//!   past its original quantity, and a terminal order never changes again. A
//!   delayed cancel overtaken by a fill leaves the order `Filled`.
//! - **One release:** a stop's remaining lock (`orig − executed`) is released
//!   exactly once, at its terminal transition.
//! - **Conservation:** per asset, the account's `free + locked`, the market
//!   counterparty and the fee sink sum to zero. Seeding a balance takes it from
//!   the market, and every trade or fee only moves quantity among them.
//! - **Order limits** (filters.md L253-L272): `MAX_NUM_ORDERS` and
//!   `MAX_NUM_ALGO_ORDERS` count open orders **per symbol**, from that symbol's
//!   [`BookRules`]. Exchange-wide limits are not modeled.
//! - **Bounds:** [`MAX_RETAINED_ORDERS`] orders are retained; past it the
//!   oldest terminal order is evicted with its trades, its client-id entry and
//!   any queued cancel, and if every retained order is live the command panics:
//!   a test-resource error, never a venue answer. Trades, scripts and recorded
//!   events are bounded too.
//!
//! A command that cannot be a venue's answer (a market order before any price
//! is set, an expiry script on a stop, a negative seed) is a test error and
//! panics, naming it.

use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Mutex, MutexGuard},
};

use rust_decimal::Decimal;

/// Orders retained before the oldest terminal one is evicted.
pub const MAX_RETAINED_ORDERS: usize = 4_096;
/// Trades one order may have; a cross past it is refused.
pub const MAX_TRADES_PER_ORDER: usize = 64;
/// Trades retained in all; a cross past it is refused, and a MARKET
/// placement at it is a test-resource error.
pub const MAX_TRADES: usize = 65_536;
/// Scripts queued at once, placement and commission scripts together.
pub const MAX_SCRIPTS: usize = 64;
/// Events kept; older ones are dropped, [`SpotBook::events_total`] counts all.
pub const MAX_EVENTS: usize = 4_096;
/// The most trades one page returns (rest-api.md, `myTrades`: maximum 1000).
pub const MAX_TRADES_PAGE: usize = 1_000;
/// The book's time zero, 2026-01-01T00:00:00Z; each command advances it 1 ms.
pub const EPOCH_MS: i64 = 1_767_225_600_000;

/// An order's side.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Buy,
    Sell,
}

/// What kind of order: executed at once, or resting until its trigger.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OrderKind {
    Market,
    /// A SELL `STOP_LOSS`: its base is locked while it rests.
    StopLoss {
        trigger: Decimal,
    },
}

/// One symbol's trading rules, as `exchangeInfo` gives them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BookRules {
    pub symbol: String,
    pub base: String,
    pub quote: String,
    /// `MAX_NUM_ORDERS`: open orders on this symbol, stops included.
    pub max_num_orders: Option<u32>,
    /// `MAX_NUM_ALGO_ORDERS`: open stops on this symbol.
    pub max_num_algo_orders: Option<u32>,
}

impl BookRules {
    /// A symbol with no order limits.
    pub fn new(symbol: &str, base: &str, quote: &str) -> Self {
        Self {
            symbol: symbol.to_string(),
            base: base.to_string(),
            quote: quote.to_string(),
            max_num_orders: None,
            max_num_algo_orders: None,
        }
    }

    /// The same symbol with both order limits set.
    pub fn with_limits(mut self, max_num_orders: u32, max_num_algo_orders: u32) -> Self {
        self.max_num_orders = Some(max_num_orders);
        self.max_num_algo_orders = Some(max_num_algo_orders);
        self
    }

    /// BTCUSDT with its live limits, 200 orders and 5 stops (the spot
    /// adapter's `exchange_info_btcusdt.json` fixture).
    pub fn btcusdt() -> Self {
        Self::new("BTCUSDT", "BTC", "USDT").with_limits(200, 5)
    }
}

/// An order's status (enums.md L45-L57), derived from its execution and its
/// end: an order the book has not ended is `New` or `PartiallyFilled`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BookStatus {
    New,
    PartiallyFilled,
    Filled,
    Canceled,
    Expired,
}

impl BookStatus {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Filled | Self::Canceled | Self::Expired)
    }

    /// Binance's spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::New => "NEW",
            Self::PartiallyFilled => "PARTIALLY_FILLED",
            Self::Filled => "FILLED",
            Self::Canceled => "CANCELED",
            Self::Expired => "EXPIRED",
        }
    }
}

/// An order as the book holds it now.
#[derive(Clone, Debug, PartialEq)]
pub struct BookOrder {
    pub id: u64,
    pub client_id: String,
    pub symbol: String,
    pub side: Side,
    pub kind: OrderKind,
    pub orig_qty: Decimal,
    /// Cumulative, monotonic, never above `orig_qty`.
    pub executed_qty: Decimal,
    /// Cumulative quote quantity of the executions.
    pub quote_qty: Decimal,
    pub status: BookStatus,
    /// Lookups and `open_orders` show the order.
    pub order_visible: bool,
    /// `trades` shows the order's trades.
    pub trades_visible: bool,
    pub archived: bool,
    pub time_ms: i64,
    pub update_ms: i64,
}

/// One execution.
#[derive(Clone, Debug, PartialEq)]
pub struct BookTrade {
    pub id: u64,
    pub order_id: u64,
    pub symbol: String,
    pub side: Side,
    pub price: Decimal,
    pub qty: Decimal,
    pub quote_qty: Decimal,
    pub commission: Decimal,
    pub commission_asset: String,
    pub time_ms: i64,
}

/// An accepted placement: the order and its fills, as a `FULL` answer
/// carries them.
#[derive(Clone, Debug, PartialEq)]
pub struct Placement {
    pub order: BookOrder,
    pub fills: Vec<BookTrade>,
}

/// What the venue answers instead of an order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BookError {
    /// The request was refused and nothing applied: `-2010`, `-1013`,
    /// `-1121`, or a scripted code.
    Rejected { code: i64, msg: String },
    /// `-2011` "Unknown order sent." (errors.md L484): a cancel of an order
    /// that is not open, or that the venue does not know.
    NotOpen,
    /// `-2013` "Order does not exist." (errors.md L435-L437): a lookup of an
    /// order the venue does not show.
    NoSuchOrder,
    /// `-2026` (errors.md L451-L453): a lookup of an archived order.
    Archived,
    /// The answer is lost. The book has acted (or queued a cancel, or
    /// refused the placement); the caller learns nothing.
    Lost,
}

impl BookError {
    /// The Binance code, where the answer has one.
    pub fn code(&self) -> Option<i64> {
        match self {
            Self::Rejected { code, .. } => Some(*code),
            Self::NotOpen => Some(-2011),
            Self::NoSuchOrder => Some(-2013),
            Self::Archived => Some(-2026),
            Self::Lost => None,
        }
    }
}

/// Why [`SpotBook::cross`] executed nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CrossRefused {
    UnknownOrder,
    /// Only a resting stop is crossed; a MARKET order executes at placement.
    NotAStop,
    Terminal,
    /// A cross executes a positive quantity.
    NotPositive,
    /// `executed + qty` would exceed the original quantity.
    Overfill,
    /// [`MAX_TRADES_PER_ORDER`] or [`MAX_TRADES`] reached.
    TradeBound,
    Overflow,
}

/// How a lookup names its order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OrderRef<'a> {
    Id(u64),
    Client(&'a str),
}

/// What the venue does with the next placement, or with the next fill.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Script {
    /// Refuse the next placement with this code and message.
    Reject { code: i64, msg: String },
    /// Lose the next placement's answer: the book acts (or refuses), the
    /// caller gets [`BookError::Lost`].
    Lost,
    /// The next MARKET order expires with nothing executed.
    ExpireUnfilled,
    /// The next MARKET order executes this much (`0 < qty < orig`), and the
    /// rest expires.
    ExpirePartial(Decimal),
    /// The next fill pays this commission in this asset, from its `free`,
    /// when it covers it; otherwise the default commission applies.
    Commission { asset: String, amount: Decimal },
}

/// One asset's balance on the account.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Balance {
    pub free: Decimal,
    pub locked: Decimal,
}

/// One asset's whole ledger: the account, the market counterparty and the
/// fee sink, which sum to zero.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AssetLedger {
    pub free: Decimal,
    pub locked: Decimal,
    pub market: Decimal,
    pub fees: Decimal,
}

/// What happened, in order.
#[derive(Clone, Debug, PartialEq)]
pub enum BookEvent {
    Seeded {
        asset: String,
        free: Decimal,
    },
    Price {
        symbol: String,
        price: Decimal,
    },
    Placed {
        order_id: u64,
        client_id: String,
    },
    Refused {
        client_id: String,
        code: i64,
    },
    /// A placement's or a cancel's answer was lost; `order_id` is the order
    /// it concerns, if the book holds one.
    AnswerLost {
        order_id: Option<u64>,
    },
    Executed {
        order_id: u64,
        trade_id: u64,
        qty: Decimal,
        price: Decimal,
    },
    CancelQueued {
        order_id: u64,
    },
    /// The order's terminal transition, with the lock it released.
    Ended {
        order_id: u64,
        status: BookStatus,
        unlocked: Decimal,
    },
    Visibility {
        order_id: u64,
        order_visible: bool,
        trades_visible: bool,
    },
    Archived {
        order_id: u64,
    },
    Evicted {
        order_id: u64,
    },
}

/// One spot account and its market; see the module documentation.
pub struct SpotBook {
    inner: Mutex<Inner>,
}

struct Order {
    id: u64,
    client_id: String,
    symbol: String,
    side: Side,
    kind: OrderKind,
    orig: Decimal,
    executed: Decimal,
    quote_qty: Decimal,
    /// The base still locked by a live stop: `orig − executed`, then 0.
    locked: Decimal,
    end: Option<BookStatus>,
    order_visible: bool,
    trades_visible: bool,
    archived: bool,
    trades: Vec<u64>,
    time_ms: i64,
    update_ms: i64,
}

impl Order {
    fn status(&self) -> BookStatus {
        match self.end {
            Some(end) => end,
            None if self.executed > Decimal::ZERO => BookStatus::PartiallyFilled,
            None => BookStatus::New,
        }
    }

    fn live(&self) -> bool {
        self.end.is_none()
    }

    fn is_stop(&self) -> bool {
        matches!(self.kind, OrderKind::StopLoss { .. })
    }

    fn view(&self) -> BookOrder {
        BookOrder {
            id: self.id,
            client_id: self.client_id.clone(),
            symbol: self.symbol.clone(),
            side: self.side,
            kind: self.kind,
            orig_qty: self.orig,
            executed_qty: self.executed,
            quote_qty: self.quote_qty,
            status: self.status(),
            order_visible: self.order_visible,
            trades_visible: self.trades_visible,
            archived: self.archived,
            time_ms: self.time_ms,
            update_ms: self.update_ms,
        }
    }
}

/// Open orders on one symbol.
#[derive(Clone, Copy, Default)]
struct Open {
    orders: u32,
    stops: u32,
}

#[derive(Clone, Copy)]
enum Slot {
    Free,
    Locked,
    Market,
    Fees,
}

/// Balance changes, applied all together or not at all.
#[derive(Default)]
struct Delta(BTreeMap<String, [Decimal; 4]>);

impl Delta {
    fn add(&mut self, asset: &str, slot: Slot, amount: Decimal) -> Option<()> {
        let entry = self.0.entry(asset.to_string()).or_default();
        let at = slot as usize;
        entry[at] = entry[at].checked_add(amount)?;
        Some(())
    }
}

enum CommitError {
    Overflow,
    Negative,
}

/// A placement as the venue receives it.
struct Request<'a> {
    symbol: &'a str,
    side: Side,
    kind: OrderKind,
    qty: Decimal,
    client_id: &'a str,
}

impl Request<'_> {
    fn is_stop(&self) -> bool {
        matches!(self.kind, OrderKind::StopLoss { .. })
    }
}

struct Inner {
    rules: BTreeMap<String, BookRules>,
    fee_rate: Decimal,
    prices: BTreeMap<String, Decimal>,
    balances: BTreeMap<String, Balance>,
    market: BTreeMap<String, Decimal>,
    fees: BTreeMap<String, Decimal>,
    orders: BTreeMap<u64, Order>,
    by_client: BTreeMap<(String, String), u64>,
    trades: BTreeMap<u64, BookTrade>,
    open: BTreeMap<String, Open>,
    sold: BTreeMap<String, Decimal>,
    bought: BTreeMap<String, Decimal>,
    delay_cancels: bool,
    queued: VecDeque<u64>,
    placement_scripts: VecDeque<Script>,
    commission_scripts: VecDeque<(String, Decimal)>,
    events: VecDeque<BookEvent>,
    events_total: u64,
    next_order: u64,
    next_trade: u64,
    now_ms: i64,
}

fn rejected(code: i64, msg: &str) -> BookError {
    BookError::Rejected { code, msg: msg.to_string() }
}

/// errors.md L487.
const INSUFFICIENT: &str = "Account has insufficient balance for requested action.";

impl SpotBook {
    /// A book for these symbols, every balance zero and no price set.
    ///
    /// # Panics
    /// On a duplicate symbol, or a fee rate outside `[0, 1)`.
    pub fn new(rules: Vec<BookRules>, fee_rate: Decimal) -> Self {
        assert!(
            fee_rate >= Decimal::ZERO && fee_rate < Decimal::ONE,
            "SpotBook test error: fee rate {fee_rate} outside [0, 1)"
        );
        let mut by_symbol = BTreeMap::new();
        for rule in rules {
            let symbol = rule.symbol.clone();
            assert!(
                by_symbol.insert(symbol.clone(), rule).is_none(),
                "SpotBook test error: {symbol} given twice"
            );
        }
        Self {
            inner: Mutex::new(Inner {
                rules: by_symbol,
                fee_rate,
                prices: BTreeMap::new(),
                balances: BTreeMap::new(),
                market: BTreeMap::new(),
                fees: BTreeMap::new(),
                orders: BTreeMap::new(),
                by_client: BTreeMap::new(),
                trades: BTreeMap::new(),
                open: BTreeMap::new(),
                sold: BTreeMap::new(),
                bought: BTreeMap::new(),
                delay_cancels: false,
                queued: VecDeque::new(),
                placement_scripts: VecDeque::new(),
                commission_scripts: VecDeque::new(),
                events: VecDeque::new(),
                events_total: 0,
                next_order: 1,
                next_trade: 1,
                now_ms: EPOCH_MS,
            }),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    // The account and the market.

    /// Seed an asset's `free` balance; the difference comes from the market.
    ///
    /// # Panics
    /// On a negative amount.
    pub fn set_balance(&self, asset: &str, free: Decimal) {
        assert!(free >= Decimal::ZERO, "SpotBook test error: negative seed {free} {asset}");
        let mut book = self.lock();
        book.tick();
        let old = book.balances.get(asset).copied().unwrap_or_default().free;
        let mut delta = Delta::default();
        let seeded = delta
            .add(asset, Slot::Free, free - old)
            .and_then(|()| delta.add(asset, Slot::Market, old - free));
        match seeded.map(|()| book.commit(&delta)) {
            Some(Ok(())) => {},
            _ => panic!("SpotBook test error: seeding {free} {asset} overflows"),
        }
        book.record(BookEvent::Seeded { asset: asset.to_string(), free });
    }

    /// The account's balance of an asset (zero if never seeded or traded).
    pub fn balance(&self, asset: &str) -> Balance {
        self.lock().balances.get(asset).copied().unwrap_or_default()
    }

    /// The asset's whole ledger, for conservation checks.
    pub fn ledger(&self, asset: &str) -> AssetLedger {
        let book = self.lock();
        let balance = book.balances.get(asset).copied().unwrap_or_default();
        AssetLedger {
            free: balance.free,
            locked: balance.locked,
            market: book.market.get(asset).copied().unwrap_or_default(),
            fees: book.fees.get(asset).copied().unwrap_or_default(),
        }
    }

    /// Set the symbol's current price: every execution and every trigger
    /// check uses it.
    ///
    /// # Panics
    /// On an unknown symbol or a price that is not positive.
    pub fn set_price(&self, symbol: &str, price: Decimal) {
        assert!(price > Decimal::ZERO, "SpotBook test error: price {price} for {symbol}");
        let mut book = self.lock();
        assert!(book.rules.contains_key(symbol), "SpotBook test error: unknown symbol {symbol}");
        book.tick();
        book.prices.insert(symbol.to_string(), price);
        book.record(BookEvent::Price { symbol: symbol.to_string(), price });
    }

    /// The symbol's current price, if set.
    pub fn price(&self, symbol: &str) -> Option<Decimal> {
        self.lock().prices.get(symbol).copied()
    }

    // What a venue does with a request.

    /// A MARKET order: executes at once at the current price (`Filled`), or
    /// as a script says (`Expired`).
    pub fn place_market(
        &self,
        symbol: &str,
        side: Side,
        qty: Decimal,
        client_id: &str,
    ) -> Result<Placement, BookError> {
        self.lock().place(&Request {
            symbol,
            side,
            kind: OrderKind::Market,
            qty,
            client_id,
        })
    }

    /// A SELL `STOP_LOSS` order: locks `qty` of the base until it ends.
    pub fn place_stop(
        &self,
        symbol: &str,
        qty: Decimal,
        trigger: Decimal,
        client_id: &str,
    ) -> Result<Placement, BookError> {
        let kind = OrderKind::StopLoss { trigger };
        self.lock().place(&Request {
            symbol,
            side: Side::Sell,
            kind,
            qty,
            client_id,
        })
    }

    /// Cancel an order. Open: `Canceled` at once, its remaining lock
    /// released; or, while cancels are delayed, queued and answered
    /// [`BookError::Lost`]. Terminal, unknown or on another symbol:
    /// [`BookError::NotOpen`] (`-2011`, errors.md L484). Visibility does not
    /// matter: the matching engine sees every order.
    pub fn cancel(&self, symbol: &str, order_id: u64) -> Result<BookOrder, BookError> {
        let mut book = self.lock();
        book.tick();
        let Some(order) = book.orders.get(&order_id) else {
            return Err(BookError::NotOpen);
        };
        if order.symbol != symbol || !order.live() {
            return Err(BookError::NotOpen);
        }
        if book.delay_cancels {
            if !book.queued.contains(&order_id) {
                book.queued.push_back(order_id);
                book.record(BookEvent::CancelQueued { order_id });
            }
            book.record(BookEvent::AnswerLost { order_id: Some(order_id) });
            return Err(BookError::Lost);
        }
        book.end(order_id, BookStatus::Canceled);
        Ok(book.orders[&order_id].view())
    }

    /// Look an order up. Unknown, on another symbol, hidden or evicted:
    /// [`BookError::NoSuchOrder`]; archived: [`BookError::Archived`]. By
    /// client id, the latest order placed with it on the symbol.
    pub fn order(&self, symbol: &str, by: OrderRef<'_>) -> Result<BookOrder, BookError> {
        let book = self.lock();
        let id = match by {
            OrderRef::Id(id) => id,
            OrderRef::Client(client_id) => *book
                .by_client
                .get(&(symbol.to_string(), client_id.to_string()))
                .ok_or(BookError::NoSuchOrder)?,
        };
        let order = book.orders.get(&id).ok_or(BookError::NoSuchOrder)?;
        if order.symbol != symbol || !order.order_visible {
            return Err(BookError::NoSuchOrder);
        }
        if order.archived {
            return Err(BookError::Archived);
        }
        Ok(order.view())
    }

    /// Visible open orders, on one symbol or all, oldest first.
    pub fn open_orders(&self, symbol: Option<&str>) -> Vec<BookOrder> {
        let book = self.lock();
        book.orders
            .values()
            .filter(|o| o.live() && o.order_visible && symbol.is_none_or(|s| o.symbol == s))
            .map(Order::view)
            .collect()
    }

    /// Visible trades on a symbol, ascending by id, at most `limit` (clamped
    /// to `1..=1000`): from `from_id` on when given, else the most recent
    /// (rest-api.md, `myTrades`). Optionally one order's only.
    pub fn trades(
        &self,
        symbol: &str,
        order_id: Option<u64>,
        from_id: Option<u64>,
        limit: usize,
    ) -> Vec<BookTrade> {
        let book = self.lock();
        let limit = limit.clamp(1, MAX_TRADES_PAGE);
        let visible = |t: &&BookTrade| {
            t.symbol == symbol
                && order_id.is_none_or(|id| t.order_id == id)
                && book.orders.get(&t.order_id).is_some_and(|o| o.trades_visible)
        };
        match from_id {
            Some(from) => book
                .trades
                .range(from..)
                .map(|(_, t)| t)
                .filter(visible)
                .take(limit)
                .cloned()
                .collect(),
            None => {
                let mut recent: Vec<BookTrade> =
                    book.trades.values().rev().filter(visible).take(limit).cloned().collect();
                recent.reverse();
                recent
            },
        }
    }

    // What the market or the venue does on its own.

    /// The market executes `qty` of a resting stop at the current price.
    /// Refused, with nothing changed, for anything but a positive quantity
    /// that fits a live stop.
    pub fn cross(&self, order_id: u64, qty: Decimal) -> Result<BookTrade, CrossRefused> {
        let mut book = self.lock();
        book.tick();
        let order = book.orders.get(&order_id).ok_or(CrossRefused::UnknownOrder)?;
        if !order.is_stop() {
            return Err(CrossRefused::NotAStop);
        }
        if !order.live() {
            return Err(CrossRefused::Terminal);
        }
        if qty <= Decimal::ZERO {
            return Err(CrossRefused::NotPositive);
        }
        let after = order.executed.checked_add(qty).ok_or(CrossRefused::Overflow)?;
        if after > order.orig {
            return Err(CrossRefused::Overfill);
        }
        if order.trades.len() >= MAX_TRADES_PER_ORDER || book.trades.len() >= MAX_TRADES {
            return Err(CrossRefused::TradeBound);
        }
        let price = book.prices[&order.symbol];
        let trade = book.execute(order_id, qty, price).map_err(|()| CrossRefused::Overflow)?;
        if book.orders[&order_id].executed == book.orders[&order_id].orig {
            book.end(order_id, BookStatus::Filled);
        }
        Ok(trade)
    }

    /// The venue expires a live order, releasing its remaining lock. `false`
    /// when the order is unknown or already terminal (nothing changes).
    pub fn expire(&self, order_id: u64) -> bool {
        let mut book = self.lock();
        book.tick();
        if !book.orders.get(&order_id).is_some_and(Order::live) {
            return false;
        }
        book.end(order_id, BookStatus::Expired);
        true
    }

    /// While on, a cancel of an open order is queued and its answer lost.
    pub fn delay_cancels(&self, on: bool) {
        self.lock().delay_cancels = on;
    }

    /// Apply every queued cancel: a still-open order is `Canceled` and its
    /// lock released; one that ended meanwhile is left as it is. The queue
    /// is emptied, so a second call changes nothing. Returns how many were
    /// cancelled.
    pub fn apply_delayed_cancels(&self) -> usize {
        let mut book = self.lock();
        book.tick();
        let mut cancelled = 0;
        while let Some(order_id) = book.queued.pop_front() {
            if book.orders.get(&order_id).is_some_and(Order::live) {
                book.end(order_id, BookStatus::Canceled);
                cancelled += 1;
            }
        }
        cancelled
    }

    /// Cancels queued and not yet applied.
    pub fn queued_cancels(&self) -> Vec<u64> {
        self.lock().queued.iter().copied().collect()
    }

    /// Lookups answer `-2013` and `open_orders` leaves the order out.
    pub fn hide_order(&self, order_id: u64) -> bool {
        self.lock().set_visibility(order_id, Some(false), None)
    }

    pub fn show_order(&self, order_id: u64) -> bool {
        self.lock().set_visibility(order_id, Some(true), None)
    }

    /// `trades` leaves the order's trades out, independently of the order's
    /// own visibility.
    pub fn withhold_trades(&self, order_id: u64) -> bool {
        self.lock().set_visibility(order_id, None, Some(false))
    }

    pub fn release_trades(&self, order_id: u64) -> bool {
        self.lock().set_visibility(order_id, None, Some(true))
    }

    /// Archive a `Canceled` or `Expired` order with nothing executed
    /// (errors.md L451-L453): its lookups then answer `-2026`. `false`, with
    /// nothing changed, for any other order.
    pub fn archive(&self, order_id: u64) -> bool {
        let mut book = self.lock();
        let eligible = book.orders.get(&order_id).is_some_and(|o| {
            matches!(o.end, Some(BookStatus::Canceled | BookStatus::Expired))
                && o.executed == Decimal::ZERO
                && !o.archived
        });
        if eligible {
            book.tick();
            book.orders.get_mut(&order_id).expect("checked").archived = true;
            book.record(BookEvent::Archived { order_id });
        }
        eligible
    }

    /// Queue a script for the next placement, or the next fill.
    ///
    /// # Panics
    /// Past [`MAX_SCRIPTS`] queued, or on a commission that is negative.
    pub fn script_next(&self, script: Script) {
        let mut book = self.lock();
        let queued = book.placement_scripts.len() + book.commission_scripts.len();
        assert!(
            queued < MAX_SCRIPTS,
            "SpotBook test-resource error: {MAX_SCRIPTS} scripts queued"
        );
        match script {
            Script::Commission { asset, amount } => {
                assert!(amount >= Decimal::ZERO, "SpotBook test error: commission {amount}");
                book.commission_scripts.push_back((asset, amount));
            },
            other => book.placement_scripts.push_back(other),
        }
    }

    // Evidence.

    /// The last [`MAX_EVENTS`] events, oldest first.
    pub fn events(&self) -> Vec<BookEvent> {
        self.lock().events.iter().cloned().collect()
    }

    /// Every event ever recorded, kept or dropped.
    pub fn events_total(&self) -> u64 {
        self.lock().events_total
    }

    /// Every retained order, whatever its visibility, oldest first.
    pub fn all_orders(&self) -> Vec<BookOrder> {
        self.lock().orders.values().map(Order::view).collect()
    }

    /// Trades retained, visible or not.
    pub fn trades_retained(&self) -> usize {
        self.lock().trades.len()
    }

    /// Base quantity the account sold on the symbol, over every execution.
    pub fn sold(&self, symbol: &str) -> Decimal {
        self.lock().sold.get(symbol).copied().unwrap_or_default()
    }

    /// Base quantity the account bought on the symbol.
    pub fn bought(&self, symbol: &str) -> Decimal {
        self.lock().bought.get(symbol).copied().unwrap_or_default()
    }

    /// The book's current time.
    pub fn now_ms(&self) -> i64 {
        self.lock().now_ms
    }
}

impl Inner {
    fn tick(&mut self) {
        self.now_ms += 1;
    }

    fn record(&mut self, event: BookEvent) {
        if self.events.len() == MAX_EVENTS {
            self.events.pop_front();
        }
        self.events.push_back(event);
        self.events_total += 1;
    }

    /// Apply a delta all together, or nothing: `Overflow` if a sum
    /// overflows, `Negative` if a `free` or `locked` would go below zero.
    fn commit(&mut self, delta: &Delta) -> Result<(), CommitError> {
        let mut staged = Vec::with_capacity(delta.0.len());
        for (asset, d) in &delta.0 {
            let balance = self.balances.get(asset).copied().unwrap_or_default();
            let market = self.market.get(asset).copied().unwrap_or_default();
            let fees = self.fees.get(asset).copied().unwrap_or_default();
            let sum = |a: Decimal, b: Decimal| a.checked_add(b).ok_or(CommitError::Overflow);
            let free = sum(balance.free, d[Slot::Free as usize])?;
            let locked = sum(balance.locked, d[Slot::Locked as usize])?;
            if free < Decimal::ZERO || locked < Decimal::ZERO {
                return Err(CommitError::Negative);
            }
            let market = sum(market, d[Slot::Market as usize])?;
            let fees = sum(fees, d[Slot::Fees as usize])?;
            staged.push((asset.clone(), Balance { free, locked }, market, fees));
        }
        for (asset, balance, market, fees) in staged {
            self.balances.insert(asset.clone(), balance);
            self.market.insert(asset.clone(), market);
            self.fees.insert(asset, fees);
        }
        Ok(())
    }

    fn place(&mut self, req: &Request<'_>) -> Result<Placement, BookError> {
        self.tick();
        let script = self.placement_scripts.pop_front();
        if let Some(Script::Reject { code, msg }) = &script {
            let code = *code;
            self.record(BookEvent::Refused {
                client_id: req.client_id.to_string(),
                code,
            });
            return Err(BookError::Rejected { code, msg: msg.clone() });
        }
        let lost = script == Some(Script::Lost);
        let executes = match &script {
            Some(Script::ExpireUnfilled) => Some(Decimal::ZERO),
            Some(Script::ExpirePartial(part)) => {
                assert!(
                    *part > Decimal::ZERO && *part < req.qty,
                    "SpotBook test error: partial expiry {part} of {}",
                    req.qty
                );
                Some(*part)
            },
            _ => None,
        };
        assert!(
            executes.is_none() || !req.is_stop(),
            "SpotBook test error: expiry scripts apply to MARKET orders"
        );
        let placed = self.check(req).and_then(|price| self.apply(req, price, executes));
        match placed {
            Ok(placement) if lost => {
                let order_id = Some(placement.order.id);
                self.record(BookEvent::AnswerLost { order_id });
                Err(BookError::Lost)
            },
            Ok(placement) => Ok(placement),
            Err(refusal) => {
                let code = refusal.code().unwrap_or_default();
                self.record(BookEvent::Refused {
                    client_id: req.client_id.to_string(),
                    code,
                });
                if lost {
                    self.record(BookEvent::AnswerLost { order_id: None });
                    return Err(BookError::Lost);
                }
                Err(refusal)
            },
        }
    }

    /// The venue's checks, in its order: symbol, quantity and trigger
    /// filters, duplicate client id, order limits, the trigger against the
    /// price, the notional, and the balance. Changes nothing; gives the
    /// price the order executes at.
    fn check(&self, req: &Request<'_>) -> Result<Decimal, BookError> {
        let Request { symbol, side, kind, qty, client_id } = *req;
        // errors.md L177-L179.
        let rules = self.rules.get(symbol).ok_or_else(|| rejected(-1121, "Invalid symbol."))?;
        // errors.md L546 and L543.
        if qty <= Decimal::ZERO {
            return Err(rejected(-1013, "Filter failure: LOT_SIZE"));
        }
        if let OrderKind::StopLoss { trigger } = kind {
            assert!(side == Side::Sell, "SpotBook test error: stops are SELL only");
            if trigger <= Decimal::ZERO {
                return Err(rejected(-1013, "Filter failure: PRICE_FILTER"));
            }
        }
        // errors.md L485.
        let key = (symbol.to_string(), client_id.to_string());
        if self
            .by_client
            .get(&key)
            .and_then(|id| self.orders.get(id))
            .is_some_and(Order::live)
        {
            return Err(rejected(-2010, "Duplicate order sent."));
        }
        // filters.md L253-L272, errors.md L552-L553.
        let open = self.open.get(symbol).copied().unwrap_or_default();
        if rules.max_num_orders.is_some_and(|max| open.orders >= max) {
            return Err(rejected(-1013, "Filter failure: MAX_NUM_ORDERS"));
        }
        if req.is_stop() && rules.max_num_algo_orders.is_some_and(|max| open.stops >= max) {
            return Err(rejected(-1013, "Filter failure: MAX_NUM_ALGO_ORDERS"));
        }
        let price = *self
            .prices
            .get(symbol)
            .unwrap_or_else(|| panic!("SpotBook test error: no price set for {symbol}"));
        // errors.md L500.
        if let OrderKind::StopLoss { trigger } = kind {
            if trigger >= price {
                return Err(rejected(-2010, "Order would trigger immediately."));
            }
        }
        // errors.md L547: a notional out of range; here, one that overflows.
        let notional = qty
            .checked_mul(price)
            .ok_or_else(|| rejected(-1013, "Filter failure: NOTIONAL"))?;
        let (asset, needed) = match side {
            Side::Buy => (&rules.quote, notional),
            Side::Sell => (&rules.base, qty),
        };
        if self.balances.get(asset).copied().unwrap_or_default().free < needed {
            return Err(rejected(-2010, INSUFFICIENT));
        }
        Ok(price)
    }

    /// Store an accepted order and act on it: lock a stop's base, or execute
    /// a MARKET order (all of it, or what a script says) and end it.
    fn apply(
        &mut self,
        req: &Request<'_>,
        price: Decimal,
        executes: Option<Decimal>,
    ) -> Result<Placement, BookError> {
        let Request { symbol, side, kind, qty, client_id } = *req;
        if !req.is_stop() && self.trades.len() >= MAX_TRADES {
            panic!("SpotBook test-resource error: {MAX_TRADES} trades retained");
        }
        self.make_room();
        if req.is_stop() {
            let base = self.rules[symbol].base.clone();
            let mut delta = Delta::default();
            let locked = delta
                .add(&base, Slot::Free, -qty)
                .and_then(|()| delta.add(&base, Slot::Locked, qty));
            match locked.map(|()| self.commit(&delta)) {
                Some(Ok(())) => {},
                Some(Err(CommitError::Negative)) => return Err(rejected(-2010, INSUFFICIENT)),
                _ => return Err(rejected(-1013, "Filter failure: NOTIONAL")),
            }
        }
        let id = self.next_order;
        self.next_order += 1;
        let order = Order {
            id,
            client_id: client_id.to_string(),
            symbol: symbol.to_string(),
            side,
            kind,
            orig: qty,
            executed: Decimal::ZERO,
            quote_qty: Decimal::ZERO,
            locked: if req.is_stop() { qty } else { Decimal::ZERO },
            end: None,
            order_visible: true,
            trades_visible: true,
            archived: false,
            trades: Vec::new(),
            time_ms: self.now_ms,
            update_ms: self.now_ms,
        };
        self.orders.insert(id, order);
        let key = (symbol.to_string(), client_id.to_string());
        let previous = self.by_client.insert(key.clone(), id);
        self.record(BookEvent::Placed {
            order_id: id,
            client_id: client_id.to_string(),
        });
        if req.is_stop() {
            let open = self.open.entry(symbol.to_string()).or_default();
            open.orders += 1;
            open.stops += 1;
            return Ok(Placement {
                order: self.orders[&id].view(),
                fills: Vec::new(),
            });
        }
        let executes = executes.unwrap_or(qty);
        let mut fills = Vec::new();
        if executes > Decimal::ZERO {
            match self.execute(id, executes, price) {
                Ok(trade) => fills.push(trade),
                Err(()) => {
                    // The execution overflowed: the order never acted.
                    self.orders.remove(&id);
                    match previous {
                        Some(previous) => self.by_client.insert(key, previous),
                        None => self.by_client.remove(&key),
                    };
                    return Err(rejected(-1013, "Filter failure: NOTIONAL"));
                },
            }
        }
        let end = if executes == qty {
            BookStatus::Filled
        } else {
            BookStatus::Expired
        };
        self.end(id, end);
        Ok(Placement { order: self.orders[&id].view(), fills })
    }

    /// Evict the oldest terminal order when the book is full.
    fn make_room(&mut self) {
        if self.orders.len() < MAX_RETAINED_ORDERS {
            return;
        }
        let Some(oldest) = self.orders.values().find(|o| !o.live()).map(|o| o.id) else {
            panic!(
                "SpotBook test-resource error: retention bound, all {MAX_RETAINED_ORDERS} retained orders are live"
            );
        };
        let order = self.orders.remove(&oldest).expect("found");
        for trade in &order.trades {
            self.trades.remove(trade);
        }
        let key = (order.symbol, order.client_id);
        if self.by_client.get(&key) == Some(&oldest) {
            self.by_client.remove(&key);
        }
        self.queued.retain(|id| *id != oldest);
        self.record(BookEvent::Evicted { order_id: oldest });
    }

    /// Execute `qty` of an order at `price`: balances, commission, trade,
    /// executed and quote quantities. A stop sells from its lock. `Err`,
    /// with nothing changed, when a sum overflows.
    fn execute(&mut self, order_id: u64, qty: Decimal, price: Decimal) -> Result<BookTrade, ()> {
        let order = &self.orders[&order_id];
        let (side, symbol, from_locked) = (order.side, order.symbol.clone(), order.is_stop());
        let rules = self.rules[&symbol].clone();
        let notional = qty.checked_mul(price).ok_or(())?;
        let default = match side {
            Side::Buy => (rules.base.clone(), qty.checked_mul(self.fee_rate).ok_or(())?),
            Side::Sell => (rules.quote.clone(), notional.checked_mul(self.fee_rate).ok_or(())?),
        };
        let mut commission = default.clone();
        let mut committed = false;
        if let Some(scripted) = self.commission_scripts.front().cloned() {
            let delta = fill_delta(&rules, side, from_locked, qty, notional, &scripted).ok_or(())?;
            match self.commit(&delta) {
                Ok(()) => {
                    commission = scripted;
                    committed = true;
                },
                // Not covered: the default commission applies.
                Err(CommitError::Negative) => {},
                Err(CommitError::Overflow) => return Err(()),
            }
        }
        if !committed {
            let delta = fill_delta(&rules, side, from_locked, qty, notional, &default).ok_or(())?;
            match self.commit(&delta) {
                Ok(()) => {},
                Err(CommitError::Overflow) => return Err(()),
                Err(CommitError::Negative) => panic!(
                    "SpotBook invariant broken: order {order_id}'s execution of {qty} found its funds short"
                ),
            }
        }
        // The fill used the next commission script, or fell back from it.
        self.commission_scripts.pop_front();
        let trade = BookTrade {
            id: self.next_trade,
            order_id,
            symbol: symbol.clone(),
            side,
            price,
            qty,
            quote_qty: notional,
            commission: commission.1,
            commission_asset: commission.0,
            time_ms: self.now_ms,
        };
        self.next_trade += 1;
        self.trades.insert(trade.id, trade.clone());
        let now = self.now_ms;
        let order = self.orders.get_mut(&order_id).expect("present");
        order.executed += qty;
        order.quote_qty += notional;
        if from_locked {
            order.locked -= qty;
        }
        order.trades.push(trade.id);
        order.update_ms = now;
        let tally = match side {
            Side::Buy => &mut self.bought,
            Side::Sell => &mut self.sold,
        };
        *tally.entry(symbol).or_default() += qty;
        self.record(BookEvent::Executed { order_id, trade_id: trade.id, qty, price });
        Ok(trade)
    }

    /// The order's one terminal transition: release what a stop still
    /// locks and close it on its symbol.
    fn end(&mut self, order_id: u64, status: BookStatus) {
        let now = self.now_ms;
        let order = self.orders.get_mut(&order_id).expect("present");
        assert!(order.end.is_none(), "SpotBook invariant broken: order {order_id} ended twice");
        let unlocked = std::mem::take(&mut order.locked);
        order.end = Some(status);
        order.update_ms = now;
        let (symbol, is_stop) = (order.symbol.clone(), order.is_stop());
        if is_stop {
            let base = self.rules[&symbol].base.clone();
            let mut delta = Delta::default();
            let released = delta
                .add(&base, Slot::Locked, -unlocked)
                .and_then(|()| delta.add(&base, Slot::Free, unlocked));
            if released.is_none() || self.commit(&delta).is_err() {
                panic!("SpotBook invariant broken: order {order_id} cannot release {unlocked}");
            }
            let open = self.open.get_mut(&symbol).expect("counted at placement");
            open.orders -= 1;
            open.stops -= 1;
        }
        self.record(BookEvent::Ended { order_id, status, unlocked });
    }

    fn set_visibility(&mut self, order_id: u64, order: Option<bool>, trades: Option<bool>) -> bool {
        self.tick();
        let Some(o) = self.orders.get_mut(&order_id) else {
            return false;
        };
        if let Some(visible) = order {
            o.order_visible = visible;
        }
        if let Some(visible) = trades {
            o.trades_visible = visible;
        }
        let event = BookEvent::Visibility {
            order_id,
            order_visible: o.order_visible,
            trades_visible: o.trades_visible,
        };
        self.record(event);
        true
    }
}

/// One execution's balance changes: the account pays one asset and receives
/// the other, the market takes the other side, and the commission moves
/// from the account's `free` to the fee sink.
fn fill_delta(
    rules: &BookRules,
    side: Side,
    from_locked: bool,
    qty: Decimal,
    notional: Decimal,
    commission: &(String, Decimal),
) -> Option<Delta> {
    let mut delta = Delta::default();
    match side {
        Side::Buy => {
            delta.add(&rules.quote, Slot::Free, -notional)?;
            delta.add(&rules.quote, Slot::Market, notional)?;
            delta.add(&rules.base, Slot::Market, -qty)?;
            delta.add(&rules.base, Slot::Free, qty)?;
        },
        Side::Sell => {
            let from = if from_locked {
                Slot::Locked
            } else {
                Slot::Free
            };
            delta.add(&rules.base, from, -qty)?;
            delta.add(&rules.base, Slot::Market, qty)?;
            delta.add(&rules.quote, Slot::Market, -notional)?;
            delta.add(&rules.quote, Slot::Free, notional)?;
        },
    }
    delta.add(&commission.0, Slot::Free, -commission.1)?;
    delta.add(&commission.0, Slot::Fees, commission.1)?;
    Some(delta)
}
