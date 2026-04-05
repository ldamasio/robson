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
//! GovernedAction is `pub(crate)` — only constructible inside this module via
//! `check_risk()`. This ensures no entry path in robsond can reach the Executor
//! without passing through the risk gate.
//!
//! Architectural note (Phase 2 decision):
//! `GovernedAction` lives in robsond (not robson-exec). The Executor continues
//! accepting `Vec<EngineAction>` unchanged. Type-level enforcement across the
//! crate boundary is deferred as a follow-up architectural concern. The governance
//! guarantee in Phase 2 is: "risk evaluated inside runtime before any Executor call".
//!
//! Ownership: lives INSIDE robsond crate. Not a separate crate.

use robson_engine::{EngineAction, ProposedTrade, RiskContext, RiskGate, RiskVerdict};

use crate::query::{ExecutionQuery, QueryError, QueryState};

// =============================================================================
// QueryRecorder - Audit and Observability
// =============================================================================

/// Records query lifecycle events for observability and audit.
/// Phase 1: TracingQueryRecorder (structured logs via tracing crate).
/// Phase 2+: EventLogQueryRecorder (persists to robson-eventlog).
pub trait QueryRecorder: Send + Sync {
    /// Called when a query state changes.
    fn on_state_change(&self, query: &ExecutionQuery);

    /// Called when a query encounters an error.
    fn on_error(&self, query: &ExecutionQuery, error: &str);
}

// Blanket implementation for Arc<T>
impl<T: QueryRecorder + ?Sized> QueryRecorder for std::sync::Arc<T> {
    fn on_state_change(&self, query: &ExecutionQuery) {
        (**self).on_state_change(query);
    }

    fn on_error(&self, query: &ExecutionQuery, error: &str) {
        (**self).on_error(query, error);
    }
}

// =============================================================================
// TracingQueryRecorder - Default implementation
// =============================================================================

/// Default implementation: structured tracing logs.
/// Zero persistence overhead. Full observability via tracing subscribers.
pub struct TracingQueryRecorder;

impl QueryRecorder for TracingQueryRecorder {
    fn on_state_change(&self, query: &ExecutionQuery) {
        tracing::info!(
            query_id = %query.id,
            kind = ?query.kind,
            state = ?query.state,
            actor = ?query.actor,
            position_id = ?query.position_id,
            active_positions_count = ?query.context_summary.as_ref().map(|c| c.active_positions_count),
            duration_ms = ?query.duration().map(|d| d.num_milliseconds()),
            "query state transition"
        );
    }

    fn on_error(&self, query: &ExecutionQuery, error: &str) {
        tracing::error!(
            query_id = %query.id,
            kind = ?query.kind,
            state = ?query.state,
            active_positions_count = ?query.context_summary.as_ref().map(|c| c.active_positions_count),
            error = %error,
            "query engine error"
        );
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
    actions: Vec<EngineAction>,
    // Private field prevents external construction even within the crate if
    // someone tries to use struct literal syntax.
    _proof: (),
}

impl GovernedAction {
    /// Private: only QueryEngine::check_risk() may construct this.
    fn new(actions: Vec<EngineAction>) -> Self {
        Self { actions, _proof: () }
    }

    /// Consume the token and return the approved actions for Executor dispatch.
    pub(crate) fn into_actions(self) -> Vec<EngineAction> {
        self.actions
    }
}

// =============================================================================
// CheckRiskError — typed error for check_risk() callers
// =============================================================================

/// Error returned by `check_risk()`.
///
/// Two distinct cases that callers MUST handle differently:
///
/// - `Denied`: Risk gate rejected the trade. The query is in `QueryState::Denied`.
///   No exchange side effects occurred. Caller should treat this as a governed
///   outcome and return `Ok(())`.
///
/// - `InvalidState`: The query lifecycle state machine is in an unexpected state
///   and cannot enter `RiskChecked`. This is an operational error (e.g. a bug
///   in the caller or concurrent mutation). Caller should propagate as an error
///   — NOT treat it as a governance denial.
#[derive(Debug)]
pub(crate) enum CheckRiskError {
    /// Risk gate rejected the trade (governed outcome — no side effects occurred).
    Denied,
    /// Query state machine is inconsistent — cannot transition to RiskChecked.
    /// This is an operational error, not a governance decision.
    InvalidState(QueryError),
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
}

impl<R: QueryRecorder> QueryEngine<R> {
    /// Create a new QueryEngine with the given recorder and risk gate.
    pub fn new(recorder: R, risk_gate: RiskGate) -> Self {
        Self { recorder, risk_gate }
    }

    /// Record that a query has been accepted.
    pub fn on_accepted(&self, query: &ExecutionQuery) {
        self.recorder.on_state_change(query);
    }

    /// Record a state transition.
    pub fn on_state_change(&self, query: &ExecutionQuery) {
        self.recorder.on_state_change(query);
    }

    /// Record an error. Caller is responsible for calling query.fail() first.
    pub fn on_error(&self, query: &ExecutionQuery, error: &str) {
        self.recorder.on_error(query, error);
    }

    /// Phase 2: Evaluate risk and return a governed proof token.
    ///
    /// This is the mandatory governance gate for all entry execution paths.
    ///
    /// Returns:
    /// - `Ok(GovernedAction)`: risk approved — pass actions to Executor via
    ///   `governed.into_actions()`.
    /// - `Err(CheckRiskError::Denied)`: risk gate rejected the trade. Query is in
    ///   `QueryState::Denied`. Caller should treat as governed outcome (`Ok(())`).
    /// - `Err(CheckRiskError::InvalidState)`: query lifecycle state machine is
    ///   inconsistent (cannot enter RiskChecked). This is an operational error.
    ///   Caller MUST propagate as `Err`, not treat as a denial.
    ///
    /// `actions` are consumed here — there is no path to the Executor without
    /// passing through this check.
    pub(crate) fn check_risk(
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
        self.recorder.on_state_change(query);

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
                Ok(GovernedAction::new(actions))
            }
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
                self.recorder.on_state_change(query);
                Err(CheckRiskError::Denied)
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
    use crate::query::{ActorKind, QueryKind, QueryOutcome, QueryState};
    use robson_domain::{Price, Side, Symbol};
    use robson_engine::{PositionSummary, RiskContext, RiskGate, RiskLimits};
    use rust_decimal::Decimal;
    use rust_decimal_macros::dec;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Mock recorder that counts calls
    struct MockRecorder {
        state_change_count: AtomicUsize,
        error_count: AtomicUsize,
    }

    impl MockRecorder {
        fn new() -> Self {
            Self {
                state_change_count: AtomicUsize::new(0),
                error_count: AtomicUsize::new(0),
            }
        }

        fn state_change_count(&self) -> usize {
            self.state_change_count.load(Ordering::SeqCst)
        }

        fn error_count(&self) -> usize {
            self.error_count.load(Ordering::SeqCst)
        }
    }

    impl QueryRecorder for MockRecorder {
        fn on_state_change(&self, _query: &ExecutionQuery) {
            self.state_change_count.fetch_add(1, Ordering::SeqCst);
        }

        fn on_error(&self, _query: &ExecutionQuery, _error: &str) {
            self.error_count.fetch_add(1, Ordering::SeqCst);
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

    fn make_engine(recorder: impl QueryRecorder) -> QueryEngine<impl QueryRecorder> {
        QueryEngine::new(recorder, RiskGate::new())
    }

    #[test]
    fn test_query_engine_delegates_to_recorder() {
        let recorder = Arc::new(MockRecorder::new());
        let engine = QueryEngine::new(Arc::clone(&recorder), RiskGate::new());

        let query = create_test_query();

        // on_accepted should call on_state_change
        engine.on_accepted(&query);
        assert_eq!(recorder.state_change_count(), 1);

        // on_state_change should call on_state_change
        engine.on_state_change(&query);
        assert_eq!(recorder.state_change_count(), 2);

        // on_error should call on_error
        engine.on_error(&query, "test error");
        assert_eq!(recorder.error_count(), 1);
    }

    #[test]
    fn test_tracing_recorder() {
        // This test just ensures TracingQueryRecorder compiles and can be created
        let recorder = TracingQueryRecorder;
        let engine = QueryEngine::new(recorder, RiskGate::new());

        let mut query = create_test_query();

        // These should not panic
        engine.on_accepted(&query);

        query.transition(QueryState::Processing).unwrap();
        engine.on_state_change(&query);

        query.fail("Test error".to_string(), "processing".to_string());
        engine.on_error(&query, "Test error");
    }

    #[test]
    fn test_full_lifecycle_with_mock_recorder() {
        let recorder = Arc::new(MockRecorder::new());
        let engine = QueryEngine::new(Arc::clone(&recorder), RiskGate::new());

        let mut query = create_test_query();

        // Accepted
        engine.on_accepted(&query);
        assert_eq!(recorder.state_change_count(), 1);

        // Processing
        query.transition(QueryState::Processing).unwrap();
        engine.on_state_change(&query);
        assert_eq!(recorder.state_change_count(), 2);

        // Acting
        query.transition(QueryState::Acting).unwrap();
        engine.on_state_change(&query);
        assert_eq!(recorder.state_change_count(), 3);

        // Completed
        query.complete(QueryOutcome::ActionsExecuted { actions_count: 2 }).unwrap();
        engine.on_state_change(&query);
        assert_eq!(recorder.state_change_count(), 4);

        // No errors
        assert_eq!(recorder.error_count(), 0);
    }

    #[test]
    fn test_error_lifecycle_with_mock_recorder() {
        let recorder = Arc::new(MockRecorder::new());
        let engine = QueryEngine::new(Arc::clone(&recorder), RiskGate::new());

        let mut query = create_test_query();

        // Accepted
        engine.on_accepted(&query);
        assert_eq!(recorder.state_change_count(), 1);

        // Processing
        query.transition(QueryState::Processing).unwrap();
        engine.on_state_change(&query);
        assert_eq!(recorder.state_change_count(), 2);

        // Failed
        query.fail("Engine error".to_string(), "processing".to_string());
        engine.on_error(&query, "Engine error");
        assert_eq!(recorder.error_count(), 1);
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
        // 3 open positions — triggers MaxOpenPositions
        RiskContext::with_positions(
            dec!(10000),
            vec![
                PositionSummary {
                    position_id: uuid::Uuid::nil(),
                    symbol: "ETHUSDT".to_string(),
                    side: "long".to_string(),
                    notional_value: dec!(1000),
                    margin_used: dec!(100),
                    unrealized_pnl: Decimal::ZERO,
                };
                3
            ],
            Decimal::ZERO,
            Decimal::ZERO,
        )
    }

    fn dummy_actions() -> Vec<EngineAction> {
        // Use EmitEvent as a stand-in (we just need a non-empty Vec)
        use robson_domain::{Event, PositionId, TechnicalStopDistance, Side, Symbol};
        use robson_domain::Price;
        use rust_decimal_macros::dec;
        vec![EngineAction::EmitEvent(Event::PositionArmed {
            position_id: uuid::Uuid::nil(),
            account_id: uuid::Uuid::nil(),
            symbol: Symbol::from_pair("BTCUSDT").unwrap(),
            side: Side::Long,
            tech_stop_distance: None,
            timestamp: chrono::Utc::now(),
        })]
    }

    #[test]
    fn test_check_risk_approved_returns_governed_action() {
        let recorder = Arc::new(MockRecorder::new());
        let engine = QueryEngine::new(Arc::clone(&recorder), RiskGate::new());

        let mut query = create_test_query();
        query.transition(QueryState::Processing).unwrap();

        let actions = dummy_actions();
        let result =
            engine.check_risk(&mut query, &sample_proposed(), &empty_context(), actions);

        assert!(result.is_ok(), "Expected Ok(GovernedAction) for approved trade");
        assert_eq!(query.state, QueryState::RiskChecked);
        // state_change recorded for Accepted (from create_test_query) + RiskChecked transition
        assert!(recorder.state_change_count() >= 1);
    }

    #[test]
    fn test_check_risk_denied_transitions_to_denied_state() {
        let recorder = Arc::new(MockRecorder::new());
        let engine = QueryEngine::new(Arc::clone(&recorder), RiskGate::new());

        let mut query = create_test_query();
        query.transition(QueryState::Processing).unwrap();

        let result =
            engine.check_risk(&mut query, &sample_proposed(), &saturated_context(), dummy_actions());

        assert!(result.is_err(), "Expected Err(CheckRiskError::Denied) for saturated portfolio");
        assert!(
            matches!(query.state, QueryState::Denied { .. }),
            "Query should be in Denied state after risk denial"
        );
        assert!(query.finished_at.is_some(), "Denied is a terminal state");
    }

    #[test]
    fn test_check_risk_denied_records_via_recorder() {
        let recorder = Arc::new(MockRecorder::new());
        let engine = QueryEngine::new(Arc::clone(&recorder), RiskGate::new());

        let mut query = create_test_query();
        query.transition(QueryState::Processing).unwrap();

        let before = recorder.state_change_count();
        engine
            .check_risk(&mut query, &sample_proposed(), &saturated_context(), dummy_actions())
            .ok();

        // check_risk records: RiskChecked + Denied = 2 additional transitions
        assert!(
            recorder.state_change_count() >= before + 2,
            "Expected at least 2 recorded transitions for risk denial"
        );
    }

    #[test]
    fn test_check_risk_invalid_state_returns_invalid_state_error() {
        // Calling check_risk() from Accepted (not Processing) must fail the
        // Processing → RiskChecked transition and return InvalidState, NOT Denied.
        // This proves that a state machine bug is not silently treated as a
        // governed denial.
        let recorder = Arc::new(MockRecorder::new());
        let engine = QueryEngine::new(Arc::clone(&recorder), RiskGate::new());

        // Do NOT transition to Processing — query is still in Accepted state.
        let mut query = create_test_query();

        let result =
            engine.check_risk(&mut query, &sample_proposed(), &empty_context(), dummy_actions());

        assert!(
            matches!(result, Err(CheckRiskError::InvalidState(_))),
            "Expected Err(CheckRiskError::InvalidState), got {:?}", result
        );
        // Query must NOT have transitioned to Denied — that would be a false governance record.
        assert!(
            !matches!(query.state, QueryState::Denied { .. }),
            "InvalidState must not produce a Denied query state"
        );
    }

    #[test]
    fn test_governed_action_into_actions_returns_original() {
        let recorder = Arc::new(MockRecorder::new());
        let engine = QueryEngine::new(Arc::clone(&recorder), RiskGate::new());

        let mut query = create_test_query();
        query.transition(QueryState::Processing).unwrap();

        let actions = dummy_actions();
        let actions_count = actions.len();

        let governed = engine
            .check_risk(&mut query, &sample_proposed(), &empty_context(), actions)
            .expect("Should approve");

        let returned = governed.into_actions();
        assert_eq!(returned.len(), actions_count);
    }
}
