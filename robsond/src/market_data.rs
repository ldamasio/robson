//! Market data manager for WebSocket integration.
//!
//! Spawns WebSocket client tasks and bridges market data events
//! from connectors to the daemon event bus.
//!
//! # Reconnection
//!
//! The spawned task runs indefinitely. When the WebSocket stream closes or
//! errors (Binance disconnects periodically — this is normal), the task waits
//! with exponential backoff (1 s → 2 s → 4 s … capped at 60 s) and reconnects.
//! The task only terminates on daemon shutdown (via `CancellationToken`).
//!
//! # REST fallback (ADR-0044)
//!
//! The WS path can fail silently: the connection opens but never delivers a
//! tick (2026-07-05: 45 watchdog reconnect cycles, every connection mute,
//! trailing frozen for ~1 h 50 while price crossed the advance target). Each
//! WS client records its last-tick instant in a shared [`FeedHealth`]; a
//! companion fallback task polls the exchange REST price while the feed is
//! silent past the watchdog threshold and emits into the **same**
//! `MarketData` pipeline, tagged [`MarketDataSource::RestFallback`]. The
//! trailing engine is source-agnostic; discrete-step trailing is a pure
//! function of the favorable extreme, so duplicate or interleaved delivery
//! during transitions cannot double-apply a step (property-tested in
//! `robson-engine/tests/trailing_stop_properties.rs`).

use std::{
    str::FromStr,
    sync::{
        atomic::{AtomicI64, Ordering},
        Arc,
    },
};

use async_trait::async_trait;
use robson_connectors::{BinanceWebSocketClient, WsMessage};
use robson_domain::{Price, Symbol};
use tokio::{
    task::JoinHandle,
    time::{sleep, Duration},
};
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use crate::{
    error::DaemonResult,
    event_bus::{DaemonEvent, EventBus, MarketDataSource},
};

/// Maximum reconnect backoff in seconds.
const MAX_BACKOFF_SECS: u64 = 60;

/// Read-idle watchdog threshold: a WS feed silent past this is treated as
/// dead (reconnect) and as the REST-fallback entry condition (ADR-0044).
pub(crate) const WATCHDOG_IDLE_SECS: u64 = 90;

pub(crate) fn next_reconnect_backoff_secs(current: u64) -> u64 {
    (current * 2).min(MAX_BACKOFF_SECS)
}

/// Shared per-symbol feed health: the instant of the last WS tick.
///
/// Written by the WS client task on every delivered tick; read by the REST
/// fallback task to decide whether the feed is silent. Initialized to "now"
/// at creation so a freshly booted daemon waits one full watchdog window
/// before engaging the fallback, mirroring the WS watchdog itself.
#[derive(Debug)]
pub struct FeedHealth {
    last_ws_tick_ms: AtomicI64,
}

impl FeedHealth {
    /// Create a health handle; the clock starts at "now".
    pub fn new() -> Self {
        Self {
            last_ws_tick_ms: AtomicI64::new(chrono::Utc::now().timestamp_millis()),
        }
    }

    /// Record a WS tick at the current instant.
    pub fn record_ws_tick(&self) {
        self.last_ws_tick_ms
            .store(chrono::Utc::now().timestamp_millis(), Ordering::Relaxed);
    }

    /// Seconds since the last WS tick (saturating at zero).
    pub fn silent_secs(&self) -> u64 {
        let last = self.last_ws_tick_ms.load(Ordering::Relaxed);
        let now = chrono::Utc::now().timestamp_millis();
        (now.saturating_sub(last).max(0) as u64) / 1000
    }

    #[cfg(test)]
    pub(crate) fn set_last_tick_secs_ago(&self, secs: i64) {
        let ms = chrono::Utc::now().timestamp_millis() - secs * 1000;
        self.last_ws_tick_ms.store(ms, Ordering::Relaxed);
    }
}

impl Default for FeedHealth {
    fn default() -> Self {
        Self::new()
    }
}

/// What the REST fallback needs from the composition root (ADR-0044).
///
/// One trait, two questions: a REST snapshot price, and whether the symbol
/// currently carries a risk-open position worth protecting. The daemon wires
/// this to the exchange port and the position store.
#[async_trait]
pub trait FallbackSupport: Send + Sync {
    /// Snapshot price via the REST path.
    async fn rest_price(&self, symbol: &Symbol) -> Result<Price, String>;

    /// Whether the symbol has an Entering or Active position. Implementations
    /// must fail protective: on lookup errors, return `true` so the fallback
    /// keeps polling rather than leaving a possible position blind.
    async fn has_risk_open(&self, symbol: &Symbol) -> bool;
}

/// REST fallback tuning (ADR-0044).
#[derive(Debug, Clone, Copy)]
pub struct RestFallbackConfig {
    /// Interval between REST polls while in fallback mode.
    pub poll_interval: Duration,
    /// WS silence that engages the fallback (matches the WS watchdog).
    pub silence_threshold: Duration,
    /// How long the WS must stay healthy before the fallback disengages.
    pub ws_holddown: Duration,
    /// Fallback persisting past this raises a recurring loud warning.
    pub alert_after: Duration,
}

impl Default for RestFallbackConfig {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_secs(5),
            silence_threshold: Duration::from_secs(WATCHDOG_IDLE_SECS),
            ws_holddown: Duration::from_secs(60),
            alert_after: Duration::from_secs(15 * 60),
        }
    }
}

impl RestFallbackConfig {
    /// Build from environment, falling back to ADR-0044 defaults.
    /// `ROBSON_REST_FALLBACK_POLL_SECS` overrides the poll interval.
    pub fn from_env() -> Self {
        let mut cfg = Self::default();
        if let Ok(v) = std::env::var("ROBSON_REST_FALLBACK_POLL_SECS") {
            if let Ok(secs) = v.parse::<u64>() {
                if secs > 0 {
                    cfg.poll_interval = Duration::from_secs(secs);
                }
            }
        }
        cfg
    }
}

/// Market data manager - spawns and manages WebSocket tasks.
pub struct MarketDataManager {
    /// Event bus for publishing market data
    event_bus: Arc<EventBus>,
    /// Cancellation token for graceful shutdown
    cancel: CancellationToken,
    /// Whether to connect to Binance testnet streams (mirrors
    /// ROBSON_BINANCE_USE_TESTNET)
    use_testnet: bool,
}

impl MarketDataManager {
    /// Create a new market data manager.
    pub fn new(event_bus: Arc<EventBus>, cancel: CancellationToken, use_testnet: bool) -> Self {
        Self { event_bus, cancel, use_testnet }
    }

    /// Spawn a WebSocket client task for a single symbol.
    ///
    /// The task runs indefinitely, reconnecting with exponential backoff when
    /// the stream closes or errors. It exits cleanly when the cancellation
    /// token is cancelled. Every delivered tick is recorded on `health` so
    /// the REST fallback task (ADR-0044) can observe feed silence.
    ///
    /// Returns a join handle that completes only on shutdown.
    pub fn spawn_ws_client(
        &self,
        symbol: Symbol,
        health: Arc<FeedHealth>,
    ) -> DaemonResult<JoinHandle<()>> {
        let event_bus = self.event_bus.clone();
        let cancel = self.cancel.clone();
        let symbol_str = symbol.as_pair();

        let use_testnet = self.use_testnet;
        let handle = tokio::spawn(async move {
            let ws_client = BinanceWebSocketClient::new(use_testnet);
            let mut backoff_secs: u64 = 1;

            'reconnect: loop {
                if cancel.is_cancelled() {
                    break;
                }

                let mut stream = match ws_client.subscribe_agg_trade(&symbol_str).await {
                    Ok(s) => {
                        info!(symbol = %symbol_str, "WebSocket client connected");
                        // Backoff resets only after first tick, not on connect, so that
                        // Binance accept-then-immediately-close loops still back off.
                        s
                    },
                    Err(e) => {
                        error!(
                            error = %e,
                            symbol = %symbol_str,
                            retry_in_secs = backoff_secs,
                            "WebSocket connect failed, retrying"
                        );
                        tokio::select! {
                            _ = sleep(Duration::from_secs(backoff_secs)) => {},
                            _ = cancel.cancelled() => break 'reconnect,
                        }
                        backoff_secs = next_reconnect_backoff_secs(backoff_secs);
                        continue 'reconnect;
                    },
                };

                let mut first_tick_logged = false;

                loop {
                    tokio::select! {
                        _ = cancel.cancelled() => {
                            info!(symbol = %symbol_str, "WebSocket client shutting down");
                            break 'reconnect;
                        }
                        // Read-idle watchdog: the keepalive ping is
                        // fire-and-forget (pongs are never awaited), so a
                        // half-open connection pends on next() forever with
                        // no error — a silent, stale feed while the soft
                        // stop goes blind (2026-07-03: the insurance stop
                        // fired 39s before the daemon noticed anything).
                        // An aggTrade stream on a traded symbol is never
                        // quiet for this long; treat silence as death.
                        msg = tokio::time::timeout(Duration::from_secs(WATCHDOG_IDLE_SECS), stream.next()) => {
                            let msg = match msg {
                                Err(_elapsed) => {
                                    warn!(
                                        symbol = %symbol_str,
                                        idle_secs = WATCHDOG_IDLE_SECS,
                                        retry_in_secs = backoff_secs,
                                        "Market data feed silent past watchdog; reconnecting"
                                    );
                                    break; // break inner loop → reconnect
                                },
                                Ok(msg) => msg,
                            };
                            match msg {
                                None => {
                                    warn!(
                                        symbol = %symbol_str,
                                        retry_in_secs = backoff_secs,
                                        "WebSocket stream closed, reconnecting"
                                    );
                                    break; // break inner loop → reconnect
                                },
                                Some(Err(e)) => {
                                    error!(
                                        error = %e,
                                        symbol = %symbol_str,
                                        retry_in_secs = backoff_secs,
                                        "WebSocket stream error, reconnecting"
                                    );
                                    break; // break inner loop → reconnect
                                },
                                Some(Ok(WsMessage::AggTrade(trade))) => {
                                    let price_decimal =
                                        match rust_decimal::Decimal::from_str(&trade.price) {
                                            Ok(d) => d,
                                            Err(e) => {
                                                error!(error = %e, "Failed to parse price");
                                                continue;
                                            },
                                        };

                                    let price = match Price::new(price_decimal) {
                                        Ok(p) => p,
                                        Err(e) => {
                                            error!(
                                                error = %e,
                                                price = %trade.price,
                                                "Invalid price value"
                                            );
                                            continue;
                                        },
                                    };

                                    if !first_tick_logged {
                                        info!(
                                            symbol = %trade.symbol,
                                            price = %price_decimal,
                                            "First tick received"
                                        );
                                        first_tick_logged = true;
                                        backoff_secs = 1; // stable connection confirmed
                                    }

                                    let trade_symbol = match Symbol::from_pair(&trade.symbol) {
                                        Ok(s) => s,
                                        Err(e) => {
                                            error!(
                                                error = %e,
                                                symbol = %trade.symbol,
                                                "Failed to parse symbol"
                                            );
                                            continue;
                                        },
                                    };

                                    health.record_ws_tick();
                                    let daemon_event =
                                        DaemonEvent::MarketData(crate::event_bus::MarketData {
                                            symbol: trade_symbol,
                                            price,
                                            timestamp: chrono::Utc::now(),
                                            source: MarketDataSource::Ws,
                                        });

                                    event_bus.send(daemon_event);
                                },
                                Some(Ok(_)) => {
                                    // Other message types not needed here
                                },
                            }
                        }
                    }
                }

                // Backoff before reconnect attempt
                tokio::select! {
                    _ = sleep(Duration::from_secs(backoff_secs)) => {},
                    _ = cancel.cancelled() => break 'reconnect,
                }
                backoff_secs = next_reconnect_backoff_secs(backoff_secs);
            }

            info!(symbol = %symbol_str, "WebSocket client task ended");
        });

        Ok(handle)
    }

    /// Spawn the REST fallback task for a single symbol (ADR-0044).
    ///
    /// The task wakes every `cfg.poll_interval`. While the WS feed is silent
    /// past `cfg.silence_threshold` AND the symbol carries a risk-open
    /// position, it fetches a REST snapshot price and emits it into the same
    /// `MarketData` pipeline, tagged `RestFallback`. It disengages only after
    /// the WS has stayed healthy for `cfg.ws_holddown` (hysteresis). A failed
    /// poll waits for the next interval — no burst retries (request budget,
    /// ADR-0044 §3).
    pub fn spawn_rest_fallback(
        &self,
        symbol: Symbol,
        health: Arc<FeedHealth>,
        support: Arc<dyn FallbackSupport>,
        cfg: RestFallbackConfig,
    ) -> JoinHandle<()> {
        let event_bus = self.event_bus.clone();
        let cancel = self.cancel.clone();
        let symbol_str = symbol.as_pair();

        tokio::spawn(async move {
            let mut in_fallback = false;
            let mut fallback_since: Option<tokio::time::Instant> = None;
            let mut ws_healthy_since: Option<tokio::time::Instant> = None;
            let mut last_persist_warn: Option<tokio::time::Instant> = None;

            loop {
                tokio::select! {
                    _ = cancel.cancelled() => {
                        info!(symbol = %symbol_str, "REST fallback task shutting down");
                        break;
                    }
                    _ = sleep(cfg.poll_interval) => {}
                }

                let silent = Duration::from_secs(health.silent_secs());
                crate::metrics::MARKET_DATA_SILENT_SECONDS
                    .with_label_values(&[&symbol_str])
                    .set(silent.as_secs() as f64);

                // A tick within ~2 poll intervals means the WS is delivering.
                let ws_delivering = silent <= cfg.poll_interval * 2;

                if in_fallback {
                    // Hysteresis: leave only after the WS stays healthy for
                    // the full hold-down window.
                    if ws_delivering {
                        let healthy_since =
                            *ws_healthy_since.get_or_insert_with(tokio::time::Instant::now);
                        if healthy_since.elapsed() >= cfg.ws_holddown {
                            info!(
                                symbol = %symbol_str,
                                "WS feed healthy past hold-down; leaving REST fallback"
                            );
                            in_fallback = false;
                            fallback_since = None;
                            ws_healthy_since = None;
                            last_persist_warn = None;
                            crate::metrics::MARKET_DATA_MODE
                                .with_label_values(&[&symbol_str])
                                .set(0.0);
                            continue;
                        }
                    } else {
                        ws_healthy_since = None;
                    }

                    // A position that closed no longer needs protection.
                    if !support.has_risk_open(&symbol).await {
                        info!(
                            symbol = %symbol_str,
                            "No risk-open position remains; leaving REST fallback"
                        );
                        in_fallback = false;
                        fallback_since = None;
                        ws_healthy_since = None;
                        last_persist_warn = None;
                        crate::metrics::MARKET_DATA_MODE.with_label_values(&[&symbol_str]).set(0.0);
                        continue;
                    }
                } else {
                    if silent < cfg.silence_threshold {
                        continue;
                    }
                    if !support.has_risk_open(&symbol).await {
                        // Feed is silent but nothing needs protection; keep
                        // observing without spending the request budget.
                        continue;
                    }
                    warn!(
                        symbol = %symbol_str,
                        silent_secs = silent.as_secs(),
                        poll_secs = cfg.poll_interval.as_secs(),
                        "WS feed silent past watchdog; entering REST fallback (ADR-0044)"
                    );
                    in_fallback = true;
                    fallback_since = Some(tokio::time::Instant::now());
                    ws_healthy_since = None;
                    crate::metrics::MARKET_DATA_MODE.with_label_values(&[&symbol_str]).set(1.0);
                }

                // In fallback: one poll per interval, no burst retries.
                match support.rest_price(&symbol).await {
                    Ok(price) => {
                        crate::metrics::MARKET_DATA_FALLBACK_POLLS
                            .with_label_values(&[&symbol_str, "ok"])
                            .inc();
                        event_bus.send(DaemonEvent::MarketData(crate::event_bus::MarketData {
                            symbol: symbol.clone(),
                            price,
                            timestamp: chrono::Utc::now(),
                            source: MarketDataSource::RestFallback,
                        }));
                    },
                    Err(e) => {
                        crate::metrics::MARKET_DATA_FALLBACK_POLLS
                            .with_label_values(&[&symbol_str, "error"])
                            .inc();
                        warn!(
                            symbol = %symbol_str,
                            error = %e,
                            "REST fallback price poll failed; retrying next interval"
                        );
                    },
                }

                // Fallback is a state to leave, not a home: nag loudly while
                // it persists past the alert threshold.
                if let Some(since) = fallback_since {
                    let persisted = since.elapsed();
                    let nag_due = last_persist_warn
                        .map(|t| t.elapsed() >= Duration::from_secs(300))
                        .unwrap_or(true);
                    if persisted >= cfg.alert_after && nag_due {
                        warn!(
                            symbol = %symbol_str,
                            fallback_minutes = persisted.as_secs() / 60,
                            "REST fallback persisting; WS feed still silent (investigate)"
                        );
                        last_persist_warn = Some(tokio::time::Instant::now());
                    }
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use rust_decimal_macros::dec;

    use super::*;

    #[test]
    fn test_market_data_manager_creation() {
        let event_bus = Arc::new(EventBus::new(100));
        let cancel = CancellationToken::new();
        let _manager = MarketDataManager::new(event_bus, cancel, false);
        // Manager is created successfully
    }

    #[test]
    fn feed_health_starts_healthy_and_tracks_ticks() {
        let health = FeedHealth::new();
        assert!(health.silent_secs() < 2, "fresh health handle must not read as silent");

        health.set_last_tick_secs_ago(120);
        assert!(health.silent_secs() >= 119, "must report silence since the last tick");

        health.record_ws_tick();
        assert!(health.silent_secs() < 2, "recording a tick must reset silence");
    }

    #[test]
    fn rest_fallback_config_env_override() {
        // Default when unset/invalid.
        std::env::remove_var("ROBSON_REST_FALLBACK_POLL_SECS");
        assert_eq!(RestFallbackConfig::from_env().poll_interval, Duration::from_secs(5));

        std::env::set_var("ROBSON_REST_FALLBACK_POLL_SECS", "0");
        assert_eq!(
            RestFallbackConfig::from_env().poll_interval,
            Duration::from_secs(5),
            "zero must not disable polling"
        );

        std::env::set_var("ROBSON_REST_FALLBACK_POLL_SECS", "9");
        assert_eq!(RestFallbackConfig::from_env().poll_interval, Duration::from_secs(9));
        std::env::remove_var("ROBSON_REST_FALLBACK_POLL_SECS");
    }

    struct StubSupport {
        price: Price,
        risk_open: bool,
    }

    #[async_trait]
    impl FallbackSupport for StubSupport {
        async fn rest_price(&self, _symbol: &Symbol) -> Result<Price, String> {
            Ok(self.price)
        }

        async fn has_risk_open(&self, _symbol: &Symbol) -> bool {
            self.risk_open
        }
    }

    fn test_cfg() -> RestFallbackConfig {
        RestFallbackConfig {
            poll_interval: Duration::from_millis(20),
            silence_threshold: Duration::from_secs(90),
            ws_holddown: Duration::from_millis(100),
            alert_after: Duration::from_secs(900),
        }
    }

    #[tokio::test]
    async fn rest_fallback_emits_while_ws_silent() {
        let event_bus = Arc::new(EventBus::new(100));
        let mut receiver = event_bus.subscribe();
        let cancel = CancellationToken::new();
        let manager = MarketDataManager::new(event_bus, cancel.clone(), false);

        let health = Arc::new(FeedHealth::new());
        health.set_last_tick_secs_ago(120); // silent past the 90s threshold

        let support = Arc::new(StubSupport {
            price: Price::new(dec!(62700)).unwrap(),
            risk_open: true,
        });
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let handle = manager.spawn_rest_fallback(symbol, health, support, test_cfg());

        let event = tokio::time::timeout(Duration::from_secs(2), receiver.recv())
            .await
            .expect("fallback must emit within the timeout")
            .expect("receiver open")
            .expect("no lag");
        match event {
            DaemonEvent::MarketData(data) => {
                assert_eq!(data.source, MarketDataSource::RestFallback);
                assert_eq!(data.price.as_decimal(), dec!(62700));
            },
            other => panic!("expected MarketData, got {:?}", other),
        }

        cancel.cancel();
        let _ = handle.await;
    }

    #[tokio::test]
    async fn rest_fallback_stays_quiet_while_ws_healthy() {
        let event_bus = Arc::new(EventBus::new(100));
        let mut receiver = event_bus.subscribe();
        let cancel = CancellationToken::new();
        let manager = MarketDataManager::new(event_bus, cancel.clone(), false);

        let health = Arc::new(FeedHealth::new()); // fresh = healthy
        let support = Arc::new(StubSupport {
            price: Price::new(dec!(62700)).unwrap(),
            risk_open: true,
        });
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let handle = manager.spawn_rest_fallback(symbol, health, support, test_cfg());

        let got = tokio::time::timeout(Duration::from_millis(200), receiver.recv()).await;
        assert!(got.is_err(), "healthy WS must produce no fallback emissions");

        cancel.cancel();
        let _ = handle.await;
    }

    #[tokio::test]
    async fn rest_fallback_respects_position_gate() {
        let event_bus = Arc::new(EventBus::new(100));
        let mut receiver = event_bus.subscribe();
        let cancel = CancellationToken::new();
        let manager = MarketDataManager::new(event_bus, cancel.clone(), false);

        let health = Arc::new(FeedHealth::new());
        health.set_last_tick_secs_ago(120); // silent, but nothing to protect

        let support = Arc::new(StubSupport {
            price: Price::new(dec!(62700)).unwrap(),
            risk_open: false,
        });
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let handle = manager.spawn_rest_fallback(symbol, health, support, test_cfg());

        let got = tokio::time::timeout(Duration::from_millis(200), receiver.recv()).await;
        assert!(got.is_err(), "no risk-open position must mean no polling");

        cancel.cancel();
        let _ = handle.await;
    }
}
