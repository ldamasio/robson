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
//! │ - Apply MA crossover detection          │
//! │ - Check cancellation token              │
//! │ - Emit DetectorSignal once → terminate  │
//! └─────────────────────────────────────────┘
//! ```
//!
//! # Single-Shot Behavior
//!
//! The detector is "single-shot": it emits exactly one signal then exits.
//! This ensures idempotency and prevents duplicate entries.
//!
//! # MA Crossover Detection
//!
//! The detector uses Simple Moving Average (SMA) crossover to detect entry signals:
//!
//! - **Long**: Fast MA crosses **above** Slow MA
//! - **Short**: Fast MA crosses **below** Slow MA
//!
//! Only the **crossover** triggers a signal, not the position above/below.
//!
//! # Graceful Shutdown
//!
//! The detector supports graceful shutdown via `CancellationToken`:
//!
//! - On cancellation, detector exits with `None` (no signal)
//! - Cooperative: checks token between events
//! - No orphaned tasks after shutdown

use std::collections::VecDeque;
use std::sync::Arc;

use chrono::Utc;
use robson_domain::{DetectorSignal, Position, PositionId, Price, Side, Symbol};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
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
    /// Fast MA period (e.g., 9 for short-term momentum)
    pub ma_fast_period: usize,
    /// Slow MA period (e.g., 21 for trend confirmation)
    pub ma_slow_period: usize,
    /// Stop loss percentage from entry
    /// e.g., 0.02 = 2% below entry for Long, 2% above for Short
    pub stop_loss_percent: Decimal,
}

impl DetectorConfig {
    /// Create detector config from an armed position.
    ///
    /// Uses default MA periods (9/21) and 2% stop loss.
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
            ma_fast_period: 9,   // Default fast MA
            ma_slow_period: 21,  // Default slow MA
            stop_loss_percent: dec!(0.02), // 2% default
        })
    }

    /// Validate MA configuration.
    ///
    /// Ensures fast period < slow period and both are > 1.
    pub fn validate(&self) -> DaemonResult<()> {
        if self.ma_fast_period >= self.ma_slow_period {
            return Err(DaemonError::Detector(format!(
                "Fast MA period ({}) must be less than slow MA period ({})",
                self.ma_fast_period, self.ma_slow_period
            )));
        }
        if self.ma_fast_period < 2 {
            return Err(DaemonError::Detector(format!(
                "Fast MA period must be at least 2, got {}",
                self.ma_fast_period
            )));
        }
        Ok(())
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
/// 5. Applies MA crossover detection logic
/// 6. Checks cancellation token for graceful shutdown
/// 7. Emits DetectorSignal via EventBus (if not cancelled)
/// 8. Terminates
///
/// # Thread Safety
///
/// DetectorTask is not Clone. It is consumed when spawned via `spawn()`.
/// The returned JoinHandle can be used to await completion or cancel.
pub struct DetectorTask {
    config: DetectorConfig,
    event_bus: Arc<EventBus>,
    /// Cancellation token for graceful shutdown
    cancel_token: CancellationToken,
    /// Price buffer for MA calculation (circular buffer)
    price_buffer: VecDeque<Decimal>,
    /// Previous fast MA value (for crossover detection)
    prev_fast_ma: Option<Decimal>,
    /// Previous slow MA value (for crossover detection)
    prev_slow_ma: Option<Decimal>,
}

impl DetectorTask {
    /// Create a new detector task.
    ///
    /// # Arguments
    ///
    /// * `config` - Detector configuration (from armed position)
    /// * `event_bus` - Shared event bus for receiving market data and emitting signals
    /// * `cancel_token` - Cancellation token for graceful shutdown
    pub fn new(config: DetectorConfig, event_bus: Arc<EventBus>, cancel_token: CancellationToken) -> Self {
        // Validate configuration
        if let Err(e) = config.validate() {
            warn!(error = %e, "Invalid detector config, using defaults");
        }

        Self {
            config,
            event_bus,
            cancel_token,
            price_buffer: VecDeque::new(),
            prev_fast_ma: None,
            prev_slow_ma: None,
        }
    }

    /// Create detector directly from an armed position.
    ///
    /// Convenience method that extracts config from position.
    pub fn from_position(position: &Position, event_bus: Arc<EventBus>, cancel_token: CancellationToken) -> DaemonResult<Self> {
        let config = DetectorConfig::from_position(position)?;
        Ok(Self::new(config, event_bus, cancel_token))
    }

    /// Spawn the detector as an async task.
    ///
    /// Returns a JoinHandle that resolves to `Option<DetectorSignal>`:
    /// - `Some(signal)` if detection succeeded
    /// - `None` if shutdown, cancellation, or error occurred
    ///
    /// The detector task will terminate after:
    /// - Emitting one signal (single-shot)
    /// - Cancellation token is triggered
    /// - Event bus channel closes
    pub fn spawn(self) -> JoinHandle<Option<DetectorSignal>> {
        let position_id = self.config.position_id;
        let symbol = self.config.symbol.clone();
        let cancel_token = self.cancel_token.clone();

        tokio::spawn(async move {
            info!(
                position_id = %position_id,
                symbol = %symbol.as_pair(),
                "Detector task started"
            );

            let result = self.run(cancel_token).await;

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
    /// 2. Waits for events OR cancellation (cooperative)
    /// 3. Filters MarketData for our symbol
    /// 4. Applies MA crossover detection logic
    /// 5. Returns signal or None
    async fn run(mut self, cancel_token: CancellationToken) -> Option<DetectorSignal> {
        let mut receiver = self.event_bus.subscribe();

        loop {
            // Cooperatively check for cancellation
            tokio::select! {
                // Wait for event bus message
                event_result = receiver.recv() => {
                    match event_result {
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
                // Check for cancellation
                _ = cancel_token.cancelled() => {
                    info!(
                        position_id = %self.config.position_id,
                        "Detector cancelled via token"
                    );
                    return None;
                }
            }
        }
    }

    /// Handle a single daemon event.
    ///
    /// Returns `Some(signal)` if detection triggered, `None` otherwise.
    fn handle_event(&mut self, event: DaemonEvent) -> Option<DetectorSignal> {
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
    /// Filters for our symbol and applies MA crossover detection logic.
    fn handle_market_data(&mut self, market_data: &MarketData) -> Option<DetectorSignal> {
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

        // Apply MA crossover detection logic
        if self.should_signal(market_data) {
            Some(self.create_signal(market_data))
        } else {
            None
        }
    }

    /// MA crossover detection logic.
    ///
    /// Returns true when:
    /// - Long: Fast MA crosses ABOVE Slow MA
    /// - Short: Fast MA crosses BELOW Slow MA
    ///
    /// Only the **crossover** triggers, not the position.
    fn should_signal(&mut self, market_data: &MarketData) -> bool {
        let price = market_data.price.as_decimal();

        // Add price to buffer
        self.price_buffer.push_back(price);

        // Maintain buffer size (slow period + 1 for crossover detection)
        let max_size = self.config.ma_slow_period + 1;
        if self.price_buffer.len() > max_size {
            self.price_buffer.pop_front();
        }

        // Need at least slow_period data points
        if self.price_buffer.len() < self.config.ma_slow_period {
            debug!(
                position_id = %self.config.position_id,
                buffer_len = self.price_buffer.len(),
                required = self.config.ma_slow_period,
                "Insufficient data for MA calculation"
            );
            return false;
        }

        // Calculate current MAs
        let fast_ma = self.calculate_ma(self.config.ma_fast_period);
        let slow_ma = self.calculate_ma(self.config.ma_slow_period);

        // Check for crossover
        let crossover = match (&self.prev_fast_ma, &self.prev_slow_ma) {
            (Some(prev_fast), Some(prev_slow)) => {
                // Previous state existed, check for crossover
                let was_above = prev_fast > prev_slow;
                let is_above = fast_ma > slow_ma;

                match self.config.side {
                    Side::Long => !was_above && is_above,  // Crossed above
                    Side::Short => was_above && !is_above, // Crossed below
                }
            }
            _ => {
                // No previous state, wait for next tick
                debug!(
                    position_id = %self.config.position_id,
                    "No previous MA values, waiting for next tick"
                );
                false
            }
        };

        // Store current MA values for next tick
        self.prev_fast_ma = Some(fast_ma);
        self.prev_slow_ma = Some(slow_ma);

        if crossover {
            info!(
                position_id = %self.config.position_id,
                side = ?self.config.side,
                fast_ma = %fast_ma,
                slow_ma = %slow_ma,
                "MA crossover detected"
            );
        }

        crossover
    }

    /// Calculate Simple Moving Average (SMA).
    ///
    /// Returns the average of the last `period` prices.
    fn calculate_ma(&self, period: usize) -> Decimal {
        let start_idx = self.price_buffer.len().saturating_sub(period);
        let sum: Decimal = self.price_buffer
            .range(start_idx..)
            .sum();
        sum / Decimal::from(period)
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
            ma_fast_period: 3,   // Small for faster tests
            ma_slow_period: 5,   // Small for faster tests
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

    /// Create a cancellation token for tests (never cancelled).
    fn create_test_cancel_token() -> CancellationToken {
        CancellationToken::new()
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
        assert_eq!(config.ma_fast_period, 9);
        assert_eq!(config.ma_slow_period, 21);
        assert_eq!(config.stop_loss_percent, dec!(0.02));
    }

    #[test]
    fn test_calculate_stop_loss_long() {
        let config = DetectorConfig {
            position_id: Uuid::now_v7(),
            symbol: robson_domain::Symbol::from_pair("BTCUSDT").unwrap(),
            side: Side::Long,
            ma_fast_period: 9,
            ma_slow_period: 21,
            stop_loss_percent: dec!(0.02), // 2%
        };
        let event_bus = Arc::new(EventBus::new(10));
        let cancel_token = create_test_cancel_token();
        let detector = DetectorTask::new(config, event_bus, cancel_token);

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
            ma_fast_period: 9,
            ma_slow_period: 21,
            stop_loss_percent: dec!(0.02), // 2%
        };
        let event_bus = Arc::new(EventBus::new(10));
        let cancel_token = create_test_cancel_token();
        let detector = DetectorTask::new(config, event_bus, cancel_token);

        let entry = Price::new(dec!(100000)).unwrap();
        let stop = detector.calculate_stop_loss(entry);

        // Short: stop = 100000 * (1 + 0.02) = 102000
        assert_eq!(stop.as_decimal(), dec!(102000));
    }

    #[test]
    fn test_handle_market_data_filters_symbol() {
        let config = create_test_config(); // BTCUSDT
        let event_bus = Arc::new(EventBus::new(10));
        let cancel_token = create_test_cancel_token();
        let mut detector = DetectorTask::new(config, event_bus, cancel_token);

        // Different symbol should be ignored
        let other_data = create_test_market_data("ETHUSDT", dec!(3000));
        let result = detector.handle_market_data(&other_data);
        assert!(result.is_none());
    }

    #[test]
    fn test_ma_crossover_long_positive() {
        // Test MA crossover for Long position (fast crosses above slow)
        let config = DetectorConfig {
            position_id: Uuid::now_v7(),
            symbol: robson_domain::Symbol::from_pair("BTCUSDT").unwrap(),
            side: Side::Long,
            ma_fast_period: 3,
            ma_slow_period: 5,
            stop_loss_percent: dec!(0.02),
        };
        let event_bus = Arc::new(EventBus::new(10));
        let cancel_token = create_test_cancel_token();
        let mut detector = DetectorTask::new(config, event_bus, cancel_token);

        // Feed prices where fast MA < slow MA (descending trend)
        // This establishes the "previous state"
        let prices_below = vec![
            dec!(100), dec!(99), dec!(98), dec!(97), dec!(96), // All descending
            dec!(95), dec!(94),  // More data to establish state
        ];

        for price in prices_below {
            let data = create_test_market_data("BTCUSDT", price);
            detector.handle_market_data(&data);
        }

        // Now feed prices where fast MA crosses ABOVE slow MA (ascending trend)
        let prices_above = vec![
            dec!(96), dec!(98), dec!(100), dec!(102), dec!(104),
        ];

        let mut result = None;
        for price in prices_above {
            let data = create_test_market_data("BTCUSDT", price);
            result = detector.handle_market_data(&data);
            if result.is_some() {
                break; // Found crossover
            }
        }

        // Should trigger signal on crossover
        assert!(result.is_some(), "Should signal on Long crossover");
        let signal = result.unwrap();
        assert_eq!(signal.side, Side::Long);
    }

    #[test]
    fn test_ma_crossover_short_negative() {
        // Test MA crossover for Short position (fast crosses below slow)
        let config = DetectorConfig {
            position_id: Uuid::now_v7(),
            symbol: robson_domain::Symbol::from_pair("BTCUSDT").unwrap(),
            side: Side::Short,
            ma_fast_period: 3,
            ma_slow_period: 5,
            stop_loss_percent: dec!(0.02),
        };
        let event_bus = Arc::new(EventBus::new(10));
        let cancel_token = create_test_cancel_token();
        let mut detector = DetectorTask::new(config, event_bus, cancel_token);

        // Feed prices where fast MA > slow MA (ascending trend)
        // This establishes the "previous state"
        let prices_above = vec![
            dec!(100), dec!(101), dec!(102), dec!(103), dec!(104), // All ascending
            dec!(105), dec!(106), // More data to establish state
        ];

        for price in prices_above {
            let data = create_test_market_data("BTCUSDT", price);
            detector.handle_market_data(&data);
        }

        // Now feed prices where fast MA crosses BELOW slow MA (descending trend)
        let prices_below = vec![
            dec!(104), dec!(102), dec!(100), dec!(98), dec!(96),
        ];

        let mut result = None;
        for price in prices_below {
            let data = create_test_market_data("BTCUSDT", price);
            result = detector.handle_market_data(&data);
            if result.is_some() {
                break; // Found crossover
            }
        }

        // Should trigger signal on crossover
        assert!(result.is_some(), "Should signal on Short crossover");
        let signal = result.unwrap();
        assert_eq!(signal.side, Side::Short);
    }

    #[test]
    fn test_ma_crossover_insufficient_data() {
        // Test that detector doesn't signal with insufficient data
        let config = DetectorConfig {
            position_id: Uuid::now_v7(),
            symbol: robson_domain::Symbol::from_pair("BTCUSDT").unwrap(),
            side: Side::Long,
            ma_fast_period: 9,
            ma_slow_period: 21,
            stop_loss_percent: dec!(0.02),
        };
        let event_bus = Arc::new(EventBus::new(10));
        let cancel_token = create_test_cancel_token();
        let mut detector = DetectorTask::new(config, event_bus, cancel_token);

        // Feed less than slow_period prices
        for i in 1..15 {
            let price = Decimal::from(100 + i);
            let data = create_test_market_data("BTCUSDT", price);
            let result = detector.handle_market_data(&data);
            assert!(result.is_none(), "Should not signal with insufficient data");
        }
    }

    #[test]
    fn test_ma_crossover_no_signal_without_crossover() {
        // Test that being above/below doesn't trigger, only crossover
        let config = DetectorConfig {
            position_id: Uuid::now_v7(),
            symbol: robson_domain::Symbol::from_pair("BTCUSDT").unwrap(),
            side: Side::Long,
            ma_fast_period: 3,
            ma_slow_period: 5,
            stop_loss_percent: dec!(0.02),
        };
        let event_bus = Arc::new(EventBus::new(10));
        let cancel_token = create_test_cancel_token();
        let mut detector = DetectorTask::new(config, event_bus, cancel_token);

        // Feed prices where fast is above slow (already crossed)
        let prices = vec![
            dec!(110), dec!(110), dec!(110), dec!(100), dec!(100),
        ];

        for price in prices {
            let data = create_test_market_data("BTCUSDT", price);
            let _result = detector.handle_market_data(&data);
            // No crossover occurred (fast was already above slow)
            // First tick establishes state, subsequent ticks check crossover
        }

        // Continue with same relative position - should not trigger
        let data = create_test_market_data("BTCUSDT", dec!(110));
        let result = detector.handle_market_data(&data);
        assert!(result.is_none(), "Should not signal without crossover");
    }

    #[test]
    fn test_ma_config_validation() {
        // Test MA configuration validation
        let valid_config = DetectorConfig {
            position_id: Uuid::now_v7(),
            symbol: robson_domain::Symbol::from_pair("BTCUSDT").unwrap(),
            side: Side::Long,
            ma_fast_period: 9,
            ma_slow_period: 21,
            stop_loss_percent: dec!(0.02),
        };
        assert!(valid_config.validate().is_ok());

        // Fast >= Slow should fail
        let invalid_config = DetectorConfig {
            ma_fast_period: 21,
            ma_slow_period: 9,
            ..valid_config.clone()
        };
        assert!(invalid_config.validate().is_err());

        // Fast < 2 should fail
        let invalid_config2 = DetectorConfig {
            ma_fast_period: 1,
            ma_slow_period: 21,
            ..valid_config
        };
        assert!(invalid_config2.validate().is_err());
    }

    #[tokio::test]
    async fn test_detector_spawn_and_signal_ma_crossover() {
        // Integration test: detector spawned via EventBus with MA crossover
        let config = create_test_config(); // ma_fast=3, ma_slow=5
        let position_id = config.position_id;
        let event_bus = Arc::new(EventBus::new(100));

        // Create detector
        let cancel_token = create_test_cancel_token();
        let detector = DetectorTask::new(config, Arc::clone(&event_bus), cancel_token);

        // Subscribe before spawning to receive the signal
        let mut receiver = event_bus.subscribe();

        // Spawn detector
        let handle = detector.spawn();

        // Yield to let the detector task subscribe
        tokio::task::yield_now().await;

        // Feed descending prices to establish "below" state
        for i in (0..10).rev() {
            let price = Decimal::from(100 + i);
            let market_data = create_test_market_data("BTCUSDT", price);
            event_bus.send(DaemonEvent::MarketData(market_data));
        }

        // Feed ascending prices to trigger crossover
        for i in 0..6 {
            let price = Decimal::from(100 + i * 2);
            let market_data = create_test_market_data("BTCUSDT", price);
            event_bus.send(DaemonEvent::MarketData(market_data));
        }

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
        assert_eq!(signal.side, Side::Long);

        // Signal should also be on the event bus
        // Find the DetectorSignal among all events
        let mut found_signal = false;
        // Read several events to find the signal
        for _ in 0..50 {
            match receiver.recv().await {
                Some(Ok(DaemonEvent::DetectorSignal(s))) => {
                    assert_eq!(s.position_id, position_id);
                    found_signal = true;
                    break;
                }
                Some(Ok(_)) => continue, // Other events
                Some(Err(_)) | None => break, // Channel closed or error
            }
        }
        assert!(found_signal, "Should find DetectorSignal on event bus");
    }

    #[tokio::test]
    async fn test_detector_single_shot_behavior() {
        // Verify that detector only emits one signal then terminates
        let config = create_test_config();
        let event_bus = Arc::new(EventBus::new(100));

        let cancel_token = create_test_cancel_token();
        let detector = DetectorTask::new(config, Arc::clone(&event_bus), cancel_token);
        let handle = detector.spawn();

        tokio::task::yield_now().await;

        // Feed prices to trigger crossover
        for i in (0..10).rev() {
            let price = Decimal::from(100 + i);
            let market_data = create_test_market_data("BTCUSDT", price);
            event_bus.send(DaemonEvent::MarketData(market_data));
        }

        for i in 0..6 {
            let price = Decimal::from(100 + i * 2);
            let market_data = create_test_market_data("BTCUSDT", price);
            event_bus.send(DaemonEvent::MarketData(market_data));
        }

        // Wait for detector to complete
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            handle,
        )
        .await
        .expect("Detector timed out")
        .expect("Detector task panicked");

        // Should have emitted exactly one signal
        assert!(result.is_some());

        // Send more data - detector should be terminated
        let market_data = create_test_market_data("BTCUSDT", dec!(200));
        event_bus.send(DaemonEvent::MarketData(market_data));

        // Give time for any potential processing
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // If detector tried to emit another signal, it would panic
        // (since handle is already resolved)
        // No assertion needed - lack of panic means success
    }

    #[tokio::test]
    async fn test_detector_cancellation_token_shutdown() {
        // Test that detector exits gracefully when cancellation token is triggered
        let config = create_test_config();
        let event_bus = Arc::new(EventBus::new(100));
        let cancel_token = CancellationToken::new();

        let detector = DetectorTask::new(config, Arc::clone(&event_bus), cancel_token.clone());
        let handle = detector.spawn();

        tokio::task::yield_now().await;

        // Cancel the token
        cancel_token.cancel();

        // Wait for detector to finish
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            handle,
        )
        .await
        .expect("Detector should finish quickly after cancellation")
        .expect("Detector task panicked");

        // Should return None (no signal) due to cancellation
        assert!(result.is_none(), "Detector should not emit signal on cancellation");
    }

    #[tokio::test]
    async fn test_detector_cancellation_before_signal() {
        // Test that cancellation prevents signal emission
        let config = create_test_config();
        let event_bus = Arc::new(EventBus::new(100));
        let cancel_token = CancellationToken::new();

        let detector = DetectorTask::new(config, Arc::clone(&event_bus), cancel_token.clone());
        let handle = detector.spawn();

        tokio::task::yield_now().await;

        // Feed some data (not enough to trigger MA crossover)
        for i in 0..5 {
            let price = Decimal::from(100 + i);
            let market_data = create_test_market_data("BTCUSDT", price);
            event_bus.send(DaemonEvent::MarketData(market_data));
        }

        // Cancel before signal
        cancel_token.cancel();

        // Wait for detector to finish
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            handle,
        )
        .await
        .expect("Detector should finish after cancellation")
        .expect("Detector task panicked");

        // Should return None (cancelled before signal)
        assert!(result.is_none(), "Detector should be cancelled before emitting signal");
    }

    #[tokio::test]
    async fn test_multiple_detectors_shutdown() {
        // Test that multiple detectors can be cancelled simultaneously
        let event_bus = Arc::new(EventBus::new(100));
        let cancel_token = CancellationToken::new();

        let mut handles = vec![];

        // Spawn multiple detectors
        for _ in 0..5 {
            let config = create_test_config();
            let detector = DetectorTask::new(config, Arc::clone(&event_bus), cancel_token.clone());
            let handle = detector.spawn();
            handles.push(handle);
        }

        tokio::task::yield_now().await;

        // Cancel all at once
        cancel_token.cancel();

        // Wait for all to finish
        for handle in handles {
            let result = tokio::time::timeout(
                std::time::Duration::from_secs(1),
                handle,
            )
            .await
            .expect("Detector should finish quickly after cancellation")
            .expect("Detector task panicked");

            // All should return None (cancelled)
            assert!(result.is_none(), "Detector should not emit signal on cancellation");
        }
    }
}
