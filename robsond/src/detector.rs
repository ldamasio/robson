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
//! The detector uses Simple Moving Average (SMA) crossover to detect entry
//! signals:
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

use std::sync::Arc;

use robson_domain::{
    entities::EffectiveStopBasis, AnchorType, Candle, DetectorSignal, EntryPolicy,
    EntryPolicyConfig, Event, Position, PositionId, Price, Side, SignalEvaluationOutcome,
    StopAnchor, Symbol, TechnicalStopAnalysisAudit, TechnicalStopConfidenceSnapshot,
    TechnicalStopConfigSnapshot, TechnicalStopMethodSnapshot,
};
use robson_engine::{
    stop_quality_classifier::{classify_stop_quality, StopQualityInput, StopQualityThresholds},
    technical_stop_analyzer::{
        StopConfidence as AnalyzerStopConfidence, TechnicalStopAnalysis, TechnicalStopAnalyzer,
        TechnicalStopConfig, TechnicalStopMethod as AnalyzerTechnicalStopMethod,
    },
    KeyLevelStrategy, ReversalPatternStrategy, SignalContext, SignalDecision, SignalReason,
    SmaCrossoverStrategy, StrategyRegistry,
};
use robson_exec::{CandleInterval, OhlcvPort};
use tokio::{task::JoinHandle, time::sleep};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::{
    event_bus::{DaemonEvent, EventBus, EventReceiver, MarketData},
    DaemonError, DaemonResult,
};

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
    /// Technical stop analysis settings (WHERE the stop is).
    pub technical_stop_config: TechnicalStopConfig,
    /// Entry and approval policy selected for this position.
    pub entry_policy: EntryPolicyConfig,
}

impl DetectorConfig {
    /// Create detector config from an armed position.
    ///
    /// Uses default MA periods (9/21) and chart-derived technical stop config.
    pub fn from_position(position: &Position) -> DaemonResult<Self> {
        Self::from_position_with_policy(position, EntryPolicyConfig::default())
    }

    /// Create detector config from an armed position and explicit entry policy.
    pub fn from_position_with_policy(
        position: &Position,
        entry_policy: EntryPolicyConfig,
    ) -> DaemonResult<Self> {
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
            ma_fast_period: 9,  // Default fast MA
            ma_slow_period: 21, // Default slow MA
            technical_stop_config: TechnicalStopConfig::default(),
            entry_policy,
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
    ohlcv_port: Arc<dyn OhlcvPort>,
    /// Cancellation token for graceful shutdown
    cancel_token: CancellationToken,
    /// Deterministic strategy registry for this detector.
    strategy_registry: StrategyRegistry,
    /// Whether the entry-time invalidation guard clamps the effective stop
    /// beyond a recent adverse extreme (ADR-0042).
    stop_invalidation_guard_enabled: bool,
    /// Number of 15m candles (incl. the forming candle) used to sample the
    /// recent adverse extreme when the guard is enabled (ADR-0042).
    stop_invalidation_lookback_candles: usize,
}

impl DetectorTask {
    /// Create a new detector task.
    ///
    /// # Arguments
    ///
    /// * `config` - Detector configuration (from armed position)
    /// * `event_bus` - Shared event bus for receiving market data and emitting
    ///   signals
    /// * `ohlcv_port` - Historical candle source for technical stop analysis
    /// * `cancel_token` - Cancellation token for graceful shutdown
    pub fn new(
        config: DetectorConfig,
        event_bus: Arc<EventBus>,
        ohlcv_port: Arc<dyn OhlcvPort>,
        cancel_token: CancellationToken,
    ) -> Self {
        // Validate configuration
        if let Err(e) = config.validate() {
            warn!(error = %e, "Invalid detector config, using defaults");
        }

        let mut strategy_registry = StrategyRegistry::empty();
        strategy_registry.register(
            StrategyRegistry::strategy_id_for_policy(EntryPolicy::ConfirmedTrend)
                .expect("confirmed trend must resolve"),
            Box::new(SmaCrossoverStrategy::new(config.ma_fast_period, config.ma_slow_period)),
        );
        strategy_registry.register(
            StrategyRegistry::strategy_id_for_policy(EntryPolicy::ConfirmedReversal)
                .expect("confirmed reversal must resolve"),
            Box::new(ReversalPatternStrategy::default()),
        );
        strategy_registry.register(
            StrategyRegistry::strategy_id_for_policy(EntryPolicy::ConfirmedKeyLevel)
                .expect("confirmed key level must resolve"),
            Box::new(KeyLevelStrategy::default()),
        );

        Self {
            config,
            event_bus,
            ohlcv_port,
            cancel_token,
            strategy_registry,
            // Guard disabled by default; the daemon enables it per
            // `EngineConfig` via `with_invalidation_guard`.
            stop_invalidation_guard_enabled: false,
            stop_invalidation_lookback_candles: 20,
        }
    }

    /// Configure the entry-time invalidation guard (ADR-0042).
    ///
    /// When enabled, `create_signal` samples the recent adverse extreme
    /// (highest high for shorts / lowest low for longs) over the last
    /// `lookback_candles` 15m candles — including the forming candle — and
    /// records it in the technical-stop audit so the engine can clamp the
    /// effective stop beyond it. Disabled is byte-for-byte identical to the
    /// historical behavior.
    pub fn with_invalidation_guard(mut self, enabled: bool, lookback_candles: usize) -> Self {
        self.stop_invalidation_guard_enabled = enabled;
        self.stop_invalidation_lookback_candles = lookback_candles.max(1);
        self
    }

    /// Create detector directly from an armed position.
    ///
    /// Convenience method that extracts config from position.
    pub fn from_position(
        position: &Position,
        event_bus: Arc<EventBus>,
        ohlcv_port: Arc<dyn OhlcvPort>,
        cancel_token: CancellationToken,
    ) -> DaemonResult<Self> {
        Self::from_position_with_policy(
            position,
            EntryPolicyConfig::default(),
            event_bus,
            ohlcv_port,
            cancel_token,
        )
    }

    /// Create detector directly from an armed position and explicit entry
    /// policy.
    pub fn from_position_with_policy(
        position: &Position,
        entry_policy: EntryPolicyConfig,
        event_bus: Arc<EventBus>,
        ohlcv_port: Arc<dyn OhlcvPort>,
        cancel_token: CancellationToken,
    ) -> DaemonResult<Self> {
        let config = DetectorConfig::from_position_with_policy(position, entry_policy)?;
        Ok(Self::new(config, event_bus, ohlcv_port, cancel_token))
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
        // Subscribe before spawning so ticks published immediately after spawn
        // are buffered for this detector instead of being lost to scheduling.
        let receiver = self.event_bus.subscribe();

        tokio::spawn(async move {
            info!(
                position_id = %position_id,
                symbol = %symbol.as_pair(),
                "Detector task started"
            );

            let result = self.run(cancel_token, receiver).await;

            match &result {
                Some(signal) => {
                    info!(
                        position_id = %position_id,
                        signal_id = %signal.signal_id,
                        entry_price = %signal.entry_price.as_decimal(),
                        "Detector emitted signal"
                    );
                },
                None => {
                    info!(
                        position_id = %position_id,
                        "Detector terminated without signal"
                    );
                },
            }

            result
        })
    }

    /// Run the detector loop.
    ///
    /// This is the main async loop that:
    /// 1. Waits for events OR cancellation (cooperative)
    /// 2. Filters MarketData for our symbol
    /// 3. Applies signal detection logic
    /// 4. Returns signal or None
    async fn run(
        mut self,
        cancel_token: CancellationToken,
        mut receiver: EventReceiver,
    ) -> Option<DetectorSignal> {
        // For Immediate mode, keep retrying proactively before falling back.
        // This avoids silently downgrading a no-signal arm into a tick-waiting arm
        // when the first candle fetch or stop analysis is transiently unavailable.
        if self.config.entry_policy.mode == EntryPolicy::Immediate {
            let mut attempt: u8 = 0;
            loop {
                attempt = attempt.saturating_add(1);
                match self.try_proactive_immediate_signal().await {
                    Ok(signal) => {
                        self.event_bus.send(DaemonEvent::DetectorSignal(signal.clone()));
                        return Some(signal);
                    },
                    Err(error) => {
                        warn!(
                            flow = "entry_immediate",
                            attempt,
                            position_id = %self.config.position_id,
                            symbol = %self.config.symbol.as_pair(),
                            side = ?self.config.side,
                            error = %error,
                            "Immediate proactive fire failed"
                        );

                        if attempt >= 3 || !Self::is_transient_immediate_error(&error) {
                            warn!(
                                flow = "entry_immediate",
                                attempt,
                                position_id = %self.config.position_id,
                                symbol = %self.config.symbol.as_pair(),
                                side = ?self.config.side,
                                error = %error,
                                "Immediate proactive fire exhausted retries — falling back to reactive loop"
                            );
                            break;
                        }

                        sleep(std::time::Duration::from_millis(250)).await;
                    },
                }
            }
        }

        loop {
            // Cooperatively check for cancellation
            tokio::select! {
                // Wait for event bus message
                event_result = receiver.recv() => {
                    match event_result {
                        Some(Ok(event)) => {
                            if let Some(signal) = self.handle_event(event).await {
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

    /// Attempt an immediate signal using the last OHLCV candle close as entry
    /// price.
    ///
    /// Used by `run` for `Immediate` mode before entering the reactive event
    /// loop. Fetches candles from the OHLCV port, takes the last close as
    /// the reference price, and delegates to `create_signal` (which
    /// computes the technical stop).
    pub(crate) async fn try_proactive_immediate_signal(&self) -> DaemonResult<DetectorSignal> {
        let candles = self
            .ohlcv_port
            .fetch_candles(&self.config.symbol, CandleInterval::FifteenMinutes, 100)
            .await?;

        let last_close = candles
            .last()
            .ok_or_else(|| {
                DaemonError::Detector("no candles available for immediate entry".to_string())
            })?
            .close;

        info!(
            flow = "entry_immediate",
            position_id = %self.config.position_id,
            symbol = %self.config.symbol.as_pair(),
            side = ?self.config.side,
            reference_price = %last_close,
            "Immediate proactive fire: using last candle close as entry price"
        );

        let decision = SignalDecision::SignalConfirmed {
            side: self.config.side,
            reason: SignalReason::Immediate,
            observed_at: chrono::Utc::now(),
            reference_price: last_close,
        };

        self.create_signal(decision).await
    }

    fn is_transient_immediate_error(error: &DaemonError) -> bool {
        matches!(error, DaemonError::Detector(msg) if {
            let msg = msg.as_str();
            msg.contains("no candles available")
                || msg.contains("Insufficient candle data")
                || msg.contains("Fetch more history")
        })
    }

    /// Handle a single daemon event.
    ///
    /// Returns `Some(signal)` if detection triggered, `None` otherwise.
    async fn handle_event(&mut self, event: DaemonEvent) -> Option<DetectorSignal> {
        match event {
            DaemonEvent::MarketData(market_data) => self.handle_market_data(&market_data).await,
            DaemonEvent::Shutdown => {
                debug!(
                    position_id = %self.config.position_id,
                    "Detector received shutdown"
                );
                // Return None to trigger loop exit
                // Note: This doesn't directly exit, but we could restructure
                // to handle shutdown more explicitly if needed
                None
            },
            _ => {
                // Ignore other event types
                None
            },
        }
    }

    /// Handle market data event.
    ///
    /// Filters for our symbol and applies MA crossover detection logic.
    async fn handle_market_data(&mut self, market_data: &MarketData) -> Option<DetectorSignal> {
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

        match self.evaluate_signal(market_data).await {
            Ok(SignalDecision::NoSignal) => None,
            Ok(decision @ SignalDecision::SignalConfirmed { .. }) => {
                if let Some(event) = self.signal_strategy_evaluated_event(&decision, market_data) {
                    self.event_bus.send(DaemonEvent::DomainEvent(event));
                }

                match self.create_signal(decision).await {
                    Ok(signal) => Some(signal),
                    Err(error) => {
                        warn!(
                            position_id = %self.config.position_id,
                            symbol = %self.config.symbol.as_pair(),
                            error = %error,
                            "Detector could not compute technical stop"
                        );
                        None
                    },
                }
            },
            Err(error) => {
                warn!(
                    position_id = %self.config.position_id,
                    symbol = %self.config.symbol.as_pair(),
                    error = %error,
                    "Detector strategy evaluation failed"
                );
                None
            },
        }
    }

    async fn evaluate_signal(&self, market_data: &MarketData) -> DaemonResult<SignalDecision> {
        if self.config.entry_policy.mode == EntryPolicy::Immediate {
            return Ok(SignalDecision::SignalConfirmed {
                side: self.config.side,
                reason: SignalReason::Immediate,
                observed_at: market_data.timestamp,
                reference_price: market_data.price.as_decimal(),
            });
        }

        let strategy_id = StrategyRegistry::strategy_id_for_policy(self.config.entry_policy.mode)
            .ok_or_else(|| {
            DaemonError::Detector("entry policy has no signal strategy".to_string())
        })?;
        let strategy = self.strategy_registry.get(&strategy_id).ok_or_else(|| {
            DaemonError::Detector(format!("strategy not registered: {}", strategy_id))
        })?;
        let candles = self
            .ohlcv_port
            .fetch_candles(&self.config.symbol, CandleInterval::FifteenMinutes, 100)
            .await?;

        Ok(strategy.evaluate(SignalContext {
            position_id: self.config.position_id,
            symbol: self.config.symbol.clone(),
            side: self.config.side,
            candles,
            evaluated_at: market_data.timestamp,
        }))
    }

    fn signal_strategy_evaluated_event(
        &self,
        decision: &SignalDecision,
        market_data: &MarketData,
    ) -> Option<Event> {
        let strategy_id = StrategyRegistry::strategy_id_for_policy(self.config.entry_policy.mode)?;
        match decision {
            SignalDecision::NoSignal => Some(Event::SignalStrategyEvaluated {
                position_id: self.config.position_id,
                entry_policy: self.config.entry_policy.mode,
                strategy_id,
                outcome: SignalEvaluationOutcome::NoSignal,
                side: None,
                reason: None,
                observed_at: None,
                reference_price: None,
                timestamp: market_data.timestamp,
            }),
            SignalDecision::SignalConfirmed {
                side,
                reason,
                observed_at,
                reference_price,
            } => Some(Event::SignalStrategyEvaluated {
                position_id: self.config.position_id,
                entry_policy: self.config.entry_policy.mode,
                strategy_id,
                outcome: SignalEvaluationOutcome::SignalConfirmed,
                side: Some(*side),
                reason: Some(reason.to_string()),
                observed_at: Some(*observed_at),
                reference_price: Price::new(*reference_price).ok(),
                timestamp: market_data.timestamp,
            }),
        }
    }

    /// Create a DetectorSignal from a confirmed strategy decision.
    async fn create_signal(&self, decision: SignalDecision) -> DaemonResult<DetectorSignal> {
        let SignalDecision::SignalConfirmed { side, reference_price, .. } = decision else {
            return Err(DaemonError::Detector(
                "cannot create detector signal from NoSignal decision".to_string(),
            ));
        };
        let entry_price = Price::new(reference_price).map_err(|e| {
            DaemonError::Detector(format!("strategy reference price is invalid: {}", e))
        })?;

        let (analysis, guard_level) =
            self.compute_technical_stop(entry_price, side, &self.config.symbol).await?;

        // Shadow metadata: StopAnchor + StopQuality (ADR-0035, shadow-only).
        let stop_anchor = Self::build_stop_anchor(&analysis, side);
        let stop_quality_input =
            Self::build_stop_quality_input(entry_price, &analysis, stop_anchor.is_some());
        let classification =
            classify_stop_quality(&stop_quality_input, &StopQualityThresholds::default(), false);

        // Shadow telemetry: extract fields before moving into audit.
        let stop_anchor_present = stop_anchor.is_some();
        let anchor_type = stop_anchor.as_ref().map(|a| a.anchor_type);
        let stop_quality_class = classification.class;
        let raw_score = classification.raw_score;
        let boost_pct = classification.boost_pct;
        let shadow_exceptional = classification.shadow_exceptional;
        debug!(
            position_id = %self.config.position_id,
            symbol = %self.config.symbol.as_pair(),
            side = ?side,
            stop_anchor_present,
            anchor_type = ?anchor_type,
            stop_quality_class = ?stop_quality_class,
            raw_score,
            boost_pct = %boost_pct,
            shadow_exceptional,
            technical_stop_method = ?analysis.method,
            technical_stop_confidence = ?analysis.confidence,
            detected_levels_count = analysis.detected_levels.len(),
            "stop-aware entry shadow telemetry"
        );

        let mut audit =
            Self::build_technical_stop_audit(&analysis, &self.config.technical_stop_config);
        audit.stop_anchor = stop_anchor.map(Box::new);
        audit.stop_quality = Some(Box::new(classification));

        // ADR-0042 invalidation guard audit fields. When the guard is active,
        // record the raw analyzer stop, the sampled guard level, and which
        // level forms the effective basis before buffering. Disabled → all
        // three stay None (byte-identical to the historical audit).
        if let Some(guard) = guard_level {
            audit.raw_technical_stop = Some(analysis.stop_price);
            audit.invalidation_guard_level = Some(guard);
            audit.effective_stop_basis =
                Some(Self::effective_stop_basis(side, analysis.stop_price, guard));
        }

        Ok(DetectorSignal::new(
            self.config.position_id,
            self.config.symbol.clone(),
            side,
            entry_price,
            analysis.stop_price,
        )
        .with_technical_stop_analysis(audit))
    }

    /// Compute the chart-derived stop for the detector signal.
    ///
    /// Returns the analyzer result and, when the invalidation guard is
    /// enabled (ADR-0042), the sampled recent adverse extreme to clamp the
    /// effective stop beyond. The guard is `None` when disabled.
    async fn compute_technical_stop(
        &self,
        entry_price: Price,
        side: Side,
        symbol: &Symbol,
    ) -> DaemonResult<(TechnicalStopAnalysis, Option<Price>)> {
        let candles = self
            .ohlcv_port
            .fetch_candles(symbol, CandleInterval::FifteenMinutes, 100)
            .await?;
        let analysis = TechnicalStopAnalyzer::analyze(
            &candles,
            entry_price,
            side,
            &self.config.technical_stop_config,
        )
        .map_err(|e| DaemonError::Detector(e.to_string()))?;

        // ADR-0042 invalidation guard: sample the recent adverse extreme once
        // at signal time. Disabled → None (historical behavior).
        let guard_level = if self.stop_invalidation_guard_enabled {
            Self::recent_adverse_extreme(&candles, side, self.stop_invalidation_lookback_candles)
        } else {
            None
        };

        Ok((analysis, guard_level))
    }

    /// Highest high (short) / lowest low (long) over the last `lookback`
    /// candles, sampled once at signal time.
    ///
    /// # Constraint — the forming candle must be in the slice
    ///
    /// The window is taken from the TAIL of the fetched candles so it includes
    /// the forming (in-progress) candle. Binance `/fapi/v1/klines` returns the
    /// in-progress candle as the last element, so `fetch_candles(... 100)`
    /// satisfies this and "include current" is real. If a future OHLCV source
    /// returns only closed candles, an explicit current-candle high/low fetch
    /// must be added here before reading the tail window.
    fn recent_adverse_extreme(candles: &[Candle], side: Side, lookback: usize) -> Option<Price> {
        let extreme = match side {
            // SHORT: invalidation is a breakout ABOVE a recent high.
            Side::Short => candles.iter().rev().take(lookback.max(1)).map(|c| c.high).max(),
            // LONG: invalidation is a breakout BELOW a recent low.
            Side::Long => candles.iter().rev().take(lookback.max(1)).map(|c| c.low).min(),
        }?;
        Price::new(extreme).ok()
    }

    /// Which level forms the effective stop basis before buffering, mirroring
    /// the domain clamp in `effective_stop_price_with_guard`: the guard binds
    /// only when it lies beyond the technical stop on the adverse side.
    fn effective_stop_basis(side: Side, technical: Price, guard: Price) -> EffectiveStopBasis {
        let guard_binds = match side {
            Side::Short => guard.as_decimal() > technical.as_decimal(),
            Side::Long => guard.as_decimal() < technical.as_decimal(),
        };
        if guard_binds {
            EffectiveStopBasis::InvalidationGuard
        } else {
            EffectiveStopBasis::TechnicalStop
        }
    }

    fn build_technical_stop_audit(
        analysis: &TechnicalStopAnalysis,
        config: &TechnicalStopConfig,
    ) -> TechnicalStopAnalysisAudit {
        TechnicalStopAnalysisAudit {
            stop_price: analysis.stop_price,
            // Invalidation guard audit (ADR-0042): populated by `create_signal`
            // when the guard is active; None (default) here.
            raw_technical_stop: None,
            invalidation_guard_level: None,
            effective_stop_basis: None,
            method: Self::map_technical_stop_method(analysis.method),
            confidence: Self::map_technical_stop_confidence(analysis.confidence),
            detected_levels: analysis.detected_levels.clone(),
            config: TechnicalStopConfigSnapshot {
                min_candles: config.min_candles,
                swing_lookback: config.swing_lookback,
                support_level_n: config.support_level_n,
                level_tolerance: config.level_tolerance,
                atr_period: config.atr_period,
                atr_multiplier: config.atr_multiplier,
                min_stop_distance_pct: config.min_stop_distance_pct,
                max_stop_distance_pct: config.max_stop_distance_pct,
            },
            stop_anchor: None,
            stop_quality: None,
        }
    }

    fn map_technical_stop_method(
        method: AnalyzerTechnicalStopMethod,
    ) -> TechnicalStopMethodSnapshot {
        match method {
            AnalyzerTechnicalStopMethod::SwingPoint { level_n } => {
                TechnicalStopMethodSnapshot::SwingPoint { level_n }
            },
            AnalyzerTechnicalStopMethod::AtrFallback => TechnicalStopMethodSnapshot::AtrFallback,
        }
    }

    fn map_technical_stop_confidence(
        confidence: AnalyzerStopConfidence,
    ) -> TechnicalStopConfidenceSnapshot {
        match confidence {
            AnalyzerStopConfidence::High => TechnicalStopConfidenceSnapshot::High,
            AnalyzerStopConfidence::Medium => TechnicalStopConfidenceSnapshot::Medium,
            AnalyzerStopConfidence::Low => TechnicalStopConfidenceSnapshot::Low,
        }
    }

    /// Build StopAnchor metadata from the technical stop analysis.
    ///
    /// Only SwingPoint stops produce a structural anchor. AtrFallback has no
    /// real chart anchor, so it returns `None` (and `stop_anchor_valid = false`
    /// for the quality classifier).
    fn build_stop_anchor(analysis: &TechnicalStopAnalysis, side: Side) -> Option<StopAnchor> {
        match analysis.method {
            AnalyzerTechnicalStopMethod::SwingPoint { .. } => Some(StopAnchor {
                anchor_type: match side {
                    Side::Long => AnchorType::SwingLow,
                    Side::Short => AnchorType::SwingHigh,
                },
                anchor_price: analysis.stop_price,
                timeframe: "15m".to_string(),
                source_event_id: None,
                invalidation_reason: None,
            }),
            AnalyzerTechnicalStopMethod::AtrFallback => None,
        }
    }

    /// Build classifier input from the analysis result.
    ///
    /// `stop_anchor_valid` is `true` only for SwingPoint stops (structural
    /// anchor present). ATR fallback stops are valid protection mechanisms
    /// but lack an explicit structural anchor.
    fn build_stop_quality_input(
        entry_price: Price,
        analysis: &TechnicalStopAnalysis,
        stop_anchor_valid: bool,
    ) -> StopQualityInput {
        let distance_pct = (entry_price.as_decimal() - analysis.stop_price.as_decimal()).abs()
            / entry_price.as_decimal();
        StopQualityInput {
            stop_anchor_valid,
            method: analysis.method,
            confidence: analysis.confidence,
            detected_levels_count: analysis.detected_levels.len(),
            distance_pct,
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use chrono::{Duration, Utc};
    use robson_domain::{Candle, Side};
    use robson_exec::StubOhlcv;
    use rust_decimal::Decimal;
    use rust_decimal_macros::dec;
    use uuid::Uuid;

    use super::*;

    fn create_test_config() -> DetectorConfig {
        DetectorConfig {
            position_id: Uuid::now_v7(),
            symbol: robson_domain::Symbol::from_pair("BTCUSDT").unwrap(),
            side: Side::Long,
            ma_fast_period: 3, // Small for faster tests
            ma_slow_period: 5, // Small for faster tests
            technical_stop_config: TechnicalStopConfig::default(),
            entry_policy: EntryPolicyConfig::default(),
        }
    }

    fn create_test_ohlcv() -> Arc<dyn OhlcvPort> {
        Arc::new(StubOhlcv::new(create_test_candles()))
    }

    fn create_sma_crossover_ohlcv(side: Side) -> Arc<dyn OhlcvPort> {
        Arc::new(StubOhlcv::new(create_sma_crossover_candles(side)))
    }

    fn create_test_candles() -> Vec<Candle> {
        let symbol = robson_domain::Symbol::from_pair("BTCUSDT").unwrap();
        let now = Utc::now();
        let base = dec!(100);
        let mut candles: Vec<Candle> = (0..100)
            .map(|i| {
                let open_time = now + Duration::minutes(i);
                Candle::new(
                    symbol.clone(),
                    base,
                    base,
                    base,
                    base,
                    dec!(100),
                    10,
                    open_time,
                    open_time + Duration::minutes(15),
                )
            })
            .collect();

        candles[50] = Candle::new(
            symbol.clone(),
            base,
            dec!(105),
            dec!(95),
            base,
            dec!(100),
            10,
            now + Duration::minutes(50),
            now + Duration::minutes(65),
        );
        candles[70] = Candle::new(
            symbol,
            base,
            dec!(110),
            dec!(90),
            base,
            dec!(100),
            10,
            now + Duration::minutes(70),
            now + Duration::minutes(85),
        );

        candles
    }

    fn create_sma_crossover_candles(side: Side) -> Vec<Candle> {
        let symbol = robson_domain::Symbol::from_pair("BTCUSDT").unwrap();
        let now = Utc::now();
        let base = dec!(100);
        let mut candles: Vec<Candle> = (0..100)
            .map(|i| {
                let open_time = now + Duration::minutes(i);
                Candle::new(
                    symbol.clone(),
                    base,
                    base,
                    base,
                    base,
                    dec!(100),
                    10,
                    open_time,
                    open_time + Duration::minutes(15),
                )
            })
            .collect();

        let closes = match side {
            Side::Long => vec![dec!(100), dec!(99), dec!(98), dec!(97), dec!(96), dec!(105)],
            Side::Short => vec![
                dec!(100),
                dec!(101),
                dec!(102),
                dec!(103),
                dec!(104),
                dec!(95),
            ],
        };
        let start = candles.len() - closes.len();
        for (offset, close) in closes.into_iter().enumerate() {
            let index = start + offset;
            let open_time = now + Duration::minutes(index as i64);
            candles[index] = Candle::new(
                symbol.clone(),
                close,
                close,
                close,
                close,
                dec!(100),
                10,
                open_time,
                open_time + Duration::minutes(15),
            );
        }

        match side {
            Side::Long => {
                candles[50] = Candle::new(
                    symbol.clone(),
                    base,
                    dec!(104),
                    dec!(98),
                    base,
                    dec!(100),
                    10,
                    now + Duration::minutes(50),
                    now + Duration::minutes(65),
                );
                candles[70] = Candle::new(
                    symbol,
                    base,
                    dec!(104),
                    dec!(96),
                    base,
                    dec!(100),
                    10,
                    now + Duration::minutes(70),
                    now + Duration::minutes(85),
                );
            },
            Side::Short => {
                candles[50] = Candle::new(
                    symbol.clone(),
                    base,
                    dec!(102),
                    dec!(96),
                    base,
                    dec!(100),
                    10,
                    now + Duration::minutes(50),
                    now + Duration::minutes(65),
                );
                candles[70] = Candle::new(
                    symbol,
                    base,
                    dec!(104),
                    dec!(96),
                    base,
                    dec!(100),
                    10,
                    now + Duration::minutes(70),
                    now + Duration::minutes(85),
                );
            },
        }

        candles
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
        assert_eq!(config.technical_stop_config.min_candles, 100);
    }

    #[tokio::test]
    async fn test_compute_technical_stop_long() {
        let config = DetectorConfig {
            position_id: Uuid::now_v7(),
            symbol: robson_domain::Symbol::from_pair("BTCUSDT").unwrap(),
            side: Side::Long,
            ma_fast_period: 9,
            ma_slow_period: 21,
            technical_stop_config: TechnicalStopConfig::default(),
            entry_policy: EntryPolicyConfig::default(),
        };
        let event_bus = Arc::new(EventBus::new(10));
        let cancel_token = create_test_cancel_token();
        let detector = DetectorTask::new(config, event_bus, create_test_ohlcv(), cancel_token);

        let entry = Price::new(dec!(100)).unwrap();
        let (analysis, guard) = detector
            .compute_technical_stop(
                entry,
                Side::Long,
                &robson_domain::Symbol::from_pair("BTCUSDT").unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(analysis.stop_price.as_decimal(), dec!(90));
        assert_eq!(analysis.method, AnalyzerTechnicalStopMethod::SwingPoint { level_n: 2 });
        assert_eq!(analysis.confidence, AnalyzerStopConfidence::High);
        assert_eq!(analysis.detected_levels.len(), 2);
        // Guard disabled by default → no recent extreme sampled.
        assert!(guard.is_none());
    }

    #[tokio::test]
    async fn test_immediate_long_fires_proactively_without_market_tick() {
        let config = DetectorConfig {
            position_id: Uuid::now_v7(),
            symbol: robson_domain::Symbol::from_pair("BTCUSDT").unwrap(),
            side: Side::Long,
            ma_fast_period: 9,
            ma_slow_period: 21,
            technical_stop_config: TechnicalStopConfig::default(),
            entry_policy: EntryPolicyConfig::new(
                EntryPolicy::Immediate,
                robson_domain::ApprovalPolicy::Automatic,
            ),
        };
        let event_bus = Arc::new(EventBus::new(10));
        let mut signal_receiver = event_bus.subscribe();
        let detector_receiver = event_bus.subscribe();
        let detector = DetectorTask::new(
            config.clone(),
            Arc::clone(&event_bus),
            create_test_ohlcv(),
            create_test_cancel_token(),
        );

        let signal = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            detector.run(create_test_cancel_token(), detector_receiver),
        )
        .await
        .expect("immediate long detector must not wait for market data")
        .expect("immediate long detector must emit a signal");

        assert_eq!(signal.position_id, config.position_id);
        assert_eq!(signal.side, Side::Long);
        assert_eq!(signal.entry_price, Price::new(dec!(100)).unwrap());
        assert_eq!(signal.stop_loss, Price::new(dec!(90)).unwrap());

        let emitted =
            signal_receiver.recv().await.expect("signal event must be broadcast").unwrap();
        assert!(matches!(
            emitted,
            DaemonEvent::DetectorSignal(event_signal)
                if event_signal.position_id == config.position_id && event_signal.side == Side::Long
        ));
    }

    #[tokio::test]
    async fn test_compute_technical_stop_short() {
        let config = DetectorConfig {
            position_id: Uuid::now_v7(),
            symbol: robson_domain::Symbol::from_pair("BTCUSDT").unwrap(),
            side: Side::Short,
            ma_fast_period: 9,
            ma_slow_period: 21,
            technical_stop_config: TechnicalStopConfig::default(),
            entry_policy: EntryPolicyConfig::default(),
        };
        let event_bus = Arc::new(EventBus::new(10));
        let cancel_token = create_test_cancel_token();
        let detector = DetectorTask::new(config, event_bus, create_test_ohlcv(), cancel_token);

        let entry = Price::new(dec!(100)).unwrap();
        let (analysis, guard) = detector
            .compute_technical_stop(
                entry,
                Side::Short,
                &robson_domain::Symbol::from_pair("BTCUSDT").unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(analysis.stop_price.as_decimal(), dec!(110));
        assert_eq!(analysis.method, AnalyzerTechnicalStopMethod::SwingPoint { level_n: 2 });
        assert_eq!(analysis.confidence, AnalyzerStopConfidence::High);
        assert_eq!(analysis.detected_levels.len(), 2);
        // Guard disabled by default → no recent extreme sampled.
        assert!(guard.is_none());
    }

    #[tokio::test]
    async fn test_handle_market_data_filters_symbol() {
        let config = create_test_config(); // BTCUSDT
        let event_bus = Arc::new(EventBus::new(10));
        let cancel_token = create_test_cancel_token();
        let mut detector = DetectorTask::new(
            config,
            event_bus,
            create_sma_crossover_ohlcv(Side::Long),
            cancel_token,
        );

        // Different symbol should be ignored
        let other_data = create_test_market_data("ETHUSDT", dec!(3000));
        let result = detector.handle_market_data(&other_data).await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_ma_crossover_long_positive() {
        // Test MA crossover for Long position (fast crosses above slow)
        let config = DetectorConfig {
            position_id: Uuid::now_v7(),
            symbol: robson_domain::Symbol::from_pair("BTCUSDT").unwrap(),
            side: Side::Long,
            ma_fast_period: 3,
            ma_slow_period: 5,
            technical_stop_config: TechnicalStopConfig::default(),
            entry_policy: EntryPolicyConfig::default(),
        };
        let event_bus = Arc::new(EventBus::new(10));
        let cancel_token = create_test_cancel_token();
        let mut detector = DetectorTask::new(
            config,
            event_bus,
            create_sma_crossover_ohlcv(Side::Long),
            cancel_token,
        );

        // Feed prices where fast MA < slow MA (descending trend)
        // This establishes the "previous state"
        let prices_below = vec![
            dec!(100),
            dec!(99),
            dec!(98),
            dec!(97),
            dec!(96), // All descending
            dec!(95),
            dec!(94), // More data to establish state
        ];

        for price in prices_below {
            let data = create_test_market_data("BTCUSDT", price);
            detector.handle_market_data(&data).await;
        }

        // Now feed prices where fast MA crosses ABOVE slow MA (ascending trend)
        let prices_above = vec![dec!(96), dec!(98), dec!(100), dec!(102), dec!(104)];

        let mut result = None;
        for price in prices_above {
            let data = create_test_market_data("BTCUSDT", price);
            result = detector.handle_market_data(&data).await;
            if result.is_some() {
                break; // Found crossover
            }
        }

        // Should trigger signal on crossover
        assert!(result.is_some(), "Should signal on Long crossover");
        let signal = result.unwrap();
        assert_eq!(signal.side, Side::Long);
    }

    #[tokio::test]
    async fn test_ma_crossover_short_negative() {
        // Test MA crossover for Short position (fast crosses below slow)
        let config = DetectorConfig {
            position_id: Uuid::now_v7(),
            symbol: robson_domain::Symbol::from_pair("BTCUSDT").unwrap(),
            side: Side::Short,
            ma_fast_period: 3,
            ma_slow_period: 5,
            technical_stop_config: TechnicalStopConfig::default(),
            entry_policy: EntryPolicyConfig::default(),
        };
        let event_bus = Arc::new(EventBus::new(10));
        let cancel_token = create_test_cancel_token();
        let mut detector = DetectorTask::new(
            config,
            event_bus,
            create_sma_crossover_ohlcv(Side::Short),
            cancel_token,
        );

        // Feed prices where fast MA > slow MA (ascending trend)
        // This establishes the "previous state"
        let prices_above = vec![
            dec!(100),
            dec!(101),
            dec!(102),
            dec!(103),
            dec!(104), // All ascending
            dec!(105),
            dec!(106), // More data to establish state
        ];

        for price in prices_above {
            let data = create_test_market_data("BTCUSDT", price);
            detector.handle_market_data(&data).await;
        }

        // Now feed prices where fast MA crosses BELOW slow MA (descending trend)
        let prices_below = vec![dec!(104), dec!(102), dec!(100), dec!(98), dec!(96)];

        let mut result = None;
        for price in prices_below {
            let data = create_test_market_data("BTCUSDT", price);
            result = detector.handle_market_data(&data).await;
            if result.is_some() {
                break; // Found crossover
            }
        }

        // Should trigger signal on crossover
        assert!(result.is_some(), "Should signal on Short crossover");
        let signal = result.unwrap();
        assert_eq!(signal.side, Side::Short);
    }

    #[tokio::test]
    async fn test_ma_crossover_insufficient_data() {
        // Test that detector doesn't signal with insufficient data
        let config = DetectorConfig {
            position_id: Uuid::now_v7(),
            symbol: robson_domain::Symbol::from_pair("BTCUSDT").unwrap(),
            side: Side::Long,
            ma_fast_period: 9,
            ma_slow_period: 21,
            technical_stop_config: TechnicalStopConfig::default(),
            entry_policy: EntryPolicyConfig::default(),
        };
        let event_bus = Arc::new(EventBus::new(10));
        let cancel_token = create_test_cancel_token();
        let mut detector = DetectorTask::new(config, event_bus, create_test_ohlcv(), cancel_token);

        // Feed less than slow_period prices
        for i in 1..15 {
            let price = Decimal::from(100 + i);
            let data = create_test_market_data("BTCUSDT", price);
            let result = detector.handle_market_data(&data).await;
            assert!(result.is_none(), "Should not signal with insufficient data");
        }
    }

    #[tokio::test]
    async fn test_ma_crossover_no_signal_without_crossover() {
        // Test that being above/below doesn't trigger, only crossover
        let config = DetectorConfig {
            position_id: Uuid::now_v7(),
            symbol: robson_domain::Symbol::from_pair("BTCUSDT").unwrap(),
            side: Side::Long,
            ma_fast_period: 3,
            ma_slow_period: 5,
            technical_stop_config: TechnicalStopConfig::default(),
            entry_policy: EntryPolicyConfig::default(),
        };
        let event_bus = Arc::new(EventBus::new(10));
        let cancel_token = create_test_cancel_token();
        let mut detector = DetectorTask::new(config, event_bus, create_test_ohlcv(), cancel_token);

        // Feed prices where fast is above slow (already crossed)
        let prices = vec![dec!(110), dec!(110), dec!(110), dec!(100), dec!(100)];

        for price in prices {
            let data = create_test_market_data("BTCUSDT", price);
            let _result = detector.handle_market_data(&data).await;
            // No crossover occurred (fast was already above slow)
            // First tick establishes state, subsequent ticks check crossover
        }

        // Continue with same relative position - should not trigger
        let data = create_test_market_data("BTCUSDT", dec!(110));
        let result = detector.handle_market_data(&data).await;
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
            technical_stop_config: TechnicalStopConfig::default(),
            entry_policy: EntryPolicyConfig::default(),
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
        let detector = DetectorTask::new(
            config,
            Arc::clone(&event_bus),
            create_sma_crossover_ohlcv(Side::Long),
            cancel_token,
        );

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
        let result = tokio::time::timeout(std::time::Duration::from_secs(1), handle)
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
                },
                Some(Ok(_)) => continue,      // Other events
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
        let detector = DetectorTask::new(
            config,
            Arc::clone(&event_bus),
            create_sma_crossover_ohlcv(Side::Long),
            cancel_token,
        );
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
        let result = tokio::time::timeout(std::time::Duration::from_secs(1), handle)
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

        let detector = DetectorTask::new(
            config,
            Arc::clone(&event_bus),
            create_test_ohlcv(),
            cancel_token.clone(),
        );
        let handle = detector.spawn();

        tokio::task::yield_now().await;

        // Cancel the token
        cancel_token.cancel();

        // Wait for detector to finish
        let result = tokio::time::timeout(std::time::Duration::from_secs(1), handle)
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

        let detector = DetectorTask::new(
            config,
            Arc::clone(&event_bus),
            create_test_ohlcv(),
            cancel_token.clone(),
        );
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
        let result = tokio::time::timeout(std::time::Duration::from_secs(1), handle)
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
            let detector = DetectorTask::new(
                config,
                Arc::clone(&event_bus),
                create_test_ohlcv(),
                cancel_token.clone(),
            );
            let handle = detector.spawn();
            handles.push(handle);
        }

        tokio::task::yield_now().await;

        // Cancel all at once
        cancel_token.cancel();

        // Wait for all to finish
        for handle in handles {
            let result = tokio::time::timeout(std::time::Duration::from_secs(1), handle)
                .await
                .expect("Detector should finish quickly after cancellation")
                .expect("Detector task panicked");

            // All should return None (cancelled)
            assert!(result.is_none(), "Detector should not emit signal on cancellation");
        }
    }

    // =========================================================================
    // Stop-Aware Entry shadow metadata tests (Slice 003, ADR-0035)
    // =========================================================================

    fn make_swing_analysis(side: Side) -> TechnicalStopAnalysis {
        let stop_price = match side {
            Side::Long => Price::new(dec!(90)).unwrap(),
            Side::Short => Price::new(dec!(110)).unwrap(),
        };
        TechnicalStopAnalysis {
            stop_price,
            method: AnalyzerTechnicalStopMethod::SwingPoint { level_n: 2 },
            confidence: AnalyzerStopConfidence::High,
            detected_levels: match side {
                Side::Long => vec![Price::new(dec!(95)).unwrap(), Price::new(dec!(90)).unwrap()],
                Side::Short => {
                    vec![
                        Price::new(dec!(105)).unwrap(),
                        Price::new(dec!(110)).unwrap(),
                    ]
                },
            },
        }
    }

    fn make_atr_analysis() -> TechnicalStopAnalysis {
        TechnicalStopAnalysis {
            stop_price: Price::new(dec!(93)).unwrap(),
            method: AnalyzerTechnicalStopMethod::AtrFallback,
            confidence: AnalyzerStopConfidence::Low,
            detected_levels: vec![],
        }
    }

    #[test]
    fn build_stop_anchor_swing_point_long_returns_swing_low() {
        let analysis = make_swing_analysis(Side::Long);
        let anchor = DetectorTask::build_stop_anchor(&analysis, Side::Long);
        assert!(anchor.is_some());
        let anchor = anchor.unwrap();
        assert_eq!(anchor.anchor_type, AnchorType::SwingLow);
        assert_eq!(anchor.anchor_price, Price::new(dec!(90)).unwrap());
        assert_eq!(anchor.timeframe, "15m");
        assert!(anchor.source_event_id.is_none());
        assert!(anchor.invalidation_reason.is_none());
    }

    #[test]
    fn build_stop_anchor_swing_point_short_returns_swing_high() {
        let analysis = make_swing_analysis(Side::Short);
        let anchor = DetectorTask::build_stop_anchor(&analysis, Side::Short);
        assert!(anchor.is_some());
        let anchor = anchor.unwrap();
        assert_eq!(anchor.anchor_type, AnchorType::SwingHigh);
        assert_eq!(anchor.anchor_price, Price::new(dec!(110)).unwrap());
    }

    #[test]
    fn build_stop_anchor_atr_fallback_returns_none() {
        let analysis = make_atr_analysis();
        let anchor = DetectorTask::build_stop_anchor(&analysis, Side::Long);
        assert!(anchor.is_none());
    }

    #[test]
    fn build_stop_quality_input_atr_fallback_anchor_invalid() {
        let analysis = make_atr_analysis();
        let entry_price = Price::new(dec!(100)).unwrap();
        let input = DetectorTask::build_stop_quality_input(entry_price, &analysis, false);
        assert!(!input.stop_anchor_valid);
        assert_eq!(input.method, AnalyzerTechnicalStopMethod::AtrFallback);
        assert_eq!(input.confidence, AnalyzerStopConfidence::Low);
        assert_eq!(input.detected_levels_count, 0);
        assert_eq!(input.distance_pct, dec!(0.07));
    }

    #[tokio::test]
    async fn create_signal_populates_stop_quality_shadow() {
        use robson_engine::signal_strategy::SignalReason;

        let config = DetectorConfig {
            position_id: Uuid::now_v7(),
            symbol: robson_domain::Symbol::from_pair("BTCUSDT").unwrap(),
            side: Side::Long,
            ma_fast_period: 9,
            ma_slow_period: 21,
            technical_stop_config: TechnicalStopConfig::default(),
            entry_policy: EntryPolicyConfig::default(),
        };
        let event_bus = Arc::new(EventBus::new(10));
        let cancel_token = create_test_cancel_token();
        let detector = DetectorTask::new(config, event_bus, create_test_ohlcv(), cancel_token);

        let decision = SignalDecision::SignalConfirmed {
            side: Side::Long,
            reason: SignalReason::Immediate,
            observed_at: Utc::now(),
            reference_price: dec!(100),
        };

        let signal = detector.create_signal(decision).await.unwrap();
        let audit = signal.technical_stop_analysis.as_ref().expect("audit must be present");
        assert!(audit.stop_quality.is_some(), "stop_quality must be populated in shadow mode");
        assert!(audit.stop_anchor.is_some(), "stop_anchor must be populated for SwingPoint");
    }

    #[tokio::test]
    async fn create_signal_does_not_return_exceptional() {
        use robson_domain::StopQuality;
        use robson_engine::signal_strategy::SignalReason;

        let config = DetectorConfig {
            position_id: Uuid::now_v7(),
            symbol: robson_domain::Symbol::from_pair("BTCUSDT").unwrap(),
            side: Side::Long,
            ma_fast_period: 9,
            ma_slow_period: 21,
            technical_stop_config: TechnicalStopConfig::default(),
            entry_policy: EntryPolicyConfig::default(),
        };
        let event_bus = Arc::new(EventBus::new(10));
        let cancel_token = create_test_cancel_token();
        let detector = DetectorTask::new(config, event_bus, create_test_ohlcv(), cancel_token);

        let decision = SignalDecision::SignalConfirmed {
            side: Side::Long,
            reason: SignalReason::Immediate,
            observed_at: Utc::now(),
            reference_price: dec!(100),
        };

        let signal = detector.create_signal(decision).await.unwrap();
        let audit = signal.technical_stop_analysis.as_ref().unwrap();
        let sq = audit.stop_quality.as_ref().unwrap();
        assert_ne!(
            sq.class,
            StopQuality::Exceptional,
            "Exceptional must be impossible with exceptional_enabled=false"
        );
    }

    #[tokio::test]
    async fn create_signal_preserves_entry_and_stop_price() {
        use robson_engine::signal_strategy::SignalReason;

        let config = DetectorConfig {
            position_id: Uuid::now_v7(),
            symbol: robson_domain::Symbol::from_pair("BTCUSDT").unwrap(),
            side: Side::Long,
            ma_fast_period: 9,
            ma_slow_period: 21,
            technical_stop_config: TechnicalStopConfig::default(),
            entry_policy: EntryPolicyConfig::default(),
        };
        let event_bus = Arc::new(EventBus::new(10));
        let cancel_token = create_test_cancel_token();
        let detector = DetectorTask::new(config, event_bus, create_test_ohlcv(), cancel_token);

        let decision = SignalDecision::SignalConfirmed {
            side: Side::Long,
            reason: SignalReason::Immediate,
            observed_at: Utc::now(),
            reference_price: dec!(100),
        };

        let signal = detector.create_signal(decision).await.unwrap();
        assert_eq!(signal.entry_price, Price::new(dec!(100)).unwrap());
        assert_eq!(signal.stop_loss, Price::new(dec!(90)).unwrap());
    }

    // ADR-0042 Invalidation Guard Tests

    /// 100 candles whose last 20 carry the given recent high/low (the rest are
    /// flat at 100). The last element is the forming candle.
    fn create_guard_test_candles(recent_high: Decimal, recent_low: Decimal) -> Vec<Candle> {
        let symbol = robson_domain::Symbol::from_pair("BTCUSDT").unwrap();
        let now = Utc::now();
        (0..100)
            .map(|i| {
                let open_time = now + Duration::minutes(i);
                let (high, low) = if i >= 80 {
                    (recent_high, recent_low)
                } else {
                    (dec!(100), dec!(100))
                };
                Candle::new(
                    symbol.clone(),
                    dec!(100),
                    high,
                    low,
                    dec!(100),
                    dec!(100),
                    10,
                    open_time,
                    open_time + Duration::minutes(15),
                )
            })
            .collect()
    }

    #[test]
    fn recent_adverse_extreme_short_picks_highest_high() {
        let candles = create_guard_test_candles(dec!(120), dec!(80));
        assert_eq!(
            DetectorTask::recent_adverse_extreme(&candles, Side::Short, 20),
            Some(Price::new(dec!(120)).unwrap())
        );
    }

    #[test]
    fn recent_adverse_extreme_long_picks_lowest_low() {
        let candles = create_guard_test_candles(dec!(120), dec!(80));
        assert_eq!(
            DetectorTask::recent_adverse_extreme(&candles, Side::Long, 20),
            Some(Price::new(dec!(80)).unwrap())
        );
    }

    #[test]
    fn recent_adverse_extreme_includes_forming_candle() {
        // The forming candle (last element) carries the extreme; the lookback
        // window reaches it from the tail, so "include current" is real.
        let symbol = robson_domain::Symbol::from_pair("BTCUSDT").unwrap();
        let now = Utc::now();
        let candles: Vec<Candle> = (0..100)
            .map(|i| {
                let open_time = now + Duration::minutes(i);
                let high = if i == 99 { dec!(150) } else { dec!(100) };
                Candle::new(
                    symbol.clone(),
                    dec!(100),
                    high,
                    dec!(100),
                    dec!(100),
                    dec!(1),
                    1,
                    open_time,
                    open_time + Duration::minutes(15),
                )
            })
            .collect();
        assert_eq!(
            DetectorTask::recent_adverse_extreme(&candles, Side::Short, 20),
            Some(Price::new(dec!(150)).unwrap())
        );
    }

    #[test]
    fn effective_stop_basis_binds_only_when_guard_beyond_technical() {
        use robson_domain::entities::EffectiveStopBasis;

        // SHORT: guard binds when above the technical stop.
        let short_tech = Price::new(dec!(110)).unwrap();
        assert_eq!(
            DetectorTask::effective_stop_basis(
                Side::Short,
                short_tech,
                Price::new(dec!(115)).unwrap()
            ),
            EffectiveStopBasis::InvalidationGuard
        );
        assert_eq!(
            DetectorTask::effective_stop_basis(
                Side::Short,
                short_tech,
                Price::new(dec!(105)).unwrap()
            ),
            EffectiveStopBasis::TechnicalStop
        );

        // LONG: guard binds when below the technical stop.
        let long_tech = Price::new(dec!(90)).unwrap();
        assert_eq!(
            DetectorTask::effective_stop_basis(
                Side::Long,
                long_tech,
                Price::new(dec!(85)).unwrap()
            ),
            EffectiveStopBasis::InvalidationGuard
        );
        assert_eq!(
            DetectorTask::effective_stop_basis(
                Side::Long,
                long_tech,
                Price::new(dec!(95)).unwrap()
            ),
            EffectiveStopBasis::TechnicalStop
        );
    }

    #[tokio::test]
    async fn create_signal_carries_invalidation_guard_when_enabled() {
        use robson_domain::entities::EffectiveStopBasis;
        use robson_engine::signal_strategy::SignalReason;

        let config = DetectorConfig {
            position_id: Uuid::now_v7(),
            symbol: robson_domain::Symbol::from_pair("BTCUSDT").unwrap(),
            side: Side::Long,
            ma_fast_period: 9,
            ma_slow_period: 21,
            technical_stop_config: TechnicalStopConfig::default(),
            entry_policy: EntryPolicyConfig::default(),
        };
        let candles = create_test_candles();
        let ohlcv: Arc<dyn OhlcvPort> = Arc::new(StubOhlcv::new(candles.clone()));
        let event_bus = Arc::new(EventBus::new(10));
        let cancel_token = create_test_cancel_token();
        let detector = DetectorTask::new(config, event_bus, ohlcv, cancel_token)
            .with_invalidation_guard(true, 20);

        let decision = SignalDecision::SignalConfirmed {
            side: Side::Long,
            reason: SignalReason::Immediate,
            observed_at: Utc::now(),
            reference_price: dec!(100),
        };

        let signal = detector.create_signal(decision).await.unwrap();
        let audit = signal.technical_stop_analysis.as_ref().expect("audit present");

        // Guard enabled → the sampled recent low is recorded, plus the raw
        // technical stop and basis.
        let expected_guard = DetectorTask::recent_adverse_extreme(&candles, Side::Long, 20);
        assert_eq!(audit.invalidation_guard_level, expected_guard);
        assert_eq!(audit.raw_technical_stop, Some(signal.stop_loss));
        // The last 20 candles are flat at 100 (recent low 100), above the
        // technical stop (90) → the guard does not bind.
        assert_eq!(audit.effective_stop_basis, Some(EffectiveStopBasis::TechnicalStop));
    }
}
