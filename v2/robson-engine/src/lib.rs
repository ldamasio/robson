//! Robson v2 Engine Layer
//!
//! Pure decision logic for position management.
//! The Engine is deterministic: same input always produces same output.
//!
//! # Key Concepts
//!
//! - **No I/O**: Engine never touches network, database, or filesystem
//! - **Pure Functions**: `process(input) -> decisions`
//! - **Trailing Stop**: Exit when price hits trailing stop (1x tech distance)
//!
//! # Example
//!
//! ```
//! use robson_engine::{Engine, MarketData, EngineAction};
//! use robson_domain::{Position, Symbol, Side, Price, RiskConfig};
//! use rust_decimal_macros::dec;
//! use chrono::Utc;
//! use uuid::Uuid;
//!
//! // Create engine with risk config
//! let config = RiskConfig::new(dec!(10000), dec!(1)).unwrap();
//! let engine = Engine::new(config);
//!
//! // Engine processes active positions and returns actions
//! // (See process_active_position for full example)
//! ```

#![warn(clippy::all)]

// Trading strategy modules
pub mod trailing_stop;

use chrono::{DateTime, Utc};
use robson_domain::{
    DetectorSignal, Event, ExitReason, Position, PositionId, PositionState, Price, Quantity,
    RiskConfig, Side, Symbol, TechnicalStopDistance, calculate_position_size,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::debug;

// =============================================================================
// Engine Errors
// =============================================================================

/// Errors that can occur during engine processing
#[derive(Debug, Clone, Error)]
pub enum EngineError {
    /// Position is not in expected state for operation
    #[error("Invalid position state: expected {expected}, got {actual}")]
    InvalidPositionState {
        /// Expected state name
        expected: String,
        /// Actual state name
        actual: String,
    },

    /// Missing required data
    #[error("Missing required data: {0}")]
    MissingData(String),

    /// Invalid market data
    #[error("Invalid market data: {0}")]
    InvalidMarketData(String),

    /// Domain error passthrough
    #[error("Domain error: {0}")]
    DomainError(#[from] robson_domain::DomainError),
}

// =============================================================================
// Market Data
// =============================================================================

/// Real-time market data for a symbol
///
/// Contains the current price and timestamp.
/// Will be extended in future phases for OHLCV data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketData {
    /// Trading pair symbol
    pub symbol: Symbol,
    /// Current market price
    pub current_price: Price,
    /// When this data was captured
    pub timestamp: DateTime<Utc>,
}

impl MarketData {
    /// Create new market data
    pub fn new(symbol: Symbol, current_price: Price) -> Self {
        Self {
            symbol,
            current_price,
            timestamp: Utc::now(),
        }
    }

    /// Create market data with explicit timestamp
    pub fn with_timestamp(symbol: Symbol, current_price: Price, timestamp: DateTime<Utc>) -> Self {
        Self { symbol, current_price, timestamp }
    }
}

// =============================================================================
// Engine Actions
// =============================================================================

/// Actions that the Engine decides should be executed
///
/// These are pure data - the execution layer handles actual I/O.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum EngineAction {
    /// Place entry order (market order to open position)
    ///
    /// Emitted by `decide_entry` when detector signal is valid.
    PlaceEntryOrder {
        /// Position entering
        position_id: PositionId,
        /// Symbol to trade
        symbol: Symbol,
        /// Order side (Long → Buy, Short → Sell)
        side: robson_domain::OrderSide,
        /// Calculated quantity (from position sizing)
        quantity: Quantity,
        /// Signal ID for idempotency tracking
        signal_id: uuid::Uuid,
    },

    /// Update the trailing stop price (favorable price movement)
    UpdateTrailingStop {
        /// Position to update
        position_id: PositionId,
        /// Previous stop price
        previous_stop: Price,
        /// New stop price
        new_stop: Price,
        /// Price that triggered the update
        trigger_price: Price,
    },

    /// Trigger position exit (trailing stop hit)
    TriggerExit {
        /// Position to exit
        position_id: PositionId,
        /// Reason for exit
        reason: ExitReason,
        /// Price that triggered the exit
        trigger_price: Price,
        /// Stop price that was hit
        stop_price: Price,
    },

    /// Place exit order (market order to close position)
    PlaceExitOrder {
        /// Position being exited
        position_id: PositionId,
        /// Symbol to trade
        symbol: Symbol,
        /// Order side (opposite of position side)
        side: robson_domain::OrderSide,
        /// Quantity to exit
        quantity: Quantity,
        /// Reason for exit
        reason: ExitReason,
    },

    /// Emit domain event for audit/persistence
    EmitEvent(Event),
}

// =============================================================================
// Engine Decision
// =============================================================================

/// Result of engine processing
///
/// Contains the actions to execute and the updated position state.
#[derive(Debug, Clone)]
pub struct EngineDecision {
    /// Actions to execute (in order)
    pub actions: Vec<EngineAction>,
    /// Updated position state (if changed)
    pub updated_position: Option<Position>,
}

impl EngineDecision {
    /// Create an empty decision (no actions needed)
    pub fn no_action() -> Self {
        Self { actions: vec![], updated_position: None }
    }

    /// Create a decision with actions
    pub fn with_actions(actions: Vec<EngineAction>) -> Self {
        Self { actions, updated_position: None }
    }

    /// Create a decision with actions and updated position
    pub fn with_position(actions: Vec<EngineAction>, position: Position) -> Self {
        Self {
            actions,
            updated_position: Some(position),
        }
    }

    /// Check if any actions were decided
    pub fn has_actions(&self) -> bool {
        !self.actions.is_empty()
    }
}

// =============================================================================
// Engine
// =============================================================================

/// Pure decision engine for position management
///
/// The Engine processes positions and market data to decide actions.
/// It is completely deterministic and performs no I/O.
///
/// # Responsibilities
///
/// 1. **Trailing Stop Updates**: When price moves favorably, update the stop
/// 2. **Exit Triggers**: When price hits trailing stop, trigger exit
/// 3. **Event Generation**: Emit events for all state changes
///
/// # Example
///
/// ```ignore
/// let engine = Engine::new(risk_config);
/// let decision = engine.process_active_position(&position, &market_data)?;
/// for action in decision.actions {
///     executor.execute(action).await?;
/// }
/// ```
#[derive(Debug, Clone)]
pub struct Engine {
    /// Risk configuration
    risk_config: RiskConfig,
}

impl Engine {
    /// Create a new Engine with risk configuration
    pub fn new(risk_config: RiskConfig) -> Self {
        Self { risk_config }
    }

    /// Get the risk configuration
    pub fn risk_config(&self) -> &RiskConfig {
        &self.risk_config
    }

    // =========================================================================
    // Entry Logic
    // =========================================================================

    /// Process detector signal to decide entry
    ///
    /// Called when a DetectorTask fires a signal for an Armed position.
    /// Validates the signal, calculates position size, and returns entry actions.
    ///
    /// # Arguments
    ///
    /// * `position` - The armed position (must be in Armed state)
    /// * `signal` - The detector signal with entry parameters
    ///
    /// # Returns
    ///
    /// * `Ok(EngineDecision)` with PlaceEntryOrder action and updated position
    /// * `Err(EngineError)` if validation fails
    ///
    /// # State Transition
    ///
    /// ```text
    /// Armed → Entering (with signal_id for idempotency)
    /// ```
    pub fn decide_entry(
        &self,
        position: &Position,
        signal: &DetectorSignal,
    ) -> Result<EngineDecision, EngineError> {
        // 1. Validate position is Armed
        if !matches!(position.state, PositionState::Armed) {
            return Err(EngineError::InvalidPositionState {
                expected: "armed".to_string(),
                actual: position.state.name().to_string(),
            });
        }

        // 2. Validate signal matches position
        signal
            .validate_for_position(position)
            .map_err(|e| EngineError::DomainError(e))?;

        // 3. Validate and get tech stop distance
        let tech_stop = signal.tech_stop_distance();
        tech_stop.validate().map_err(|e| EngineError::DomainError(e))?;

        // 4. Calculate position size (Golden Rule)
        let quantity = calculate_position_size(&self.risk_config, &tech_stop)
            .map_err(|e| EngineError::DomainError(e))?;

        debug!(
            position_id = %position.id,
            signal_id = %signal.signal_id,
            entry_price = %signal.entry_price,
            stop_loss = %signal.stop_loss,
            quantity = %quantity,
            "Entry signal received, placing order"
        );

        // 5. Create updated position in Entering state
        let mut updated_position = position.clone();
        updated_position.state = PositionState::Entering {
            entry_order_id: uuid::Uuid::now_v7(),
            expected_entry: signal.entry_price,
            signal_id: signal.signal_id,
        };
        updated_position.entry_price = Some(signal.entry_price);
        updated_position.tech_stop_distance = Some(tech_stop);
        updated_position.quantity = quantity;
        updated_position.updated_at = Utc::now();

        // 6. Build actions
        let actions = vec![
            // Emit signal received event
            EngineAction::EmitEvent(Event::EntrySignalReceived {
                position_id: position.id,
                signal_id: signal.signal_id,
                entry_price: signal.entry_price,
                stop_loss: signal.stop_loss,
                quantity,
                timestamp: Utc::now(),
            }),
            // Place entry order
            EngineAction::PlaceEntryOrder {
                position_id: position.id,
                symbol: position.symbol.clone(),
                side: position.side.entry_action(),
                quantity,
                signal_id: signal.signal_id,
            },
        ];

        Ok(EngineDecision::with_position(actions, updated_position))
    }

    /// Process entry order fill
    ///
    /// Called when the entry market order is filled.
    /// Transitions position to Active state with initial trailing stop.
    ///
    /// # Arguments
    ///
    /// * `position` - The entering position (must be in Entering state)
    /// * `fill_price` - Actual fill price from exchange
    /// * `filled_quantity` - Actual filled quantity
    ///
    /// # Returns
    ///
    /// * `Ok(EngineDecision)` with updated position in Active state
    /// * `Err(EngineError)` if validation fails
    ///
    /// # State Transition
    ///
    /// ```text
    /// Entering → Active (trailing stop = entry - tech_distance)
    /// ```
    pub fn process_entry_fill(
        &self,
        position: &Position,
        fill_price: Price,
        filled_quantity: Quantity,
    ) -> Result<EngineDecision, EngineError> {
        // 1. Validate position is Entering
        let entry_order_id = match &position.state {
            PositionState::Entering {
                entry_order_id,
                expected_entry: _,
                signal_id: _,
            } => *entry_order_id,
            other => {
                return Err(EngineError::InvalidPositionState {
                    expected: "entering".to_string(),
                    actual: other.name().to_string(),
                });
            },
        };

        // 2. Get tech stop distance
        let tech_stop = position.tech_stop_distance.as_ref().ok_or_else(|| {
            EngineError::MissingData("Position missing tech_stop_distance".to_string())
        })?;

        // 3. Calculate initial trailing stop
        let initial_trailing_stop = match position.side {
            Side::Long => tech_stop.calculate_trailing_stop_long(fill_price.as_decimal()),
            Side::Short => tech_stop.calculate_trailing_stop_short(fill_price.as_decimal()),
        };

        debug!(
            position_id = %position.id,
            fill_price = %fill_price,
            filled_quantity = %filled_quantity,
            initial_trailing_stop = %initial_trailing_stop,
            "Entry filled, position now active"
        );

        // 4. Create updated position in Active state
        let mut updated_position = position.clone();
        updated_position.state = PositionState::Active {
            current_price: fill_price,
            trailing_stop: initial_trailing_stop,
            favorable_extreme: fill_price,
            extreme_at: Utc::now(),
            insurance_stop_id: None, // No insurance stop (Robson manages exits)
            last_emitted_stop: None, // No stop emitted yet
        };
        updated_position.entry_price = Some(fill_price);
        updated_position.quantity = filled_quantity;
        updated_position.entry_filled_at = Some(Utc::now());
        updated_position.updated_at = Utc::now();

        // 5. Build actions (emit event)
        let actions = vec![EngineAction::EmitEvent(Event::EntryFilled {
            position_id: position.id,
            order_id: entry_order_id,
            fill_price,
            filled_quantity,
            fee: rust_decimal::Decimal::ZERO, // Will be updated by executor
            initial_stop: initial_trailing_stop,
            timestamp: Utc::now(),
        })];

        Ok(EngineDecision::with_position(actions, updated_position))
    }

    // =========================================================================
    // Active Position Logic (Exit/Trailing Stop)
    // =========================================================================

    /// Process an active position with current market data
    ///
    /// This is the main entry point for the engine.
    /// It checks if the trailing stop should be updated or if exit should trigger.
    ///
    /// # Arguments
    ///
    /// * `position` - The active position to process
    /// * `market_data` - Current market data
    ///
    /// # Returns
    ///
    /// * `Ok(EngineDecision)` - Actions to execute
    /// * `Err(EngineError)` - If position is not active or data is invalid
    pub fn process_active_position(
        &self,
        position: &Position,
        market_data: &MarketData,
    ) -> Result<EngineDecision, EngineError> {
        // Validate position is active
        let (_current_price_in_state, trailing_stop, favorable_extreme, last_emitted_stop) = match &position.state {
            PositionState::Active {
                current_price,
                trailing_stop,
                favorable_extreme,
                last_emitted_stop,
                ..
            } => (*current_price, *trailing_stop, *favorable_extreme, *last_emitted_stop),
            other => {
                return Err(EngineError::InvalidPositionState {
                    expected: "active".to_string(),
                    actual: other.name().to_string(),
                });
            },
        };

        // Validate symbol matches
        if position.symbol != market_data.symbol {
            return Err(EngineError::InvalidMarketData(format!(
                "Symbol mismatch: position={}, market={}",
                position.symbol, market_data.symbol
            )));
        }

        // Get tech stop distance
        let tech_stop = position.tech_stop_distance.as_ref().ok_or_else(|| {
            EngineError::MissingData("Position missing tech_stop_distance".to_string())
        })?;

        let current_price = market_data.current_price;

        // Check exit first (higher priority)
        if self.should_exit(position.side, current_price, trailing_stop) {
            debug!(
                position_id = %position.id,
                current_price = %current_price,
                trailing_stop = %trailing_stop,
                "Exit triggered"
            );
            return Ok(self.create_exit_decision(position, current_price, trailing_stop));
        }

        // Check if trailing stop should be updated
        if let Some(new_stop) = self.calculate_new_trailing_stop(
            position.side,
            current_price,
            favorable_extreme,
            tech_stop,
        ) {
            // Only update if new stop is more favorable than current stop
            if self.is_more_favorable_stop(position.side, new_stop, trailing_stop) {
                // Idempotency check: only emit if different from last emitted
                if last_emitted_stop != Some(new_stop) {
                    debug!(
                        position_id = %position.id,
                        current_price = %current_price,
                        old_stop = %trailing_stop,
                        new_stop = %new_stop,
                        "Trailing stop updated"
                    );
                    return Ok(self.create_update_stop_decision(
                        position,
                        trailing_stop,
                        new_stop,
                        current_price,
                    ));
                }
            }
        }

        // No action needed
        Ok(EngineDecision::no_action())
    }

    /// Check if position should exit (trailing stop hit)
    fn should_exit(&self, side: Side, current_price: Price, trailing_stop: Price) -> bool {
        match side {
            // LONG: exit when price drops to or below trailing stop
            Side::Long => current_price.as_decimal() <= trailing_stop.as_decimal(),
            // SHORT: exit when price rises to or above trailing stop
            Side::Short => current_price.as_decimal() >= trailing_stop.as_decimal(),
        }
    }

    /// Calculate new trailing stop based on favorable price movement
    ///
    /// Returns `Some(new_stop)` if price has moved favorably beyond current extreme.
    /// Returns `None` if no update is needed.
    fn calculate_new_trailing_stop(
        &self,
        side: Side,
        current_price: Price,
        favorable_extreme: Price,
        tech_stop: &TechnicalStopDistance,
    ) -> Option<Price> {
        match side {
            Side::Long => {
                // LONG: check if we have a new high
                if current_price.as_decimal() > favorable_extreme.as_decimal() {
                    Some(tech_stop.calculate_trailing_stop_long(current_price.as_decimal()))
                } else {
                    None
                }
            },
            Side::Short => {
                // SHORT: check if we have a new low
                if current_price.as_decimal() < favorable_extreme.as_decimal() {
                    Some(tech_stop.calculate_trailing_stop_short(current_price.as_decimal()))
                } else {
                    None
                }
            },
        }
    }

    /// Check if new stop is more favorable than current stop
    fn is_more_favorable_stop(&self, side: Side, new_stop: Price, current_stop: Price) -> bool {
        match side {
            // LONG: higher stop is more favorable
            Side::Long => new_stop.as_decimal() > current_stop.as_decimal(),
            // SHORT: lower stop is more favorable
            Side::Short => new_stop.as_decimal() < current_stop.as_decimal(),
        }
    }

    /// Create decision for exit trigger
    fn create_exit_decision(
        &self,
        position: &Position,
        trigger_price: Price,
        stop_price: Price,
    ) -> EngineDecision {
        let reason = ExitReason::TrailingStop;
        let exit_side = position.side.exit_action();

        let actions = vec![
            // 1. Trigger exit event
            EngineAction::TriggerExit {
                position_id: position.id,
                reason,
                trigger_price,
                stop_price,
            },
            // 2. Place exit order
            EngineAction::PlaceExitOrder {
                position_id: position.id,
                symbol: position.symbol.clone(),
                side: exit_side,
                quantity: position.quantity,
                reason,
            },
            // 3. Emit event
            EngineAction::EmitEvent(Event::ExitTriggered {
                position_id: position.id,
                reason,
                trigger_price,
                stop_price,
                timestamp: Utc::now(),
            }),
        ];

        EngineDecision::with_actions(actions)
    }

    /// Create decision for trailing stop update
    fn create_update_stop_decision(
        &self,
        position: &Position,
        previous_stop: Price,
        new_stop: Price,
        trigger_price: Price,
    ) -> EngineDecision {
        // Update position state with new last_emitted_stop
        let mut updated_position = position.clone();
        if let PositionState::Active {
            current_price,
            trailing_stop,
            favorable_extreme,
            extreme_at,
            insurance_stop_id,
            ..
        } = updated_position.state
        {
            updated_position.state = PositionState::Active {
                current_price,
                trailing_stop,
                favorable_extreme,
                extreme_at,
                insurance_stop_id,
                last_emitted_stop: Some(new_stop), // Mark as emitted
            };
            updated_position.updated_at = Utc::now();
        }

        let actions = vec![
            // 1. Update trailing stop
            EngineAction::UpdateTrailingStop {
                position_id: position.id,
                previous_stop,
                new_stop,
                trigger_price,
            },
            // 2. Emit event
            EngineAction::EmitEvent(Event::TrailingStopUpdated {
                position_id: position.id,
                previous_stop,
                new_stop,
                trigger_price,
                timestamp: Utc::now(),
            }),
        ];

        EngineDecision::with_position(actions, updated_position)
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::Decimal;
    use rust_decimal_macros::dec;
    use uuid::Uuid;

    /// Helper to create a test position in Active state
    fn create_active_position(
        side: Side,
        entry_price: Decimal,
        trailing_stop: Decimal,
        favorable_extreme: Decimal,
        tech_distance: Decimal,
    ) -> Position {
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let mut position = Position::new(Uuid::now_v7(), symbol, side);

        // Set entry price
        position.entry_price = Some(Price::new(entry_price).unwrap());

        // Set tech stop distance
        let stop_price = if side == Side::Long {
            entry_price - tech_distance
        } else {
            entry_price + tech_distance
        };
        position.tech_stop_distance = Some(TechnicalStopDistance::from_entry_and_stop(
            Price::new(entry_price).unwrap(),
            Price::new(stop_price).unwrap(),
        ));

        // Set quantity
        position.quantity = Quantity::new(dec!(0.1)).unwrap();

        // Set active state
        position.state = PositionState::Active {
            current_price: Price::new(entry_price).unwrap(),
            trailing_stop: Price::new(trailing_stop).unwrap(),
            favorable_extreme: Price::new(favorable_extreme).unwrap(),
            extreme_at: Utc::now(),
            insurance_stop_id: None,
            last_emitted_stop: None,
        };

        position
    }

    fn create_market_data(price: Decimal) -> MarketData {
        MarketData::new(Symbol::from_pair("BTCUSDT").unwrap(), Price::new(price).unwrap())
    }

    // =========================================================================
    // Long Position Tests
    // =========================================================================

    #[test]
    fn test_long_no_action_price_stable() {
        let config = RiskConfig::new(dec!(10000), dec!(1)).unwrap();
        let engine = Engine::new(config);

        // Position: entry $95k, stop $93.5k, distance $1.5k
        let position = create_active_position(
            Side::Long,
            dec!(95000),
            dec!(93500), // trailing stop
            dec!(95000), // favorable extreme (entry)
            dec!(1500),  // tech distance
        );

        // Price stable at entry
        let market = create_market_data(dec!(95000));
        let decision = engine.process_active_position(&position, &market).unwrap();

        assert!(!decision.has_actions());
    }

    #[test]
    fn test_long_trailing_stop_update_new_high() {
        let config = RiskConfig::new(dec!(10000), dec!(1)).unwrap();
        let engine = Engine::new(config);

        // Position: entry $95k, stop $93.5k, distance $1.5k
        let position = create_active_position(
            Side::Long,
            dec!(95000),
            dec!(93500), // trailing stop
            dec!(95000), // favorable extreme
            dec!(1500),  // tech distance
        );

        // Price moved up to $96k (new high!)
        let market = create_market_data(dec!(96000));
        let decision = engine.process_active_position(&position, &market).unwrap();

        assert!(decision.has_actions());
        assert_eq!(decision.actions.len(), 2);

        // Check first action is UpdateTrailingStop
        match &decision.actions[0] {
            EngineAction::UpdateTrailingStop { new_stop, previous_stop, .. } => {
                // New stop should be $96k - $1.5k = $94.5k
                assert_eq!(new_stop.as_decimal(), dec!(94500));
                assert_eq!(previous_stop.as_decimal(), dec!(93500));
            },
            _ => panic!("Expected UpdateTrailingStop action"),
        }
    }

    #[test]
    fn test_long_trailing_stop_no_update_price_below_extreme() {
        let config = RiskConfig::new(dec!(10000), dec!(1)).unwrap();
        let engine = Engine::new(config);

        // Position already made a high at $96k
        let position = create_active_position(
            Side::Long,
            dec!(95000),
            dec!(94500), // trailing stop (from $96k high)
            dec!(96000), // favorable extreme
            dec!(1500),  // tech distance
        );

        // Price pulled back to $95.5k (still above stop, below extreme)
        let market = create_market_data(dec!(95500));
        let decision = engine.process_active_position(&position, &market).unwrap();

        // No update - price is below favorable extreme
        assert!(!decision.has_actions());
    }

    #[test]
    fn test_long_exit_triggered_stop_hit() {
        let config = RiskConfig::new(dec!(10000), dec!(1)).unwrap();
        let engine = Engine::new(config);

        let position = create_active_position(
            Side::Long,
            dec!(95000),
            dec!(94500), // trailing stop
            dec!(96000), // favorable extreme
            dec!(1500),  // tech distance
        );

        // Price dropped to stop level
        let market = create_market_data(dec!(94500));
        let decision = engine.process_active_position(&position, &market).unwrap();

        assert!(decision.has_actions());

        // Check for TriggerExit action
        let has_exit = decision.actions.iter().any(|a| {
            matches!(a, EngineAction::TriggerExit { reason, .. } if *reason == ExitReason::TrailingStop)
        });
        assert!(has_exit, "Should have TriggerExit action");

        // Check for PlaceExitOrder action
        let has_order = decision.actions.iter().any(|a| {
            matches!(a, EngineAction::PlaceExitOrder { side, .. } if *side == robson_domain::OrderSide::Sell)
        });
        assert!(has_order, "Should have PlaceExitOrder with Sell side");
    }

    #[test]
    fn test_long_exit_triggered_price_below_stop() {
        let config = RiskConfig::new(dec!(10000), dec!(1)).unwrap();
        let engine = Engine::new(config);

        let position = create_active_position(
            Side::Long,
            dec!(95000),
            dec!(94500), // trailing stop
            dec!(96000), // favorable extreme
            dec!(1500),  // tech distance
        );

        // Price crashed below stop (gap down scenario)
        let market = create_market_data(dec!(94000));
        let decision = engine.process_active_position(&position, &market).unwrap();

        assert!(decision.has_actions());
        let has_exit =
            decision.actions.iter().any(|a| matches!(a, EngineAction::TriggerExit { .. }));
        assert!(has_exit);
    }

    // =========================================================================
    // Short Position Tests
    // =========================================================================

    #[test]
    fn test_short_no_action_price_stable() {
        let config = RiskConfig::new(dec!(10000), dec!(1)).unwrap();
        let engine = Engine::new(config);

        // Short position: entry $95k, stop $96.5k, distance $1.5k
        let position = create_active_position(
            Side::Short,
            dec!(95000),
            dec!(96500), // trailing stop (above entry for short)
            dec!(95000), // favorable extreme (entry)
            dec!(1500),  // tech distance
        );

        let market = create_market_data(dec!(95000));
        let decision = engine.process_active_position(&position, &market).unwrap();

        assert!(!decision.has_actions());
    }

    #[test]
    fn test_short_trailing_stop_update_new_low() {
        let config = RiskConfig::new(dec!(10000), dec!(1)).unwrap();
        let engine = Engine::new(config);

        let position = create_active_position(
            Side::Short,
            dec!(95000),
            dec!(96500), // trailing stop
            dec!(95000), // favorable extreme
            dec!(1500),  // tech distance
        );

        // Price moved down to $94k (new low!)
        let market = create_market_data(dec!(94000));
        let decision = engine.process_active_position(&position, &market).unwrap();

        assert!(decision.has_actions());

        match &decision.actions[0] {
            EngineAction::UpdateTrailingStop { new_stop, previous_stop, .. } => {
                // New stop should be $94k + $1.5k = $95.5k
                assert_eq!(new_stop.as_decimal(), dec!(95500));
                assert_eq!(previous_stop.as_decimal(), dec!(96500));
            },
            _ => panic!("Expected UpdateTrailingStop action"),
        }
    }

    #[test]
    fn test_short_exit_triggered_stop_hit() {
        let config = RiskConfig::new(dec!(10000), dec!(1)).unwrap();
        let engine = Engine::new(config);

        let position = create_active_position(
            Side::Short,
            dec!(95000),
            dec!(95500), // trailing stop (tightened from gains)
            dec!(94000), // favorable extreme (made money)
            dec!(1500),  // tech distance
        );

        // Price rose to stop level
        let market = create_market_data(dec!(95500));
        let decision = engine.process_active_position(&position, &market).unwrap();

        assert!(decision.has_actions());

        // Check for PlaceExitOrder with Buy side (closing short)
        let has_order = decision.actions.iter().any(|a| {
            matches!(a, EngineAction::PlaceExitOrder { side, .. } if *side == robson_domain::OrderSide::Buy)
        });
        assert!(has_order, "Should have PlaceExitOrder with Buy side for short exit");
    }

    // =========================================================================
    // Error Cases
    // =========================================================================

    #[test]
    fn test_error_position_not_active() {
        let config = RiskConfig::new(dec!(10000), dec!(1)).unwrap();
        let engine = Engine::new(config);

        // Create armed position (not active)
        let position =
            Position::new(Uuid::now_v7(), Symbol::from_pair("BTCUSDT").unwrap(), Side::Long);

        let market = create_market_data(dec!(95000));
        let result = engine.process_active_position(&position, &market);

        assert!(result.is_err());
        match result.unwrap_err() {
            EngineError::InvalidPositionState { expected, actual } => {
                assert_eq!(expected, "active");
                assert_eq!(actual, "armed");
            },
            _ => panic!("Expected InvalidPositionState error"),
        }
    }

    #[test]
    fn test_error_symbol_mismatch() {
        let config = RiskConfig::new(dec!(10000), dec!(1)).unwrap();
        let engine = Engine::new(config);

        let position =
            create_active_position(Side::Long, dec!(95000), dec!(93500), dec!(95000), dec!(1500));

        // Market data for different symbol
        let market =
            MarketData::new(Symbol::from_pair("ETHUSDT").unwrap(), Price::new(dec!(3000)).unwrap());

        let result = engine.process_active_position(&position, &market);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), EngineError::InvalidMarketData(_)));
    }

    // =========================================================================
    // Edge Cases
    // =========================================================================

    #[test]
    fn test_long_multiple_updates_sequence() {
        let config = RiskConfig::new(dec!(10000), dec!(1)).unwrap();
        let engine = Engine::new(config);

        // Simulate price moving up in steps
        let mut current_stop = dec!(93500);
        let mut favorable_extreme = dec!(95000);

        for new_price in [dec!(96000), dec!(97000), dec!(98000)] {
            let position = create_active_position(
                Side::Long,
                dec!(95000),
                current_stop,
                favorable_extreme,
                dec!(1500),
            );

            let market = create_market_data(new_price);
            let decision = engine.process_active_position(&position, &market).unwrap();

            if decision.has_actions() {
                if let EngineAction::UpdateTrailingStop { new_stop, .. } = &decision.actions[0] {
                    // Update for next iteration
                    current_stop = new_stop.as_decimal();
                    favorable_extreme = new_price;
                }
            }
        }

        // After three moves up ($95k -> $96k -> $97k -> $98k)
        // Stop should be at $98k - $1.5k = $96.5k
        assert_eq!(current_stop, dec!(96500));
    }

    #[test]
    fn test_exit_takes_priority_over_update() {
        let config = RiskConfig::new(dec!(10000), dec!(1)).unwrap();
        let engine = Engine::new(config);

        // Edge case: price gaps through both update and exit levels
        // This shouldn't happen in practice, but let's ensure exit takes priority
        let position = create_active_position(
            Side::Long,
            dec!(95000),
            dec!(94000), // trailing stop
            dec!(95000), // favorable extreme
            dec!(1500),  // tech distance
        );

        // Price crashed below stop
        let market = create_market_data(dec!(93000));
        let decision = engine.process_active_position(&position, &market).unwrap();

        // Should exit, not update
        let has_exit =
            decision.actions.iter().any(|a| matches!(a, EngineAction::TriggerExit { .. }));
        assert!(has_exit, "Exit should take priority");
    }

    // =========================================================================
    // Entry Logic Tests
    // =========================================================================

    /// Helper to create an armed position for entry tests
    fn create_armed_position(side: Side) -> Position {
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        Position::new(Uuid::now_v7(), symbol, side)
    }

    /// Helper to create a detector signal
    fn create_detector_signal(
        position: &Position,
        entry_price: Decimal,
        stop_loss: Decimal,
    ) -> DetectorSignal {
        DetectorSignal::new(
            position.id,
            position.symbol.clone(),
            position.side,
            Price::new(entry_price).unwrap(),
            Price::new(stop_loss).unwrap(),
        )
    }

    #[test]
    fn test_decide_entry_long_success() {
        let config = RiskConfig::new(dec!(10000), dec!(1)).unwrap();
        let engine = Engine::new(config);

        let position = create_armed_position(Side::Long);
        let signal = create_detector_signal(&position, dec!(95000), dec!(93500));

        let decision = engine.decide_entry(&position, &signal).unwrap();

        // Should have 2 actions: EmitEvent + PlaceEntryOrder
        assert_eq!(decision.actions.len(), 2);

        // Check PlaceEntryOrder action
        let has_entry_order = decision.actions.iter().any(|a| {
            matches!(a, EngineAction::PlaceEntryOrder { side: robson_domain::OrderSide::Buy, .. })
        });
        assert!(has_entry_order, "Should have PlaceEntryOrder with Buy side");

        // Check updated position
        let updated_position = decision.updated_position.expect("Should have updated position");
        assert!(matches!(
            updated_position.state,
            PositionState::Entering { signal_id, .. } if signal_id == signal.signal_id
        ));

        // Verify position sizing (Golden Rule)
        // Capital: $10,000, Risk: 1% = $100 max risk
        // Stop distance: $1,500
        // Expected size: $100 / $1,500 = 0.0666... BTC
        let expected_size = dec!(100) / dec!(1500);
        assert_eq!(updated_position.quantity.as_decimal(), expected_size);
    }

    #[test]
    fn test_decide_entry_short_success() {
        let config = RiskConfig::new(dec!(10000), dec!(1)).unwrap();
        let engine = Engine::new(config);

        let position = create_armed_position(Side::Short);
        // For short: stop is above entry
        let signal = create_detector_signal(&position, dec!(95000), dec!(96500));

        let decision = engine.decide_entry(&position, &signal).unwrap();

        // Check PlaceEntryOrder has Sell side (short entry)
        let has_entry_order = decision.actions.iter().any(|a| {
            matches!(a, EngineAction::PlaceEntryOrder { side: robson_domain::OrderSide::Sell, .. })
        });
        assert!(has_entry_order, "Should have PlaceEntryOrder with Sell side for short");
    }

    #[test]
    fn test_decide_entry_rejects_non_armed() {
        let config = RiskConfig::new(dec!(10000), dec!(1)).unwrap();
        let engine = Engine::new(config);

        // Create an Active position (not Armed)
        let position =
            create_active_position(Side::Long, dec!(95000), dec!(93500), dec!(95000), dec!(1500));

        let signal = DetectorSignal::new(
            position.id,
            position.symbol.clone(),
            position.side,
            Price::new(dec!(95000)).unwrap(),
            Price::new(dec!(93500)).unwrap(),
        );

        let result = engine.decide_entry(&position, &signal);

        assert!(result.is_err());
        match result.unwrap_err() {
            EngineError::InvalidPositionState { expected, actual } => {
                assert_eq!(expected, "armed");
                assert_eq!(actual, "active");
            },
            _ => panic!("Expected InvalidPositionState error"),
        }
    }

    #[test]
    fn test_decide_entry_rejects_signal_mismatch() {
        let config = RiskConfig::new(dec!(10000), dec!(1)).unwrap();
        let engine = Engine::new(config);

        let position = create_armed_position(Side::Long);

        // Create signal with different position_id
        let signal = DetectorSignal::new(
            Uuid::now_v7(), // Different position ID!
            position.symbol.clone(),
            position.side,
            Price::new(dec!(95000)).unwrap(),
            Price::new(dec!(93500)).unwrap(),
        );

        let result = engine.decide_entry(&position, &signal);

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), EngineError::DomainError(_)));
    }

    #[test]
    fn test_decide_entry_validates_tech_stop() {
        let config = RiskConfig::new(dec!(10000), dec!(1)).unwrap();
        let engine = Engine::new(config);

        let position = create_armed_position(Side::Long);

        // Create signal with stop too wide (>10%)
        let signal = create_detector_signal(&position, dec!(100), dec!(80)); // 20% distance

        let result = engine.decide_entry(&position, &signal);

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), EngineError::DomainError(_)));
    }

    #[test]
    fn test_process_entry_fill_long() {
        let config = RiskConfig::new(dec!(10000), dec!(1)).unwrap();
        let engine = Engine::new(config);

        // First, create a position in Entering state
        let position = create_armed_position(Side::Long);
        let signal = create_detector_signal(&position, dec!(95000), dec!(93500));
        let entry_decision = engine.decide_entry(&position, &signal).unwrap();
        let entering_position = entry_decision.updated_position.unwrap();

        // Now process the fill
        let fill_price = Price::new(dec!(95100)).unwrap(); // Slightly higher than expected
        let filled_quantity = Quantity::new(dec!(0.0666666666666666666666666667)).unwrap();

        let fill_decision = engine
            .process_entry_fill(&entering_position, fill_price, filled_quantity)
            .unwrap();

        // Check updated position is Active
        let active_position = fill_decision.updated_position.expect("Should have updated position");
        match &active_position.state {
            PositionState::Active {
                current_price,
                trailing_stop,
                favorable_extreme,
                ..
            } => {
                assert_eq!(current_price.as_decimal(), dec!(95100));
                assert_eq!(favorable_extreme.as_decimal(), dec!(95100));
                // Initial trailing stop = 95100 - 1500 = 93600
                assert_eq!(trailing_stop.as_decimal(), dec!(93600));
            },
            other => panic!("Expected Active state, got {:?}", other.name()),
        }
    }

    #[test]
    fn test_process_entry_fill_short() {
        let config = RiskConfig::new(dec!(10000), dec!(1)).unwrap();
        let engine = Engine::new(config);

        // Create a short position in Entering state
        let position = create_armed_position(Side::Short);
        let signal = create_detector_signal(&position, dec!(95000), dec!(96500)); // Short: stop above
        let entry_decision = engine.decide_entry(&position, &signal).unwrap();
        let entering_position = entry_decision.updated_position.unwrap();

        // Process the fill
        let fill_price = Price::new(dec!(94900)).unwrap(); // Slightly lower (favorable for short)
        let filled_quantity = Quantity::new(dec!(0.0666666666666666666666666667)).unwrap();

        let fill_decision = engine
            .process_entry_fill(&entering_position, fill_price, filled_quantity)
            .unwrap();

        // Check trailing stop for short
        let active_position = fill_decision.updated_position.expect("Should have updated position");
        match &active_position.state {
            PositionState::Active { trailing_stop, favorable_extreme, .. } => {
                assert_eq!(favorable_extreme.as_decimal(), dec!(94900));
                // Initial trailing stop = 94900 + 1500 = 96400
                assert_eq!(trailing_stop.as_decimal(), dec!(96400));
            },
            other => panic!("Expected Active state, got {:?}", other.name()),
        }
    }

    #[test]
    fn test_process_entry_fill_rejects_non_entering() {
        let config = RiskConfig::new(dec!(10000), dec!(1)).unwrap();
        let engine = Engine::new(config);

        // Use an Armed position (not Entering)
        let position = create_armed_position(Side::Long);

        let fill_price = Price::new(dec!(95000)).unwrap();
        let filled_quantity = Quantity::new(dec!(0.1)).unwrap();

        let result = engine.process_entry_fill(&position, fill_price, filled_quantity);

        assert!(result.is_err());
        match result.unwrap_err() {
            EngineError::InvalidPositionState { expected, actual } => {
                assert_eq!(expected, "entering");
                assert_eq!(actual, "armed");
            },
            _ => panic!("Expected InvalidPositionState error"),
        }
    }

    #[test]
    fn test_full_entry_to_exit_flow() {
        let config = RiskConfig::new(dec!(10000), dec!(1)).unwrap();
        let engine = Engine::new(config);

        // 1. Armed position
        let position = create_armed_position(Side::Long);
        assert!(position.can_enter());
        assert!(!position.can_exit());

        // 2. Detector fires signal
        let signal = create_detector_signal(&position, dec!(95000), dec!(93500));
        let entry_decision = engine.decide_entry(&position, &signal).unwrap();
        let entering_position = entry_decision.updated_position.unwrap();

        // Position is now Entering
        assert!(matches!(entering_position.state, PositionState::Entering { .. }));

        // 3. Entry fill received
        let fill_price = Price::new(dec!(95000)).unwrap();
        let filled_quantity = entering_position.quantity;
        let fill_decision = engine
            .process_entry_fill(&entering_position, fill_price, filled_quantity)
            .unwrap();
        let active_position = fill_decision.updated_position.unwrap();

        // Position is now Active
        assert!(active_position.can_exit());
        assert!(!active_position.can_enter());

        // 4. Price moves up, trailing stop updates
        let market_up = create_market_data(dec!(97000));
        let update_decision = engine.process_active_position(&active_position, &market_up).unwrap();
        assert!(update_decision.has_actions());

        // 5. Simulate price hitting trailing stop (exit)
        // After price went to 97k, trailing stop should be 97k - 1.5k = 95.5k
        // Create position with updated trailing stop for this test
        let mut position_after_update = active_position.clone();
        position_after_update.state = PositionState::Active {
            current_price: Price::new(dec!(97000)).unwrap(),
            trailing_stop: Price::new(dec!(95500)).unwrap(),
            favorable_extreme: Price::new(dec!(97000)).unwrap(),
            extreme_at: Utc::now(),
            insurance_stop_id: None,
            last_emitted_stop: Some(Price::new(dec!(95500)).unwrap()),
        };

        // Price drops to trailing stop
        let market_exit = create_market_data(dec!(95500));
        let exit_decision =
            engine.process_active_position(&position_after_update, &market_exit).unwrap();

        // Should trigger exit
        let has_exit = exit_decision
            .actions
            .iter()
            .any(|a| matches!(a, EngineAction::TriggerExit { .. }));
        assert!(has_exit, "Should trigger exit when trailing stop is hit");
    }

    // =========================================================================
    // Idempotency Tests
    // =========================================================================

    #[test]
    fn test_idempotent_trailing_stop_update_same_tick() {
        let config = RiskConfig::new(dec!(10000), dec!(1)).unwrap();
        let engine = Engine::new(config);

        // Position: entry $95k, stop $93.5k, distance $1.5k
        // Price already moved to $97k, trailing stop moved to $95.5k
        let position = create_active_position(
            Side::Long,
            dec!(95000),
            dec!(94500), // trailing stop (previous update from $96k high)
            dec!(96000), // favorable extreme
            dec!(1500),  // tech distance
        );

        // Create position with last_emitted_stop set
        let mut position_with_emitted = position.clone();
        if let PositionState::Active {
            current_price,
            trailing_stop,
            favorable_extreme,
            extreme_at,
            insurance_stop_id,
            ..
        } = position_with_emitted.state
        {
            position_with_emitted.state = PositionState::Active {
                current_price,
                trailing_stop,
                favorable_extreme,
                extreme_at,
                insurance_stop_id,
                last_emitted_stop: Some(Price::new(dec!(94500)).unwrap()), // Already emitted $94.5k
            };
        }

        // Process same price again ($96k - same as favorable extreme)
        // This should NOT trigger a new trailing stop update (idempotency)
        let market = create_market_data(dec!(96000));
        let decision = engine
            .process_active_position(&position_with_emitted, &market)
            .unwrap();

        // No action should be taken because:
        // 1. Price ($96k) is not above favorable_extreme ($96k) - it's equal
        // 2. Even if calculated, the new stop would be same as last_emitted_stop
        assert!(!decision.has_actions(), "Should not emit duplicate trailing stop update");
    }

    #[test]
    fn test_idempotent_trailing_stop_update_repeated_same_price() {
        let config = RiskConfig::new(dec!(10000), dec!(1)).unwrap();
        let engine = Engine::new(config);

        // Position: entry $95k, stop $93.5k, distance $1.5k
        let position = create_active_position(
            Side::Long,
            dec!(95000),
            dec!(94500), // trailing stop
            dec!(96000), // favorable extreme (price went to $96k)
            dec!(1500),  // tech distance
        );

        // Create position with last_emitted_stop set to current trailing stop
        let mut position_with_emitted = position.clone();
        if let PositionState::Active {
            current_price,
            trailing_stop,
            favorable_extreme,
            extreme_at,
            insurance_stop_id,
            ..
        } = position_with_emitted.state
        {
            position_with_emitted.state = PositionState::Active {
                current_price,
                trailing_stop,
                favorable_extreme,
                extreme_at,
                insurance_stop_id,
                last_emitted_stop: Some(Price::new(dec!(94500)).unwrap()), // Already emitted
            };
        }

        // Process same price multiple times (simulating duplicate ticks)
        let market = create_market_data(dec!(95500)); // Price below extreme, no update
        let decision1 = engine
            .process_active_position(&position_with_emitted, &market)
            .unwrap();
        let decision2 = engine
            .process_active_position(&position_with_emitted, &market)
            .unwrap();
        let decision3 = engine
            .process_active_position(&position_with_emitted, &market)
            .unwrap();

        // None should trigger updates
        assert!(!decision1.has_actions());
        assert!(!decision2.has_actions());
        assert!(!decision3.has_actions());
    }
}
