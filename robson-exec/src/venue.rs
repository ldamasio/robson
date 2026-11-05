//! Venue port contracts (ADR-0056 §2).
//!
//! A **venue** is exchange × market kind × account scope (`binance:usdm`,
//! later `binance:spot`). These traits replace the flat [`ExchangePort`]:
//! the required surface is split into [`TradingPort`] and [`AccountPort`],
//! and the account-kind specific or optional surfaces are borrowed accessors
//! on [`Venue`] ([`PositionAccountPort`], [`InventoryPort`],
//! [`SettlementPort`], [`FundingPort`]).
//!
//! Step 2 only introduces the contracts; [`ExchangePort`] and [`IncomePort`]
//! remain as a forwarding layer over them (`compat.rs`). Callers branch on
//! [`VenueCapabilities`], never on [`VenueId`]. [`StopCancelOutcome`] here is
//! the venue contract; the legacy two-variant outcome stays at
//! `robson_exec::ports::StopCancelOutcome` until the legacy port is removed.
//!
//! [`ExchangePort`]: crate::ports::ExchangePort
//! [`IncomePort`]: crate::ports::IncomePort

use std::{fmt, time::Duration};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use robson_domain::{OrderSide, Price, Quantity, Side, Symbol, SymbolTradingRules};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::{
    error::ExecError,
    ports::{
        ExchangePosition, FuturesBalance, FuturesSettings, IncomeRecord, OpenOrderRecord,
        OrderResult, SpotBalance, SpotOrder, SpotOrderRequest, Transfer, TransferId,
        UniversalTransferType, UserTradeRecord,
    },
};

// =============================================================================
// Vocabulary (ADR-0056 §1)
// =============================================================================

/// Stable venue identifier used in config, logs, `/status` and allocations
/// (`binance:usdm`, `binance:spot`, `b3:btc-fut`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct VenueId(String);

impl VenueId {
    /// Binance USDⓈ-M perpetual futures.
    pub const BINANCE_USDM: &'static str = "binance:usdm";

    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for VenueId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Market kind of a venue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MarketKind {
    /// USDⓈ-margined perpetual: position account, funding, reduce-only.
    PerpUsdM,
    /// Spot: balance account, holdings, protection is an order on the book.
    Spot,
}

/// What the venue reports about the operated account.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AccountKind {
    /// The venue reports positions ([`PositionAccountPort`]).
    PositionAccount,
    /// The venue reports holdings only; Robson derives its governed position
    /// from its own fills ([`InventoryPort`]).
    BalanceAccount,
}

/// Exchange-resident protection primitive (ADR-0056 §2.1). Only
/// market-on-trigger is eligible (ADR-0039/0052); stop-limit has no variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProtectionKind {
    /// A resting order that triggers **market** execution at the trigger
    /// (Binance USDⓈ-M conditional `STOP_MARKET`; Binance spot `STOP_LOSS`).
    MarketOnTrigger,
}

/// How a protective stop is replaced (ADR-0056 §2.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReplacementProtocol {
    /// The successor is placed first and the old stop is cancelled only after
    /// the successor is accepted: the position is never unprotected.
    PlaceThenCancel,
    /// Balance accounts: the stop locks the inventory, so it must be cancelled
    /// before the successor can be placed (non-atomic transition, N1-a).
    CancelThenPlace,
}

/// Rule under which a venue's "order not found" is evidence that the order
/// was never accepted ([`OrderLookup::Absent`]).
///
/// A not-found is final only when the order's age (from `submitted_at`, a
/// time no later than the moment the order request was signed) is at least
/// `propagation_window` (the venue can no longer accept or surface the
/// request) and the age plus that same margin is still below
/// `lookup_retention` (the venue's order lookup still returns every order it
/// accepted, including cancelled or expired ones without fills). Outside that
/// interval, or without a submission time, a not-found is
/// [`OrderLookup::Unknown`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VenueFinalityRule {
    pub propagation_window: Duration,
    pub lookup_retention: Duration,
}

impl VenueFinalityRule {
    /// True when a not-found on the venue's lookup is final for an order
    /// submitted at `submitted_at`, observed at `now`.
    pub fn not_found_is_final(
        &self,
        submitted_at: Option<DateTime<Utc>>,
        now: DateTime<Utc>,
    ) -> bool {
        let Some(submitted_at) = submitted_at else {
            return false;
        };
        // A negative age (clock skew) is never final.
        let Ok(age) = (now - submitted_at).to_std() else {
            return false;
        };
        age >= self.propagation_window
            && age.saturating_add(self.propagation_window) < self.lookup_retention
    }

    /// Classify a raw not-found response: `Absent` only under this rule,
    /// otherwise `Unknown`.
    pub fn classify_not_found(
        &self,
        submitted_at: Option<DateTime<Utc>>,
        now: DateTime<Utc>,
    ) -> OrderLookup {
        if self.not_found_is_final(submitted_at, now) {
            OrderLookup::Absent { finality: *self }
        } else {
            OrderLookup::Unknown
        }
    }
}

/// Static venue capabilities (ADR-0056 §2). Data, never inferred from the id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VenueCapabilities {
    pub market_kind: MarketKind,
    /// Must agree with the accessors: `PositionAccount` ⇔
    /// `position_account()` is `Some`, `BalanceAccount` ⇔ `inventory()` is
    /// `Some`.
    pub account_kind: AccountKind,
    /// Whether `Side::Short` can be armed on this venue (ADR-0056 §6).
    pub supports_short: bool,
    /// Whether [`EntryOrReduce::Reduce`] is sent to the venue as a wire flag
    /// (USDⓈ-M `reduceOnly=true`); spot has none.
    pub reduce_only_wire_flag: bool,
    pub protection: ProtectionKind,
    pub replacement: ReplacementProtocol,
    /// Must agree with the accessor: `true` ⇔ `funding()` is `Some` (the
    /// spot→derivatives conversion surface, [`FundingPort`]). Perpetual
    /// funding *costs* are described by
    /// [`ExecutionCostEnvelope::funding`].
    pub has_funding: bool,
    pub order_finality: VenueFinalityRule,
}

// =============================================================================
// Trading contract types
// =============================================================================

/// Purpose of a market order. On venues with
/// `reduce_only_wire_flag = true`, `Reduce` is sent as `reduceOnly`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryOrReduce {
    Entry,
    Reduce,
}

impl EntryOrReduce {
    pub fn from_reduce_only(reduce_only: bool) -> Self {
        if reduce_only {
            Self::Reduce
        } else {
            Self::Entry
        }
    }

    pub fn is_reduce(self) -> bool {
        matches!(self, Self::Reduce)
    }
}

/// Identity of an exchange-resident protective stop. Today only the id is
/// persisted (`insurance_stop_id`), so the reference is the id.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StopRef {
    /// Venue id of the resting order (Binance USDⓈ-M: the algo id).
    pub exchange_order_id: String,
}

impl StopRef {
    pub fn new(exchange_order_id: impl Into<String>) -> Self {
        Self {
            exchange_order_id: exchange_order_id.into(),
        }
    }
}

/// An accepted (unfilled) protective stop: its reference plus the acceptance
/// record the executor journals (zero fill, the requested trigger echoed as
/// `fill_price`, acceptance time as `filled_at`).
#[derive(Debug, Clone)]
pub struct PlacedStop {
    pub stop: StopRef,
    pub acceptance: OrderResult,
}

/// Parameters of a successor protective stop.
#[derive(Debug, Clone)]
pub struct StopParams {
    pub close_side: OrderSide,
    pub quantity: Quantity,
    pub trigger: Price,
    pub client_order_id: String,
}

/// Lifecycle state of an order looked up on the venue.
#[derive(Debug, Clone)]
pub enum OrderLookup {
    /// The venue never accepted the order. Returned **only** under the
    /// venue's documented [`VenueFinalityRule`]; a raw not-found outside that
    /// rule is `Unknown`.
    Absent { finality: VenueFinalityRule },
    /// Live on the book with no execution (resting, or a conditional order
    /// that has not triggered).
    Open { filled_qty: Decimal },
    /// Live with partial execution.
    PartiallyFilled { filled_qty: Decimal },
    /// Final. `filled_qty` is the venue's authoritative cumulative execution;
    /// `fills` is fee-grade fill evidence when the venue's lookup carries it
    /// (`None` means look up user trades, Policy 11), `cancelled_qty` the
    /// unexecuted remainder.
    Terminal {
        filled_qty: Decimal,
        cancelled_qty: Decimal,
        fills: Option<OrderResult>,
    },
    /// The state could not be established.
    Unknown,
}

/// Outcome of cancelling a protective stop (ADR-0056 §2).
#[derive(Debug, Clone)]
pub enum StopCancelOutcome {
    /// The venue acknowledged the cancel. `filled_before_cancel` is the
    /// stop's cumulative execution at cancel time (zero for a conditional
    /// stop that never triggered).
    Cancelled {
        filled_before_cancel: Decimal,
        fills: Option<OrderResult>,
    },
    /// The venue no longer knows the order; the commonest reason is that it
    /// triggered. `final_state` is `Unknown` unless the adapter established
    /// it without an extra venue request.
    AlreadyGone { final_state: OrderLookup },
    /// The cancel outcome is indeterminate; reconcile before any further
    /// submission.
    Unknown,
}

/// Outcome of replacing a protective stop (ADR-0056 §2).
#[derive(Debug, Clone)]
pub enum ReplaceOutcome {
    /// The successor is accepted and the old stop is gone with `old_filled`
    /// cumulative execution.
    Replaced {
        new: PlacedStop,
        old_filled: Decimal,
    },
    /// The venue rejected the successor; the old stop was never touched.
    OldKeptNewRejected { reason: String },
    /// `CancelThenPlace` only: the old stop was cancelled and the successor
    /// rejected; the residual is unprotected.
    OldCancelledNewRejected { old_filled: Decimal, reason: String },
    /// `CancelThenPlace` only: the venue rejected the successor's trigger as
    /// already crossed. Not execution evidence: `old` is the old stop's final
    /// state and `residual = governed − old_filled`.
    TriggerAlreadyCrossed { old: OrderLookup, residual: Decimal },
    /// Indeterminate. `new` carries the successor when it is known to be
    /// accepted (the old stop's state is then what is unknown); `None` when
    /// even the placement outcome is unknown. Reconcile before any further
    /// submission.
    Unknown { new: Option<PlacedStop> },
}

// =============================================================================
// Account contract types
// =============================================================================

/// Cash in one asset (ADR-0056 §2.4). `free` is the cap for new entries.
#[derive(Debug, Clone, PartialEq)]
pub struct Cash {
    pub free: Decimal,
    pub locked: Decimal,
    pub as_of: DateTime<Utc>,
}

/// Valuation mark for a symbol.
#[derive(Debug, Clone, PartialEq)]
pub struct Mark {
    pub price: Price,
    pub as_of: DateTime<Utc>,
}

/// Where commissions settle (ADR-0056 §2.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeeAssetPolicy {
    /// Commission charged in the quote asset.
    Quote,
    /// Commission charged in the base asset; governed quantity is net of it.
    Base,
    /// Commission charged in a third asset, converted at the venue mark at
    /// fill time and recorded with provenance.
    Other,
}

/// Periodic funding terms of a perpetual venue.
#[derive(Debug, Clone, PartialEq)]
pub struct FundingTerms {
    pub interval: Duration,
    pub rate_bps: Decimal,
}

/// Execution cost envelope of a venue for a symbol (ADR-0056 §2.5). A missing
/// or stale envelope fails admission closed (once admission reads it).
#[derive(Debug, Clone, PartialEq)]
pub struct ExecutionCostEnvelope {
    pub maker_bps: Decimal,
    pub taker_bps: Decimal,
    pub gap_bps: Decimal,
    pub fee_asset_policy: FeeAssetPolicy,
    pub funding: Option<FundingTerms>,
    pub as_of: DateTime<Utc>,
    /// Provenance of the numbers (venue endpoint or dated configuration).
    pub source: String,
}

/// Account-wide snapshot of a position account (ADR-0056 §2.3, §2.4).
#[derive(Debug, Clone, PartialEq)]
pub struct PositionSnapshot {
    /// The connector's `wallet_balance` mapping, unchanged.
    pub wallet_balance: Decimal,
    /// Unrealized P&L reported separately by the venue.
    pub unrealized_pnl: Decimal,
    pub positions: Vec<ExchangePosition>,
    pub as_of: DateTime<Utc>,
}

/// Free and locked quantity of one asset on a balance account.
#[derive(Debug, Clone, PartialEq)]
pub struct Holding {
    pub free: Decimal,
    pub locked: Decimal,
}

/// Account-wide observation of a balance account: every nonzero holding of
/// every asset and every open order, independent of configured symbols.
#[derive(Debug, Clone, PartialEq)]
pub struct InventoryObservation {
    pub holdings: Vec<(String, Holding)>,
    pub open_orders: Vec<OpenOrderRecord>,
    pub as_of: DateTime<Utc>,
}

/// Base-asset inventory bought by Robson's legs, derived from authoritative
/// fills (net of base-asset commission).
#[derive(Debug, Clone, PartialEq)]
pub struct GovernedInventory {
    pub qty: Decimal,
    pub fill_ids: Vec<String>,
}

/// What the reconciliation worker, startup recovery, the live-risk filter and
/// `/status` consume (ADR-0056 §2.3).
#[derive(Debug, Clone, PartialEq)]
pub enum AccountObservation {
    Positions(PositionSnapshot),
    Inventory(InventoryObservation),
}

/// Settlement evidence item: the typed income record (ADR-0045) is the
/// settlement record of every venue.
pub type SettlementRecord = IncomeRecord;

// =============================================================================
// Ports
// =============================================================================

/// Order placement, protection lifecycle and order evidence (required).
#[async_trait]
pub trait TradingPort: Send + Sync {
    /// Runtime symbol trading rules (issue #154 contract unchanged: bounded
    /// TTL cache, single-flight, never per tick, stale on refresh failure).
    /// Default: unavailable (`ExecutableSpan` flows fail closed).
    async fn trading_rules(&self, symbol: &Symbol) -> Result<SymbolTradingRules, ExecError> {
        Err(ExecError::Exchange(format!(
            "trading rules not available on this exchange port for {}",
            symbol.as_pair()
        )))
    }

    /// Read-only rules lookup for previews (issue #154 contract unchanged:
    /// never touches the production cache). Default: `trading_rules`.
    async fn trading_rules_preview(
        &self,
        symbol: &Symbol,
    ) -> Result<SymbolTradingRules, ExecError> {
        self.trading_rules(symbol).await
    }

    /// Place a market order. `intent = Reduce` must only reduce exposure.
    async fn place_market_order(
        &self,
        symbol: &Symbol,
        side: OrderSide,
        quantity: Quantity,
        client_order_id: &str,
        intent: EntryOrReduce,
    ) -> Result<OrderResult, ExecError>;

    /// Place exchange-resident protection of kind
    /// `capabilities().protection` (ADR-0039). `trigger` is chart-derived
    /// (ADR-0021), never a percentage of entry.
    async fn place_protection(
        &self,
        symbol: &Symbol,
        close_side: OrderSide,
        quantity: Quantity,
        trigger: Price,
        client_order_id: &str,
    ) -> Result<PlacedStop, ExecError>;

    /// Cancel a protective stop.
    async fn cancel_protection(
        &self,
        symbol: &Symbol,
        stop: &StopRef,
    ) -> Result<StopCancelOutcome, ExecError>;

    /// Replace a protective stop per `capabilities().replacement`.
    async fn replace_protection(
        &self,
        symbol: &Symbol,
        old: &StopRef,
        new: StopParams,
    ) -> Result<ReplaceOutcome, ExecError>;

    /// Cancel a regular (non-protective) order.
    async fn cancel_order(&self, symbol: &Symbol, order_id: &str) -> Result<(), ExecError>;

    /// Look up an order by the client id it was submitted with.
    /// `submitted_at` (no later than the request signature) is what the
    /// venue's [`VenueFinalityRule`] is evaluated against; `None` means a
    /// not-found can never be `Absent`. A request failure is `Err`.
    async fn order_by_client_id(
        &self,
        symbol: &Symbol,
        client_order_id: &str,
        submitted_at: Option<DateTime<Utc>>,
    ) -> Result<OrderLookup, ExecError>;

    /// Fee-grade fill evidence for a filled order by exchange id
    /// (TD-2026-05-05-001); `None` when not found or not filled.
    async fn order_by_exchange_id(
        &self,
        symbol: &Symbol,
        order_id: &str,
    ) -> Result<Option<OrderResult>, ExecError>;

    /// The real triggered fill of a protective stop (Policy 11: from the
    /// actual order, never from algo-level estimates); `None` when it has not
    /// triggered.
    async fn protection_fill(
        &self,
        symbol: &Symbol,
        stop: &StopRef,
    ) -> Result<Option<OrderResult>, ExecError>;

    /// Open protective orders for a symbol (orphan sweep, startup heal).
    async fn open_protective_orders(
        &self,
        symbol: &Symbol,
    ) -> Result<Vec<OpenOrderRecord>, ExecError>;

    /// User trades since `since`, oldest first.
    async fn user_trades_since(
        &self,
        symbol: &Symbol,
        since: DateTime<Utc>,
        limit: u16,
    ) -> Result<Vec<UserTradeRecord>, ExecError>;

    async fn price(&self, symbol: &Symbol) -> Result<Price, ExecError>;

    async fn health(&self) -> Result<(), ExecError>;
}

/// Cash, valuation, costs and capabilities (required).
#[async_trait]
pub trait AccountPort: Send + Sync {
    async fn cash(&self, asset: &str) -> Result<Cash, ExecError>;

    async fn valuation(&self, symbol: &Symbol) -> Result<Mark, ExecError>;

    async fn execution_costs(&self, symbol: &Symbol) -> Result<ExecutionCostEnvelope, ExecError>;

    fn capabilities(&self) -> VenueCapabilities;
}

/// Required when `account_kind = PositionAccount`.
#[async_trait]
pub trait PositionAccountPort: Send + Sync {
    /// Validate One-way position mode with the expected leverage (1).
    /// **SAFETY CHECK** before any order.
    async fn validate_position_settings(
        &self,
        symbol: &Symbol,
        expected_leverage: u8,
    ) -> Result<FuturesSettings, ExecError>;

    /// Account balance: `wallet_balance` (mapping unchanged),
    /// `available_balance` and the separate `unrealized_pnl`.
    async fn balance(&self) -> Result<FuturesBalance, ExecError>;

    /// Every open position on the account, independent of tracked symbols.
    async fn open_positions(&self) -> Result<Vec<ExchangePosition>, ExecError>;

    /// Close a position with a reduce-only market order. `side` is the
    /// position side being closed, not the outbound order side.
    async fn close_position_market(
        &self,
        symbol: &Symbol,
        side: Side,
        quantity: Quantity,
        client_order_id: &str,
    ) -> Result<OrderResult, ExecError>;

    /// Account-wide snapshot (balance, then positions: two venue requests).
    async fn position_snapshot(&self) -> Result<PositionSnapshot, ExecError> {
        let balance = self.balance().await?;
        let positions = self.open_positions().await?;
        Ok(PositionSnapshot {
            wallet_balance: balance.wallet_balance,
            unrealized_pnl: balance.unrealized_pnl,
            positions,
            as_of: Utc::now(),
        })
    }
}

/// Required when `account_kind = BalanceAccount` (ADR-0056 step 8).
#[async_trait]
pub trait InventoryPort: Send + Sync {
    async fn account_observation(&self) -> Result<InventoryObservation, ExecError>;

    async fn governed_inventory(&self, symbol: &Symbol) -> Result<GovernedInventory, ExecError>;
}

/// Settlement evidence (required on every venue): realized P&L, commission
/// and, where applicable, funding.
#[async_trait]
pub trait SettlementPort: Send + Sync {
    /// Items since `since` (inclusive), across all symbols, oldest first.
    async fn settlement_since(
        &self,
        since: DateTime<Utc>,
        limit: u16,
    ) -> Result<Vec<SettlementRecord>, ExecError>;
}

/// Optional spot→derivatives conversion surface (position accounts only).
#[async_trait]
pub trait FundingPort: Send + Sync {
    async fn spot_balances(&self) -> Result<Vec<SpotBalance>, ExecError>;

    async fn spot_price(&self, symbol: &str) -> Result<Price, ExecError>;

    async fn spot_symbol_is_trading(&self, symbol: &str) -> Result<bool, ExecError> {
        let _ = symbol;
        Err(ExecError::Exchange("spot symbol trading lookup is not implemented".to_string()))
    }

    async fn place_spot_market_order(
        &self,
        request: SpotOrderRequest,
    ) -> Result<SpotOrder, ExecError>;

    async fn spot_order(
        &self,
        symbol: &str,
        client_order_id: &str,
    ) -> Result<Option<SpotOrder>, ExecError>;

    async fn transfer(
        &self,
        asset: &str,
        amount: Decimal,
        transfer_type: UniversalTransferType,
        client_tran_key: &str,
    ) -> Result<TransferId, ExecError>;

    async fn transfer_history(
        &self,
        transfer_type: UniversalTransferType,
        start_time: DateTime<Utc>,
    ) -> Result<Vec<Transfer>, ExecError>;
}

/// A venue: the required trading and account surfaces plus borrowed
/// accessors for the account-kind specific and optional ones.
pub trait Venue: TradingPort + AccountPort {
    fn id(&self) -> &VenueId;
    fn position_account(&self) -> Option<&dyn PositionAccountPort>;
    fn inventory(&self) -> Option<&dyn InventoryPort>;
    fn settlement(&self) -> &dyn SettlementPort;
    fn funding(&self) -> Option<&dyn FundingPort>;
}

// =============================================================================
// Generic helpers
// =============================================================================

/// Produce the account-wide observation of a venue from whichever account
/// accessor it exposes (ADR-0056 §2.3).
pub async fn observe_account<V: Venue + ?Sized>(
    venue: &V,
) -> Result<AccountObservation, ExecError> {
    if let Some(account) = venue.position_account() {
        return account.position_snapshot().await.map(AccountObservation::Positions);
    }
    if let Some(inventory) = venue.inventory() {
        return inventory.account_observation().await.map(AccountObservation::Inventory);
    }
    Err(ExecError::Config(format!(
        "venue {} exposes no account observation",
        venue.id()
    )))
}

/// Disagreements between a venue's capability flags and its accessors; empty
/// when consistent (the conformance suite, ADR-0056 §10, asserts this).
pub fn capability_mismatches<V: Venue + ?Sized>(venue: &V) -> Vec<&'static str> {
    let caps = venue.capabilities();
    let mut mismatches = Vec::new();
    let position = venue.position_account().is_some();
    let inventory = venue.inventory().is_some();
    match caps.account_kind {
        AccountKind::PositionAccount if !position || inventory => {
            mismatches.push("PositionAccount requires position_account() and no inventory()")
        },
        AccountKind::BalanceAccount if !inventory || position => {
            mismatches.push("BalanceAccount requires inventory() and no position_account()")
        },
        _ => {},
    }
    if caps.has_funding != venue.funding().is_some() {
        mismatches.push("has_funding disagrees with funding()");
    }
    mismatches
}

/// `ReplacementProtocol::PlaceThenCancel` over [`TradingPort`]: the old stop
/// is never cancelled before the successor is accepted, so this protocol
/// never yields `OldCancelledNewRejected` or `TriggerAlreadyCrossed` (a
/// crossed trigger is a rejection of the successor with the old stop live).
pub async fn replace_place_then_cancel<T: TradingPort + ?Sized>(
    venue: &T,
    symbol: &Symbol,
    old: &StopRef,
    new: StopParams,
) -> Result<ReplaceOutcome, ExecError> {
    let placed = match venue
        .place_protection(symbol, new.close_side, new.quantity, new.trigger, &new.client_order_id)
        .await
    {
        Ok(placed) => placed,
        Err(ExecError::OrderRejected(reason)) => {
            return Ok(ReplaceOutcome::OldKeptNewRejected { reason });
        },
        // Transport or unclassified failure: the successor may exist.
        Err(_) => return Ok(ReplaceOutcome::Unknown { new: None }),
    };
    match venue.cancel_protection(symbol, old).await {
        Ok(StopCancelOutcome::Cancelled { filled_before_cancel, .. }) => {
            Ok(ReplaceOutcome::Replaced {
                new: placed,
                old_filled: filled_before_cancel,
            })
        },
        Ok(StopCancelOutcome::AlreadyGone {
            final_state: OrderLookup::Terminal { filled_qty, .. },
        }) => Ok(ReplaceOutcome::Replaced { new: placed, old_filled: filled_qty }),
        Ok(_) | Err(_) => Ok(ReplaceOutcome::Unknown { new: Some(placed) }),
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    #[test]
    fn not_found_is_absent_only_between_window_and_retention() {
        let rule = VenueFinalityRule {
            propagation_window: Duration::from_secs(60),
            lookup_retention: Duration::from_secs(3 * 24 * 60 * 60),
        };
        let now = Utc.with_ymd_and_hms(2026, 11, 5, 12, 0, 0).unwrap();
        let ago = |secs: i64| Some(now - chrono::Duration::seconds(secs));
        let absent = |at| matches!(rule.classify_not_found(at, now), OrderLookup::Absent { .. });

        assert!(!absent(None), "no submission time");
        assert!(!absent(ago(59)), "inside the propagation window");
        assert!(!absent(ago(-5)), "clock skew: submitted in the future");
        assert!(absent(ago(60)));
        assert!(absent(ago(2 * 24 * 60 * 60)));
        assert!(!absent(ago(3 * 24 * 60 * 60 - 30)), "within the margin of retention");
        assert!(!absent(ago(4 * 24 * 60 * 60)), "past retention");
        assert!(matches!(
            rule.classify_not_found(ago(60), now),
            OrderLookup::Absent { finality } if finality == rule
        ));
    }

    #[test]
    fn entry_or_reduce_round_trips_the_legacy_flag() {
        assert_eq!(EntryOrReduce::from_reduce_only(true), EntryOrReduce::Reduce);
        assert_eq!(EntryOrReduce::from_reduce_only(false), EntryOrReduce::Entry);
        assert!(EntryOrReduce::Reduce.is_reduce() && !EntryOrReduce::Entry.is_reduce());
    }
}
