//! Event payload types for projection handlers
//!
//! Only the events we actually implement are defined here.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// =============================================================================
// QUERY AUDIT EVENTS
// =============================================================================

/// QUERY_STATE_CHANGED payload
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryStateChanged {
    pub query_id: Uuid,
    pub position_id: Option<Uuid>,
    pub state: String,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub transition_cause: String,
    pub snapshot: serde_json::Value,
}

// =============================================================================
// ORDER EVENTS
// =============================================================================

/// ORDER_SUBMITTED payload
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderSubmitted {
    pub order_id: Uuid,
    pub tenant_id: Uuid,
    pub account_id: Uuid,
    pub position_id: Option<Uuid>,
    pub client_order_id: String,
    pub symbol: String,
    pub side: String,       // "buy" or "sell"
    pub order_type: String, // "market", "limit", "stop_loss", "stop_loss_limit"
    pub quantity: Decimal,
    pub price: Option<Decimal>,
    pub stop_price: Option<Decimal>,
}

/// ORDER_ACKED payload
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderAcked {
    pub order_id: Uuid,
    pub exchange_order_id: String,
}

/// ORDER_REJECTED payload
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderRejected {
    pub order_id: Uuid,
    pub reason: String,
}

/// ORDER_CANCELED payload
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderCanceled {
    pub order_id: Uuid,
}

/// FILL_RECEIVED payload
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FillReceived {
    pub fill_id: Uuid,
    pub tenant_id: Uuid,
    pub account_id: Uuid,
    pub order_id: Uuid,
    pub exchange_order_id: String,
    pub exchange_trade_id: String,
    pub symbol: String,
    pub side: String,
    pub fill_price: Decimal,
    pub fill_quantity: Decimal,
    pub fee: Decimal,
    pub fee_asset: String,
    pub is_maker: bool,
    pub filled_at: DateTime<Utc>,
}

// =============================================================================
// POSITION EVENTS
// =============================================================================

/// POSITION_OPENED payload
///
/// INVARIANT: technical_stop_price and technical_stop_distance MUST be
/// non-null. This is the Golden Rule of position sizing - stop distance is
/// derived from technical analysis (2nd support level on chart), not arbitrary
/// percentage.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PositionOpened {
    pub position_id: Uuid,
    pub tenant_id: Uuid,
    pub account_id: Uuid,
    pub strategy_id: Option<Uuid>,
    pub symbol: String,
    pub side: String, // "long" or "short"
    pub entry_price: Option<Decimal>,
    pub entry_quantity: Option<Decimal>,
    pub entry_filled_at: Option<DateTime<Utc>>,

    /// INVARIANT: MUST be non-null - derived from technical analysis (2nd
    /// support)
    pub technical_stop_price: Decimal,

    /// INVARIANT: MUST be non-null - |Entry - Technical Stop|
    pub technical_stop_distance: Decimal,

    pub entry_order_id: Option<Uuid>,
    pub stop_loss_order_id: Option<Uuid>,
}

/// ENTRY_ORDER_PLACED payload (legacy)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntryOrderPlaced {
    pub position_id: Uuid,
    pub order_id: Uuid,
    pub expected_price: Decimal,
    pub quantity: Decimal,
    pub signal_id: Uuid,
    pub timestamp: DateTime<Utc>,
}

/// ENTRY_ORDER_REQUESTED payload
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntryOrderRequested {
    pub position_id: Uuid,
    pub cycle_id: Option<Uuid>,
    pub order_id: Uuid,
    pub client_order_id: String,
    pub expected_price: Decimal,
    pub quantity: Decimal,
    pub signal_id: Uuid,
    pub timestamp: DateTime<Utc>,
}

/// ENTRY_ORDER_ACCEPTED payload (post-exchange ack, no fill fields)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntryOrderAccepted {
    pub position_id: Uuid,
    pub cycle_id: Uuid,
    pub order_id: Uuid,
    pub client_order_id: String,
    pub exchange_order_id: String,
    pub expected_price: Decimal,
    pub quantity: Decimal,
    pub signal_id: Uuid,
    pub timestamp: DateTime<Utc>,
}

/// ENTRY_ORDER_FAILED payload
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntryOrderFailed {
    pub position_id: Uuid,
    pub cycle_id: Uuid,
    pub order_id: Uuid,
    pub client_order_id: String,
    pub signal_id: Uuid,
    pub reason: String,
    pub timestamp: DateTime<Utc>,
}

/// ENTRY_EXECUTION_REJECTED payload
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntryExecutionRejected {
    pub position_id: Uuid,
    pub cycle_id: Uuid,
    pub order_id: Uuid,
    pub client_order_id: String,
    pub signal_id: Uuid,
    pub reason: String,
    pub recoverable: bool,
    pub timestamp: DateTime<Utc>,
}

/// POSITION_CLOSED payload
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PositionClosed {
    pub position_id: Uuid,
    pub exit_order_id: Option<Uuid>,
    pub closed_at: DateTime<Utc>,
}

/// ENTRY_FILLED payload (entry_filled event from domain)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntryFilled {
    pub position_id: Uuid,
    pub order_id: Uuid,
    pub fill_price: Decimal,
    pub filled_quantity: Decimal,
    pub fee: Decimal,
    pub initial_stop: Decimal,
    pub timestamp: DateTime<Utc>,
}

/// ENTRY_SIGNAL_RECEIVED payload (entry_signal_received event from domain)
///
/// Emitted by the engine when a detector signal is received for an armed
/// position. This carries the detector-derived technical stop. It does not
/// change position state; entry_order_accepted performs the Entering transition
/// after the projector verifies this stop exists.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntrySignalReceived {
    pub position_id: Uuid,
    pub signal_id: Uuid,
    pub entry_price: Decimal,
    pub stop_loss: Decimal,
    pub quantity: Decimal,
    pub timestamp: DateTime<Utc>,
}

/// TRAILING_STOP_UPDATED payload (trailing_stop_updated event from domain)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrailingStopUpdated {
    pub position_id: Uuid,
    pub previous_stop: Decimal,
    pub new_stop: Decimal,
    pub trigger_price: Decimal,
    pub timestamp: DateTime<Utc>,
}

/// EXIT_TRIGGERED payload (exit_triggered event from domain)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExitTriggered {
    pub position_id: Uuid,
    pub reason: String,
    pub trigger_price: Decimal,
    pub stop_price: Decimal,
    pub timestamp: DateTime<Utc>,
}

/// EXIT_ORDER_PLACED payload
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExitOrderPlaced {
    pub position_id: Uuid,
    pub order_id: Uuid,
    pub expected_price: Decimal,
    pub quantity: Decimal,
    pub exit_reason: String,
    pub timestamp: DateTime<Utc>,
}

// =============================================================================
// BALANCE EVENTS
// =============================================================================

/// BALANCE_SAMPLED payload
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BalanceSampled {
    pub balance_id: Uuid,
    pub tenant_id: Uuid,
    pub account_id: Uuid,
    pub asset: String,
    pub free: Decimal,
    pub locked: Decimal,
    pub sampled_at: DateTime<Utc>,
}

// =============================================================================
// RISK EVENTS
// =============================================================================

/// RISK_CHECK_FAILED payload
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RiskCheckFailed {
    pub tenant_id: Uuid,
    pub account_id: Uuid,
    pub strategy_id: Option<Uuid>,
    pub violation_reason: String,
}

// =============================================================================
// STRATEGY EVENTS
// =============================================================================

/// STRATEGY_ENABLED payload
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrategyEnabled {
    pub strategy_id: Uuid,
    pub tenant_id: Uuid,
    pub account_id: Uuid,
    pub strategy_name: String,
    pub strategy_type: String,
    pub detector_config: Option<serde_json::Value>,
    pub risk_config: serde_json::Value,
}

/// STRATEGY_DISABLED payload
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrategyDisabled {
    pub strategy_id: Uuid,
    pub reason: Option<String>,
}

// =============================================================================
// DOMAIN POSITION LIFECYCLE EVENTS (emitted by robsond executor, snake_case)
// =============================================================================

/// Minimal Symbol representation matching robson-domain::Symbol serialization.
/// Symbol serializes as {"base": "BTC", "quote": "USDT"}.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymbolPayload {
    pub base: String,
    pub quote: String,
}

impl SymbolPayload {
    /// Produce the canonical trading pair string (e.g. "BTCUSDT").
    pub fn as_pair(&self) -> String {
        format!("{}{}", self.base, self.quote)
    }
}

/// TechnicalStopDistance representation matching
/// robson-domain::TechnicalStopDistance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TechnicalStopDistancePayload {
    pub distance: Decimal,
    pub distance_pct: Decimal,
    /// initial_stop is a Price value object: serialized as Decimal by serde.
    pub initial_stop: Decimal,
}

/// position_armed payload (robson-domain::Event::PositionArmed)
///
/// Emitted by PositionManager::arm_position() via Executor::EmitEvent.
/// Creates an 'armed' row in positions_current. Technical stop data is optional
/// because no detector entry price exists at ARM time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PositionArmed {
    pub position_id: Uuid,
    pub account_id: Uuid,
    pub symbol: SymbolPayload,
    /// "Long" or "Short" (PascalCase, matches Side enum default serde)
    pub side: String,
    pub tech_stop_distance: Option<TechnicalStopDistancePayload>,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

/// position_disarmed payload (robson-domain::Event::PositionDisarmed)
///
/// Emitted when an armed position is disarmed before any entry order.
/// Transitions the row from 'armed' to 'closed'.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PositionDisarmed {
    pub position_id: Uuid,
    pub reason: String,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

/// exit_filled payload (robson-domain::Event::ExitFilled)
///
/// Emitted when the exit order is confirmed filled.
/// Records actual exit fill price and fees; does NOT close the position row —
/// that is done by the subsequent position_closed event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExitFilled {
    pub position_id: Uuid,
    pub order_id: Uuid,
    pub fill_price: Decimal,
    pub filled_quantity: Decimal,
    pub fee: Decimal,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

/// position_closed payload (robson-domain::Event::PositionClosed, lowercase)
///
/// Emitted after the exit fill is confirmed, with final P&L summary.
/// Transitions the row to 'closed' and records realized_pnl + total_fees.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PositionClosedDomain {
    pub position_id: Uuid,
    /// Serialized as string (ExitReason enum variants: "TrailingStop",
    /// "UserPanic", etc.)
    pub exit_reason: String,
    pub entry_price: Decimal,
    pub exit_price: Decimal,
    pub realized_pnl: Decimal,
    pub total_fees: Decimal,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}
