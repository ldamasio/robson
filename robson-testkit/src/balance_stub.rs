//! A balance account in process (ADR-0056 step 8, 8d): [`BalanceStub`]
//! answers the spot venue's port contracts (8c) over a [`SpotBook`], with no
//! wire.
//!
//! - **Capabilities** are exactly the spot adapter's: `Spot`, `BalanceAccount`,
//!   no short, no reduce-only flag, `MarketOnTrigger`, `CancelThenPlace`, no
//!   funding, `EveryAttemptRejected`, one attempt per client id.
//! - **Malformed values** are refused first, as the adapter refuses them, with
//!   nothing recorded, no grant taken and nothing sent: a symbol that is not
//!   one value (ASCII only as letters and digits), for placements and cancels,
//!   and a client id outside `^[a-zA-Z0-9-_]{1,36}$`.
//! - **Placements** pass the one-attempt ledger
//!   ([`SubmitAttemptLedger::one_attempt`]): its send start takes the grant the
//!   attached journal gave the order, so nothing reaches the book without a
//!   grant and nothing twice under one client id. Then the gate, then the book.
//!   [`BalanceStub::sends`] counts what reached the book. A refused `-2010` is
//!   `OrderRejected` and `-1013` `RulesRejected`, the ledger's only rejections;
//!   a lost answer and any other code are uncertain and leave the attempt
//!   unresolved; a MARKET order that expired with nothing executed is
//!   [`ExecError::ExpiredUnfilled`], never a rejection.
//! - **Cancels** take no grant and pass the gate. `-2011` is
//!   [`CancelAnswer::NotFound`], a delayed cancel's lost answer
//!   [`CancelAnswer::Unknown`].
//! - **Evidence** is 8c's, under the adapter's own rules
//!   ([`crate::spot_evidence`]): every result carries the book's record of the
//!   order; a MARKET answer's fills are checked one by one and as a set, and
//!   unusable fills leave the result with no fills and no fee total; a
//!   composite's trades, read as the adapter reads them (at most five pages,
//!   stopping once they cover the execution), give fills only when each is
//!   valid and together they are exactly the execution, never undoing the
//!   record's terminality; a lookup's not-found is classified only by the
//!   finality rule over the ledger's evidence.
//! - **Refused like spot**, with nothing sent: trades by time, governed
//!   inventory, costs and settlement ([`ExecError::Config`]).
//!
//! It is test support: no production crate depends on this crate.

use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering},
        Arc, Mutex, OnceLock,
    },
};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use robson_domain::{OrderSide, Price, Quantity, Symbol, SymbolTradingRules};
use robson_exec::{
    gate::OperationGate,
    order_journal::OrderJournal,
    ports::{FeeEvidence, OpenOrderRecord, OrderEvidence, TradeFill, UserTradeRecord},
    quantize,
    venue::{
        replace_cancel_then_place, AccountKind, AccountOpenOrder, AccountPort, CancelAnswer, Cash,
        EntryOrReduce, ExecutionCostEnvelope, FundingPort, GovernedInventory, Holding,
        InventoryObservation, InventoryPort, Mark, MarketKind, OrderLookup, OrderObservation,
        OrderPhase, OrderRef, PlacedStop, PositionAccountPort, ProtectionKind, ReplaceOutcome,
        ReplacementProtocol, SettlementPort, SettlementRecord, StopCancelOutcome, StopParams,
        StopRef, SubmitAttemptLedger, TradePage, TradingPort, Venue, VenueCapabilities,
        VenueFinalityRule, VenueId, VenueOrderRecord,
    },
    ExecError, OrderResult,
};
use rust_decimal::Decimal;

use crate::{
    spot_book::{
        BookError, BookOrder, BookStatus, BookTrade, OrderKind, OrderRef as BookRef, Placement,
        Side as BookSide, SpotBook,
    },
    spot_evidence::{market_evidence, terminal_result, MarketFill},
};

/// The stub's venue id.
pub const BALANCE_STUB_VENUE: &str = "stub:balance";
/// Client ids the attempt ledger tracks, as on the spot adapter.
const SUBMIT_ATTEMPTS_CAPACITY: usize = 4096;
/// Trades per page, Binance's maximum (rest-api.md, `myTrades`).
pub const TRADES_PAGE_LIMIT: usize = 1000;
/// The most trade pages a composite reads for one order, as on the adapter.
const COMPOSITE_TRADE_PAGES: usize = 5;

/// The spot adapter's capabilities (8c), which the stub shares.
pub fn spot_capabilities() -> VenueCapabilities {
    VenueCapabilities {
        market_kind: MarketKind::Spot,
        account_kind: AccountKind::BalanceAccount,
        supports_short: false,
        reduce_only_wire_flag: false,
        protection: ProtectionKind::MarketOnTrigger,
        replacement: ReplacementProtocol::CancelThenPlace,
        has_funding: false,
        order_finality: VenueFinalityRule::EveryAttemptRejected,
        one_attempt_per_client_id: true,
    }
}

/// A spot balance account in process, over a [`SpotBook`]; see the module
/// documentation.
pub struct BalanceStub {
    book: Arc<SpotBook>,
    venue_id: VenueId,
    ledger: SubmitAttemptLedger,
    gate: OnceLock<Arc<OperationGate>>,
    /// Placements that passed the ledger and the gate and reached the book.
    sends: AtomicU64,
    rules: Mutex<HashMap<String, SymbolTradingRules>>,
    page_limit: AtomicUsize,
    /// Trade pages served, each one request on the adapter.
    pages_read: AtomicU64,
}

/// Which answer a record comes from: each carries the times the venue's
/// does.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Answer {
    Placement,
    Cancel,
    Lookup,
}

fn side(side: BookSide) -> OrderSide {
    match side {
        BookSide::Buy => OrderSide::Buy,
        BookSide::Sell => OrderSide::Sell,
    }
}

/// The book's order as the venue's record of it.
fn record(order: &BookOrder, answer: Answer) -> VenueOrderRecord {
    let (order_type, stop_price) = match order.kind {
        OrderKind::Market => ("MARKET", None),
        OrderKind::StopLoss { trigger } => ("STOP_LOSS", Some(trigger)),
    };
    let lookup = answer == Answer::Lookup;
    VenueOrderRecord {
        exchange_order_id: order.id.to_string(),
        client_order_id: order.client_id.clone(),
        orig_client_order_id: (answer == Answer::Cancel).then(|| order.client_id.clone()),
        symbol: order.symbol.clone(),
        side: side(order.side),
        order_type: order_type.to_string(),
        protection_kind: stop_price.map(|_| ProtectionKind::MarketOnTrigger),
        status: order.status.as_str().to_string(),
        phase: if order.status.is_terminal() {
            OrderPhase::Terminal
        } else {
            OrderPhase::Live
        },
        orig_qty: order.orig_qty,
        executed_qty: order.executed_qty,
        cumulative_quote_qty: Some(order.quote_qty),
        stop_price,
        expiry_reason: None,
        transact_time_ms: (!lookup).then_some(order.update_ms),
        time_ms: lookup.then_some(order.time_ms),
        update_time_ms: lookup.then_some(order.update_ms),
        working_time_ms: None,
    }
}

fn time(ms: i64) -> DateTime<Utc> {
    DateTime::from_timestamp_millis(ms).expect("the book's clock starts in 2026")
}

/// A page's trade as fill evidence, with its time.
fn trade_fill(trade: &BookTrade) -> TradeFill {
    TradeFill {
        trade_id: trade.id.to_string(),
        price: trade.price,
        qty: trade.qty,
        quote_qty: trade.quote_qty,
        commission: trade.commission,
        commission_asset: trade.commission_asset.clone(),
        time: Some(time(trade.time_ms)),
    }
}

/// An answer that established nothing: the order may exist.
fn uncertain(why: impl std::fmt::Display) -> ExecError {
    ExecError::Exchange(format!("{BALANCE_STUB_VENUE} order uncertain, it may exist: {why}"))
}

/// The ledger's rejections: the venue's final refusal of this attempt.
fn is_final_rejection(error: &ExecError) -> bool {
    matches!(error, ExecError::OrderRejected(_) | ExecError::RulesRejected(_))
}

/// A placement the book refused or lost, typed as the spot adapter types it.
fn placement_error(error: BookError) -> ExecError {
    match error {
        BookError::Rejected { code: -2010, msg } => {
            ExecError::OrderRejected(format!("-2010: {msg}"))
        },
        BookError::Rejected { code: -1013, msg } => {
            ExecError::RulesRejected(format!("-1013: {msg}"))
        },
        other => uncertain(format!("{other:?}")),
    }
}

/// A MARKET order's answer (8c plan §4): `FILLED`, or `EXPIRED` with
/// execution, is the result of its execution, its fills and fee under the
/// adapter's evidence rules ([`market_evidence`]); an expiry with nothing
/// executed is [`ExecError::ExpiredUnfilled`].
fn market_result(placement: Placement) -> Result<OrderResult, ExecError> {
    let order = &placement.order;
    let record = record(order, Answer::Placement);
    let executed = order.executed_qty;
    match order.status {
        BookStatus::Filled | BookStatus::Expired if executed > Decimal::ZERO => {},
        BookStatus::Expired => return Err(ExecError::ExpiredUnfilled(Box::new(record))),
        status => return Err(uncertain(format!("MARKET order {} is {status:?}", order.id))),
    }
    let fills: Vec<MarketFill> = placement
        .fills
        .iter()
        .map(|trade| MarketFill {
            price: trade.price,
            qty: trade.qty,
            commission: trade.commission,
            commission_asset: trade.commission_asset.clone(),
            trade_id: trade.id,
        })
        .collect();
    let evidence = market_evidence(record.cumulative_quote_qty, Some(&fills), executed, None)
        .map_err(|why| uncertain(format!("MARKET order {}: {why}", order.id)))?;
    Ok(OrderResult {
        exchange_order_id: record.exchange_order_id.clone(),
        client_order_id: record.client_order_id.clone(),
        fill_price: Price::new(evidence.price)?,
        filled_quantity: Quantity::new(executed)?,
        fee: evidence.fee,
        fee_asset: evidence.fee_asset,
        filled_at: time(order.update_ms),
        evidence: Some(Box::new(OrderEvidence {
            fills: evidence.fills,
            record: Some(record),
            fee_evidence: evidence.fee_evidence,
        })),
    })
}

/// A stop's answer (8c plan §4): whatever its status, the stop with its
/// record, which alone says whether it rests.
fn stop_result(placement: Placement, symbol: &Symbol, trigger: Price) -> PlacedStop {
    let order = &placement.order;
    let record = record(order, Answer::Placement);
    PlacedStop {
        stop: StopRef::new(record.exchange_order_id.clone()),
        acceptance: OrderResult {
            exchange_order_id: record.exchange_order_id.clone(),
            client_order_id: record.client_order_id.clone(),
            fill_price: trigger,
            filled_quantity: Quantity::zero(),
            fee: Decimal::ZERO,
            fee_asset: symbol.quote().to_string(),
            filled_at: time(order.update_ms),
            evidence: Some(Box::new(OrderEvidence {
                fills: Vec::new(),
                record: Some(record),
                fee_evidence: FeeEvidence::Total,
            })),
        },
    }
}

/// What a record says of the order's lifecycle. A lookup gives no fills.
fn order_lookup(record: &VenueOrderRecord) -> OrderLookup {
    let filled_qty = record.executed_qty;
    match record.phase {
        OrderPhase::Live if filled_qty.is_zero() => OrderLookup::Open { filled_qty },
        OrderPhase::Live => OrderLookup::PartiallyFilled { filled_qty },
        OrderPhase::Terminal => OrderLookup::Terminal {
            filled_qty,
            cancelled_qty: record.orig_qty - filled_qty,
            fills: None,
        },
        OrderPhase::Unknown => OrderLookup::Unknown,
    }
}

/// The legacy open-order view of a record. Spot has no reduce-only flag.
fn open_order(record: &VenueOrderRecord) -> Result<OpenOrderRecord, ExecError> {
    let stop_price = record.stop_price.map(Price::new).transpose()?;
    Ok(OpenOrderRecord {
        exchange_order_id: record.exchange_order_id.clone(),
        client_order_id: record.client_order_id.clone(),
        order_type: record.order_type.clone(),
        reduce_only: false,
        stop_price,
        side: record.side,
    })
}

/// A venue order id, refused locally (nothing sent) when it is not one.
fn order_id(id: &str) -> Result<u64, ExecError> {
    id.parse().map_err(|_| {
        ExecError::Config(format!(
            "{BALANCE_STUB_VENUE} order id {id:?} is not a number; nothing was sent"
        ))
    })
}

/// Binance's documented client id pattern, `^[a-zA-Z0-9-_]{1,36}$`, as the
/// spot adapter checks it before the ledger.
fn valid_client_order_id(id: &str) -> bool {
    (1..=36).contains(&id.len())
        && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Whether `pair` can go as one symbol value, as the adapter checks it: not
/// empty, and ASCII only as letters and digits (non-ASCII characters pass:
/// Binance documents them in symbol names and the transport encodes them).
fn valid_symbol(pair: &str) -> bool {
    !pair.is_empty() && pair.chars().all(|c| !c.is_ascii() || c.is_ascii_alphanumeric())
}

/// A request value the adapter refuses before anything is recorded or sent
/// (its `malformed`): a symbol that is not one value, then a client id
/// outside Binance's pattern.
fn malformed(symbol: &Symbol, client_order_id: Option<&str>) -> Option<ExecError> {
    let pair = symbol.as_pair();
    if !valid_symbol(&pair) {
        return Some(refused(format!("symbol {pair:?} is not one value")));
    }
    let id = client_order_id?;
    (!valid_client_order_id(id))
        .then(|| refused(format!("client order id {id:?} is not ^[a-zA-Z0-9-_]{{1,36}}$")))
}

fn refused(what: String) -> ExecError {
    ExecError::Config(format!("{BALANCE_STUB_VENUE} {what}; nothing was sent"))
}

impl BalanceStub {
    /// The stub over `book`. Its ledger refuses every send until a
    /// one-attempt journal is attached.
    pub fn new(book: Arc<SpotBook>) -> Self {
        Self {
            book,
            venue_id: VenueId::new(BALANCE_STUB_VENUE),
            ledger: SubmitAttemptLedger::one_attempt(SUBMIT_ATTEMPTS_CAPACITY),
            gate: OnceLock::new(),
            sends: AtomicU64::new(0),
            rules: Mutex::new(HashMap::new()),
            page_limit: AtomicUsize::new(TRADES_PAGE_LIMIT),
            pages_read: AtomicU64::new(0),
        }
    }

    pub fn book(&self) -> &SpotBook {
        &self.book
    }

    pub fn ledger(&self) -> &SubmitAttemptLedger {
        &self.ledger
    }

    /// Placements that reached the book.
    pub fn sends(&self) -> u64 {
        self.sends.load(Ordering::SeqCst)
    }

    /// Trade pages served, by `order_trades_page` and the composites.
    pub fn pages_read(&self) -> u64 {
        self.pages_read.load(Ordering::SeqCst)
    }

    /// The rules every rules read serves for `rules.symbol()`.
    pub fn set_trading_rules(&self, rules: SymbolTradingRules) {
        let pair = rules.symbol().as_pair();
        self.rules.lock().unwrap_or_else(|e| e.into_inner()).insert(pair, rules);
    }

    /// Trades per page, for tests of the cursor ([`TRADES_PAGE_LIMIT`] by
    /// default).
    pub fn set_trades_page_limit(&self, limit: usize) {
        self.page_limit.store(limit.max(1), Ordering::SeqCst);
    }

    fn rules_for(&self, symbol: &Symbol) -> Result<SymbolTradingRules, ExecError> {
        let rules = self.rules.lock().unwrap_or_else(|e| e.into_inner());
        rules.get(&symbol.as_pair()).cloned().ok_or_else(|| {
            ExecError::Exchange(format!(
                "{BALANCE_STUB_VENUE} has no rules for {}",
                symbol.as_pair()
            ))
        })
    }

    /// The gate's choke point, as the transport passes it before every
    /// signed mutation. A refusal sends nothing.
    fn mutate(&self, what: &str) -> Result<(), ExecError> {
        match self.gate.get() {
            Some(gate) => gate.mutation(what).map_err(|r| ExecError::SendRefused(r.to_string())),
            None => Ok(()),
        }
    }

    /// One placement: the symbol and the client id refused when malformed,
    /// with nothing recorded and no grant taken; then the ledger (the
    /// grant), the gate, the book and `read`, which types the answer inside
    /// the ledger, as the adapter's does: only the answer's final rejections
    /// are the attempt's.
    async fn send<T>(
        &self,
        symbol: &Symbol,
        client_order_id: &str,
        place: impl FnOnce() -> Result<Placement, BookError> + Send,
        read: impl FnOnce(Placement) -> Result<T, ExecError> + Send,
    ) -> Result<T, ExecError> {
        if let Some(refusal) = malformed(symbol, Some(client_order_id)) {
            return Err(refusal);
        }
        self.ledger
            .submit(client_order_id, is_final_rejection, async {
                self.mutate("POST /api/v3/order")?;
                self.sends.fetch_add(1, Ordering::SeqCst);
                place().map_err(placement_error).and_then(read)
            })
            .await
    }

    /// The fills rule (8c plan §4) over at most five trade pages from id 0,
    /// as the adapter reads them: reading stops as soon as the trades read
    /// cover the execution, a full page or not, and [`terminal_result`]
    /// decides. `None` with no read when it is not terminal or nothing
    /// executed, and when a read fails or five pages fall short.
    async fn terminal_fills(
        &self,
        symbol: &Symbol,
        record: &VenueOrderRecord,
    ) -> Option<OrderResult> {
        if record.phase != OrderPhase::Terminal || record.executed_qty <= Decimal::ZERO {
            return None;
        }
        let mut trades = Vec::new();
        let mut covered = Decimal::ZERO;
        let mut from_id = 0;
        for _ in 0..COMPOSITE_TRADE_PAGES {
            let page =
                self.order_trades_page(symbol, &record.exchange_order_id, from_id).await.ok()?;
            for trade in &page.trades {
                covered = covered.checked_add(trade.qty)?;
            }
            trades.extend(page.trades);
            match page.next_from_id {
                Some(next) if covered < record.executed_qty => from_id = next,
                _ => return terminal_result(record, trades, None),
            }
        }
        None
    }

    /// One lookup by exchange id, then the fills rule (the adapter's
    /// `executed_order`).
    async fn executed_order(
        &self,
        symbol: &Symbol,
        order_id: &str,
        protective: bool,
    ) -> Result<Option<OrderResult>, ExecError> {
        let by = OrderRef::ExchangeId(order_id.to_string());
        let record = match self.observe_order(symbol, &by).await? {
            OrderObservation::Found(record) => record,
            OrderObservation::NotFound | OrderObservation::Archived => return Ok(None),
        };
        if protective && record.protection_kind.is_none() {
            return Err(ExecError::Exchange(format!(
                "{BALANCE_STUB_VENUE} order {order_id} is a {}, not a protective stop",
                record.order_type
            )));
        }
        if record.phase != OrderPhase::Terminal || record.executed_qty <= Decimal::ZERO {
            return Ok(None);
        }
        match self.terminal_fills(symbol, &record).await {
            Some(result) => Ok(Some(result)),
            None => Err(ExecError::Exchange(format!(
                "{BALANCE_STUB_VENUE} order {order_id} executed {} but its trades were not \
                 established",
                record.executed_qty
            ))),
        }
    }
}

#[async_trait]
impl TradingPort for BalanceStub {
    async fn trading_rules(&self, symbol: &Symbol) -> Result<SymbolTradingRules, ExecError> {
        self.rules_for(symbol)
    }

    fn last_validated_rules(&self, symbol: &Symbol) -> Option<SymbolTradingRules> {
        self.rules_for(symbol).ok()
    }

    /// Spot holds no short: an entry buys and a reduction sells. An entry is
    /// checked against the rules; a reduction goes as given. One placement
    /// through the ledger.
    async fn place_market_order(
        &self,
        symbol: &Symbol,
        side: OrderSide,
        quantity: Quantity,
        client_order_id: &str,
        intent: EntryOrReduce,
    ) -> Result<OrderResult, ExecError> {
        if matches!(
            (intent, side),
            (EntryOrReduce::Entry, OrderSide::Sell) | (EntryOrReduce::Reduce, OrderSide::Buy)
        ) {
            return Err(refused(format!("has no short: a {intent:?} cannot {side:?}")));
        }
        let quantity = match intent {
            EntryOrReduce::Entry => quantize::market_quantity(&self.rules_for(symbol)?, quantity)?,
            EntryOrReduce::Reduce => quantity,
        };
        let book_side = match side {
            OrderSide::Buy => BookSide::Buy,
            OrderSide::Sell => BookSide::Sell,
        };
        let (pair, qty) = (symbol.as_pair(), quantity.as_decimal());
        let place = || self.book.place_market(&pair, book_side, qty, client_order_id);
        self.send(symbol, client_order_id, place, market_result).await
    }

    /// One `STOP_LOSS` SELL through the ledger. `Ok` whatever its status: the
    /// record says whether it rests. A BUY is refused.
    async fn place_protection(
        &self,
        symbol: &Symbol,
        close_side: OrderSide,
        quantity: Quantity,
        trigger: Price,
        client_order_id: &str,
    ) -> Result<PlacedStop, ExecError> {
        if close_side != OrderSide::Sell {
            return Err(refused("protection sells what it holds: a BUY stop".to_string()));
        }
        let pair = symbol.as_pair();
        let (qty, stop) = (quantity.as_decimal(), trigger.as_decimal());
        let place = || self.book.place_stop(&pair, qty, stop, client_order_id);
        let read = |placed| Ok(stop_result(placed, symbol, trigger));
        self.send(symbol, client_order_id, place, read).await
    }

    /// The adapter's composite: one cancel; after a not-found, one lookup;
    /// then the fills rule.
    async fn cancel_protection(
        &self,
        symbol: &Symbol,
        stop: &StopRef,
    ) -> Result<StopCancelOutcome, ExecError> {
        match self.cancel_order_once(symbol, &stop.exchange_order_id).await? {
            CancelAnswer::Cancelled(record) => Ok(StopCancelOutcome::Cancelled {
                filled_before_cancel: record.executed_qty,
                fills: self.terminal_fills(symbol, &record).await,
            }),
            CancelAnswer::NotFound { .. } => {
                let by = OrderRef::ExchangeId(stop.exchange_order_id.clone());
                let final_state = match self.observe_order(symbol, &by).await {
                    Ok(OrderObservation::Found(record)) => match order_lookup(&record) {
                        OrderLookup::Terminal { filled_qty, cancelled_qty, .. } => {
                            OrderLookup::Terminal {
                                filled_qty,
                                cancelled_qty,
                                fills: self.terminal_fills(symbol, &record).await,
                            }
                        },
                        other => other,
                    },
                    _ => OrderLookup::Unknown,
                };
                Ok(StopCancelOutcome::AlreadyGone { final_state })
            },
            CancelAnswer::Unknown { .. } => Ok(StopCancelOutcome::Unknown),
        }
    }

    /// Cancel-then-place through [`replace_cancel_then_place`], as on spot.
    async fn replace_protection(
        &self,
        symbol: &Symbol,
        old: &StopRef,
        new: StopParams,
    ) -> Result<ReplaceOutcome, ExecError> {
        replace_cancel_then_place(self, &self.ledger, symbol, old, new).await
    }

    /// The legacy cancel: `Ok` only when the answer shows it cancelled.
    async fn cancel_order(&self, symbol: &Symbol, order_id: &str) -> Result<(), ExecError> {
        match self.cancel_order_once(symbol, order_id).await? {
            CancelAnswer::Cancelled(_) => Ok(()),
            CancelAnswer::NotFound { code } => Err(ExecError::OrderRejected(format!(
                "{BALANCE_STUB_VENUE} cancel of {order_id} rejected ({code}); the order's \
                 lifecycle is not established"
            ))),
            CancelAnswer::Unknown { reason } => Err(ExecError::Exchange(format!(
                "{BALANCE_STUB_VENUE} cancel of {order_id} uncertain: {reason}"
            ))),
        }
    }

    /// One lookup by client id. A not-found is read through the finality
    /// rule over the ledger's evidence, read after the answer; an archived
    /// order is `Unknown`, never `Absent`.
    async fn order_by_client_id(
        &self,
        symbol: &Symbol,
        client_order_id: &str,
    ) -> Result<OrderLookup, ExecError> {
        match self.book.order(&symbol.as_pair(), BookRef::Client(client_order_id)) {
            Ok(order) => Ok(order_lookup(&record(&order, Answer::Lookup))),
            Err(BookError::NoSuchOrder) => Ok(VenueFinalityRule::EveryAttemptRejected
                .classify_not_found(self.ledger.final_evidence(client_order_id).await)),
            Err(BookError::Archived) => Ok(OrderLookup::Unknown),
            Err(other) => Err(uncertain(format!("{other:?}"))),
        }
    }

    async fn order_by_exchange_id(
        &self,
        symbol: &Symbol,
        order_id: &str,
    ) -> Result<Option<OrderResult>, ExecError> {
        self.executed_order(symbol, order_id, false).await
    }

    async fn protection_fill(
        &self,
        symbol: &Symbol,
        stop: &StopRef,
    ) -> Result<Option<OrderResult>, ExecError> {
        self.executed_order(symbol, &stop.exchange_order_id, true).await
    }

    /// The symbol's open `STOP_LOSS` SELL orders, as the venue lists them.
    async fn open_protective_orders(
        &self,
        symbol: &Symbol,
    ) -> Result<Vec<OpenOrderRecord>, ExecError> {
        let open = self.book.open_orders(Some(&symbol.as_pair()));
        let records = open.iter().map(|order| record(order, Answer::Lookup));
        records
            .filter(|r| r.protection_kind.is_some() && r.side == OrderSide::Sell)
            .map(|r| open_order(&r))
            .collect()
    }

    /// Refused: spot trades are read by order, never by time.
    async fn user_trades_since(
        &self,
        symbol: &Symbol,
        _since: DateTime<Utc>,
        _limit: u16,
    ) -> Result<Vec<UserTradeRecord>, ExecError> {
        Err(refused(format!("reads no trades by time for {}", symbol.as_pair())))
    }

    async fn price(&self, symbol: &Symbol) -> Result<Price, ExecError> {
        let price = self.book.price(&symbol.as_pair()).ok_or_else(|| {
            ExecError::Exchange(format!(
                "{BALANCE_STUB_VENUE} has no price for {}",
                symbol.as_pair()
            ))
        })?;
        Ok(Price::new(price)?)
    }

    async fn health(&self) -> Result<(), ExecError> {
        Ok(())
    }
}

#[async_trait]
impl AccountPort for BalanceStub {
    async fn cash(&self, asset: &str) -> Result<Cash, ExecError> {
        let balance = self.book.balance(asset);
        Ok(Cash {
            free: balance.free,
            locked: balance.locked,
            as_of: Utc::now(),
        })
    }

    async fn valuation(&self, symbol: &Symbol) -> Result<Mark, ExecError> {
        Ok(Mark {
            price: self.price(symbol).await?,
            as_of: Utc::now(),
        })
    }

    /// Refused, as on spot until its costs (8g).
    async fn execution_costs(&self, symbol: &Symbol) -> Result<ExecutionCostEnvelope, ExecError> {
        Err(refused(format!("has no cost envelope for {}", symbol.as_pair())))
    }

    fn capabilities(&self) -> VenueCapabilities {
        spot_capabilities()
    }
}

#[async_trait]
impl InventoryPort for BalanceStub {
    /// Every nonzero holding and every visible open order with its record.
    async fn account_observation(&self) -> Result<InventoryObservation, ExecError> {
        let holdings = self.book.balances().into_iter().map(|(asset, balance)| {
            let held = Holding {
                free: balance.free,
                locked: balance.locked,
            };
            (asset, held)
        });
        let mut open_orders = Vec::new();
        for order in self.book.open_orders(None) {
            let record = record(&order, Answer::Lookup);
            open_orders.push(AccountOpenOrder {
                symbol: Symbol::from_pair(&record.symbol)?,
                order: open_order(&record)?,
                record: Some(record),
            });
        }
        Ok(InventoryObservation {
            holdings: holdings.collect(),
            open_orders,
            as_of: Utc::now(),
            open_order_lists: 0,
            uid: BALANCE_STUB_VENUE.to_string(),
            can_trade: true,
            account_type: "SPOT".to_string(),
            permissions: vec!["SPOT".to_string()],
        })
    }

    /// Refused, as on spot until its ledger (8e).
    async fn governed_inventory(&self, symbol: &Symbol) -> Result<GovernedInventory, ExecError> {
        Err(refused(format!("has no governed inventory for {}", symbol.as_pair())))
    }

    /// One cancel: no grant, the gate, then the book. `-2011` is `NotFound`
    /// and proves nothing about execution; a delayed cancel is `Unknown`.
    async fn cancel_order_once(
        &self,
        symbol: &Symbol,
        order_id_text: &str,
    ) -> Result<CancelAnswer, ExecError> {
        let id = order_id(order_id_text)?;
        if let Some(refusal) = malformed(symbol, None) {
            return Err(refusal);
        }
        self.mutate("DELETE /api/v3/order")?;
        Ok(match self.book.cancel(&symbol.as_pair(), id) {
            Ok(order) => CancelAnswer::Cancelled(Box::new(record(&order, Answer::Cancel))),
            Err(BookError::NotOpen) => CancelAnswer::NotFound { code: -2011 },
            Err(other) => CancelAnswer::Unknown {
                reason: format!("cancel of {id}: {other:?}"),
            },
        })
    }

    /// One lookup: the order's own record, or `NotFound` (`-2013`) or
    /// `Archived` (`-2026`).
    async fn observe_order(
        &self,
        symbol: &Symbol,
        by: &OrderRef,
    ) -> Result<OrderObservation, ExecError> {
        let found = match by {
            OrderRef::ExchangeId(id) => {
                self.book.order(&symbol.as_pair(), BookRef::Id(order_id(id)?))
            },
            OrderRef::ClientId(id) => self.book.order(&symbol.as_pair(), BookRef::Client(id)),
        };
        match found {
            Ok(order) => Ok(OrderObservation::Found(Box::new(record(&order, Answer::Lookup)))),
            Err(BookError::NoSuchOrder) => Ok(OrderObservation::NotFound),
            Err(BookError::Archived) => Ok(OrderObservation::Archived),
            Err(other) => Err(uncertain(format!("{other:?}"))),
        }
    }

    /// One page of the order's visible trades from `from_id`, each with its
    /// time; a full page's cursor is its highest id plus one (8c-2).
    async fn order_trades_page(
        &self,
        symbol: &Symbol,
        order_id_text: &str,
        from_id: u64,
    ) -> Result<TradePage, ExecError> {
        let id = order_id(order_id_text)?;
        self.pages_read.fetch_add(1, Ordering::SeqCst);
        let limit = self.page_limit.load(Ordering::SeqCst);
        let trades = self.book.trades(&symbol.as_pair(), Some(id), Some(from_id), limit);
        let next_from_id = match trades.last() {
            Some(last) if trades.len() >= limit => last.id.checked_add(1),
            _ => None,
        };
        Ok(TradePage {
            trades: trades.iter().map(trade_fill).collect(),
            next_from_id,
        })
    }
}

#[async_trait]
impl SettlementPort for BalanceStub {
    /// Refused, as on spot until its settlement (8l).
    async fn settlement_since(
        &self,
        _since: DateTime<Utc>,
        _limit: u16,
    ) -> Result<Vec<SettlementRecord>, ExecError> {
        Err(refused("has no settlement evidence".to_string()))
    }
}

impl Venue for BalanceStub {
    fn id(&self) -> &VenueId {
        &self.venue_id
    }

    fn attach_order_journal(&self, journal: Arc<OrderJournal>) {
        self.ledger.attach_journal(journal);
    }

    fn attach_gate(&self, gate: Arc<OperationGate>) {
        let _ = self.gate.set(gate);
    }

    fn position_account(&self) -> Option<&dyn PositionAccountPort> {
        None
    }

    fn inventory(&self) -> Option<&dyn InventoryPort> {
        Some(self)
    }

    fn settlement(&self) -> &dyn SettlementPort {
        self
    }

    fn funding(&self) -> Option<&dyn FundingPort> {
        None
    }
}
