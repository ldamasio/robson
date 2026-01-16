//! Domain Entities for Robson v2
//!
//! Core business entities with lifecycle management.
//! All entities have identity and state transitions.

use crate::value_objects::{DomainError, Price, Quantity, Side, Symbol, Leverage, TechnicalStopDistance, OrderSide};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

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
/// - Isolated margin trading (not spot)
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

    // Position sizing
    pub quantity: Quantity,
    pub leverage: Leverage,

    // P&L Tracking
    pub realized_pnl: rust_decimal::Decimal,
    pub fees_paid: rust_decimal::Decimal,

    // Associated orders
    pub entry_order_id: Option<OrderId>,
    pub exit_order_id: Option<OrderId>,
    pub insurance_stop_id: Option<OrderId>,  // Exchange insurance stop (if enabled)

    // Audit
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub closed_at: Option<DateTime<Utc>>,
}

impl Position {
    /// Create a new armed position
    pub fn new(
        account_id: AccountId,
        symbol: Symbol,
        side: Side,
        leverage: Leverage,
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
            leverage,
            realized_pnl: rust_decimal::Decimal::ZERO,
            fees_paid: rust_decimal::Decimal::ZERO,
            entry_order_id: None,
            exit_order_id: None,
            insurance_stop_id: None,
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
        entry_order_id: OrderId,
        expected_entry: Price,
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
        /// Insurance stop order on exchange (if enabled)
        insurance_stop_id: Option<OrderId>,
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
    Error {
        error: String,
        recoverable: bool,
    },
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
        }
    }
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
}

// =============================================================================
// Order
// =============================================================================

/// Order represents an instruction to buy/sell on the exchange
///
/// NOTE: Trade entity was removed - fill info is consolidated here
/// since isolated margin market orders execute in single fill.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Order {
    pub id: OrderId,
    pub position_id: PositionId,
    pub exchange_order_id: Option<String>,
    pub client_order_id: String,  // intent_id (UUID v7)

    pub symbol: Symbol,
    pub side: OrderSide,
    pub order_type: OrderType,
    pub quantity: Quantity,
    pub price: Option<Price>,  // None for market orders

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
        _stop_price: Price,  // Stop price (stored on exchange, not locally)
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
            price: Some(limit_price),  // Limit price
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
    /// Partially filled (rare in isolated margin)
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

/// Signal from detector to trigger entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectorSignal {
    pub position_id: PositionId,
    pub symbol: Symbol,
    pub side: Side,
    pub entry_price: Price,
    pub stop_loss: Price,  // Technical stop (from chart analysis)
}

impl DetectorSignal {
    /// Calculate technical stop distance from signal
    pub fn tech_stop_distance(&self) -> TechnicalStopDistance {
        TechnicalStopDistance::from_entry_and_stop(self.entry_price, self.stop_loss)
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn test_position_creation() {
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let position = Position::new(
            Uuid::now_v7(),
            symbol,
            Side::Long,
            Leverage::new(3).unwrap(),
        );

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
}
