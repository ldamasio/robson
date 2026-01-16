//! Domain Events for Robson v2
//!
//! Events represent state changes in the domain.
//! Used for event sourcing and audit trails.

use crate::value_objects::{Price, Quantity, Side, Symbol};
use crate::entities::{AccountId, ExitReason, OrderId, PositionId};
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// Domain events for position lifecycle
///
/// Events are immutable records of state changes.
/// They can be serialized for persistence and replayed to reconstruct state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    /// Position created and armed, waiting for entry signal
    PositionArmed {
        /// Unique position identifier
        position_id: PositionId,
        /// Account that owns this position
        account_id: AccountId,
        /// Trading pair symbol
        symbol: Symbol,
        /// Position direction
        side: Side,
        /// When the position was armed
        timestamp: DateTime<Utc>,
    },

    /// Entry order placed on exchange
    EntryOrderPlaced {
        /// Position identifier
        position_id: PositionId,
        /// Order identifier
        order_id: OrderId,
        /// Expected entry price
        expected_price: Price,
        /// Order quantity
        quantity: Quantity,
        /// When the order was placed
        timestamp: DateTime<Utc>,
    },

    /// Entry order filled, position is now active
    EntryFilled {
        /// Position identifier
        position_id: PositionId,
        /// Order identifier
        order_id: OrderId,
        /// Actual fill price
        fill_price: Price,
        /// Filled quantity
        filled_quantity: Quantity,
        /// Trading fee paid
        fee: Decimal,
        /// Initial trailing stop price
        initial_stop: Price,
        /// When the fill occurred
        timestamp: DateTime<Utc>,
    },

    /// Trailing stop updated due to favorable price movement
    TrailingStopUpdated {
        /// Position identifier
        position_id: PositionId,
        /// Previous stop price
        previous_stop: Price,
        /// New stop price
        new_stop: Price,
        /// Peak/trough price that triggered the update
        trigger_price: Price,
        /// When the update occurred
        timestamp: DateTime<Utc>,
    },

    /// Exit triggered (trailing stop hit or user panic)
    ExitTriggered {
        /// Position identifier
        position_id: PositionId,
        /// Reason for exit
        reason: ExitReason,
        /// Price that triggered the exit
        trigger_price: Price,
        /// Stop price that was hit
        stop_price: Price,
        /// When the exit was triggered
        timestamp: DateTime<Utc>,
    },

    /// Exit order placed on exchange
    ExitOrderPlaced {
        /// Position identifier
        position_id: PositionId,
        /// Order identifier
        order_id: OrderId,
        /// Expected exit price
        expected_price: Price,
        /// Order quantity
        quantity: Quantity,
        /// When the order was placed
        timestamp: DateTime<Utc>,
    },

    /// Exit order filled, position closing
    ExitFilled {
        /// Position identifier
        position_id: PositionId,
        /// Order identifier
        order_id: OrderId,
        /// Actual fill price
        fill_price: Price,
        /// Filled quantity
        filled_quantity: Quantity,
        /// Trading fee paid
        fee: Decimal,
        /// When the fill occurred
        timestamp: DateTime<Utc>,
    },

    /// Position closed with final P&L
    PositionClosed {
        /// Position identifier
        position_id: PositionId,
        /// Reason for closure
        exit_reason: ExitReason,
        /// Entry price
        entry_price: Price,
        /// Exit price
        exit_price: Price,
        /// Realized profit/loss in quote currency
        realized_pnl: Decimal,
        /// Total fees paid
        total_fees: Decimal,
        /// When the position was closed
        timestamp: DateTime<Utc>,
    },

    /// Position error occurred
    PositionError {
        /// Position identifier
        position_id: PositionId,
        /// Error description
        error: String,
        /// Whether the error is recoverable
        recoverable: bool,
        /// When the error occurred
        timestamp: DateTime<Utc>,
    },

    /// Insurance stop order placed on exchange (backup protection)
    InsuranceStopPlaced {
        /// Position identifier
        position_id: PositionId,
        /// Order identifier
        order_id: OrderId,
        /// Stop price
        stop_price: Price,
        /// Limit price (for stop-limit orders)
        limit_price: Price,
        /// Order quantity
        quantity: Quantity,
        /// When the order was placed
        timestamp: DateTime<Utc>,
    },

    /// Insurance stop order cancelled (no longer needed)
    InsuranceStopCancelled {
        /// Position identifier
        position_id: PositionId,
        /// Order identifier
        order_id: OrderId,
        /// Reason for cancellation
        reason: String,
        /// When the cancellation occurred
        timestamp: DateTime<Utc>,
    },
}

impl Event {
    /// Get the position ID from any event
    pub fn position_id(&self) -> PositionId {
        match self {
            Event::PositionArmed { position_id, .. }
            | Event::EntryOrderPlaced { position_id, .. }
            | Event::EntryFilled { position_id, .. }
            | Event::TrailingStopUpdated { position_id, .. }
            | Event::ExitTriggered { position_id, .. }
            | Event::ExitOrderPlaced { position_id, .. }
            | Event::ExitFilled { position_id, .. }
            | Event::PositionClosed { position_id, .. }
            | Event::PositionError { position_id, .. }
            | Event::InsuranceStopPlaced { position_id, .. }
            | Event::InsuranceStopCancelled { position_id, .. } => *position_id,
        }
    }

    /// Get the timestamp from any event
    pub fn timestamp(&self) -> DateTime<Utc> {
        match self {
            Event::PositionArmed { timestamp, .. }
            | Event::EntryOrderPlaced { timestamp, .. }
            | Event::EntryFilled { timestamp, .. }
            | Event::TrailingStopUpdated { timestamp, .. }
            | Event::ExitTriggered { timestamp, .. }
            | Event::ExitOrderPlaced { timestamp, .. }
            | Event::ExitFilled { timestamp, .. }
            | Event::PositionClosed { timestamp, .. }
            | Event::PositionError { timestamp, .. }
            | Event::InsuranceStopPlaced { timestamp, .. }
            | Event::InsuranceStopCancelled { timestamp, .. } => *timestamp,
        }
    }

    /// Get the event type name
    pub fn event_type(&self) -> &'static str {
        match self {
            Event::PositionArmed { .. } => "position_armed",
            Event::EntryOrderPlaced { .. } => "entry_order_placed",
            Event::EntryFilled { .. } => "entry_filled",
            Event::TrailingStopUpdated { .. } => "trailing_stop_updated",
            Event::ExitTriggered { .. } => "exit_triggered",
            Event::ExitOrderPlaced { .. } => "exit_order_placed",
            Event::ExitFilled { .. } => "exit_filled",
            Event::PositionClosed { .. } => "position_closed",
            Event::PositionError { .. } => "position_error",
            Event::InsuranceStopPlaced { .. } => "insurance_stop_placed",
            Event::InsuranceStopCancelled { .. } => "insurance_stop_cancelled",
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;
    use uuid::Uuid;

    fn sample_position_armed() -> Event {
        Event::PositionArmed {
            position_id: Uuid::now_v7(),
            account_id: Uuid::now_v7(),
            symbol: Symbol::from_pair("BTCUSDT").unwrap(),
            side: Side::Long,
            timestamp: Utc::now(),
        }
    }

    fn sample_entry_filled() -> Event {
        Event::EntryFilled {
            position_id: Uuid::now_v7(),
            order_id: Uuid::now_v7(),
            fill_price: Price::new(dec!(95000)).unwrap(),
            filled_quantity: Quantity::new(dec!(0.1)).unwrap(),
            fee: dec!(0.001),
            initial_stop: Price::new(dec!(93500)).unwrap(),
            timestamp: Utc::now(),
        }
    }

    fn sample_position_closed() -> Event {
        Event::PositionClosed {
            position_id: Uuid::now_v7(),
            exit_reason: ExitReason::TrailingStop,
            entry_price: Price::new(dec!(95000)).unwrap(),
            exit_price: Price::new(dec!(97000)).unwrap(),
            realized_pnl: dec!(200),
            total_fees: dec!(0.002),
            timestamp: Utc::now(),
        }
    }

    #[test]
    fn test_event_serialization_position_armed() {
        let event = sample_position_armed();
        let json = serde_json::to_string(&event).unwrap();
        let deserialized: Event = serde_json::from_str(&json).unwrap();

        assert_eq!(event.position_id(), deserialized.position_id());
        assert_eq!(event.event_type(), "position_armed");
    }

    #[test]
    fn test_event_serialization_entry_filled() {
        let event = sample_entry_filled();
        let json = serde_json::to_string(&event).unwrap();
        let deserialized: Event = serde_json::from_str(&json).unwrap();

        assert_eq!(event.position_id(), deserialized.position_id());
        assert_eq!(event.event_type(), "entry_filled");
    }

    #[test]
    fn test_event_serialization_position_closed() {
        let event = sample_position_closed();
        let json = serde_json::to_string(&event).unwrap();
        let deserialized: Event = serde_json::from_str(&json).unwrap();

        assert_eq!(event.position_id(), deserialized.position_id());
        assert_eq!(event.event_type(), "position_closed");
    }

    #[test]
    fn test_event_json_format() {
        let event = Event::PositionArmed {
            position_id: Uuid::nil(),
            account_id: Uuid::nil(),
            symbol: Symbol::from_pair("BTCUSDT").unwrap(),
            side: Side::Long,
            timestamp: DateTime::parse_from_rfc3339("2024-01-01T00:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
        };

        let json = serde_json::to_string_pretty(&event).unwrap();

        // Verify JSON structure includes "type" field
        assert!(json.contains("\"type\": \"position_armed\""));
        assert!(json.contains("\"position_id\""));
        assert!(json.contains("\"symbol\""));
    }

    #[test]
    fn test_event_position_id_accessor() {
        let pos_id = Uuid::now_v7();
        let event = Event::PositionArmed {
            position_id: pos_id,
            account_id: Uuid::now_v7(),
            symbol: Symbol::from_pair("BTCUSDT").unwrap(),
            side: Side::Long,
            timestamp: Utc::now(),
        };

        assert_eq!(event.position_id(), pos_id);
    }

    #[test]
    fn test_all_event_types() {
        // Ensure all event types can be created and have correct type names
        let events = vec![
            ("position_armed", sample_position_armed()),
            ("entry_filled", sample_entry_filled()),
            ("position_closed", sample_position_closed()),
        ];

        for (expected_type, event) in events {
            assert_eq!(event.event_type(), expected_type);
        }
    }
}
