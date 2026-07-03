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
    EntryOrderRequested,
    EntryOrderAccepted,
    EntryOrderFailed,
    EntryExecutionRejected,
    EntryFilled,
    TechnicalStopAnalyzed,
    EntrySignalReceived,
    MonthBoundaryReset,
    CapitalBaseRecalibrated,
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
    EntryPolicyResolved,
    PositionDisarmed,
    ExitFilled,
    PositionClosedDomain,
    // ADR-0039 insurance-stop events are audit-only in the postgres projection
    // (the live order id is reconciled in mission 2). Routes exist so the
    // projector does not reject them as unhandled.
    InsuranceStopPlaced,
    InsuranceStopReplaced,
    InsuranceStopCancelled,
    InsuranceStopFailed,
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
        "entry_order_requested" => Some(ProjectionRoute::EntryOrderRequested),
        "entry_order_accepted" => Some(ProjectionRoute::EntryOrderAccepted),
        "entry_order_failed" => Some(ProjectionRoute::EntryOrderFailed),
        "entry_execution_rejected" => Some(ProjectionRoute::EntryExecutionRejected),
        "entry_filled" | "ENTRY_FILLED" => Some(ProjectionRoute::EntryFilled),
        "technical_stop_analyzed" => Some(ProjectionRoute::TechnicalStopAnalyzed),
        "entry_signal_received" | "ENTRY_SIGNAL_RECEIVED" => {
            Some(ProjectionRoute::EntrySignalReceived)
        },
        "month_boundary_reset" => Some(ProjectionRoute::MonthBoundaryReset),
        "capital_base_recalibrated" => Some(ProjectionRoute::CapitalBaseRecalibrated),
        "trailing_stop_updated" | "TRAILING_STOP_UPDATED" => {
            Some(ProjectionRoute::TrailingStopUpdated)
        },
        "position_monitor_tick" | "POSITION_MONITOR_TICK" => {
            Some(ProjectionRoute::PositionMonitorTick)
        },
        "exit_triggered" | "EXIT_TRIGGERED" => Some(ProjectionRoute::ExitTriggered),
        "exit_order_placed" | "EXIT_ORDER_PLACED" => Some(ProjectionRoute::ExitOrderPlaced),

        // ADR-0039 insurance-stop events (audit-only in the projection).
        "insurance_stop_placed" => Some(ProjectionRoute::InsuranceStopPlaced),
        "insurance_stop_replaced" => Some(ProjectionRoute::InsuranceStopReplaced),
        "insurance_stop_cancelled" => Some(ProjectionRoute::InsuranceStopCancelled),
        "insurance_stop_failed" => Some(ProjectionRoute::InsuranceStopFailed),

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
        "entry_policy_resolved" => Some(ProjectionRoute::EntryPolicyResolved),
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
        Some(ProjectionRoute::EntryOrderRequested) => {
            handlers::positions::handle_entry_order_requested(pool, envelope).await?
        },
        Some(ProjectionRoute::EntryOrderAccepted) => {
            handlers::positions::handle_entry_order_accepted(pool, envelope).await?
        },
        Some(ProjectionRoute::EntryOrderFailed) => {
            handlers::positions::handle_entry_order_failed(pool, envelope).await?
        },
        Some(ProjectionRoute::EntryExecutionRejected) => {
            handlers::positions::handle_entry_execution_rejected(pool, envelope).await?
        },
        Some(ProjectionRoute::EntryFilled) => {
            handlers::positions::handle_entry_filled(pool, envelope).await?;
            handlers::monthly_state::handle_entry_filled_monthly(pool, envelope).await?;
        },
        Some(ProjectionRoute::TechnicalStopAnalyzed) => {
            // Audit-only event. The full analysis payload is recoverable from
            // event_log and does not mutate positions_current.
        },
        Some(ProjectionRoute::EntrySignalReceived) => {
            handlers::positions::handle_entry_signal_received(pool, envelope).await?
        },
        Some(ProjectionRoute::MonthBoundaryReset) => {
            handlers::monthly_state::handle_month_boundary_reset(pool, envelope).await?
        },
        Some(ProjectionRoute::CapitalBaseRecalibrated) => {
            handlers::monthly_state::handle_capital_base_recalibrated(pool, envelope).await?
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
        Some(ProjectionRoute::InsuranceStopPlaced)
        | Some(ProjectionRoute::InsuranceStopReplaced) => {
            // Persist the live protective-order linkage so restarts hydrate
            // it (ADR-0039; 2026-07-03 incident: a lost linkage orphaned the
            // filled stop and stranded its reconciled close).
            handlers::positions::handle_insurance_stop_linked(pool, envelope).await?
        },
        Some(ProjectionRoute::InsuranceStopCancelled) => {
            handlers::positions::handle_insurance_stop_cleared(pool, envelope).await?
        },
        Some(ProjectionRoute::InsuranceStopFailed) => {
            // Audit-only: a failed placement leaves no live order to link.
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
        Some(ProjectionRoute::EntryPolicyResolved) => {
            handlers::positions::handle_entry_policy_resolved(pool, envelope).await?
        },
        Some(ProjectionRoute::PositionDisarmed) => {
            handlers::positions::handle_position_disarmed(pool, envelope).await?
        },
        Some(ProjectionRoute::ExitFilled) => {
            handlers::positions::handle_exit_filled(pool, envelope).await?
        },
        Some(ProjectionRoute::PositionClosedDomain) => {
            handlers::positions::handle_position_closed_domain(pool, envelope).await?;
            handlers::monthly_state::handle_position_closed_monthly(pool, envelope).await?;
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
            "entry_policy_resolved",
            "position_disarmed",
            "technical_stop_analyzed",
            "entry_signal_received",
            "month_boundary_reset",
            "entry_order_placed", // legacy
            "entry_order_requested",
            "entry_order_accepted",
            "entry_order_failed",
            "entry_execution_rejected",
            "entry_filled",
            "trailing_stop_updated",
            "position_monitor_tick",
            "exit_triggered",
            "exit_order_placed",
            "position_closed",
            // ADR-0039 insurance-stop lifecycle events.
            "insurance_stop_placed",
            "insurance_stop_replaced",
            "insurance_stop_cancelled",
            "insurance_stop_failed",
        ];

        for event_type in runtime_event_types {
            assert!(
                projection_route(event_type).is_some(),
                "runtime-emitted event type must have a projector route: {event_type}"
            );
        }
    }
}
