//! QueryEngine: the governed execution core.
//!
//! Phase 1: Lifecycle tracker.
//! Records query state transitions via QueryRecorder.
//! Does NOT dispatch to Engine/Executor (PositionManager still does that).
//!
//! Phase 2+: Becomes the governed dispatcher.
//!
//! Ownership: lives INSIDE robsond crate. Not a separate crate.

use crate::query::ExecutionQuery;

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
// QueryEngine
// =============================================================================

/// Phase 1: Lifecycle tracker.
/// Records query state transitions via QueryRecorder.
/// Does NOT dispatch to Engine/Executor (PositionManager still does that).
///
/// Phase 2+: Becomes the governed dispatcher.
///
/// Ownership: lives INSIDE robsond crate. Not a separate crate.
pub struct QueryEngine<R: QueryRecorder> {
    recorder: R,
}

impl<R: QueryRecorder> QueryEngine<R> {
    /// Create a new QueryEngine with the given recorder.
    pub fn new(recorder: R) -> Self {
        Self { recorder }
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
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::{ActorKind, QueryKind, QueryOutcome, QueryState};
    use robson_domain::{Price, Side, Symbol};
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

    #[test]
    fn test_query_engine_delegates_to_recorder() {
        let recorder = Arc::new(MockRecorder::new());
        let engine = QueryEngine::new(Arc::clone(&recorder));

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
        let engine = QueryEngine::new(recorder);

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
        let engine = QueryEngine::new(Arc::clone(&recorder));

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
        let engine = QueryEngine::new(Arc::clone(&recorder));

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
        let _engine = QueryEngine::new(recorder);
    }
}
