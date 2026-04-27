//! Public SSE contract for operator-facing runtime events.
//!
//! This module maps internal `DaemonEvent` values into a narrow, stable stream
//! contract for UI/monitoring consumers. The stream is a projection only:
//! REST remains responsible for bootstrap/snapshots, and runtime state remains
//! the operational source of truth.

use axum::response::sse::Event;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::event_bus::DaemonEvent;

/// Current SSE schema version for public operator events.
pub(crate) const SSE_SCHEMA_VERSION: u8 = 1;

/// Public event envelope sent over the SSE stream.
///
/// `occurred_at` is the projection timestamp for this public SSE envelope.
/// It is intentionally not a replay cursor and not a guarantee of durable
/// ordering across reconnects. `event_id` exists for uniqueness and client-side
/// deduplication only in v2.5.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct PublicSseEvent {
    pub schema_version: u8,
    pub event_id: Uuid,
    pub event_type: String,
    pub occurred_at: DateTime<Utc>,
    pub payload: Value,
}

impl PublicSseEvent {
    fn new(event_type: impl Into<String>, payload: Value) -> Self {
        Self {
            schema_version: SSE_SCHEMA_VERSION,
            event_id: Uuid::now_v7(),
            event_type: event_type.into(),
            occurred_at: Utc::now(),
            payload,
        }
    }

    /// Convert the public envelope into a native Axum SSE frame.
    pub(crate) fn into_sse_event(self) -> Event {
        Event::default()
            .id(self.event_id.to_string())
            .event(self.event_type.clone())
            .data(serde_json::to_string(&self).expect("public SSE event must serialize"))
    }
}

/// Map an internal daemon event into the public SSE contract.
///
/// Returns `None` for internal-only events that are intentionally not exposed
/// to operator clients in the v2.5 SSE stream.
pub(crate) fn map_daemon_event(event: &DaemonEvent) -> Option<PublicSseEvent> {
    match event {
        DaemonEvent::PositionStateChanged {
            position_id,
            previous_state,
            new_state,
            timestamp,
        } => Some(PublicSseEvent::new(
            "position.changed",
            json!({
                "position_id": position_id,
                "previous_state": previous_state,
                "new_state": new_state,
                "source_occurred_at": timestamp,
            }),
        )),
        DaemonEvent::QueryAwaitingApproval {
            query_id,
            position_id,
            reason,
            expires_at,
        } => Some(PublicSseEvent::new(
            "query.awaiting_approval",
            json!({
                "query_id": query_id,
                "position_id": position_id,
                "reason": reason,
                "expires_at": expires_at,
            }),
        )),
        DaemonEvent::QueryAuthorized { query_id, position_id, approved_at } => {
            Some(PublicSseEvent::new(
                "query.authorized",
                json!({
                    "query_id": query_id,
                    "position_id": position_id,
                    "approved_at": approved_at,
                }),
            ))
        },
        DaemonEvent::QueryExpired { query_id, position_id, expired_at } => {
            Some(PublicSseEvent::new(
                "query.expired",
                json!({
                    "query_id": query_id,
                    "position_id": position_id,
                    "expired_at": expired_at,
                }),
            ))
        },
        DaemonEvent::CorePositionOpened { position_id, symbol, side, .. } => {
            Some(PublicSseEvent::new(
                "position.opened",
                json!({
                    "position_id": position_id,
                    "symbol": symbol.as_pair(),
                    "side": side.to_string().to_lowercase(),
                }),
            ))
        },
        DaemonEvent::CorePositionClosed { position_id, symbol, side } => Some(PublicSseEvent::new(
            "position.closed",
            json!({
                "position_id": position_id,
                "symbol": symbol.as_pair(),
                "side": side.to_string().to_lowercase(),
            }),
        )),
        DaemonEvent::RoguePositionDetected { symbol, side, entry_price, stop_price } => {
            Some(PublicSseEvent::new(
                "safety.rogue_position_detected",
                json!({
                    "symbol": symbol,
                    "side": side.to_string().to_lowercase(),
                    "entry_price": entry_price.as_decimal(),
                    "stop_price": stop_price.as_decimal(),
                }),
            ))
        },
        DaemonEvent::SafetyExitExecuted { symbol, order_id, executed_quantity } => {
            Some(PublicSseEvent::new(
                "safety.exit_executed",
                json!({
                    "symbol": symbol,
                    "order_id": order_id,
                    "executed_quantity": executed_quantity,
                }),
            ))
        },
        DaemonEvent::SafetyExitFailed { symbol, error } => Some(PublicSseEvent::new(
            "safety.exit_failed",
            json!({
                "symbol": symbol,
                "error": error,
            }),
        )),
        DaemonEvent::SafetyPanic {
            position_id,
            symbol,
            side,
            error,
            consecutive_failures,
        } => Some(PublicSseEvent::new(
            "safety.panic",
            json!({
                "position_id": position_id,
                "symbol": symbol,
                "side": side.to_string().to_lowercase(),
                "error": error,
                "consecutive_failures": consecutive_failures,
            }),
        )),
        DaemonEvent::MonthlyHaltTriggered { reason, triggered_at } => Some(PublicSseEvent::new(
            "monthly_halt.triggered",
            json!({
                "reason": reason,
                "triggered_at": triggered_at,
            }),
        )),
        DaemonEvent::MonthlyHaltReset {} => {
            Some(PublicSseEvent::new("monthly_halt.reset", json!({})))
        },
        DaemonEvent::DetectorSignal(_)
        | DaemonEvent::DomainEvent(_)
        | DaemonEvent::MarketData(_)
        | DaemonEvent::OrderFill(_)
        | DaemonEvent::Shutdown => None,
    }
}

/// Emit a public signal that the consumer must re-bootstrap via REST because
/// the SSE stream has lost continuity.
pub(crate) fn resync_required_event(
    reason: impl Into<String>,
    message: impl Into<String>,
) -> PublicSseEvent {
    PublicSseEvent::new(
        "system.resync_required",
        json!({
            "reason": reason.into(),
            "message": message.into(),
        }),
    )
}

#[cfg(test)]
mod tests {
    use robson_domain::{DetectorSignal, Price, Quantity, Side, Symbol};
    use rust_decimal_macros::dec;

    use super::*;
    use crate::event_bus::{MarketData, OrderFill};

    fn test_symbol() -> Symbol {
        Symbol::from_pair("BTCUSDT").unwrap()
    }

    fn test_position_changed() -> DaemonEvent {
        DaemonEvent::PositionStateChanged {
            position_id: Uuid::now_v7(),
            previous_state: "armed".to_string(),
            new_state: "active".to_string(),
            timestamp: Utc::now(),
        }
    }

    fn test_detector_signal() -> DaemonEvent {
        DaemonEvent::DetectorSignal(DetectorSignal {
            signal_id: Uuid::now_v7(),
            position_id: Uuid::now_v7(),
            symbol: test_symbol(),
            side: Side::Long,
            entry_price: Price::new(dec!(95000)).unwrap(),
            stop_loss: Price::new(dec!(87400)).unwrap(),
            technical_stop_analysis: None,
            timestamp: Utc::now(),
        })
    }

    fn test_market_data() -> DaemonEvent {
        DaemonEvent::MarketData(MarketData {
            symbol: test_symbol(),
            price: Price::new(dec!(95000)).unwrap(),
            timestamp: Utc::now(),
        })
    }

    fn test_order_fill() -> DaemonEvent {
        DaemonEvent::OrderFill(OrderFill {
            position_id: Uuid::now_v7(),
            order_id: Uuid::now_v7(),
            fill_price: Price::new(dec!(95000)).unwrap(),
            filled_quantity: Quantity::new(dec!(0.01)).unwrap(),
            fee: dec!(1.25),
            filled_at: Utc::now(),
        })
    }

    #[test]
    fn test_public_event_mapping_types() {
        let position_changed = map_daemon_event(&test_position_changed()).unwrap();
        assert_eq!(position_changed.event_type, "position.changed");

        let position_opened = map_daemon_event(&DaemonEvent::CorePositionOpened {
            position_id: Uuid::now_v7(),
            symbol: test_symbol(),
            side: Side::Long,
            binance_position_id: "binance-123".to_string(),
        })
        .unwrap();
        assert_eq!(position_opened.event_type, "position.opened");

        let query_awaiting = map_daemon_event(&DaemonEvent::QueryAwaitingApproval {
            query_id: Uuid::now_v7(),
            position_id: Some(Uuid::now_v7()),
            reason: "Manual confirmation required".to_string(),
            expires_at: Utc::now(),
        })
        .unwrap();
        assert_eq!(query_awaiting.event_type, "query.awaiting_approval");

        let query_authorized = map_daemon_event(&DaemonEvent::QueryAuthorized {
            query_id: Uuid::now_v7(),
            position_id: Some(Uuid::now_v7()),
            approved_at: Utc::now(),
        })
        .unwrap();
        assert_eq!(query_authorized.event_type, "query.authorized");

        let query_expired = map_daemon_event(&DaemonEvent::QueryExpired {
            query_id: Uuid::now_v7(),
            position_id: Some(Uuid::now_v7()),
            expired_at: Utc::now(),
        })
        .unwrap();
        assert_eq!(query_expired.event_type, "query.expired");

        let position_closed = map_daemon_event(&DaemonEvent::CorePositionClosed {
            position_id: Uuid::now_v7(),
            symbol: test_symbol(),
            side: Side::Short,
        })
        .unwrap();
        assert_eq!(position_closed.event_type, "position.closed");

        let rogue_detected = map_daemon_event(&DaemonEvent::RoguePositionDetected {
            symbol: "BTCUSDT".to_string(),
            side: Side::Long,
            entry_price: Price::new(dec!(95000)).unwrap(),
            stop_price: Price::new(dec!(93000)).unwrap(),
        })
        .unwrap();
        assert_eq!(rogue_detected.event_type, "safety.rogue_position_detected");

        let safety_exit_executed = map_daemon_event(&DaemonEvent::SafetyExitExecuted {
            symbol: "BTCUSDT".to_string(),
            order_id: "order-123".to_string(),
            executed_quantity: dec!(0.01),
        })
        .unwrap();
        assert_eq!(safety_exit_executed.event_type, "safety.exit_executed");

        let safety_exit_failed = map_daemon_event(&DaemonEvent::SafetyExitFailed {
            symbol: "BTCUSDT".to_string(),
            error: "exchange unavailable".to_string(),
        })
        .unwrap();
        assert_eq!(safety_exit_failed.event_type, "safety.exit_failed");

        let safety_panic = map_daemon_event(&DaemonEvent::SafetyPanic {
            position_id: "BTCUSDT:long".to_string(),
            symbol: "BTCUSDT".to_string(),
            side: Side::Long,
            error: "all retries exhausted".to_string(),
            consecutive_failures: 5,
        })
        .unwrap();
        assert_eq!(safety_panic.event_type, "safety.panic");
    }

    #[test]
    fn test_internal_only_events_are_not_mapped() {
        assert!(map_daemon_event(&test_detector_signal()).is_none());
        assert!(map_daemon_event(&test_market_data()).is_none());
        assert!(map_daemon_event(&test_order_fill()).is_none());
        assert!(map_daemon_event(&DaemonEvent::Shutdown).is_none());
    }

    #[test]
    fn test_public_event_envelope_serialization_contains_required_fields() {
        let event = map_daemon_event(&test_position_changed()).unwrap();
        let serialized = serde_json::to_value(&event).unwrap();

        assert_eq!(serialized["schema_version"], SSE_SCHEMA_VERSION);
        assert!(serialized["event_id"].as_str().is_some());
        assert_eq!(serialized["event_type"], "position.changed");
        assert!(serialized["occurred_at"].as_str().is_some());
        assert!(serialized["payload"].is_object());
    }

    #[test]
    fn test_position_events_expose_position_id_and_source_time_consistently() {
        let position_changed = map_daemon_event(&test_position_changed()).unwrap();
        assert!(position_changed.payload["position_id"].as_str().is_some());
        assert!(position_changed.payload["source_occurred_at"].as_str().is_some());

        let awaiting_approval = map_daemon_event(&DaemonEvent::QueryAwaitingApproval {
            query_id: Uuid::now_v7(),
            position_id: Some(Uuid::now_v7()),
            reason: "Manual confirmation required".to_string(),
            expires_at: Utc::now(),
        })
        .unwrap();
        assert!(awaiting_approval.payload["query_id"].as_str().is_some());
        assert!(awaiting_approval.payload["expires_at"].as_str().is_some());

        let position_opened = map_daemon_event(&DaemonEvent::CorePositionOpened {
            position_id: Uuid::now_v7(),
            symbol: test_symbol(),
            side: Side::Long,
            binance_position_id: "binance-123".to_string(),
        })
        .unwrap();
        assert!(position_opened.payload["position_id"].as_str().is_some());
        assert!(position_opened.payload.get("source_occurred_at").is_none());
    }

    #[test]
    fn test_resync_required_event_uses_public_contract() {
        let event = resync_required_event("lagged", "missed 4 events");

        assert_eq!(event.event_type, "system.resync_required");
        assert_eq!(event.payload["reason"], "lagged");
        assert_eq!(event.payload["message"], "missed 4 events");
    }
}
