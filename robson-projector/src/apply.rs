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
    FavorableExtremeAdvanced,
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
    StartupRecoveryInsuranceStopChecked,
    ExecutableStopPlanDriftDetected,
    EntryFillProtectionFallback,
    ExecutableStopRiskResolutionFailed,
    ResidualInventoryRecorded,
    // Domain audit events with no projection of their own (ADR-0056 step 7):
    // the event log is what a restart reads them from.
    EntryAttemptExhausted,
    SignalStrategyEvaluated,
    EntryApprovalPending,
    PositionError,
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
        "favorable_extreme_advanced" => Some(ProjectionRoute::FavorableExtremeAdvanced),
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
        "startup_recovery_insurance_stop_checked" => {
            Some(ProjectionRoute::StartupRecoveryInsuranceStopChecked)
        },
        "executable_stop_plan_drift_detected" => {
            Some(ProjectionRoute::ExecutableStopPlanDriftDetected)
        },
        "entry_fill_protection_fallback" => Some(ProjectionRoute::EntryFillProtectionFallback),
        "executable_stop_risk_resolution_failed" => {
            Some(ProjectionRoute::ExecutableStopRiskResolutionFailed)
        },
        "residual_inventory_recorded" => Some(ProjectionRoute::ResidualInventoryRecorded),
        "entry_attempt_exhausted" => Some(ProjectionRoute::EntryAttemptExhausted),
        "signal_strategy_evaluated" => Some(ProjectionRoute::SignalStrategyEvaluated),
        "entry_approval_pending" => Some(ProjectionRoute::EntryApprovalPending),
        "position_error" => Some(ProjectionRoute::PositionError),

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

/// Whether an event of this type, on a position stream, applies through a
/// handler guarded by the leg's `last_seq`, or is audit-only: what the startup
/// repair of a suspended position stream may apply again (ADR-0056 step 7, PR
/// P). Every route says, so a new one cannot be repairable by default. The
/// monthly counters `entry_filled` and `position_closed` also add to are not
/// guarded by `last_seq`.
pub fn is_repairable_position_event(event_type: &str) -> bool {
    use ProjectionRoute::*;
    let Some(route) = projection_route(event_type) else {
        return false;
    };
    match route {
        PositionArmed
        | EntryPolicyResolved
        | PositionDisarmed
        | EntrySignalReceived
        | TechnicalStopAnalyzed
        | EntryOrderPlaced
        | EntryOrderRequested
        | EntryOrderAccepted
        | EntryOrderFailed
        | EntryExecutionRejected
        | EntryFilled
        | TrailingStopUpdated
        | FavorableExtremeAdvanced
        | PositionMonitorTick
        | ExitTriggered
        | ExitOrderPlaced
        | ExitFilled
        | PositionClosedDomain
        | InsuranceStopPlaced
        | InsuranceStopReplaced
        | InsuranceStopCancelled
        | InsuranceStopFailed
        | StartupRecoveryInsuranceStopChecked
        | ExecutableStopPlanDriftDetected
        | EntryFillProtectionFallback
        | ExecutableStopRiskResolutionFailed
        | ResidualInventoryRecorded
        | EntryAttemptExhausted
        | SignalStrategyEvaluated
        | EntryApprovalPending
        | PositionError => true,
        // System, legacy and other streams' types: never on a position stream
        // the daemon wrote.
        MonthBoundaryReset
        | CapitalBaseRecalibrated
        | OrderSubmitted
        | OrderAcked
        | OrderRejected
        | OrderCanceled
        | FillReceived
        | PositionOpened
        | PositionClosedLegacy
        | BalanceSampled
        | RiskCheckFailed
        | StrategyEnabled
        | StrategyDisabled
        | QueryStateChanged => false,
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
        Some(ProjectionRoute::FavorableExtremeAdvanced) => {
            handlers::positions::handle_favorable_extreme_advanced(pool, envelope).await?
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
        Some(ProjectionRoute::InsuranceStopFailed)
        | Some(ProjectionRoute::StartupRecoveryInsuranceStopChecked)
        | Some(ProjectionRoute::ExecutableStopPlanDriftDetected)
        | Some(ProjectionRoute::EntryFillProtectionFallback)
        | Some(ProjectionRoute::ExecutableStopRiskResolutionFailed)
        | Some(ProjectionRoute::ResidualInventoryRecorded) => {
            // Audit-only: no position projection mutation.
        },
        Some(ProjectionRoute::EntryAttemptExhausted)
        | Some(ProjectionRoute::SignalStrategyEvaluated)
        | Some(ProjectionRoute::EntryApprovalPending)
        | Some(ProjectionRoute::PositionError) => {
            // Audit-only. Until these routes existed every one of them landed
            // in the event log and then failed here as unhandled. A
            // restart reads `entry_attempt_exhausted` back from the
            // event log, not from a projection.
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
    use super::{projection_route, ProjectionRoute};

    /// Every type a `robson_domain::Event` is persisted under, read from the
    /// enum itself: serde names every variant when it meets an unknown tag, so
    /// a variant added later is checked here with no change to this test.
    fn every_domain_event_type() -> Vec<String> {
        let unknown = serde_json::json!({ "type": "no_such_event_type" });
        let message = serde_json::from_value::<robson_domain::Event>(unknown)
            .expect_err("an unknown tag is refused")
            .to_string();
        let (_, expected) = message
            .split_once("expected one of")
            .unwrap_or_else(|| panic!("serde no longer lists the variants: {message}"));
        expected.split('`').skip(1).step_by(2).map(str::to_string).collect()
    }

    /// MIG-v2.5#2 freeze test, over every domain event (ADR-0056 step 7): an
    /// event with no route lands in the event log and then fails its apply, and
    /// from PR P a failed apply suspends its stream.
    #[test]
    fn every_domain_event_type_has_a_projection_route() {
        let event_types = every_domain_event_type();
        for listed in [
            "position_armed",
            "entry_filled",
            "position_closed",
            "month_boundary_reset",
        ] {
            assert!(event_types.iter().any(|t| t == listed), "{listed} not in {event_types:?}");
        }
        for event_type in &event_types {
            assert!(
                projection_route(event_type).is_some(),
                "domain event type must have a projector route: {event_type}"
            );
        }
    }

    /// What a daemon writes on a position stream is repairable; a system
    /// stream's types, legacy types and unknown ones are not.
    #[test]
    fn every_position_stream_event_type_is_repairable_and_nothing_else() {
        for event_type in every_domain_event_type() {
            let system =
                matches!(event_type.as_str(), "month_boundary_reset" | "capital_base_recalibrated");
            assert_eq!(super::is_repairable_position_event(&event_type), !system, "{event_type}");
        }
        for other in [
            "POSITION_OPENED",
            "ORDER_SUBMITTED",
            "BALANCE_SAMPLED",
            "no_such_type",
        ] {
            assert!(!super::is_repairable_position_event(other), "{other}");
        }
    }

    #[test]
    fn the_unprojected_domain_audit_events_have_their_own_routes() {
        for (event_type, route) in [
            ("entry_attempt_exhausted", ProjectionRoute::EntryAttemptExhausted),
            ("signal_strategy_evaluated", ProjectionRoute::SignalStrategyEvaluated),
            ("entry_approval_pending", ProjectionRoute::EntryApprovalPending),
            ("position_error", ProjectionRoute::PositionError),
        ] {
            assert_eq!(projection_route(event_type), Some(route));
        }
    }
}
