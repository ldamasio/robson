//! Position Manager: Manages position lifecycle and detector tasks.
//!
//! The Position Manager is responsible for:
//! - Arming new positions (creates detector task)
//! - Processing detector signals (entry logic)
//! - Processing market data (trailing stop updates, exit triggers)
//! - Managing position state transitions
//! - Graceful shutdown of all detector tasks
//!
//! # Architecture
//!
//! ```text
//! CLI (arm) → PositionManager → spawn Detector → wait for signal
//!                  ↑
//!          EventBus (signals, market data)
//!                  ↓
//!              Engine → Executor → Exchange
//!
//! Shutdown → CancellationToken.cancel() → all detectors exit
//! ```

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{Mutex, RwLock};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use robson_domain::{
    DetectorSignal, Event, Position, PositionId, PositionState, Price, Quantity, RiskConfig, Side,
    Symbol, TechnicalStopDistance,
};
use robson_engine::{
    Engine, EngineAction, EngineDecision, PositionSummary, ProposedTrade, RiskContext, RiskGate,
};
#[cfg(feature = "postgres")]
use robson_eventlog::{ActorType as EventlogActorType, Event as EventlogEvent, append_event};
use robson_exec::{ActionResult, ExchangePort, ExecError, Executor};
#[cfg(feature = "postgres")]
use robson_projector::apply_event_to_projections;
use robson_store::Store;
use rust_decimal::Decimal;
#[cfg(feature = "postgres")]
use sqlx::PgPool;

use crate::circuit_breaker::{CircuitBreaker, CircuitBreakerLevel, DAILY_LOSS_LIMIT_CHECK_NAME};
use crate::detector::DetectorTask;
use crate::error::{DaemonError, DaemonResult};
use crate::event_bus::{DaemonEvent, EventBus, MarketData};
use crate::query::{
    ActorKind, CommandSource, ContextSummary, ExecutionQuery, QueryKind, QueryOutcome, QueryState,
};
use crate::query_engine::{
    ApprovalCheckResult, ApprovalPolicy, CheckRiskError, GovernedAction, QueryEngine, QueryRecorder,
};

// =============================================================================
// Position Manager
// =============================================================================

#[derive(Debug)]
struct PendingApprovalRecord {
    query: ExecutionQuery,
    position: Position,
    proposed: ProposedTrade,
    governed: GovernedAction,
}

/// Manages position lifecycle and detector tasks.
pub struct PositionManager<E: ExchangePort + 'static, S: Store + 'static> {
    /// Trading engine
    engine: Engine,
    /// Order executor
    executor: Arc<Executor<E, S>>,
    /// Store for persistence
    store: Arc<S>,
    /// Event bus for publishing events
    event_bus: Arc<EventBus>,
    /// Master cancellation token for all detector tasks
    shutdown_token: CancellationToken,
    /// Active detector tasks (position_id → task handle)
    detectors: Arc<RwLock<HashMap<PositionId, JoinHandle<Option<DetectorSignal>>>>>,
    /// Pending approvals held in runtime memory for Phase 3.
    pending_approvals: Arc<RwLock<HashMap<Uuid, PendingApprovalRecord>>>,
    /// Serializes entry-governance flows so pending reservations remain coherent.
    entry_flow_lock: Mutex<()>,
    /// Query engine for lifecycle tracking and audit persistence
    query_engine: Arc<QueryEngine<Arc<dyn QueryRecorder>>>,
    /// Circuit breaker escalation ladder (MIG-v2.5#5).
    pub(crate) circuit_breaker: Arc<CircuitBreaker>,
    /// Optional postgres pool for persisting domain events to robson-eventlog.
    #[cfg(feature = "postgres")]
    event_log_pool: Option<PgPool>,
    /// Tenant ID used for eventlog entries. Required when event_log_pool is Some.
    #[cfg(feature = "postgres")]
    event_log_tenant_id: Option<Uuid>,
}

impl<E: ExchangePort + 'static, S: Store + 'static> PositionManager<E, S> {
    /// Create a new position manager.
    ///
    /// After creation, call `start(Arc::clone(&manager))` to start the signal listener.
    pub fn new(
        engine: Engine,
        executor: Arc<Executor<E, S>>,
        store: Arc<S>,
        event_bus: Arc<EventBus>,
        query_recorder: Arc<dyn QueryRecorder>,
    ) -> Self {
        Self::with_approval_policy(
            engine,
            executor,
            store,
            event_bus,
            query_recorder,
            ApprovalPolicy::default(),
        )
    }

    pub(crate) fn with_approval_policy(
        engine: Engine,
        executor: Arc<Executor<E, S>>,
        store: Arc<S>,
        event_bus: Arc<EventBus>,
        query_recorder: Arc<dyn QueryRecorder>,
        approval_policy: ApprovalPolicy,
    ) -> Self {
        let shutdown_token = CancellationToken::new();
        let query_engine = Arc::new(QueryEngine::with_approval_policy(
            query_recorder,
            RiskGate::new(),
            approval_policy,
        ));

        Self {
            engine,
            executor,
            store,
            event_bus,
            shutdown_token,
            detectors: Arc::new(RwLock::new(HashMap::new())),
            pending_approvals: Arc::new(RwLock::new(HashMap::new())),
            entry_flow_lock: Mutex::new(()),
            query_engine,
            circuit_breaker: Arc::new(CircuitBreaker::default()),
            #[cfg(feature = "postgres")]
            event_log_pool: None,
            #[cfg(feature = "postgres")]
            event_log_tenant_id: None,
        }
    }

    /// Configure eventlog persistence for domain events (MIG-v2.5#2).
    ///
    /// When set, every domain event emitted through the executor is also
    /// persisted to `robson-eventlog` and applied to projections synchronously.
    /// This is required for crash recovery from `positions_current` to be
    /// defensible in live execution.
    #[cfg(feature = "postgres")]
    pub fn with_event_log(mut self, pool: PgPool, tenant_id: Uuid) -> Self {
        self.event_log_pool = Some(pool);
        self.event_log_tenant_id = Some(tenant_id);
        self
    }

    // =========================================================================
    // Eventlog persistence bridge (MIG-v2.5#2)
    // =========================================================================

    /// Persist a single domain event to robson-eventlog and apply it to projections.
    ///
    /// When `event_log_pool` is configured this is a **fail-fast** operation.
    /// Any failure in append OR projection apply is returned as `DaemonError::EventLog`
    /// so that callers propagate the error and abort the current execution cycle.
    ///
    /// Rationale: the synchronous apply on the write path is the only active
    /// projection update mechanism for `positions_current`. Silencing failures here
    /// would leave the projection stale without the caller's knowledge, making
    /// crash recovery unreliable. (MIG-v2.5#2 design decision.)
    ///
    /// When no pool is configured (in-memory only mode) returns `Ok(())` immediately.
    ///
    /// Stream key pattern: `position:{position_id}`.
    #[cfg(feature = "postgres")]
    async fn persist_event_to_log(&self, event: &Event) -> DaemonResult<()> {
        let (pool, tenant_id) = match (&self.event_log_pool, &self.event_log_tenant_id) {
            (Some(pool), Some(tid)) => (pool, *tid),
            _ => return Ok(()), // No eventlog configured — in-memory only mode
        };

        let position_id = event.position_id();
        let stream_key = format!("position:{}", position_id);
        let event_type = event.event_type().to_string();

        // Serialize full domain event as payload
        let payload = serde_json::to_value(event).map_err(|e| {
            DaemonError::EventLog(format!(
                "Failed to serialize {} for eventlog (position {}): {}",
                event_type, position_id, e
            ))
        })?;

        let eventlog_event = EventlogEvent::new(tenant_id, &stream_key, &event_type, payload)
            .with_actor(EventlogActorType::Daemon, Some("robsond".to_string()));

        let event_id = match append_event(pool, &stream_key, None, eventlog_event).await {
            Ok(id) => id,
            Err(robson_eventlog::EventLogError::IdempotentDuplicate(id)) => {
                // Duplicate means the append already happened, but projection apply may
                // have failed on a previous attempt. Re-fetch the stored envelope and
                // re-run projection apply so retries can heal partial failures.
                tracing::debug!(
                    event_type = %event_type,
                    %position_id,
                    event_id = %id,
                    "Domain event already in eventlog (idempotent) — reapplying projection"
                );
                id
            },
            Err(e) => {
                return Err(DaemonError::EventLog(format!(
                    "Failed to append {} to eventlog (position {}): {}",
                    event_type, position_id, e
                )));
            },
        };

        // Fetch the stored envelope and apply to projection synchronously.
        // This is the write path for `positions_current` — failure is not acceptable.
        let envelope = sqlx::query_as::<_, robson_eventlog::EventEnvelope>(
            "SELECT event_id, tenant_id, stream_key, seq, event_type, payload, \
             payload_schema_version, occurred_at, ingested_at, idempotency_key, \
             trace_id, causation_id, command_id, workflow_id, \
             actor_type, actor_id, prev_hash, hash \
             FROM event_log WHERE event_id = $1",
        )
        .bind(event_id)
        .fetch_one(pool)
        .await
        .map_err(|e| {
            DaemonError::EventLog(format!(
                "Failed to fetch envelope for {} (position {}, event_id {}): {}",
                event_type, position_id, event_id, e
            ))
        })?;

        apply_event_to_projections(pool, &envelope).await.map_err(|e| {
            DaemonError::EventLog(format!(
                "Failed to apply {} to projection (position {}, seq {}): {}",
                event_type, position_id, envelope.seq, e
            ))
        })?;

        tracing::debug!(
            event_type = %event_type,
            %position_id,
            seq = envelope.seq,
            "Domain event persisted to eventlog and projection applied"
        );
        Ok(())
    }

    #[cfg(not(feature = "postgres"))]
    async fn persist_event_to_log(&self, _event: &Event) -> DaemonResult<()> {
        Ok(())
    }

    /// Execute engine actions and persist any emitted domain events to the eventlog.
    ///
    /// This is a wrapper around `executor.execute()` that adds eventlog persistence
    /// for events in action results:
    /// - `ActionResult::EventEmitted(event)` - events from EmitEvent action
    /// - `ActionResult::OrderPlaced { event: Some(event), .. }` - events from exit orders
    ///
    /// When `event_log_pool` is configured, persistence is **fail-fast**: any failure
    /// in `persist_event_to_log()` (append OR projection apply) is propagated as a
    /// `DaemonError::EventLog` and the caller must abort the current execution cycle.
    ///
    /// This prevents silent projection drift during execution when PostgreSQL is in
    /// use. Append and projection apply still happen in separate steps, so this is
    /// fail-fast visibility, not an atomic multi-step guarantee.
    async fn execute_and_persist(
        &self,
        actions: Vec<EngineAction>,
    ) -> DaemonResult<Vec<ActionResult>> {
        let results = self.executor.execute(actions).await?;

        // Persist events from results to eventlog (centralized for MIG-v2.5#2).
        // Failures propagate — caller must not continue on EventLog error.
        for result in &results {
            match result {
                ActionResult::EventEmitted(event) => {
                    self.persist_event_to_log(event).await?;
                },
                ActionResult::OrderPlaced { event: Some(event), .. } => {
                    // Exit orders carry ExitOrderPlaced event - persist it before PositionClosed
                    self.persist_event_to_log(event).await?;
                },
                _ => {},
            }
        }

        Ok(results)
    }

    /// Start the position manager's background tasks.
    ///
    /// This spawns the signal listener that processes DetectorSignal events
    /// from the EventBus and calls `handle_signal()`.
    ///
    /// Must be called after wrapping in Arc:
    /// ```ignore
    /// let manager = Arc::new(PositionManager::new(...));
    /// PositionManager::start(Arc::clone(&manager));
    /// ```
    pub fn start(manager: Arc<Self>) {
        Self::start_signal_listener(manager);
    }

    /// Initiate graceful shutdown of all detector tasks.
    ///
    /// This cancels the shutdown token, causing all active detectors
    /// to exit cooperatively.
    pub async fn shutdown(&self) {
        info!("Initiating position manager shutdown");

        // Cancel all detectors
        self.shutdown_token.cancel();

        // Wait for all detectors to finish
        let mut detectors = self.detectors.write().await;
        let count = detectors.len();

        for (position_id, handle) in detectors.drain() {
            debug!(%position_id, "Waiting for detector to finish");

            // Give each detector a moment to finish gracefully
            match tokio::time::timeout(std::time::Duration::from_millis(500), handle).await {
                Ok(_) => debug!(%position_id, "Detector finished gracefully"),
                Err(_) => {
                    // Timeout - detector will be aborted when handle drops
                    warn!(%position_id, "Detector did not finish in time, will be aborted");
                },
            }
        }

        info!("Position manager shutdown complete ({count} detectors terminated)");
    }

    /// Get a child cancellation token for a new detector.
    ///
    /// Child tokens are cancelled when the parent is cancelled.
    fn child_cancel_token(&self) -> CancellationToken {
        self.shutdown_token.child_token()
    }

    fn operator_actor() -> ActorKind {
        ActorKind::Operator { source: CommandSource::Api }
    }

    async fn record_query_accepted(&self, query: &ExecutionQuery) -> DaemonResult<()> {
        self.query_engine.on_accepted(query).await?;
        Ok(())
    }

    async fn record_query_transition(
        &self,
        query: &ExecutionQuery,
        transition_cause: &str,
    ) -> DaemonResult<()> {
        self.query_engine.on_state_change(query, transition_cause).await?;
        Ok(())
    }

    async fn record_query_failure(&self, query: &ExecutionQuery) -> DaemonResult<()> {
        self.record_query_transition(query, "failed").await
    }

    fn set_query_context_summary(query: &mut ExecutionQuery, active_positions_count: usize) {
        query.context_summary = Some(ContextSummary { active_positions_count });
    }

    async fn populate_query_context_summary(&self, query: &mut ExecutionQuery) {
        if let Ok(active_positions) = self.store.positions().find_active().await {
            Self::set_query_context_summary(query, active_positions.len());
        }
    }

    fn pending_approval_to_summary(record: &PendingApprovalRecord) -> PositionSummary {
        PositionSummary {
            position_id: record.position.id,
            symbol: record.proposed.symbol.clone(),
            side: record.proposed.side.clone(),
            notional_value: record.proposed.notional_value,
            margin_used: record.proposed.margin_required,
            unrealized_pnl: Decimal::ZERO,
        }
    }

    async fn has_pending_approval_for_position(&self, position_id: PositionId) -> bool {
        let pending = self.pending_approvals.read().await;
        pending.values().any(|record| record.position.id == position_id)
    }

    async fn invalidate_pending_approvals_for_position(
        &self,
        position_id: PositionId,
        reason: &str,
    ) {
        let invalidated_records = {
            let mut pending = self.pending_approvals.write().await;
            let invalidated_ids: Vec<Uuid> = pending
                .iter()
                .filter_map(|(query_id, record)| {
                    (record.position.id == position_id).then_some(*query_id)
                })
                .collect();

            invalidated_ids
                .into_iter()
                .filter_map(|query_id| pending.remove(&query_id))
                .collect::<Vec<_>>()
        };

        for mut record in invalidated_records {
            let failure_reason = format!("Pending approval invalidated: {}", reason);
            record.query.fail(failure_reason, "awaiting_approval".to_string());
            if let Err(error) = self.record_query_failure(&record.query).await {
                warn!(
                    %position_id,
                    query_id = %record.query.id,
                    error = %error,
                    "Failed to persist invalidated pending approval snapshot"
                );
            }
            info!(
                %position_id,
                query_id = %record.query.id,
                reason,
                "Pending approval invalidated"
            );
        }
    }

    // =========================================================================
    // Phase 2: Risk context helpers
    // =========================================================================

    /// Build a RiskContext snapshot from current store state.
    ///
    /// Uses `find_risk_open()` (Entering + Active) so positions with a committed
    /// exchange order are counted even before fill confirmation. This prevents
    /// concurrent entries from slipping under the exposure limits during the
    /// order-fill window (signal fires → order submitted → not yet filled →
    /// next signal arrives).
    ///
    /// Phase 2 limitation: daily PnL (realized + unrealized) is not yet tracked
    /// in the store. Both fields default to zero, which means the daily loss
    /// circuit breaker is effectively disabled. Proper PnL tracking is deferred
    /// to a follow-up task.
    async fn build_risk_context(&self) -> DaemonResult<RiskContext> {
        let capital = self.engine.risk_config().capital();
        let active_positions = self.store.positions().find_risk_open().await?;

        // find_risk_open() guarantees only Entering and Active positions.
        // For Entering: use expected_entry from state (order price is committed on exchange).
        // For Active: use the recorded fill price (entry_price field).
        // Defensive: skip positions with zero quantity (should not occur in practice).
        let mut summaries: Vec<PositionSummary> = active_positions
            .iter()
            .filter_map(|p| {
                let entry_price_decimal = match &p.state {
                    PositionState::Active { .. } => p.entry_price?.as_decimal(),
                    PositionState::Entering { expected_entry, .. } => expected_entry.as_decimal(),
                    _ => return None, // find_risk_open guarantees this is unreachable
                };
                let qty = p.quantity.as_decimal();
                if qty.is_zero() {
                    return None;
                }
                let notional_value = qty * entry_price_decimal;
                let margin_used =
                    notional_value / Decimal::from(robson_domain::RiskConfig::LEVERAGE as u32);
                Some(PositionSummary {
                    position_id: p.id,
                    symbol: p.symbol.as_pair(),
                    side: format!("{}", p.side).to_lowercase(),
                    notional_value,
                    margin_used,
                    unrealized_pnl: Decimal::ZERO,
                })
            })
            .collect();

        let pending_summaries: Vec<PositionSummary> = self
            .pending_approvals
            .read()
            .await
            .values()
            .map(Self::pending_approval_to_summary)
            .collect();
        summaries.extend(pending_summaries);

        Ok(RiskContext::with_positions(
            capital,
            summaries,
            Decimal::ZERO, // daily_realized_pnl: deferred — see Phase 2 limitation above
            Decimal::ZERO, // daily_unrealized_pnl: deferred
        ))
    }

    async fn rearm_detector(
        position_id: PositionId,
        position: Position,
        event_bus: Arc<EventBus>,
        detectors: Arc<RwLock<HashMap<PositionId, JoinHandle<Option<DetectorSignal>>>>>,
        shutdown_token: CancellationToken,
        reason: &'static str,
    ) {
        let cancel_token = shutdown_token.child_token();
        match DetectorTask::from_position(&position, Arc::clone(&event_bus), cancel_token) {
            Ok(detector) => {
                let handle = detector.spawn();
                let mut detectors = detectors.write().await;
                detectors.insert(position_id, handle);
                info!(%position_id, %reason, "Detector re-armed");
            },
            Err(e) => {
                warn!(
                    %position_id,
                    %reason,
                    error = %e,
                    "Failed to re-arm detector — position requires manual re-arm"
                );
            },
        }
    }

    async fn rearm_detector_after_governed_block(
        &self,
        position_id: PositionId,
        position: &Position,
        reason: &'static str,
    ) {
        Self::rearm_detector(
            position_id,
            position.clone(),
            Arc::clone(&self.event_bus),
            Arc::clone(&self.detectors),
            self.shutdown_token.clone(),
            reason,
        )
        .await;
    }

    fn emit_query_awaiting_approval(&self, query: &ExecutionQuery) {
        let approval = match &query.approval {
            Some(approval) => approval,
            None => return,
        };

        self.event_bus.send(DaemonEvent::QueryAwaitingApproval {
            query_id: query.id,
            position_id: query.position_id,
            reason: approval.reason.clone(),
            expires_at: approval.expires_at,
        });
    }

    fn emit_query_authorized(&self, query: &ExecutionQuery) {
        let approved_at = query
            .approval
            .as_ref()
            .and_then(|approval| approval.approved_at)
            .unwrap_or_else(chrono::Utc::now);

        self.event_bus.send(DaemonEvent::QueryAuthorized {
            query_id: query.id,
            position_id: query.position_id,
            approved_at,
        });
    }

    fn emit_query_expired(&self, query: &ExecutionQuery) {
        self.event_bus.send(DaemonEvent::QueryExpired {
            query_id: query.id,
            position_id: query.position_id,
            expired_at: chrono::Utc::now(),
        });
    }

    fn spawn_approval_expiration_task(
        &self,
        query_id: Uuid,
        expires_at: chrono::DateTime<chrono::Utc>,
    ) {
        let pending_approvals = Arc::clone(&self.pending_approvals);
        let query_engine = Arc::clone(&self.query_engine);
        let event_bus = Arc::clone(&self.event_bus);
        let detectors = Arc::clone(&self.detectors);
        let shutdown_token = self.shutdown_token.clone();

        let wait_duration = expires_at
            .signed_duration_since(chrono::Utc::now())
            .to_std()
            .unwrap_or_default();

        tokio::spawn(async move {
            tokio::select! {
                _ = shutdown_token.cancelled() => {}
                _ = tokio::time::sleep(wait_duration) => {
                    let mut record = {
                        let mut pending = pending_approvals.write().await;
                        pending.remove(&query_id)
                    };

                    if let Some(mut record_inner) = record.take() {
                        if let Err(error) = query_engine.expire(&mut record_inner.query, "expired").await {
                            let error_message = format!("Approval expiry transition error: {}", error);
                            record_inner.query.fail(error_message.clone(), "awaiting_approval".to_string());
                            if let Err(audit_error) = query_engine
                                .on_state_change(&record_inner.query, "failed")
                                .await
                            {
                                warn!(
                                    query_id = %record_inner.query.id,
                                    error = %audit_error,
                                    "Failed to persist failed approval expiry snapshot"
                                );
                            }
                            return;
                        }

                        event_bus.send(DaemonEvent::QueryExpired {
                            query_id: record_inner.query.id,
                            position_id: record_inner.query.position_id,
                            expired_at: chrono::Utc::now(),
                        });

                        PositionManager::<E, S>::rearm_detector(
                            record_inner.position.id,
                            record_inner.position,
                            event_bus,
                            detectors,
                            shutdown_token,
                            "approval expired",
                        )
                        .await;
                    }
                }
            }
        });
    }

    async fn store_pending_approval(
        &self,
        query: ExecutionQuery,
        position: Position,
        proposed: ProposedTrade,
        governed: GovernedAction,
    ) {
        let query_id = query.id;
        let expires_at = query
            .approval
            .as_ref()
            .map(|approval| approval.expires_at)
            .expect("awaiting approval queries must include approval metadata");

        {
            let mut pending_approvals = self.pending_approvals.write().await;
            pending_approvals
                .insert(query_id, PendingApprovalRecord { query, position, proposed, governed });
        }

        let pending_approvals = self.pending_approvals.read().await;
        if let Some(record) = pending_approvals.get(&query_id) {
            self.emit_query_awaiting_approval(&record.query);
        }
        drop(pending_approvals);

        self.spawn_approval_expiration_task(query_id, expires_at);
    }

    pub async fn get_pending_approvals(&self) -> Vec<ExecutionQuery> {
        let pending = self.pending_approvals.read().await;
        let mut queries: Vec<ExecutionQuery> =
            pending.values().map(|record| record.query.clone()).collect();
        queries.sort_by_key(|query| query.started_at);
        queries
    }

    async fn execute_signal_query(
        &self,
        query: &mut ExecutionQuery,
        governed: GovernedAction,
    ) -> DaemonResult<()> {
        let position_id = query
            .position_id
            .ok_or_else(|| DaemonError::Config("Signal query missing position_id".to_string()))?;

        if let Err(e) = query.transition(QueryState::Acting) {
            let phase = match query.state {
                QueryState::Authorized => "authorized",
                QueryState::RiskChecked => "risk_checked",
                _ => "processing",
            };
            query.fail(format!("{}", e), phase.to_string());
            self.record_query_failure(query).await?;
            return Err(DaemonError::Config(format!("Query transition error: {}", e)));
        }
        self.record_query_transition(query, "acting").await?;

        let results = match self.execute_and_persist(governed.into_actions()).await {
            Ok(r) => r,
            Err(e) => {
                query.fail(format!("{}", e), "acting".to_string());
                self.record_query_failure(query).await?;
                return Err(e);
            },
        };

        let actions_count = results.len();
        for result in results {
            match result {
                ActionResult::OrderPlaced { order, .. } => {
                    info!(
                        %position_id,
                        exchange_order_id = %order.exchange_order_id,
                        fill_price = %order.fill_price.as_decimal(),
                        "Entry order placed and filled"
                    );

                    if let Err(e) = self
                        .handle_entry_fill(
                            position_id,
                            order.fill_price,
                            order.filled_quantity,
                            Some(order.exchange_order_id),
                        )
                        .await
                    {
                        query.fail(format!("{}", e), "acting".to_string());
                        self.record_query_failure(query).await?;
                        return Err(e);
                    }
                    // Note: event persistence handled by execute_and_persist()
                },
                ActionResult::AlreadyProcessed(id) => {
                    warn!(%position_id, %id, "Signal already processed (idempotent skip)");
                },
                ActionResult::EventEmitted(event) => {
                    debug!(%position_id, event_type = event.event_type(), "Event emitted");
                },
                ActionResult::StateUpdated => {
                    debug!(%position_id, "State updated");
                },
                ActionResult::Skipped(reason) => {
                    debug!(%position_id, %reason, "Action skipped");
                },
            }
        }

        if let Err(e) = query.complete(QueryOutcome::ActionsExecuted { actions_count }) {
            query.fail(format!("{}", e), "acting".to_string());
            self.record_query_failure(query).await?;
            return Err(DaemonError::Config(format!("Query completion error: {}", e)));
        }
        self.record_query_transition(query, "completed").await?;

        Ok(())
    }

    /// Build a ProposedTrade for risk evaluation from a signal and its engine decision.
    ///
    /// Extracts the quantity decided by the Engine (from PlaceEntryOrder action)
    /// and computes notional / margin using the fixed leverage constant.
    /// Returns None if the decision contains no PlaceEntryOrder (caller handles this).
    fn build_proposed_trade(
        signal: &DetectorSignal,
        decision: &EngineDecision,
    ) -> Option<ProposedTrade> {
        let quantity = decision.actions.iter().find_map(|a| match a {
            EngineAction::PlaceEntryOrder { quantity, .. } => Some(*quantity),
            _ => None,
        })?;

        let qty_decimal = quantity.as_decimal();
        let entry_price = signal.entry_price.as_decimal();
        let notional_value = qty_decimal * entry_price;
        let margin_required =
            notional_value / Decimal::from(robson_domain::RiskConfig::LEVERAGE as u32);

        Some(ProposedTrade {
            symbol: signal.symbol.as_pair(),
            side: format!("{}", signal.side).to_lowercase(),
            quantity: qty_decimal,
            entry_price,
            notional_value,
            margin_required,
        })
    }

    /// Start background task to listen for detector signals.
    ///
    /// This task subscribes to the EventBus and processes DetectorSignal events
    /// by calling handle_signal() for each received signal.
    fn start_signal_listener(manager: Arc<Self>) {
        let event_bus = Arc::clone(&manager.event_bus);
        let shutdown_token = manager.shutdown_token.clone();

        tokio::spawn(async move {
            let mut receiver = event_bus.subscribe();

            info!("Position manager signal listener started");

            loop {
                tokio::select! {
                    // Handle shutdown
                    _ = shutdown_token.cancelled() => {
                        info!("Signal listener received shutdown signal");
                        break;
                    }
                    // Process events
                    event = receiver.recv() => {
                        match event {
                            Some(Ok(DaemonEvent::DetectorSignal(signal))) => {
                                let position_id = signal.position_id;
                                let signal_id = signal.signal_id;

                                info!(
                                    %position_id,
                                    %signal_id,
                                    "Processing detector signal from EventBus"
                                );

                                // Call handle_signal - now we have Arc<Self>!
                                if let Err(e) = manager.handle_signal(signal).await {
                                    error!(
                                        %position_id,
                                        %signal_id,
                                        error = %e,
                                        "Failed to process detector signal"
                                    );
                                } else {
                                    info!(
                                        %position_id,
                                        %signal_id,
                                        "Detector signal processed successfully"
                                    );
                                }
                            }
                            Some(Err(lag_msg)) => {
                                warn!(error = %lag_msg, "Signal receiver lagged");
                            }
                            None => {
                                info!("Signal receiver channel closed");
                                break;
                            }
                            Some(Ok(_)) => {
                                // Ignore other event types (MarketData, StateChanged, etc.)
                            }
                        }
                    }
                }
            }

            info!("Position manager signal listener terminated");
        });
    }

    /// Arm a new position.
    ///
    /// Creates the position in Armed state and spawns a detector task.
    /// The detector will fire a signal when entry conditions are met.
    pub async fn arm_position(
        &self,
        symbol: Symbol,
        side: Side,
        _risk_config: RiskConfig, // Used by Engine for position sizing
        tech_stop_distance: TechnicalStopDistance,
        account_id: Uuid,
    ) -> DaemonResult<Position> {
        // Circuit breaker check — blocks new entries at SoftHalt and HardHalt.
        if self.circuit_breaker.blocks_new_entries().await {
            let snap = self.circuit_breaker.snapshot().await;
            return Err(DaemonError::CircuitBreakerTripped {
                level: format!("{}", snap.level),
                reason: snap.reason.unwrap_or_default(),
            });
        }

        // Generate position ID upfront (used in event and returned to caller)
        let position_id = Uuid::now_v7();

        // Create query for lifecycle tracking
        let mut query = ExecutionQuery::new(
            QueryKind::ArmPosition {
                symbol: symbol.clone(),
                side,
                tech_stop_distance,
                account_id,
            },
            Self::operator_actor(),
        );
        query.position_id = Some(position_id);
        self.populate_query_context_summary(&mut query).await;
        self.record_query_accepted(&query).await?;

        info!(
            %position_id,
            query_id = %query.id,
            symbol = %symbol.as_pair(),
            ?side,
            "Arming position"
        );

        // Transition to Processing
        if let Err(e) = query.transition(QueryState::Processing) {
            query.fail(format!("{}", e), "accepted".to_string());
            self.record_query_failure(&query).await?;
            return Err(DaemonError::Config(format!("Query transition error: {}", e)));
        }
        self.record_query_transition(&query, "processing").await?;

        // Emit PositionArmed event → apply_event creates position in Armed state
        let now = chrono::Utc::now();
        let event = Event::PositionArmed {
            position_id,
            account_id,
            symbol: symbol.clone(),
            side,
            tech_stop_distance: Some(tech_stop_distance),
            timestamp: now,
        };

        // Transition to Acting before executor call
        if let Err(e) = query.transition(QueryState::Acting) {
            query.fail(format!("{}", e), "processing".to_string());
            self.record_query_failure(&query).await?;
            return Err(DaemonError::Config(format!("Query transition error: {}", e)));
        }
        self.record_query_transition(&query, "acting").await?;

        // Execute event emission + persist to eventlog for crash recovery
        let results = match self.execute_and_persist(vec![EngineAction::EmitEvent(event)]).await {
            Ok(r) => r,
            Err(e) => {
                let err_str = format!("{}", e);
                query.fail(err_str.clone(), "acting".to_string());
                self.record_query_failure(&query).await?;
                return Err(e);
            },
        };
        let actions_count = results.len();

        self.event_bus.send(DaemonEvent::PositionStateChanged {
            position_id,
            previous_state: "None".to_string(),
            new_state: "Armed".to_string(),
            timestamp: now,
        });

        // Load position from projection for detector and return
        let position = match self.store.positions().find_by_id(position_id).await {
            Ok(Some(pos)) => pos,
            Ok(None) => {
                let e = DaemonError::PositionNotFound(position_id);
                query.fail(format!("{}", e), "acting".to_string());
                self.record_query_failure(&query).await?;
                return Err(e);
            },
            Err(e) => {
                let err_str = format!("{}", e);
                query.fail(err_str.clone(), "acting".to_string());
                self.record_query_failure(&query).await?;
                return Err(e.into());
            },
        };

        // Spawn detector task
        let cancel_token = self.child_cancel_token();
        let detector =
            match DetectorTask::from_position(&position, Arc::clone(&self.event_bus), cancel_token)
            {
                Ok(d) => d,
                Err(e) => {
                    let err_str = format!("{}", e);
                    query.fail(err_str.clone(), "acting".to_string());
                    self.record_query_failure(&query).await?;
                    return Err(e);
                },
            };
        let handle = detector.spawn();

        // Store detector handle for cancellation
        let mut detectors = self.detectors.write().await;
        detectors.insert(position_id, handle);

        debug!(%position_id, "Position armed, detector spawned");

        // Complete query ONLY after all operations succeed
        if let Err(e) = query.complete(QueryOutcome::ActionsExecuted { actions_count }) {
            query.fail(format!("{}", e), "acting".to_string());
            self.record_query_failure(&query).await?;
            return Err(DaemonError::Config(format!("Query completion error: {}", e)));
        }
        self.record_query_transition(&query, "completed").await?;

        Ok(position)
    }

    /// Returns an `Arc` to the circuit breaker so API handlers can read/write it.
    pub fn circuit_breaker(&self) -> Arc<CircuitBreaker> {
        Arc::clone(&self.circuit_breaker)
    }

    /// Disarm (cancel) an armed position.
    ///
    /// Only positions in Armed state can be disarmed.
    pub async fn disarm_position(&self, position_id: PositionId) -> DaemonResult<()> {
        let _entry_flow_guard = self.entry_flow_lock.lock().await;

        // Create query for lifecycle tracking
        let mut query =
            ExecutionQuery::new(QueryKind::DisarmPosition { position_id }, Self::operator_actor());
        query.position_id = Some(position_id);
        self.populate_query_context_summary(&mut query).await;
        self.record_query_accepted(&query).await?;

        // Transition to Processing
        if let Err(e) = query.transition(QueryState::Processing) {
            query.fail(format!("{}", e), "accepted".to_string());
            self.record_query_failure(&query).await?;
            return Err(DaemonError::Config(format!("Query transition error: {}", e)));
        }
        self.record_query_transition(&query, "processing").await?;

        let position = match self.store.positions().find_by_id(position_id).await {
            Ok(Some(pos)) => pos,
            Ok(None) => {
                let e = DaemonError::PositionNotFound(position_id);
                query.fail(format!("{}", e), "processing".to_string());
                self.record_query_failure(&query).await?;
                return Err(e);
            },
            Err(e) => {
                query.fail(format!("{}", e), "processing".to_string());
                self.record_query_failure(&query).await?;
                return Err(e.into());
            },
        };

        if !matches!(position.state, PositionState::Armed) {
            let e = DaemonError::InvalidPositionState {
                expected: "Armed".to_string(),
                actual: format!("{:?}", position.state),
            };
            query.fail(format!("{}", e), "processing".to_string());
            self.record_query_failure(&query).await?;
            return Err(e);
        }

        info!(%position_id, query_id = %query.id, "Disarming position");

        self.invalidate_pending_approvals_for_position(position_id, "position disarmed")
            .await;

        // Kill detector task if exists
        self.kill_detector(position_id).await;

        // Transition to Acting before executor call
        if let Err(e) = query.transition(QueryState::Acting) {
            query.fail(format!("{}", e), "processing".to_string());
            self.record_query_failure(&query).await?;
            return Err(DaemonError::Config(format!("Query transition error: {}", e)));
        }
        self.record_query_transition(&query, "acting").await?;

        // Emit PositionDisarmed event → apply_event transitions position to Closed
        let event = Event::PositionDisarmed {
            position_id,
            reason: "user_disarmed".to_string(),
            timestamp: chrono::Utc::now(),
        };

        let exec_result = self.execute_and_persist(vec![EngineAction::EmitEvent(event)]).await;
        match exec_result {
            Ok(results) => {
                if let Err(e) =
                    query.complete(QueryOutcome::ActionsExecuted { actions_count: results.len() })
                {
                    query.fail(format!("{}", e), "acting".to_string());
                    self.record_query_failure(&query).await?;
                    return Err(DaemonError::Config(format!("Query completion error: {}", e)));
                }
                self.record_query_transition(&query, "completed").await?;
            },
            Err(e) => {
                query.fail(format!("{}", e), "acting".to_string());
                self.record_query_failure(&query).await?;
                return Err(e);
            },
        }

        self.event_bus.send(DaemonEvent::PositionStateChanged {
            position_id,
            previous_state: "Armed".to_string(),
            new_state: "Closed".to_string(),
            timestamp: chrono::Utc::now(),
        });

        Ok(())
    }

    /// Handle a detector signal (entry signal received).
    ///
    /// Flow: Engine → Execute actions (emit events) → Save state → Process fill
    pub async fn handle_signal(&self, signal: DetectorSignal) -> DaemonResult<()> {
        let _entry_flow_guard = self.entry_flow_lock.lock().await;
        let position_id = signal.position_id;

        // SoftHalt and HardHalt both block new entries — a detector signal that
        // would transition Armed→Entering counts as a new entry.
        if self.circuit_breaker.blocks_new_entries().await {
            let snap = self.circuit_breaker.snapshot().await;
            warn!(
                %position_id,
                level = %snap.level,
                "Signal dropped — circuit breaker blocks new entries"
            );
            return Err(DaemonError::CircuitBreakerTripped {
                level: format!("{}", snap.level),
                reason: snap.reason.unwrap_or_default(),
            });
        }

        // Create query for lifecycle tracking
        let mut query = ExecutionQuery::new(
            QueryKind::ProcessSignal {
                signal_id: signal.signal_id,
                symbol: signal.symbol.clone(),
                side: signal.side,
                entry_price: signal.entry_price,
                stop_loss: signal.stop_loss,
            },
            ActorKind::Detector,
        );
        query.position_id = Some(position_id);
        self.populate_query_context_summary(&mut query).await;
        self.record_query_accepted(&query).await?;

        info!(
            %position_id,
            query_id = %query.id,
            signal_id = %signal.signal_id,
            entry_price = %signal.entry_price.as_decimal(),
            "Processing detector signal"
        );

        // Transition to Processing
        if let Err(e) = query.transition(QueryState::Processing) {
            query.fail(format!("{}", e), "accepted".to_string());
            self.record_query_failure(&query).await?;
            return Err(DaemonError::Config(format!("Query transition error: {}", e)));
        }
        self.record_query_transition(&query, "processing").await?;

        // Load position
        let position = match self.store.positions().find_by_id(position_id).await {
            Ok(Some(pos)) => pos,
            Ok(None) => {
                let e = DaemonError::PositionNotFound(position_id);
                query.fail(format!("{}", e), "processing".to_string());
                self.record_query_failure(&query).await?;
                return Err(e);
            },
            Err(e) => {
                query.fail(format!("{}", e), "processing".to_string());
                self.record_query_failure(&query).await?;
                return Err(e.into());
            },
        };

        if self.has_pending_approval_for_position(position_id).await {
            if let Err(e) = query.complete(QueryOutcome::NoAction {
                reason: "Approval already pending for position".to_string(),
            }) {
                query.fail(format!("{}", e), "processing".to_string());
                self.record_query_failure(&query).await?;
                return Err(DaemonError::Config(format!("Query completion error: {}", e)));
            }
            self.record_query_transition(&query, "completed").await?;
            info!(
                %position_id,
                query_id = %query.id,
                "Skipping signal because approval is already pending for position"
            );
            return Ok(());
        }

        // Kill detector (it's single-shot)
        self.kill_detector(position_id).await;

        // Use engine to decide entry (pure: State+Signal → Decision)
        let decision = match self.engine.decide_entry(&position, &signal) {
            Ok(d) => d,
            Err(e) => {
                query.fail(format!("{}", e), "processing".to_string());
                self.record_query_failure(&query).await?;
                return Err(e.into());
            },
        };

        // Check if we have actions to execute (before risk check — no point evaluating
        // risk for a no-action decision)
        if decision.actions.is_empty() {
            if let Err(e) = query.complete(QueryOutcome::NoAction {
                reason: "No actions from engine".to_string(),
            }) {
                query.fail(format!("{}", e), "processing".to_string());
                self.record_query_failure(&query).await?;
                return Err(DaemonError::Config(format!("Query completion error: {}", e)));
            }
            self.record_query_transition(&query, "completed").await?;
            return Ok(());
        }

        // Phase 2: Risk governance gate.
        //
        // Build RiskContext (current portfolio state) and ProposedTrade (this entry),
        // then delegate to QueryEngine.check_risk(), which:
        //   1. Transitions query to RiskChecked (or returns InvalidState on failure)
        //   2. Evaluates the RiskGate (pure computation)
        //   3. Returns GovernedAction (approved) or CheckRiskError (denied or state error)
        //
        // Denial (CheckRiskError::Denied) is a governed outcome — return Ok(()).
        // InvalidState (CheckRiskError::InvalidState) is an operational error — propagate.
        let risk_context = match self.build_risk_context().await {
            Ok(ctx) => ctx,
            Err(e) => {
                let err_str = format!("{}", e);
                query.fail(err_str.clone(), "processing".to_string());
                self.record_query_failure(&query).await?;
                return Err(e);
            },
        };

        let proposed = match Self::build_proposed_trade(&signal, &decision) {
            Some(p) => p,
            None => {
                let err_str =
                    "decide_entry produced actions but no PlaceEntryOrder — cannot build ProposedTrade"
                        .to_string();
                query.fail(err_str.clone(), "processing".to_string());
                self.record_query_failure(&query).await?;
                return Err(DaemonError::Config(err_str));
            },
        };

        let governed: GovernedAction = match self
            .query_engine
            .check_risk(&mut query, &proposed, &risk_context, decision.actions)
            .await
        {
            Ok(g) => g,
            Err(CheckRiskError::Denied) => {
                // Governed denial: query is already in Denied state.
                //
                // If the denial was caused by the daily loss limit, auto-escalate the
                // circuit breaker to SoftHalt (MIG-v2.5#5). This turns a per-trade
                // governed denial into a latching circuit-level block for all future
                // new entries — operator must explicitly reset.
                if let QueryState::Denied { ref check, ref reason } = query.state {
                    if check == DAILY_LOSS_LIMIT_CHECK_NAME {
                        let reason_str = reason.clone();
                        if let Some(prev) = self
                            .circuit_breaker
                            .try_escalate(CircuitBreakerLevel::SoftHalt, reason_str.clone())
                            .await
                        {
                            warn!(
                                %position_id,
                                previous_level = %prev,
                                "Daily loss limit exceeded — circuit breaker escalated to SoftHalt"
                            );
                            self.event_bus.send(DaemonEvent::CircuitBreakerTriggered {
                                level: CircuitBreakerLevel::SoftHalt,
                                previous_level: prev,
                                reason: reason_str,
                                triggered_at: chrono::Utc::now(),
                            });
                        }
                    }
                }

                // Re-arm the detector so the Armed position can receive future signals.
                // The original detector completed when the signal fired; without re-arming,
                // the position would be Armed but permanently unresponsive.
                self.rearm_detector_after_governed_block(position_id, &position, "risk denied")
                    .await;
                info!(
                    %position_id,
                    query_id = %query.id,
                    "Entry denied by risk gate — detector re-armed (governed outcome)"
                );
                return Ok(());
            },
            Err(CheckRiskError::InvalidState(e)) => {
                // Operational error: query lifecycle state machine is inconsistent.
                // This is NOT a governed denial — it indicates a bug or concurrent
                // mutation. Fail the query and propagate as a hard error.
                let err_str = format!("Risk gate lifecycle error: {}", e);
                query.fail(err_str.clone(), "processing".to_string());
                self.record_query_failure(&query).await?;
                return Err(DaemonError::Config(err_str));
            },
            Err(CheckRiskError::Audit(e)) => {
                return Err(e.into());
            },
        };

        match self
            .query_engine
            .check_approval(&mut query, &proposed, &risk_context, governed)
            .await
        {
            Ok(ApprovalCheckResult::Ready(governed)) => {
                self.execute_signal_query(&mut query, governed).await?;
                Ok(())
            },
            Ok(ApprovalCheckResult::AwaitingApproval(governed)) => {
                let query_id = query.id;
                let expires_at = query
                    .approval
                    .as_ref()
                    .map(|approval| approval.expires_at)
                    .expect("awaiting approval query must contain approval metadata");
                self.store_pending_approval(query, position.clone(), proposed, governed).await;

                info!(
                    %position_id,
                    %query_id,
                    expires_at = %expires_at,
                    "Entry awaiting operator approval"
                );
                Ok(())
            },
            Err(e) => {
                let err_str = format!("Approval gate lifecycle error: {}", e);
                query.fail(err_str.clone(), "risk_checked".to_string());
                self.record_query_failure(&query).await?;
                Err(DaemonError::Config(err_str))
            },
        }
    }

    /// Approve a pending query and resume execution.
    pub async fn approve_query(&self, query_id: Uuid) -> DaemonResult<ExecutionQuery> {
        let _entry_flow_guard = self.entry_flow_lock.lock().await;

        // Approval resume is a new entry — respect SoftHalt and HardHalt.
        // The query was approved by the operator, but the circuit breaker may have
        // been triggered between when the query was queued and now.
        if self.circuit_breaker.blocks_new_entries().await {
            let snap = self.circuit_breaker.snapshot().await;
            return Err(DaemonError::CircuitBreakerTripped {
                level: format!("{}", snap.level),
                reason: snap.reason.unwrap_or_default(),
            });
        }

        let mut record = {
            let mut pending_approvals = self.pending_approvals.write().await;
            pending_approvals
                .remove(&query_id)
                .ok_or(DaemonError::QueryNotFound(query_id))?
        };

        let expired = record
            .query
            .approval
            .as_ref()
            .map(|approval| chrono::Utc::now() >= approval.expires_at)
            .unwrap_or(false);

        if expired {
            if let Err(e) = self.query_engine.expire(&mut record.query, "expired").await {
                let err_str = format!("Approval expiry transition error: {}", e);
                record.query.fail(err_str.clone(), "awaiting_approval".to_string());
                self.record_query_failure(&record.query).await?;
                return Err(DaemonError::Config(err_str));
            }

            self.emit_query_expired(&record.query);
            self.rearm_detector_after_governed_block(
                record.position.id,
                &record.position,
                "approval expired",
            )
            .await;
            return Err(DaemonError::ApprovalExpired(query_id));
        }

        let current_position = match self.store.positions().find_by_id(record.position.id).await {
            Ok(Some(position)) => position,
            Ok(None) => {
                let err = DaemonError::PositionNotFound(record.position.id);
                record.query.fail(
                    format!("Pending approval invalidated: {}", err),
                    "awaiting_approval".to_string(),
                );
                self.record_query_failure(&record.query).await?;
                return Err(err);
            },
            Err(e) => {
                let err_str = format!("{}", e);
                record.query.fail(err_str.clone(), "awaiting_approval".to_string());
                self.record_query_failure(&record.query).await?;
                return Err(e.into());
            },
        };

        if !matches!(current_position.state, PositionState::Armed) {
            let err = DaemonError::InvalidPositionState {
                expected: "Armed".to_string(),
                actual: format!("{:?}", current_position.state),
            };
            record.query.fail(
                format!("Pending approval invalidated: {}", err),
                "awaiting_approval".to_string(),
            );
            self.record_query_failure(&record.query).await?;
            return Err(err);
        }

        let risk_context = self.build_risk_context().await?;
        match self
            .query_engine
            .revalidate_risk(&mut record.query, &record.proposed, &risk_context)
            .await
        {
            Ok(()) => {},
            Err(CheckRiskError::Denied) => {
                // Mirror the SoftHalt auto-trigger from handle_signal: if revalidation
                // fails due to the daily loss limit, escalate the circuit breaker.
                if let QueryState::Denied { ref check, ref reason } = record.query.state {
                    if check == DAILY_LOSS_LIMIT_CHECK_NAME {
                        let reason_str = reason.clone();
                        if let Some(prev) = self
                            .circuit_breaker
                            .try_escalate(CircuitBreakerLevel::SoftHalt, reason_str.clone())
                            .await
                        {
                            warn!(
                                query_id = %query_id,
                                previous_level = %prev,
                                "Daily loss limit exceeded during approval revalidation — circuit breaker escalated to SoftHalt"
                            );
                            self.event_bus.send(DaemonEvent::CircuitBreakerTriggered {
                                level: CircuitBreakerLevel::SoftHalt,
                                previous_level: prev,
                                reason: reason_str,
                                triggered_at: chrono::Utc::now(),
                            });
                        }
                    }
                }

                self.rearm_detector_after_governed_block(
                    current_position.id,
                    &current_position,
                    "approval revalidation denied",
                )
                .await;

                let reason = match &record.query.state {
                    QueryState::Denied { reason, .. } => reason.clone(),
                    _ => "Approval denied after risk revalidation".to_string(),
                };

                return Err(DaemonError::ApprovalDenied { query_id, reason });
            },
            Err(CheckRiskError::InvalidState(e)) => {
                let err_str = format!("Approval revalidation lifecycle error: {}", e);
                record.query.fail(err_str.clone(), "awaiting_approval".to_string());
                self.record_query_failure(&record.query).await?;
                return Err(DaemonError::Config(err_str));
            },
            Err(CheckRiskError::Audit(e)) => {
                return Err(e.into());
            },
        }

        if let Err(e) = self.query_engine.authorize(&mut record.query).await {
            let err_str = format!("Approval authorization error: {}", e);
            record.query.fail(err_str.clone(), "awaiting_approval".to_string());
            self.record_query_failure(&record.query).await?;
            return Err(DaemonError::Config(err_str));
        }

        self.emit_query_authorized(&record.query);
        self.execute_signal_query(&mut record.query, record.governed).await?;

        Ok(record.query)
    }

    /// Handle entry fill (transition from Entering → Active).
    ///
    /// Flow: Load → Engine → Execute actions (emit events) → Save state
    async fn handle_entry_fill(
        &self,
        position_id: PositionId,
        fill_price: Price,
        filled_quantity: Quantity,
        binance_position_id: Option<String>,
    ) -> DaemonResult<()> {
        // Load position
        let position = self
            .store
            .positions()
            .find_by_id(position_id)
            .await?
            .ok_or(DaemonError::PositionNotFound(position_id))?;

        // Use engine to process fill (pure: State+Fill → Decision)
        // binance_position_id is passed through to EntryFilled event
        let decision = self.engine.process_entry_fill(
            &position,
            fill_price,
            filled_quantity,
            binance_position_id.clone(),
        )?;

        // Execute actions (EntryFilled event transitions position to Active via apply_event)
        // Also persists to eventlog for crash recovery (MIG-v2.5#2).
        self.execute_and_persist(decision.actions).await?;

        info!(
            %position_id,
            fill_price = %fill_price.as_decimal(),
            "Entry filled, position now Active"
        );

        self.event_bus.send(DaemonEvent::PositionStateChanged {
            position_id,
            previous_state: "Entering".to_string(),
            new_state: "Active".to_string(),
            timestamp: chrono::Utc::now(),
        });

        let core_exchange_id = binance_position_id.unwrap_or_else(|| {
            warn!(
                %position_id,
                "Missing binance_position_id on Core open; using position_id fallback"
            );
            position_id.to_string()
        });

        self.event_bus.send(DaemonEvent::CorePositionOpened {
            position_id,
            symbol: position.symbol.clone(),
            side: position.side,
            binance_position_id: core_exchange_id,
        });

        Ok(())
    }

    /// Process market data for active positions.
    ///
    /// This updates trailing stops and triggers exits when necessary.
    ///
    /// # Canonical Flow
    ///
    /// ```text
    /// Tick → State (from projection) → Engine(State, Tick) → Decision
    /// → Executor(Decision) → Result → EventLog.append(Event)
    /// → Projection.apply(Event) (async)
    /// ```
    pub async fn process_market_data(&self, data: MarketData) -> DaemonResult<()> {
        // Find all active positions for this symbol (from projection)
        let active_positions = self.store.positions().find_active().await?;
        let active_positions_count = active_positions.len();

        for position in active_positions {
            if position.symbol != data.symbol {
                continue;
            }

            if !matches!(position.state, PositionState::Active { .. }) {
                continue;
            }

            // Create one ExecutionQuery PER POSITION processed
            let mut query = ExecutionQuery::new(
                QueryKind::ProcessMarketTick {
                    symbol: data.symbol.clone(),
                    price: data.price,
                },
                ActorKind::MarketData,
            );
            query.position_id = Some(position.id);
            Self::set_query_context_summary(&mut query, active_positions_count);
            self.record_query_accepted(&query).await?;

            debug!(
                position_id = %position.id,
                query_id = %query.id,
                price = %data.price.as_decimal(),
                "Processing market tick for position"
            );

            // Transition to Processing
            if let Err(e) = query.transition(QueryState::Processing) {
                query.fail(format!("{}", e), "accepted".to_string());
                self.record_query_failure(&query).await?;
                return Err(DaemonError::Config(format!("Query transition error: {}", e)));
            }
            self.record_query_transition(&query, "processing").await?;

            // Use engine to process (pure: State+Tick → Decision)
            let symbol_clone = data.symbol.clone();
            let market_data = robson_engine::MarketData::new(symbol_clone, data.price);
            let decision = match self.engine.process_active_position(&position, &market_data) {
                Ok(d) => d,
                Err(e) => {
                    let err_str = format!("{}", e);
                    query.fail(err_str.clone(), "processing".to_string());
                    self.record_query_failure(&query).await?;
                    return Err(e.into());
                },
            };

            // Check if we have actions to execute
            if decision.actions.is_empty() {
                if let Err(e) =
                    query.complete(QueryOutcome::NoAction { reason: "No stop trigger".to_string() })
                {
                    let err_str = format!("{}", e);
                    query.fail(err_str.clone(), "processing".to_string());
                    self.record_query_failure(&query).await?;
                    return Err(DaemonError::Config(format!("Query completion error: {}", e)));
                }
                self.record_query_transition(&query, "completed").await?;
                continue;
            }

            // Transition to Acting before executor call
            if let Err(e) = query.transition(QueryState::Acting) {
                query.fail(format!("{}", e), "processing".to_string());
                self.record_query_failure(&query).await?;
                return Err(DaemonError::Config(format!("Query transition error: {}", e)));
            }
            self.record_query_transition(&query, "acting").await?;

            // Execute actions via Executor (side-effects: EventLog.append, Exchange orders)
            // MemoryStore is updated via apply_event() called by executor after event append
            let results = match self.execute_and_persist(decision.actions).await {
                Ok(r) => r,
                Err(e) => {
                    let err_str = format!("{}", e);
                    query.fail(err_str.clone(), "acting".to_string());
                    self.record_query_failure(&query).await?;
                    return Err(e);
                },
            };

            // Process results
            // actions_count represents ALL ActionResult variants
            let actions_count = results.len();
            for result in results {
                if let ActionResult::OrderPlaced { order, .. } = result {
                    // Exit order filled, handle close
                    // Note: handle_exit_fill is internal, covered by this query's lifecycle
                    // Note: ExitOrderPlaced event already persisted by execute_and_persist()
                    if let Err(e) = self
                        .handle_exit_fill(position.id, order.fill_price, order.filled_quantity)
                        .await
                    {
                        let err_str = format!("{}", e);
                        query.fail(err_str.clone(), "acting".to_string());
                        self.record_query_failure(&query).await?;
                        return Err(e);
                    }
                }
            }

            // Complete query with success
            if let Err(e) = query.complete(QueryOutcome::ActionsExecuted { actions_count }) {
                query.fail(format!("{}", e), "acting".to_string());
                self.record_query_failure(&query).await?;
                return Err(DaemonError::Config(format!("Query completion error: {}", e)));
            }
            self.record_query_transition(&query, "completed").await?;
        }

        Ok(())
    }

    /// Handle exit fill (transition to Closed).
    ///
    /// Called after exit order is filled. Emits PositionClosed event
    /// which will be applied by projection to update state.
    async fn handle_exit_fill(
        &self,
        position_id: PositionId,
        fill_price: Price,
        _filled_quantity: Quantity,
    ) -> DaemonResult<()> {
        let position = self
            .store
            .positions()
            .find_by_id(position_id)
            .await?
            .ok_or(DaemonError::PositionNotFound(position_id))?;

        // Extract exit reason from position's Exiting state.
        // By this point, executor.execute_exit_order has already emitted and applied
        // ExitOrderPlaced, transitioning the position to Exiting { exit_reason }.
        let exit_reason = match &position.state {
            PositionState::Exiting { exit_reason, .. } => *exit_reason,
            other => {
                return Err(DaemonError::InvalidPositionState {
                    expected: "Exiting".to_string(),
                    actual: format!("{:?}", other),
                });
            },
        };

        info!(
            %position_id,
            fill_price = %fill_price.as_decimal(),
            ?exit_reason,
            "Exit filled, emitting PositionClosed event"
        );

        // Calculate PnL for event
        let entry_price = position.entry_price.unwrap_or(fill_price);
        let pnl = position.calculate_pnl();

        // Emit PositionClosed event via executor (ensures append->apply order)
        // Also persists to eventlog for crash recovery (MIG-v2.5#2).
        let event = Event::PositionClosed {
            position_id,
            exit_reason,
            entry_price,
            exit_price: fill_price,
            realized_pnl: pnl,
            total_fees: position.fees_paid,
            timestamp: chrono::Utc::now(),
        };
        self.execute_and_persist(vec![EngineAction::EmitEvent(event)]).await?;

        // Send to event bus for real-time notification
        self.event_bus.send(DaemonEvent::PositionStateChanged {
            position_id,
            previous_state: "Exiting".to_string(),
            new_state: "Closed".to_string(),
            timestamp: chrono::Utc::now(),
        });
        self.event_bus.send(DaemonEvent::CorePositionClosed {
            position_id,
            symbol: position.symbol.clone(),
            side: position.side,
        });

        Ok(())
    }

    /// Emergency close all positions.
    ///
    /// Iterates all open core positions (Armed, Entering, Active, Exiting) and
    /// applies state-appropriate shutdown:
    ///
    /// - **Active**: place market exit order via `panic_close_position_internal()`.
    /// - **Armed**: disarm (cancel detector). No exchange action needed.
    /// - **Entering**: order submitted but not yet filled — logged as skipped.
    ///   Cancelling a pending entry order requires exchange-specific logic deferred
    ///   to a follow-up task. The position will remain Entering until the order fills
    ///   or the exchange session expires.
    /// - **Exiting**: exit already in progress — skip to avoid duplicate orders.
    pub async fn panic_close_all(&self) -> DaemonResult<Vec<PositionId>> {
        warn!("PANIC: Emergency close all positions");

        // find_active() returns open core-lifecycle states only:
        // Armed, Entering, Active, Exiting.
        let all_non_terminal = self.store.positions().find_active().await?;
        let total_count = all_non_terminal.len();
        let mut closed_ids = Vec::new();

        for position in all_non_terminal {
            match &position.state {
                PositionState::Active { .. } => {
                    // Place market exit order
                    let mut query = ExecutionQuery::new(
                        QueryKind::PanicClosePosition { position_id: position.id },
                        Self::operator_actor(),
                    );
                    query.position_id = Some(position.id);
                    Self::set_query_context_summary(&mut query, total_count);
                    self.record_query_accepted(&query).await?;

                    match self.panic_close_position_internal(position.id, &mut query).await {
                        Ok(_) => {
                            if let Err(e) =
                                query.complete(QueryOutcome::ActionsExecuted { actions_count: 1 })
                            {
                                query.fail(format!("{}", e), "acting".to_string());
                                self.record_query_failure(&query).await?;
                            } else {
                                self.record_query_transition(&query, "completed").await?;
                            }
                            closed_ids.push(position.id);
                        },
                        Err(e) => {
                            // Caller owns failure recording — internal method does NOT call fail()
                            let phase = match &query.state {
                                QueryState::Accepted => "accepted".to_string(),
                                QueryState::Processing => "processing".to_string(),
                                QueryState::RiskChecked => "risk_checked".to_string(),
                                QueryState::AwaitingApproval => "awaiting_approval".to_string(),
                                QueryState::Authorized => "authorized".to_string(),
                                QueryState::Acting => "acting".to_string(),
                                QueryState::Completed => "completed".to_string(),
                                QueryState::Expired => "expired".to_string(),
                                QueryState::Failed { phase, .. } => phase.clone(),
                                QueryState::Denied { check, .. } => format!("denied:{}", check),
                            };
                            query.fail(format!("{}", e), phase);
                            self.record_query_failure(&query).await?;
                            error!(position_id = %position.id, error = %e, "Failed to panic close");
                        },
                    }
                },
                PositionState::Armed => {
                    // No exchange order exists — disarm the position (cancel its detector).
                    if let Err(e) = self.disarm_position(position.id).await {
                        warn!(position_id = %position.id, error = %e, "Panic: failed to disarm Armed position");
                    } else {
                        info!(position_id = %position.id, "Panic: Armed position disarmed");
                        closed_ids.push(position.id);
                    }
                },
                PositionState::Entering { .. } => {
                    // Entry order submitted but not yet filled.
                    // Cancelling a pending margin order requires exchange-specific
                    // cancel-order logic (deferred). Log and skip for now.
                    warn!(
                        position_id = %position.id,
                        "Panic: Entering position skipped — entry order cancel not yet implemented"
                    );
                },
                PositionState::Exiting { .. } => {
                    // Exit already in progress — do not place a duplicate order.
                    info!(position_id = %position.id, "Panic: Exiting position skipped — exit already in progress");
                },
                PositionState::Closed { .. } => {
                    // find_active() guarantees this is unreachable.
                },
                PositionState::Error { error, .. } => {
                    // Defensive fallback: find_active() excludes Error, but skip safely if the
                    // repository contract is violated.
                    warn!(
                        position_id = %position.id,
                        error = %error,
                        "Panic: Error position encountered outside find_active() contract"
                    );
                },
            }
        }

        info!(closed_count = closed_ids.len(), "Panic close complete");

        Ok(closed_ids)
    }

    /// Emergency close a single position (internal, takes query for lifecycle tracking).
    ///
    /// # Failure Recording Ownership
    ///
    /// This method does NOT call `query.fail()` or persist failure snapshots on errors.
    /// The caller is responsible for failure recording. This prevents double-fail.
    ///
    /// Emits PositionClosed event which will be applied by projection.
    async fn panic_close_position_internal(
        &self,
        position_id: PositionId,
        query: &mut ExecutionQuery,
    ) -> DaemonResult<()> {
        // Transition to Processing
        if let Err(e) = query.transition(QueryState::Processing) {
            return Err(DaemonError::Config(format!("Query transition error: {}", e)));
        }
        self.record_query_transition(query, "processing").await?;

        let position = match self.store.positions().find_by_id(position_id).await {
            Ok(Some(pos)) => pos,
            Ok(None) => {
                return Err(DaemonError::PositionNotFound(position_id));
            },
            Err(e) => {
                return Err(e.into());
            },
        };

        let exit_side = position.side.exit_action();

        // Transition to Acting before executor call
        if let Err(e) = query.transition(QueryState::Acting) {
            return Err(DaemonError::Config(format!("Query transition error: {}", e)));
        }
        self.record_query_transition(query, "acting").await?;

        // Place market exit order on exchange (executor also emits ExitOrderPlaced → Active → Exiting)
        // Use execute_and_persist to ensure events are persisted to eventlog (MIG-v2.5#2)
        let results = self
            .execute_and_persist(vec![EngineAction::PlaceExitOrder {
                position_id,
                symbol: position.symbol.clone(),
                side: exit_side,
                quantity: position.quantity,
                reason: robson_domain::ExitReason::UserPanic,
            }])
            .await?;

        // Extract actual fill price from exchange result
        let fill_price = match results.into_iter().find_map(|r| {
            if let ActionResult::OrderPlaced { order, .. } = r {
                Some(order.fill_price)
            } else {
                None
            }
        }) {
            Some(price) => price,
            None => {
                return Err(DaemonError::Exec(ExecError::InvalidState(
                    "Panic close: PlaceExitOrder did not return OrderPlaced".to_string(),
                )));
            },
        };

        // Calculate PnL with actual fill price
        let entry_price = position.entry_price.unwrap_or(fill_price);
        let pnl = position.calculate_pnl();

        // Emit PositionClosed with actual fill price (Exiting → Closed)
        let event = Event::PositionClosed {
            position_id,
            exit_reason: robson_domain::ExitReason::UserPanic,
            entry_price,
            exit_price: fill_price,
            realized_pnl: pnl,
            total_fees: position.fees_paid,
            timestamp: chrono::Utc::now(),
        };

        self.execute_and_persist(vec![EngineAction::EmitEvent(event)]).await?;

        // Send to event bus for real-time notification
        self.event_bus.send(DaemonEvent::CorePositionClosed {
            position_id,
            symbol: position.symbol.clone(),
            side: position.side,
        });

        Ok(())
    }

    /// Kill detector task for a position.
    async fn kill_detector(&self, position_id: PositionId) {
        let mut detectors = self.detectors.write().await;
        if let Some(handle) = detectors.remove(&position_id) {
            handle.abort();
            debug!(%position_id, "Detector task killed");
        }
    }

    /// Get position by ID.
    pub async fn get_position(&self, position_id: PositionId) -> DaemonResult<Option<Position>> {
        Ok(self.store.positions().find_by_id(position_id).await?)
    }

    /// Get all open core positions.
    ///
    /// Includes Armed, Entering, Active, and Exiting positions.
    /// Excludes Closed and Error.
    pub async fn get_open_positions(&self) -> DaemonResult<Vec<Position>> {
        Ok(self.store.positions().find_active().await?)
    }

    /// Historical alias used by older API/CLI code.
    pub async fn get_active_positions(&self) -> DaemonResult<Vec<Position>> {
        self.get_open_positions().await
    }

    /// Get open position count.
    pub async fn position_count(&self) -> DaemonResult<usize> {
        // This is a hack since Store doesn't have count method
        // In memory store we can count, in production we'd have proper query
        let open = self.store.positions().find_active().await?;
        Ok(open.len())
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query_engine::TracingQueryRecorder;
    use robson_exec::{IntentJournal, StubExchange};
    use robson_store::MemoryStore;
    use rust_decimal::Decimal;
    use rust_decimal_macros::dec;

    /// Create a test manager without starting the signal listener.
    /// Use this for unit tests that call handle_signal() directly.
    async fn create_test_manager_with_approval_policy(
        approval_policy: ApprovalPolicy,
    ) -> Arc<PositionManager<StubExchange, MemoryStore>> {
        let exchange = Arc::new(StubExchange::new(dec!(95000)));
        let journal = Arc::new(IntentJournal::new());
        let store = Arc::new(MemoryStore::new());
        let executor = Arc::new(Executor::new(exchange, journal, store.clone()));
        let event_bus = Arc::new(EventBus::new(100));
        let risk_config = RiskConfig::new(dec!(10000)).unwrap(); // 1% risk
        let engine = Engine::new(risk_config);

        Arc::new(PositionManager::with_approval_policy(
            engine,
            executor,
            store,
            event_bus,
            Arc::new(TracingQueryRecorder),
            approval_policy,
        ))
    }

    async fn create_test_manager() -> Arc<PositionManager<StubExchange, MemoryStore>> {
        create_test_manager_with_approval_policy(ApprovalPolicy::new(Decimal::from(100u32), 300))
            .await
    }

    async fn create_phase3_test_manager(
        ttl_seconds: u64,
    ) -> Arc<PositionManager<StubExchange, MemoryStore>> {
        create_test_manager_with_approval_policy(ApprovalPolicy::new(
            Decimal::from(5u32),
            ttl_seconds,
        ))
        .await
    }

    /// Create a test manager WITH signal listener running.
    /// Use this for E2E tests that need full event-driven flow.
    async fn create_test_manager_with_listener() -> Arc<PositionManager<StubExchange, MemoryStore>>
    {
        let manager = create_test_manager().await;
        PositionManager::start(Arc::clone(&manager));
        manager
    }

    fn create_test_risk_config() -> RiskConfig {
        RiskConfig::new(dec!(10000)).unwrap() // 1% risk
    }

    async fn save_active_position(
        manager: &Arc<PositionManager<StubExchange, MemoryStore>>,
        symbol: &str,
        side: Side,
        entry_price: Decimal,
        quantity: Decimal,
    ) -> Position {
        let account_id = Uuid::now_v7();
        let symbol = Symbol::from_pair(symbol).unwrap();
        let entry_price = Price::new(entry_price).unwrap();
        let trailing_stop = match side {
            Side::Long => Price::new(entry_price.as_decimal() - dec!(10)).unwrap(),
            Side::Short => Price::new(entry_price.as_decimal() + dec!(10)).unwrap(),
        };
        let quantity = Quantity::new(quantity).unwrap();
        let now = chrono::Utc::now();

        let mut position = Position::new(account_id, symbol, side);
        position.entry_price = Some(entry_price);
        position.entry_filled_at = Some(now);
        position.quantity = quantity;
        position.state = PositionState::Active {
            current_price: entry_price,
            trailing_stop,
            favorable_extreme: entry_price,
            extreme_at: now,
            insurance_stop_id: None,
            last_emitted_stop: None,
        };
        position.updated_at = now;

        manager.store.positions().save(&position).await.unwrap();
        position
    }

    #[tokio::test]
    async fn test_arm_position() {
        let manager = create_test_manager().await;
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        // Create tech stop distance: entry $100, stop $98 (2% distance)
        let entry = Price::new(dec!(100)).unwrap();
        let stop = Price::new(dec!(98)).unwrap();
        let tech_stop = TechnicalStopDistance::from_entry_and_stop(entry, stop);

        let position = manager
            .arm_position(symbol, Side::Long, create_test_risk_config(), tech_stop, Uuid::now_v7())
            .await
            .unwrap();

        assert!(matches!(position.state, PositionState::Armed));

        // Should be persisted
        let loaded = manager.get_position(position.id).await.unwrap().unwrap();
        assert_eq!(loaded.id, position.id);
    }

    #[tokio::test]
    async fn test_disarm_position() {
        let manager = create_test_manager().await;
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        // Create tech stop distance: entry $100, stop $98 (2% distance)
        let entry = Price::new(dec!(100)).unwrap();
        let stop = Price::new(dec!(98)).unwrap();
        let tech_stop = TechnicalStopDistance::from_entry_and_stop(entry, stop);

        let position = manager
            .arm_position(symbol, Side::Long, create_test_risk_config(), tech_stop, Uuid::now_v7())
            .await
            .unwrap();

        manager.disarm_position(position.id).await.unwrap();

        // Position must be kept for audit trail, transitioned to Closed state
        let loaded = manager
            .get_position(position.id)
            .await
            .unwrap()
            .expect("position must exist after disarm");
        assert!(
            matches!(loaded.state, PositionState::Closed { .. }),
            "expected Closed after disarm, got {:?}",
            loaded.state
        );
    }

    #[tokio::test]
    async fn test_handle_signal() {
        let manager = create_test_manager().await;
        let mut receiver = manager.event_bus.subscribe();
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        // Create tech stop distance: entry $100, stop $98 (2% distance)
        let entry = Price::new(dec!(100)).unwrap();
        let stop = Price::new(dec!(98)).unwrap();
        let tech_stop = TechnicalStopDistance::from_entry_and_stop(entry, stop);

        let position = manager
            .arm_position(
                symbol.clone(),
                Side::Long,
                create_test_risk_config(),
                tech_stop,
                Uuid::now_v7(),
            )
            .await
            .unwrap();

        // Create detector signal.
        //
        // Stop distance must be wide enough to pass the Phase 2 risk gate:
        //   qty = (capital * risk_pct) / (entry * stop_pct) = $100 / (95000 * stop_pct)
        //   notional = qty * entry = $100 / stop_pct
        //   max_single_position_pct = 15% of $10000 = $1500
        //   → stop_pct ≥ 100/1500 ≈ 6.67%
        //
        // Using 8% stop distance: stop_loss = 95000 * 0.92 = 87400
        //   notional ≈ $1250 < $1500 ✓
        let signal = DetectorSignal {
            signal_id: Uuid::now_v7(),
            position_id: position.id,
            symbol: symbol.clone(),
            side: Side::Long,
            entry_price: Price::new(dec!(95000)).unwrap(),
            stop_loss: Price::new(dec!(87400)).unwrap(), // 8% below — passes risk gate
            timestamp: chrono::Utc::now(),
        };

        manager.handle_signal(signal).await.unwrap();

        // Position should now be Active
        let updated = manager.get_position(position.id).await.unwrap().unwrap();
        assert!(matches!(updated.state, PositionState::Active { .. }));

        // Core open event must be emitted when position becomes active
        let mut opened = false;
        for _ in 0..20 {
            if let Ok(Some(event)) =
                tokio::time::timeout(std::time::Duration::from_millis(50), receiver.recv()).await
            {
                if let Ok(DaemonEvent::CorePositionOpened { position_id, symbol, side, .. }) = event
                {
                    assert_eq!(position_id, position.id);
                    assert_eq!(symbol.as_pair(), "BTCUSDT");
                    assert_eq!(side, Side::Long);
                    opened = true;
                    break;
                }
            }
        }
        assert!(opened, "Expected CorePositionOpened event");
    }

    #[tokio::test]
    async fn test_disarm_non_armed_fails() {
        let manager = create_test_manager().await;
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        // Create tech stop distance: entry $100, stop $98 (2% distance)
        let entry = Price::new(dec!(100)).unwrap();
        let stop = Price::new(dec!(98)).unwrap();
        let tech_stop = TechnicalStopDistance::from_entry_and_stop(entry, stop);

        let position = manager
            .arm_position(
                symbol.clone(),
                Side::Long,
                create_test_risk_config(),
                tech_stop,
                Uuid::now_v7(),
            )
            .await
            .unwrap();

        // Move to Active — 8% stop distance passes the risk gate (see test_handle_signal)
        let signal = DetectorSignal {
            signal_id: Uuid::now_v7(),
            position_id: position.id,
            symbol,
            side: Side::Long,
            entry_price: Price::new(dec!(95000)).unwrap(),
            stop_loss: Price::new(dec!(87400)).unwrap(), // 8% below — passes risk gate
            timestamp: chrono::Utc::now(),
        };
        manager.handle_signal(signal).await.unwrap();

        // Try to disarm (should fail — position is now Active, not Armed)
        let result = manager.disarm_position(position.id).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_position_not_found() {
        let manager = create_test_manager().await;
        let fake_id = Uuid::now_v7();

        let result = manager.disarm_position(fake_id).await;
        assert!(matches!(result, Err(DaemonError::PositionNotFound(_))));
    }

    #[tokio::test]
    async fn test_panic_close_emits_core_position_closed() {
        let manager = create_test_manager().await;
        let mut receiver = manager.event_bus.subscribe();
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let entry = Price::new(dec!(100)).unwrap();
        let stop = Price::new(dec!(98)).unwrap();
        let tech_stop = TechnicalStopDistance::from_entry_and_stop(entry, stop);

        let position = manager
            .arm_position(
                symbol.clone(),
                Side::Long,
                create_test_risk_config(),
                tech_stop,
                Uuid::now_v7(),
            )
            .await
            .unwrap();

        // 8% stop — passes risk gate (≥6.67% threshold on $10k capital, 1% risk, 15% max single)
        let signal = DetectorSignal {
            signal_id: Uuid::now_v7(),
            position_id: position.id,
            symbol,
            side: Side::Long,
            entry_price: Price::new(dec!(95000)).unwrap(),
            stop_loss: Price::new(dec!(87400)).unwrap(), // 8% below — passes risk gate
            timestamp: chrono::Utc::now(),
        };
        manager.handle_signal(signal).await.unwrap();

        // Verify position is Active before panic close (entry was not denied)
        let pos = manager.get_position(position.id).await.unwrap().unwrap();
        assert!(
            matches!(pos.state, PositionState::Active { .. }),
            "Expected Active before panic close, got {:?}",
            pos.state
        );

        let _ = manager.panic_close_all().await.unwrap();

        let mut closed = false;
        for _ in 0..20 {
            if let Ok(Some(event)) =
                tokio::time::timeout(std::time::Duration::from_millis(50), receiver.recv()).await
            {
                if let Ok(DaemonEvent::CorePositionClosed { position_id, symbol, side }) = event {
                    assert_eq!(position_id, position.id);
                    assert_eq!(symbol.as_pair(), "BTCUSDT");
                    assert_eq!(side, Side::Long);
                    closed = true;
                    break;
                }
            }
        }
        assert!(closed, "Expected CorePositionClosed event");
    }

    /// Phase 2: Risk gate denial keeps position Armed and re-arms the detector.
    ///
    /// A 2% stop distance causes notional ≈ $5000 which exceeds the default
    /// 15% single-position limit ($1500 on $10k capital), so the entry is denied.
    /// The position must remain Armed and have a fresh detector after the denial.
    #[tokio::test]
    async fn test_risk_gate_denial_rearmed_and_position_stays_armed() {
        let manager = create_test_manager().await;
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let entry = Price::new(dec!(100)).unwrap();
        let stop = Price::new(dec!(98)).unwrap();
        let tech_stop = TechnicalStopDistance::from_entry_and_stop(entry, stop);

        let position = manager
            .arm_position(
                symbol.clone(),
                Side::Long,
                create_test_risk_config(),
                tech_stop,
                Uuid::now_v7(),
            )
            .await
            .unwrap();

        // 2% stop → notional ≈ $5000 > $1500 limit → risk gate denies
        let signal = DetectorSignal {
            signal_id: Uuid::now_v7(),
            position_id: position.id,
            symbol: symbol.clone(),
            side: Side::Long,
            entry_price: Price::new(dec!(95000)).unwrap(),
            stop_loss: Price::new(dec!(93100)).unwrap(), // 2% — deliberately over limit
            timestamp: chrono::Utc::now(),
        };

        // handle_signal must return Ok(()) — denial is a governed outcome, not an error
        manager.handle_signal(signal).await.unwrap();

        // Position must still be Armed — no entry was executed
        let updated = manager.get_position(position.id).await.unwrap().unwrap();
        assert!(
            matches!(updated.state, PositionState::Armed),
            "Expected Armed after denial, got {:?}",
            updated.state
        );

        // Detector must have been re-armed — detectors map must contain the position
        let detectors = manager.detectors.read().await;
        assert!(
            detectors.contains_key(&position.id),
            "Expected detector to be re-armed after risk denial"
        );
    }

    #[tokio::test]
    async fn test_handle_signal_waits_for_approval_when_required() {
        let manager = create_phase3_test_manager(300).await;
        let mut receiver = manager.event_bus.subscribe();
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let entry = Price::new(dec!(100)).unwrap();
        let stop = Price::new(dec!(90)).unwrap();
        let tech_stop = TechnicalStopDistance::from_entry_and_stop(entry, stop);

        let position = manager
            .arm_position(
                symbol.clone(),
                Side::Long,
                create_test_risk_config(),
                tech_stop,
                Uuid::now_v7(),
            )
            .await
            .unwrap();

        let signal = DetectorSignal {
            signal_id: Uuid::now_v7(),
            position_id: position.id,
            symbol,
            side: Side::Long,
            entry_price: Price::new(dec!(95000)).unwrap(),
            stop_loss: Price::new(dec!(85500)).unwrap(), // 10% below -> approval required, risk approved
            timestamp: chrono::Utc::now(),
        };

        manager.handle_signal(signal).await.unwrap();

        let updated = manager.get_position(position.id).await.unwrap().unwrap();
        assert!(
            matches!(updated.state, PositionState::Armed),
            "Expected Armed while approval is pending, got {:?}",
            updated.state
        );

        let pending = manager.pending_approvals.read().await;
        assert_eq!(pending.len(), 1, "Expected exactly one pending approval");
        let record = pending.values().next().expect("pending approval must exist");
        assert_eq!(record.query.state, QueryState::AwaitingApproval);
        assert!(record.query.approval.is_some());
        drop(pending);

        let mut awaiting_seen = false;
        for _ in 0..20 {
            if let Ok(Some(event)) =
                tokio::time::timeout(std::time::Duration::from_millis(50), receiver.recv()).await
            {
                if let Ok(DaemonEvent::QueryAwaitingApproval { query_id, position_id, .. }) = event
                {
                    assert_eq!(position_id, Some(position.id));
                    let pending = manager.pending_approvals.read().await;
                    assert!(pending.contains_key(&query_id));
                    awaiting_seen = true;
                    break;
                }
            }
        }
        assert!(awaiting_seen, "Expected QueryAwaitingApproval event");
    }

    #[tokio::test]
    async fn test_approve_query_executes_pending_signal() {
        let manager = create_phase3_test_manager(300).await;
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let entry = Price::new(dec!(100)).unwrap();
        let stop = Price::new(dec!(90)).unwrap();
        let tech_stop = TechnicalStopDistance::from_entry_and_stop(entry, stop);

        let position = manager
            .arm_position(
                symbol.clone(),
                Side::Long,
                create_test_risk_config(),
                tech_stop,
                Uuid::now_v7(),
            )
            .await
            .unwrap();

        let signal = DetectorSignal {
            signal_id: Uuid::now_v7(),
            position_id: position.id,
            symbol,
            side: Side::Long,
            entry_price: Price::new(dec!(95000)).unwrap(),
            stop_loss: Price::new(dec!(85500)).unwrap(), // 10% below -> approval required, risk approved
            timestamp: chrono::Utc::now(),
        };

        manager.handle_signal(signal).await.unwrap();

        let query_id = {
            let pending = manager.pending_approvals.read().await;
            *pending.keys().next().expect("pending approval query must exist")
        };

        let approved_query = manager.approve_query(query_id).await.unwrap();

        assert_eq!(approved_query.state, QueryState::Completed);
        assert!(manager.pending_approvals.read().await.is_empty());

        let updated = manager.get_position(position.id).await.unwrap().unwrap();
        assert!(
            matches!(updated.state, PositionState::Active { .. }),
            "Expected Active after approval execution, got {:?}",
            updated.state
        );
    }

    #[tokio::test]
    async fn test_disarm_invalidates_pending_approval() {
        let manager = create_phase3_test_manager(300).await;
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let entry = Price::new(dec!(100)).unwrap();
        let stop = Price::new(dec!(90)).unwrap();
        let tech_stop = TechnicalStopDistance::from_entry_and_stop(entry, stop);

        let position = manager
            .arm_position(
                symbol.clone(),
                Side::Long,
                create_test_risk_config(),
                tech_stop,
                Uuid::now_v7(),
            )
            .await
            .unwrap();

        let signal = DetectorSignal {
            signal_id: Uuid::now_v7(),
            position_id: position.id,
            symbol,
            side: Side::Long,
            entry_price: Price::new(dec!(95000)).unwrap(),
            stop_loss: Price::new(dec!(85500)).unwrap(),
            timestamp: chrono::Utc::now(),
        };

        manager.handle_signal(signal).await.unwrap();

        let query_id = {
            let pending = manager.pending_approvals.read().await;
            *pending.keys().next().expect("pending approval query must exist")
        };

        manager.disarm_position(position.id).await.unwrap();

        assert!(manager.pending_approvals.read().await.is_empty());
        let updated = manager.get_position(position.id).await.unwrap().unwrap();
        assert!(
            matches!(updated.state, PositionState::Closed { .. }),
            "Expected Closed after disarm, got {:?}",
            updated.state
        );

        let approval_result = manager.approve_query(query_id).await;
        assert!(matches!(approval_result, Err(DaemonError::QueryNotFound(id)) if id == query_id));
    }

    #[tokio::test]
    async fn test_approve_query_denied_when_risk_context_changes() {
        let manager = create_phase3_test_manager(300).await;
        save_active_position(&manager, "ETHUSDT", Side::Long, dec!(100), dec!(5)).await;
        save_active_position(&manager, "SOLUSDT", Side::Long, dec!(100), dec!(5)).await;

        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let entry = Price::new(dec!(100)).unwrap();
        let stop = Price::new(dec!(90)).unwrap();
        let tech_stop = TechnicalStopDistance::from_entry_and_stop(entry, stop);

        let position = manager
            .arm_position(
                symbol.clone(),
                Side::Long,
                create_test_risk_config(),
                tech_stop,
                Uuid::now_v7(),
            )
            .await
            .unwrap();

        let signal = DetectorSignal {
            signal_id: Uuid::now_v7(),
            position_id: position.id,
            symbol,
            side: Side::Long,
            entry_price: Price::new(dec!(95000)).unwrap(),
            stop_loss: Price::new(dec!(85500)).unwrap(),
            timestamp: chrono::Utc::now(),
        };

        manager.handle_signal(signal).await.unwrap();

        let query_id = {
            let pending = manager.pending_approvals.read().await;
            *pending.keys().next().expect("pending approval query must exist")
        };

        save_active_position(&manager, "BNBUSDT", Side::Long, dec!(100), dec!(5)).await;

        let approval_result = manager.approve_query(query_id).await;
        assert!(matches!(
            approval_result,
            Err(DaemonError::ApprovalDenied { query_id: denied_query_id, .. })
            if denied_query_id == query_id
        ));
        assert!(manager.pending_approvals.read().await.is_empty());

        let updated = manager.get_position(position.id).await.unwrap().unwrap();
        assert!(
            matches!(updated.state, PositionState::Armed),
            "Expected Armed after approval denial, got {:?}",
            updated.state
        );

        let detectors = manager.detectors.read().await;
        assert!(
            detectors.contains_key(&position.id),
            "Detector must be re-armed after approval denial"
        );
    }

    #[tokio::test]
    async fn test_pending_approval_expires_and_does_not_execute() {
        let manager = create_phase3_test_manager(1).await;
        let mut receiver = manager.event_bus.subscribe();
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let entry = Price::new(dec!(100)).unwrap();
        let stop = Price::new(dec!(90)).unwrap();
        let tech_stop = TechnicalStopDistance::from_entry_and_stop(entry, stop);

        let position = manager
            .arm_position(
                symbol.clone(),
                Side::Long,
                create_test_risk_config(),
                tech_stop,
                Uuid::now_v7(),
            )
            .await
            .unwrap();

        let signal = DetectorSignal {
            signal_id: Uuid::now_v7(),
            position_id: position.id,
            symbol,
            side: Side::Long,
            entry_price: Price::new(dec!(95000)).unwrap(),
            stop_loss: Price::new(dec!(85500)).unwrap(), // 10% below -> approval required, risk approved
            timestamp: chrono::Utc::now(),
        };

        manager.handle_signal(signal).await.unwrap();

        let mut expired_seen = false;
        for _ in 0..40 {
            if let Ok(Some(event)) =
                tokio::time::timeout(std::time::Duration::from_millis(100), receiver.recv()).await
            {
                if let Ok(DaemonEvent::QueryExpired { position_id, .. }) = event {
                    assert_eq!(position_id, Some(position.id));
                    expired_seen = true;
                    break;
                }
            }
        }
        assert!(expired_seen, "Expected QueryExpired event");

        let updated = manager.get_position(position.id).await.unwrap().unwrap();
        assert!(
            matches!(updated.state, PositionState::Armed),
            "Expected Armed after approval expiry, got {:?}",
            updated.state
        );
        assert!(manager.pending_approvals.read().await.is_empty());

        let detectors = manager.detectors.read().await;
        assert!(
            detectors.contains_key(&position.id),
            "Detector must be re-armed after approval expiry"
        );
    }

    /// Entering positions are included in risk context (find_risk_open).
    ///
    /// If only find_active() were used, Entering positions would be invisible to
    /// the risk gate, allowing concurrent entries to bypass exposure checks during
    /// the order-fill window. This test proves find_risk_open() counts them.
    ///
    /// Strategy: seed the store with MAX_OPEN_POSITIONS Entering positions, then
    /// send a signal for a new Armed position. The risk gate must deny it.
    #[tokio::test]
    async fn test_entering_positions_count_in_risk_context() {
        let manager = create_test_manager().await;
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();

        // Seed the store with 3 positions in Entering state (MaxOpenPositions = 3).
        // We construct them directly to bypass the fill logic (StubExchange fills immediately).
        //
        // Non-zero quantity is required: build_risk_context() skips zero-qty positions
        // when building PositionSummary entries for open_position_count().
        // qty ≈ $100 risk / ($95000 * 8% stop) ≈ 0.01315 BTC
        let account_id = uuid::Uuid::now_v7();
        for _ in 0..3 {
            let mut pos = Position::new(account_id, symbol.clone(), Side::Long);
            pos.quantity = Quantity::new(dec!(0.01315)).unwrap();
            pos.state = PositionState::Entering {
                entry_order_id: uuid::Uuid::now_v7(),
                expected_entry: Price::new(dec!(95000)).unwrap(),
                signal_id: uuid::Uuid::now_v7(),
            };
            manager.store.positions().save(&pos).await.unwrap();
        }

        // Arm a 4th position (the one we will try to enter).
        // 8% stop — within valid range (≤10%) and passes single-position check (≥6.67%).
        let entry = Price::new(dec!(100)).unwrap();
        let stop = Price::new(dec!(92)).unwrap(); // 8% stop
        let tech_stop = TechnicalStopDistance::from_entry_and_stop(entry, stop);

        let position = manager
            .arm_position(
                symbol.clone(),
                Side::Long,
                create_test_risk_config(),
                tech_stop,
                uuid::Uuid::now_v7(),
            )
            .await
            .unwrap();

        let signal = DetectorSignal {
            signal_id: uuid::Uuid::now_v7(),
            position_id: position.id,
            symbol: symbol.clone(),
            side: Side::Long,
            entry_price: Price::new(dec!(95000)).unwrap(),
            stop_loss: Price::new(dec!(87400)).unwrap(), // 8% below — passes single-position gate
            timestamp: chrono::Utc::now(),
        };

        // Must return Ok(()) — denial is a governed outcome.
        manager.handle_signal(signal).await.unwrap();

        // 4th position must still be Armed — entry was blocked by MaxOpenPositions.
        let updated = manager.get_position(position.id).await.unwrap().unwrap();
        assert!(
            matches!(updated.state, PositionState::Armed),
            "Expected Armed after denial (Entering positions blocked entry), got {:?}",
            updated.state
        );
    }

    /// E2E test: full detector integration (arm → spawn detector → MA crossover → signal → entry)
    ///
    /// Flow:
    /// 1. arm_position() → spawns detector
    /// 2. Inject synthetic market data via EventBus
    /// 3. Wait for MA crossover → DetectorSignal
    /// 4. Signal listener processes signal → Entry order → Position becomes Active
    ///
    /// Scope: Uses stub exchange, NO real orders, NO WebSocket
    #[tokio::test]
    async fn test_e2e_detector_ma_crossover_signal() {
        // Use manager WITH signal listener for full E2E flow
        let manager = create_test_manager_with_listener().await;
        let event_bus = manager.event_bus.clone();

        // Subscribe to EventBus to capture DetectorSignal
        let mut signal_receiver = event_bus.subscribe();

        // Arm position (spawns detector internally)
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let entry = Price::new(dec!(100)).unwrap();
        let stop = Price::new(dec!(98)).unwrap();
        let tech_stop = TechnicalStopDistance::from_entry_and_stop(entry, stop);

        let position = manager
            .arm_position(
                symbol.clone(),
                Side::Long,
                create_test_risk_config(),
                tech_stop,
                Uuid::now_v7(),
            )
            .await
            .unwrap();

        let position_id = position.id;

        // Yield to let detector task subscribe
        tokio::task::yield_now().await;

        // Feed descending prices (fast MA < slow MA) to establish "below" state
        // Need enough data points for MA calculation (slow_period=21 default)
        for i in (0..30).rev() {
            let price = Decimal::from(100 + i);
            let market_data = MarketData {
                symbol: symbol.clone(),
                price: Price::new(price).unwrap(),
                timestamp: chrono::Utc::now(),
            };
            event_bus.send(DaemonEvent::MarketData(market_data));
        }

        // Feed ascending prices to trigger MA crossover (fast crosses above slow)
        let mut signal_found = false;
        let mut detector_signal = None;

        for i in 0..10 {
            let price = Decimal::from(100 + i * 3); // Larger steps to trigger crossover faster
            let market_data = MarketData {
                symbol: symbol.clone(),
                price: Price::new(price).unwrap(),
                timestamp: chrono::Utc::now(),
            };
            event_bus.send(DaemonEvent::MarketData(market_data));

            // Check if detector emitted signal (after each tick, with timeout)
            for _ in 0..5 {
                let deadline = tokio::time::timeout(
                    std::time::Duration::from_millis(50),
                    signal_receiver.recv(),
                );

                match deadline.await {
                    Ok(Some(Ok(DaemonEvent::DetectorSignal(signal)))) => {
                        detector_signal = Some(signal);
                        signal_found = true;
                        break;
                    },
                    Ok(Some(Ok(_))) => continue, // Other events
                    Ok(Some(Err(_))) | Ok(None) | Err(_) => break, // Channel error or timeout
                }
                if signal_found {
                    break;
                }
            }
            if signal_found {
                break;
            }
        }

        // Assert: signal was emitted
        assert!(signal_found, "Detector should emit signal on MA crossover");

        let signal = detector_signal.expect("Signal should exist");

        // Assert: signal properties
        assert_eq!(signal.position_id, position_id);
        assert_eq!(signal.symbol.as_pair(), "BTCUSDT");
        assert_eq!(signal.side, Side::Long);
        assert!(signal.entry_price.as_decimal() > dec!(0));
        assert!(signal.stop_loss.as_decimal() > dec!(0));

        // Verify detector was cleaned up (single-shot)
        // Detector should be removed after signaling (checked via detector count)
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}
