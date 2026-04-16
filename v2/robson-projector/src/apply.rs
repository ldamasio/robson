//! Event dispatcher for projections
//!
//! Routes events to their appropriate projection handlers.

use robson_eventlog::{EventEnvelope, QUERY_STATE_CHANGED_EVENT_TYPE};
use sqlx::PgPool;

use crate::{
    error::{ProjectionError, Result},
    handlers,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProjectionRoute {
    OrderSubmitted,
    OrderAcked,
    OrderRejected,
    OrderCanceled,
    FillReceived,
    PositionOpened,
    PositionClosedLegacy,
    EntryOrderPlaced,
    EntryFilled,
    EntrySignalReceived,
    TrailingStopUpdated,
    PositionMonitorTick,
    ExitTriggered,
    ExitOrderPlaced,
    BalanceSampled,
    RiskCheckFailed,
    StrategyEnabled,
    StrategyDisabled,
    QueryStateChanged,
    PositionArmed,
    PositionDisarmed,
    ExitFilled,
    PositionClosedDomain,
}

fn projection_route(event_type: &str) -> Option<ProjectionRoute> {
    match event_type {
        // Order events
        "ORDER_SUBMITTED" => Some(ProjectionRoute::OrderSubmitted),
        "ORDER_ACKED" => Some(ProjectionRoute::OrderAcked),
        "ORDER_REJECTED" => Some(ProjectionRoute::OrderRejected),
        "ORDER_CANCELED" => Some(ProjectionRoute::OrderCanceled),

        // Fill events
        "FILL_RECEIVED" => Some(ProjectionRoute::FillReceived),

        // Position events
        "POSITION_OPENED" => Some(ProjectionRoute::PositionOpened),
        "POSITION_CLOSED" => Some(ProjectionRoute::PositionClosedLegacy),
        "entry_order_placed" | "ENTRY_ORDER_PLACED" => Some(ProjectionRoute::EntryOrderPlaced),
        "entry_filled" | "ENTRY_FILLED" => Some(ProjectionRoute::EntryFilled),
        "entry_signal_received" | "ENTRY_SIGNAL_RECEIVED" => {
            Some(ProjectionRoute::EntrySignalReceived)
        },
        "trailing_stop_updated" | "TRAILING_STOP_UPDATED" => {
            Some(ProjectionRoute::TrailingStopUpdated)
        },
        "position_monitor_tick" | "POSITION_MONITOR_TICK" => {
            Some(ProjectionRoute::PositionMonitorTick)
        },
        "exit_triggered" | "EXIT_TRIGGERED" => Some(ProjectionRoute::ExitTriggered),
        "exit_order_placed" | "EXIT_ORDER_PLACED" => Some(ProjectionRoute::ExitOrderPlaced),

        // Balance events
        "BALANCE_SAMPLED" => Some(ProjectionRoute::BalanceSampled),

        // Risk events
        "RISK_CHECK_FAILED" => Some(ProjectionRoute::RiskCheckFailed),

        // Strategy events
        "STRATEGY_ENABLED" => Some(ProjectionRoute::StrategyEnabled),
        "STRATEGY_DISABLED" => Some(ProjectionRoute::StrategyDisabled),

        // Query audit events
        QUERY_STATE_CHANGED_EVENT_TYPE => Some(ProjectionRoute::QueryStateChanged),

        // Domain position lifecycle events (snake_case, emitted by robsond via executor)
        "position_armed" => Some(ProjectionRoute::PositionArmed),
        "position_disarmed" => Some(ProjectionRoute::PositionDisarmed),
        "exit_filled" | "EXIT_FILLED" => Some(ProjectionRoute::ExitFilled),
        "position_closed" => Some(ProjectionRoute::PositionClosedDomain),

        _ => None,
    }
}

/// Apply a single event to all relevant projection tables.
///
/// This is idempotent and safe for replay - handlers use UPSERT
/// and check sequence numbers to prevent double-application.
pub async fn apply_event_to_projections(pool: &PgPool, envelope: &EventEnvelope) -> Result<()> {
    match projection_route(envelope.event_type.as_str()) {
        Some(ProjectionRoute::OrderSubmitted) => {
            handlers::orders::handle_order_submitted(pool, envelope).await?
        },
        Some(ProjectionRoute::OrderAcked) => {
            handlers::orders::handle_order_acked(pool, envelope).await?
        },
        Some(ProjectionRoute::OrderRejected) => {
            handlers::orders::handle_order_rejected(pool, envelope).await?
        },
        Some(ProjectionRoute::OrderCanceled) => {
            handlers::orders::handle_order_canceled(pool, envelope).await?
        },
        Some(ProjectionRoute::FillReceived) => {
            handlers::orders::handle_fill_received(pool, envelope).await?
        },
        Some(ProjectionRoute::PositionOpened) => {
            handlers::positions::handle_position_opened(pool, envelope).await?
        },
        Some(ProjectionRoute::PositionClosedLegacy) => {
            handlers::positions::handle_position_closed(pool, envelope).await?
        },
        Some(ProjectionRoute::EntryOrderPlaced) => {
            handlers::positions::handle_entry_order_placed(pool, envelope).await?
        },
        Some(ProjectionRoute::EntryFilled) => {
            handlers::positions::handle_entry_filled(pool, envelope).await?
        },
        Some(ProjectionRoute::EntrySignalReceived) => {
            handlers::positions::handle_entry_signal_received(pool, envelope).await?
        },
        Some(ProjectionRoute::TrailingStopUpdated) => {
            handlers::positions::handle_trailing_stop_updated(pool, envelope).await?
        },
        Some(ProjectionRoute::PositionMonitorTick) => {
            // Audit-only tick evidence. The canonical position projection is
            // updated by lifecycle events such as trailing_stop_updated.
        },
        Some(ProjectionRoute::ExitTriggered) => {
            handlers::positions::handle_exit_triggered(pool, envelope).await?
        },
        Some(ProjectionRoute::ExitOrderPlaced) => {
            handlers::positions::handle_exit_order_placed(pool, envelope).await?
        },
        Some(ProjectionRoute::BalanceSampled) => {
            handlers::balances::handle_balance_sampled(pool, envelope).await?
        },
        Some(ProjectionRoute::RiskCheckFailed) => {
            handlers::risk::handle_risk_check_failed(pool, envelope).await?
        },
        Some(ProjectionRoute::StrategyEnabled) => {
            handlers::strategy::handle_strategy_enabled(pool, envelope).await?
        },
        Some(ProjectionRoute::StrategyDisabled) => {
            handlers::strategy::handle_strategy_disabled(pool, envelope).await?
        },
        Some(ProjectionRoute::QueryStateChanged) => {
            handlers::queries::handle_query_state_changed(pool, envelope).await?
        },
        Some(ProjectionRoute::PositionArmed) => {
            handlers::positions::handle_position_armed(pool, envelope).await?
        },
        Some(ProjectionRoute::PositionDisarmed) => {
            handlers::positions::handle_position_disarmed(pool, envelope).await?
        },
        Some(ProjectionRoute::ExitFilled) => {
            handlers::positions::handle_exit_filled(pool, envelope).await?
        },
        Some(ProjectionRoute::PositionClosedDomain) => {
            handlers::positions::handle_position_closed_domain(pool, envelope).await?
        },
        None => {
            // Unknown event types are a configuration error - they indicate the projector
            // is missing a handler for an event type that was persisted to the eventlog.
            // Return an error so the caller can decide how to handle (retry, alert, etc).
            // The checkpoint should NOT advance for unhandled events.
            return Err(ProjectionError::MissingHandler {
                event_type: envelope.event_type.clone(),
                seq: envelope.seq,
                stream_key: envelope.stream_key.clone(),
            });
        },
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::projection_route;

    #[test]
    fn runtime_emitted_domain_events_have_projection_routes() {
        // MIG-v2.5#2 freeze test:
        // these are the robson-domain::Event variants actually emitted by the
        // current runtime path (robson-engine, robsond, robson-exec).
        let runtime_event_types = [
            "position_armed",
            "position_disarmed",
            "entry_signal_received",
            "entry_order_placed",
            "entry_filled",
            "trailing_stop_updated",
            "position_monitor_tick",
            "exit_triggered",
            "exit_order_placed",
            "position_closed",
        ];

        for event_type in runtime_event_types {
            assert!(
                projection_route(event_type).is_some(),
                "runtime-emitted event type must have a projector route: {event_type}"
            );
        }
    }
}
