//! Event bus for internal daemon communication.
//!
//! The event bus allows decoupled communication between:
//! - Detectors → Position Manager (entry signals)
//! - Market Data → Position Manager (price updates)
//! - Executor → Position Manager (order fills)
//!
//! Uses tokio broadcast channels for fan-out to multiple receivers.

use chrono::{DateTime, Utc};
use robson_domain::{DetectorSignal, Price, PositionId, Symbol};
use tokio::sync::broadcast;
use uuid::Uuid;

// =============================================================================
// Event Types
// =============================================================================

/// Events that flow through the daemon event bus.
#[derive(Debug, Clone)]
pub enum DaemonEvent {
    /// Detector fired an entry signal
    DetectorSignal(DetectorSignal),

    /// Market data price update
    MarketData(MarketData),

    /// Order was filled on exchange
    OrderFill(OrderFill),

    /// Position state changed
    PositionStateChanged {
        position_id: PositionId,
        previous_state: String,
        new_state: String,
        timestamp: DateTime<Utc>,
    },

    /// Shutdown signal
    Shutdown,
}

/// Market data price update.
#[derive(Debug, Clone)]
pub struct MarketData {
    /// Symbol this update is for
    pub symbol: Symbol,
    /// Current price
    pub price: Price,
    /// When this price was observed
    pub timestamp: DateTime<Utc>,
}

/// Order fill notification.
#[derive(Debug, Clone)]
pub struct OrderFill {
    /// Position this fill belongs to
    pub position_id: PositionId,
    /// Order that was filled
    pub order_id: Uuid,
    /// Fill price
    pub fill_price: Price,
    /// Filled quantity
    pub filled_quantity: robson_domain::Quantity,
    /// Fee paid
    pub fee: rust_decimal::Decimal,
    /// When the fill occurred
    pub filled_at: DateTime<Utc>,
}

// =============================================================================
// Event Bus
// =============================================================================

/// Event bus for daemon-wide communication.
///
/// Multiple producers can send events, and multiple consumers can receive.
/// Uses broadcast channels for fan-out pattern.
pub struct EventBus {
    sender: broadcast::Sender<DaemonEvent>,
}

impl EventBus {
    /// Create a new event bus with specified capacity.
    ///
    /// Capacity determines how many events can be buffered before
    /// slow receivers start missing events (lagging).
    pub fn new(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity);
        Self { sender }
    }

    /// Send an event to all subscribers.
    ///
    /// Returns the number of receivers that received the event.
    /// Returns 0 if there are no active receivers.
    pub fn send(&self, event: DaemonEvent) -> usize {
        // send() returns Err if there are no receivers, but we don't care
        self.sender.send(event).unwrap_or(0)
    }

    /// Subscribe to events.
    ///
    /// Returns a receiver that will receive all events sent after subscription.
    pub fn subscribe(&self) -> EventReceiver {
        EventReceiver {
            receiver: self.sender.subscribe(),
        }
    }

    /// Get the number of active receivers.
    pub fn receiver_count(&self) -> usize {
        self.sender.receiver_count()
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new(1000)
    }
}

/// Receiver for daemon events.
pub struct EventReceiver {
    receiver: broadcast::Receiver<DaemonEvent>,
}

impl EventReceiver {
    /// Receive the next event.
    ///
    /// Returns `None` if the sender has been dropped.
    /// Returns error description if the receiver lagged (missed events).
    pub async fn recv(&mut self) -> Option<Result<DaemonEvent, String>> {
        match self.receiver.recv().await {
            Ok(event) => Some(Ok(event)),
            Err(broadcast::error::RecvError::Closed) => None,
            Err(broadcast::error::RecvError::Lagged(count)) => {
                Some(Err(format!("Receiver lagged, missed {} events", count)))
            }
        }
    }

    /// Try to receive an event without blocking.
    ///
    /// Returns `None` if no event is immediately available.
    pub fn try_recv(&mut self) -> Option<Result<DaemonEvent, String>> {
        match self.receiver.try_recv() {
            Ok(event) => Some(Ok(event)),
            Err(broadcast::error::TryRecvError::Empty) => None,
            Err(broadcast::error::TryRecvError::Closed) => None,
            Err(broadcast::error::TryRecvError::Lagged(count)) => {
                Some(Err(format!("Receiver lagged, missed {} events", count)))
            }
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use robson_domain::{OrderSide, Quantity, Side, Symbol};
    use rust_decimal_macros::dec;

    fn create_test_signal() -> DetectorSignal {
        DetectorSignal {
            signal_id: Uuid::now_v7(),
            position_id: Uuid::now_v7(),
            symbol: Symbol::from_pair("BTCUSDT").unwrap(),
            side: Side::Long,
            entry_price: Price::new(dec!(95000)).unwrap(),
            stop_loss: Price::new(dec!(93500)).unwrap(),
            timestamp: Utc::now(),
        }
    }

    #[tokio::test]
    async fn test_event_bus_send_recv() {
        let bus = EventBus::new(10);
        let mut receiver = bus.subscribe();

        let signal = create_test_signal();
        let position_id = signal.position_id;

        bus.send(DaemonEvent::DetectorSignal(signal));

        let event = receiver.recv().await.unwrap().unwrap();
        match event {
            DaemonEvent::DetectorSignal(s) => {
                assert_eq!(s.position_id, position_id);
            }
            _ => panic!("Expected DetectorSignal event"),
        }
    }

    #[tokio::test]
    async fn test_event_bus_multiple_receivers() {
        let bus = EventBus::new(10);
        let mut receiver1 = bus.subscribe();
        let mut receiver2 = bus.subscribe();

        assert_eq!(bus.receiver_count(), 2);

        let signal = create_test_signal();
        bus.send(DaemonEvent::DetectorSignal(signal));

        // Both receivers should get the event
        let event1 = receiver1.recv().await.unwrap().unwrap();
        let event2 = receiver2.recv().await.unwrap().unwrap();

        assert!(matches!(event1, DaemonEvent::DetectorSignal(_)));
        assert!(matches!(event2, DaemonEvent::DetectorSignal(_)));
    }

    #[tokio::test]
    async fn test_event_bus_no_receivers() {
        let bus = EventBus::new(10);

        // Send with no receivers should not panic
        let count = bus.send(DaemonEvent::Shutdown);
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn test_event_bus_market_data() {
        let bus = EventBus::new(10);
        let mut receiver = bus.subscribe();

        let market_data = MarketData {
            symbol: Symbol::from_pair("BTCUSDT").unwrap(),
            price: Price::new(dec!(96000)).unwrap(),
            timestamp: Utc::now(),
        };

        bus.send(DaemonEvent::MarketData(market_data));

        let event = receiver.recv().await.unwrap().unwrap();
        match event {
            DaemonEvent::MarketData(data) => {
                assert_eq!(data.price.as_decimal(), dec!(96000));
            }
            _ => panic!("Expected MarketData event"),
        }
    }

    #[tokio::test]
    async fn test_event_bus_order_fill() {
        let bus = EventBus::new(10);
        let mut receiver = bus.subscribe();

        let position_id = Uuid::now_v7();
        let order_fill = OrderFill {
            position_id,
            order_id: Uuid::now_v7(),
            fill_price: Price::new(dec!(95000)).unwrap(),
            filled_quantity: Quantity::new(dec!(0.1)).unwrap(),
            fee: dec!(0.0001),
            filled_at: Utc::now(),
        };

        bus.send(DaemonEvent::OrderFill(order_fill));

        let event = receiver.recv().await.unwrap().unwrap();
        match event {
            DaemonEvent::OrderFill(fill) => {
                assert_eq!(fill.position_id, position_id);
            }
            _ => panic!("Expected OrderFill event"),
        }
    }

    #[test]
    fn test_try_recv_empty() {
        let bus = EventBus::new(10);
        let mut receiver = bus.subscribe();

        // No events sent yet
        assert!(receiver.try_recv().is_none());
    }
}
