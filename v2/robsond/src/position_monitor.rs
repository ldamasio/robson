//! Position Monitor: Safety Net for Rogue Positions
//!
//! The Position Monitor is a background service that:
//! - Polls Binance API for isolated margin positions
//! - Detects positions not created through Robson v2 (rogue positions)
//! - Calculates safety stops (2% from entry)
//! - Executes market orders when stops are hit
//!
//! This runs independently of the normal position flow to provide
//! risk management even when the user bypasses Robson v2.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use robson_connectors::{BinanceRestClient, BinanceRestError, IsolatedMarginPosition};
use robson_domain::{DetectedPosition, Price, Quantity, Side, Symbol};
use robson_store::{DetectedPositionRepository, PositionRepository};

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
}

impl Default for PositionMonitorConfig {
    fn default() -> Self {
        Self {
            poll_interval_secs: 20,      // 20 seconds default
            symbols: vec!["BTCUSDT".to_string()],
            enabled: true,
            max_retry_attempts: 3,
            execution_cooldown_secs: 60,  // Don't retry within 60 seconds
            price_validation_tolerance_pct: Decimal::new(1, 3), // 0.1%
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

/// Monitors Binance isolated margin for rogue positions and manages safety stops.
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
        }
    }

    /// Create a new position monitor with Core Trading exclusion filter.
    ///
    /// This variant accepts a Core position repository to exclude Core-managed positions
    /// from Safety Net monitoring, preventing double execution.
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
        }
    }

    fn exclusion_key(symbol: &str, side: Side) -> String {
        format!("{symbol}:{}", if side == Side::Long { "long" } else { "short" })
    }

    async fn is_core_excluded_in_memory(&self, symbol: &Symbol, side: Side) -> bool {
        let key = Self::exclusion_key(&symbol.as_pair(), side);
        self.core_exclusion_set.read().await.contains(&key)
    }

    async fn add_core_exclusion(&self, symbol: &Symbol, side: Side) {
        let key = Self::exclusion_key(&symbol.as_pair(), side);
        self.core_exclusion_set.write().await.insert(key);
    }

    async fn remove_core_exclusion(&self, symbol: &Symbol, side: Side) {
        let key = Self::exclusion_key(&symbol.as_pair(), side);
        self.core_exclusion_set.write().await.remove(&key);
    }

    /// Load persisted positions from repository on startup.
    pub async fn load_persisted_positions(&self) -> Result<(), MonitorError> {
        if let Some(repo) = &self.repository {
            match repo.find_active().await {
                Ok(positions) => {
                    let mut tracked = self.tracked_positions.write().await;
                    for pos in positions {
                        let position_id = format!("{}:{}", pos.symbol.as_pair(),
                            if pos.side == Side::Long { "long" } else { "short" });
                        tracked.insert(position_id, pos);
                    }
                    info!(count = tracked.len(), "Loaded persisted positions from database");
                }
                Err(e) => {
                    warn!(error = %e, "Failed to load persisted positions, starting fresh");
                }
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
                }
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
                }
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
        debug!(symbol, "Checking for isolated margin positions");

        // Get current positions from Binance
        let binance_positions = self
            .binance_client
            .get_open_positions(symbol)
            .await
            .map_err(|e| MonitorError::BinanceError(e.to_string()))?;

        // Get current price
        let current_price = self
            .binance_client
            .get_price(symbol)
            .await
            .map_err(|e| MonitorError::BinanceError(e.to_string()))?;

        // Check each position
        for binance_pos in binance_positions {
            self.process_binance_position(binance_pos, current_price).await?;
        }

        // Clean up positions that no longer exist
        self.cleanup_closed_positions(symbol).await;

        Ok(())
    }

    /// Process a position detected from Binance.
    async fn process_binance_position(
        &self,
        binance_pos: IsolatedMarginPosition,
        current_price: Price,
    ) -> Result<(), MonitorError> {
        let position_id = format!("{}:{}", binance_pos.symbol, binance_pos.side);

        // EXCLUSION FILTER: Check if Core Trading is managing this position
        let symbol = Symbol::from_pair(&binance_pos.symbol)
            .map_err(|_| MonitorError::InvalidSymbol(binance_pos.symbol.clone()))?;

        if self.is_core_managed(&symbol, binance_pos.side).await? {
            info!(
                symbol = %binance_pos.symbol,
                ?binance_pos.side,
                "Safety Net: Skipping position (Core-managed)"
            );
            return Ok(());
        }
        if self.is_core_excluded_in_memory(&symbol, binance_pos.side).await {
            info!(
                symbol = %binance_pos.symbol,
                ?binance_pos.side,
                "Safety Net: Skipping position (Core-managed via event cache)"
            );
            return Ok(());
        }

        let mut tracked = self.tracked_positions.write().await;

        if let Some(existing) = tracked.get_mut(&position_id) {
            // Position already tracked, update and check stop
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
                    ).await?;
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
                ).await?;
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
        let tolerance = stop_price.as_decimal() * self.config.price_validation_tolerance_pct / Decimal::from(100u32);
        let is_still_hit = match side {
            Side::Long => {
                // LONG: price must be at or below stop (minus tolerance)
                current_price.as_decimal() <= (stop_price.as_decimal() + tolerance)
            }
            Side::Short => {
                // SHORT: price must be at or above stop (plus tolerance)
                current_price.as_decimal() >= (stop_price.as_decimal() - tolerance)
            }
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
                let exec_attempt = attempts.entry(position_id.clone())
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
                Side::Long => Side::Short,  // Sell to close long
                Side::Short => Side::Long,  // Buy to close short
            };

            // Place market order
            let result = self.binance_client.place_market_order(
                &symbol.as_pair(),
                exit_side,
                quantity.as_decimal(),
            ).await;

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
                }
                Err(e) => {
                    last_error = Some(e.to_string());

                    // Check if error is transient (should retry)
                    let is_transient = match &e {
                        BinanceRestError::Timeout => true,
                        BinanceRestError::RequestFailed(_) => true,
                        BinanceRestError::ApiError { code, .. } if *code == -1001 => true, // Disconnect
                        BinanceRestError::ApiError { code, .. } if *code == -1021 => true, // Timestamp out of sync
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
                }
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

                    return Err(MonitorError::PanicMode {
                        position_id,
                        error: error_msg,
                    });
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
        let attempt = attempts.entry(position_id.to_string())
            .or_insert_with(|| ExecutionAttempt::new(position_id.to_string()));
        attempt.record_failure(error.clone(), self.config.max_retry_attempts);

        // Persist to database if repository is configured
        if let Some(repo) = &self.repository {
            let is_panic = attempt.is_panic_mode;
            let failures = attempt.consecutive_failures as i32;
            if let Err(e) = repo.update_execution_attempt(
                position_id,
                Utc::now(),
                failures,
                is_panic,
                Some(error),
            ).await {
                warn!(error = %e, "Failed to persist execution attempt to database");
            }
        }
    }

    /// Execute a market order to exit a position (DEPRECATED - use execute_stop_with_retry).
    ///
    /// This method is kept for compatibility but should not be used directly.
    async fn execute_exit(&self, position: &IsolatedMarginPosition) -> Result<(), MonitorError> {
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
                let position_id = format!("{}:{}", position.symbol, position.side);
                let mut tracked = self.tracked_positions.write().await;
                tracked.remove(&position_id);

                // Emit event
                self.event_bus.send(DaemonEvent::SafetyExitExecuted {
                    symbol: position.symbol.clone(),
                    order_id: order.order_id.to_string(),
                    executed_quantity: order.executed_qty,
                });

                Ok(())
            }
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
            }
        }
    }

    /// Clean up positions that are no longer open on Binance.
    async fn cleanup_closed_positions(&self, symbol: &str) {
        // Get all positions for this symbol
        let binance_positions = match self
            .binance_client
            .get_open_positions(symbol)
            .await
        {
            Ok(p) => p,
            Err(e) => {
                error!(symbol = %symbol, error = %e, "Failed to get positions for cleanup");
                return;
            }
        };

        // Build set of active position IDs
        let active_ids: std::collections::HashSet<String> = binance_positions
            .iter()
            .map(|p| format!("{}:{}", p.symbol, p.side))
            .collect();

        // Remove closed positions
        let mut tracked = self.tracked_positions.write().await;
        let mut to_remove = Vec::new();

        for (position_id, position) in tracked.iter() {
            if position.symbol.as_pair() == symbol && !active_ids.contains(position_id) {
                to_remove.push(position_id.clone());

                info!(
                    symbol = %symbol,
                    %position_id,
                    "Position closed externally, removing from tracking"
                );
            }
        }

        for id in to_remove {
            tracked.remove(&id);
            // Also remove from execution attempts
            let mut attempts = self.execution_attempts.write().await;
            attempts.remove(&id);
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
    PanicMode {
        position_id: String,
        error: String,
    },
}

/// Type alias for monitor results.
pub type DaemonResult<T> = Result<T, MonitorError>;

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    fn create_test_config() -> PositionMonitorConfig {
        PositionMonitorConfig {
            poll_interval_secs: 1,
            symbols: vec!["BTCUSDT".to_string()],
            enabled: true,
            max_retry_attempts: 3,
            execution_cooldown_secs: 60,
            price_validation_tolerance_pct: dec!(0.1),
        }
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

        let binance_pos = IsolatedMarginPosition {
            symbol: "BTCUSDT".to_string(),
            side: Side::Long,
            quantity: Quantity::new(dec!(0.1)).unwrap(),
            entry_price: Price::new(dec!(95000)).unwrap(),
            asset: "BTC".to_string(),
        };
        let current_price = Price::new(dec!(95000)).unwrap();

        monitor
            .process_binance_position(binance_pos, current_price)
            .await
            .unwrap();

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
        let exit = Price::new(dec!(93100)).unwrap(); // Loss (2% stop)
        let qty = Quantity::new(dec!(0.1)).unwrap();

        let pnl = monitor.calculate_expected_pnl(Side::Long, entry, exit, qty);

        // LONG: (93100 - 95000) * 0.1 = -190
        assert_eq!(pnl, dec!(-190));
    }
}
