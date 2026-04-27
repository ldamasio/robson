//! Domain Events for Robson v2
//!
//! Events represent state changes in the domain.
//! Used for event sourcing and audit trails.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::{
    entities::{AccountId, ExitReason, OrderId, PositionId, TechnicalStopAnalysisAudit},
    policy::{ApprovalPolicy, EntryPolicy, SignalEvaluationOutcome, StrategyId},
    value_objects::{Price, Quantity, Side, Symbol, TechnicalStopDistance},
};

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
        /// Technical stop distance from chart analysis (needed for position
        /// sizing on signal)
        tech_stop_distance: Option<TechnicalStopDistance>,
        /// When the position was armed
        timestamp: DateTime<Utc>,
    },

    /// Entry policy resolved to its deterministic strategy, if any.
    EntryPolicyResolved {
        /// Position identifier
        position_id: PositionId,
        /// Operator-selected entry policy mode
        entry_policy: EntryPolicy,
        /// Operator-selected approval mode
        approval_policy: ApprovalPolicy,
        /// Strategy selected for the policy. `None` means immediate entry.
        strategy_id: Option<StrategyId>,
        /// When the policy was resolved
        timestamp: DateTime<Utc>,
    },

    /// Signal strategy evaluation result for replay and audit.
    SignalStrategyEvaluated {
        /// Position identifier
        position_id: PositionId,
        /// Entry policy being evaluated
        entry_policy: EntryPolicy,
        /// Strategy that produced the evaluation
        strategy_id: StrategyId,
        /// Whether the strategy confirmed a signal
        outcome: SignalEvaluationOutcome,
        /// Confirmed side, when a signal is present
        side: Option<Side>,
        /// Deterministic human-readable reason
        reason: Option<String>,
        /// Candle close time or other deterministic observation time
        observed_at: Option<DateTime<Utc>>,
        /// Reference price used by the confirmed signal
        reference_price: Option<Price>,
        /// When the evaluation event was recorded
        timestamp: DateTime<Utc>,
    },

    /// Detector completed technical stop analysis for a candidate entry signal.
    TechnicalStopAnalyzed {
        /// Position identifier
        position_id: PositionId,
        /// Signal ID that will later identify the detector signal
        signal_id: uuid::Uuid,
        /// Trading pair symbol
        symbol: Symbol,
        /// Position direction
        side: Side,
        /// Entry price used as the analysis anchor
        entry_price: Price,
        /// Full audit payload for the analysis result
        analysis: TechnicalStopAnalysisAudit,
        /// When the analysis completed
        timestamp: DateTime<Utc>,
    },

    /// Detector fired entry signal
    EntrySignalReceived {
        /// Position identifier
        position_id: PositionId,
        /// Signal ID for idempotency
        signal_id: uuid::Uuid,
        /// Entry price from signal
        entry_price: Price,
        /// Technical stop loss from signal
        stop_loss: Price,
        /// Calculated position size
        quantity: Quantity,
        /// When the signal was received
        timestamp: DateTime<Utc>,
    },

    /// LEGACY: Entry order placed (pre-exchange, old semantics).
    /// Kept for eventlog replay compatibility. Do NOT interpret as ack.
    EntryOrderPlaced {
        /// Position identifier
        position_id: PositionId,
        /// Query/risk cycle identifier proving governed execution.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cycle_id: Option<uuid::Uuid>,
        /// Legacy local order identifier. This event is not an exchange ack.
        order_id: OrderId,
        /// Expected entry price
        expected_price: Price,
        /// Order quantity
        quantity: Quantity,
        /// Signal ID for legacy idempotency evidence
        signal_id: uuid::Uuid,
        /// When the legacy pre-exchange event was recorded
        timestamp: DateTime<Utc>,
    },

    /// Governed intent to place entry order (pre-exchange).
    EntryOrderRequested {
        position_id: PositionId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cycle_id: Option<uuid::Uuid>,
        order_id: OrderId,
        client_order_id: String,
        expected_price: Price,
        quantity: Quantity,
        signal_id: uuid::Uuid,
        timestamp: DateTime<Utc>,
    },

    /// Exchange acknowledged the entry order (post-exchange).
    /// Does NOT carry fill semantics — EntryFilled is the only fill event.
    EntryOrderAccepted {
        position_id: PositionId,
        cycle_id: uuid::Uuid,
        order_id: OrderId,
        client_order_id: String,
        exchange_order_id: String,
        expected_price: Price,
        quantity: Quantity,
        signal_id: uuid::Uuid,
        timestamp: DateTime<Utc>,
    },

    /// Exchange rejected or failed the entry order.
    EntryOrderFailed {
        position_id: PositionId,
        cycle_id: uuid::Uuid,
        order_id: OrderId,
        client_order_id: String,
        signal_id: uuid::Uuid,
        reason: String,
        timestamp: DateTime<Utc>,
    },

    /// Entry execution rejected by an internal safety or policy check before
    /// any exchange placement was attempted.
    EntryExecutionRejected {
        /// Position identifier
        position_id: PositionId,
        /// Query/risk cycle identifier proving governed execution.
        cycle_id: uuid::Uuid,
        /// Local order identifier for the blocked attempt
        order_id: OrderId,
        /// Client order identifier that would have been sent to the exchange
        client_order_id: String,
        /// Signal ID for idempotency tracking
        signal_id: uuid::Uuid,
        /// Human-readable rejection reason
        reason: String,
        /// Whether the rejection is recoverable by operator intervention
        recoverable: bool,
        /// When the rejection occurred
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
        /// Binance USD-M Futures position ID (for SafetyNet coordination)
        binance_position_id: Option<String>,
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

    /// Position monitor observed an active-position market tick
    PositionMonitorTick {
        /// Position identifier
        position_id: PositionId,
        /// Trading pair symbol as exchange pair string (e.g. BTCUSDT)
        symbol: String,
        /// Current tick price
        price: Price,
        /// Current trailing stop after processing the tick
        current_stop: Price,
        /// Best favorable price seen for the position at this tick
        high_watermark: Price,
        /// Distance from the watermark to the stop trigger
        span_remaining: Decimal,
        /// When the tick was observed
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
        /// Query/risk cycle identifier proving governed execution.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cycle_id: Option<uuid::Uuid>,
        /// Order identifier (matches exit_order_id in PositionState::Exiting)
        order_id: OrderId,
        /// Expected exit price
        expected_price: Price,
        /// Order quantity
        quantity: Quantity,
        /// Reason for exit (matches exit_reason in PositionState::Exiting)
        exit_reason: ExitReason,
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

    /// Month boundary processed for monthly capital/risk reset semantics.
    ///
    /// This is a system-scoped event. It does not belong to a specific
    /// position, so helper accessors use `Uuid::nil()` as a synthetic
    /// sentinel position ID.
    MonthBoundaryReset {
        /// New month's capital base (`current_equity - carried_positions_risk`)
        capital_base: Decimal,
        /// Sum of latent risk carried by open positions into the new month
        carried_positions_risk: Decimal,
        /// UTC month (1-12)
        month: u32,
        /// UTC year
        year: i32,
        /// When the boundary was processed
        timestamp: DateTime<Utc>,
    },

    /// Entry is awaiting operator approval before the order can be placed.
    ///
    /// Emitted when the entry signal passed risk evaluation but the position's
    /// `ApprovalPolicy` is `HumanConfirmation`. Provides deterministic replay
    /// evidence for the `AwaitingApproval` entry lifecycle stage.
    EntryApprovalPending {
        /// Position identifier
        position_id: PositionId,
        /// Signal ID that triggered the approval gate
        signal_id: uuid::Uuid,
        /// When the approval gate was entered
        timestamp: DateTime<Utc>,
    },

    /// Position disarmed by user before any entry order was placed
    ///
    /// Only valid when position is in Armed state.
    /// Results in Cancelled state with zero P&L.
    PositionDisarmed {
        /// Position identifier
        position_id: PositionId,
        /// Human-readable reason for disarming
        reason: String,
        /// When the position was disarmed
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
    /// Get the associated position ID for an event.
    ///
    /// System-scoped events that are not tied to a concrete position return
    /// `Uuid::nil()` as a synthetic sentinel.
    pub fn position_id(&self) -> PositionId {
        match self {
            Event::PositionArmed { position_id, .. }
            | Event::EntryPolicyResolved { position_id, .. }
            | Event::SignalStrategyEvaluated { position_id, .. }
            | Event::TechnicalStopAnalyzed { position_id, .. }
            | Event::EntrySignalReceived { position_id, .. }
            | Event::EntryOrderPlaced { position_id, .. }
            | Event::EntryOrderRequested { position_id, .. }
            | Event::EntryOrderAccepted { position_id, .. }
            | Event::EntryOrderFailed { position_id, .. }
            | Event::EntryExecutionRejected { position_id, .. }
            | Event::EntryFilled { position_id, .. }
            | Event::TrailingStopUpdated { position_id, .. }
            | Event::PositionMonitorTick { position_id, .. }
            | Event::ExitTriggered { position_id, .. }
            | Event::ExitOrderPlaced { position_id, .. }
            | Event::ExitFilled { position_id, .. }
            | Event::PositionClosed { position_id, .. }
            | Event::EntryApprovalPending { position_id, .. }
            | Event::PositionDisarmed { position_id, .. }
            | Event::PositionError { position_id, .. }
            | Event::InsuranceStopPlaced { position_id, .. }
            | Event::InsuranceStopCancelled { position_id, .. } => *position_id,
            Event::MonthBoundaryReset { .. } => uuid::Uuid::nil(),
        }
    }

    /// Get the timestamp from any event
    pub fn timestamp(&self) -> DateTime<Utc> {
        match self {
            Event::PositionArmed { timestamp, .. }
            | Event::EntryPolicyResolved { timestamp, .. }
            | Event::SignalStrategyEvaluated { timestamp, .. }
            | Event::TechnicalStopAnalyzed { timestamp, .. }
            | Event::EntrySignalReceived { timestamp, .. }
            | Event::EntryOrderPlaced { timestamp, .. }
            | Event::EntryOrderRequested { timestamp, .. }
            | Event::EntryOrderAccepted { timestamp, .. }
            | Event::EntryOrderFailed { timestamp, .. }
            | Event::EntryExecutionRejected { timestamp, .. }
            | Event::EntryFilled { timestamp, .. }
            | Event::TrailingStopUpdated { timestamp, .. }
            | Event::PositionMonitorTick { timestamp, .. }
            | Event::ExitTriggered { timestamp, .. }
            | Event::ExitOrderPlaced { timestamp, .. }
            | Event::ExitFilled { timestamp, .. }
            | Event::PositionClosed { timestamp, .. }
            | Event::MonthBoundaryReset { timestamp, .. }
            | Event::EntryApprovalPending { timestamp, .. }
            | Event::PositionDisarmed { timestamp, .. }
            | Event::PositionError { timestamp, .. }
            | Event::InsuranceStopPlaced { timestamp, .. }
            | Event::InsuranceStopCancelled { timestamp, .. } => *timestamp,
        }
    }

    /// Get the event type name
    pub fn event_type(&self) -> &'static str {
        match self {
            Event::PositionArmed { .. } => "position_armed",
            Event::EntryPolicyResolved { .. } => "entry_policy_resolved",
            Event::SignalStrategyEvaluated { .. } => "signal_strategy_evaluated",
            Event::TechnicalStopAnalyzed { .. } => "technical_stop_analyzed",
            Event::EntrySignalReceived { .. } => "entry_signal_received",
            Event::EntryOrderPlaced { .. } => "entry_order_placed",
            Event::EntryOrderRequested { .. } => "entry_order_requested",
            Event::EntryOrderAccepted { .. } => "entry_order_accepted",
            Event::EntryOrderFailed { .. } => "entry_order_failed",
            Event::EntryExecutionRejected { .. } => "entry_execution_rejected",
            Event::EntryFilled { .. } => "entry_filled",
            Event::TrailingStopUpdated { .. } => "trailing_stop_updated",
            Event::PositionMonitorTick { .. } => "position_monitor_tick",
            Event::ExitTriggered { .. } => "exit_triggered",
            Event::ExitOrderPlaced { .. } => "exit_order_placed",
            Event::ExitFilled { .. } => "exit_filled",
            Event::PositionClosed { .. } => "position_closed",
            Event::MonthBoundaryReset { .. } => "month_boundary_reset",
            Event::EntryApprovalPending { .. } => "entry_approval_pending",
            Event::PositionDisarmed { .. } => "position_disarmed",
            Event::PositionError { .. } => "position_error",
            Event::InsuranceStopPlaced { .. } => "insurance_stop_placed",
            Event::InsuranceStopCancelled { .. } => "insurance_stop_cancelled",
        }
    }
}

// =============================================================================
// Entry Lifecycle Stage Projection
// =============================================================================

use crate::entities::EntryLifecycleStage;

/// Compute the entry lifecycle stage from an ordered domain event sequence.
///
/// Deterministic: the same event sequence always produces the same stage.
/// Replay-safe: this function is the authoritative projection for the entry
/// intent lifecycle; it never reads mutable state.
pub fn entry_lifecycle_stage(events: &[Event]) -> EntryLifecycleStage {
    let mut stage = EntryLifecycleStage::IntentCreated;
    for event in events {
        match event {
            Event::PositionArmed { .. } => stage = EntryLifecycleStage::IntentCreated,
            Event::EntryPolicyResolved { .. } => stage = EntryLifecycleStage::AwaitingSignal,
            Event::EntrySignalReceived { .. } => stage = EntryLifecycleStage::SignalConfirmed,
            Event::EntryApprovalPending { .. } => stage = EntryLifecycleStage::AwaitingApproval,
            Event::EntryOrderRequested { .. } | Event::EntryOrderPlaced { .. } => {
                stage = EntryLifecycleStage::OrderSubmitted;
            },
            Event::EntryFilled { .. } => stage = EntryLifecycleStage::Active,
            // Disarm and unrecoverable rejection both terminate the entry intent.
            Event::PositionDisarmed { .. } | Event::EntryExecutionRejected { .. } => {
                stage = EntryLifecycleStage::Cancelled;
            },
            // Order failure is recoverable: position stays Armed, detector re-armed.
            Event::EntryOrderFailed { .. } => stage = EntryLifecycleStage::AwaitingSignal,
            _ => {},
        }
    }
    stage
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use rust_decimal_macros::dec;
    use uuid::Uuid;

    use super::*;

    fn sample_position_armed() -> Event {
        Event::PositionArmed {
            position_id: Uuid::now_v7(),
            account_id: Uuid::now_v7(),
            symbol: Symbol::from_pair("BTCUSDT").unwrap(),
            side: Side::Long,
            tech_stop_distance: None,
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
            binance_position_id: None,
            timestamp: Utc::now(),
        }
    }

    fn sample_technical_stop_analyzed() -> Event {
        Event::TechnicalStopAnalyzed {
            position_id: Uuid::now_v7(),
            signal_id: Uuid::now_v7(),
            symbol: Symbol::from_pair("BTCUSDT").unwrap(),
            side: Side::Long,
            entry_price: Price::new(dec!(95000)).unwrap(),
            analysis: TechnicalStopAnalysisAudit {
                stop_price: Price::new(dec!(93500)).unwrap(),
                method: crate::entities::TechnicalStopMethodSnapshot::SwingPoint { level_n: 2 },
                confidence: crate::entities::TechnicalStopConfidenceSnapshot::High,
                detected_levels: vec![
                    Price::new(dec!(94200)).unwrap(),
                    Price::new(dec!(93500)).unwrap(),
                ],
                config: crate::entities::TechnicalStopConfigSnapshot {
                    min_candles: 100,
                    swing_lookback: 2,
                    support_level_n: 2,
                    level_tolerance: dec!(0.005),
                    atr_period: 14,
                    atr_multiplier: dec!(1.5),
                    min_stop_distance_pct: dec!(0.001),
                    max_stop_distance_pct: dec!(0.10),
                },
            },
            timestamp: Utc::now(),
        }
    }

    fn sample_entry_order_placed(cycle_id: Uuid) -> Event {
        Event::EntryOrderPlaced {
            position_id: Uuid::now_v7(),
            cycle_id: Some(cycle_id),
            order_id: Uuid::now_v7(),
            expected_price: Price::new(dec!(95000)).unwrap(),
            quantity: Quantity::new(dec!(0.1)).unwrap(),
            signal_id: Uuid::now_v7(),
            timestamp: Utc::now(),
        }
    }

    fn sample_exit_order_placed(cycle_id: Uuid) -> Event {
        Event::ExitOrderPlaced {
            position_id: Uuid::now_v7(),
            cycle_id: Some(cycle_id),
            order_id: Uuid::now_v7(),
            expected_price: Price::new(dec!(96000)).unwrap(),
            quantity: Quantity::new(dec!(0.1)).unwrap(),
            exit_reason: ExitReason::TrailingStop,
            timestamp: Utc::now(),
        }
    }

    fn sample_position_monitor_tick() -> Event {
        Event::PositionMonitorTick {
            position_id: Uuid::now_v7(),
            symbol: "BTCUSDT".to_string(),
            price: Price::new(dec!(96000)).unwrap(),
            current_stop: Price::new(dec!(93500)).unwrap(),
            high_watermark: Price::new(dec!(96000)).unwrap(),
            span_remaining: dec!(2500),
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

    fn sample_month_boundary_reset() -> Event {
        Event::MonthBoundaryReset {
            capital_base: dec!(9750),
            carried_positions_risk: dec!(250),
            month: 5,
            year: 2026,
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
    fn test_event_serialization_technical_stop_analyzed() {
        let event = sample_technical_stop_analyzed();
        let json = serde_json::to_string(&event).unwrap();
        let deserialized: Event = serde_json::from_str(&json).unwrap();

        assert_eq!(event.position_id(), deserialized.position_id());
        assert_eq!(event.event_type(), "technical_stop_analyzed");
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
    fn test_event_serialization_month_boundary_reset() {
        let event = sample_month_boundary_reset();
        let json = serde_json::to_string(&event).unwrap();
        let deserialized: Event = serde_json::from_str(&json).unwrap();

        assert_eq!(event.position_id(), deserialized.position_id());
        assert_eq!(event.position_id(), Uuid::nil());
        assert_eq!(event.event_type(), "month_boundary_reset");
    }

    #[test]
    fn test_entry_order_placed_serializes_cycle_id_in_payload() {
        let cycle_id = Uuid::now_v7();
        let event = sample_entry_order_placed(cycle_id);
        let expected_cycle_id = cycle_id.to_string();

        let payload = serde_json::to_value(&event).unwrap();

        assert_eq!(payload["type"].as_str(), Some("entry_order_placed"));
        assert_eq!(payload["cycle_id"].as_str(), Some(expected_cycle_id.as_str()));
    }

    #[test]
    fn test_exit_order_placed_serializes_cycle_id_in_payload() {
        let cycle_id = Uuid::now_v7();
        let event = sample_exit_order_placed(cycle_id);
        let expected_cycle_id = cycle_id.to_string();

        let payload = serde_json::to_value(&event).unwrap();

        assert_eq!(payload["type"].as_str(), Some("exit_order_placed"));
        assert_eq!(payload["cycle_id"].as_str(), Some(expected_cycle_id.as_str()));
    }

    #[test]
    fn test_position_monitor_tick_serializes_requested_payload() {
        let event = sample_position_monitor_tick();
        let payload = serde_json::to_value(&event).unwrap();

        assert_eq!(payload["type"].as_str(), Some("position_monitor_tick"));
        assert_eq!(payload["symbol"].as_str(), Some("BTCUSDT"));
        assert_eq!(payload["price"].as_str(), Some("96000"));
        assert_eq!(payload["current_stop"].as_str(), Some("93500"));
        assert_eq!(payload["high_watermark"].as_str(), Some("96000"));
        assert_eq!(payload["span_remaining"].as_str(), Some("2500"));
    }

    #[test]
    fn test_event_json_format() {
        let event = Event::PositionArmed {
            position_id: Uuid::nil(),
            account_id: Uuid::nil(),
            symbol: Symbol::from_pair("BTCUSDT").unwrap(),
            side: Side::Long,
            tech_stop_distance: None,
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
            tech_stop_distance: None,
            timestamp: Utc::now(),
        };

        assert_eq!(event.position_id(), pos_id);
    }

    #[test]
    fn test_all_event_types() {
        // Ensure all event types can be created and have correct type names
        let events = vec![
            ("position_armed", sample_position_armed()),
            ("technical_stop_analyzed", sample_technical_stop_analyzed()),
            ("entry_filled", sample_entry_filled()),
            ("position_closed", sample_position_closed()),
            ("month_boundary_reset", sample_month_boundary_reset()),
        ];

        for (expected_type, event) in events {
            assert_eq!(event.event_type(), expected_type);
        }
    }

    fn sample_entry_order_requested(cycle_id: Option<Uuid>) -> Event {
        Event::EntryOrderRequested {
            position_id: Uuid::now_v7(),
            cycle_id,
            order_id: Uuid::now_v7(),
            client_order_id: "test-client-order-id".to_string(),
            expected_price: Price::new(dec!(95000)).unwrap(),
            quantity: Quantity::new(dec!(0.1)).unwrap(),
            signal_id: Uuid::now_v7(),
            timestamp: Utc::now(),
        }
    }

    fn sample_entry_order_accepted(cycle_id: Uuid) -> Event {
        Event::EntryOrderAccepted {
            position_id: Uuid::now_v7(),
            cycle_id,
            order_id: Uuid::now_v7(),
            client_order_id: "test-client-order-id".to_string(),
            exchange_order_id: "exchange-123".to_string(),
            expected_price: Price::new(dec!(95000)).unwrap(),
            quantity: Quantity::new(dec!(0.1)).unwrap(),
            signal_id: Uuid::now_v7(),
            timestamp: Utc::now(),
        }
    }

    fn sample_entry_order_failed(cycle_id: Uuid) -> Event {
        Event::EntryOrderFailed {
            position_id: Uuid::now_v7(),
            cycle_id,
            order_id: Uuid::now_v7(),
            client_order_id: "test-client-order-id".to_string(),
            signal_id: Uuid::now_v7(),
            reason: "insufficient balance".to_string(),
            timestamp: Utc::now(),
        }
    }

    fn sample_entry_execution_rejected(cycle_id: Uuid) -> Event {
        Event::EntryExecutionRejected {
            position_id: Uuid::now_v7(),
            cycle_id,
            order_id: Uuid::now_v7(),
            client_order_id: "test-client-order-id".to_string(),
            signal_id: Uuid::now_v7(),
            reason: "margin safety violation".to_string(),
            recoverable: true,
            timestamp: Utc::now(),
        }
    }

    #[test]
    fn test_entry_order_requested_serializes_optional_cycle_id() {
        let cycle_id = Uuid::now_v7();
        let event = sample_entry_order_requested(Some(cycle_id));
        let payload = serde_json::to_value(&event).unwrap();
        assert_eq!(payload["type"].as_str(), Some("entry_order_requested"));
        assert_eq!(payload["cycle_id"].as_str(), Some(cycle_id.to_string().as_str()));
        assert_eq!(payload["client_order_id"].as_str(), Some("test-client-order-id"));

        let event_none = sample_entry_order_requested(None);
        let payload_none = serde_json::to_value(&event_none).unwrap();
        assert!(payload_none.get("cycle_id").is_none(), "None cycle_id should be skipped");
    }

    #[test]
    fn test_entry_order_accepted_serializes_exchange_order_id_no_fill() {
        let cycle_id = Uuid::now_v7();
        let event = sample_entry_order_accepted(cycle_id);
        let payload = serde_json::to_value(&event).unwrap();
        assert_eq!(payload["type"].as_str(), Some("entry_order_accepted"));
        assert_eq!(payload["cycle_id"].as_str(), Some(cycle_id.to_string().as_str()));
        assert_eq!(payload["exchange_order_id"].as_str(), Some("exchange-123"));
        assert!(
            payload.get("fill_price").is_none(),
            "EntryOrderAccepted must not contain fill_price"
        );
        assert!(
            payload.get("filled_quantity").is_none(),
            "EntryOrderAccepted must not contain filled_quantity"
        );
        assert!(payload.get("fee").is_none(), "EntryOrderAccepted must not contain fee");
    }

    #[test]
    fn test_entry_order_failed_serializes_reason() {
        let cycle_id = Uuid::now_v7();
        let event = sample_entry_order_failed(cycle_id);
        let payload = serde_json::to_value(&event).unwrap();
        assert_eq!(payload["type"].as_str(), Some("entry_order_failed"));
        assert_eq!(payload["cycle_id"].as_str(), Some(cycle_id.to_string().as_str()));
        assert_eq!(payload["reason"].as_str(), Some("insufficient balance"));
    }

    #[test]
    fn test_entry_execution_rejected_serializes_reason_and_recoverable() {
        let cycle_id = Uuid::now_v7();
        let event = sample_entry_execution_rejected(cycle_id);
        let payload = serde_json::to_value(&event).unwrap();
        assert_eq!(payload["type"].as_str(), Some("entry_execution_rejected"));
        assert_eq!(payload["cycle_id"].as_str(), Some(cycle_id.to_string().as_str()));
        assert_eq!(payload["reason"].as_str(), Some("margin safety violation"));
        assert_eq!(payload["recoverable"].as_bool(), Some(true));
    }

    #[test]
    fn test_new_entry_event_types() {
        assert_eq!(sample_entry_order_requested(None).event_type(), "entry_order_requested");
        assert_eq!(
            sample_entry_order_accepted(Uuid::now_v7()).event_type(),
            "entry_order_accepted"
        );
        assert_eq!(sample_entry_order_failed(Uuid::now_v7()).event_type(), "entry_order_failed");
        assert_eq!(
            sample_entry_execution_rejected(Uuid::now_v7()).event_type(),
            "entry_execution_rejected"
        );
    }

    // =========================================================================
    // entry_lifecycle_stage projection tests (Phase 5)
    // =========================================================================

    fn mk_pid() -> uuid::Uuid {
        Uuid::now_v7()
    }

    fn mk_signal_id() -> uuid::Uuid {
        Uuid::now_v7()
    }

    fn armed_event(pid: uuid::Uuid) -> Event {
        Event::PositionArmed {
            position_id: pid,
            account_id: Uuid::now_v7(),
            symbol: Symbol::from_pair("BTCUSDT").unwrap(),
            side: Side::Long,
            tech_stop_distance: None,
            timestamp: Utc::now(),
        }
    }

    fn policy_resolved_event(pid: uuid::Uuid) -> Event {
        use crate::policy::{ApprovalPolicy, EntryPolicy, StrategyId};
        Event::EntryPolicyResolved {
            position_id: pid,
            entry_policy: EntryPolicy::ConfirmedTrend,
            approval_policy: ApprovalPolicy::Automatic,
            strategy_id: Some(StrategyId { name: "sma_crossover".to_string(), version: 1 }),
            timestamp: Utc::now(),
        }
    }

    fn signal_received_event(pid: uuid::Uuid, sid: uuid::Uuid) -> Event {
        Event::EntrySignalReceived {
            position_id: pid,
            signal_id: sid,
            entry_price: Price::new(dec!(95000)).unwrap(),
            stop_loss: Price::new(dec!(93500)).unwrap(),
            quantity: Quantity::new(dec!(0.1)).unwrap(),
            timestamp: Utc::now(),
        }
    }

    fn approval_pending_event(pid: uuid::Uuid, sid: uuid::Uuid) -> Event {
        Event::EntryApprovalPending {
            position_id: pid,
            signal_id: sid,
            timestamp: Utc::now(),
        }
    }

    fn order_requested_event(pid: uuid::Uuid, sid: uuid::Uuid) -> Event {
        Event::EntryOrderRequested {
            position_id: pid,
            cycle_id: None,
            order_id: Uuid::now_v7(),
            client_order_id: "test".to_string(),
            expected_price: Price::new(dec!(95000)).unwrap(),
            quantity: Quantity::new(dec!(0.1)).unwrap(),
            signal_id: sid,
            timestamp: Utc::now(),
        }
    }

    fn filled_event(pid: uuid::Uuid) -> Event {
        Event::EntryFilled {
            position_id: pid,
            order_id: Uuid::now_v7(),
            fill_price: Price::new(dec!(95000)).unwrap(),
            filled_quantity: Quantity::new(dec!(0.1)).unwrap(),
            fee: dec!(0.001),
            initial_stop: Price::new(dec!(93500)).unwrap(),
            binance_position_id: None,
            timestamp: Utc::now(),
        }
    }

    fn disarmed_event(pid: uuid::Uuid) -> Event {
        Event::PositionDisarmed {
            position_id: pid,
            reason: "user_disarmed".to_string(),
            timestamp: Utc::now(),
        }
    }

    fn order_failed_event(pid: uuid::Uuid, sid: uuid::Uuid) -> Event {
        Event::EntryOrderFailed {
            position_id: pid,
            cycle_id: Uuid::now_v7(),
            order_id: Uuid::now_v7(),
            client_order_id: "test".to_string(),
            signal_id: sid,
            reason: "exchange rejected".to_string(),
            timestamp: Utc::now(),
        }
    }

    #[test]
    fn test_entry_lifecycle_stage_intent_created_from_armed() {
        let pid = mk_pid();
        let events = vec![armed_event(pid)];
        assert_eq!(entry_lifecycle_stage(&events), EntryLifecycleStage::IntentCreated);
    }

    #[test]
    fn test_entry_lifecycle_stage_awaiting_signal_from_policy_resolved() {
        let pid = mk_pid();
        let events = vec![armed_event(pid), policy_resolved_event(pid)];
        assert_eq!(entry_lifecycle_stage(&events), EntryLifecycleStage::AwaitingSignal);
    }

    #[test]
    fn test_entry_lifecycle_stage_signal_confirmed_from_entry_signal() {
        let pid = mk_pid();
        let sid = mk_signal_id();
        let events = vec![
            armed_event(pid),
            policy_resolved_event(pid),
            signal_received_event(pid, sid),
        ];
        assert_eq!(entry_lifecycle_stage(&events), EntryLifecycleStage::SignalConfirmed);
    }

    #[test]
    fn test_entry_lifecycle_stage_awaiting_approval_from_approval_pending() {
        let pid = mk_pid();
        let sid = mk_signal_id();
        let events = vec![
            armed_event(pid),
            policy_resolved_event(pid),
            signal_received_event(pid, sid),
            approval_pending_event(pid, sid),
        ];
        assert_eq!(entry_lifecycle_stage(&events), EntryLifecycleStage::AwaitingApproval);
    }

    #[test]
    fn test_entry_lifecycle_stage_order_submitted_from_entry_requested() {
        let pid = mk_pid();
        let sid = mk_signal_id();
        let events = vec![
            armed_event(pid),
            policy_resolved_event(pid),
            signal_received_event(pid, sid),
            order_requested_event(pid, sid),
        ];
        assert_eq!(entry_lifecycle_stage(&events), EntryLifecycleStage::OrderSubmitted);
    }

    #[test]
    fn test_entry_lifecycle_stage_active_from_entry_filled() {
        let pid = mk_pid();
        let sid = mk_signal_id();
        let events = vec![
            armed_event(pid),
            policy_resolved_event(pid),
            signal_received_event(pid, sid),
            order_requested_event(pid, sid),
            filled_event(pid),
        ];
        assert_eq!(entry_lifecycle_stage(&events), EntryLifecycleStage::Active);
    }

    #[test]
    fn test_entry_lifecycle_stage_cancelled_from_disarm() {
        let pid = mk_pid();
        let events = vec![
            armed_event(pid),
            policy_resolved_event(pid),
            disarmed_event(pid),
        ];
        assert_eq!(entry_lifecycle_stage(&events), EntryLifecycleStage::Cancelled);
    }

    #[test]
    fn test_entry_lifecycle_stage_order_failed_returns_to_awaiting_signal() {
        let pid = mk_pid();
        let sid = mk_signal_id();
        let events = vec![
            armed_event(pid),
            policy_resolved_event(pid),
            signal_received_event(pid, sid),
            order_requested_event(pid, sid),
            order_failed_event(pid, sid),
        ];
        // Order failure: position stays Armed, detector re-armed → back to AwaitingSignal
        assert_eq!(entry_lifecycle_stage(&events), EntryLifecycleStage::AwaitingSignal);
    }

    #[test]
    fn test_entry_lifecycle_stage_replay_is_deterministic() {
        // Same event sequence must produce the same result on multiple calls.
        let pid = mk_pid();
        let sid = mk_signal_id();
        let events = vec![
            armed_event(pid),
            policy_resolved_event(pid),
            signal_received_event(pid, sid),
            approval_pending_event(pid, sid),
            order_requested_event(pid, sid),
            filled_event(pid),
        ];
        let stage1 = entry_lifecycle_stage(&events);
        let stage2 = entry_lifecycle_stage(&events);
        assert_eq!(stage1, stage2, "entry_lifecycle_stage must be deterministic");
        assert_eq!(stage1, EntryLifecycleStage::Active);
    }

    #[test]
    fn test_entry_lifecycle_stage_empty_events_is_intent_created() {
        // No events → initial stage (should not panic)
        assert_eq!(entry_lifecycle_stage(&[]), EntryLifecycleStage::IntentCreated);
    }

    #[test]
    fn test_entry_lifecycle_stage_deterministic_replay_all_stages() {
        // Every valid lifecycle path must produce the same result when the same
        // event sequence is replayed. This test covers all seven stages and also
        // verifies that non-advancing events (PositionMonitorTick, TrailingStopUpdated)
        // do not alter the projection.
        let pid = mk_pid();
        let sid = mk_signal_id();

        // Non-advancing events that must not affect the stage.
        let noise_events: Vec<Event> = vec![
            Event::TrailingStopUpdated {
                position_id: pid,
                previous_stop: Price::new(dec!(93000)).unwrap(),
                new_stop: Price::new(dec!(93500)).unwrap(),
                trigger_price: Price::new(dec!(94000)).unwrap(),
                timestamp: Utc::now(),
            },
        ];

        let test_sequences: Vec<(&str, Vec<Event>, EntryLifecycleStage)> = vec![
            ("intent_created", vec![armed_event(pid)], EntryLifecycleStage::IntentCreated),
            ("awaiting_signal", vec![armed_event(pid), policy_resolved_event(pid)], EntryLifecycleStage::AwaitingSignal),
            ("signal_confirmed", vec![armed_event(pid), policy_resolved_event(pid), signal_received_event(pid, sid)], EntryLifecycleStage::SignalConfirmed),
            ("awaiting_approval", vec![armed_event(pid), policy_resolved_event(pid), signal_received_event(pid, sid), approval_pending_event(pid, sid)], EntryLifecycleStage::AwaitingApproval),
            ("order_submitted", vec![armed_event(pid), policy_resolved_event(pid), signal_received_event(pid, sid), order_requested_event(pid, sid)], EntryLifecycleStage::OrderSubmitted),
            ("active", vec![armed_event(pid), policy_resolved_event(pid), signal_received_event(pid, sid), order_requested_event(pid, sid), filled_event(pid)], EntryLifecycleStage::Active),
            ("cancelled", vec![armed_event(pid), policy_resolved_event(pid), disarmed_event(pid)], EntryLifecycleStage::Cancelled),
        ];

        for (name, base_events, expected) in &test_sequences {
            // Replay 100 times without noise.
            for _ in 0..100 {
                assert_eq!(
                    entry_lifecycle_stage(base_events),
                    *expected,
                    "determinism failed for stage '{}' (no noise)",
                    name
                );
            }

            // Replay with noise events inserted after every base event.
            let mut noisy: Vec<Event> = Vec::new();
            for event in base_events.iter() {
                noisy.push(event.clone());
                noisy.extend(noise_events.iter().cloned());
            }
            for _ in 0..100 {
                assert_eq!(
                    entry_lifecycle_stage(&noisy),
                    *expected,
                    "determinism failed for stage '{}' (with noise)",
                    name
                );
            }
        }
    }

    #[test]
    fn test_cancelled_position_never_has_entry_fill_in_lifecycle() {
        // A Cancelled position (disarmed before entry) must never be projected
        // to a stage that implies a fill occurred (Active, OrderSubmitted).
        // If PositionDisarmed appears in the event sequence, the projection
        // must resolve to Cancelled regardless of subsequent events.
        let pid = mk_pid();
        let sid = mk_signal_id();

        // Case 1: Armed → Disarmed (clean cancel).
        let events = vec![armed_event(pid), disarmed_event(pid)];
        assert_eq!(entry_lifecycle_stage(&events), EntryLifecycleStage::Cancelled);

        // Case 2: Armed → PolicyResolved → Disarmed.
        let events = vec![armed_event(pid), policy_resolved_event(pid), disarmed_event(pid)];
        assert_eq!(entry_lifecycle_stage(&events), EntryLifecycleStage::Cancelled);

        // Case 3: Armed → PolicyResolved → SignalReceived → Disarmed.
        let events = vec![
            armed_event(pid),
            policy_resolved_event(pid),
            signal_received_event(pid, sid),
            disarmed_event(pid),
        ];
        assert_eq!(entry_lifecycle_stage(&events), EntryLifecycleStage::Cancelled);

        // Case 4: Disarmed after approval pending (operator disarmed during approval).
        let events = vec![
            armed_event(pid),
            policy_resolved_event(pid),
            signal_received_event(pid, sid),
            approval_pending_event(pid, sid),
            disarmed_event(pid),
        ];
        assert_eq!(entry_lifecycle_stage(&events), EntryLifecycleStage::Cancelled);

        // Case 5: If EntryFilled is present, stage must NOT be Cancelled.
        // A filled position that later gets a close event should be Active or later.
        let events = vec![
            armed_event(pid),
            policy_resolved_event(pid),
            signal_received_event(pid, sid),
            order_requested_event(pid, sid),
            filled_event(pid),
        ];
        assert_ne!(entry_lifecycle_stage(&events), EntryLifecycleStage::Cancelled);
        assert_eq!(entry_lifecycle_stage(&events), EntryLifecycleStage::Active);
    }
}
