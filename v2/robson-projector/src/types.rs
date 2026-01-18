//! Event payload types for projection handlers
//!
//! Only the events we actually implement are defined here.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

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
/// INVARIANT: technical_stop_price and technical_stop_distance MUST be non-null.
/// This is the Golden Rule of position sizing - stop distance is derived from
/// technical analysis (2nd support level on chart), not arbitrary percentage.
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

    /// INVARIANT: MUST be non-null - derived from technical analysis (2nd support)
    pub technical_stop_price: Decimal,

    /// INVARIANT: MUST be non-null - |Entry - Technical Stop|
    pub technical_stop_distance: Decimal,

    pub entry_order_id: Option<Uuid>,
    pub stop_loss_order_id: Option<Uuid>,
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
