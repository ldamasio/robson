//! Per-position detector task.
//!
//! The detector monitors market data for a specific armed position
//! and emits a DetectorSignal when entry conditions are met.
//!
//! # Architecture
//!
//! Each armed position spawns its own DetectorTask:
//!
//! ```text
//! Position (Armed)
//!     ↓
//! DetectorTask::spawn()
//!     ↓
//! ┌─────────────────────────────────────────┐
//! │ Async Loop                              │
//! │ - Subscribe to EventBus                 │
//! │ - Filter MarketData for symbol          │
//! │ - Apply detection logic                 │
//! │ - Emit DetectorSignal once → terminate  │
//! └─────────────────────────────────────────┘
//! ```
//!
//! # Single-Shot Behavior
//!
//! The detector is "single-shot": it emits exactly one signal then exits.
//! This ensures idempotency and prevents duplicate entries.

use std::sync::Arc;

use chrono::Utc;
use robson_domain::{DetectorSignal, Position, PositionId, Price, Side, Symbol};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::event_bus::{DaemonEvent, EventBus, MarketData};
use crate::{DaemonError, DaemonResult};

// =============================================================================
// Detector Configuration
// =============================================================================

/// Configuration for a detector task.
///
/// Extracted from an armed Position to provide the detector
/// with immutable context during its lifetime.
#[derive(Debug, Clone)]
pub struct DetectorConfig {
    /// Position this detector belongs to
    pub position_id: PositionId,
    /// Symbol to monitor
    pub symbol: Symbol,
    /// Direction (Long/Short)
    pub side: Side,
    /// Stop loss percentage from entry (placeholder)
    /// e.g., 0.02 = 2% below entry for Long, 2% above for Short
    pub stop_loss_percent: Decimal,
}

impl DetectorConfig {
    /// Create detector config from an armed position.
    ///
    /// Uses default stop loss percentage (2%) for placeholder logic.
    pub fn from_position(position: &Position) -> DaemonResult<Self> {
        if !position.can_enter() {
            return Err(DaemonError::InvalidPositionState {
                expected: "Armed".to_string(),
                actual: format!("{:?}", position.state),
            });
        }

        Ok(Self {
            position_id: position.id,
            symbol: position.symbol.clone(),
            side: position.side,
            stop_loss_percent: dec!(0.02), // 2% placeholder
        })
    }
}

// =============================================================================
// Detector Task
// =============================================================================

/// Per-position detector that monitors market data and emits entry signals.
///
/// # Lifecycle
///
/// 1. Created from an armed Position
/// 2. Spawned as async task
/// 3. Subscribes to EventBus for MarketData events
/// 4. Filters events for its symbol
/// 5. Applies detection logic (placeholder: trigger on first tick)
/// 6. Emits DetectorSignal via EventBus
/// 7. Terminates
///
/// # Thread Safety
///
/// DetectorTask is not Clone. It is consumed when spawned via `spawn()`.
/// The returned JoinHandle can be used to await completion or cancel.
pub struct DetectorTask {
    config: DetectorConfig,
    event_bus: Arc<EventBus>,
}

impl DetectorTask {
    /// Create a new detector task.
    ///
    /// # Arguments
    ///
    /// * `config` - Detector configuration (from armed position)
    /// * `event_bus` - Shared event bus for receiving market data and emitting signals
    pub fn new(config: DetectorConfig, event_bus: Arc<EventBus>) -> Self {
        Self { config, event_bus }
    }

    /// Create detector directly from an armed position.
    ///
    /// Convenience method that extracts config from position.
    pub fn from_position(position: &Position, event_bus: Arc<EventBus>) -> DaemonResult<Self> {
        let config = DetectorConfig::from_position(position)?;
        Ok(Self::new(config, event_bus))
    }

    /// Spawn the detector as an async task.
    ///
    /// Returns a JoinHandle that resolves to `Option<DetectorSignal>`:
    /// - `Some(signal)` if detection succeeded
    /// - `None` if shutdown or error occurred
    ///
    /// The detector task will terminate after emitting one signal.
    pub fn spawn(self) -> JoinHandle<Option<DetectorSignal>> {
        let position_id = self.config.position_id;
        let symbol = self.config.symbol.clone();

        tokio::spawn(async move {
            info!(
                position_id = %position_id,
                symbol = %symbol.as_pair(),
                "Detector task started"
            );

            let result = self.run().await;

            match &result {
                Some(signal) => {
                    info!(
                        position_id = %position_id,
                        signal_id = %signal.signal_id,
                        entry_price = %signal.entry_price.as_decimal(),
                        "Detector emitted signal"
                    );
                }
                None => {
                    info!(
                        position_id = %position_id,
                        "Detector terminated without signal"
                    );
                }
            }

            result
        })
    }

    /// Run the detector loop.
    ///
    /// This is the main async loop that:
    /// 1. Subscribes to EventBus
    /// 2. Filters MarketData for our symbol
    /// 3. Applies detection logic
    /// 4. Returns signal or None
    async fn run(self) -> Option<DetectorSignal> {
        let mut receiver = self.event_bus.subscribe();

        loop {
            match receiver.recv().await {
                Some(Ok(event)) => {
                    if let Some(signal) = self.handle_event(event) {
                        // Emit signal via event bus
                        self.event_bus.send(DaemonEvent::DetectorSignal(signal.clone()));
                        return Some(signal);
                    }
                }
                Some(Err(lag_msg)) => {
                    warn!(
                        position_id = %self.config.position_id,
                        error = %lag_msg,
                        "Detector receiver lagged"
                    );
                    // Continue processing despite lag
                }
                None => {
                    // Channel closed (shutdown)
                    debug!(
                        position_id = %self.config.position_id,
                        "Detector channel closed"
                    );
                    return None;
                }
            }
        }
    }

    /// Handle a single daemon event.
    ///
    /// Returns `Some(signal)` if detection triggered, `None` otherwise.
    fn handle_event(&self, event: DaemonEvent) -> Option<DetectorSignal> {
        match event {
            DaemonEvent::MarketData(market_data) => {
                self.handle_market_data(&market_data)
            }
            DaemonEvent::Shutdown => {
                debug!(
                    position_id = %self.config.position_id,
                    "Detector received shutdown"
                );
                // Return None to trigger loop exit
                // Note: This doesn't directly exit, but we could restructure
                // to handle shutdown more explicitly if needed
                None
            }
            _ => {
                // Ignore other event types
                None
            }
        }
    }

    /// Handle market data event.
    ///
    /// Filters for our symbol and applies detection logic.
    fn handle_market_data(&self, market_data: &MarketData) -> Option<DetectorSignal> {
        // Filter: only process our symbol
        if market_data.symbol != self.config.symbol {
            return None;
        }

        debug!(
            position_id = %self.config.position_id,
            symbol = %market_data.symbol.as_pair(),
            price = %market_data.price.as_decimal(),
            "Detector received market data"
        );

        // Apply detection logic
        if self.should_signal(market_data) {
            Some(self.create_signal(market_data))
        } else {
            None
        }
    }

    /// Placeholder detection logic.
    ///
    /// Current implementation: trigger on first tick (single-shot).
    ///
    /// TODO: Replace with real pattern detection in future phases:
    /// - MA crossover
    /// - Support/resistance bounce
    /// - Consolidation breakout
    /// - etc.
    fn should_signal(&self, _market_data: &MarketData) -> bool {
        // PLACEHOLDER: Always trigger on first tick
        // This satisfies "single-shot" requirement
        true
    }

    /// Create a DetectorSignal from current market data.
    fn create_signal(&self, market_data: &MarketData) -> DetectorSignal {
        let entry_price = market_data.price;

        // Calculate stop loss based on side
        let stop_loss = self.calculate_stop_loss(entry_price);

        DetectorSignal {
            signal_id: Uuid::now_v7(),
            position_id: self.config.position_id,
            symbol: self.config.symbol.clone(),
            side: self.config.side,
            entry_price,
            stop_loss,
            timestamp: Utc::now(),
        }
    }

    /// Calculate stop loss price based on entry and side.
    ///
    /// For Long: stop = entry * (1 - stop_loss_percent)
    /// For Short: stop = entry * (1 + stop_loss_percent)
    fn calculate_stop_loss(&self, entry_price: Price) -> Price {
        let entry = entry_price.as_decimal();
        let pct = self.config.stop_loss_percent;

        let stop = match self.config.side {
            Side::Long => entry * (Decimal::ONE - pct),
            Side::Short => entry * (Decimal::ONE + pct),
        };

        // Unwrap is safe: result will be positive if entry is positive
        Price::new(stop).expect("Stop loss calculation produced invalid price")
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use robson_domain::Side;
    use rust_decimal_macros::dec;

    fn create_test_config() -> DetectorConfig {
        DetectorConfig {
            position_id: Uuid::now_v7(),
            symbol: robson_domain::Symbol::from_pair("BTCUSDT").unwrap(),
            side: Side::Long,
            stop_loss_percent: dec!(0.02),
        }
    }

    fn create_test_market_data(symbol: &str, price: Decimal) -> MarketData {
        MarketData {
            symbol: robson_domain::Symbol::from_pair(symbol).unwrap(),
            price: Price::new(price).unwrap(),
            timestamp: Utc::now(),
        }
    }

    #[test]
    fn test_detector_config_from_position() {
        let position = Position::new(
            Uuid::now_v7(),
            robson_domain::Symbol::from_pair("ETHUSDT").unwrap(),
            Side::Short,
        );

        let config = DetectorConfig::from_position(&position).unwrap();

        assert_eq!(config.position_id, position.id);
        assert_eq!(config.symbol.as_pair(), "ETHUSDT");
        assert_eq!(config.side, Side::Short);
        assert_eq!(config.stop_loss_percent, dec!(0.02));
    }

    #[test]
    fn test_calculate_stop_loss_long() {
        let config = DetectorConfig {
            position_id: Uuid::now_v7(),
            symbol: robson_domain::Symbol::from_pair("BTCUSDT").unwrap(),
            side: Side::Long,
            stop_loss_percent: dec!(0.02), // 2%
        };
        let event_bus = Arc::new(EventBus::new(10));
        let detector = DetectorTask::new(config, event_bus);

        let entry = Price::new(dec!(100000)).unwrap();
        let stop = detector.calculate_stop_loss(entry);

        // Long: stop = 100000 * (1 - 0.02) = 98000
        assert_eq!(stop.as_decimal(), dec!(98000));
    }

    #[test]
    fn test_calculate_stop_loss_short() {
        let config = DetectorConfig {
            position_id: Uuid::now_v7(),
            symbol: robson_domain::Symbol::from_pair("BTCUSDT").unwrap(),
            side: Side::Short,
            stop_loss_percent: dec!(0.02), // 2%
        };
        let event_bus = Arc::new(EventBus::new(10));
        let detector = DetectorTask::new(config, event_bus);

        let entry = Price::new(dec!(100000)).unwrap();
        let stop = detector.calculate_stop_loss(entry);

        // Short: stop = 100000 * (1 + 0.02) = 102000
        assert_eq!(stop.as_decimal(), dec!(102000));
    }

    #[test]
    fn test_handle_market_data_filters_symbol() {
        let config = create_test_config(); // BTCUSDT
        let event_bus = Arc::new(EventBus::new(10));
        let detector = DetectorTask::new(config, event_bus);

        // Different symbol should be ignored
        let other_data = create_test_market_data("ETHUSDT", dec!(3000));
        let result = detector.handle_market_data(&other_data);
        assert!(result.is_none());
    }

    #[test]
    fn test_handle_market_data_triggers_signal() {
        let config = create_test_config(); // BTCUSDT, Long
        let position_id = config.position_id;
        let event_bus = Arc::new(EventBus::new(10));
        let detector = DetectorTask::new(config, event_bus);

        let market_data = create_test_market_data("BTCUSDT", dec!(95000));
        let result = detector.handle_market_data(&market_data);

        assert!(result.is_some());
        let signal = result.unwrap();
        assert_eq!(signal.position_id, position_id);
        assert_eq!(signal.entry_price.as_decimal(), dec!(95000));
        assert_eq!(signal.stop_loss.as_decimal(), dec!(93100)); // 95000 * 0.98
        assert_eq!(signal.side, Side::Long);
    }

    #[tokio::test]
    async fn test_detector_spawn_and_signal() {
        let config = create_test_config();
        let position_id = config.position_id;
        let event_bus = Arc::new(EventBus::new(100));

        // Create detector
        let detector = DetectorTask::new(config, Arc::clone(&event_bus));

        // Subscribe before spawning to receive the signal
        let mut receiver = event_bus.subscribe();

        // Spawn detector
        let handle = detector.spawn();

        // Yield to let the detector task subscribe
        tokio::task::yield_now().await;

        // Send market data
        let market_data = create_test_market_data("BTCUSDT", dec!(96000));
        event_bus.send(DaemonEvent::MarketData(market_data));

        // Wait for detector to complete with timeout
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            handle,
        )
        .await
        .expect("Detector timed out")
        .expect("Detector task panicked");

        assert!(result.is_some());

        let signal = result.unwrap();
        assert_eq!(signal.position_id, position_id);
        assert_eq!(signal.entry_price.as_decimal(), dec!(96000));

        // Signal should also be on the event bus
        // Skip the MarketData event we sent
        let _ = receiver.recv().await;
        // Get the DetectorSignal
        let event = receiver.recv().await.unwrap().unwrap();
        match event {
            DaemonEvent::DetectorSignal(s) => {
                assert_eq!(s.position_id, position_id);
            }
            _ => panic!("Expected DetectorSignal event"),
        }
    }

    #[tokio::test]
    async fn test_detector_ignores_wrong_symbol() {
        let config = create_test_config(); // BTCUSDT
        let event_bus = Arc::new(EventBus::new(100));

        let detector = DetectorTask::new(config, Arc::clone(&event_bus));
        let handle = detector.spawn();

        // Yield to let the detector task subscribe
        tokio::task::yield_now().await;

        // Send wrong symbol first
        let eth_data = create_test_market_data("ETHUSDT", dec!(3000));
        event_bus.send(DaemonEvent::MarketData(eth_data));

        // Then send correct symbol
        let btc_data = create_test_market_data("BTCUSDT", dec!(95000));
        event_bus.send(DaemonEvent::MarketData(btc_data));

        // Detector should signal on BTC, not ETH (with timeout)
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            handle,
        )
        .await
        .expect("Detector timed out")
        .expect("Detector task panicked");

        assert!(result.is_some());

        let signal = result.unwrap();
        assert_eq!(signal.symbol.as_pair(), "BTCUSDT");
        assert_eq!(signal.entry_price.as_decimal(), dec!(95000));
    }
}
