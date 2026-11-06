//! Market data manager for WebSocket integration.
//!
//! Spawns WebSocket client tasks and bridges market data events
//! from connectors to the daemon event bus.
//!
//! A manager serves one venue: its feed comes from the venue factory (ADR-0056
//! §3) and every event it publishes carries that venue. The reconnect loop, the
//! watchdog and the REST fallback below are venue neutral, and each
//! `(venue, symbol)` pair runs its own instance with its own [`FeedHealth`].
//!
//! # Reconnection
//!
//! The spawned task runs indefinitely. When the WebSocket stream closes or
//! errors, the task rotates through configured endpoints and waits with
//! exponential backoff. The cap is 60 s normally and expands to 15 min while
//! the REST fallback is healthy, avoiding reconnect churn during a persistent
//! WS outage. The task only terminates on daemon shutdown.
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

use std::sync::{
    atomic::{AtomicBool, AtomicI64, Ordering},
    Arc,
};

use async_trait::async_trait;
#[cfg(test)]
use robson_connectors::binance::usdm::{
    BinanceUsdmTradeFeed, MAINNET_MARKET_WS_ENDPOINT, TESTNET_MARKET_WS_ENDPOINT,
};
use robson_domain::{Price, Symbol};
use robson_exec::{venue::VenueId, TradeFeedMessage, TradeFeedPort};
use tokio::{
    task::JoinHandle,
    time::{sleep, Duration},
};
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use crate::{
    error::{DaemonError, DaemonResult},
    event_bus::{DaemonEvent, EventBus, MarketDataSource},
};

/// Maximum reconnect backoff when no healthy REST fallback is protecting the
/// symbol.
const MAX_RECONNECT_BACKOFF_SECS: u64 = 60;

/// Maximum reconnect backoff while REST fallback polls are healthy.
const MAX_DEGRADED_RECONNECT_BACKOFF_SECS: u64 = 15 * 60;

/// Maximum time allowed for the TCP, TLS, and HTTP upgrade attempt.
const WS_CONNECT_TIMEOUT_SECS: u64 = 10;

/// Read-idle watchdog threshold: a WS feed silent past this is treated as
/// dead (reconnect) and as the REST-fallback entry condition (ADR-0044).
pub(crate) const WATCHDOG_IDLE_SECS: u64 = 90;

pub(crate) fn next_reconnect_backoff_secs(current: u64) -> u64 {
    next_reconnect_backoff_secs_for_health(current, false)
}

fn next_reconnect_backoff_secs_for_health(current: u64, rest_fallback_healthy: bool) -> u64 {
    let cap = if rest_fallback_healthy {
        MAX_DEGRADED_RECONNECT_BACKOFF_SECS
    } else {
        MAX_RECONNECT_BACKOFF_SECS
    };
    current.saturating_mul(2).min(cap)
}

fn reconnect_delay_secs(current: u64, rest_fallback_healthy: bool) -> u64 {
    let cap = if rest_fallback_healthy {
        MAX_DEGRADED_RECONNECT_BACKOFF_SECS
    } else {
        MAX_RECONNECT_BACKOFF_SECS
    };
    current.min(cap)
}

#[derive(Debug)]
struct WsReconnectState {
    endpoint_index: usize,
    backoff_secs: u64,
}

impl WsReconnectState {
    fn new() -> Self {
        Self { endpoint_index: 0, backoff_secs: 1 }
    }

    fn retry_in_secs(&self, rest_fallback_healthy: bool) -> u64 {
        reconnect_delay_secs(self.backoff_secs, rest_fallback_healthy)
    }

    fn next_endpoint_index(&self, endpoint_count: usize) -> usize {
        (self.endpoint_index + 1) % endpoint_count
    }

    fn record_failure(&mut self, endpoint_count: usize, rest_fallback_healthy: bool) {
        self.endpoint_index = self.next_endpoint_index(endpoint_count);
        self.backoff_secs = if rest_fallback_healthy {
            next_reconnect_backoff_secs_for_health(self.backoff_secs, true)
        } else {
            next_reconnect_backoff_secs(self.backoff_secs)
        };
    }

    fn record_first_message(&mut self) {
        self.backoff_secs = 1;
    }
}

#[derive(Debug)]
enum WsAttemptFailure {
    Connect(String),
    ConnectTimeout,
    HandshakeNoData,
    FeedIdle,
    ClosedBeforeData,
    Closed,
    StreamBeforeData(String),
    Stream(String),
}

impl WsAttemptFailure {
    fn metric_reason(&self) -> &'static str {
        match self {
            Self::Connect(_) => "connect",
            Self::ConnectTimeout => "connect_timeout",
            Self::HandshakeNoData => "handshake_no_data",
            Self::FeedIdle => "idle",
            Self::ClosedBeforeData => "closed_before_data",
            Self::Closed => "closed",
            Self::StreamBeforeData(_) => "stream_error_before_data",
            Self::Stream(_) => "stream_error",
        }
    }

    fn error(&self) -> Option<&str> {
        match self {
            Self::Connect(error) | Self::StreamBeforeData(error) | Self::Stream(error) => {
                Some(error)
            },
            _ => None,
        }
    }
}

fn watchdog_failure(first_message_received: bool) -> WsAttemptFailure {
    if first_message_received {
        WsAttemptFailure::FeedIdle
    } else {
        WsAttemptFailure::HandshakeNoData
    }
}

fn fallback_persist_warning_due(
    persisted: Duration,
    since_last_warning: Option<Duration>,
    alert_interval: Duration,
) -> bool {
    persisted >= alert_interval
        && since_last_warning.map(|elapsed| elapsed >= alert_interval).unwrap_or(true)
}

/// Shared per-feed health: the instant of the last WS tick.
///
/// Written by the WS client task on every delivered tick; read by the REST
/// fallback task to decide whether the feed is silent. Initialized to "now"
/// at creation so a freshly booted daemon waits one full watchdog window
/// before engaging the fallback, mirroring the WS watchdog itself.
#[derive(Debug)]
pub struct FeedHealth {
    last_ws_tick_ms: AtomicI64,
    rest_fallback_healthy: AtomicBool,
}

impl FeedHealth {
    /// Create a health handle; the clock starts at "now".
    pub fn new() -> Self {
        Self {
            last_ws_tick_ms: AtomicI64::new(chrono::Utc::now().timestamp_millis()),
            rest_fallback_healthy: AtomicBool::new(false),
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

    /// Whether the latest REST fallback poll succeeded while fallback mode is
    /// active. The WS task uses this only to widen its reconnect backoff.
    pub fn rest_fallback_healthy(&self) -> bool {
        self.rest_fallback_healthy.load(Ordering::Relaxed)
    }

    fn set_rest_fallback_healthy(&self, healthy: bool) {
        self.rest_fallback_healthy.store(healthy, Ordering::Relaxed);
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

/// Market data manager - spawns and manages WebSocket tasks for one venue.
pub struct MarketDataManager {
    /// Event bus for publishing market data
    event_bus: Arc<EventBus>,
    /// Cancellation token for graceful shutdown
    cancel: CancellationToken,
    /// Venue stamped on every published event.
    venue: VenueId,
    /// The venue's trade feed.
    feed: Arc<dyn TradeFeedPort>,
    /// Ordered stream bases used by each per-symbol reconnect loop.
    ws_endpoints: Arc<Vec<String>>,
}

impl MarketDataManager {
    /// Create a market data manager for `venue` over its feed.
    pub fn for_venue(
        event_bus: Arc<EventBus>,
        cancel: CancellationToken,
        venue: VenueId,
        feed: Arc<dyn TradeFeedPort>,
    ) -> Self {
        let ws_endpoints = Arc::new(feed.endpoints().to_vec());
        Self {
            event_bus,
            cancel,
            venue,
            feed,
            ws_endpoints,
        }
    }

    /// Test fixture: the USDⓈ-M feed with its environment defaults.
    #[cfg(test)]
    fn new(
        event_bus: Arc<EventBus>,
        cancel: CancellationToken,
        use_testnet: bool,
        configured_ws_endpoints: Vec<String>,
    ) -> Self {
        Self::for_venue(
            event_bus,
            cancel,
            VenueId::new(VenueId::BINANCE_USDM),
            Arc::new(BinanceUsdmTradeFeed::new(use_testnet, configured_ws_endpoints)),
        )
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
        if self.ws_endpoints.is_empty() {
            return Err(DaemonError::Config(format!(
                "Market data feed for venue {} has no endpoints",
                self.venue
            )));
        }

        let event_bus = self.event_bus.clone();
        let cancel = self.cancel.clone();
        let symbol_str = symbol.as_pair();
        let venue = self.venue.clone();
        let venue_str = venue.to_string();
        let feed = Arc::clone(&self.feed);

        let ws_endpoints = Arc::clone(&self.ws_endpoints);
        let handle = tokio::spawn(async move {
            let mut reconnect = WsReconnectState::new();

            'reconnect: loop {
                if cancel.is_cancelled() {
                    break;
                }

                let endpoint = ws_endpoints[reconnect.endpoint_index].clone();

                let failure = 'attempt: {
                    let subscription = tokio::select! {
                        _ = cancel.cancelled() => break 'reconnect,
                        result = tokio::time::timeout(
                            Duration::from_secs(WS_CONNECT_TIMEOUT_SECS),
                            feed.connect(&endpoint, &symbol),
                        ) => result,
                    };
                    let mut stream = match subscription {
                        Ok(Ok(stream)) => stream,
                        Ok(Err(error)) => break 'attempt WsAttemptFailure::Connect(error),
                        Err(_elapsed) => break 'attempt WsAttemptFailure::ConnectTimeout,
                    };
                    let mut first_message_received = false;

                    loop {
                        tokio::select! {
                            _ = cancel.cancelled() => {
                                info!(venue = %venue_str, symbol = %symbol_str, "WebSocket client shutting down");
                                break 'reconnect;
                            }
                            // Read-idle watchdog: a successful HTTP upgrade is
                            // not a healthy feed. Only a delivered stream message
                            // confirms that the endpoint is connected.
                            msg = tokio::time::timeout(
                                Duration::from_secs(WATCHDOG_IDLE_SECS),
                                stream.next_message(),
                            ) => {
                                let msg = match msg {
                                    Err(_elapsed) => {
                                        break 'attempt watchdog_failure(first_message_received);
                                    },
                                    Ok(msg) => msg,
                                };

                                let message = match msg {
                                    None if first_message_received => {
                                        break 'attempt WsAttemptFailure::Closed;
                                    },
                                    None => {
                                        break 'attempt WsAttemptFailure::ClosedBeforeData;
                                    },
                                    Some(Err(error)) if first_message_received => {
                                        break 'attempt WsAttemptFailure::Stream(error);
                                    },
                                    Some(Err(error)) => {
                                        break 'attempt WsAttemptFailure::StreamBeforeData(error);
                                    },
                                    Some(Ok(message)) => message,
                                };

                                if !first_message_received {
                                    info!(
                                        venue = %venue_str,
                                        symbol = %symbol_str,
                                        endpoint = %endpoint,
                                        "WebSocket client connected; first stream message received"
                                    );
                                    first_message_received = true;
                                    reconnect.record_first_message();
                                }

                                match message {
                                    TradeFeedMessage::Trade { symbol: trade_symbol, price } => {
                                        health.record_ws_tick();
                                        let daemon_event =
                                            DaemonEvent::MarketData(crate::event_bus::MarketData {
                                                venue: venue.clone(),
                                                symbol: trade_symbol,
                                                price,
                                                timestamp: chrono::Utc::now(),
                                                source: MarketDataSource::Ws,
                                            });

                                        event_bus.send(daemon_event);
                                    },
                                    TradeFeedMessage::Malformed { reason, value, error } => {
                                        error!(error = %error, value = %value, "{}", reason);
                                    },
                                    TradeFeedMessage::NonTrade => {},
                                }
                            }
                        }
                    }
                };

                let rest_fallback_healthy = health.rest_fallback_healthy();
                let retry_in_secs = reconnect.retry_in_secs(rest_fallback_healthy);
                let next_endpoint =
                    &ws_endpoints[reconnect.next_endpoint_index(ws_endpoints.len())];
                crate::metrics::MARKET_DATA_WS_FAILURES
                    .with_label_values(&[
                        venue_str.as_str(),
                        symbol_str.as_str(),
                        endpoint.as_str(),
                        failure.metric_reason(),
                    ])
                    .inc();

                match (&failure, rest_fallback_healthy) {
                    (WsAttemptFailure::HandshakeNoData, true) => info!(
                        venue = %venue_str,
                        symbol = %symbol_str,
                        endpoint = %endpoint,
                        next_endpoint = %next_endpoint,
                        idle_secs = WATCHDOG_IDLE_SECS,
                        retry_in_secs,
                        "WebSocket handshake succeeded but stream delivered no data; REST fallback healthy"
                    ),
                    (WsAttemptFailure::HandshakeNoData, false) => warn!(
                        venue = %venue_str,
                        symbol = %symbol_str,
                        endpoint = %endpoint,
                        next_endpoint = %next_endpoint,
                        idle_secs = WATCHDOG_IDLE_SECS,
                        retry_in_secs,
                        "WebSocket handshake succeeded but stream delivered no data; reconnecting"
                    ),
                    (_, true) => info!(
                        venue = %venue_str,
                        symbol = %symbol_str,
                        endpoint = %endpoint,
                        next_endpoint = %next_endpoint,
                        failure_reason = failure.metric_reason(),
                        error = failure.error().unwrap_or(""),
                        retry_in_secs,
                        "WebSocket endpoint failed; REST fallback healthy"
                    ),
                    (_, false) => warn!(
                        venue = %venue_str,
                        symbol = %symbol_str,
                        endpoint = %endpoint,
                        next_endpoint = %next_endpoint,
                        failure_reason = failure.metric_reason(),
                        error = failure.error().unwrap_or(""),
                        retry_in_secs,
                        "WebSocket endpoint failed; reconnecting"
                    ),
                }

                reconnect.record_failure(ws_endpoints.len(), rest_fallback_healthy);

                // Backoff before reconnect attempt
                tokio::select! {
                    _ = sleep(Duration::from_secs(retry_in_secs)) => {},
                    _ = cancel.cancelled() => break 'reconnect,
                }
            }

            info!(venue = %venue_str, symbol = %symbol_str, "WebSocket client task ended");
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
        let venue = self.venue.clone();
        let venue_str = venue.to_string();

        tokio::spawn(async move {
            let mut in_fallback = false;
            let mut fallback_since: Option<tokio::time::Instant> = None;
            let mut ws_healthy_since: Option<tokio::time::Instant> = None;
            let mut last_persist_warn: Option<tokio::time::Instant> = None;

            loop {
                tokio::select! {
                    _ = cancel.cancelled() => {
                        health.set_rest_fallback_healthy(false);
                        info!(venue = %venue_str, symbol = %symbol_str, "REST fallback task shutting down");
                        break;
                    }
                    _ = sleep(cfg.poll_interval) => {}
                }

                let silent = Duration::from_secs(health.silent_secs());
                crate::metrics::MARKET_DATA_SILENT_SECONDS
                    .with_label_values(&[&venue_str, &symbol_str])
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
                                venue = %venue_str,
                                symbol = %symbol_str,
                                "WS feed healthy past hold-down; leaving REST fallback"
                            );
                            in_fallback = false;
                            fallback_since = None;
                            ws_healthy_since = None;
                            last_persist_warn = None;
                            health.set_rest_fallback_healthy(false);
                            crate::metrics::MARKET_DATA_MODE
                                .with_label_values(&[&venue_str, &symbol_str])
                                .set(0.0);
                            continue;
                        }
                    } else {
                        ws_healthy_since = None;
                    }

                    // A position that closed no longer needs protection.
                    if !support.has_risk_open(&symbol).await {
                        info!(
                            venue = %venue_str,
                            symbol = %symbol_str,
                            "No risk-open position remains; leaving REST fallback"
                        );
                        in_fallback = false;
                        fallback_since = None;
                        ws_healthy_since = None;
                        last_persist_warn = None;
                        health.set_rest_fallback_healthy(false);
                        crate::metrics::MARKET_DATA_MODE
                            .with_label_values(&[&venue_str, &symbol_str])
                            .set(0.0);
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
                        venue = %venue_str,
                        symbol = %symbol_str,
                        silent_secs = silent.as_secs(),
                        poll_secs = cfg.poll_interval.as_secs(),
                        "WS feed silent past watchdog; entering REST fallback (ADR-0044)"
                    );
                    in_fallback = true;
                    fallback_since = Some(tokio::time::Instant::now());
                    ws_healthy_since = None;
                    health.set_rest_fallback_healthy(false);
                    crate::metrics::MARKET_DATA_MODE
                        .with_label_values(&[&venue_str, &symbol_str])
                        .set(1.0);
                }

                // In fallback: one poll per interval, no burst retries.
                match support.rest_price(&symbol).await {
                    Ok(price) => {
                        health.set_rest_fallback_healthy(true);
                        crate::metrics::MARKET_DATA_FALLBACK_POLLS
                            .with_label_values(&[venue_str.as_str(), symbol_str.as_str(), "ok"])
                            .inc();
                        event_bus.send(DaemonEvent::MarketData(crate::event_bus::MarketData {
                            venue: venue.clone(),
                            symbol: symbol.clone(),
                            price,
                            timestamp: chrono::Utc::now(),
                            source: MarketDataSource::RestFallback,
                        }));
                    },
                    Err(e) => {
                        health.set_rest_fallback_healthy(false);
                        crate::metrics::MARKET_DATA_FALLBACK_POLLS
                            .with_label_values(&[venue_str.as_str(), symbol_str.as_str(), "error"])
                            .inc();
                        warn!(
                            venue = %venue_str,
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
                    let since_last_warning = last_persist_warn.map(|t| t.elapsed());
                    if fallback_persist_warning_due(persisted, since_last_warning, cfg.alert_after)
                    {
                        warn!(
                            venue = %venue_str,
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
        let manager = MarketDataManager::new(event_bus, cancel, false, vec![]);

        assert_eq!(manager.ws_endpoints.as_slice(), &[MAINNET_MARKET_WS_ENDPOINT]);
    }

    #[test]
    fn market_data_manager_uses_testnet_or_configured_endpoints() {
        let event_bus = Arc::new(EventBus::new(100));
        let cancel = CancellationToken::new();
        let testnet = MarketDataManager::new(event_bus.clone(), cancel.clone(), true, vec![]);
        assert_eq!(testnet.ws_endpoints.as_slice(), &[TESTNET_MARKET_WS_ENDPOINT]);

        let configured = vec![
            "wss://primary.example/market".to_string(),
            "wss://fallback.example/market".to_string(),
        ];
        let manager = MarketDataManager::new(event_bus, cancel, false, configured.clone());
        assert_eq!(manager.ws_endpoints.as_slice(), configured.as_slice());
    }

    #[test]
    fn websocket_reconnect_policy_rotates_endpoints_and_resets_after_data() {
        let mut reconnect = WsReconnectState::new();
        assert_eq!(reconnect.endpoint_index, 0);
        assert_eq!(reconnect.retry_in_secs(false), 1);

        reconnect.record_failure(3, false);
        assert_eq!(reconnect.endpoint_index, 1);
        assert_eq!(reconnect.retry_in_secs(false), 2);

        reconnect.record_failure(3, false);
        assert_eq!(reconnect.endpoint_index, 2);
        reconnect.record_failure(3, false);
        assert_eq!(reconnect.endpoint_index, 0);

        reconnect.record_first_message();
        assert_eq!(reconnect.retry_in_secs(false), 1);
    }

    #[test]
    fn websocket_reconnect_backoff_expands_only_while_rest_is_healthy() {
        let mut reconnect = WsReconnectState { endpoint_index: 0, backoff_secs: 60 };

        for expected in [120, 240, 480, 900, 900] {
            reconnect.record_failure(1, true);
            assert_eq!(reconnect.retry_in_secs(true), expected);
        }

        reconnect.record_failure(1, false);
        assert_eq!(reconnect.retry_in_secs(false), 60);
    }

    #[test]
    fn watchdog_distinguishes_handshake_without_data_from_later_idle() {
        assert_eq!(WsAttemptFailure::ConnectTimeout.metric_reason(), "connect_timeout");
        assert!(matches!(watchdog_failure(false), WsAttemptFailure::HandshakeNoData));
        assert!(matches!(watchdog_failure(true), WsAttemptFailure::FeedIdle));
    }

    #[test]
    fn persistent_fallback_warning_is_periodic_at_the_alert_interval() {
        let interval = Duration::from_secs(15 * 60);

        assert!(!fallback_persist_warning_due(Duration::from_secs(14 * 60), None, interval,));
        assert!(fallback_persist_warning_due(interval, None, interval));
        assert!(!fallback_persist_warning_due(
            Duration::from_secs(30 * 60),
            Some(Duration::from_secs(14 * 60)),
            interval,
        ));
        assert!(fallback_persist_warning_due(
            Duration::from_secs(30 * 60),
            Some(interval),
            interval,
        ));
    }

    #[test]
    fn feed_health_starts_healthy_and_tracks_ticks() {
        let health = FeedHealth::new();
        assert!(health.silent_secs() < 2, "fresh health handle must not read as silent");
        assert!(!health.rest_fallback_healthy());

        health.set_last_tick_secs_ago(120);
        assert!(health.silent_secs() >= 119, "must report silence since the last tick");

        health.record_ws_tick();
        assert!(health.silent_secs() < 2, "recording a tick must reset silence");

        health.set_rest_fallback_healthy(true);
        assert!(health.rest_fallback_healthy());
        health.set_rest_fallback_healthy(false);
        assert!(!health.rest_fallback_healthy());
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
        let manager = MarketDataManager::new(event_bus, cancel.clone(), false, vec![]);

        let health = Arc::new(FeedHealth::new());
        health.set_last_tick_secs_ago(120); // silent past the 90s threshold
        let observed_health = Arc::clone(&health);

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
        assert!(observed_health.rest_fallback_healthy());

        cancel.cancel();
        let _ = handle.await;
        assert!(!observed_health.rest_fallback_healthy());
    }

    #[tokio::test]
    async fn rest_fallback_stays_quiet_while_ws_healthy() {
        let event_bus = Arc::new(EventBus::new(100));
        let mut receiver = event_bus.subscribe();
        let cancel = CancellationToken::new();
        let manager = MarketDataManager::new(event_bus, cancel.clone(), false, vec![]);

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
        let manager = MarketDataManager::new(event_bus, cancel.clone(), false, vec![]);

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

    // ADR-0056 §3: a manager serves one venue; each (venue, symbol) has its
    // own loop and health. These drive the unchanged loop through the port.

    /// A feed whose connections deliver scripted messages and then stay open
    /// and silent. An empty `scripts` refuses the connection.
    struct ScriptedFeed {
        endpoints: Vec<String>,
        connects: std::sync::Mutex<Vec<String>>,
        scripts: std::sync::Mutex<Vec<Vec<TradeFeedMessage>>>,
    }

    impl ScriptedFeed {
        fn new(endpoints: &[&str], scripts: Vec<Vec<TradeFeedMessage>>) -> Arc<Self> {
            Arc::new(Self {
                endpoints: endpoints.iter().map(|endpoint| endpoint.to_string()).collect(),
                connects: std::sync::Mutex::new(Vec::new()),
                scripts: std::sync::Mutex::new(scripts),
            })
        }
    }

    struct ScriptedStream {
        messages: std::vec::IntoIter<TradeFeedMessage>,
    }

    #[async_trait]
    impl robson_exec::TradeStream for ScriptedStream {
        async fn next_message(&mut self) -> Option<Result<TradeFeedMessage, String>> {
            match self.messages.next() {
                Some(message) => Some(Ok(message)),
                None => std::future::pending().await,
            }
        }
    }

    #[async_trait]
    impl TradeFeedPort for ScriptedFeed {
        fn endpoints(&self) -> &[String] {
            &self.endpoints
        }

        async fn connect(
            &self,
            endpoint: &str,
            _symbol: &Symbol,
        ) -> Result<Box<dyn robson_exec::TradeStream>, String> {
            self.connects.lock().unwrap().push(endpoint.to_string());
            let mut scripts = self.scripts.lock().unwrap();
            if scripts.is_empty() {
                return Err("connection refused".to_string());
            }
            Ok(Box::new(ScriptedStream { messages: scripts.remove(0).into_iter() }))
        }
    }

    fn btc() -> Symbol {
        Symbol::from_pair("BTCUSDT").unwrap()
    }

    fn trade(price: rust_decimal::Decimal) -> TradeFeedMessage {
        TradeFeedMessage::Trade {
            symbol: btc(),
            price: Price::new(price).unwrap(),
        }
    }

    /// Push until a stream of `feed` is connected and has received the trade.
    async fn push_when_connected(feed: &robson_exec::StubTradeFeed, price: Price) {
        tokio::time::timeout(Duration::from_secs(2), async {
            while feed.push(&btc(), price) == 0 {
                sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("the WS client must connect to the feed");
    }

    #[tokio::test]
    async fn the_same_symbol_on_two_venues_does_not_cross_events_or_health() {
        let event_bus = Arc::new(EventBus::new(100));
        let mut receiver = event_bus.subscribe();
        let cancel = CancellationToken::new();

        let venue_a = VenueId::new("binance:usdm");
        let feed_a = Arc::new(robson_exec::StubTradeFeed::new());
        let manager_a = MarketDataManager::for_venue(
            event_bus.clone(),
            cancel.clone(),
            venue_a.clone(),
            feed_a.clone(),
        );
        let health_a = Arc::new(FeedHealth::new());
        health_a.set_last_tick_secs_ago(120);
        let handle_a = manager_a.spawn_ws_client(btc(), Arc::clone(&health_a)).unwrap();

        let venue_b = VenueId::new("binance:spot");
        let feed_b = Arc::new(robson_exec::StubTradeFeed::new());
        let manager_b = MarketDataManager::for_venue(
            event_bus.clone(),
            cancel.clone(),
            venue_b.clone(),
            feed_b.clone(),
        );
        let health_b = Arc::new(FeedHealth::new());
        health_b.set_last_tick_secs_ago(120);
        let handle_b = manager_b.spawn_ws_client(btc(), Arc::clone(&health_b)).unwrap();

        push_when_connected(&feed_a, Price::new(dec!(95000)).unwrap()).await;
        let event = tokio::time::timeout(Duration::from_secs(2), receiver.recv())
            .await
            .expect("venue A's tick must be published")
            .expect("receiver open")
            .expect("no lag");
        match event {
            DaemonEvent::MarketData(data) => {
                assert_eq!(data.venue, venue_a);
                assert_eq!(data.symbol, btc());
                assert_eq!(data.source, MarketDataSource::Ws);
            },
            other => panic!("expected MarketData, got {other:?}"),
        }
        assert!(health_a.silent_secs() < 2, "venue A's feed delivered");
        assert!(health_b.silent_secs() >= 119, "venue B's watchdog must not see venue A's tick");

        push_when_connected(&feed_b, Price::new(dec!(94000)).unwrap()).await;
        let event = tokio::time::timeout(Duration::from_secs(2), receiver.recv())
            .await
            .expect("venue B's tick must be published")
            .expect("receiver open")
            .expect("no lag");
        assert!(matches!(event, DaemonEvent::MarketData(data) if data.venue == venue_b));

        cancel.cancel();
        let _ = handle_a.await;
        let _ = handle_b.await;
    }

    #[tokio::test]
    async fn delivered_messages_that_are_not_trades_are_never_ticks() {
        let event_bus = Arc::new(EventBus::new(100));
        let mut receiver = event_bus.subscribe();
        let cancel = CancellationToken::new();
        let feed = ScriptedFeed::new(&["scripted://primary"], vec![vec![
            TradeFeedMessage::NonTrade,
            TradeFeedMessage::Malformed {
                reason: "Failed to parse price",
                value: "not-a-number".to_string(),
                error: "invalid decimal".to_string(),
            },
        ]]);
        let manager = MarketDataManager::for_venue(
            event_bus,
            cancel.clone(),
            VenueId::new(VenueId::BINANCE_USDM),
            feed,
        );
        let health = Arc::new(FeedHealth::new());
        health.set_last_tick_secs_ago(120);
        let handle = manager.spawn_ws_client(btc(), Arc::clone(&health)).unwrap();

        let got = tokio::time::timeout(Duration::from_millis(100), receiver.recv()).await;
        assert!(got.is_err(), "neither message may publish market data");
        assert!(health.silent_secs() >= 119, "neither message is a tick for the fallback");

        cancel.cancel();
        let _ = handle.await;
    }

    #[tokio::test]
    async fn a_trade_tick_publishes_the_symbol_on_the_message() {
        let event_bus = Arc::new(EventBus::new(100));
        let mut receiver = event_bus.subscribe();
        let cancel = CancellationToken::new();
        let feed = ScriptedFeed::new(&["scripted://primary"], vec![vec![trade(dec!(101))]]);
        let manager =
            MarketDataManager::for_venue(event_bus, cancel.clone(), VenueId::new("stub"), feed);
        let handle = manager.spawn_ws_client(btc(), Arc::new(FeedHealth::new())).unwrap();

        let event = tokio::time::timeout(Duration::from_secs(2), receiver.recv())
            .await
            .expect("the tick must be published")
            .expect("receiver open")
            .expect("no lag");
        match event {
            DaemonEvent::MarketData(data) => {
                assert_eq!(data.venue.as_str(), "stub");
                assert_eq!(data.symbol, btc());
                assert_eq!(data.price.as_decimal(), dec!(101));
            },
            other => panic!("expected MarketData, got {other:?}"),
        }

        cancel.cancel();
        let _ = handle.await;
    }

    #[tokio::test]
    async fn a_failed_connection_rotates_to_the_feeds_next_endpoint() {
        let event_bus = Arc::new(EventBus::new(100));
        let cancel = CancellationToken::new();
        let feed = ScriptedFeed::new(&["scripted://primary", "scripted://fallback"], vec![]);
        let manager = MarketDataManager::for_venue(
            event_bus,
            cancel.clone(),
            VenueId::new(VenueId::BINANCE_USDM),
            feed.clone(),
        );
        let handle = manager.spawn_ws_client(btc(), Arc::new(FeedHealth::new())).unwrap();

        // The first retry waits the 1 s initial backoff.
        tokio::time::timeout(Duration::from_secs(5), async {
            while feed.connects.lock().unwrap().len() < 2 {
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the client must retry on the next endpoint");
        assert_eq!(feed.connects.lock().unwrap()[..2], [
            "scripted://primary".to_string(),
            "scripted://fallback".to_string()
        ]);

        cancel.cancel();
        let _ = handle.await;
    }

    #[tokio::test]
    async fn a_feed_without_endpoints_is_refused_instead_of_panicking_its_task() {
        let manager = MarketDataManager::for_venue(
            Arc::new(EventBus::new(100)),
            CancellationToken::new(),
            VenueId::new(VenueId::BINANCE_USDM),
            ScriptedFeed::new(&[], vec![]),
        );
        let error = manager.spawn_ws_client(btc(), Arc::new(FeedHealth::new())).unwrap_err();
        assert!(error.to_string().contains("binance:usdm"), "{error}");
    }

    #[tokio::test]
    async fn rest_fallback_ticks_carry_the_managers_venue() {
        let event_bus = Arc::new(EventBus::new(100));
        let mut receiver = event_bus.subscribe();
        let cancel = CancellationToken::new();
        let manager = MarketDataManager::for_venue(
            event_bus,
            cancel.clone(),
            VenueId::new("binance:spot"),
            Arc::new(robson_exec::StubTradeFeed::new()),
        );
        let health = Arc::new(FeedHealth::new());
        health.set_last_tick_secs_ago(120);
        let support = Arc::new(StubSupport {
            price: Price::new(dec!(62700)).unwrap(),
            risk_open: true,
        });
        let handle = manager.spawn_rest_fallback(btc(), health, support, test_cfg());

        let event = tokio::time::timeout(Duration::from_secs(2), receiver.recv())
            .await
            .expect("fallback must emit within the timeout")
            .expect("receiver open")
            .expect("no lag");
        match event {
            DaemonEvent::MarketData(data) => {
                assert_eq!(data.venue.as_str(), "binance:spot");
                assert_eq!(data.source, MarketDataSource::RestFallback);
            },
            other => panic!("expected MarketData, got {other:?}"),
        }

        cancel.cancel();
        let _ = handle.await;
    }
}
