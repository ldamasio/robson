//! Domain Entities for Robson v2
//!
//! Core business entities with lifecycle management.
//! All entities have identity and state transitions.

use crate::value_objects::{
    DomainError, OrderSide, Price, Quantity, RiskConfig, Side, Symbol, TechnicalStopDistance,
};
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
/// - **FIXED 10x leverage** (no configuration needed)
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

    // Position sizing (10x leverage is implicit)
    pub quantity: Quantity,

    // P&L Tracking
    pub realized_pnl: rust_decimal::Decimal,
    pub fees_paid: rust_decimal::Decimal,

    // Associated orders
    pub entry_order_id: Option<OrderId>,
    pub exit_order_id: Option<OrderId>,
    pub insurance_stop_id: Option<OrderId>, // Exchange insurance stop (if enabled)

    // Audit
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub closed_at: Option<DateTime<Utc>>,
}

impl Position {
    /// Create a new armed position
    pub fn new(account_id: AccountId, symbol: Symbol, side: Side) -> Self {
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

/// Calculate position size based on risk management rules
///
/// **THE GOLDEN RULE**: Position size is DERIVED from technical stop distance.
///
/// ```text
/// Position Size = Max Risk Amount / Stop Distance
///               = (Capital × Risk%) / |Entry - Technical Stop|
/// ```
///
/// # Example
///
/// ```
/// # use robson_domain::value_objects::{RiskConfig, Price, TechnicalStopDistance};
/// # use robson_domain::entities::calculate_position_size;
/// # use rust_decimal_macros::dec;
/// let config = RiskConfig::new(dec!(10000), dec!(1)).unwrap(); // $10k, 1% risk
/// let entry = Price::new(dec!(95000)).unwrap();
/// let stop = Price::new(dec!(93500)).unwrap();
/// let tech_stop = TechnicalStopDistance::from_entry_and_stop(entry, stop);
///
/// let size = calculate_position_size(&config, &tech_stop).unwrap();
///
/// // Max Risk = $10,000 × 1% = $100
/// // Stop Distance = $1,500
/// // Position Size = $100 / $1,500 = 0.0666... BTC
/// assert!(size.as_decimal() > dec!(0.066) && size.as_decimal() < dec!(0.067));
/// ```
///
/// # Key Insight
///
/// - Wide technical stop → Smaller position size
/// - Tight technical stop → Larger position size
/// - **Risk amount stays CONSTANT at the configured percentage**
///
/// # Errors
///
/// Returns `DomainError::PositionSizingError` if:
/// - Technical stop distance is zero
/// - Calculated quantity would be <= 0
pub fn calculate_position_size(
    risk_config: &RiskConfig,
    tech_stop: &TechnicalStopDistance,
) -> Result<Quantity, DomainError> {
    // Validate tech stop first
    tech_stop.validate()?;

    let stop_distance = tech_stop.distance;

    if stop_distance <= rust_decimal::Decimal::ZERO {
        return Err(DomainError::PositionSizingError("Stop distance must be positive".to_string()));
    }

    // Golden Rule: Position Size = Max Risk / Stop Distance
    let max_risk = risk_config.max_risk_amount();
    let position_size = max_risk / stop_distance;

    if position_size <= rust_decimal::Decimal::ZERO {
        return Err(DomainError::PositionSizingError(
            "Calculated position size must be positive".to_string(),
        ));
    }

    Quantity::new(position_size).map_err(|e| DomainError::PositionSizingError(e.to_string()))
}

/// Calculate notional value of position
///
/// Notional = Quantity × Entry Price
pub fn calculate_notional_value(quantity: &Quantity, entry_price: &Price) -> rust_decimal::Decimal {
    quantity.as_decimal() * entry_price.as_decimal()
}

/// Calculate margin required for position
///
/// Margin = Notional / Leverage = Notional / 10
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
        /// Insurance stop order on exchange (if enabled)
        insurance_stop_id: Option<OrderId>,
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
            timestamp: Utc::now(),
        }
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
    use super::*;
    use rust_decimal_macros::dec;

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

    // Position Sizing tests (Golden Rule)
    #[test]
    fn test_calculate_position_size_basic() {
        // Setup: $10,000 capital, 1% risk
        let config = RiskConfig::new(dec!(10000), dec!(1)).unwrap();

        // Entry: $95,000, Stop: $93,500 (distance = $1,500)
        let entry = Price::new(dec!(95000)).unwrap();
        let stop = Price::new(dec!(93500)).unwrap();
        let tech_stop = TechnicalStopDistance::from_entry_and_stop(entry, stop);

        let size = calculate_position_size(&config, &tech_stop).unwrap();

        // Expected: $100 risk / $1,500 distance = 0.0666... BTC
        // Check it's approximately 0.0666...
        let expected = dec!(100) / dec!(1500);
        assert_eq!(size.as_decimal(), expected);
    }

    #[test]
    fn test_calculate_position_size_wider_stop() {
        // Wider stop = smaller position
        let config = RiskConfig::new(dec!(10000), dec!(1)).unwrap();

        // Wide stop: $3,000 distance
        let entry = Price::new(dec!(95000)).unwrap();
        let stop = Price::new(dec!(92000)).unwrap();
        let tech_stop = TechnicalStopDistance::from_entry_and_stop(entry, stop);

        let size = calculate_position_size(&config, &tech_stop).unwrap();

        // Expected: $100 / $3,000 = 0.0333... BTC
        let expected = dec!(100) / dec!(3000);
        assert_eq!(size.as_decimal(), expected);
    }

    #[test]
    fn test_calculate_position_size_tighter_stop() {
        // Tighter stop = larger position
        let config = RiskConfig::new(dec!(10000), dec!(1)).unwrap();

        // Tight stop: $500 distance (still valid, ~0.5%)
        let entry = Price::new(dec!(95000)).unwrap();
        let stop = Price::new(dec!(94500)).unwrap();
        let tech_stop = TechnicalStopDistance::from_entry_and_stop(entry, stop);

        let size = calculate_position_size(&config, &tech_stop).unwrap();

        // Expected: $100 / $500 = 0.2 BTC
        assert_eq!(size.as_decimal(), dec!(0.2));
    }

    #[test]
    fn test_calculate_position_size_higher_risk() {
        // 2% risk = double position size
        let config = RiskConfig::new(dec!(10000), dec!(2)).unwrap();

        let entry = Price::new(dec!(95000)).unwrap();
        let stop = Price::new(dec!(93500)).unwrap();
        let tech_stop = TechnicalStopDistance::from_entry_and_stop(entry, stop);

        let size = calculate_position_size(&config, &tech_stop).unwrap();

        // Expected: $200 / $1,500 = 0.1333... BTC
        let expected = dec!(200) / dec!(1500);
        assert_eq!(size.as_decimal(), expected);
    }

    #[test]
    fn test_calculate_notional_value() {
        let quantity = Quantity::new(dec!(0.1)).unwrap();
        let price = Price::new(dec!(95000)).unwrap();

        let notional = calculate_notional_value(&quantity, &price);
        assert_eq!(notional, dec!(9500)); // 0.1 * 95000
    }

    #[test]
    fn test_calculate_margin_required() {
        let quantity = Quantity::new(dec!(0.1)).unwrap();
        let price = Price::new(dec!(95000)).unwrap();

        let margin = calculate_margin_required(&quantity, &price);
        // Notional = $9,500, Leverage = 10x, Margin = $950
        assert_eq!(margin, dec!(950));
    }

    #[test]
    fn test_position_sizing_risk_stays_constant() {
        // This test validates the golden rule:
        // Regardless of stop distance, the risk amount is always 1% of capital
        let config = RiskConfig::new(dec!(10000), dec!(1)).unwrap(); // $100 risk

        // Test 1: Wide stop ($3,000)
        let entry1 = Price::new(dec!(95000)).unwrap();
        let stop1 = Price::new(dec!(92000)).unwrap();
        let tech_stop1 = TechnicalStopDistance::from_entry_and_stop(entry1, stop1);
        let size1 = calculate_position_size(&config, &tech_stop1).unwrap();
        // If stopped out: loss = 0.0333... * $3,000 = $100 ✓
        let loss1 = size1.as_decimal() * dec!(3000);
        // Use round to handle decimal precision
        assert_eq!(loss1.round_dp(2), dec!(100));

        // Test 2: Tight stop ($1,000)
        let entry2 = Price::new(dec!(95000)).unwrap();
        let stop2 = Price::new(dec!(94000)).unwrap();
        let tech_stop2 = TechnicalStopDistance::from_entry_and_stop(entry2, stop2);
        let size2 = calculate_position_size(&config, &tech_stop2).unwrap();
        // If stopped out: loss = 0.1 * $1,000 = $100 ✓
        let loss2 = size2.as_decimal() * dec!(1000);
        assert_eq!(loss2, dec!(100));

        // Both positions risk exactly $100 (1% of capital)
    }
}
