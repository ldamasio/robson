//! Domain Entities for Robson v2
//!
//! Core business entities with lifecycle management.
//! All entities have identity and state transitions.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    stop_policy::StopPolicy,
    value_objects::{
        DomainError, OrderSide, Price, Quantity, RiskConfig, Side, Symbol, TechnicalStopDistance,
    },
};

// =============================================================================
// Position ID
// =============================================================================

/// Unique identifier for a Position
pub type PositionId = Uuid;

/// Unique identifier for an Order
pub type OrderId = Uuid;

/// Unique identifier for an Account
pub type AccountId = Uuid;

// =============================================================================
// Position
// =============================================================================

/// Position represents a managed trading position with full lifecycle
///
/// Key concepts:
/// - NO stop_gain: Exit happens when trailing stop is hit
/// - Trailing stop uses 1x technical stop distance technique
/// - USD-M Futures trading with fixed 1x leverage
/// - Margin availability is the physical bound for stop-derived sizing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Position {
    pub id: PositionId,
    pub account_id: AccountId,
    pub symbol: Symbol,
    pub side: Side,
    pub state: PositionState,

    // Entry parameters
    pub entry_price: Option<Price>,
    pub entry_filled_at: Option<DateTime<Utc>>,

    // Technical stop distance (trailing stop anchor)
    pub tech_stop_distance: Option<TechnicalStopDistance>,

    // Position sizing (1x leverage is implicit)
    pub quantity: Quantity,

    // P&L Tracking
    pub realized_pnl: rust_decimal::Decimal,
    pub fees_paid: rust_decimal::Decimal,

    // Associated orders
    pub entry_order_id: Option<OrderId>,
    pub exit_order_id: Option<OrderId>,
    /// Exchange-assigned order id of the protective insurance stop, when one
    /// is live on the exchange for this position (ADR-0039). Mirrors the id
    /// held in `PositionState::Active` so the reconciliation worker can read it
    /// without a state match.
    pub insurance_stop_id: Option<String>,
    /// Binance exchange identifier captured on entry fill for Core/SafetyNet
    /// coordination.
    pub binance_position_id: Option<String>,

    /// Stop-policy version pinned at arm time (issue #154). Missing on wire
    /// data written before versioning = `LegacyUncapped`; the policy never
    /// changes for the lifetime of the position, so a deploy cannot retroact
    /// on live positions.
    #[serde(default)]
    pub stop_policy: StopPolicy,
    /// ADR-0041 buffer (basis points) snapshotted at arm. `None` on
    /// positions armed before stop-policy versioning: those follow the live
    /// `ROBSON_STOP_BUFFER_BPS` config, the historical behavior. When
    /// present, the snapshot is authoritative so a config change between
    /// restarts cannot move a live position's executable stop.
    #[serde(default)]
    pub stop_buffer_bps_at_arm: Option<rust_decimal::Decimal>,

    // Audit
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub closed_at: Option<DateTime<Utc>>,
}

impl Position {
    /// Create a new armed position with the legacy stop policy.
    ///
    /// This constructor never decides between Legacy and V1: it always
    /// yields `LegacyUncapped`, the safe default for every replay and
    /// recovery path. Arming a `SpanCappedV1` position is an EXPLICIT
    /// decision made through [`Self::new_with_stop_policy`].
    pub fn new(account_id: AccountId, symbol: Symbol, side: Side) -> Self {
        Self::new_with_stop_policy(account_id, symbol, side, StopPolicy::LegacyUncapped, None)
    }

    /// Create a new armed position under an explicit stop policy with the
    /// arm-time buffer snapshot (issue #154 deliverable 3).
    pub fn new_with_stop_policy(
        account_id: AccountId,
        symbol: Symbol,
        side: Side,
        stop_policy: StopPolicy,
        stop_buffer_bps_at_arm: Option<rust_decimal::Decimal>,
    ) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::now_v7(),
            account_id,
            symbol,
            side,
            state: PositionState::Armed,
            entry_price: None,
            entry_filled_at: None,
            tech_stop_distance: None,
            quantity: Quantity::zero(),
            realized_pnl: rust_decimal::Decimal::ZERO,
            fees_paid: rust_decimal::Decimal::ZERO,
            entry_order_id: None,
            exit_order_id: None,
            insurance_stop_id: None,
            binance_position_id: None,
            stop_policy,
            stop_buffer_bps_at_arm,
            created_at: now,
            updated_at: now,
            closed_at: None,
        }
    }

    /// Check if position can enter (is in Armed state)
    pub fn can_enter(&self) -> bool {
        matches!(self.state, PositionState::Armed)
    }

    /// Check if position can exit (is in Active state)
    pub fn can_exit(&self) -> bool {
        matches!(self.state, PositionState::Active { .. })
    }

    /// Check if position is closed
    pub fn is_closed(&self) -> bool {
        matches!(self.state, PositionState::Closed { .. })
    }

    /// Get current trailing stop price (only valid in Active state)
    pub fn get_trailing_stop(&self) -> Option<Price> {
        match &self.state {
            PositionState::Active { trailing_stop, .. } => Some(*trailing_stop),
            _ => None,
        }
    }

    /// Calculate realized P&L for this position
    ///
    /// For active positions, returns unrealized P&L.
    /// For closed positions, returns realized P&L from state.
    pub fn calculate_pnl(&self) -> rust_decimal::Decimal {
        let entry_price = match self.entry_price {
            Some(p) => p.as_decimal(),
            None => return rust_decimal::Decimal::ZERO,
        };

        match &self.state {
            PositionState::Active { current_price, .. } => {
                // Unrealized P&L
                let quantity = self.quantity.as_decimal();
                match self.side {
                    Side::Long => (current_price.as_decimal() - entry_price) * quantity,
                    Side::Short => (entry_price - current_price.as_decimal()) * quantity,
                }
            },
            PositionState::Closed { realized_pnl, .. } => *realized_pnl,
            _ => rust_decimal::Decimal::ZERO,
        }
    }
}

// =============================================================================
// Position Sizing (Golden Rule)
// =============================================================================

/// The admission-time sizing result: the final quantity plus the exact risk
/// numbers it was priced with, so callers charge the REAL planned risk
/// against the monthly budget (ADR-0043) instead of re-deriving it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SizedEntry {
    /// Final quantity: risk-sized, margin-capped, and lot-step quantized
    /// down when trading rules are available.
    pub quantity: Quantity,
    /// Worst expected realized loss per unit (adverse-fill costing from the
    /// executable stop plan).
    pub worst_case_loss_per_unit: rust_decimal::Decimal,
    /// `worst_case_loss_per_unit x quantity`, recomputed AFTER quantization.
    /// Guaranteed `<= capital x 1%`; an overrun is a hard error, never a
    /// silently clamped report.
    pub planned_risk: rust_decimal::Decimal,
}

/// Calculate the admission-time entry size from the executable stop plan
/// (ADR-0050 §3, issue #154 deliverable 4).
///
/// **THE GOLDEN RULE**: position size is DERIVED from the stop distance, and
/// the 1% budget is a maximum-loss CEILING that must absorb the worst
/// expected realized loss through the executable path:
///
/// ```text
/// trigger          = plan.trigger (tick-quantized under span_capped_v1)
/// gap              = trigger x gap_bps / 10_000
/// adverse_fill     = Long: trigger - gap | Short: trigger + gap
/// worst_loss/unit  = directional_distance(entry, adverse_fill)
///                  + taker_fee x entry + taker_fee x adverse_fill
/// Risk-sized qty   = Max Risk Amount / worst_loss_per_unit   (truncated)
/// Margin basis     = min(Capital, Available Margin)
/// Margin-sized qty = Margin basis x (1 - margin_headroom) / Entry Price
/// qty              = quantize_down(min(risk qty, margin qty), lot step)
/// planned_risk     = qty x worst_loss_per_unit               (<= ceiling)
/// ```
///
/// Validity stages re-checked here against the single bounds source
/// (ADR-0050 §5): the raw technical span (stage 1), the guard-aware basis
/// (stage 2), and the FINAL executable trigger (stage 3, typed
/// [`DomainError::ExecutableStopOutOfBounds`]).
///
/// # Errors
///
/// Returns a `DomainError` when any validity stage fails, the lot-quantized
/// quantity falls below the exchange minimum, the notional is below
/// `MIN_NOTIONAL`, or the recomputed planned risk would exceed the ceiling
/// (never masked by clamping the reported number).
pub fn size_entry(
    risk_config: &RiskConfig,
    entry_price: &Price,
    tech_stop: &TechnicalStopDistance,
    plan: &crate::executable_stop::ExecutableStopPlan,
    available_margin: Option<rust_decimal::Decimal>,
    rules: Option<&crate::trading_rules::SymbolTradingRules>,
) -> Result<SizedEntry, DomainError> {
    let bounds = risk_config.stop_distance_bounds();

    // Stage 1: raw technical span.
    tech_stop.validate_with_bounds(&bounds)?;
    // Stage 2: guard-aware basis (a guard that widens the distance past the
    // cap rejects the entry, ADR-0042 "guard too wide").
    TechnicalStopDistance::from_entry_and_stop(*entry_price, plan.basis)
        .validate_with_bounds(&bounds)?;
    // Stage 3: FINAL executable trigger, post guard + cap + tick. Typed so
    // the rejection carries its own reason code.
    plan.validate_admission_bounds(*entry_price, &bounds)?;

    let entry = entry_price.as_decimal();
    let worst_loss_per_unit =
        crate::executable_stop::worst_case_loss_per_unit_planned(risk_config, *entry_price, plan)?;

    let max_risk = risk_config.max_risk_amount();
    // Truncate the risk-sized quantity (12 dp, far finer than any exchange
    // lot step) so `qty x per_unit <= max_risk` holds exactly in Decimal:
    // nearest-rounding of the 28-digit quotient could land a hair ABOVE the
    // cap, which used to be masked by clamping the reported risk.
    let risk_sized_qty = (max_risk / worst_loss_per_unit)
        .round_dp_with_strategy(12, rust_decimal::RoundingStrategy::ToZero);
    // The margin cap is a PHYSICAL bound, distinct from the policy capital
    // that anchors the 1% risk budget (ADR-0024 §6: capital_base stays at the
    // month-start snapshot while governed losses accrue). Its basis is the
    // LIVE exchange balance when known — mid-month drawdown makes the wallet
    // smaller than the policy capital, and an order sized past the wallet is
    // rejected by the exchange no matter what the ledger says (2026-07-04
    // prod incident, Binance -2019 "Margin is insufficient"). The policy
    // capital stays as an upper bound so a wallet ABOVE the ledger never
    // sizes beyond policy. On top of the basis, the operator-configured
    // headroom covers the taker fee and the exchange's mark-price cushion,
    // and the cap is truncated to 8 decimal places (finer than any exchange
    // quantity step) so the risk gate's qty × entry round-trip is exact in
    // Decimal and provably within the basis at 1x.
    let margin_basis = match available_margin {
        Some(available) => risk_config.capital().min(available),
        None => risk_config.capital(),
    };
    if margin_basis <= rust_decimal::Decimal::ZERO {
        return Err(DomainError::PositionSizingError(
            "Available margin must be positive".to_string(),
        ));
    }
    let headroom_factor = (rust_decimal::Decimal::from(10_000) - risk_config.margin_headroom_bps())
        / rust_decimal::Decimal::from(10_000);
    let margin_sized_qty =
        ((margin_basis * headroom_factor * rust_decimal::Decimal::from(RiskConfig::LEVERAGE))
            / entry)
            .round_dp_with_strategy(8, rust_decimal::RoundingStrategy::ToZero);
    let mut position_size = risk_sized_qty.min(margin_sized_qty);

    // Lot-step quantization DOWN before submission (ADR-0050 §3): the
    // quantity the risk gate prices is the quantity the exchange executes.
    if let Some(rules) = rules {
        position_size = rules.quantize_qty_down(position_size);
        if position_size < rules.min_qty() {
            return Err(DomainError::PositionSizingError(format!(
                "Quantity {position_size} quantizes below {} minimum {} at lot step {}. \
                 Increase capital or widen the stop distance; Robson will not round up.",
                rules.symbol().as_pair(),
                rules.min_qty(),
                rules.step_size()
            )));
        }
        rules
            .validate_notional(entry, position_size)
            .map_err(|e| DomainError::PositionSizingError(e.to_string()))?;
    }

    if position_size <= rust_decimal::Decimal::ZERO {
        return Err(DomainError::PositionSizingError(
            "Calculated position size must be positive".to_string(),
        ));
    }

    // Recompute the REAL planned risk from the final quantity. Quantization
    // only reduced the quantity, so this must sit at or under the ceiling;
    // if it ever does not, that is a sizing defect and the entry is
    // rejected — the number reported to the risk gate is never clamped.
    let planned_risk = position_size * worst_loss_per_unit;
    if planned_risk > max_risk {
        return Err(DomainError::PositionSizingError(format!(
            "Planned risk {planned_risk} exceeds the per-trade cap {max_risk}"
        )));
    }

    let quantity = Quantity::new(position_size)
        .map_err(|e| DomainError::PositionSizingError(e.to_string()))?;
    Ok(SizedEntry {
        quantity,
        worst_case_loss_per_unit: worst_loss_per_unit,
        planned_risk,
    })
}

/// Calculate notional value of position
///
/// Notional = Quantity × Entry Price
pub fn calculate_notional_value(quantity: &Quantity, entry_price: &Price) -> rust_decimal::Decimal {
    quantity.as_decimal() * entry_price.as_decimal()
}

/// Calculate margin required for position
///
/// Margin = Notional / Leverage = Notional / 1 (integral margin at 1x)
pub fn calculate_margin_required(
    quantity: &Quantity,
    entry_price: &Price,
) -> rust_decimal::Decimal {
    let notional = calculate_notional_value(quantity, entry_price);
    notional / rust_decimal::Decimal::from(RiskConfig::LEVERAGE)
}

// =============================================================================
// Position State Machine
// =============================================================================

/// Position state machine with trailing stop tracking
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PositionState {
    /// Position armed, waiting for detector signal
    Armed,

    /// Entry order submitted, waiting for fill
    Entering {
        /// Order ID for the entry order
        entry_order_id: OrderId,
        /// Expected entry price from signal
        expected_entry: Price,
        /// Signal ID for idempotency (prevents duplicate processing)
        signal_id: Uuid,
    },

    /// Position active, monitoring trailing stop (1x technical stop distance)
    Active {
        /// Current price from WebSocket
        current_price: Price,
        /// Current trailing stop price
        trailing_stop: Price,
        /// Peak price seen so far (for Long) or lowest price (for Short)
        favorable_extreme: Price,
        /// When the favorable extreme was reached
        extreme_at: DateTime<Utc>,
        /// Exchange-assigned order id of the protective insurance stop
        /// (ADR-0039). `None` while no stop is live on the exchange.
        insurance_stop_id: Option<String>,
        /// Entry-time invalidation guard level, if active.
        #[serde(default)]
        invalidation_guard_level: Option<Price>,
        /// Last trailing stop price that was emitted (for idempotency)
        last_emitted_stop: Option<Price>,
    },

    /// Exit order submitted, waiting for fill
    Exiting {
        exit_order_id: OrderId,
        exit_reason: ExitReason,
    },

    /// Position closed, PnL realized
    Closed {
        exit_price: Price,
        realized_pnl: rust_decimal::Decimal,
        exit_reason: ExitReason,
    },

    /// Error state, requires manual intervention
    Error { error: String, recoverable: bool },

    /// Position was cancelled before any entry order was placed.
    /// Results from user disarm or unrecoverable internal rejection.
    /// Terminal: no exchange action was taken, P&L is zero.
    Cancelled,
}

impl PositionState {
    /// Get the name of the state for display
    pub fn name(&self) -> &str {
        match self {
            PositionState::Armed => "armed",
            PositionState::Entering { .. } => "entering",
            PositionState::Active { .. } => "active",
            PositionState::Exiting { .. } => "exiting",
            PositionState::Closed { .. } => "closed",
            PositionState::Error { .. } => "error",
            PositionState::Cancelled => "cancelled",
        }
    }
}

// =============================================================================
// EntryLifecycleStage - computed projection of entry intent lifecycle
// =============================================================================

/// Computed projection of entry intent lifecycle from domain events.
///
/// Derived deterministically from an ordered event sequence; never stored
/// directly. Replay of the same events always produces the same stage.
///
/// Mapping:
/// - `PositionArmed`              → `IntentCreated`
/// - `EntryPolicyResolved`        → `AwaitingSignal`
/// - `EntrySignalReceived`        → `SignalConfirmed`
/// - `EntryApprovalPending`       → `AwaitingApproval`
/// - `EntryOrderRequested`        → `OrderSubmitted`
/// - `EntryFilled`                → `Active`
/// - `PositionDisarmed` / `EntryExecutionRejected` → `Cancelled`
/// - `EntryOrderFailed`           → back to `AwaitingSignal` (retry path)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryLifecycleStage {
    /// ARM request processed; entry intent recorded.
    IntentCreated,
    /// Entry policy resolved; detector is running, waiting for signal.
    AwaitingSignal,
    /// Signal strategy confirmed a signal; risk/approval evaluation next.
    SignalConfirmed,
    /// Signal passed risk; operator confirmation required before order.
    AwaitingApproval,
    /// Entry order placed on exchange; awaiting fill.
    OrderSubmitted,
    /// Entry filled; position is now live.
    Active,
    /// Position was cancelled before any exchange action took place.
    Cancelled,
}

/// Exit reason for position closure
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ExitReason {
    /// Trailing stop was hit (normal exit)
    TrailingStop,
    /// Insurance stop on exchange was triggered (daemon down)
    InsuranceStop,
    /// User manually triggered panic
    UserPanic,
    /// Degraded mode emergency exit
    DegradedMode,
    /// Position error (e.g., margin call)
    PositionError,
    /// User disarmed the position before any entry order was placed
    DisarmedByUser,
    /// Reverse reconciliation closed a Robson `Active` position whose
    /// counterpart had disappeared from the exchange (liquidation, manual
    /// close, externally-resident insurance stop fill, etc.).
    ///
    /// See `docs/policies/UNTRACKED-POSITION-RECONCILIATION.md` (I3) and
    /// `docs/implementation/TD-2026-05-05-001-CORE-LIFECYCLE-DRIFT.md`.
    ReconciledMissingOnExchange,
}

// =============================================================================
// Closure Evidence (TD-2026-05-05-001)
// =============================================================================

/// Source of a real exchange fill that closed a position.
///
/// Discriminator inside [`RealFillEvidence`]; identifies which lifecycle
/// path produced the fill. Default is [`Self::ExitFill`] — the canonical
/// `Active → Exiting → Closed` path.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExitOrderFillSource {
    /// Exit order placed via the daemon's normal exit path.
    #[default]
    ExitFill,
    /// Insurance stop order resident on the exchange was filled.
    InsuranceStopFill,
}

/// Evidence captured when a position was closed by a real exchange fill.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct RealFillEvidence {
    /// Lifecycle path that produced the fill.
    pub source: ExitOrderFillSource,
    /// Exchange-assigned order id, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exchange_order_id: Option<String>,
}

/// Specific exchange order confirmed filled via `GET /fapi/v1/order`
/// (highest-fidelity reconciliation evidence).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OrderFillEvidence {
    /// Exchange-assigned order id (the order whose fill closed the position).
    pub exchange_order_id: String,
    /// Fill price reported by the exchange.
    pub fill_price: Price,
    /// Filled quantity reported by the exchange.
    pub filled_quantity: Quantity,
    /// Trading fee paid.
    pub fee: rust_decimal::Decimal,
    /// Fee asset (e.g. "USDT", "BNB").
    pub fee_asset: String,
    /// When the fill occurred (exchange-reported).
    pub filled_at: DateTime<Utc>,
}

/// User trade history record covering the gap between last-known-active
/// and the missing observation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UserTradeEvidence {
    /// Exchange-assigned order id of the originating order.
    pub exchange_order_id: String,
    /// Exchange-assigned trade id.
    pub exchange_trade_id: String,
    /// Fill price reported by the exchange.
    pub fill_price: Price,
    /// Filled quantity reported by the exchange.
    pub filled_quantity: Quantity,
    /// Trading fee paid.
    pub fee: rust_decimal::Decimal,
    /// Fee asset.
    pub fee_asset: String,
    /// When the trade occurred (exchange-reported).
    pub filled_at: DateTime<Utc>,
}

/// Account-level evidence proving the position is zero on the exchange.
///
/// Two consecutive `get_all_open_positions()` snapshots are required;
/// the close is finalized only after the second confirms absence.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AccountSnapshotEvidence {
    /// First observation that the position was missing on the exchange.
    pub first_observed_missing_at: DateTime<Utc>,
    /// Second consecutive observation (after the grace period) that
    /// confirmed the position is gone.
    pub confirmed_missing_at: DateTime<Utc>,
    /// Optional change in futures wallet balance between the two
    /// observations, when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub futures_balance_delta: Option<rust_decimal::Decimal>,
}

/// Basis used to estimate the exit price when no fill record exists.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EstimationBasis {
    /// Use the trailing stop price recorded in `PositionState::Active`
    /// at the moment the drift was detected.
    TrailingStopAtDetection,
    /// Use the exchange-reported mark price at detection time.
    ExchangeMarkPrice,
    /// Use the last price observed by the daemon prior to detection.
    LastObservedPrice,
}

/// Estimated terminal-price evidence — last-resort source.
///
/// Realized PnL derived from this evidence MUST be flagged as
/// estimated (not real) by downstream consumers. The daemon emits a
/// `CRITICAL` alert and increments
/// `robson_reconciliation_estimated_closes_total` on every close that
/// reaches this branch.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EstimatedEvidence {
    /// How the exit price was estimated.
    pub estimation_basis: EstimationBasis,
    /// Estimated exit price.
    pub exit_price: Price,
    /// Optional identity of the evaluator that produced the estimate
    /// (e.g. `"operator:ldamasio"`, `"auto:reconcile_close"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evaluator: Option<String>,
    /// When the drift was detected and the estimate was sealed.
    pub detected_at: DateTime<Utc>,
}

/// Evidence sources for a reverse-reconciliation close, in priority order.
///
/// See `docs/implementation/TD-2026-05-05-001-CORE-LIFECYCLE-DRIFT.md`
/// (Amendment §2 — Evidence ordering, no silent fallback).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "source", content = "data", rename_all = "snake_case")]
pub enum ReconciliationEvidence {
    /// Specific exchange order (typically the `insurance_stop_id`)
    /// confirmed filled. Highest fidelity.
    OrderFillRecord(OrderFillEvidence),
    /// Per-symbol user trade record covering the drift window.
    UserTradeRecord(UserTradeEvidence),
    /// Two consecutive empty snapshots prove the position is gone.
    /// No fill data available; exit price must be carried separately.
    AccountSnapshot(AccountSnapshotEvidence),
    /// Last resort: estimated terminal price. Operator-driven or
    /// explicitly opted-in via runbook.
    Estimated(EstimatedEvidence),
}

/// Provenance of a `PositionClosed` event.
///
/// Default is [`Self::RealFill`] with [`ExitOrderFillSource::ExitFill`] —
/// preserves the historical contract for events written before
/// TD-2026-05-05-001 landed (legacy events deserialize to this default
/// via `#[serde(default)]` on `Event::PositionClosed::closure_evidence`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", content = "evidence", rename_all = "snake_case")]
pub enum ClosureEvidence {
    /// Position closed via a real exchange fill (canonical path).
    RealFill(RealFillEvidence),
    /// Position closed via reverse reconciliation against the exchange.
    Reconciled(ReconciliationEvidence),
}

impl Default for ClosureEvidence {
    fn default() -> Self {
        ClosureEvidence::RealFill(RealFillEvidence::default())
    }
}

impl ClosureEvidence {
    /// Convenience constructor for the canonical exit-fill path.
    pub fn real_exit_fill(exchange_order_id: Option<String>) -> Self {
        ClosureEvidence::RealFill(RealFillEvidence {
            source: ExitOrderFillSource::ExitFill,
            exchange_order_id,
        })
    }

    /// `true` if the close came from a real fill (any source).
    pub fn is_real_fill(&self) -> bool {
        matches!(self, ClosureEvidence::RealFill(_))
    }

    /// `true` if the close came from reverse reconciliation.
    pub fn is_reconciled(&self) -> bool {
        matches!(self, ClosureEvidence::Reconciled(_))
    }
}

// =============================================================================
// Order
// =============================================================================

/// Order represents an instruction to buy/sell on the exchange
///
/// NOTE: Trade entity was removed - fill info is consolidated here
/// since futures market orders execute in single fill.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Order {
    pub id: OrderId,
    pub position_id: PositionId,
    pub exchange_order_id: Option<String>,
    pub client_order_id: String, // intent_id (UUID v7)

    pub symbol: Symbol,
    pub side: OrderSide,
    pub order_type: OrderType,
    pub quantity: Quantity,
    pub price: Option<Price>, // None for market orders

    pub status: OrderStatus,

    // Fill information (when status == Filled)
    pub filled_quantity: Option<Quantity>,
    pub fill_price: Option<Price>,
    pub filled_at: Option<DateTime<Utc>>,
    pub fee_paid: Option<rust_decimal::Decimal>,

    pub created_at: DateTime<Utc>,
}

impl Order {
    /// Create a new market order
    pub fn new_market(
        position_id: PositionId,
        symbol: Symbol,
        side: OrderSide,
        quantity: Quantity,
    ) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::now_v7(),
            position_id,
            exchange_order_id: None,
            client_order_id: Uuid::now_v7().to_string(),
            symbol,
            side,
            order_type: OrderType::Market,
            quantity,
            price: None,
            status: OrderStatus::Pending,
            filled_quantity: None,
            fill_price: None,
            filled_at: None,
            fee_paid: None,
            created_at: now,
        }
    }

    /// Create a new stop-loss limit order (for insurance stop)
    pub fn new_stop_loss_limit(
        position_id: PositionId,
        symbol: Symbol,
        side: OrderSide,
        quantity: Quantity,
        _stop_price: Price, // Stop price (stored on exchange, not locally)
        limit_price: Price,
    ) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::now_v7(),
            position_id,
            exchange_order_id: None,
            client_order_id: Uuid::now_v7().to_string(),
            symbol,
            side,
            order_type: OrderType::StopLossLimit,
            quantity,
            price: Some(limit_price), // Limit price
            status: OrderStatus::Pending,
            filled_quantity: None,
            fill_price: None,
            filled_at: None,
            fee_paid: None,
            created_at: now,
        }
    }

    /// Mark order as filled
    pub fn mark_filled(
        &mut self,
        exchange_order_id: String,
        fill_price: Price,
        filled_quantity: Quantity,
        fee: rust_decimal::Decimal,
    ) -> Result<(), DomainError> {
        if self.status != OrderStatus::Pending && self.status != OrderStatus::Submitted {
            return Err(DomainError::InvalidTechnicalStopDistance(
                "Cannot mark order as filled: invalid state".to_string(),
            ));
        }

        self.exchange_order_id = Some(exchange_order_id);
        self.fill_price = Some(fill_price);
        self.filled_quantity = Some(filled_quantity);
        self.fee_paid = Some(fee);
        self.status = OrderStatus::Filled;
        self.filled_at = Some(Utc::now());

        Ok(())
    }

    /// Check if order is filled
    pub fn is_filled(&self) -> bool {
        matches!(self.status, OrderStatus::Filled)
    }
}

/// Order types supported
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum OrderType {
    /// Market order (immediate execution)
    Market,
    /// Limit order (price guaranteed)
    Limit,
    /// Stop-loss limit (insurance stop on exchange)
    StopLossLimit,
}

/// Order status lifecycle
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum OrderStatus {
    /// Created locally, not sent yet
    Pending,
    /// Submitted to exchange
    Submitted,
    /// Partially filled (rare in futures)
    PartialFill,
    /// Completely filled
    Filled,
    /// Cancelled
    Cancelled,
    /// Rejected by exchange
    Rejected,
    /// Expired
    Expired,
}

// =============================================================================
// Detector Signal
// =============================================================================

/// Method used to derive a detector technical stop.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TechnicalStopMethodSnapshot {
    /// Stop derived from the Nth clustered swing level below/above entry.
    SwingPoint {
        /// 1-indexed support/resistance level selected by the analyzer.
        level_n: usize,
    },
    /// Stop derived from ATR fallback because swing levels were insufficient.
    AtrFallback,
}

/// Confidence assigned to a technical stop analysis result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TechnicalStopConfidenceSnapshot {
    /// Primary path succeeded with the configured support/resistance level.
    High,
    /// Analyzer found fewer levels than requested and degraded gracefully.
    Medium,
    /// Analyzer had to fall back to ATR.
    Low,
}

/// Stop level used as the basis for effective executable stop pricing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectiveStopBasis {
    /// The analyzer's chart-derived technical stop was used directly.
    TechnicalStop,
    /// The invalidation guard clamped the effective stop basis.
    InvalidationGuard,
}

// =============================================================================
// Stop-Aware Entry Types (ADR-0035)
// =============================================================================

/// Classification of anchor types for a technical stop.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AnchorType {
    /// Support level (for LONG entries)
    Support,
    /// Resistance level (for SHORT entries)
    Resistance,
    /// Swing low point
    SwingLow,
    /// Swing high point
    SwingHigh,
    /// Breakout retest level
    BreakoutRetest,
    /// Liquidity sweep level
    LiquidityLevel,
}

/// Explicit metadata about the technical stop anchor event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StopAnchor {
    /// Type of anchor (support, resistance, swing point, etc.)
    pub anchor_type: AnchorType,
    /// Price level of the anchor
    pub anchor_price: Price,
    /// Timeframe of the anchor — normalized string (e.g. "15m").
    /// Will be promoted to a proper domain type in a future slice.
    pub timeframe: String,
    /// Reference to the technical event (optional, future)
    pub source_event_id: Option<Uuid>,
    /// Reason for anchor invalidation (if applicable)
    pub invalidation_reason: Option<String>,
}

/// Stop quality class with associated boost percentage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StopQuality {
    /// No boost (0%) — valid anchor, no structural advantage
    None,
    /// Weak boost (+5%) — distant/old anchor, low confluence
    Weak,
    /// Good boost (+10%) — recent anchor, moderate distance, 1+ confirmation
    Good,
    /// Premium boost (+15%) — recent + clean, efficient distance, multiple
    /// confluences
    Premium,
    /// Exceptional boost (+20%) — rare, feature-flagged, shadow-mode only
    Exceptional,
}

/// Stop quality classification result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StopQualityClassification {
    /// Quality class (None through Exceptional)
    pub class: StopQuality,
    /// Raw score before thresholds
    pub raw_score: i32,
    /// Boost percentage (0.0 to 0.20)
    pub boost_pct: Decimal,
    /// Whether this would be Exceptional if flag enabled
    pub shadow_exceptional: bool,
    /// Human-readable reasons for classification
    pub reasons: Vec<String>,
}

/// Immutable snapshot of the analyzer configuration used for a detector signal.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TechnicalStopConfigSnapshot {
    /// Minimum candle count required before analysis runs.
    pub min_candles: usize,
    /// Swing-point lookback on each side of a candidate candle.
    pub swing_lookback: usize,
    /// 1-indexed support/resistance level requested by policy.
    pub support_level_n: usize,
    /// Tolerance used to cluster nearby levels, as a fraction of entry.
    pub level_tolerance: Decimal,
    /// ATR period used by the fallback path.
    pub atr_period: usize,
    /// ATR multiplier used by the fallback path.
    pub atr_multiplier: Decimal,
    /// Minimum allowed stop distance as a fraction of entry.
    pub min_stop_distance_pct: Decimal,
    /// Maximum allowed stop distance as a fraction of entry.
    pub max_stop_distance_pct: Decimal,
}

/// Audit payload describing how the detector derived a technical stop.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TechnicalStopAnalysisAudit {
    /// Absolute stop price selected by the analyzer.
    pub stop_price: Price,
    /// Raw analyzer stop before any invalidation guard clamp.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_technical_stop: Option<Price>,
    /// Optional invalidation guard level sampled at signal time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invalidation_guard_level: Option<Price>,
    /// Which level forms the effective stop basis before buffering.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective_stop_basis: Option<EffectiveStopBasis>,
    /// Method used to derive the stop.
    pub method: TechnicalStopMethodSnapshot,
    /// Confidence assigned to the result.
    pub confidence: TechnicalStopConfidenceSnapshot,
    /// Swing levels detected on the chart, ordered by distance from entry.
    pub detected_levels: Vec<Price>,
    /// Analyzer configuration snapshot used to produce this result.
    pub config: TechnicalStopConfigSnapshot,
    /// Explicit metadata about the stop anchor event (ADR-0035).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_anchor: Option<Box<StopAnchor>>,
    /// Stop quality classification in shadow mode (ADR-0035).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_quality: Option<Box<StopQualityClassification>>,
    /// The `support_level_n` the selection anchored at (ADR-0050 §1).
    /// `None` on pre-ADR-0050 events.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub configured_level_n: Option<usize>,
    /// The 1-indexed level actually selected; `None` for the ATR fallback
    /// and on pre-ADR-0050 events.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_level_n: Option<usize>,
    /// Levels considered and skipped during the anchor-N walk (ADR-0050 §1).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skipped_levels: Vec<SkippedLevelSnapshot>,
    /// Selection rule identifier; `"anchor_n_walk_deeper"` after ADR-0050.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selection_rule: Option<String>,
}

/// A chart level considered and skipped by valid-level selection
/// (ADR-0050 §1 audit).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkippedLevelSnapshot {
    /// The skipped chart level.
    pub level: Price,
    /// Its distance from entry as a fraction (0.001 = 0.1%).
    pub distance_fraction: rust_decimal::Decimal,
    /// Why it was skipped (`below_min` | `above_max`).
    pub reason: String,
}

/// Signal from detector to trigger entry
///
/// Emitted by a DetectorTask when entry conditions are met.
/// Each detector emits at most ONE signal per position (single-shot).
///
/// # Idempotency
///
/// The `signal_id` ensures idempotent processing:
/// - Engine checks if signal was already processed before transitioning
/// - Duplicate signals with same `signal_id` are safely ignored
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectorSignal {
    /// Unique signal identifier for idempotency
    pub signal_id: Uuid,
    /// Position this signal belongs to (detector is per-position)
    pub position_id: PositionId,
    /// Trading pair symbol
    pub symbol: Symbol,
    /// Position direction (must match armed position)
    pub side: Side,
    /// Suggested entry price (current market price when signal fired)
    pub entry_price: Price,
    /// Technical stop loss from chart analysis
    pub stop_loss: Price,
    /// Optional audit payload describing how the technical stop was derived.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub technical_stop_analysis: Option<TechnicalStopAnalysisAudit>,
    /// When the signal was generated
    pub timestamp: DateTime<Utc>,
}

impl DetectorSignal {
    /// Create a new detector signal
    pub fn new(
        position_id: PositionId,
        symbol: Symbol,
        side: Side,
        entry_price: Price,
        stop_loss: Price,
    ) -> Self {
        Self {
            signal_id: Uuid::now_v7(),
            position_id,
            symbol,
            side,
            entry_price,
            stop_loss,
            technical_stop_analysis: None,
            timestamp: Utc::now(),
        }
    }

    /// Attach technical stop audit metadata to this signal.
    pub fn with_technical_stop_analysis(mut self, analysis: TechnicalStopAnalysisAudit) -> Self {
        self.technical_stop_analysis = Some(analysis);
        self
    }

    /// Calculate technical stop distance from signal
    pub fn tech_stop_distance(&self) -> TechnicalStopDistance {
        TechnicalStopDistance::from_entry_and_stop(self.entry_price, self.stop_loss)
    }

    /// Validate the signal matches the position
    pub fn validate_for_position(&self, position: &Position) -> Result<(), DomainError> {
        if self.position_id != position.id {
            return Err(DomainError::InvalidSignal(format!(
                "Signal position_id {} does not match position {}",
                self.position_id, position.id
            )));
        }

        if self.symbol != position.symbol {
            return Err(DomainError::InvalidSignal(format!(
                "Signal symbol {} does not match position symbol {}",
                self.symbol, position.symbol
            )));
        }

        if self.side != position.side {
            return Err(DomainError::InvalidSignal(format!(
                "Signal side {:?} does not match position side {:?}",
                self.side, position.side
            )));
        }

        Ok(())
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use rust_decimal_macros::dec;

    use super::*;

    #[test]
    fn test_position_creation() {
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let position = Position::new(Uuid::now_v7(), symbol, Side::Long);

        assert_eq!(position.state.name(), "armed");
        assert!(position.can_enter());
        assert!(!position.can_exit());
        assert!(!position.is_closed());
    }

    #[test]
    fn test_order_market_creation() {
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let order = Order::new_market(
            Uuid::now_v7(),
            symbol,
            OrderSide::Buy,
            Quantity::new(dec!(0.1)).unwrap(),
        );

        assert_eq!(order.order_type, OrderType::Market);
        assert_eq!(order.status, OrderStatus::Pending);
        assert!(order.price.is_none());
    }

    #[test]
    fn test_order_stop_loss_limit_creation() {
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let order = Order::new_stop_loss_limit(
            Uuid::now_v7(),
            symbol,
            OrderSide::Sell,
            Quantity::new(dec!(0.1)).unwrap(),
            Price::new(dec!(93500.0)).unwrap(),
            Price::new(dec!(93400.0)).unwrap(),
        );

        assert_eq!(order.order_type, OrderType::StopLossLimit);
        assert_eq!(order.status, OrderStatus::Pending);
        assert_eq!(order.price.unwrap().as_decimal(), dec!(93400.0));
    }

    #[test]
    fn test_order_mark_filled() {
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let mut order = Order::new_market(
            Uuid::now_v7(),
            symbol,
            OrderSide::Buy,
            Quantity::new(dec!(0.1)).unwrap(),
        );

        let result = order.mark_filled(
            "123456".to_string(),
            Price::new(dec!(95000.0)).unwrap(),
            Quantity::new(dec!(0.1)).unwrap(),
            dec!(0.001),
        );

        assert!(result.is_ok());
        assert!(order.is_filled());
        assert_eq!(order.fill_price.unwrap().as_decimal(), dec!(95000.0));
    }

    // Position Sizing tests (Golden Rule, plan-based per issue #154)

    use crate::{
        executable_stop::{build_executable_stop_plan, StopPlanInputs},
        stop_policy::StopPolicy,
        trading_rules::SymbolTradingRules,
    };

    /// Legacy-policy plan (the derivation production runs today).
    fn legacy_plan(
        side: Side,
        technical_stop: Decimal,
        stop_buffer_bps: Decimal,
        guard: Option<Decimal>,
    ) -> crate::executable_stop::ExecutableStopPlan {
        build_executable_stop_plan(StopPlanInputs {
            policy: StopPolicy::LegacyUncapped,
            side,
            technical_stop: Price::new(technical_stop).unwrap(),
            guard: guard.map(|g| Price::new(g).unwrap()),
            entry_reference: None,
            technical_span: None,
            stop_buffer_bps,
            rules: None,
        })
        .unwrap()
    }

    /// Expected per-unit worst loss under the normative adverse-fill formula
    /// (gap 10 bps, taker fee 0.05% per side, the RiskConfig defaults).
    fn expected_per_unit(entry: Decimal, trigger: Decimal, side: Side) -> Decimal {
        let gap = trigger * dec!(10) / dec!(10000);
        let adverse = match side {
            Side::Long => trigger - gap,
            Side::Short => trigger + gap,
        };
        let distance = match side {
            Side::Long => entry - adverse,
            Side::Short => adverse - entry,
        };
        distance + dec!(0.0005) * (entry + adverse)
    }

    fn btcusdt_rules() -> SymbolTradingRules {
        SymbolTradingRules::new(
            Symbol::from_pair("BTCUSDT").unwrap(),
            dec!(0.10),
            dec!(556.80),
            dec!(0.001),
            dec!(0.001),
            dec!(1000),
            dec!(100),
            2,
            3,
        )
        .unwrap()
    }

    #[test]
    fn test_size_entry_basic() {
        // Setup: $10,000 capital, 1% cap; entry $95,000, stop $93,500.
        let config = RiskConfig::new(dec!(10000)).unwrap();
        let entry = Price::new(dec!(95000)).unwrap();
        let stop = Price::new(dec!(93500)).unwrap();
        let tech_stop = TechnicalStopDistance::from_entry_and_stop(entry, stop);
        let plan = legacy_plan(Side::Long, dec!(93500), Decimal::ZERO, None);

        let sized = size_entry(&config, &entry, &tech_stop, &plan, None, None).unwrap();

        let per_unit = expected_per_unit(dec!(95000), dec!(93500), Side::Long);
        assert_eq!(sized.worst_case_loss_per_unit, per_unit);
        let expected_qty = (dec!(100) / per_unit)
            .round_dp_with_strategy(12, rust_decimal::RoundingStrategy::ToZero);
        assert_eq!(sized.quantity.as_decimal(), expected_qty);
        assert_eq!(sized.planned_risk, expected_qty * per_unit);
        assert!(sized.planned_risk <= config.max_risk_amount());
    }

    #[test]
    fn test_size_entry_wider_stop_smaller_position() {
        let config = RiskConfig::new(dec!(10000)).unwrap();
        let entry = Price::new(dec!(95000)).unwrap();

        let narrow_stop = Price::new(dec!(93500)).unwrap();
        let narrow = size_entry(
            &config,
            &entry,
            &TechnicalStopDistance::from_entry_and_stop(entry, narrow_stop),
            &legacy_plan(Side::Long, dec!(93500), Decimal::ZERO, None),
            None,
            None,
        )
        .unwrap();

        let wide_stop = Price::new(dec!(92000)).unwrap();
        let wide = size_entry(
            &config,
            &entry,
            &TechnicalStopDistance::from_entry_and_stop(entry, wide_stop),
            &legacy_plan(Side::Long, dec!(92000), Decimal::ZERO, None),
            None,
            None,
        )
        .unwrap();

        assert!(wide.quantity.as_decimal() < narrow.quantity.as_decimal());
    }

    #[test]
    fn test_size_entry_tighter_stop_is_margin_capped() {
        // Tighter stop would allow a larger position, but 1x margin caps it.
        let config = RiskConfig::new(dec!(10000)).unwrap();
        let entry = Price::new(dec!(95000)).unwrap();
        let stop = Price::new(dec!(94500)).unwrap();
        let tech_stop = TechnicalStopDistance::from_entry_and_stop(entry, stop);
        let plan = legacy_plan(Side::Long, dec!(94500), Decimal::ZERO, None);

        let sized = size_entry(&config, &entry, &tech_stop, &plan, None, None).unwrap();

        let expected = (config.capital() * dec!(0.99) / entry.as_decimal())
            .round_dp_with_strategy(8, rust_decimal::RoundingStrategy::ToZero);
        assert_eq!(sized.quantity.as_decimal(), expected);
        assert!(sized.planned_risk < config.max_risk_amount());
    }

    #[test]
    fn test_size_entry_higher_capital() {
        let config = RiskConfig::new(dec!(50000)).unwrap();
        let entry = Price::new(dec!(95000)).unwrap();
        let stop = Price::new(dec!(93500)).unwrap();
        let tech_stop = TechnicalStopDistance::from_entry_and_stop(entry, stop);
        let plan = legacy_plan(Side::Long, dec!(93500), Decimal::ZERO, None);

        let sized = size_entry(&config, &entry, &tech_stop, &plan, None, None).unwrap();

        let per_unit = expected_per_unit(dec!(95000), dec!(93500), Side::Long);
        let expected_qty = (dec!(500) / per_unit)
            .round_dp_with_strategy(12, rust_decimal::RoundingStrategy::ToZero);
        assert_eq!(sized.quantity.as_decimal(), expected_qty);
    }

    #[test]
    fn test_position_sizing_risk_stays_at_or_under_cap() {
        // Regardless of stop distance, the planned worst-case loss lands on
        // the 1% ceiling (within truncation) and NEVER above it.
        let config = RiskConfig::new(dec!(10000)).unwrap(); // $100 cap

        for (entry, stop) in [(dec!(95000), dec!(92000)), (dec!(95000), dec!(94000))] {
            let entry = Price::new(entry).unwrap();
            let stop_price = Price::new(stop).unwrap();
            let tech_stop = TechnicalStopDistance::from_entry_and_stop(entry, stop_price);
            let plan = legacy_plan(Side::Long, stop, Decimal::ZERO, None);
            let sized = size_entry(&config, &entry, &tech_stop, &plan, None, None).unwrap();

            assert!(sized.planned_risk <= dec!(100), "planned {} > cap", sized.planned_risk);
            assert!(
                sized.planned_risk > dec!(99.99),
                "planned {} too far under",
                sized.planned_risk
            );
            // The chart-distance loss alone stays strictly inside the cap.
            let chart_loss = sized.quantity.as_decimal() * tech_stop.distance;
            assert!(chart_loss < config.max_risk_amount());
        }
    }

    #[test]
    fn test_size_entry_prices_stop_buffer_into_budget() {
        // A non-zero buffer widens the executable distance, so the same
        // budget buys a smaller position.
        let base_cfg = RiskConfig::new(dec!(10000)).unwrap();
        let entry = Price::new(dec!(95000)).unwrap();
        let stop = Price::new(dec!(93500)).unwrap();
        let tech_stop = TechnicalStopDistance::from_entry_and_stop(entry, stop);

        let base_plan = legacy_plan(Side::Long, dec!(93500), Decimal::ZERO, None);
        let buffered_plan = legacy_plan(Side::Long, dec!(93500), dec!(20), None);
        // 20 bps of 93500 = 187.00 below the technical stop.
        assert_eq!(buffered_plan.trigger.as_decimal(), dec!(93500) - dec!(187.00));

        let base = size_entry(&base_cfg, &entry, &tech_stop, &base_plan, None, None).unwrap();
        let buffered =
            size_entry(&base_cfg, &entry, &tech_stop, &buffered_plan, None, None).unwrap();

        assert!(buffered.quantity.as_decimal() < base.quantity.as_decimal());
        assert!(buffered.planned_risk <= base_cfg.max_risk_amount());
        assert!(buffered.planned_risk > dec!(99.99));
    }

    #[test]
    fn test_size_entry_uses_binding_guard_distance() {
        let config = RiskConfig::new(dec!(10000)).unwrap();
        let entry = Price::new(dec!(95000)).unwrap();
        let technical = Price::new(dec!(96500)).unwrap();
        let tech_stop = TechnicalStopDistance::from_entry_and_stop(entry, technical);

        let technical_plan = legacy_plan(Side::Short, dec!(96500), dec!(20), None);
        let guarded_plan = legacy_plan(Side::Short, dec!(96500), dec!(20), Some(dec!(98000)));
        assert!(guarded_plan.guard_bound);

        let sized_technical =
            size_entry(&config, &entry, &tech_stop, &technical_plan, None, None).unwrap();
        let sized_guarded =
            size_entry(&config, &entry, &tech_stop, &guarded_plan, None, None).unwrap();
        assert!(sized_guarded.quantity.as_decimal() < sized_technical.quantity.as_decimal());
        assert!(sized_guarded.planned_risk <= config.max_risk_amount());
    }

    #[test]
    fn test_size_entry_rejects_over_wide_effective_guard_distance() {
        let config = RiskConfig::new(dec!(10000)).unwrap();
        let entry = Price::new(dec!(100)).unwrap();
        let technical = Price::new(dec!(105)).unwrap();
        let tech_stop = TechnicalStopDistance::from_entry_and_stop(entry, technical);
        let plan = legacy_plan(Side::Short, dec!(105), Decimal::ZERO, Some(dec!(111)));

        let result = size_entry(&config, &entry, &tech_stop, &plan, None, None);

        assert!(matches!(result, Err(DomainError::InvalidTechnicalStopDistance(_))));
    }

    #[test]
    fn test_size_entry_rejects_executable_stop_out_of_bounds() {
        // The RAW level is inside the 10% max, but a 100 bps buffer pushes
        // the FINAL executable trigger past it: typed stage-3 rejection.
        let config = RiskConfig::new(dec!(10000)).unwrap();
        let entry = Price::new(dec!(100000)).unwrap();
        let stop = Price::new(dec!(90200)).unwrap(); // 9.8%
        let tech_stop = TechnicalStopDistance::from_entry_and_stop(entry, stop);
        let plan = legacy_plan(Side::Long, dec!(90200), dec!(100), None);

        let result = size_entry(&config, &entry, &tech_stop, &plan, None, None);

        assert!(
            matches!(result, Err(DomainError::ExecutableStopOutOfBounds(_))),
            "got {result:?}"
        );
    }

    #[test]
    fn test_size_entry_caps_by_margin() {
        let config = RiskConfig::new(dec!(351.92170492)).unwrap();
        let entry = Price::new(dec!(59623.10)).unwrap();
        let stop = Price::new(dec!(59295.60)).unwrap();
        let tech_stop = TechnicalStopDistance::from_entry_and_stop(entry, stop);
        let plan = legacy_plan(Side::Long, dec!(59295.60), Decimal::ZERO, None);

        let sized = size_entry(&config, &entry, &tech_stop, &plan, None, None).unwrap();
        let qty = sized.quantity.as_decimal();
        let margin_cap = (config.capital() * dec!(0.99) / entry.as_decimal())
            .round_dp_with_strategy(8, rust_decimal::RoundingStrategy::ToZero);

        assert_eq!(qty, margin_cap);
        assert!(sized.planned_risk < config.max_risk_amount());
    }

    #[test]
    fn test_margin_capped_size_survives_gate_round_trip() {
        // Regression: 2026-07-04 prod entry denial. The margin cap must
        // reserve the 100 bps headroom so the gate's qty x entry round-trip
        // stays below capital.
        let config = RiskConfig::new(dec!(1643.18373001)).unwrap();
        let entry = Price::new(dec!(62496.40)).unwrap();
        let stop = Price::new(dec!(62898.833333333333333333333333)).unwrap();
        let tech_stop = TechnicalStopDistance::from_entry_and_stop(entry, stop);
        let plan =
            legacy_plan(Side::Short, dec!(62898.833333333333333333333333), Decimal::ZERO, None);

        let sized = size_entry(&config, &entry, &tech_stop, &plan, None, None).unwrap();
        let qty = sized.quantity.as_decimal();

        assert!(qty * entry.as_decimal() > config.capital() * dec!(0.989));
        assert!(
            qty * entry.as_decimal() <= config.capital() * dec!(0.99),
            "margin round-trip {} exceeds headroom-adjusted capital {}",
            qty * entry.as_decimal(),
            config.capital() * dec!(0.99)
        );
    }

    #[test]
    fn test_margin_headroom_is_configurable_and_validated() {
        let base = RiskConfig::new(dec!(1000)).unwrap();
        let entry = Price::new(dec!(50000)).unwrap();
        let stop = Price::new(dec!(49900)).unwrap(); // tight -> margin cap binds
        let tech_stop = TechnicalStopDistance::from_entry_and_stop(entry, stop);
        let plan = legacy_plan(Side::Long, dec!(49900), Decimal::ZERO, None);

        let with_zero = base.with_margin_headroom(Decimal::ZERO).unwrap();
        let qty_zero =
            size_entry(&with_zero, &entry, &tech_stop, &plan, None, None).unwrap().quantity;
        let qty_default =
            size_entry(&base, &entry, &tech_stop, &plan, None, None).unwrap().quantity;
        assert!(qty_default.as_decimal() < qty_zero.as_decimal());
        assert_eq!(qty_zero.as_decimal(), dec!(0.02));
        assert_eq!(qty_default.as_decimal(), dec!(0.0198));

        assert!(base.with_margin_headroom(dec!(-1)).is_err());
        assert!(base.with_margin_headroom(dec!(1001)).is_err());
    }

    #[test]
    fn test_margin_cap_bounded_by_live_available_balance() {
        // Regression: 2026-07-04 prod entry rejection (third act). The
        // physical margin cap must bind to the LIVE balance when known.
        let config = RiskConfig::new(dec!(1643.18373001)).unwrap();
        let entry = Price::new(dec!(62440.30)).unwrap();
        let stop = Price::new(dec!(62811.05)).unwrap(); // tight -> margin cap binds
        let tech_stop = TechnicalStopDistance::from_entry_and_stop(entry, stop);
        let plan = legacy_plan(Side::Short, dec!(62811.05), Decimal::ZERO, None);
        let available = dec!(1617.67600541);

        let sized = size_entry(&config, &entry, &tech_stop, &plan, Some(available), None).unwrap();
        let notional = sized.quantity.as_decimal() * entry.as_decimal();
        assert!(notional <= available * dec!(0.99), "notional {} exceeds wallet cap", notional);
        assert!(
            notional > available * dec!(0.989),
            "margin cap must engage, notional {}",
            notional
        );

        // An available balance ABOVE policy capital must not size past policy.
        let sized_rich =
            size_entry(&config, &entry, &tech_stop, &plan, Some(dec!(999999)), None).unwrap();
        let sized_none = size_entry(&config, &entry, &tech_stop, &plan, None, None).unwrap();
        assert_eq!(sized_rich.quantity, sized_none.quantity);

        // A non-positive available balance is a hard sizing error.
        assert!(size_entry(&config, &entry, &tech_stop, &plan, Some(Decimal::ZERO), None).is_err());
    }

    #[test]
    fn test_size_entry_quantizes_down_to_lot_step_and_recomputes_risk() {
        let rules = btcusdt_rules();
        let config = RiskConfig::new(dec!(100000)).unwrap();
        let entry = Price::new(dec!(95000)).unwrap();
        let stop = Price::new(dec!(93500)).unwrap();
        let tech_stop = TechnicalStopDistance::from_entry_and_stop(entry, stop);
        let plan = legacy_plan(Side::Long, dec!(93500), Decimal::ZERO, None);

        let unquantized = size_entry(&config, &entry, &tech_stop, &plan, None, None).unwrap();
        let quantized = size_entry(&config, &entry, &tech_stop, &plan, None, Some(&rules)).unwrap();

        // The quantity is on the 0.001 lot grid and never rounded up.
        let steps = quantized.quantity.as_decimal() / rules.step_size();
        assert_eq!(steps, steps.trunc(), "qty must sit on the lot grid");
        assert!(quantized.quantity.as_decimal() <= unquantized.quantity.as_decimal());
        // Planned risk is recomputed from the FINAL quantity, not the
        // pre-quantization one.
        assert_eq!(
            quantized.planned_risk,
            quantized.quantity.as_decimal() * quantized.worst_case_loss_per_unit
        );
        assert!(quantized.planned_risk <= config.max_risk_amount());
    }

    #[test]
    fn test_size_entry_rejects_below_min_qty_never_rounds_up() {
        let rules = btcusdt_rules();
        // Tiny capital: risk-sized qty quantizes to zero lots.
        let config = RiskConfig::new(dec!(50)).unwrap();
        let entry = Price::new(dec!(95000)).unwrap();
        let stop = Price::new(dec!(93500)).unwrap();
        let tech_stop = TechnicalStopDistance::from_entry_and_stop(entry, stop);
        let plan = legacy_plan(Side::Long, dec!(93500), Decimal::ZERO, None);

        let result = size_entry(&config, &entry, &tech_stop, &plan, None, Some(&rules));
        match result {
            Err(DomainError::PositionSizingError(message)) => {
                assert!(message.contains("will not round up"), "message: {message}");
            },
            other => panic!("expected PositionSizingError, got {other:?}"),
        }
    }

    #[test]
    fn test_size_entry_short_costing_covers_modeled_loss() {
        // Short regression (#153 review): the exit fee must be charged on
        // the adverse fill, so planned risk >= the modeled realized loss.
        let config = RiskConfig::new(dec!(10000)).unwrap();
        let entry = Price::new(dec!(62000)).unwrap();
        let stop = Price::new(dec!(62873.90)).unwrap();
        let tech_stop = TechnicalStopDistance::from_entry_and_stop(entry, stop);
        let plan = legacy_plan(Side::Short, dec!(62873.90), dec!(10), None);

        let sized = size_entry(&config, &entry, &tech_stop, &plan, None, None).unwrap();

        // Model the realized loss at the adverse fill with both fees.
        let adverse = plan.adverse_fill_bound(config.stop_gap_bps()).unwrap().as_decimal();
        let modeled = sized.quantity.as_decimal()
            * ((adverse - entry.as_decimal())
                + config.taker_fee_rate() * (entry.as_decimal() + adverse));
        assert!(
            sized.planned_risk >= modeled,
            "planned {} < modeled {}",
            sized.planned_risk,
            modeled
        );
        assert!(sized.planned_risk <= config.max_risk_amount());
    }
}
