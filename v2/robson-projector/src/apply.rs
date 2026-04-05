//! Event dispatcher for projections
//!
//! Routes events to their appropriate projection handlers.

use crate::error::{ProjectionError, Result};
use crate::handlers;
use robson_eventlog::EventEnvelope;
use robson_eventlog::QUERY_STATE_CHANGED_EVENT_TYPE;
use sqlx::PgPool;

/// Apply a single event to all relevant projection tables.
///
/// This is idempotent and safe for replay - handlers use UPSERT
/// and check sequence numbers to prevent double-application.
pub async fn apply_event_to_projections(pool: &PgPool, envelope: &EventEnvelope) -> Result<()> {
    match envelope.event_type.as_str() {
        // Order events
        "ORDER_SUBMITTED" => handlers::orders::handle_order_submitted(pool, envelope).await?,
        "ORDER_ACKED" => handlers::orders::handle_order_acked(pool, envelope).await?,
        "ORDER_REJECTED" => handlers::orders::handle_order_rejected(pool, envelope).await?,
        "ORDER_CANCELED" => handlers::orders::handle_order_canceled(pool, envelope).await?,

        // Fill events
        "FILL_RECEIVED" => handlers::orders::handle_fill_received(pool, envelope).await?,

        // Position events
        "POSITION_OPENED" => handlers::positions::handle_position_opened(pool, envelope).await?,
        "POSITION_CLOSED" => handlers::positions::handle_position_closed(pool, envelope).await?,
        "entry_order_placed" | "ENTRY_ORDER_PLACED" => {
            handlers::positions::handle_entry_order_placed(pool, envelope).await?
        },
        "entry_filled" | "ENTRY_FILLED" => {
            handlers::positions::handle_entry_filled(pool, envelope).await?
        },
        "entry_signal_received" | "ENTRY_SIGNAL_RECEIVED" => {
            handlers::positions::handle_entry_signal_received(pool, envelope).await?
        },
        "trailing_stop_updated" | "TRAILING_STOP_UPDATED" => {
            handlers::positions::handle_trailing_stop_updated(pool, envelope).await?
        },
        "exit_triggered" | "EXIT_TRIGGERED" => {
            handlers::positions::handle_exit_triggered(pool, envelope).await?
        },
        "exit_order_placed" | "EXIT_ORDER_PLACED" => {
            handlers::positions::handle_exit_order_placed(pool, envelope).await?
        },

        // Balance events
        "BALANCE_SAMPLED" => handlers::balances::handle_balance_sampled(pool, envelope).await?,

        // Risk events
        "RISK_CHECK_FAILED" => handlers::risk::handle_risk_check_failed(pool, envelope).await?,

        // Strategy events
        "STRATEGY_ENABLED" => handlers::strategy::handle_strategy_enabled(pool, envelope).await?,
        "STRATEGY_DISABLED" => handlers::strategy::handle_strategy_disabled(pool, envelope).await?,

        // Query audit events
        QUERY_STATE_CHANGED_EVENT_TYPE => {
            handlers::queries::handle_query_state_changed(pool, envelope).await?
        },

        // Domain position lifecycle events (snake_case, emitted by robsond via executor)
        "position_armed" => {
            handlers::positions::handle_position_armed(pool, envelope).await?
        },
        "position_disarmed" => {
            handlers::positions::handle_position_disarmed(pool, envelope).await?
        },
        "exit_filled" | "EXIT_FILLED" => {
            handlers::positions::handle_exit_filled(pool, envelope).await?
        },
        // Lowercase position_closed: domain event with P&L fields
        "position_closed" => {
            handlers::positions::handle_position_closed_domain(pool, envelope).await?
        },

        _ => {
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
