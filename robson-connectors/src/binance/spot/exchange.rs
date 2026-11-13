//! The Binance spot venue, `binance:spot` (ADR-0056 step 8, PRs 8c-2, 8c-3a
//! and 8c-3b): the adapter over the shared REST client, its rules cache, the
//! read side of the venue ports, order placement and cancels.
//!
//! Built here: trading rules through [`SpotRulesCache`]; the venue's order
//! records ([`VenueOrderRecord`], phase from status alone); account and
//! inventory reads; `order_by_client_id`, `open_protective_orders`, and the
//! single-request `observe_order`, `order_trades_page` and
//! `cancel_order_once` the leg owner uses (8c plan, C1). Every read is one of
//! 8a's [`SpotRestReader`] reads at its documented weight, one request, no
//! retry. `place_market_order` and `place_protection` send one order each
//! through `trading.rs`, under the one-attempt ledger (8c-3a); a cancel goes
//! through `trading.rs` too, with no grant (8c-3b).
//!
//! The composite wrappers keep the legacy `TradingPort` contract within
//! documented bounds, and no owner path may use them (C3): `cancel_protection`
//! (a cancel, one lookup after a not-found, at most five trade pages: at most
//! 7 requests, weight 30), `order_by_exchange_id` and `protection_fill` (one
//! lookup and at most five pages: weight 29), `replace_protection` (the
//! cancel wrapper and one placement) and the legacy `cancel_order`. Every
//! spot request, read or mutation, is one request on the wire (the client's
//! single-shot helpers follow no redirect and retry nothing), so these are
//! wire bounds.
//!
//! Not built here: ledger, costs and settlement are later steps' (8e, 8g, 8l)
//! and refuse with [`ExecError::Config`] and nothing sent.
//!
//! No production path constructs [`BinanceSpotAdapter`]: the factory refuses
//! `binance_spot` until step 8's activation (SD12; `inertness_tests.rs`).
//! Venue facts are from `binance-spot-api-docs` at `995f6eb3`.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use robson_domain::{OrderSide, Price, Quantity, Symbol, SymbolTradingRules};
use robson_exec::{
    gate::OperationGate,
    order_journal::OrderJournal,
    ports::{OpenOrderRecord, TradeFill, UserTradeRecord},
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
use tracing::warn;

use super::{
    cache::{SpotRulesCache, SpotRulesError},
    records::SpotOrderRecord,
    rest::{SpotOrderRef, SpotReadError, SpotRestReader},
    rules::SpotSymbolRules,
    trades::SpotTrade,
    trading::{self, NewOrder, OrderKind, SPOT_STP_MODE},
};
use crate::binance_rest::BinanceRestClient;

/// Finality of `binance:spot`: a not-found is final only when every attempt
/// under the client id was authoritatively rejected, as on USD-M. Time proves
/// nothing.
pub const SPOT_FINALITY: VenueFinalityRule = VenueFinalityRule::EveryAttemptRejected;

/// Client ids the attempt ledger tracks, as on USD-M.
const SUBMIT_ATTEMPTS_CAPACITY: usize = 4096;

/// The most trade pages a composite wrapper reads for one order (C3).
const COMPOSITE_TRADE_PAGES: usize = 5;

/// The Binance spot venue.
pub struct BinanceSpotAdapter {
    venue_id: VenueId,
    /// Shared with the reader: signing, the request bound and the gate.
    client: Arc<BinanceRestClient>,
    reader: SpotRestReader,
    rules: SpotRulesCache,
    /// Every order sent needs the attached journal's grant (8c-1).
    submit_attempts: SubmitAttemptLedger,
}

impl BinanceSpotAdapter {
    /// The venue over `client`. A testnet client is refused: step 8 does not
    /// support the spot testnet (design note v16 §0), as 8b's klines refuse
    /// it.
    pub fn new(client: Arc<BinanceRestClient>) -> Result<Self, ExecError> {
        if client.is_testnet() {
            return Err(ExecError::Config(
                "binance:spot needs a mainnet client: the spot testnet is out of scope \
                 (ADR-0056 step 8)"
                    .to_string(),
            ));
        }
        Ok(Self {
            venue_id: VenueId::new(VenueId::BINANCE_SPOT),
            reader: SpotRestReader::new(Arc::clone(&client)),
            client,
            rules: SpotRulesCache::new(),
            submit_attempts: SubmitAttemptLedger::one_attempt(SUBMIT_ATTEMPTS_CAPACITY),
        })
    }

    /// The whole venue rules last validated for `symbol` (STP modes,
    /// precisions, filters), with no request.
    pub fn validated_symbol_rules(&self, symbol: &Symbol) -> Option<SpotSymbolRules> {
        self.rules.spot_rules(symbol)
    }

    /// The reader, for the wire tests.
    #[cfg(test)]
    pub(super) fn reader(&self) -> &SpotRestReader {
        &self.reader
    }

    /// The attempt ledger, for the placement tests.
    #[cfg(test)]
    pub(super) fn ledger(&self) -> &SubmitAttemptLedger {
        &self.submit_attempts
    }

    /// The fills rule (8c plan §4) over at most five trade pages from id 0:
    /// the order's result when it is terminal with execution and its valid,
    /// distinct trades sum exactly to `executedQty`
    /// ([`trading::terminal_result`]). Reading stops as soon as the trades
    /// read cover the execution, a full page or not: completeness is that
    /// sum, not a short page (v16 §3.7). `None` with no read when it is not
    /// terminal or nothing executed; `None` when a read fails, five pages
    /// fall short, or the trades fail the evidence rules. Never more than
    /// five requests.
    async fn terminal_fills(
        &self,
        symbol: &Symbol,
        record: &VenueOrderRecord,
    ) -> Option<OrderResult> {
        if record.phase != OrderPhase::Terminal || record.executed_qty <= Decimal::ZERO {
            return None;
        }
        let quote_precision = self.rules.spot_rules(symbol).map(|spot| spot.quote_asset_precision);
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
                _ => return trading::terminal_result(record, trades, quote_precision),
            }
        }
        None
    }

    /// One lookup by exchange id, then the fills rule: the result of a
    /// terminal order with execution. `None` when it has not executed or is
    /// not terminal, when the venue does not know it (HTTP 400 -2013) and
    /// when it is archived (HTTP 400 -2026, nothing executed). `Err` when the
    /// read fails, and when it executed and its trades could not be
    /// established. `protective` refuses an order that is not a stop.
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
                "binance:spot order {order_id} on {} is a {}, not a protective stop",
                record.symbol, record.order_type
            )));
        }
        if record.phase != OrderPhase::Terminal || record.executed_qty <= Decimal::ZERO {
            return Ok(None);
        }
        match self.terminal_fills(symbol, &record).await {
            Some(result) => Ok(Some(result)),
            None => Err(ExecError::Exchange(format!(
                "binance:spot order {order_id} executed {} but its trades were not established \
                 within {COMPOSITE_TRADE_PAGES} pages",
                record.executed_qty
            ))),
        }
    }
}

/// A failed read: a timeout stays a timeout, the rest keeps its typed text.
fn read_error(error: SpotReadError) -> ExecError {
    match error {
        SpotReadError::Timeout => ExecError::Timeout("Binance spot request timed out".to_string()),
        other => ExecError::Exchange(other.to_string()),
    }
}

fn rules_error(error: SpotRulesError) -> ExecError {
    match error {
        SpotRulesError::Read(error) => read_error(error),
        recent => ExecError::Exchange(recent.to_string()),
    }
}

/// An order's phase, from its status alone (design §3.4.2; enums.md
/// L45-L57). `PENDING_CANCEL` is documented as unused (L54) and, like any
/// other status, is `Unknown`.
pub(super) fn order_phase(status: &str) -> OrderPhase {
    match status {
        "NEW" | "PARTIALLY_FILLED" | "PENDING_NEW" => OrderPhase::Live,
        "FILLED" | "CANCELED" | "EXPIRED" | "EXPIRED_IN_MATCH" | "REJECTED" => OrderPhase::Terminal,
        _ => OrderPhase::Unknown,
    }
}

/// The venue's record of `order`, as given: its quantities must be
/// consistent (`0 ≤ executedQty ≤ origQty`), or the answer is no evidence.
fn venue_record(order: SpotOrderRecord) -> Result<VenueOrderRecord, ExecError> {
    let side = match order.side.as_str() {
        "BUY" => OrderSide::Buy,
        "SELL" => OrderSide::Sell,
        other => {
            return Err(ExecError::Exchange(format!(
                "spot order {} on {} has side {other}",
                order.order_id, order.symbol
            )))
        },
    };
    let record = VenueOrderRecord {
        exchange_order_id: order.order_id.to_string(),
        client_order_id: order.client_order_id,
        orig_client_order_id: None,
        symbol: order.symbol,
        side,
        protection_kind: (order.order_type == "STOP_LOSS")
            .then_some(ProtectionKind::MarketOnTrigger),
        order_type: order.order_type,
        phase: order_phase(&order.status),
        status: order.status,
        orig_qty: order.orig_qty,
        executed_qty: order.executed_qty,
        cumulative_quote_qty: order.cummulative_quote_qty,
        stop_price: order.stop_price,
        expiry_reason: order.expiry_reason,
        transact_time_ms: None,
        time_ms: Some(order.time),
        update_time_ms: Some(order.update_time),
        working_time_ms: order.working_time,
    };
    if !record.quantities_consistent() {
        return Err(ExecError::Exchange(format!(
            "spot order {} on {} answered executedQty {} against origQty {}",
            record.exchange_order_id, record.symbol, record.executed_qty, record.orig_qty
        )));
    }
    Ok(record)
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
    let stop_price = match record.stop_price {
        Some(stop) if stop > Decimal::ZERO => Some(Price::new(stop)?),
        _ => None,
    };
    Ok(OpenOrderRecord {
        exchange_order_id: record.exchange_order_id.clone(),
        client_order_id: record.client_order_id.clone(),
        order_type: record.order_type.clone(),
        reduce_only: false,
        stop_price,
        side: record.side,
    })
}

/// One asset's balance, as a holding: each component as given and never
/// negative, or the answer is no evidence. Nothing is added, so no value the
/// venue sends can overflow here.
fn holding(asset: &str, free: Decimal, locked: Decimal) -> Result<Holding, ExecError> {
    if free < Decimal::ZERO || locked < Decimal::ZERO {
        return Err(ExecError::Exchange(format!(
            "binance:spot account lists {asset} with free {free} and locked {locked}: a balance is \
             never negative"
        )));
    }
    Ok(Holding { free, locked })
}

/// One trade of an order, with its time. Its quote amount is the venue's
/// figure, kept as given for its rounding, and is never negative: a negative
/// one makes the answer no evidence.
fn trade_fill(trade: SpotTrade) -> Result<TradeFill, ExecError> {
    if trade.quote_qty < Decimal::ZERO {
        return Err(ExecError::Exchange(format!(
            "spot trade {} has quoteQty {}: a quote amount is never negative",
            trade.id, trade.quote_qty
        )));
    }
    let time = DateTime::from_timestamp_millis(trade.time).ok_or_else(|| {
        ExecError::Exchange(format!("spot trade {} has time {}", trade.id, trade.time))
    })?;
    Ok(TradeFill {
        trade_id: trade.id.to_string(),
        price: trade.price,
        qty: trade.qty,
        quote_qty: trade.quote_qty,
        commission: trade.commission,
        commission_asset: trade.commission_asset,
        time: Some(time),
    })
}

/// A venue order id, refused locally (nothing sent) when it is not one.
fn order_id(id: &str) -> Result<u64, ExecError> {
    id.parse().map_err(|_| {
        ExecError::Config(format!("binance:spot order id {id:?} is not a number; nothing was sent"))
    })
}

#[async_trait]
impl TradingPort for BinanceSpotAdapter {
    async fn trading_rules(&self, symbol: &Symbol) -> Result<SymbolTradingRules, ExecError> {
        self.rules.cached(&self.reader, symbol).await.map_err(rules_error)
    }

    async fn trading_rules_preview(
        &self,
        symbol: &Symbol,
    ) -> Result<SymbolTradingRules, ExecError> {
        self.rules.preview(&self.reader, symbol).await.map_err(rules_error)
    }

    async fn trading_rules_fresh(&self, symbol: &Symbol) -> Result<SymbolTradingRules, ExecError> {
        self.rules.within_ttl(&self.reader, symbol).await.map_err(rules_error)
    }

    fn last_validated_rules(&self, symbol: &Symbol) -> Option<SymbolTradingRules> {
        self.rules.last_validated(symbol)
    }

    fn rules_refresh_due(&self, symbol: &Symbol) -> bool {
        self.rules.refresh_due(symbol)
    }

    async fn refresh_trading_rules(
        &self,
        symbol: &Symbol,
    ) -> Result<SymbolTradingRules, ExecError> {
        self.rules
            .refresh_after_rejection(&self.reader, symbol)
            .await
            .map_err(rules_error)
    }

    /// One MARKET order with `FULL` (`POST /api/v3/order`, weight 1; 8c plan
    /// §4), through the one-attempt ledger: nothing is sent without the
    /// journal's grant for `client_order_id`, and never twice.
    ///
    /// Spot holds no short: an entry buys and a reduction sells; the other
    /// two are refused with nothing sent. An entry is checked against fresh
    /// rules ([`quantize::market_quantity`]) and refused when the symbol does
    /// not allow `EXPIRE_MAKER`; a reduction goes out as given. An entry can
    /// therefore make two requests, an `exchangeInfo` refresh (weight 20)
    /// before the order (weight 1): weight 21 (8c plan §13, item 11).
    ///
    /// `Ok` is `FILLED`, or `EXPIRED` with execution, as the result of
    /// `executedQty`, with the record, the fills and typed fee evidence. An
    /// expiry with nothing executed is [`ExecError::ExpiredUnfilled`], never
    /// a rejection. HTTP 400 with -2010 is `OrderRejected` and with -1013
    /// `RulesRejected`. Everything else is an uncertain `Err`, among them a
    /// 200 with `CANCELED`, `REJECTED` or `EXPIRED_IN_MATCH`: this sends one
    /// request only, and the caller's later [`InventoryPort::observe_order`]
    /// resolves it.
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
            return Err(ExecError::Config(format!(
                "binance:spot has no short: a {intent:?} cannot {side:?}; nothing was sent"
            )));
        }
        let quantity = match intent {
            EntryOrReduce::Entry => {
                let rules = self.trading_rules_fresh(symbol).await?;
                let allowed = self.rules.spot_rules(symbol).is_some_and(|spot| {
                    spot.allowed_stp_modes.iter().any(|mode| mode == SPOT_STP_MODE)
                });
                if !allowed {
                    return Err(ExecError::Config(format!(
                        "binance:spot {} does not allow {SPOT_STP_MODE}; nothing was sent",
                        symbol.as_pair()
                    )));
                }
                quantize::market_quantity(&rules, quantity)?
            },
            EntryOrReduce::Reduce => quantity,
        };
        let order = NewOrder {
            symbol,
            side,
            quantity: quantity.as_decimal(),
            client_order_id,
            kind: OrderKind::Market,
        };
        let quote_precision = self.rules.spot_rules(symbol).map(|spot| spot.quote_asset_precision);
        trading::place_market(&self.client, &self.submit_attempts, order, quote_precision).await
    }

    /// One `STOP_LOSS` SELL with `stopPrice` and `RESULT` (`POST
    /// /api/v3/order`, weight 1; 8c plan §4), through the one-attempt ledger.
    /// `quantity` and `trigger` go out as given. A BUY is refused with
    /// nothing sent.
    ///
    /// Any 200 that echoes the request is `Ok`, whatever the status: the
    /// record in `acceptance` says whether the stop rests, and `Ok` alone
    /// never means protected. HTTP 400 with -2010 is `OrderRejected` and with
    /// -1013 `RulesRejected`; everything else is an uncertain `Err` that the
    /// caller's later lookup resolves.
    async fn place_protection(
        &self,
        symbol: &Symbol,
        close_side: OrderSide,
        quantity: Quantity,
        trigger: Price,
        client_order_id: &str,
    ) -> Result<PlacedStop, ExecError> {
        if close_side != OrderSide::Sell {
            return Err(ExecError::Config(
                "binance:spot protection sells what it holds: a BUY stop is refused; nothing \
                 was sent"
                    .to_string(),
            ));
        }
        trading::place_stop(
            &self.client,
            &self.submit_attempts,
            symbol,
            quantity,
            trigger,
            client_order_id,
        )
        .await
    }

    /// Composite (C3), for conformance and legacy callers only: one cancel
    /// ([`InventoryPort::cancel_order_once`]); after HTTP 400 -2011 or -2013,
    /// one lookup by exchange id; then at most five trade pages. At most 7
    /// requests, weight 30.
    ///
    /// `Cancelled` only when the cancel's own answer is terminal and
    /// consistent, its execution as `filled_before_cancel`. After a
    /// not-found, `AlreadyGone` with what the lookup establishes; a further
    /// not-found, an archive or a failed lookup proves nothing (`Unknown`).
    /// `Terminal.fills` follows the fills rule. Anything else is `Unknown`.
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
            CancelAnswer::Unknown { reason } => {
                warn!(
                    symbol = %symbol.as_pair(),
                    order_id = %stop.exchange_order_id,
                    %reason,
                    "Binance spot stop cancel uncertain"
                );
                Ok(StopCancelOutcome::Unknown)
            },
        }
    }

    /// Cancel-then-place (C6) through [`replace_cancel_then_place`]: the
    /// cancel wrapper, then, only after a definite cancel of an unexecuted
    /// stop, one placement under the successor's grant. `Replaced` certifies
    /// no protection.
    async fn replace_protection(
        &self,
        symbol: &Symbol,
        old: &StopRef,
        new: StopParams,
    ) -> Result<ReplaceOutcome, ExecError> {
        replace_cancel_then_place(self, &self.submit_attempts, symbol, old, new).await
    }

    /// The legacy cancel: one cancel request. `Ok` only when the venue's own
    /// answer shows the order cancelled and terminal. A rejected cancel (HTTP
    /// 400 -2011 or -2013) is `OrderRejected`: the cancel was refused, and the
    /// order's lifecycle is not established, -2011 being a generic rejection
    /// that proves neither absence nor execution. Anything else is an
    /// uncertain `Err`. 8k uses [`InventoryPort::cancel_order_once`] and an
    /// exact lookup (Q18).
    async fn cancel_order(&self, symbol: &Symbol, order_id: &str) -> Result<(), ExecError> {
        match self.cancel_order_once(symbol, order_id).await? {
            CancelAnswer::Cancelled(_) => Ok(()),
            CancelAnswer::NotFound { code } => Err(ExecError::OrderRejected(format!(
                "binance:spot cancel of {order_id} rejected (Binance {code}); the order's \
                 lifecycle is not established"
            ))),
            CancelAnswer::Unknown { reason } => Err(ExecError::Exchange(format!(
                "binance:spot cancel of {order_id} uncertain: {reason}"
            ))),
        }
    }

    /// One `GET /api/v3/order` by `origClientOrderId` (weight 4); no fills.
    /// HTTP 400 with -2013 is a raw not-found, read through [`SPOT_FINALITY`]
    /// over the attempts recorded for the id, read after the answer. -2026
    /// (archived) is `Unknown`, never `Absent`. Every other failure, -2013 at
    /// another status included, is the request's `Err`.
    async fn order_by_client_id(
        &self,
        symbol: &Symbol,
        client_order_id: &str,
    ) -> Result<OrderLookup, ExecError> {
        let by = SpotOrderRef::ClientOrderId(client_order_id.to_string());
        match self.reader.order(symbol, &by).await {
            Ok(order) => Ok(order_lookup(&venue_record(order)?)),
            Err(SpotReadError::NoSuchOrder { http_status: 400, .. }) => Ok(SPOT_FINALITY
                .classify_not_found(self.submit_attempts.final_evidence(client_order_id).await)),
            Err(SpotReadError::OrderArchived { .. }) => Ok(OrderLookup::Unknown),
            Err(error) => Err(read_error(error)),
        }
    }

    /// Composite (C3): one lookup by exchange id and at most five trade
    /// pages, weight at most 29. The result of a terminal order with
    /// execution, with its record and its trades as fills; `None` when it
    /// has not executed or is not terminal, unknown to the venue or
    /// archived.
    async fn order_by_exchange_id(
        &self,
        symbol: &Symbol,
        order_id: &str,
    ) -> Result<Option<OrderResult>, ExecError> {
        self.executed_order(symbol, order_id, false).await
    }

    /// Composite (C3): [`TradingPort::order_by_exchange_id`] for a stop,
    /// refusing an order that is not one. `None` until it executes and ends.
    async fn protection_fill(
        &self,
        symbol: &Symbol,
        stop: &StopRef,
    ) -> Result<Option<OrderResult>, ExecError> {
        self.executed_order(symbol, &stop.exchange_order_id, true).await
    }

    /// The symbol's open `STOP_LOSS` SELL orders, from one `openOrders`
    /// (weight 6). It detects; it never certifies protection.
    async fn open_protective_orders(
        &self,
        symbol: &Symbol,
    ) -> Result<Vec<OpenOrderRecord>, ExecError> {
        let orders = self.reader.open_orders(symbol).await.map_err(read_error)?;
        let mut protective = Vec::new();
        for order in orders {
            let record = venue_record(order)?;
            if record.protection_kind.is_some() && record.side == OrderSide::Sell {
                protective.push(open_order(&record)?);
            }
        }
        Ok(protective)
    }

    /// Refused: spot trades are read by order, a page at a time
    /// ([`InventoryPort::order_trades_page`]), never by time (8a's decision,
    /// `trades.rs`). Never an empty answer.
    async fn user_trades_since(
        &self,
        symbol: &Symbol,
        _since: DateTime<Utc>,
        _limit: u16,
    ) -> Result<Vec<UserTradeRecord>, ExecError> {
        Err(ExecError::Config(format!(
            "binance:spot reads no trades by time for {}: trades are read by order \
             (order_trades_page); nothing was sent",
            symbol.as_pair()
        )))
    }

    /// One `ticker/price` (weight 2).
    async fn price(&self, symbol: &Symbol) -> Result<Price, ExecError> {
        let price = self.reader.ticker_price(symbol).await.map_err(read_error)?;
        Ok(Price::new(price)?)
    }

    /// One `ping` (weight 1).
    async fn health(&self) -> Result<(), ExecError> {
        self.reader.ping().await.map_err(read_error)
    }
}

#[async_trait]
impl AccountPort for BinanceSpotAdapter {
    /// One `account` read (weight 20). An asset the account does not list
    /// holds nothing; a negative component is an error.
    async fn cash(&self, asset: &str) -> Result<Cash, ExecError> {
        let account = self.reader.account().await.map_err(read_error)?;
        let Holding { free, locked } = match account.balance(asset) {
            Some(balance) => holding(asset, balance.free, balance.locked)?,
            None => Holding {
                free: Decimal::ZERO,
                locked: Decimal::ZERO,
            },
        };
        Ok(Cash { free, locked, as_of: Utc::now() })
    }

    /// The last price, one `ticker/price` (weight 2), stamped on receipt.
    async fn valuation(&self, symbol: &Symbol) -> Result<Mark, ExecError> {
        Ok(Mark {
            price: self.price(symbol).await?,
            as_of: Utc::now(),
        })
    }

    /// The cost envelope is 8g's (ADR-0056 N5): fails closed, as on USD-M.
    async fn execution_costs(&self, symbol: &Symbol) -> Result<ExecutionCostEnvelope, ExecError> {
        Err(ExecError::Config(format!(
            "binance:spot execution cost envelope for {} arrives with step 8's costs (8g)",
            symbol.as_pair()
        )))
    }

    fn capabilities(&self) -> VenueCapabilities {
        VenueCapabilities {
            market_kind: MarketKind::Spot,
            account_kind: AccountKind::BalanceAccount,
            supports_short: false,
            reduce_only_wire_flag: false,
            protection: ProtectionKind::MarketOnTrigger,
            replacement: ReplacementProtocol::CancelThenPlace,
            has_funding: false,
            order_finality: SPOT_FINALITY,
            one_attempt_per_client_id: true,
        }
    }
}

#[async_trait]
impl InventoryPort for BinanceSpotAdapter {
    /// `account` (20), every `openOrders` (80) and `openOrderList` (6): three
    /// requests, weight 106, not one atomic snapshot. Every holding with a
    /// nonzero component (a negative one is an error), every open order with
    /// its full record, and the account's identity as given.
    ///
    /// An open order on a pair `Symbol::from_pair` cannot name fails the
    /// whole observation: fail-closed, never dropped. How reconciliation
    /// treats such an order is 8k's to decide before activation (8c plan §13,
    /// item 10).
    async fn account_observation(&self) -> Result<InventoryObservation, ExecError> {
        let account = self.reader.account().await.map_err(read_error)?;
        let orders = self.reader.all_open_orders().await.map_err(read_error)?;
        let open_order_lists = self.reader.open_order_list_count().await.map_err(read_error)?;
        let mut holdings = Vec::new();
        for balance in &account.balances {
            let held = holding(&balance.asset, balance.free, balance.locked)?;
            if !held.free.is_zero() || !held.locked.is_zero() {
                holdings.push((balance.asset.clone(), held));
            }
        }
        let mut open_orders = Vec::with_capacity(orders.len());
        for order in orders {
            let record = venue_record(order)?;
            open_orders.push(AccountOpenOrder {
                symbol: Symbol::from_pair(&record.symbol)?,
                order: open_order(&record)?,
                record: Some(record),
            });
        }
        Ok(InventoryObservation {
            holdings,
            open_orders,
            as_of: Utc::now(),
            open_order_lists,
            uid: account.uid.to_string(),
            can_trade: account.can_trade,
            account_type: account.account_type,
            permissions: account.permissions,
        })
    }

    /// Governed inventory reads the ledger (8e).
    async fn governed_inventory(&self, symbol: &Symbol) -> Result<GovernedInventory, ExecError> {
        Err(ExecError::Config(format!(
            "binance:spot governed inventory for {} arrives with step 8's ledger (8e)",
            symbol.as_pair()
        )))
    }

    /// One `DELETE /api/v3/order` by `orderId` (weight 1), through
    /// `trading.rs`: no grant, no retry. `Cancelled` only on HTTP 200 whose
    /// own record is this order's, terminal and consistent; `NotFound` on
    /// HTTP 400 with -2011 or -2013, which proves nothing about execution
    /// (-2011 is a generic cancel rejection, not proof of absence: an exact
    /// lookup decides); `Unknown` for everything else that was sent, a
    /// timeout, a failed transport and a redirect included. `Err` is a proven
    /// no-send: the gate refused, the symbol is not one value, or the id is
    /// not a number.
    async fn cancel_order_once(
        &self,
        symbol: &Symbol,
        order_id_text: &str,
    ) -> Result<CancelAnswer, ExecError> {
        trading::cancel_once(&self.client, symbol, order_id(order_id_text)?).await
    }

    /// One `GET /api/v3/order` (weight 4): the order's own record, its ids
    /// and symbol checked and its quantities consistent. HTTP 400 with -2013
    /// is `NotFound` and with -2026 `Archived`; anything else that failed
    /// established nothing (`Err`).
    ///
    /// By client id, the record keeps the identity the venue returned: a
    /// cancel renames an order's client id (F12), so the record's id is not
    /// compared with the one asked for. This raw observation proves no
    /// ownership; known exchange ids and durable conflict checks are 8e's and
    /// 8j's.
    async fn observe_order(
        &self,
        symbol: &Symbol,
        by: &OrderRef,
    ) -> Result<OrderObservation, ExecError> {
        let by = match by {
            OrderRef::ExchangeId(id) => SpotOrderRef::OrderId(order_id(id)?),
            OrderRef::ClientId(id) => SpotOrderRef::ClientOrderId(id.clone()),
        };
        match self.reader.order(symbol, &by).await {
            Ok(order) => Ok(OrderObservation::Found(Box::new(venue_record(order)?))),
            Err(SpotReadError::NoSuchOrder { http_status: 400, .. }) => {
                Ok(OrderObservation::NotFound)
            },
            Err(SpotReadError::OrderArchived { http_status: 400, .. }) => {
                Ok(OrderObservation::Archived)
            },
            Err(error) => Err(read_error(error)),
        }
    }

    /// One `myTrades` page by order from `from_id` (weight 5), each trade with
    /// its time; the cursor checks are the reader's. A trade with a negative
    /// quote amount makes the page an `Err`: it is no evidence.
    async fn order_trades_page(
        &self,
        symbol: &Symbol,
        order_id_text: &str,
        from_id: u64,
    ) -> Result<TradePage, ExecError> {
        let page = self
            .reader
            .trades_page(symbol, order_id(order_id_text)?, from_id)
            .await
            .map_err(read_error)?;
        Ok(TradePage {
            trades: page.trades.into_iter().map(trade_fill).collect::<Result<_, _>>()?,
            next_from_id: page.next_from_id,
        })
    }
}

#[async_trait]
impl SettlementPort for BinanceSpotAdapter {
    /// Settlement evidence is 8l's.
    async fn settlement_since(
        &self,
        _since: DateTime<Utc>,
        _limit: u16,
    ) -> Result<Vec<SettlementRecord>, ExecError> {
        Err(ExecError::Config(
            "binance:spot settlement evidence arrives with step 8's settlement (8l)".to_string(),
        ))
    }
}

impl Venue for BinanceSpotAdapter {
    fn id(&self) -> &VenueId {
        &self.venue_id
    }

    fn attach_order_journal(&self, journal: Arc<OrderJournal>) {
        self.submit_attempts.attach_journal(journal);
    }

    /// Every order goes through the REST client, which passes the gate.
    fn attach_gate(&self, gate: Arc<OperationGate>) {
        self.client.attach_gate(gate);
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
