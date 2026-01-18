//! Event dispatcher for projections
//!
//! Routes events to their appropriate projection handlers.

use crate::error::{ProjectionError, Result};
use crate::handlers;
use robson_eventlog::EventEnvelope;
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
        "entry_filled" | "ENTRY_FILLED" => {
            handlers::positions::handle_entry_filled(pool, envelope).await?
        },
        "trailing_stop_updated" | "TRAILING_STOP_UPDATED" => {
            handlers::positions::handle_trailing_stop_updated(pool, envelope).await?
        },
        "exit_triggered" | "EXIT_TRIGGERED" => {
            handlers::positions::handle_exit_triggered(pool, envelope).await?
        },

        // Balance events
        "BALANCE_SAMPLED" => handlers::balances::handle_balance_sampled(pool, envelope).await?,

        // Risk events
        "RISK_CHECK_FAILED" => handlers::risk::handle_risk_check_failed(pool, envelope).await?,

        // Strategy events
        "STRATEGY_ENABLED" => handlers::strategy::handle_strategy_enabled(pool, envelope).await?,
        "STRATEGY_DISABLED" => handlers::strategy::handle_strategy_disabled(pool, envelope).await?,

        _ => {
            tracing::warn!("Unknown event type: {}", envelope.event_type);
            return Err(ProjectionError::UnknownEventType(envelope.event_type.clone()));
        },
    }

    Ok(())
}
