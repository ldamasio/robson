//! Position Monitor: Safety Net for Rogue Positions
//!
//! The Position Monitor is a background service that:
//! - Polls Binance USD-M Futures API for open positions
//! - Detects positions not created through Robson v2 (rogue positions)
//! - Calculates safety stops (2% from entry)
//! - Executes market orders when stops are hit
//!
//! This runs independently of the normal position flow to provide
//! risk management even when the user bypasses Robson v2.

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Duration,
};

use chrono::{DateTime, Utc};
use robson_connectors::{BinanceRestClient, BinanceRestError, FuturesPosition};
use robson_domain::{DetectedPosition, Price, Quantity, Side, Symbol};
use robson_store::{DetectedPositionRepository, PositionRepository};
use rust_decimal::Decimal;
use tokio::{sync::RwLock, task::JoinHandle};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use crate::event_bus::{DaemonEvent, EventBus};

// =============================================================================
// Configuration
// =============================================================================

/// Position monitor configuration.
#[derive(Debug, Clone)]
pub struct PositionMonitorConfig {
    /// Polling interval in seconds
    pub poll_interval_secs: u64,
    /// Symbols to monitor (e.g., ["BTCUSDT"])
    pub symbols: Vec<String>,
    /// Whether the monitor is enabled
    pub enabled: bool,
    /// Maximum retry attempts for stop execution
    pub max_retry_attempts: u32,
    /// Cooldown period (seconds) before retrying execution
    pub execution_cooldown_secs: u64,
    /// Tolerance for price validation (0.1% = avoids flickering)
    pub price_validation_tolerance_pct: Decimal,
    /// Relative tolerance when matching a tracked position's quantity against
    /// the live exchange quantity (0.5% = 0.005).
    ///
    /// Required by invariant I3 of
    /// `docs/policies/UNTRACKED-POSITION-RECONCILIATION.md`: a local position
    /// matches an exchange position by `(symbol, side)` *and* quantity within
    /// tolerance. Beyond it the two are not the same position and the tracked
    /// state must not be reused.
    pub quantity_tolerance_pct: Decimal,
}

impl Default for PositionMonitorConfig {
    fn default() -> Self {
        Self {
            poll_interval_secs: 20, // 20 seconds default
            symbols: vec!["BTCUSDT".to_string()],
            enabled: true,
            max_retry_attempts: 3,
            execution_cooldown_secs: 60, // Don't retry within 60 seconds
            price_validation_tolerance_pct: Decimal::new(1, 3), // 0.1%
            quantity_tolerance_pct: Decimal::new(5, 3), // 0.5%
        }
    }
}

// =============================================================================
// Execution Tracking (Idempotency)
// =============================================================================

/// Tracks execution attempts for idempotency.
#[derive(Debug, Clone)]
struct ExecutionAttempt {
    /// Position ID being executed
    position_id: String,
    /// When execution was attempted
    attempted_at: DateTime<Utc>,
    /// Number of consecutive failures
    consecutive_failures: u32,
    /// Last error message
    last_error: Option<String>,
    /// Whether this is in panic mode (3+ failures)
    is_panic_mode: bool,
}

impl ExecutionAttempt {
    /// Create a new execution attempt.
    fn new(position_id: String) -> Self {
        Self {
            position_id,
            attempted_at: Utc::now(),
            consecutive_failures: 0,
            last_error: None,
            is_panic_mode: false,
        }
    }

    /// Check if enough time has passed to retry execution.
    fn can_retry(&self, cooldown_secs: u64) -> bool {
        let elapsed = Utc::now().signed_duration_since(self.attempted_at).num_seconds();
        elapsed > cooldown_secs as i64
    }

    /// Record a failed execution attempt.
    fn record_failure(&mut self, error: String, max_failures: u32) {
        self.consecutive_failures += 1;
        self.last_error = Some(error);
        self.is_panic_mode = self.consecutive_failures >= max_failures;
    }

    /// Record a successful execution.
    fn record_success(&mut self) {
        self.consecutive_failures = 0;
        self.last_error = None;
        self.is_panic_mode = false;
    }

    /// Check if in panic mode.
    fn is_panic(&self) -> bool {
        self.is_panic_mode
    }
}

// =============================================================================
// Position Monitor
// =============================================================================

/// Monitors Binance USD-M Futures for rogue positions and manages safety
/// stops.
pub struct PositionMonitor {
    /// Binance REST client
    binance_client: Arc<BinanceRestClient>,
    /// Event bus for publishing events
    event_bus: Arc<EventBus>,
    /// Configuration
    config: PositionMonitorConfig,
    /// Tracked detected positions (position_id -> position)
    tracked_positions: RwLock<HashMap<String, DetectedPosition>>,
    /// Execution attempts tracking (position_id -> attempt)
    execution_attempts: RwLock<HashMap<String, ExecutionAttempt>>,
    /// Shutdown token
    shutdown_token: CancellationToken,
    /// Optional repository for persistence (None = in-memory only)
    repository: Option<Arc<dyn DetectedPositionRepository>>,
    /// Core position repository for exclusion filter
    core_position_repo: Option<Arc<dyn PositionRepository>>,
    /// In-memory exclusion set maintained from Core open/close events.
    core_exclusion_set: RwLock<HashSet<String>>,
    /// Closures whose database write failed, retried on later ticks.
    ///
    /// Without this, a transient `mark_closed` failure is unrecoverable: the
    /// position is already out of `tracked_positions`, so no later tick can
    /// rediscover that the row still needs closing, and the ghost returns on
    /// the next restart.
    pending_closures: RwLock<HashSet<String>>,
}

impl PositionMonitor {
    /// Create a new position monitor.
    pub fn new(
        binance_client: Arc<BinanceRestClient>,
        event_bus: Arc<EventBus>,
        config: PositionMonitorConfig,
    ) -> Self {
        let shutdown_token = CancellationToken::new();

        Self {
            binance_client,
            event_bus,
            config,
            tracked_positions: RwLock::new(HashMap::new()),
            execution_attempts: RwLock::new(HashMap::new()),
            shutdown_token,
            repository: None,
            core_position_repo: None,
            core_exclusion_set: RwLock::new(HashSet::new()),
            pending_closures: RwLock::new(HashSet::new()),
        }
    }

    /// Create a new position monitor with persistence.
    pub fn with_repository(
        binance_client: Arc<BinanceRestClient>,
        event_bus: Arc<EventBus>,
        config: PositionMonitorConfig,
        repository: Arc<dyn DetectedPositionRepository>,
    ) -> Self {
        let shutdown_token = CancellationToken::new();

        Self {
            binance_client,
            event_bus,
            config,
            tracked_positions: RwLock::new(HashMap::new()),
            execution_attempts: RwLock::new(HashMap::new()),
            shutdown_token,
            repository: Some(repository),
            core_position_repo: None,
            core_exclusion_set: RwLock::new(HashSet::new()),
            pending_closures: RwLock::new(HashSet::new()),
        }
    }

    /// Create a new position monitor with Core Trading exclusion filter.
    ///
    /// This variant accepts a Core position repository to exclude Core-managed
    /// positions from Safety Net monitoring, preventing double execution.
    pub fn with_core_exclusion(
        binance_client: Arc<BinanceRestClient>,
        event_bus: Arc<EventBus>,
        config: PositionMonitorConfig,
        repository: Arc<dyn DetectedPositionRepository>,
        core_position_repo: Arc<dyn PositionRepository>,
    ) -> Self {
        let shutdown_token = CancellationToken::new();

        Self {
            binance_client,
            event_bus,
            config,
            tracked_positions: RwLock::new(HashMap::new()),
            execution_attempts: RwLock::new(HashMap::new()),
            shutdown_token,
            repository: Some(repository),
            core_position_repo: Some(core_position_repo),
            core_exclusion_set: RwLock::new(HashSet::new()),
            pending_closures: RwLock::new(HashSet::new()),
        }
    }

    /// Canonical identity of a detected position: `"{SYMBOL}:{side}"`, e.g.
    /// `"BTCUSDT:long"`.
    ///
    /// This is the single source of truth for three things that MUST agree:
    /// the in-memory `tracked_positions` key, the `core_exclusion_set` key, and
    /// the `detected_positions.position_id` column written by
    /// `DetectedPositionDto::from_domain`. Building the key ad hoc from
    /// `Side`'s `Display` impl yields `"BTCUSDT:LONG"`, which silently
    /// desynchronises startup-loaded rows from live-detected ones and makes
    /// every repository call keyed by the id (`mark_closed`,
    /// `clear_execution_attempts`, `update_execution_attempt`) match zero rows,
    /// leaving ghost positions `is_active = TRUE` forever. See ADR-0039.
    ///
    /// It takes a parsed `Symbol` rather than a raw string on purpose: the
    /// symbol half is `Symbol::as_pair()`, byte-identical to what the DTO
    /// persists. Normalising a raw exchange string here instead (upper-casing,
    /// trimming) would reintroduce the same class of divergence one layer down.
    pub fn position_key(symbol: &Symbol, side: Side) -> String {
        format!("{}:{}", symbol.as_pair(), match side {
            Side::Long => "long",
            Side::Short => "short",
        })
    }

    async fn is_core_excluded_in_memory(&self, symbol: &Symbol, side: Side) -> bool {
        let key = Self::position_key(symbol, side);
        self.core_exclusion_set.read().await.contains(&key)
    }

    async fn add_core_exclusion(&self, symbol: &Symbol, side: Side) {
        let key = Self::position_key(symbol, side);
        self.core_exclusion_set.write().await.insert(key);
    }

    async fn remove_core_exclusion(&self, symbol: &Symbol, side: Side) {
        let key = Self::position_key(symbol, side);
        self.core_exclusion_set.write().await.remove(&key);
    }

    /// Load persisted positions from repository on startup.
    pub async fn load_persisted_positions(&self) -> Result<(), MonitorError> {
        if let Some(repo) = &self.repository {
            match repo.find_active().await {
                Ok(positions) => {
                    let mut tracked = self.tracked_positions.write().await;
                    for pos in positions {
                        let position_id = Self::position_key(&pos.symbol, pos.side);
                        tracked.insert(position_id, pos);
                    }
                    info!(count = tracked.len(), "Loaded persisted positions from database");
                },
                Err(e) => {
                    warn!(error = %e, "Failed to load persisted positions, starting fresh");
                },
            }
        }
        Ok(())
    }

    /// Check if a position is managed by Core Trading.
    ///
    /// Returns true if there's an active Core position for this (symbol, side).
    /// Used by Safety Net to exclude Core-managed positions from monitoring.
    async fn is_core_managed(&self, symbol: &Symbol, side: Side) -> Result<bool, MonitorError> {
        if let Some(repo) = &self.core_position_repo {
            match repo.find_active_by_symbol_and_side(symbol, side).await {
                Ok(Some(_)) => {
                    debug!(
                        symbol = %symbol.as_pair(),
                        ?side,
                        "Position is Core-managed, Safety Net will skip"
                    );
                    Ok(true)
                },
                Ok(None) => Ok(false),
                Err(e) => {
                    // Fail-safe: On error, skip monitoring (don't risk double execution)
                    warn!(
                        symbol = %symbol.as_pair(),
                        ?side,
                        error = %e,
                        "Error checking Core positions, failing safe (skipping monitoring)"
                    );
                    Ok(true) // Err on the side of caution
                },
            }
        } else {
            // No core repo configured, Safety Net monitors everything
            Ok(false)
        }
    }

    /// Start the position monitor in the background.
    ///
    /// Returns a JoinHandle that can be awaited or aborted.
    pub fn start(self: Arc<Self>) -> JoinHandle<()> {
        tokio::spawn(async move {
            let mut event_receiver = self.event_bus.subscribe();
            info!(
                interval_secs = self.config.poll_interval_secs,
                symbols = ?self.config.symbols,
                "Position monitor started"
            );

            loop {
                tokio::select! {
                    _ = self.shutdown_token.cancelled() => {
                        info!("Position monitor received shutdown signal");
                        break;
                    }
                    Some(event_result) = event_receiver.recv() => {
                        match event_result {
                            Ok(DaemonEvent::CorePositionOpened { symbol, side, .. }) => {
                                self.add_core_exclusion(&symbol, side).await;
                                debug!(
                                    symbol = %symbol.as_pair(),
                                    ?side,
                                    "Updated core exclusion set (add)"
                                );
                            }
                            Ok(DaemonEvent::CorePositionClosed { symbol, side, .. }) => {
                                self.remove_core_exclusion(&symbol, side).await;
                                debug!(
                                    symbol = %symbol.as_pair(),
                                    ?side,
                                    "Updated core exclusion set (remove)"
                                );
                            }
                            Ok(_) => {
                                // Ignore unrelated events
                            }
                            Err(lag_msg) => {
                                warn!(%lag_msg, "Position monitor event receiver lagged");
                            }
                        }
                    }
                    _ = tokio::time::sleep(Duration::from_secs(self.config.poll_interval_secs)) => {
                        if let Err(e) = self.check_positions().await {
                            error!(error = %e, "Error checking positions");
                        }
                    }
                }
            }

            info!("Position monitor stopped");
        })
    }

    /// Check for new or updated positions on Binance.
    async fn check_positions(&self) -> Result<(), MonitorError> {
        for symbol in &self.config.symbols {
            if let Err(e) = self.check_symbol(symbol).await {
                error!(symbol = %symbol, error = %e, "Error checking symbol");
            }
        }

        Ok(())
    }

    /// Check a single symbol for positions.
    async fn check_symbol(&self, symbol: &str) -> Result<(), MonitorError> {
        debug!(symbol, "Checking for futures positions");

        // Get current positions from Binance. This is the only hard failure of
        // the tick: without an authoritative position list we cannot tell
        // "closed on the exchange" from "exchange unreachable", and cleaning up
        // on the latter would drop a live position from the safety net.
        let binance_positions = self
            .binance_client
            .get_open_positions(symbol)
            .await
            .map_err(|e| MonitorError::BinanceError(e.to_string()))?;

        // Every failure below is recorded and reported, but none of them may
        // skip cleanup: reconciling closures only needs the position list we
        // already hold, and letting a stop-evaluation error suppress it is what
        // keeps ghost positions alive for months.
        let mut first_error: Option<MonitorError> = None;

        match self.binance_client.get_price(symbol).await {
            Ok(current_price) => {
                for binance_pos in binance_positions.iter().cloned() {
                    if let Err(e) = self.process_binance_position(binance_pos, current_price).await
                    {
                        error!(symbol = %symbol, error = %e, "Error processing position");
                        first_error.get_or_insert(e);
                    }
                }
            },
            Err(e) => {
                error!(
                    symbol = %symbol,
                    error = %e,
                    "Failed to get price, skipping stop evaluation for this tick"
                );
                first_error.get_or_insert(MonitorError::BinanceError(e.to_string()));
            },
        }

        // Clean up positions that no longer exist
        self.cleanup_closed_positions(symbol, &binance_positions).await;

        match first_error {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    /// Process a position detected from Binance.
    async fn process_binance_position(
        &self,
        binance_pos: FuturesPosition,
        current_price: Price,
    ) -> Result<(), MonitorError> {
        // Parse the symbol first: the tracking key must be built from the
        // canonical pair, never from the raw exchange payload.
        let symbol = Symbol::from_pair(&binance_pos.symbol)
            .map_err(|_| MonitorError::InvalidSymbol(binance_pos.symbol.clone()))?;

        let position_id = Self::position_key(&symbol, binance_pos.side);

        // EXCLUSION FILTER. Confirmed Core ownership must also *release* any
        // Safety Net state for this key, not merely skip it. A Core position
        // and a stale detected row share the canonical id, so a skip leaves the
        // row tracked and `is_active = TRUE` for as long as the Core position
        // lives — and `cleanup_closed_positions` sees the Core position in the
        // exchange list and treats the row as still open. That is exactly how a
        // ghost survives for months. Ownership is exclusive (ADR-0022): a
        // Core-managed position is not a rogue position, so the Safety Net must
        // hold no state for it.
        if self.is_core_managed(&symbol, binance_pos.side).await? {
            info!(
                symbol = %binance_pos.symbol,
                ?binance_pos.side,
                "Safety Net: Skipping position (Core-managed)"
            );
            self.release_position(&position_id, "core-managed").await;
            return Ok(());
        }
        if self.is_core_excluded_in_memory(&symbol, binance_pos.side).await {
            info!(
                symbol = %binance_pos.symbol,
                ?binance_pos.side,
                "Safety Net: Skipping position (Core-managed via event cache)"
            );
            self.release_position(&position_id, "core-managed via event cache").await;
            return Ok(());
        }

        let mut tracked = self.tracked_positions.write().await;

        if let Some(existing) = tracked.get_mut(&position_id) {
            // Position already tracked. Before trusting any of its state,
            // reconcile it against what the exchange reports right now: the
            // entry may be a row reloaded at startup, describing a position
            // that was resized, or closed and reopened, while the daemon was
            // down. Acting on the persisted entry/quantity would fire the stop
            // at the wrong level and submit the wrong reduce-only quantity.
            // Invariant I3 of UNTRACKED-POSITION-RECONCILIATION.md requires the
            // match to hold on (symbol, side) *and* quantity within tolerance.
            if !Self::same_position(existing, &binance_pos, self.config.quantity_tolerance_pct) {
                warn!(
                    %position_id,
                    tracked_entry = %existing.entry_price,
                    live_entry = %binance_pos.entry_price,
                    tracked_qty = %existing.quantity,
                    live_qty = %binance_pos.quantity,
                    "Tracked position diverges from the exchange, replacing tracked state"
                );

                let mut replacement = DetectedPosition::new(
                    position_id.clone(),
                    symbol.clone(),
                    binance_pos.side,
                    binance_pos.entry_price,
                    binance_pos.quantity,
                );
                replacement.calculate_safety_stop();
                *existing = replacement;

                if let Some(repo) = &self.repository {
                    if let Err(e) = repo.save(existing).await {
                        warn!(error = %e, "Failed to persist replaced detected position");
                    }
                }
            } else if existing.quantity != binance_pos.quantity {
                // Within tolerance: same position, refreshed size.
                existing.quantity = binance_pos.quantity;
            }

            existing.mark_verified();

            // Check if stop is hit
            if let Some(hit) = existing.is_stop_hit(current_price) {
                if hit {
                    // Stop is hit, execute exit
                    let entry_price = existing.entry_price;
                    let stop_price = existing.calculated_stop.as_ref().map(|s| s.stop_price);
                    let quantity = existing.quantity;
                    let side = existing.side;
                    let symbol = existing.symbol.clone();
                    drop(tracked); // Release lock before executing

                    info!(
                        symbol = %symbol.as_pair(),
                        ?side,
                        %entry_price,
                        %current_price,
                        stop_price = ?stop_price.map(|p| p.as_decimal()),
                        "Safety stop hit, executing exit"
                    );

                    self.execute_stop_with_retry(
                        position_id.clone(),
                        symbol,
                        side,
                        entry_price,
                        stop_price.unwrap(),
                        quantity,
                        current_price,
                    )
                    .await?;
                    return Ok(());
                }
            }

            debug!(
                symbol = %binance_pos.symbol,
                ?binance_pos.side,
                %current_price,
                stop_price = ?existing.calculated_stop.as_ref().map(|s| s.stop_price.as_decimal()),
                "Position verified, stop not hit"
            );
        } else {
            // New position detected

            let mut detected = DetectedPosition::new(
                position_id.clone(),
                symbol.clone(),
                binance_pos.side,
                binance_pos.entry_price,
                binance_pos.quantity,
            );

            // Calculate safety stop (2% from entry)
            let calculated_stop = detected.calculate_safety_stop();

            info!(
                symbol = %symbol.as_pair(),
                ?binance_pos.side,
                %binance_pos.entry_price,
                %binance_pos.quantity,
                %calculated_stop.stop_price,
                %calculated_stop.distance_pct,
                "New rogue position detected, safety stop calculated"
            );

            // Emit event
            self.event_bus.send(DaemonEvent::RoguePositionDetected {
                symbol: binance_pos.symbol.clone(),
                side: binance_pos.side,
                entry_price: binance_pos.entry_price,
                stop_price: calculated_stop.stop_price,
            });

            // Check if already at stop (unlikely but possible)
            if calculated_stop.is_hit(detected.side, current_price) {
                drop(tracked); // Release lock before executing

                warn!(
                    symbol = %symbol.as_pair(),
                    ?detected.side,
                    %current_price,
                    %calculated_stop.stop_price,
                    "New position already at stop, executing exit immediately"
                );

                self.execute_stop_with_retry(
                    position_id.clone(),
                    symbol,
                    detected.side,
                    detected.entry_price,
                    calculated_stop.stop_price,
                    detected.quantity,
                    current_price,
                )
                .await?;
                return Ok(());
            }

            // Persist to database if repository is configured
            if let Some(repo) = &self.repository {
                if let Err(e) = repo.save(&detected).await {
                    warn!(error = %e, "Failed to persist detected position to database");
                }
            }

            tracked.insert(position_id, detected);
        }

        Ok(())
    }

    /// Execute stop with retry logic and idempotency tracking.
    ///
    /// This is the enhanced version that handles:
    /// - Idempotency (don't retry if recently attempted)
    /// - Pre-execution validation
    /// - Retry with exponential backoff
    /// - Panic mode on repeated failures
    async fn execute_stop_with_retry(
        &self,
        position_id: String,
        symbol: Symbol,
        side: Side,
        entry_price: Price,
        stop_price: Price,
        quantity: Quantity,
        current_price: Price,
    ) -> Result<(), MonitorError> {
        if self.is_core_excluded_in_memory(&symbol, side).await {
            info!(
                %position_id,
                symbol = %symbol.as_pair(),
                ?side,
                "Safety Net execution skipped (Core-managed via event cache)"
            );
            return Ok(());
        }

        // =========================================
        // 1. IDEMPOTENCY CHECK
        // =========================================
        {
            let attempts = self.execution_attempts.read().await;
            if let Some(attempt) = attempts.get(&position_id) {
                if attempt.is_panic() {
                    // Already in panic mode, log but don't retry yet
                    warn!(
                        %position_id,
                        consecutive_failures = attempt.consecutive_failures,
                        last_error = ?attempt.last_error,
                        "Position in panic mode, will retry after cooldown"
                    );
                    return Ok(()); // Skip this cycle, will retry later
                }

                if !attempt.can_retry(self.config.execution_cooldown_secs) {
                    debug!(
                        %position_id,
                        "Execution attempted recently, skipping for cooldown"
                    );
                    return Ok(()); // Skip this cycle
                }
            }
        }

        // =========================================
        // 2. PRE-EXECUTION VALIDATION
        // =========================================
        // Re-validate price vs stop with tolerance
        let tolerance = stop_price.as_decimal() * self.config.price_validation_tolerance_pct
            / Decimal::from(100u32);
        let is_still_hit = match side {
            Side::Long => {
                // LONG: price must be at or below stop (minus tolerance)
                current_price.as_decimal() <= (stop_price.as_decimal() + tolerance)
            },
            Side::Short => {
                // SHORT: price must be at or above stop (plus tolerance)
                current_price.as_decimal() >= (stop_price.as_decimal() - tolerance)
            },
        };

        if !is_still_hit {
            info!(
                %position_id,
                %current_price,
                %stop_price,
                "Price recovered above stop, skipping execution"
            );
            return Ok(());
        }

        // Calculate expected PnL
        let expected_pnl = self.calculate_expected_pnl(side, entry_price, current_price, quantity);

        info!(
            %position_id,
            %expected_pnl,
            %entry_price,
            %current_price,
            "Executing safety stop with expected PnL"
        );

        // =========================================
        // 3. EXECUTION WITH RETRY
        // =========================================
        let mut last_error: Option<String> = None;
        let mut attempt_num = 0;

        for attempt in 0..self.config.max_retry_attempts {
            attempt_num = attempt + 1;

            // Mark as attempting (before the actual try)
            {
                let mut attempts = self.execution_attempts.write().await;
                let exec_attempt = attempts
                    .entry(position_id.clone())
                    .or_insert_with(|| ExecutionAttempt::new(position_id.clone()));
                exec_attempt.attempted_at = Utc::now();
            }

            // Exponential backoff: 0s, 1s, 2s, 4s...
            if attempt > 0 {
                let delay_ms = 1000 * (1 << (attempt - 1)); // 1s, 2s, 4s...
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            }

            // Determine exit side
            let exit_side = match side {
                Side::Long => Side::Short, // Sell to close long
                Side::Short => Side::Long, // Buy to close short
            };

            // Place market order
            let result = self
                .binance_client
                .place_market_order(&symbol.as_pair(), exit_side, quantity.as_decimal(), None, true)
                .await;

            match result {
                Ok(order) => {
                    // =========================================
                    // 4. SUCCESS: Clean up and emit event
                    // =========================================
                    info!(
                        %position_id,
                        order_id = %order.order_id,
                        executed_qty = %order.executed_qty,
                        "Safety exit executed successfully"
                    );

                    // Remove from tracked positions
                    let mut tracked = self.tracked_positions.write().await;
                    tracked.remove(&position_id);

                    // Remove from execution attempts
                    let mut attempts = self.execution_attempts.write().await;
                    attempts.remove(&position_id);

                    // Mark as closed in database if repository is configured
                    if let Some(repo) = &self.repository {
                        if let Err(e) = repo.mark_closed(&position_id, Utc::now()).await {
                            warn!(error = %e, "Failed to mark position as closed in database");
                        }
                        if let Err(e) = repo.clear_execution_attempts(&position_id).await {
                            warn!(error = %e, "Failed to clear execution attempts in database");
                        }
                    }

                    // Emit event
                    self.event_bus.send(DaemonEvent::SafetyExitExecuted {
                        symbol: symbol.as_pair(),
                        order_id: order.order_id.to_string(),
                        executed_quantity: order.executed_qty,
                    });

                    return Ok(());
                },
                Err(e) => {
                    last_error = Some(e.to_string());

                    // Check if error is transient (should retry)
                    let is_transient = match &e {
                        BinanceRestError::Timeout => true,
                        BinanceRestError::RequestFailed(_) => true,
                        BinanceRestError::ApiError { code, .. } if *code == -1001 => true, /* Disconnect */
                        BinanceRestError::ApiError { code, .. } if *code == -1021 => true, /* Timestamp out of sync */
                        _ => false,
                    };

                    if !is_transient {
                        // Non-transient error, don't retry
                        error!(
                            %position_id,
                            error = %e,
                            "Non-transient error, aborting execution"
                        );

                        // Mark as failed
                        self.mark_execution_failed(&position_id, e.to_string()).await;

                        self.event_bus.send(DaemonEvent::SafetyExitFailed {
                            symbol: symbol.as_pair(),
                            error: e.to_string(),
                        });

                        return Err(MonitorError::ExecutionFailed(e.to_string()));
                    }

                    warn!(
                        %position_id,
                        attempt = attempt_num,
                        max_attempts = self.config.max_retry_attempts,
                        error = %e,
                        "Transient error, retrying"
                    );
                },
            }
        }

        // =========================================
        // 5. ALL RETRIES FAILED: PANIC MODE
        // =========================================
        let error_msg = last_error.unwrap_or_else(|| "Unknown error".to_string());
        self.mark_execution_failed(&position_id, error_msg.clone()).await;

        // Check if entering panic mode
        {
            let attempts = self.execution_attempts.read().await;
            if let Some(attempt) = attempts.get(&position_id) {
                if attempt.is_panic() {
                    // Emit panic event
                    error!(
                        %position_id,
                        consecutive_failures = attempt.consecutive_failures,
                        %error_msg,
                        "PANIC: All retry attempts failed, entering panic mode"
                    );

                    self.event_bus.send(DaemonEvent::SafetyPanic {
                        position_id: position_id.clone(),
                        symbol: symbol.as_pair(),
                        side,
                        error: error_msg.clone(),
                        consecutive_failures: attempt.consecutive_failures,
                    });

                    return Err(MonitorError::PanicMode { position_id, error: error_msg });
                }
            }
        }

        // Not yet in panic mode, will retry next cycle
        Err(MonitorError::ExecutionFailed(error_msg))
    }

    /// Calculate expected PnL for a position exit.
    fn calculate_expected_pnl(
        &self,
        side: Side,
        entry_price: Price,
        exit_price: Price,
        quantity: Quantity,
    ) -> Decimal {
        let entry = entry_price.as_decimal();
        let exit = exit_price.as_decimal();
        let qty = quantity.as_decimal();

        match side {
            Side::Long => (exit - entry) * qty,
            Side::Short => (entry - exit) * qty,
        }
    }

    /// Mark an execution as failed.
    async fn mark_execution_failed(&self, position_id: &str, error: String) {
        let mut attempts = self.execution_attempts.write().await;
        let attempt = attempts
            .entry(position_id.to_string())
            .or_insert_with(|| ExecutionAttempt::new(position_id.to_string()));
        attempt.record_failure(error.clone(), self.config.max_retry_attempts);

        // Persist to database if repository is configured
        if let Some(repo) = &self.repository {
            let is_panic = attempt.is_panic_mode;
            let failures = attempt.consecutive_failures as i32;
            if let Err(e) = repo
                .update_execution_attempt(position_id, Utc::now(), failures, is_panic, Some(error))
                .await
            {
                warn!(error = %e, "Failed to persist execution attempt to database");
            }
        }
    }

    /// Execute a market order to exit a position (DEPRECATED - use
    /// execute_stop_with_retry).
    ///
    /// This method is kept for compatibility but should not be used directly.
    async fn execute_exit(&self, position: &FuturesPosition) -> Result<(), MonitorError> {
        info!(
            symbol = %position.symbol,
            side = ?position.side,
            quantity = %position.quantity.as_decimal(),
            "Executing safety exit order"
        );

        // Determine exit side (opposite of position side)
        let exit_side = match position.side {
            Side::Long => Side::Short, // Sell to close long
            Side::Short => Side::Long, // Buy to close short
        };

        // Place market order
        let order_result = self
            .binance_client
            .place_market_order(
                &position.symbol,
                exit_side,
                position.quantity.as_decimal(),
                None,
                true,
            )
            .await;

        match order_result {
            Ok(order) => {
                info!(
                    symbol = %position.symbol,
                    order_id = %order.order_id,
                    executed_qty = %order.executed_qty,
                    "Safety exit executed successfully"
                );

                // Remove from tracked positions
                let position_id = Symbol::from_pair(&position.symbol)
                    .map(|sym| Self::position_key(&sym, position.side))
                    .unwrap_or_default();
                let mut tracked = self.tracked_positions.write().await;
                tracked.remove(&position_id);

                // Emit event
                self.event_bus.send(DaemonEvent::SafetyExitExecuted {
                    symbol: position.symbol.clone(),
                    order_id: order.order_id.to_string(),
                    executed_quantity: order.executed_qty,
                });

                Ok(())
            },
            Err(e) => {
                error!(
                    symbol = %position.symbol,
                    error = %e,
                    "Failed to execute safety exit"
                );

                // Emit error event
                self.event_bus.send(DaemonEvent::SafetyExitFailed {
                    symbol: position.symbol.clone(),
                    error: e.to_string(),
                });

                Err(MonitorError::ExecutionFailed(e.to_string()))
            },
        }
    }

    /// Whether a tracked position still describes the live exchange position.
    ///
    /// Entry price must match exactly — a different entry means the position
    /// was closed and reopened, or averaged into, and every derived value
    /// (including the safety stop) is stale. Quantity is compared relatively,
    /// within the configured tolerance, per invariant I3.
    fn same_position(
        tracked: &DetectedPosition,
        live: &FuturesPosition,
        quantity_tolerance_pct: Decimal,
    ) -> bool {
        if tracked.entry_price != live.entry_price {
            return false;
        }

        let tracked_qty = tracked.quantity.as_decimal();
        let live_qty = live.quantity.as_decimal();
        if tracked_qty == live_qty {
            return true;
        }
        if tracked_qty.is_zero() {
            return false;
        }

        let drift = (tracked_qty - live_qty).abs() / tracked_qty.abs();
        drift <= quantity_tolerance_pct
    }

    /// Persist a position's closure: mark the row closed, then clear its
    /// execution attempts.
    ///
    /// The order matters and is not cosmetic. `clear_execution_attempts` erases
    /// the panic/retry evidence of a row; doing that when `mark_closed` failed
    /// leaves an *active* row stripped of the state that explains it. So the
    /// clear only runs after a successful close, and a failed close is parked
    /// in `pending_closures` for a later tick to retry.
    async fn persist_closure(&self, position_id: &str) {
        let Some(repo) = &self.repository else {
            return;
        };

        if let Err(e) = repo.mark_closed(position_id, Utc::now()).await {
            warn!(
                %position_id,
                error = %e,
                "Failed to mark position closed in database, queued for retry"
            );
            self.pending_closures.write().await.insert(position_id.to_string());
            return;
        }

        self.pending_closures.write().await.remove(position_id);

        if let Err(e) = repo.clear_execution_attempts(position_id).await {
            warn!(
                %position_id,
                error = %e,
                "Failed to clear execution attempts in database"
            );
        }
    }

    /// Retry closures whose database write failed on an earlier tick.
    async fn retry_pending_closures(&self) {
        let pending: Vec<String> = {
            let guard = self.pending_closures.read().await;
            if guard.is_empty() {
                return;
            }
            guard.iter().cloned().collect()
        };

        info!(count = pending.len(), "Retrying pending position closures");
        for id in pending {
            self.persist_closure(&id).await;
        }
    }

    /// Drop a position from Safety Net tracking and persist its closure.
    async fn release_position(&self, position_id: &str, reason: &str) {
        let was_tracked = self.tracked_positions.write().await.remove(position_id).is_some();
        self.execution_attempts.write().await.remove(position_id);

        if !was_tracked {
            return;
        }

        info!(%position_id, %reason, "Releasing position from Safety Net tracking");
        self.persist_closure(position_id).await;
    }

    /// Clean up positions that are no longer open on Binance.
    ///
    /// Takes the position list already fetched by the caller: re-fetching here
    /// would both double the API cost of a tick and open a window where a
    /// position opened between the two calls is treated as closed.
    async fn cleanup_closed_positions(&self, symbol: &str, binance_positions: &[FuturesPosition]) {
        // A closure whose database write failed earlier is retried here, where
        // we are already talking to the repository.
        self.retry_pending_closures().await;

        // Build set of active position IDs. Symbols the domain cannot parse are
        // skipped rather than normalised: an unparseable symbol could never
        // have produced a tracked entry in the first place, and inventing a key
        // for it is how the two forms drift apart again.
        let active_ids: HashSet<String> = binance_positions
            .iter()
            .filter_map(|p| match Symbol::from_pair(&p.symbol) {
                Ok(sym) => Some(Self::position_key(&sym, p.side)),
                Err(_) => {
                    warn!(symbol = %p.symbol, "Unparseable symbol in cleanup, skipping");
                    None
                },
            })
            .collect();

        // Remove closed positions
        let to_remove: Vec<String> = {
            let mut tracked = self.tracked_positions.write().await;
            let closed: Vec<String> = tracked
                .iter()
                .filter(|(position_id, position)| {
                    position.symbol.as_pair().eq_ignore_ascii_case(symbol)
                        && !active_ids.contains(position_id.as_str())
                })
                .map(|(position_id, _)| position_id.clone())
                .collect();

            for id in &closed {
                info!(
                    symbol = %symbol,
                    position_id = %id,
                    "Position closed externally, removing from tracking"
                );
                tracked.remove(id);
            }
            closed
        };

        if to_remove.is_empty() {
            return;
        }

        {
            let mut attempts = self.execution_attempts.write().await;
            for id in &to_remove {
                attempts.remove(id);
            }
        }

        // Persist the closure. Dropping the position from memory alone leaves
        // the row `is_active = TRUE`, so the next startup reloads it and the
        // ghost comes back forever.
        for id in &to_remove {
            self.persist_closure(id).await;
        }
    }

    /// Shutdown the position monitor.
    pub async fn shutdown(self) {
        info!("Shutting down position monitor");
        self.shutdown_token.cancel();
    }

    /// Get all tracked positions.
    pub async fn get_tracked_positions(&self) -> Vec<DetectedPosition> {
        self.tracked_positions.read().await.values().cloned().collect()
    }

    /// Get tracked positions for a specific symbol.
    pub async fn get_positions_for_symbol(&self, symbol: &str) -> Vec<DetectedPosition> {
        self.tracked_positions
            .read()
            .await
            .values()
            .filter(|p| p.symbol.as_pair() == symbol)
            .cloned()
            .collect()
    }

    /// Get execution attempts for debugging/monitoring.
    pub async fn get_execution_attempts(&self) -> Vec<(String, ExecutionAttempt)> {
        self.execution_attempts
            .read()
            .await
            .iter()
            .map(|(id, attempt)| (id.clone(), attempt.clone()))
            .collect()
    }

    /// Get count of pending execution attempts.
    pub async fn get_pending_execution_count(&self) -> usize {
        self.execution_attempts.read().await.len()
    }
}

// =============================================================================
// Errors
// =============================================================================

/// Errors that can occur in the position monitor.
#[derive(Debug, thiserror::Error)]
pub enum MonitorError {
    /// Binance API error
    #[error("Binance error: {0}")]
    BinanceError(String),

    /// Invalid symbol
    #[error("Invalid symbol: {0}")]
    InvalidSymbol(String),

    /// Execution failed
    #[error("Execution failed: {0}")]
    ExecutionFailed(String),

    /// Panic mode activated
    #[error("PANIC mode for position {position_id}: {error}")]
    PanicMode { position_id: String, error: String },
}

/// Type alias for monitor results.
pub type DaemonResult<T> = Result<T, MonitorError>;

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use robson_store::MemoryDetectedPositionRepository;
    use rust_decimal_macros::dec;

    use super::*;

    fn create_test_config() -> PositionMonitorConfig {
        PositionMonitorConfig {
            poll_interval_secs: 1,
            symbols: vec!["BTCUSDT".to_string()],
            enabled: true,
            max_retry_attempts: 3,
            execution_cooldown_secs: 60,
            price_validation_tolerance_pct: dec!(0.1),
            quantity_tolerance_pct: dec!(0.005),
        }
    }

    fn ghost_position() -> DetectedPosition {
        let mut pos = DetectedPosition::new(
            "BTCUSDT:long".to_string(),
            Symbol::from_pair("BTCUSDT").unwrap(),
            Side::Long,
            Price::new(dec!(81328.30)).unwrap(),
            Quantity::new(dec!(0.086)).unwrap(),
        );
        pos.calculate_safety_stop();
        pos
    }

    fn futures_position(symbol: &str, side: Side, entry: Decimal) -> FuturesPosition {
        futures_position_qty(symbol, side, entry, dec!(0.086))
    }

    fn futures_position_qty(
        symbol: &str,
        side: Side,
        entry: Decimal,
        quantity: Decimal,
    ) -> FuturesPosition {
        FuturesPosition {
            symbol: symbol.to_string(),
            side,
            quantity: Quantity::new(quantity).unwrap(),
            entry_price: Price::new(entry).unwrap(),
            unrealized_pnl: dec!(0),
            leverage: 10,
        }
    }

    /// Repository whose first `n` `mark_closed` calls fail, counting how often
    /// `clear_execution_attempts` was reached.
    struct FlakyRepo {
        inner: Arc<MemoryDetectedPositionRepository>,
        remaining_failures: std::sync::atomic::AtomicUsize,
        clear_calls: std::sync::atomic::AtomicUsize,
    }

    impl FlakyRepo {
        fn new(inner: Arc<MemoryDetectedPositionRepository>, failures: usize) -> Self {
            Self {
                inner,
                remaining_failures: std::sync::atomic::AtomicUsize::new(failures),
                clear_calls: std::sync::atomic::AtomicUsize::new(0),
            }
        }

        fn clear_calls(&self) -> usize {
            self.clear_calls.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    #[async_trait::async_trait]
    impl DetectedPositionRepository for FlakyRepo {
        async fn save(&self, position: &DetectedPosition) -> Result<(), robson_store::StoreError> {
            self.inner.save(position).await
        }

        async fn find_by_id(
            &self,
            id: &str,
        ) -> Result<Option<DetectedPosition>, robson_store::StoreError> {
            self.inner.find_by_id(id).await
        }

        async fn find_active(&self) -> Result<Vec<DetectedPosition>, robson_store::StoreError> {
            self.inner.find_active().await
        }

        async fn find_by_symbol(
            &self,
            symbol: &str,
        ) -> Result<Vec<DetectedPosition>, robson_store::StoreError> {
            self.inner.find_by_symbol(symbol).await
        }

        async fn mark_closed(
            &self,
            id: &str,
            closed_at: DateTime<Utc>,
        ) -> Result<(), robson_store::StoreError> {
            if self
                .remaining_failures
                .fetch_update(
                    std::sync::atomic::Ordering::SeqCst,
                    std::sync::atomic::Ordering::SeqCst,
                    |n| if n > 0 { Some(n - 1) } else { None },
                )
                .is_ok()
            {
                return Err(robson_store::StoreError::Deserialization(
                    "injected failure".to_string(),
                ));
            }
            self.inner.mark_closed(id, closed_at).await
        }

        async fn update_execution_attempt(
            &self,
            id: &str,
            attempted_at: DateTime<Utc>,
            failures: i32,
            is_panic: bool,
            error: Option<String>,
        ) -> Result<(), robson_store::StoreError> {
            self.inner
                .update_execution_attempt(id, attempted_at, failures, is_panic, error)
                .await
        }

        async fn clear_execution_attempts(&self, id: &str) -> Result<(), robson_store::StoreError> {
            self.clear_calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.inner.clear_execution_attempts(id).await
        }

        async fn log_execution(
            &self,
            execution: &robson_store::SafetyExecutionDto,
        ) -> Result<(), robson_store::StoreError> {
            self.inner.log_execution(execution).await
        }

        async fn get_executions(
            &self,
            position_id: &str,
        ) -> Result<Vec<robson_store::SafetyExecutionDto>, robson_store::StoreError> {
            self.inner.get_executions(position_id).await
        }

        async fn find_panic_mode(&self) -> Result<Vec<DetectedPosition>, robson_store::StoreError> {
            self.inner.find_panic_mode().await
        }

        async fn cleanup_old_positions(
            &self,
            older_than: DateTime<Utc>,
        ) -> Result<u64, robson_store::StoreError> {
            self.inner.cleanup_old_positions(older_than).await
        }
    }

    /// The tracking key must be byte-identical to the id the repository
    /// persists. When they diverge, every repository call keyed by the id is a
    /// silent no-op and detected positions stay `is_active = TRUE` forever.
    #[test]
    fn test_position_key_matches_persisted_position_id() {
        let btc = Symbol::from_pair("BTCUSDT").unwrap();
        assert_eq!(PositionMonitor::position_key(&btc, Side::Long), "BTCUSDT:long");
        assert_eq!(PositionMonitor::position_key(&btc, Side::Short), "BTCUSDT:short");

        // Side's Display impl is uppercase; building the key from it is the
        // defect this test guards against.
        assert_ne!(
            format!("{}:{}", "BTCUSDT", Side::Long),
            PositionMonitor::position_key(&btc, Side::Long)
        );

        // The key must be byte-identical to what the repository persists,
        // otherwise every repository call keyed by the id is a silent no-op.
        let pos = ghost_position();
        assert_eq!(
            robson_store::DetectedPositionDto::from_domain(&pos).position_id,
            PositionMonitor::position_key(&pos.symbol, pos.side),
        );
    }

    /// Regression: confirmed Core ownership must *release* Safety Net state for
    /// the key, not merely skip it. A Core position and a stale detected row
    /// share the canonical id, so a bare skip leaves the row tracked and
    /// `is_active = TRUE` for as long as the Core position lives — which is how
    /// the production ghost survived three months.
    #[tokio::test]
    async fn test_core_managed_position_releases_tracked_ghost() {
        let repo = Arc::new(MemoryDetectedPositionRepository::new());
        repo.save(&ghost_position()).await.unwrap();

        let monitor = PositionMonitor::with_repository(
            Arc::new(BinanceRestClient::new("key".to_string(), "secret".to_string())),
            Arc::new(EventBus::new(100)),
            create_test_config(),
            repo.clone(),
        );
        monitor.load_persisted_positions().await.unwrap();
        assert_eq!(monitor.get_tracked_positions().await.len(), 1);

        // Core takes ownership of the same (symbol, side).
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        monitor.add_core_exclusion(&symbol, Side::Long).await;

        monitor
            .process_binance_position(
                futures_position("BTCUSDT", Side::Long, dec!(62999.70)),
                Price::new(dec!(64667)).unwrap(),
            )
            .await
            .unwrap();

        assert!(
            monitor.get_tracked_positions().await.is_empty(),
            "Core-managed key must not stay tracked by the Safety Net"
        );
        assert!(
            repo.find_active().await.unwrap().is_empty(),
            "released row must be marked closed, otherwise it reloads on restart"
        );
    }

    /// Regression: a reloaded row describing a position that was resized, or
    /// closed and reopened, must not be trusted. Acting on the persisted entry
    /// would fire the stop at the wrong level and exit the wrong quantity.
    #[tokio::test]
    async fn test_diverged_tracked_position_is_replaced_from_exchange() {
        let repo = Arc::new(MemoryDetectedPositionRepository::new());
        repo.save(&ghost_position()).await.unwrap();

        let monitor = PositionMonitor::with_repository(
            Arc::new(BinanceRestClient::new("key".to_string(), "secret".to_string())),
            Arc::new(EventBus::new(100)),
            create_test_config(),
            repo.clone(),
        );
        monitor.load_persisted_positions().await.unwrap();

        // The stale stop (79701.73, from entry 81328.30) is far above the live
        // price, so trusting it would fire an immediate market exit.
        let live_price = Price::new(dec!(64667)).unwrap();
        assert!(monitor.get_tracked_positions().await[0].is_stop_hit(live_price).unwrap());

        monitor
            .process_binance_position(
                futures_position_qty("BTCUSDT", Side::Long, dec!(62999.70), dec!(0.022)),
                live_price,
            )
            .await
            .unwrap();

        let tracked = monitor.get_tracked_positions().await;
        assert_eq!(tracked.len(), 1);
        assert_eq!(tracked[0].entry_price, Price::new(dec!(62999.70)).unwrap());
        assert_eq!(tracked[0].quantity, Quantity::new(dec!(0.022)).unwrap());
        assert!(
            !tracked[0].is_stop_hit(live_price).unwrap(),
            "stop must be recomputed from the live entry, not the stale one"
        );
    }

    /// A size change within tolerance is the same position: refresh the
    /// quantity, keep the row (and its detected_at) intact.
    #[tokio::test]
    async fn test_quantity_within_tolerance_refreshes_without_replacing() {
        let repo = Arc::new(MemoryDetectedPositionRepository::new());
        let ghost = ghost_position();
        let detected_at = ghost.detected_at;
        repo.save(&ghost).await.unwrap();

        let monitor = PositionMonitor::with_repository(
            Arc::new(BinanceRestClient::new("key".to_string(), "secret".to_string())),
            Arc::new(EventBus::new(100)),
            create_test_config(),
            repo.clone(),
        );
        monitor.load_persisted_positions().await.unwrap();

        // Same entry, 0.086 -> 0.0859 is ~0.12%, inside the 0.5% tolerance.
        monitor
            .process_binance_position(
                futures_position_qty("BTCUSDT", Side::Long, dec!(81328.30), dec!(0.0859)),
                Price::new(dec!(81000)).unwrap(),
            )
            .await
            .unwrap();

        let tracked = monitor.get_tracked_positions().await;
        assert_eq!(tracked.len(), 1);
        assert_eq!(tracked[0].quantity, Quantity::new(dec!(0.0859)).unwrap());
        assert_eq!(tracked[0].detected_at, detected_at, "same position must not be replaced");
    }

    /// Regression: a failed `mark_closed` must not clear the row's retry
    /// evidence, and must stay retryable. Otherwise a transient database error
    /// leaves an active row stripped of its panic state, with nothing left in
    /// memory for a later tick to rediscover.
    #[tokio::test]
    async fn test_failed_close_is_retried_and_keeps_execution_attempts() {
        let inner = Arc::new(MemoryDetectedPositionRepository::new());
        inner.save(&ghost_position()).await.unwrap();
        let repo = Arc::new(FlakyRepo::new(inner.clone(), 1));

        let monitor = PositionMonitor::with_repository(
            Arc::new(BinanceRestClient::new("key".to_string(), "secret".to_string())),
            Arc::new(EventBus::new(100)),
            create_test_config(),
            repo.clone(),
        );
        monitor.load_persisted_positions().await.unwrap();

        // First cleanup: mark_closed fails.
        monitor.cleanup_closed_positions("BTCUSDT", &[]).await;
        assert!(monitor.get_tracked_positions().await.is_empty());
        assert_eq!(
            repo.clear_calls(),
            0,
            "attempts must not be cleared while the row is still active"
        );
        assert_eq!(inner.find_active().await.unwrap().len(), 1, "row is still active");

        // Second cleanup: nothing tracked, but the pending closure is retried.
        monitor.cleanup_closed_positions("BTCUSDT", &[]).await;
        assert!(
            inner.find_active().await.unwrap().is_empty(),
            "queued closure must be retried on a later tick"
        );
        assert_eq!(repo.clear_calls(), 1, "attempts cleared only after a successful close");
    }

    /// Regression: a row reloaded at startup and the same position seen live
    /// must land on one entry, not two.
    #[tokio::test]
    async fn test_persisted_position_reloads_under_live_detection_key() {
        let repo = Arc::new(MemoryDetectedPositionRepository::new());
        repo.save(&ghost_position()).await.unwrap();

        let monitor = PositionMonitor::with_repository(
            Arc::new(BinanceRestClient::new("key".to_string(), "secret".to_string())),
            Arc::new(EventBus::new(100)),
            create_test_config(),
            repo.clone(),
        );

        monitor.load_persisted_positions().await.unwrap();
        assert_eq!(monitor.get_tracked_positions().await.len(), 1);

        // Live detection of the same (symbol, side), price well above the stop.
        monitor
            .process_binance_position(
                futures_position("BTCUSDT", Side::Long, dec!(81328.30)),
                Price::new(dec!(81000)).unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(
            monitor.get_tracked_positions().await.len(),
            1,
            "startup-loaded and live-detected keys must agree, otherwise the same position is tracked twice"
        );
    }

    /// Regression: cleanup must persist the closure. Removing the position from
    /// memory alone leaves `is_active = TRUE`, so the next startup reloads the
    /// ghost and it survives indefinitely.
    #[tokio::test]
    async fn test_cleanup_marks_externally_closed_position_closed_in_repository() {
        let repo = Arc::new(MemoryDetectedPositionRepository::new());
        repo.save(&ghost_position()).await.unwrap();

        let monitor = PositionMonitor::with_repository(
            Arc::new(BinanceRestClient::new("key".to_string(), "secret".to_string())),
            Arc::new(EventBus::new(100)),
            create_test_config(),
            repo.clone(),
        );
        monitor.load_persisted_positions().await.unwrap();

        // Binance reports no open positions for the symbol.
        monitor.cleanup_closed_positions("BTCUSDT", &[]).await;

        assert!(monitor.get_tracked_positions().await.is_empty());
        assert!(
            repo.find_active().await.unwrap().is_empty(),
            "externally closed position must be marked closed, otherwise it reloads on restart"
        );
    }

    /// Cleanup must not drop a position that is still open on the exchange.
    #[tokio::test]
    async fn test_cleanup_keeps_positions_still_open_on_exchange() {
        let repo = Arc::new(MemoryDetectedPositionRepository::new());
        repo.save(&ghost_position()).await.unwrap();

        let monitor = PositionMonitor::with_repository(
            Arc::new(BinanceRestClient::new("key".to_string(), "secret".to_string())),
            Arc::new(EventBus::new(100)),
            create_test_config(),
            repo.clone(),
        );
        monitor.load_persisted_positions().await.unwrap();

        monitor
            .cleanup_closed_positions("BTCUSDT", &[futures_position(
                "BTCUSDT",
                Side::Long,
                dec!(81328.30),
            )])
            .await;

        assert_eq!(monitor.get_tracked_positions().await.len(), 1);
        assert_eq!(repo.find_active().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn test_core_exclusion_set_add_remove() {
        let monitor = PositionMonitor::new(
            Arc::new(BinanceRestClient::new("key".to_string(), "secret".to_string())),
            Arc::new(EventBus::new(100)),
            create_test_config(),
        );

        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        assert!(!monitor.is_core_excluded_in_memory(&symbol, Side::Long).await);

        monitor.add_core_exclusion(&symbol, Side::Long).await;
        assert!(monitor.is_core_excluded_in_memory(&symbol, Side::Long).await);

        monitor.remove_core_exclusion(&symbol, Side::Long).await;
        assert!(!monitor.is_core_excluded_in_memory(&symbol, Side::Long).await);
    }

    #[tokio::test]
    async fn test_process_binance_position_skips_core_exclusion_cache() {
        let monitor = PositionMonitor::new(
            Arc::new(BinanceRestClient::new("key".to_string(), "secret".to_string())),
            Arc::new(EventBus::new(100)),
            create_test_config(),
        );

        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        monitor.add_core_exclusion(&symbol, Side::Long).await;

        let binance_pos = FuturesPosition {
            symbol: "BTCUSDT".to_string(),
            side: Side::Long,
            quantity: Quantity::new(dec!(0.1)).unwrap(),
            entry_price: Price::new(dec!(95000)).unwrap(),
            unrealized_pnl: dec!(0),
            leverage: 10,
        };
        let current_price = Price::new(dec!(95000)).unwrap();

        monitor.process_binance_position(binance_pos, current_price).await.unwrap();

        // Position should not be tracked because it was excluded as Core-managed.
        assert!(monitor.get_tracked_positions().await.is_empty());
    }

    #[test]
    fn test_position_monitor_config_default() {
        let config = PositionMonitorConfig::default();

        assert_eq!(config.poll_interval_secs, 20);
        assert_eq!(config.symbols, vec!["BTCUSDT"]);
        assert!(config.enabled);
        assert_eq!(config.max_retry_attempts, 3);
        assert_eq!(config.execution_cooldown_secs, 60);
    }

    #[test]
    fn test_execution_attempt_can_retry() {
        // A newly created attempt cannot retry immediately (elapsed = 0, cooldown = 0)
        // because the check is elapsed > cooldown (0 > 0 = false)
        let attempt = ExecutionAttempt::new("test_pos".to_string());
        assert!(!attempt.can_retry(0));

        // After 1 second, can retry with cooldown 0
        let mut attempt = ExecutionAttempt::new("test_pos".to_string());
        attempt.attempted_at = Utc::now() - chrono::Duration::seconds(1);
        assert!(attempt.can_retry(0));

        // With cooldown 60, can retry after 60+ seconds
        let mut attempt = ExecutionAttempt::new("test_pos".to_string());
        attempt.attempted_at = Utc::now() - chrono::Duration::seconds(120);
        assert!(attempt.can_retry(60));

        // With cooldown 60, cannot retry after only 30 seconds
        let mut attempt = ExecutionAttempt::new("test_pos".to_string());
        attempt.attempted_at = Utc::now() - chrono::Duration::seconds(30);
        assert!(!attempt.can_retry(60));
    }

    #[test]
    fn test_execution_attempt_failure_tracking() {
        let mut attempt = ExecutionAttempt::new("test_pos".to_string());

        assert_eq!(attempt.consecutive_failures, 0);
        assert!(!attempt.is_panic());

        // Record 2 failures
        attempt.record_failure("error1".to_string(), 3);
        attempt.record_failure("error2".to_string(), 3);

        assert_eq!(attempt.consecutive_failures, 2);
        assert!(!attempt.is_panic());

        // 3rd failure triggers panic mode
        attempt.record_failure("error3".to_string(), 3);

        assert_eq!(attempt.consecutive_failures, 3);
        assert!(attempt.is_panic());
    }

    #[test]
    fn test_calculate_expected_pnl_long() {
        let monitor = PositionMonitor::new(
            Arc::new(BinanceRestClient::new("key".to_string(), "secret".to_string())),
            Arc::new(EventBus::new(100)),
            create_test_config(),
        );

        let entry = Price::new(dec!(95000)).unwrap();
        let exit = Price::new(dec!(96000)).unwrap(); // Profit
        let qty = Quantity::new(dec!(0.1)).unwrap();

        let pnl = monitor.calculate_expected_pnl(Side::Long, entry, exit, qty);

        // LONG: (exit - entry) * qty = (96000 - 95000) * 0.1 = 100
        assert_eq!(pnl, dec!(100));
    }

    #[test]
    fn test_calculate_expected_pnl_short() {
        let monitor = PositionMonitor::new(
            Arc::new(BinanceRestClient::new("key".to_string(), "secret".to_string())),
            Arc::new(EventBus::new(100)),
            create_test_config(),
        );

        let entry = Price::new(dec!(95000)).unwrap();
        let exit = Price::new(dec!(94000)).unwrap(); // Profit
        let qty = Quantity::new(dec!(0.1)).unwrap();

        let pnl = monitor.calculate_expected_pnl(Side::Short, entry, exit, qty);

        // SHORT: (entry - exit) * qty = (95000 - 94000) * 0.1 = 100
        assert_eq!(pnl, dec!(100));
    }

    #[test]
    fn test_calculate_expected_pnl_loss() {
        let monitor = PositionMonitor::new(
            Arc::new(BinanceRestClient::new("key".to_string(), "secret".to_string())),
            Arc::new(EventBus::new(100)),
            create_test_config(),
        );

        let entry = Price::new(dec!(95000)).unwrap();
        let exit = Price::new(dec!(93100)).unwrap(); // Loss exit
        let qty = Quantity::new(dec!(0.1)).unwrap();

        let pnl = monitor.calculate_expected_pnl(Side::Long, entry, exit, qty);

        // LONG: (93100 - 95000) * 0.1 = -190
        assert_eq!(pnl, dec!(-190));
    }
}
