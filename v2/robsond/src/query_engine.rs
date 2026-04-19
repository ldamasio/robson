//! QueryEngine: the governed execution core.
//!
//! Phase 1: Lifecycle tracker.
//! Records query state transitions via QueryRecorder.
//!
//! Phase 2 (current): Blocking governance.
//! Holds the RiskGate. All entry execution paths must call `check_risk()` to
//! obtain a `GovernedAction` before dispatching to the Executor. Denial is a
//! governed terminal outcome (`QueryState::Denied`), not an operational error.
//!
//! Phase 3: Approval gates.
//! After risk approval, QueryEngine decides whether a human approval gate is
//! required before execution can proceed.
//!
//! GovernedAction is `pub(crate)` — only constructible inside this module via
//! `check_risk()`. This ensures no entry path in robsond can reach the Executor
//! without passing through the risk gate.
//!
//! Architectural note (Phase 2 decision):
//! `GovernedAction` lives in robsond (not robson-exec). The Executor continues
//! accepting `Vec<EngineAction>` unchanged. Type-level enforcement across the
//! crate boundary is deferred as a follow-up architectural concern. The
//! governance guarantee in Phase 2 is: "risk evaluated inside runtime before
//! any Executor call".
//!
//! Ownership: lives INSIDE robsond crate. Not a separate crate.

use async_trait::async_trait;
use robson_engine::{EngineAction, ProposedTrade, RiskContext, RiskGate, RiskVerdict};
#[cfg(feature = "postgres")]
use robson_eventlog::{
    append_event, ActorType, Event, EventLogError, QUERY_STATE_CHANGED_EVENT_TYPE,
};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
#[cfg(feature = "postgres")]
use sqlx::PgPool;
use uuid::Uuid;

use crate::query::{ApprovalRequirement, ExecutionQuery, QueryError, QueryState};

// =============================================================================
// QueryRecorder - Audit and Observability
// =============================================================================

/// Canonical durable payload for query lifecycle persistence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryStateChangedEvent {
    pub query_id: Uuid,
    pub position_id: Option<Uuid>,
    pub state: String,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub finished_at: Option<chrono::DateTime<chrono::Utc>>,
    pub transition_cause: String,
    pub snapshot: ExecutionQuery,
}

impl QueryStateChangedEvent {
    pub fn from_query(query: &ExecutionQuery, transition_cause: &str) -> Self {
        Self {
            query_id: query.id,
            position_id: query.position_id,
            state: query.state.label().to_string(),
            started_at: query.started_at,
            finished_at: query.finished_at,
            transition_cause: transition_cause.to_string(),
            snapshot: query.clone(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum QueryRecorderError {
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[cfg(feature = "postgres")]
    #[error("Event log error: {0}")]
    EventLog(#[from] EventLogError),
}

fn trace_query_transition(query: &ExecutionQuery, transition_cause: &str) {
    let duration_ms = query.duration().map(|d| d.num_milliseconds());

    match &query.state {
        QueryState::Failed { reason, phase } => {
            tracing::error!(
                query_id = %query.id,
                kind = ?query.kind,
                state = %query.state.label(),
                actor = ?query.actor,
                position_id = ?query.position_id,
                active_positions_count = ?query.context_summary.as_ref().map(|c| c.active_positions_count),
                duration_ms = ?duration_ms,
                transition_cause = %transition_cause,
                reason = %reason,
                phase = %phase,
                "query state transition"
            );
        },
        QueryState::Denied { reason, check } => {
            tracing::warn!(
                query_id = %query.id,
                kind = ?query.kind,
                state = %query.state.label(),
                actor = ?query.actor,
                position_id = ?query.position_id,
                active_positions_count = ?query.context_summary.as_ref().map(|c| c.active_positions_count),
                duration_ms = ?duration_ms,
                transition_cause = %transition_cause,
                reason = %reason,
                check = %check,
                "query state transition"
            );
        },
        _ => {
            tracing::info!(
                query_id = %query.id,
                kind = ?query.kind,
                state = %query.state.label(),
                actor = ?query.actor,
                position_id = ?query.position_id,
                active_positions_count = ?query.context_summary.as_ref().map(|c| c.active_positions_count),
                duration_ms = ?duration_ms,
                transition_cause = %transition_cause,
                "query state transition"
            );
        },
    }
}

#[cfg(feature = "postgres")]
pub(crate) async fn append_query_state_changed_event(
    pool: &PgPool,
    tenant_id: Uuid,
    stream_key: &str,
    query: &ExecutionQuery,
    transition_cause: &str,
) -> Result<(), QueryRecorderError> {
    let payload = QueryStateChangedEvent::from_query(query, transition_cause);
    let event = Event::new(
        tenant_id,
        stream_key,
        QUERY_STATE_CHANGED_EVENT_TYPE,
        serde_json::to_value(payload)?,
    )
    .with_actor(ActorType::Daemon, Some("robsond".to_string()));

    match append_event(pool, stream_key, None, event).await {
        Ok(_) | Err(EventLogError::IdempotentDuplicate(_)) => Ok(()),
        Err(err) => Err(QueryRecorderError::EventLog(err)),
    }
}

/// Records query lifecycle snapshots for observability and audit.
#[async_trait]
pub trait QueryRecorder: Send + Sync {
    /// Persist or emit the latest query snapshot after a lifecycle transition.
    async fn record_transition(
        &self,
        query: &ExecutionQuery,
        transition_cause: &str,
    ) -> Result<(), QueryRecorderError>;
}

// Blanket implementation for Arc<T>
#[async_trait]
impl<T: QueryRecorder + ?Sized> QueryRecorder for std::sync::Arc<T> {
    async fn record_transition(
        &self,
        query: &ExecutionQuery,
        transition_cause: &str,
    ) -> Result<(), QueryRecorderError> {
        (**self).record_transition(query, transition_cause).await
    }
}

// =============================================================================
// TracingQueryRecorder - Default implementation
// =============================================================================

/// Default implementation: structured tracing logs.
/// Zero persistence overhead. Full observability via tracing subscribers.
pub struct TracingQueryRecorder;

#[async_trait]
impl QueryRecorder for TracingQueryRecorder {
    async fn record_transition(
        &self,
        query: &ExecutionQuery,
        transition_cause: &str,
    ) -> Result<(), QueryRecorderError> {
        trace_query_transition(query, transition_cause);
        Ok(())
    }
}

#[cfg(feature = "postgres")]
pub struct EventLogQueryRecorder {
    pool: PgPool,
    tenant_id: Uuid,
    stream_key: String,
}

#[cfg(feature = "postgres")]
impl EventLogQueryRecorder {
    pub fn new(pool: PgPool, tenant_id: Uuid, stream_key: impl Into<String>) -> Self {
        Self {
            pool,
            tenant_id,
            stream_key: stream_key.into(),
        }
    }
}

#[cfg(feature = "postgres")]
#[async_trait]
impl QueryRecorder for EventLogQueryRecorder {
    async fn record_transition(
        &self,
        query: &ExecutionQuery,
        transition_cause: &str,
    ) -> Result<(), QueryRecorderError> {
        trace_query_transition(query, transition_cause);
        append_query_state_changed_event(
            &self.pool,
            self.tenant_id,
            &self.stream_key,
            query,
            transition_cause,
        )
        .await
    }
}

// =============================================================================
// GovernedAction — Phase 2 proof token
// =============================================================================

/// Proof that a set of engine actions has passed the risk gate.
///
/// Only constructible via `QueryEngine::check_risk()`. This ensures no entry
/// execution path in robsond can reach the Executor without risk evaluation.
///
/// `pub(crate)`: visible within robsond only. The Executor still accepts
/// `Vec<EngineAction>` (unchanged); the governance enforcement is runtime-level
/// within the crate, not type-level across the crate boundary.
#[derive(Debug)]
pub(crate) struct GovernedAction {
    cycle_id: Uuid,
    actions: Vec<EngineAction>,
    // Private field prevents external construction even within the crate if
    // someone tries to use struct literal syntax.
    _proof: (),
}

impl GovernedAction {
    /// Private: only QueryEngine::check_risk() may construct this.
    fn new(cycle_id: Uuid, actions: Vec<EngineAction>) -> Self {
        Self { cycle_id, actions, _proof: () }
    }

    /// Consume the token and return the approved actions for Executor dispatch.
    pub(crate) fn into_actions(self) -> Vec<EngineAction> {
        self.actions
            .into_iter()
            .map(|action| action.with_cycle_id(self.cycle_id))
            .collect()
    }

    fn actions(&self) -> &[EngineAction] {
        &self.actions
    }

    #[cfg(test)]
    fn cycle_id(&self) -> Uuid {
        self.cycle_id
    }
}

// =============================================================================
// ApprovalPolicy / ApprovalCheckResult
// =============================================================================

/// Minimal Phase 3 approval policy.
///
/// Scope is intentionally narrow:
/// - Only entry orders are considered for approval
/// - Approval is required when the proposed entry notional exceeds a fixed
///   percentage of capital
/// - TTL is fixed and explicit
#[derive(Debug, Clone)]
pub(crate) struct ApprovalPolicy {
    entry_notional_threshold_pct: Decimal,
    ttl_seconds: u64,
}

impl ApprovalPolicy {
    pub(crate) fn new(entry_notional_threshold_pct: Decimal, ttl_seconds: u64) -> Self {
        Self {
            entry_notional_threshold_pct,
            ttl_seconds,
        }
    }

    fn requirement_for(
        &self,
        actions: &[EngineAction],
        proposed: &ProposedTrade,
        context: &RiskContext,
    ) -> ApprovalRequirement {
        let has_entry_order = actions
            .iter()
            .any(|action| matches!(action, EngineAction::PlaceEntryOrder { .. }));

        if !has_entry_order || context.capital <= Decimal::ZERO {
            return ApprovalRequirement::NotRequired;
        }

        let threshold = context.capital * self.entry_notional_threshold_pct / Decimal::from(100u32);

        if proposed.notional_value > threshold {
            ApprovalRequirement::Required {
                reason: format!(
                    "Entry notional {} exceeds approval threshold {}",
                    proposed.notional_value, threshold
                ),
                ttl_seconds: self.ttl_seconds,
            }
        } else {
            ApprovalRequirement::NotRequired
        }
    }
}

impl Default for ApprovalPolicy {
    fn default() -> Self {
        Self::new(Decimal::from(5u32), 300)
    }
}

/// Result of the approval gate after risk approval.
#[derive(Debug)]
pub(crate) enum ApprovalCheckResult {
    /// No approval required. Execution may proceed immediately.
    Ready(GovernedAction),
    /// Approval required. Runtime must hold the query until operator approval.
    AwaitingApproval(GovernedAction),
}

// =============================================================================
// CheckRiskError — typed error for check_risk() callers
// =============================================================================

/// Error returned by `check_risk()`.
///
/// Two distinct cases that callers MUST handle differently:
///
/// - `Denied`: Risk gate rejected the trade. The query is in
///   `QueryState::Denied`. No exchange side effects occurred. Caller should
///   treat this as a governed outcome and return `Ok(())`.
///
/// - `InvalidState`: The query lifecycle state machine is in an unexpected
///   state and cannot enter `RiskChecked`. This is an operational error (e.g. a
///   bug in the caller or concurrent mutation). Caller should propagate as an
///   error — NOT treat it as a governance denial.
#[derive(Debug)]
pub(crate) enum CheckRiskError {
    /// Risk gate rejected the trade (governed outcome — no side effects
    /// occurred).
    Denied,
    /// Query state machine is inconsistent — cannot transition to RiskChecked.
    /// This is an operational error, not a governance decision.
    InvalidState(QueryError),
    /// Query snapshot could not be persisted after a lifecycle transition.
    Audit(QueryRecorderError),
}

#[derive(Debug, thiserror::Error)]
pub enum QueryTransitionError {
    #[error("Invalid query transition: {0}")]
    InvalidState(#[from] QueryError),

    #[error("Query audit error: {0}")]
    Audit(#[from] QueryRecorderError),
}

// =============================================================================
// QueryEngine
// =============================================================================

/// Phase 2: Lifecycle tracker + Risk governance gate.
///
/// Holds the RiskGate. Entry execution paths call `check_risk()` to obtain
/// a `GovernedAction`. All other lifecycle recording methods remain unchanged.
///
/// Ownership: lives INSIDE robsond crate. Not a separate crate.
pub struct QueryEngine<R: QueryRecorder> {
    recorder: R,
    risk_gate: RiskGate,
    approval_policy: ApprovalPolicy,
}

impl<R: QueryRecorder> QueryEngine<R> {
    /// Create a new QueryEngine with the given recorder and risk gate.
    pub fn new(recorder: R, risk_gate: RiskGate) -> Self {
        Self::with_approval_policy(recorder, risk_gate, ApprovalPolicy::default())
    }

    pub(crate) fn with_approval_policy(
        recorder: R,
        risk_gate: RiskGate,
        approval_policy: ApprovalPolicy,
    ) -> Self {
        Self { recorder, risk_gate, approval_policy }
    }

    /// Record that a query has been accepted.
    pub async fn on_accepted(&self, query: &ExecutionQuery) -> Result<(), QueryRecorderError> {
        self.recorder.record_transition(query, "accepted").await
    }

    /// Record a state transition.
    pub async fn on_state_change(
        &self,
        query: &ExecutionQuery,
        transition_cause: &str,
    ) -> Result<(), QueryRecorderError> {
        self.recorder.record_transition(query, transition_cause).await
    }

    /// Phase 2: Evaluate risk and return a governed proof token.
    ///
    /// This is the mandatory governance gate for all entry execution paths.
    ///
    /// Returns:
    /// - `Ok(GovernedAction)`: risk approved — pass actions to Executor via
    ///   `governed.into_actions()`.
    /// - `Err(CheckRiskError::Denied)`: risk gate rejected the trade. Query is
    ///   in `QueryState::Denied`. Caller should treat as governed outcome
    ///   (`Ok(())`).
    /// - `Err(CheckRiskError::InvalidState)`: query lifecycle state machine is
    ///   inconsistent (cannot enter RiskChecked). This is an operational error.
    ///   Caller MUST propagate as `Err`, not treat as a denial.
    ///
    /// `actions` are consumed here — there is no path to the Executor without
    /// passing through this check.
    pub(crate) async fn check_risk(
        &self,
        query: &mut ExecutionQuery,
        proposed: &ProposedTrade,
        context: &RiskContext,
        actions: Vec<EngineAction>,
    ) -> Result<GovernedAction, CheckRiskError> {
        // Transition to RiskChecked FIRST — before evaluating the verdict.
        // If this fails, the query is in an unexpected state (operational bug).
        // Return InvalidState so the caller can propagate it as an error,
        // not silently absorb it as a governance denial.
        if let Err(e) = query.transition(QueryState::RiskChecked) {
            tracing::error!(
                query_id = %query.id,
                current_state = ?query.state,
                error = %e,
                "Cannot transition to RiskChecked — query lifecycle inconsistency"
            );
            return Err(CheckRiskError::InvalidState(e));
        }
        if let Err(err) = self.recorder.record_transition(query, "risk_checked").await {
            return Err(CheckRiskError::Audit(err));
        }

        let verdict = self.risk_gate.evaluate(proposed, context);

        match verdict {
            RiskVerdict::Approved => {
                tracing::info!(
                    query_id = %query.id,
                    symbol = %proposed.symbol,
                    side = %proposed.side,
                    notional = %proposed.notional_value,
                    "risk check approved"
                );
                Ok(GovernedAction::new(query.id, actions))
            },
            RiskVerdict::Rejected { check, reason } => {
                tracing::warn!(
                    query_id = %query.id,
                    symbol = %proposed.symbol,
                    side = %proposed.side,
                    risk_check = %check.name(),
                    reason = %reason,
                    "risk check denied — governed rejection, no side effects"
                );
                query.deny(reason, check.name().to_string());
                if let Err(err) = self.recorder.record_transition(query, "risk_denied").await {
                    return Err(CheckRiskError::Audit(err));
                }
                Err(CheckRiskError::Denied)
            },
        }
    }

    /// Phase 3: decide whether an already risk-approved query requires
    /// operator approval before execution.
    pub(crate) async fn check_approval(
        &self,
        query: &mut ExecutionQuery,
        proposed: &ProposedTrade,
        context: &RiskContext,
        governed: GovernedAction,
    ) -> Result<ApprovalCheckResult, QueryTransitionError> {
        match self.approval_policy.requirement_for(governed.actions(), proposed, context) {
            ApprovalRequirement::NotRequired => Ok(ApprovalCheckResult::Ready(governed)),
            ApprovalRequirement::Required { reason, ttl_seconds } => {
                query.await_approval(reason, ttl_seconds)?;
                self.recorder.record_transition(query, "awaiting_approval").await?;
                Ok(ApprovalCheckResult::AwaitingApproval(governed))
            },
        }
    }

    /// Revalidate a pending approval against the current risk context.
    ///
    /// Approval is not a risk override. If the portfolio changed while the
    /// query was waiting for operator confirmation, execution must be
    /// denied.
    pub(crate) async fn revalidate_risk(
        &self,
        query: &mut ExecutionQuery,
        proposed: &ProposedTrade,
        context: &RiskContext,
    ) -> Result<(), CheckRiskError> {
        match self.risk_gate.evaluate(proposed, context) {
            RiskVerdict::Approved => Ok(()),
            RiskVerdict::Rejected { check, reason } => {
                tracing::warn!(
                    query_id = %query.id,
                    symbol = %proposed.symbol,
                    side = %proposed.side,
                    risk_check = %check.name(),
                    reason = %reason,
                    "approval revalidation denied — governed rejection, no side effects"
                );
                query.deny(reason, check.name().to_string());
                if let Err(err) = self.recorder.record_transition(query, "risk_denied").await {
                    return Err(CheckRiskError::Audit(err));
                }
                Err(CheckRiskError::Denied)
            },
        }
    }

    /// Record an operator authorization transition.
    pub(crate) async fn authorize(
        &self,
        query: &mut ExecutionQuery,
    ) -> Result<(), QueryTransitionError> {
        query.authorize()?;
        self.recorder.record_transition(query, "authorized").await?;
        Ok(())
    }

    /// Record approval expiration.
    pub(crate) async fn expire(
        &self,
        query: &mut ExecutionQuery,
        transition_cause: &str,
    ) -> Result<(), QueryTransitionError> {
        query.expire_approval()?;
        self.recorder.record_transition(query, transition_cause).await?;
        Ok(())
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    use robson_domain::{Price, Side, Symbol};
    use robson_engine::{PositionSummary, RiskContext, RiskGate};
    use rust_decimal::Decimal;
    use rust_decimal_macros::dec;

    use super::*;
    use crate::query::{ActorKind, QueryKind, QueryOutcome, QueryState};

    /// Mock recorder that counts calls
    struct MockRecorder {
        transition_count: AtomicUsize,
    }

    impl MockRecorder {
        fn new() -> Self {
            Self { transition_count: AtomicUsize::new(0) }
        }

        fn transition_count(&self) -> usize {
            self.transition_count.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl QueryRecorder for MockRecorder {
        async fn record_transition(
            &self,
            _query: &ExecutionQuery,
            _transition_cause: &str,
        ) -> Result<(), QueryRecorderError> {
            self.transition_count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    fn create_test_query() -> ExecutionQuery {
        ExecutionQuery::new(
            QueryKind::ProcessSignal {
                signal_id: uuid::Uuid::now_v7(),
                symbol: Symbol::from_pair("BTCUSDT").unwrap(),
                side: Side::Long,
                entry_price: Price::new(dec!(95000)).unwrap(),
                stop_loss: Price::new(dec!(93500)).unwrap(),
            },
            ActorKind::Detector,
        )
    }

    #[tokio::test]
    async fn test_query_engine_delegates_to_recorder() {
        let recorder = Arc::new(MockRecorder::new());
        let engine = QueryEngine::new(Arc::clone(&recorder), RiskGate::new());

        let mut query = create_test_query();

        engine.on_accepted(&query).await.unwrap();
        assert_eq!(recorder.transition_count(), 1);

        query.transition(QueryState::Processing).unwrap();
        engine.on_state_change(&query, "processing").await.unwrap();
        assert_eq!(recorder.transition_count(), 2);

        query.fail("test error".to_string(), "processing".to_string());
        engine.on_state_change(&query, "failed").await.unwrap();
        assert_eq!(recorder.transition_count(), 3);
    }

    #[tokio::test]
    async fn test_tracing_recorder() {
        // This test just ensures TracingQueryRecorder compiles and can be created
        let recorder = TracingQueryRecorder;
        let engine = QueryEngine::new(recorder, RiskGate::new());

        let mut query = create_test_query();

        // These should not panic
        engine.on_accepted(&query).await.unwrap();

        query.transition(QueryState::Processing).unwrap();
        engine.on_state_change(&query, "processing").await.unwrap();

        query.fail("Test error".to_string(), "processing".to_string());
        engine.on_state_change(&query, "failed").await.unwrap();
    }

    #[tokio::test]
    async fn test_full_lifecycle_with_mock_recorder() {
        let recorder = Arc::new(MockRecorder::new());
        let engine = QueryEngine::new(Arc::clone(&recorder), RiskGate::new());

        let mut query = create_test_query();

        // Accepted
        engine.on_accepted(&query).await.unwrap();
        assert_eq!(recorder.transition_count(), 1);

        // Processing
        query.transition(QueryState::Processing).unwrap();
        engine.on_state_change(&query, "processing").await.unwrap();
        assert_eq!(recorder.transition_count(), 2);

        // Acting
        query.transition(QueryState::Acting).unwrap();
        engine.on_state_change(&query, "acting").await.unwrap();
        assert_eq!(recorder.transition_count(), 3);

        // Completed
        query.complete(QueryOutcome::ActionsExecuted { actions_count: 2 }).unwrap();
        engine.on_state_change(&query, "completed").await.unwrap();
        assert_eq!(recorder.transition_count(), 4);
    }

    #[tokio::test]
    async fn test_error_lifecycle_with_mock_recorder() {
        let recorder = Arc::new(MockRecorder::new());
        let engine = QueryEngine::new(Arc::clone(&recorder), RiskGate::new());

        let mut query = create_test_query();

        // Accepted
        engine.on_accepted(&query).await.unwrap();
        assert_eq!(recorder.transition_count(), 1);

        // Processing
        query.transition(QueryState::Processing).unwrap();
        engine.on_state_change(&query, "processing").await.unwrap();
        assert_eq!(recorder.transition_count(), 2);

        // Failed
        query.fail("Engine error".to_string(), "processing".to_string());
        engine.on_state_change(&query, "failed").await.unwrap();
        assert_eq!(recorder.transition_count(), 3);
    }

    #[test]
    fn test_query_engine_new() {
        let recorder = TracingQueryRecorder;
        let _engine = QueryEngine::new(recorder, RiskGate::new());
    }

    // =========================================================================
    // Phase 2: check_risk tests
    // =========================================================================

    fn sample_proposed() -> ProposedTrade {
        ProposedTrade {
            symbol: "BTCUSDT".to_string(),
            side: "long".to_string(),
            quantity: dec!(0.02),
            entry_price: dec!(50000),
            notional_value: dec!(1000),
            margin_required: dec!(100),
        }
    }

    fn empty_context() -> RiskContext {
        RiskContext::new(dec!(10000))
    }

    fn saturated_context() -> RiskContext {
        // Exhaust monthly budget: 4 positions each with 100 latent risk.
        // budget = 10000 * 4% = 400, risk_per_trade = 100
        // latent_risk = 4 * 100 = 400 → remaining = 0 → no slots
        RiskContext::with_positions(dec!(10000), vec![
            PositionSummary {
                position_id: uuid::Uuid::nil(),
                symbol: "ETHUSDT".to_string(),
                side: "long".to_string(),
                notional_value: dec!(1000),
                margin_used: dec!(100),
                unrealized_pnl: Decimal::ZERO,
                entry_price: dec!(10000),
                quantity: dec!(0.01),
                current_stop: dec!(0), // stop at 0 → latent = entry * qty = 100
            },
            PositionSummary {
                position_id: uuid::Uuid::nil(),
                symbol: "SOLUSDT".to_string(),
                side: "long".to_string(),
                notional_value: dec!(1000),
                margin_used: dec!(100),
                unrealized_pnl: Decimal::ZERO,
                entry_price: dec!(10000),
                quantity: dec!(0.01),
                current_stop: dec!(0),
            },
            PositionSummary {
                position_id: uuid::Uuid::nil(),
                symbol: "XRPUSDT".to_string(),
                side: "long".to_string(),
                notional_value: dec!(1000),
                margin_used: dec!(100),
                unrealized_pnl: Decimal::ZERO,
                entry_price: dec!(10000),
                quantity: dec!(0.01),
                current_stop: dec!(0),
            },
            PositionSummary {
                position_id: uuid::Uuid::nil(),
                symbol: "DOGEUSDT".to_string(),
                side: "long".to_string(),
                notional_value: dec!(1000),
                margin_used: dec!(100),
                unrealized_pnl: Decimal::ZERO,
                entry_price: dec!(10000),
                quantity: dec!(0.01),
                current_stop: dec!(0),
            },
        ])
    }

    fn dummy_actions() -> Vec<EngineAction> {
        use robson_domain::{OrderSide, Price, Quantity, Symbol};
        vec![EngineAction::PlaceEntryOrder {
            position_id: uuid::Uuid::nil(),
            cycle_id: None,
            symbol: Symbol::from_pair("BTCUSDT").unwrap(),
            side: OrderSide::Buy,
            quantity: Quantity::new(dec!(0.01)).unwrap(),
            order_id: uuid::Uuid::nil(),
            client_order_id: String::new(),
            expected_price: Price::new(dec!(95000)).unwrap(),
            signal_id: uuid::Uuid::nil(),
        }]
    }

    #[tokio::test]
    async fn test_check_risk_approved_returns_governed_action() {
        let recorder = Arc::new(MockRecorder::new());
        let engine = QueryEngine::new(Arc::clone(&recorder), RiskGate::new());

        let mut query = create_test_query();
        query.transition(QueryState::Processing).unwrap();

        let actions = dummy_actions();
        let result = engine
            .check_risk(&mut query, &sample_proposed(), &empty_context(), actions)
            .await;

        assert!(result.is_ok(), "Expected Ok(GovernedAction) for approved trade");
        assert_eq!(query.state, QueryState::RiskChecked);
        assert_eq!(recorder.transition_count(), 1);
    }

    #[tokio::test]
    async fn test_governed_action_stamps_cycle_id_on_entry_order_requested() {
        use robson_domain::{Event, OrderSide, Quantity, Symbol};

        let recorder = Arc::new(MockRecorder::new());
        let engine = QueryEngine::new(Arc::clone(&recorder), RiskGate::new());

        let mut query = create_test_query();
        query.transition(QueryState::Processing).unwrap();

        let position_id = Uuid::now_v7();
        let signal_id = Uuid::now_v7();
        let actions = vec![
            EngineAction::EmitEvent(Event::EntryOrderRequested {
                position_id,
                cycle_id: None,
                order_id: Uuid::now_v7(),
                client_order_id: signal_id.to_string(),
                expected_price: Price::new(dec!(95000)).unwrap(),
                quantity: Quantity::new(dec!(0.01)).unwrap(),
                signal_id,
                timestamp: chrono::Utc::now(),
            }),
            EngineAction::PlaceEntryOrder {
                position_id,
                cycle_id: None,
                symbol: Symbol::from_pair("BTCUSDT").unwrap(),
                side: OrderSide::Buy,
                quantity: Quantity::new(dec!(0.01)).unwrap(),
                order_id: Uuid::now_v7(),
                client_order_id: signal_id.to_string(),
                expected_price: Price::new(dec!(95000)).unwrap(),
                signal_id,
            },
        ];

        let governed = engine
            .check_risk(&mut query, &sample_proposed(), &empty_context(), actions)
            .await
            .expect("risk should approve");

        assert_eq!(governed.cycle_id(), query.id);

        let returned = governed.into_actions();
        let cycle_id = returned.iter().find_map(|action| match action {
            EngineAction::EmitEvent(Event::EntryOrderRequested { cycle_id, .. }) => *cycle_id,
            _ => None,
        });

        assert_eq!(cycle_id, Some(query.id));
    }

    #[tokio::test]
    async fn test_check_approval_not_required_returns_ready() {
        let recorder = Arc::new(MockRecorder::new());
        let engine = QueryEngine::new(Arc::clone(&recorder), RiskGate::new());

        let mut query = create_test_query();
        query.transition(QueryState::Processing).unwrap();

        let no_approval_proposed = ProposedTrade {
            symbol: "BTCUSDT".to_string(),
            side: "long".to_string(),
            quantity: dec!(0.004),
            entry_price: dec!(95000),
            notional_value: dec!(400),
            margin_required: dec!(40),
        };

        let governed = engine
            .check_risk(&mut query, &no_approval_proposed, &empty_context(), dummy_actions())
            .await
            .expect("risk should approve");

        let result = engine
            .check_approval(&mut query, &no_approval_proposed, &empty_context(), governed)
            .await;

        assert!(matches!(result, Ok(ApprovalCheckResult::Ready(_))));
        assert_eq!(query.state, QueryState::RiskChecked);
        assert!(query.approval.is_none(), "No approval metadata should be attached");
    }

    #[tokio::test]
    async fn test_check_approval_required_moves_to_awaiting_approval() {
        let recorder = Arc::new(MockRecorder::new());
        let engine = QueryEngine::new(Arc::clone(&recorder), RiskGate::new());

        let mut query = create_test_query();
        query.transition(QueryState::Processing).unwrap();

        let approval_proposed = ProposedTrade {
            symbol: "BTCUSDT".to_string(),
            side: "long".to_string(),
            quantity: dec!(0.01),
            entry_price: dec!(95000),
            notional_value: dec!(1000),
            margin_required: dec!(100),
        };

        let governed = engine
            .check_risk(&mut query, &approval_proposed, &empty_context(), dummy_actions())
            .await
            .expect("risk should approve");

        let result = engine
            .check_approval(&mut query, &approval_proposed, &empty_context(), governed)
            .await;

        assert!(matches!(result, Ok(ApprovalCheckResult::AwaitingApproval(_))));
        assert_eq!(query.state, QueryState::AwaitingApproval);
        let approval = query.approval.as_ref().expect("approval metadata must be present");
        assert_eq!(approval.ttl_seconds, 300);
        assert!(approval.reason.contains("approval threshold"));
        assert_eq!(recorder.transition_count(), 2);
    }

    #[tokio::test]
    async fn test_authorize_transitions_query_to_authorized() {
        let recorder = Arc::new(MockRecorder::new());
        let engine = QueryEngine::new(Arc::clone(&recorder), RiskGate::new());

        let mut query = create_test_query();
        query.transition(QueryState::Processing).unwrap();
        query.transition(QueryState::RiskChecked).unwrap();
        query.await_approval("Manual gate".to_string(), 30).unwrap();

        engine.authorize(&mut query).await.unwrap();

        assert_eq!(query.state, QueryState::Authorized);
        assert!(
            query.approval.as_ref().and_then(|approval| approval.approved_at).is_some(),
            "Authorized queries must record approved_at"
        );
    }

    #[tokio::test]
    async fn test_expire_transitions_query_to_expired() {
        let recorder = Arc::new(MockRecorder::new());
        let engine = QueryEngine::new(Arc::clone(&recorder), RiskGate::new());

        let mut query = create_test_query();
        query.transition(QueryState::Processing).unwrap();
        query.transition(QueryState::RiskChecked).unwrap();
        query.await_approval("Manual gate".to_string(), 30).unwrap();

        engine.expire(&mut query, "expired").await.unwrap();

        assert_eq!(query.state, QueryState::Expired);
        assert!(query.finished_at.is_some(), "Expired queries are terminal");
    }

    #[tokio::test]
    async fn test_revalidate_risk_denied_from_awaiting_approval() {
        let recorder = Arc::new(MockRecorder::new());
        let engine = QueryEngine::new(Arc::clone(&recorder), RiskGate::new());

        let mut query = create_test_query();
        query.transition(QueryState::Processing).unwrap();
        query.transition(QueryState::RiskChecked).unwrap();
        query.await_approval("Manual gate".to_string(), 30).unwrap();

        let result = engine
            .revalidate_risk(&mut query, &sample_proposed(), &saturated_context())
            .await;

        assert!(matches!(result, Err(CheckRiskError::Denied)));
        assert!(matches!(query.state, QueryState::Denied { .. }));
        assert!(query.finished_at.is_some(), "Denied queries are terminal");
    }

    #[tokio::test]
    async fn test_check_risk_denied_transitions_to_denied_state() {
        let recorder = Arc::new(MockRecorder::new());
        let engine = QueryEngine::new(Arc::clone(&recorder), RiskGate::new());

        let mut query = create_test_query();
        query.transition(QueryState::Processing).unwrap();

        let result = engine
            .check_risk(&mut query, &sample_proposed(), &saturated_context(), dummy_actions())
            .await;

        assert!(result.is_err(), "Expected Err(CheckRiskError::Denied) for saturated portfolio");
        assert!(
            matches!(query.state, QueryState::Denied { .. }),
            "Query should be in Denied state after risk denial"
        );
        assert!(query.finished_at.is_some(), "Denied is a terminal state");
    }

    #[tokio::test]
    async fn test_check_risk_denied_records_via_recorder() {
        let recorder = Arc::new(MockRecorder::new());
        let engine = QueryEngine::new(Arc::clone(&recorder), RiskGate::new());

        let mut query = create_test_query();
        query.transition(QueryState::Processing).unwrap();

        let before = recorder.transition_count();
        engine
            .check_risk(&mut query, &sample_proposed(), &saturated_context(), dummy_actions())
            .await
            .ok();

        // check_risk records: RiskChecked + Denied = 2 additional transitions
        assert!(
            recorder.transition_count() >= before + 2,
            "Expected at least 2 recorded transitions for risk denial"
        );
    }

    #[tokio::test]
    async fn test_check_risk_invalid_state_returns_invalid_state_error() {
        // Calling check_risk() from Accepted (not Processing) must fail the
        // Processing → RiskChecked transition and return InvalidState, NOT Denied.
        // This proves that a state machine bug is not silently treated as a
        // governed denial.
        let recorder = Arc::new(MockRecorder::new());
        let engine = QueryEngine::new(Arc::clone(&recorder), RiskGate::new());

        // Do NOT transition to Processing — query is still in Accepted state.
        let mut query = create_test_query();

        let result = engine
            .check_risk(&mut query, &sample_proposed(), &empty_context(), dummy_actions())
            .await;

        assert!(
            matches!(result, Err(CheckRiskError::InvalidState(_))),
            "Expected Err(CheckRiskError::InvalidState), got {:?}",
            result
        );
        // Query must NOT have transitioned to Denied — that would be a false governance
        // record.
        assert!(
            !matches!(query.state, QueryState::Denied { .. }),
            "InvalidState must not produce a Denied query state"
        );
    }

    #[tokio::test]
    async fn test_governed_action_into_actions_returns_original() {
        let recorder = Arc::new(MockRecorder::new());
        let engine = QueryEngine::new(Arc::clone(&recorder), RiskGate::new());

        let mut query = create_test_query();
        query.transition(QueryState::Processing).unwrap();

        let actions = dummy_actions();
        let actions_count = actions.len();

        let governed = engine
            .check_risk(&mut query, &sample_proposed(), &empty_context(), actions)
            .await
            .expect("Should approve");

        let returned = governed.into_actions();
        assert_eq!(returned.len(), actions_count);
    }
}
