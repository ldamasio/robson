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
//! the venue contract; its lossless mirror stays at
//! `robson_exec::ports::StopCancelOutcome` until the legacy port is removed.
//!
//! [`ExchangePort`]: crate::ports::ExchangePort
//! [`IncomePort`]: crate::ports::IncomePort

use std::{
    collections::HashMap,
    fmt,
    future::Future,
    sync::{Arc, Mutex, MutexGuard, OnceLock},
    time::Duration,
};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use robson_domain::{OrderSide, Price, Quantity, Side, Symbol, SymbolTradingRules};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::{
    error::ExecError,
    gate::OperationGate,
    order_journal::{begin_send, report_rejected, report_send_started, OrderJournal, SendRefusal},
    ports::{
        ExchangePosition, FuturesBalance, FuturesSettings, IncomeRecord, OpenOrderRecord,
        OrderResult, SpotBalance, SpotOrder, SpotOrderRequest, TradeFill, Transfer, TransferId,
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
    /// Binance spot (ADR-0056 step 8). No factory selects it before step 8's
    /// activation.
    pub const BINANCE_SPOT: &'static str = "binance:spot";

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

/// Rule under which a venue's "order not found" proves the order was never
/// accepted ([`OrderLookup::Absent`]).
///
/// Elapsed time is deliberately not an input. A signed-request window such as
/// Binance's `recvWindow` bounds when a request can still be accepted, not
/// when an accepted order becomes visible to a lookup, and no venue used here
/// documents a visibility bound. A not-found stays `Unknown` however long ago
/// the attempt was, unless the venue itself refused every attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VenueFinalityRule {
    /// Final only when every submit attempt recorded for the client id ended
    /// in an authoritative venue rejection
    /// ([`AttemptEvidence::AllRejected`]).
    EveryAttemptRejected,
}

impl VenueFinalityRule {
    /// Classify a raw not-found: `Absent` only under this rule, otherwise
    /// `Unknown`. `evidence` must be read after the not-found response(s)
    /// returned, so an attempt that started during the lookup is seen.
    pub fn classify_not_found(self, evidence: AttemptEvidence) -> OrderLookup {
        match self {
            Self::EveryAttemptRejected => match evidence {
                AttemptEvidence::AllRejected { .. } => OrderLookup::Absent { finality: self },
                AttemptEvidence::NoRecord
                | AttemptEvidence::Truncated
                | AttemptEvidence::Unavailable
                | AttemptEvidence::Unresolved { .. } => OrderLookup::Unknown,
            },
        }
    }
}

/// What a [`SubmitAttemptLedger`] knows about one client id. It speaks only
/// for the current process: see the ledger's restart limitation, which is why
/// `NoRecord` does not mean "never attempted" and `AllRejected` does not cover
/// attempts made by an earlier process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttemptEvidence {
    /// No attempt on record: never attempted by this process.
    NoRecord,
    /// The ledger dropped evidence for capacity, so nothing can be concluded
    /// about any client id.
    Truncated,
    /// The durable evidence could not be read: nothing can be concluded.
    Unavailable,
    /// `unresolved` attempts are not proven rejected (in flight, dropped,
    /// accepted or indeterminate); `rejected` others were refused.
    Unresolved { unresolved: u64, rejected: u64 },
    /// Every attempt on record (`rejected`, at least one) ended in an
    /// authoritative venue rejection.
    AllRejected { rejected: u64 },
}

#[derive(Debug)]
struct AttemptCounts {
    unresolved: u64,
    rejected: u64,
}

#[derive(Debug, Default)]
struct LedgerState {
    attempts: HashMap<String, AttemptCounts>,
    truncated: bool,
}

/// Bounded in-memory evidence of submit attempts by client id, the input of
/// [`VenueFinalityRule`] (ADR-0056 §2).
///
/// An attempt is registered before its request can be sent and starts
/// unresolved. Its only transition is [`SubmitAttempt::rejected`], for an
/// authoritative venue rejection; acceptance, an indeterminate outcome and a
/// dropped future all leave it unresolved for good, so cancellation cannot
/// read as a rejection (there is no `Drop` to run). One unresolved attempt
/// blocks `Absent` whatever happened to other attempts with that client id.
/// The lock is private to these synchronous methods, so it cannot be held
/// across an await.
///
/// **Truncation.** A new client id at capacity drops all evidence and sets a
/// sticky flag: every client id reads [`AttemptEvidence::Truncated`] for the
/// rest of this instance's life. Evicting per entry would be unsound: once an
/// indeterminate attempt is evicted, a rejected retry starts a fresh record
/// that reads `AllRejected`.
///
/// **Across a restart.** On its own this ledger cannot see an earlier
/// process's attempts. With the [`OrderJournal`] attached,
/// [`Self::final_evidence`] answers from the durable rows; `evidence` stays
/// the in-memory view. A daemon with no database has neither, and every
/// not-found on an earlier process's id stays `Unknown`.
///
/// **One attempt per client id (ADR-0056 step 8).** A ledger built with
/// [`Self::one_attempt`] starts a send only by taking the grant its attached
/// journal gave the order ([`OrderJournal::authorize_send`]); with no journal
/// attached, or one without the capability, it refuses every send.
#[derive(Debug)]
pub struct SubmitAttemptLedger {
    capacity: usize,
    state: Mutex<LedgerState>,
    journal: OnceLock<Arc<OrderJournal>>,
    one_attempt: bool,
}

impl fmt::Debug for OrderJournal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("OrderJournal")
    }
}

impl SubmitAttemptLedger {
    /// A ledger tracking at most `capacity` client ids.
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            state: Mutex::new(LedgerState::default()),
            journal: OnceLock::new(),
            one_attempt: false,
        }
    }

    /// A ledger whose every send needs its journal's grant (ADR-0056 step 8):
    /// for a venue with `one_attempt_per_client_id`.
    pub fn one_attempt(capacity: usize) -> Self {
        Self { one_attempt: true, ..Self::new(capacity) }
    }

    /// Answer the finality rule from the durable journal from now on.
    pub fn attach_journal(&self, journal: Arc<OrderJournal>) {
        let _ = self.journal.set(journal);
    }

    fn lock(&self) -> MutexGuard<'_, LedgerState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Start a send: the gate's send start, then one attempt registered,
    /// unresolved until [`SubmitAttempt::rejected`]. Every send passes here
    /// last, immediately before its request (ADR-0056 step 7).
    ///
    /// # Errors
    /// The gate refused an entry because the daemon is stopping. Nothing is
    /// registered and nothing may be sent: a proven no-send.
    pub fn begin_send<'a>(
        &'a self,
        client_order_id: &'a str,
    ) -> Result<SubmitAttempt<'a>, SendRefusal> {
        if self.one_attempt {
            let Some(journal) = self.journal.get() else {
                return Err(SendRefusal::NotSent(
                    "send not granted: the one-attempt ledger has no journal".to_string(),
                ));
            };
            journal.begin_authorized_send(client_order_id)?;
        } else {
            begin_send(client_order_id)?;
        }
        Ok(self.register(client_order_id))
    }

    /// Register one attempt and report its ticket without the gate's refusal:
    /// for tests of the ledger. A send goes through [`Self::begin_send`].
    pub fn begin<'a>(&'a self, client_order_id: &'a str) -> SubmitAttempt<'a> {
        report_send_started(client_order_id);
        self.register(client_order_id)
    }

    fn register<'a>(&'a self, client_order_id: &'a str) -> SubmitAttempt<'a> {
        let mut state = self.lock();
        if !state.truncated {
            if let Some(counts) = state.attempts.get_mut(client_order_id) {
                counts.unresolved += 1;
            } else if state.attempts.len() < self.capacity {
                let counts = AttemptCounts { unresolved: 1, rejected: 0 };
                state.attempts.insert(client_order_id.to_string(), counts);
            } else {
                state.attempts = HashMap::new();
                state.truncated = true;
                tracing::warn!(
                    capacity = self.capacity,
                    "Submit-attempt evidence truncated; every not-found is Unknown until restart"
                );
            }
        }
        SubmitAttempt { ledger: self, client_order_id }
    }

    /// Run `request` as one attempt with `client_order_id`. The gate's send
    /// start runs and the attempt is on record immediately before `request` is
    /// first polled, with no await in between, so before anything can be sent;
    /// the attempt stays unresolved if this future is dropped at any point.
    /// `is_rejection` is the venue's allow-list of authoritative rejections;
    /// any other outcome leaves the attempt unresolved.
    ///
    /// # Errors
    /// `request`'s error, or the gate's refusal of an entry, which never polls
    /// `request`.
    pub async fn submit<T, E: From<SendRefusal>>(
        &self,
        client_order_id: &str,
        is_rejection: impl FnOnce(&E) -> bool,
        request: impl Future<Output = Result<T, E>>,
    ) -> Result<T, E> {
        let attempt = self.begin_send(client_order_id)?;
        let result = request.await;
        if let Err(error) = &result {
            if is_rejection(error) {
                attempt.rejected();
            }
        }
        result
    }

    /// The evidence the finality rule decides on (ADR-0056 step 7, rule 4):
    /// with a journal attached, the durable rows and the journal's memory, and
    /// an attempt unresolved here can only keep it unresolved. Without one,
    /// this ledger alone.
    pub async fn final_evidence(&self, client_order_id: &str) -> AttemptEvidence {
        let local = self.evidence(client_order_id);
        let Some(journal) = self.journal.get() else {
            return local;
        };
        let durable = journal.attempt_evidence(client_order_id).await;
        match (durable, self.evidence(client_order_id), local) {
            (
                AttemptEvidence::AllRejected { .. },
                unresolved @ AttemptEvidence::Unresolved { .. },
                _,
            )
            | (
                AttemptEvidence::AllRejected { .. },
                _,
                unresolved @ AttemptEvidence::Unresolved { .. },
            ) => unresolved,
            (durable, ..) => durable,
        }
    }

    /// The evidence for `client_order_id` as of now.
    pub fn evidence(&self, client_order_id: &str) -> AttemptEvidence {
        let state = self.lock();
        if state.truncated {
            return AttemptEvidence::Truncated;
        }
        match state.attempts.get(client_order_id) {
            None => AttemptEvidence::NoRecord,
            Some(counts) if counts.unresolved == 0 => {
                AttemptEvidence::AllRejected { rejected: counts.rejected }
            },
            Some(counts) => AttemptEvidence::Unresolved {
                unresolved: counts.unresolved,
                rejected: counts.rejected,
            },
        }
    }
}

/// One attempt registered by [`SubmitAttemptLedger::begin`]. Dropping it
/// leaves the attempt unresolved.
pub struct SubmitAttempt<'a> {
    ledger: &'a SubmitAttemptLedger,
    client_order_id: &'a str,
}

impl SubmitAttempt<'_> {
    /// The venue authoritatively refused this attempt. Consumes the handle, so
    /// one attempt cannot be counted twice.
    pub fn rejected(self) {
        report_rejected(self.client_order_id);
        let mut state = self.ledger.lock();
        // Missing only after truncation, when nothing is recorded any more.
        if let Some(counts) = state.attempts.get_mut(self.client_order_id) {
            counts.unresolved -= 1;
            counts.rejected += 1;
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
    /// Whether every client id is sent at most once (ADR-0056 step 8): the
    /// venue accepts an id again once its earlier order filled (Binance spot,
    /// F12), so a resubmission under the same id bounds nothing. The journal
    /// built for such a venue refuses a second attempt and grants each send.
    pub one_attempt_per_client_id: bool,
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
///
/// On a balance account (ADR-0056 step 8) `acceptance.record` carries the
/// venue's record of the stop, whatever its status, and `Ok` by itself never
/// means the stop is resting or the leg protected: the record decides.
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
    /// venue's [`VenueFinalityRule`]; a raw not-found outside that rule is
    /// `Unknown`.
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

/// Lifecycle phase of an order, read from its status alone (ADR-0056 step
/// 8, design §3.4.2). The expiry reason never changes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrderPhase {
    /// On the book or executing (Binance spot: `NEW`, `PARTIALLY_FILLED`,
    /// `PENDING_NEW`).
    Live,
    /// Nothing more executes (Binance spot: `FILLED`, `CANCELED`, `EXPIRED`,
    /// `EXPIRED_IN_MATCH`, `REJECTED`).
    Terminal,
    /// A status that is neither (Binance spot: `PENDING_CANCEL`, documented as
    /// unused, and anything unrecognised).
    Unknown,
}

/// The venue's own record of one order, as one answer gave it (ADR-0056 step
/// 8, design §3.6): what a balance account's evidence rests on. Quantities are
/// the venue's; nothing here is derived from fills or fees.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VenueOrderRecord {
    pub exchange_order_id: String,
    /// The order's client id now. A cancel renames it on Binance spot (F12).
    pub client_order_id: String,
    /// The id the order was placed under, when the answer names it apart
    /// (a cancel answer's `origClientOrderId`).
    pub orig_client_order_id: Option<String>,
    /// The venue's symbol, as given (`BTCUSDT`).
    pub symbol: String,
    pub side: OrderSide,
    /// The venue's order type, as given (`MARKET`, `STOP_LOSS`, ...).
    pub order_type: String,
    /// Set by the adapter only when `order_type` is exactly the venue's
    /// exchange-resident protection primitive (Binance spot: `STOP_LOSS`, never
    /// `STOP_LOSS_LIMIT`).
    pub protection_kind: Option<ProtectionKind>,
    /// The venue's status, as given.
    pub status: String,
    pub phase: OrderPhase,
    pub orig_qty: Decimal,
    /// Cumulative executed quantity.
    pub executed_qty: Decimal,
    /// Cumulative quote quantity; `None` when the venue reports it as
    /// unavailable (never read as zero).
    pub cumulative_quote_qty: Option<Decimal>,
    pub stop_price: Option<Decimal>,
    /// Recorded, never used to decide (design §3.4.2).
    pub expiry_reason: Option<String>,
    /// The venue's times as given, in milliseconds: recorded for audit, never
    /// used to decide (design §16, v10).
    pub transact_time_ms: Option<i64>,
    pub update_time_ms: Option<i64>,
}

impl VenueOrderRecord {
    /// `0 ≤ executed_qty ≤ orig_qty`: a record outside it is no evidence.
    pub fn quantities_consistent(&self) -> bool {
        self.executed_qty >= Decimal::ZERO && self.executed_qty <= self.orig_qty
    }
}

/// Which order a single observation asks for (ADR-0056 step 8). By exchange id
/// once one is known; by client id only while none is recorded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OrderRef {
    ExchangeId(String),
    ClientId(String),
}

/// The typed answer to one cancel request (ADR-0056 step 8,
/// [`InventoryPort::cancel_order_once`]).
#[derive(Debug, Clone, PartialEq)]
pub enum CancelAnswer {
    /// The venue cancelled it: the answer's own record is terminal, matches
    /// the order and has consistent quantities. Boxed: the record is large.
    Cancelled(Box<VenueOrderRecord>),
    /// The venue said the order is not open (Binance spot: HTTP 400 with
    /// -2011 or -2013). It proves nothing about execution: an exact lookup
    /// decides, in a later request.
    NotFound { code: i64 },
    /// Sent, and the answer established nothing.
    Unknown { reason: String },
}

/// The typed answer to one order lookup (ADR-0056 step 8,
/// [`InventoryPort::observe_order`]).
#[derive(Debug, Clone, PartialEq)]
pub enum OrderObservation {
    /// The order's own record: ids and symbol match, quantities consistent.
    /// Boxed: the record is large.
    Found(Box<VenueOrderRecord>),
    /// The venue does not know it (Binance spot: HTTP 400 with -2013). Only
    /// the caller, through [`VenueFinalityRule::classify_not_found`] over the
    /// journal's evidence, may turn this into [`OrderLookup::Absent`].
    NotFound,
    /// Archived by the venue (Binance spot: HTTP 400 with -2026). Never
    /// [`OrderLookup::Absent`].
    Archived,
}

/// One page of an order's trades (ADR-0056 step 8,
/// [`InventoryPort::order_trades_page`]).
#[derive(Debug, Clone, PartialEq)]
pub struct TradePage {
    /// Distinct trades of the order, each with its time.
    pub trades: Vec<TradeFill>,
    /// Where the next page starts, when this one was full; `None` when the
    /// venue had no more. Stored durably by the caller, and read in a later
    /// request.
    pub next_from_id: Option<u64>,
}

/// Outcome of cancelling a protective stop (ADR-0056 §2).
#[derive(Debug, Clone)]
pub enum StopCancelOutcome {
    /// The venue acknowledged the cancel. `filled_before_cancel` is the
    /// stop's cumulative execution at cancel time (zero for a conditional
    /// stop that never triggered). On a balance account (ADR-0056 step 8)
    /// only an answer of exactly success whose own record is terminal, with a
    /// parsed, consistent executed quantity, is this; any other success is
    /// `Unknown`.
    Cancelled {
        filled_before_cancel: Decimal,
        fills: Option<OrderResult>,
    },
    /// The venue no longer knows the order; the commonest reason is that it
    /// triggered. On a position account, `final_state` is `Unknown` unless the
    /// adapter established it without an extra venue request. On a balance
    /// account (ADR-0056 step 8, design §3.4.2), after the venue's not-found
    /// the adapter looks the order up once by its exchange id and returns what
    /// that establishes; a further not-found proves nothing. `Terminal.fills`
    /// is `Some` only when trades with distinct ids sum to `filled_qty`.
    AlreadyGone { final_state: OrderLookup },
    /// The cancel outcome is indeterminate; reconcile before any further
    /// submission.
    Unknown,
}

/// Outcome of replacing a protective stop (ADR-0056 §2).
#[derive(Debug, Clone)]
pub enum ReplaceOutcome {
    /// The successor is accepted and the old stop is gone with `old_filled`
    /// cumulative execution. Under `CancelThenPlace` the successor's record
    /// is live, unexecuted and matches what was asked; it still certifies no
    /// protection (ADR-0056 step 8: that is the core's).
    Replaced {
        new: PlacedStop,
        old_filled: Decimal,
    },
    /// The venue authoritatively rejected every attempt with the successor's
    /// client id; the old stop was never touched.
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
    /// `CancelThenPlace` only: the old stop executed `old_filled` before it
    /// ended, so the quantity to protect changed. Nothing was placed: the
    /// caller recomputes it (ADR-0056 step 8).
    OldExecuted { old_filled: Decimal },
    /// `CancelThenPlace` only: the successor answered live and executing.
    SuccessorExecuting { new: PlacedStop },
    /// `CancelThenPlace` only: the successor's own record is terminal (an
    /// expiry with nothing executed included): it protects nothing.
    SuccessorTerminal { new: PlacedStop },
    /// `CancelThenPlace` only: the successor is live but its record does not
    /// match what was asked (symbol, references, side, protection kind,
    /// quantity, trigger).
    SuccessorMismatched { new: PlacedStop },
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

/// An open order observed account-wide, with the symbol it rests on (the
/// legacy [`OpenOrderRecord`] comes from a per-symbol query and has none).
#[derive(Debug, Clone, PartialEq)]
pub struct AccountOpenOrder {
    pub symbol: Symbol,
    pub order: OpenOrderRecord,
    /// The order's full record, when the venue's listing gives one (ADR-0056
    /// step 8).
    pub record: Option<VenueOrderRecord>,
}

/// Account-wide observation of a balance account: every nonzero holding of
/// every asset and every open order, independent of configured symbols.
///
/// Its reads are not one atomic snapshot (design §3.1): it detects and bounds,
/// and never certifies by itself; deciding when an observation is coherent is
/// reconciliation's (ADR-0056 step 8, 8k).
#[derive(Debug, Clone, PartialEq)]
pub struct InventoryObservation {
    pub holdings: Vec<(String, Holding)>,
    pub open_orders: Vec<AccountOpenOrder>,
    pub as_of: DateTime<Utc>,
    /// Open order lists on the account (Robson places none).
    pub open_order_lists: usize,
    /// The account's identity and authorisation as the venue reports them.
    pub uid: String,
    pub can_trade: bool,
    pub account_type: String,
    pub permissions: Vec<String>,
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

    /// Rules for admission (ADR-0056 §5): validated within the TTL or
    /// refreshed now, never a stale snapshot. An error fails admission closed.
    /// Default: `trading_rules`.
    async fn trading_rules_fresh(&self, symbol: &Symbol) -> Result<SymbolTradingRules, ExecError> {
        self.trading_rules(symbol).await
    }

    /// The last validated rules of any age, for risk reduction (ADR-0056 §5):
    /// no venue request and no wait on a refresh in flight. Default: `None`.
    fn last_validated_rules(&self, _symbol: &Symbol) -> Option<SymbolTradingRules> {
        None
    }

    /// Whether the last validated rules are missing or past their TTL, so a
    /// caller that must not wait can ask for a refresh on the side.
    /// Default: `false`.
    fn rules_refresh_due(&self, _symbol: &Symbol) -> bool {
        false
    }

    /// Re-read the rules after a rules rejection, updating what
    /// `last_validated_rules` serves. Never serves a stale snapshot. Bounded by
    /// the venue under the ADR-0056 §10 ceiling. Default: `trading_rules`.
    async fn refresh_trading_rules(
        &self,
        symbol: &Symbol,
    ) -> Result<SymbolTradingRules, ExecError> {
        self.trading_rules(symbol).await
    }

    /// Place a market order. `intent = Reduce` must only reduce exposure.
    ///
    /// `Entry` validates against fresh rules and fails closed; `Reduce` sends
    /// `quantity` as given, with no metadata lookup here: [`crate::quantize`]
    /// decides it (ADR-0056 §5).
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
    /// (ADR-0021), never a percentage of entry. `quantity` and `trigger` are
    /// sent as given, with no metadata lookup: [`crate::quantize`] decides
    /// them. On a balance account `Ok` carries the venue's record in
    /// `acceptance.record` and does not by itself mean resting (ADR-0056 step
    /// 8).
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

    /// Look up an order by the client id it was submitted with, in every
    /// order namespace the venue places under (regular and conditional).
    /// A not-found is `Absent` only under the venue's [`VenueFinalityRule`],
    /// evaluated against the [`SubmitAttemptLedger`] evidence for this client
    /// id, read after the not-found responses returned; otherwise it is
    /// `Unknown`, however much time has passed. A request failure is `Err`.
    ///
    /// With the order journal attached the evidence is durable and spans
    /// restarts (ADR-0056 step 7, rule 4): at least one row, all rejected,
    /// nothing unlanded in memory.
    async fn order_by_client_id(
        &self,
        symbol: &Symbol,
        client_order_id: &str,
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
///
/// The three single-request methods are what the leg owner uses for
/// cancellation and recovery reads (design §3.5.1, §3.14): each sends exactly
/// one request, never retries and holds no lock. `Err` is a proven no-send or,
/// for the reads, a request that established nothing. They have no default
/// bodies: a default could only refuse, never invent an absence or evidence.
#[async_trait]
pub trait InventoryPort: Send + Sync {
    async fn account_observation(&self) -> Result<InventoryObservation, ExecError>;

    async fn governed_inventory(&self, symbol: &Symbol) -> Result<GovernedInventory, ExecError>;

    /// One cancel request by exchange order id.
    async fn cancel_order_once(
        &self,
        symbol: &Symbol,
        order_id: &str,
    ) -> Result<CancelAnswer, ExecError>;

    /// One lookup, returning the order's full record.
    async fn observe_order(
        &self,
        symbol: &Symbol,
        by: &OrderRef,
    ) -> Result<OrderObservation, ExecError>;

    /// One page of an order's trades, from trade id `from_id` on.
    async fn order_trades_page(
        &self,
        symbol: &Symbol,
        order_id: &str,
        from_id: u64,
    ) -> Result<TradePage, ExecError>;
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
    /// Give the venue's attempt ledger the durable journal. A venue without a
    /// ledger has nothing to attach.
    fn attach_order_journal(&self, _journal: Arc<OrderJournal>) {}
    /// Give the venue's transport the permit gate (ADR-0056 step 7): every
    /// mutation it sends passes [`OperationGate::mutation`] first. No default:
    /// a venue says how its mutations reach the gate.
    fn attach_gate(&self, gate: Arc<OperationGate>);
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
///
/// A failed placement is classified from `attempts`, the ledger `venue`
/// registers its placements in, read after the failure returned; the error
/// variant proves nothing (Binance `-2010` maps to `OrderRejected` at any HTTP
/// status). `OldKeptNewRejected` requires
/// [`AttemptEvidence::AllRejected`] for the successor's client id. Anything
/// else is `Unknown { new: None }`, to reconcile before any further
/// submission: a rejected retry after an earlier unresolved attempt, truncated
/// evidence, and a placement refused locally before anything was sent
/// (`NoRecord`).
pub async fn replace_place_then_cancel<T: TradingPort + ?Sized>(
    venue: &T,
    attempts: &SubmitAttemptLedger,
    symbol: &Symbol,
    old: &StopRef,
    new: StopParams,
) -> Result<ReplaceOutcome, ExecError> {
    let placed = match venue
        .place_protection(symbol, new.close_side, new.quantity, new.trigger, &new.client_order_id)
        .await
    {
        Ok(placed) => placed,
        Err(error) => {
            return Ok(match attempts.final_evidence(&new.client_order_id).await {
                AttemptEvidence::AllRejected { .. } => {
                    ReplaceOutcome::OldKeptNewRejected { reason: error.to_string() }
                },
                // The successor may exist.
                AttemptEvidence::NoRecord
                | AttemptEvidence::Truncated
                | AttemptEvidence::Unavailable
                | AttemptEvidence::Unresolved { .. } => ReplaceOutcome::Unknown { new: None },
            });
        },
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

/// `ReplacementProtocol::CancelThenPlace` over [`TradingPort`] (ADR-0056 step
/// 8): the venue-neutral helper for the conformance suite. Production
/// replacement on a balance account is the core's transition protocol.
///
/// The old stop is cancelled first and nothing is placed unless that cancel
/// is definite (R1). When the old stop executed, the quantity to protect
/// changed: [`ReplaceOutcome::OldExecuted`], nothing placed. A failed
/// placement is a rejection only with [`AttemptEvidence::AllRejected`] for the
/// successor's id, read after the failure; the venue's message is never read,
/// so this never yields `TriggerAlreadyCrossed`. A placement that answered is
/// read from its record: only a live, unexecuted record matching what was
/// asked is [`ReplaceOutcome::Replaced`], which still certifies no protection.
pub async fn replace_cancel_then_place<T: TradingPort + AccountPort + ?Sized>(
    venue: &T,
    attempts: &SubmitAttemptLedger,
    symbol: &Symbol,
    old: &StopRef,
    new: StopParams,
) -> Result<ReplaceOutcome, ExecError> {
    let old_filled = match venue.cancel_protection(symbol, old).await {
        Ok(StopCancelOutcome::Cancelled { filled_before_cancel, .. }) => filled_before_cancel,
        Ok(StopCancelOutcome::AlreadyGone {
            final_state: OrderLookup::Terminal { filled_qty, .. },
        }) => filled_qty,
        Ok(_) | Err(_) => return Ok(ReplaceOutcome::Unknown { new: None }),
    };
    if old_filled != Decimal::ZERO {
        return Ok(ReplaceOutcome::OldExecuted { old_filled });
    }
    let placed = match venue
        .place_protection(symbol, new.close_side, new.quantity, new.trigger, &new.client_order_id)
        .await
    {
        Ok(placed) => placed,
        Err(error) => {
            return Ok(match attempts.final_evidence(&new.client_order_id).await {
                AttemptEvidence::AllRejected { .. } => ReplaceOutcome::OldCancelledNewRejected {
                    old_filled,
                    reason: error.to_string(),
                },
                AttemptEvidence::NoRecord
                | AttemptEvidence::Truncated
                | AttemptEvidence::Unavailable
                | AttemptEvidence::Unresolved { .. } => ReplaceOutcome::Unknown { new: None },
            });
        },
    };
    let protection = venue.capabilities().protection;
    let Some(record) = placed.acceptance.record.clone() else {
        return Ok(ReplaceOutcome::Unknown { new: Some(placed) });
    };
    Ok(match record.phase {
        OrderPhase::Unknown => ReplaceOutcome::Unknown { new: Some(placed) },
        OrderPhase::Terminal => ReplaceOutcome::SuccessorTerminal { new: placed },
        OrderPhase::Live if record.executed_qty > Decimal::ZERO => {
            ReplaceOutcome::SuccessorExecuting { new: placed }
        },
        OrderPhase::Live if successor_matches(&record, symbol, &placed.stop, &new, protection) => {
            ReplaceOutcome::Replaced { new: placed, old_filled }
        },
        OrderPhase::Live => ReplaceOutcome::SuccessorMismatched { new: placed },
    })
}

/// Whether a successor's own record is the stop that was asked for.
fn successor_matches(
    record: &VenueOrderRecord,
    symbol: &Symbol,
    stop: &StopRef,
    new: &StopParams,
    protection: ProtectionKind,
) -> bool {
    record.symbol == symbol.as_pair()
        && record.exchange_order_id == stop.exchange_order_id
        && record.client_order_id == new.client_order_id
        && record.side == new.close_side
        && record.protection_kind == Some(protection)
        && record.orig_qty == new.quantity.as_decimal()
        && record.stop_price == Some(new.trigger.as_decimal())
        && record.executed_qty == Decimal::ZERO
}

#[cfg(test)]
#[path = "replace_tests.rs"]
mod replace_tests;

#[cfg(test)]
mod tests {
    use super::*;

    const RULE: VenueFinalityRule = VenueFinalityRule::EveryAttemptRejected;

    fn is_absent(ledger: &SubmitAttemptLedger, id: &str) -> bool {
        matches!(
            RULE.classify_not_found(ledger.evidence(id)),
            OrderLookup::Absent { finality } if finality == RULE
        )
    }

    fn is_unknown(ledger: &SubmitAttemptLedger, id: &str) -> bool {
        matches!(RULE.classify_not_found(ledger.evidence(id)), OrderLookup::Unknown)
    }

    /// Time cannot promote a lookup because no input carries it: the signature
    /// takes evidence only, the evidence holds counts only, and the ledger
    /// reads no clock. Whenever the lookup runs, these inputs give these
    /// outputs.
    #[test]
    fn elapsed_time_cannot_promote_a_not_found() {
        let classify: fn(VenueFinalityRule, AttemptEvidence) -> OrderLookup =
            VenueFinalityRule::classify_not_found;
        for evidence in [
            AttemptEvidence::NoRecord,
            AttemptEvidence::Truncated,
            AttemptEvidence::Unresolved { unresolved: 1, rejected: 0 },
            AttemptEvidence::Unresolved { unresolved: 1, rejected: u64::MAX },
        ] {
            assert!(matches!(classify(RULE, evidence), OrderLookup::Unknown), "{evidence:?}");
        }
    }

    #[test]
    fn no_evidence_is_unknown() {
        let ledger = SubmitAttemptLedger::new(8);
        assert_eq!(ledger.evidence("c1"), AttemptEvidence::NoRecord);
        assert!(is_unknown(&ledger, "c1"));
    }

    #[test]
    fn all_attempts_rejected_is_absent() {
        let ledger = SubmitAttemptLedger::new(8);
        ledger.begin("c1").rejected();
        assert_eq!(ledger.evidence("c1"), AttemptEvidence::AllRejected { rejected: 1 });
        assert!(is_absent(&ledger, "c1"));
        ledger.begin("c1").rejected();
        assert_eq!(ledger.evidence("c1"), AttemptEvidence::AllRejected { rejected: 2 });
        assert!(is_absent(&ledger, "c1"));
        assert!(is_unknown(&ledger, "other"));
    }

    #[test]
    fn in_flight_attempt_is_unknown_until_it_is_rejected() {
        let ledger = SubmitAttemptLedger::new(8);
        let attempt = ledger.begin("c1");
        assert_eq!(ledger.evidence("c1"), AttemptEvidence::Unresolved {
            unresolved: 1,
            rejected: 0
        });
        assert!(is_unknown(&ledger, "c1"));
        attempt.rejected();
        assert!(is_absent(&ledger, "c1"));
    }

    #[test]
    fn mixed_rejected_and_accepted_is_unknown_in_either_order() {
        // Nothing ever resolves an accepted (or indeterminate) attempt.
        let ledger = SubmitAttemptLedger::new(8);
        ledger.begin("rejected-first").rejected();
        ledger.begin("rejected-first");
        ledger.begin("accepted-first");
        ledger.begin("accepted-first").rejected();
        for id in ["rejected-first", "accepted-first"] {
            assert_eq!(ledger.evidence(id), AttemptEvidence::Unresolved {
                unresolved: 1,
                rejected: 1
            });
            assert!(is_unknown(&ledger, id));
        }
    }

    fn refused() -> ExecError {
        ExecError::OrderRejected("refused".to_string())
    }

    #[tokio::test]
    async fn submit_registers_before_the_request_is_polled() {
        let ledger = SubmitAttemptLedger::new(8);
        let request = async {
            assert_eq!(ledger.evidence("c1"), AttemptEvidence::Unresolved {
                unresolved: 1,
                rejected: 0
            });
            Err::<(), ExecError>(ExecError::OrderRejected("refused".to_string()))
        };
        let submit = ledger.submit("c1", |_| true, request);
        // Futures are lazy: built but never polled, nothing was sent.
        assert_eq!(ledger.evidence("c1"), AttemptEvidence::NoRecord);
        assert!(submit.await.is_err());
        assert_eq!(ledger.evidence("c1"), AttemptEvidence::AllRejected { rejected: 1 });
    }

    #[tokio::test]
    async fn dropped_submit_stays_unresolved_even_after_a_rejected_retry() {
        let ledger = SubmitAttemptLedger::new(8);
        let stuck = ledger.submit(
            "c1",
            |_: &ExecError| true,
            std::future::pending::<Result<(), ExecError>>(),
        );
        // One poll sends the request; the elapsed timeout then drops the future.
        assert!(tokio::time::timeout(Duration::from_millis(1), stuck).await.is_err());
        assert_eq!(ledger.evidence("c1"), AttemptEvidence::Unresolved {
            unresolved: 1,
            rejected: 0
        });
        assert!(is_unknown(&ledger, "c1"));

        let retry = ledger.submit("c1", |_| true, async { Err::<(), _>(refused()) });
        assert!(retry.await.is_err());
        assert_eq!(ledger.evidence("c1"), AttemptEvidence::Unresolved {
            unresolved: 1,
            rejected: 1
        });
        assert!(is_unknown(&ledger, "c1"));
    }

    #[tokio::test]
    async fn submit_leaves_outcomes_outside_the_allow_list_unresolved() {
        let ledger = SubmitAttemptLedger::new(8);
        let accepted = ledger.submit("ok", |_: &ExecError| true, async { Ok::<(), _>(()) });
        accepted.await.unwrap();
        let failed = ledger.submit("failed", |_| false, async { Err::<(), _>(refused()) });
        assert!(failed.await.is_err());
        for id in ["ok", "failed"] {
            assert_eq!(ledger.evidence(id), AttemptEvidence::Unresolved {
                unresolved: 1,
                rejected: 0
            });
        }
    }

    #[test]
    fn truncation_is_sticky_and_a_rejected_retry_cannot_look_fresh() {
        let ledger = SubmitAttemptLedger::new(2);
        ledger.begin("indeterminate");
        ledger.begin("refused").rejected();
        assert!(is_absent(&ledger, "refused"));
        // Repeating a tracked client id at capacity needs no new entry.
        ledger.begin("refused").rejected();
        assert!(is_absent(&ledger, "refused"));

        let in_flight = ledger.begin("refused");
        ledger.begin("overflow").rejected();
        // A fresh record for this retry would read `AllRejected`.
        ledger.begin("indeterminate").rejected();
        in_flight.rejected();
        for id in ["indeterminate", "refused", "overflow", "never"] {
            assert_eq!(ledger.evidence(id), AttemptEvidence::Truncated);
            assert!(is_unknown(&ledger, id));
        }
        // Everything was dropped and nothing is recorded any more.
        assert!(ledger.lock().attempts.is_empty());
    }

    #[test]
    fn ledger_stays_within_capacity() {
        let ledger = SubmitAttemptLedger::new(4);
        for i in 0..4 {
            ledger.begin(&format!("c{i}")).rejected();
            assert!(ledger.lock().attempts.len() <= 4);
        }
        assert!(is_absent(&ledger, "c0"));
        ledger.begin("c4").rejected();
        assert!(ledger.lock().attempts.is_empty());
        assert_eq!(ledger.evidence("c0"), AttemptEvidence::Truncated);
    }

    #[test]
    fn entry_or_reduce_round_trips_the_legacy_flag() {
        assert_eq!(EntryOrReduce::from_reduce_only(true), EntryOrReduce::Reduce);
        assert_eq!(EntryOrReduce::from_reduce_only(false), EntryOrReduce::Entry);
        assert!(EntryOrReduce::Reduce.is_reduce() && !EntryOrReduce::Entry.is_reduce());
    }
}
