//! The Binance spot venue, `binance:spot` (ADR-0056 step 8, PR 8c-2): the
//! adapter over the shared REST client, its rules cache, and the read side of
//! the venue ports.
//!
//! Built here: trading rules through [`SpotRulesCache`]; the venue's order
//! records ([`VenueOrderRecord`], phase from status alone); account and
//! inventory reads; `order_by_client_id`, `open_protective_orders`, and the
//! single-request `observe_order` and `order_trades_page` the leg owner uses
//! (8c plan, C1). Every read is one of 8a's [`SpotRestReader`] reads at its
//! documented weight, one request, no retry.
//!
//! Not built here: every method that places, cancels or replaces an order,
//! and the composite lookups beside them, are 8c-3's. Until then each
//! returns [`ExecError::Config`] and sends nothing. Ledger, costs and
//! settlement are later steps' (8e, 8g, 8l) and refuse the same way.
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
    venue::{
        AccountKind, AccountOpenOrder, AccountPort, CancelAnswer, Cash, EntryOrReduce,
        ExecutionCostEnvelope, FundingPort, GovernedInventory, Holding, InventoryObservation,
        InventoryPort, Mark, MarketKind, OrderLookup, OrderObservation, OrderPhase, OrderRef,
        PlacedStop, PositionAccountPort, ProtectionKind, ReplaceOutcome, ReplacementProtocol,
        SettlementPort, SettlementRecord, StopCancelOutcome, StopParams, StopRef,
        SubmitAttemptLedger, TradePage, TradingPort, Venue, VenueCapabilities, VenueFinalityRule,
        VenueId, VenueOrderRecord,
    },
    ExecError, OrderResult,
};
use rust_decimal::Decimal;

use super::{
    cache::{SpotRulesCache, SpotRulesError},
    records::SpotOrderRecord,
    rest::{SpotOrderRef, SpotReadError, SpotRestReader},
    rules::SpotSymbolRules,
    trades::SpotTrade,
};
use crate::binance_rest::BinanceRestClient;

/// Finality of `binance:spot`: a not-found is final only when every attempt
/// under the client id was authoritatively rejected, as on USD-M. Time proves
/// nothing.
pub const SPOT_FINALITY: VenueFinalityRule = VenueFinalityRule::EveryAttemptRejected;

/// Client ids the attempt ledger tracks, as on USD-M.
const SUBMIT_ATTEMPTS_CAPACITY: usize = 4096;

/// The Binance spot venue.
pub struct BinanceSpotAdapter {
    venue_id: VenueId,
    /// Shared with the reader: signing, the request bound and the gate.
    client: Arc<BinanceRestClient>,
    reader: SpotRestReader,
    rules: SpotRulesCache,
    /// Every send needs the attached journal's grant (8c-1); 8c-3 sends.
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
}

/// What a method that sends an order or a cancel answers before 8c-3: a
/// proven no-send.
fn not_yet(what: &str) -> ExecError {
    ExecError::Config(format!(
        "binance:spot {what} arrives with ADR-0056 step 8, PR 8c-3; nothing was sent"
    ))
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
fn order_phase(status: &str) -> OrderPhase {
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

/// One trade of an order, with its time.
fn trade_fill(trade: SpotTrade) -> Result<TradeFill, ExecError> {
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

    async fn place_market_order(
        &self,
        _symbol: &Symbol,
        _side: OrderSide,
        _quantity: Quantity,
        _client_order_id: &str,
        _intent: EntryOrReduce,
    ) -> Result<OrderResult, ExecError> {
        Err(not_yet("place_market_order"))
    }

    async fn place_protection(
        &self,
        _symbol: &Symbol,
        _close_side: OrderSide,
        _quantity: Quantity,
        _trigger: Price,
        _client_order_id: &str,
    ) -> Result<PlacedStop, ExecError> {
        Err(not_yet("place_protection"))
    }

    async fn cancel_protection(
        &self,
        _symbol: &Symbol,
        _stop: &StopRef,
    ) -> Result<StopCancelOutcome, ExecError> {
        Err(not_yet("cancel_protection"))
    }

    async fn replace_protection(
        &self,
        _symbol: &Symbol,
        _old: &StopRef,
        _new: StopParams,
    ) -> Result<ReplaceOutcome, ExecError> {
        Err(not_yet("replace_protection"))
    }

    async fn cancel_order(&self, _symbol: &Symbol, _order_id: &str) -> Result<(), ExecError> {
        Err(not_yet("cancel_order"))
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

    async fn order_by_exchange_id(
        &self,
        _symbol: &Symbol,
        _order_id: &str,
    ) -> Result<Option<OrderResult>, ExecError> {
        Err(not_yet("order_by_exchange_id"))
    }

    async fn protection_fill(
        &self,
        _symbol: &Symbol,
        _stop: &StopRef,
    ) -> Result<Option<OrderResult>, ExecError> {
        Err(not_yet("protection_fill"))
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

    async fn cancel_order_once(
        &self,
        _symbol: &Symbol,
        _order_id: &str,
    ) -> Result<CancelAnswer, ExecError> {
        Err(not_yet("cancel_order_once"))
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
    /// its time; the cursor checks are the reader's.
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

    /// Every signed mutation will go through the REST client, which passes
    /// the gate (8c-3).
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
