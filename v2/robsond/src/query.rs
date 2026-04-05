//! ExecutionQuery: the typed lifecycle unit for one runtime trigger.
//!
//! Every trigger that enters the Runtime becomes one or more ExecutionQueries.
//! For single-position triggers (signal, disarm): one query per trigger.
//! For fan-out triggers (market tick, panic): one query PER POSITION affected.
//!
//! This is the control-loop unit: it tracks one complete
//! Observe -> Interpret -> Decide -> Act -> Evaluate -> Persist cycle.

use chrono::{DateTime, Duration, Utc};
use robson_domain::{PositionId, Price, Quantity, Side, Symbol, TechnicalStopDistance};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

// =============================================================================
// Error Types
// =============================================================================

/// Errors that can occur during query lifecycle.
#[derive(Debug, Error)]
pub enum QueryError {
    /// Invalid state transition attempted
    #[error("Invalid transition from {from} to {to}")]
    InvalidTransition { from: String, to: String },
}

// =============================================================================
// QueryKind - What triggered the execution query
// =============================================================================

/// What triggered the execution query.
///
/// IMPORTANT: Fan-out triggers (ProcessMarketTick, PanicClose) do NOT become
/// a single query. The PositionManager creates one ExecutionQuery PER POSITION
/// affected. This preserves per-position auditability and outcome tracking.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum QueryKind {
    /// Detector fired an entry signal (one query, one position)
    ProcessSignal {
        signal_id: Uuid,
        symbol: Symbol,
        side: Side,
        entry_price: Price,
        stop_loss: Price,
    },

    /// Market price update for ONE active position
    /// (PositionManager creates one query per active position matching the symbol)
    ProcessMarketTick { symbol: Symbol, price: Price },

    /// Operator requests position arming
    ArmPosition {
        symbol: Symbol,
        side: Side,
        tech_stop_distance: TechnicalStopDistance,
        account_id: Uuid,
    },

    /// Operator requests position disarming
    DisarmPosition { position_id: PositionId },

    /// Emergency close ONE position (PanicClose creates one query per position)
    PanicClosePosition { position_id: PositionId },

    /// Safety Net detected rogue position
    #[allow(dead_code)]
    SafetyNetExit {
        position_id: PositionId,
        reason: String,
    },

    /// Order fill received from exchange
    ProcessOrderFill {
        order_id: Uuid,
        fill_price: Price,
        fill_quantity: Quantity,
    },

    /// Periodic health/reconciliation check
    HealthCheck,
}

// =============================================================================
// QueryState - Lifecycle state machine
// =============================================================================

/// Lifecycle state machine for an ExecutionQuery.
///
/// Phase 1:
/// ```text
///   Accepted -> Processing -> Acting -> Completed
///                  |            |
///                  v            v
///               Failed       Failed
/// ```
///
/// Phase 2 (current):
/// ```text
///   Accepted -> Processing -> RiskChecked -> Acting -> Completed
///                  |              |             |
///                  v              v             v
///               Failed         Denied        Failed
/// ```
///
/// Phase 3+ adds: AwaitingApproval, Authorized between RiskChecked and Acting.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum QueryState {
    /// Query created, validated, queued
    Accepted,

    /// Engine + Risk are evaluating (Interpret + Decide phases)
    Processing,

    /// Phase 2: Risk gate evaluated. Either approved (→ Acting) or denied (→ Denied).
    RiskChecked,

    /// Executor is executing governed actions (Act phase)
    Acting,

    /// Successfully completed.
    Completed,

    /// Terminal failure (operational error — not a governance decision)
    Failed { reason: String, phase: String },

    /// Phase 2: Terminal governance denial.
    ///
    /// Distinct from `Failed`: denial is an intentional governed outcome,
    /// not a system failure. The risk gate or a future approval gate rejected
    /// the action before any side effects occurred.
    ///
    /// `check` identifies which governance rule triggered the denial.
    Denied { reason: String, check: String },
}

// =============================================================================
// QueryOutcome - The result of a completed query
// =============================================================================

/// The result of a completed query.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum QueryOutcome {
    /// Actions were executed via Executor.
    ActionsExecuted { actions_count: usize },

    /// Query evaluated but no action was needed (e.g., price tick didn't trigger stop)
    NoAction { reason: String },

    /// Risk Engine or governance denied the action (Phase 2+)
    Denied { reason: String },
}

// =============================================================================
// CommandSource - Where the operator command originated
// =============================================================================

/// Source of an operator command.
/// Phase 1: informational only (logged via tracing).
/// Phase 2+: may determine approval requirements.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum CommandSource {
    /// Command from CLI (robson CLI or direct invocation)
    Cli,
    /// Command from HTTP API (external system or dashboard)
    Api,
    /// Command from internal system (recovery, reconciliation)
    Internal,
}

// =============================================================================
// ActorKind - Who or what initiated the query
// =============================================================================

/// Who or what initiated the query.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ActorKind {
    /// Operator via CLI or API
    Operator {
        /// Source of the command
        source: CommandSource,
    },

    /// Signal detector (external or manual injection)
    Detector,

    /// Market data feed (WebSocket or REST fallback)
    MarketData,

    /// Safety Net (rogue position monitor)
    SafetyNet,

    /// Internal system (timer, recovery, reconciliation)
    System { subsystem: String },
}

// =============================================================================
// Scaffolding Enums (Phase 1 — Permissive, Future-Ready)
// =============================================================================

/// Classification of side effects. Used by governance layers.
/// Phase 1: informational only (logged via tracing).
/// Phase 2+: determines approval requirements.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ActionClass {
    /// Reads state, no side effects
    ReadOnly,

    /// Writes to exchange (orders, cancellations)
    ExchangeWrite,

    /// Modifies runtime configuration or risk limits
    ControlPlaneWrite,

    /// Overrides a risk denial
    RiskOverride,

    /// Replay or recovery operation
    ReplayRepair,
}

/// Whether an action requires human approval.
/// Phase 1: always NotRequired.
/// Phase 3+: determined by action class, risk level, and configuration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ApprovalRequirement {
    /// No approval needed
    NotRequired,

    /// Approval required with reason and TTL
    Required { reason: String, ttl_seconds: u64 },
}

/// Result of a permission check.
/// Phase 1: always Granted.
/// Phase 2+: determined by Risk Engine and governance rules.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum PermissionDecision {
    /// Action permitted
    Granted,

    /// Action denied
    Denied { reason: String },

    /// Requires elevation
    RequiresElevation { scope: String },
}

// =============================================================================
// ContextSummary - Runtime context snapshot
// =============================================================================

/// Runtime context snapshot at query creation time.
/// Phase 1: informational only (logged via tracing).
/// Phase 2+: may determine risk thresholds.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextSummary {
    /// Number of active positions at query creation time
    pub active_positions_count: usize,
}

// =============================================================================
// ExecutionQuery - The typed lifecycle unit
// =============================================================================

/// A typed execution lifecycle unit.
/// Every trigger that enters the Runtime becomes one or more ExecutionQueries.
/// For single-position triggers (signal, disarm): one query per trigger.
/// For fan-out triggers (market tick, panic): one query PER POSITION affected.
/// This is the control-loop unit: it tracks one complete
/// Observe -> Interpret -> Decide -> Act -> Evaluate -> Persist cycle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionQuery {
    /// Unique identifier (UUID v7, time-ordered)
    pub id: Uuid,

    /// What triggered this query
    pub kind: QueryKind,

    /// Current lifecycle state
    pub state: QueryState,

    /// Who/what initiated this query
    pub actor: ActorKind,

    /// Associated position (if applicable)
    pub position_id: Option<PositionId>,

    /// Runtime context snapshot at creation time
    pub context_summary: Option<ContextSummary>,

    /// When this query was created
    pub started_at: DateTime<Utc>,

    /// When this query reached a terminal state
    pub finished_at: Option<DateTime<Utc>>,

    /// Final outcome (set when Completed or Failed)
    pub outcome: Option<QueryOutcome>,
}

impl ExecutionQuery {
    /// Create a new query. Always starts in Accepted state.
    pub fn new(kind: QueryKind, actor: ActorKind) -> Self {
        Self {
            id: Uuid::now_v7(),
            kind,
            state: QueryState::Accepted,
            actor,
            position_id: None,
            context_summary: None,
            started_at: Utc::now(),
            finished_at: None,
            outcome: None,
        }
    }

    /// Transition to a new state. Returns error if transition is invalid.
    pub fn transition(&mut self, new_state: QueryState) -> Result<(), QueryError> {
        // Enforce valid transitions
        match (&self.state, &new_state) {
            // Accepted -> Processing
            (QueryState::Accepted, QueryState::Processing) => Ok(()),

            // Processing -> RiskChecked (Phase 2: entry through risk gate)
            (QueryState::Processing, QueryState::RiskChecked) => Ok(()),

            // Processing -> Acting (exit/safe operations that bypass risk gate)
            (QueryState::Processing, QueryState::Acting) => Ok(()),

            // Processing -> Completed (no action before risk check)
            (QueryState::Processing, QueryState::Completed) => Ok(()),

            // RiskChecked -> Acting (risk approved)
            (QueryState::RiskChecked, QueryState::Acting) => Ok(()),

            // RiskChecked -> Denied (risk denied — governed terminal state)
            (QueryState::RiskChecked, QueryState::Denied { .. }) => Ok(()),

            // Acting -> Completed
            (QueryState::Acting, QueryState::Completed) => Ok(()),

            // Any non-terminal state can fail (operational error)
            (QueryState::Accepted, QueryState::Failed { .. }) => Ok(()),
            (QueryState::Processing, QueryState::Failed { .. }) => Ok(()),
            (QueryState::RiskChecked, QueryState::Failed { .. }) => Ok(()),
            (QueryState::Acting, QueryState::Failed { .. }) => Ok(()),

            // All other transitions are invalid
            _ => Err(QueryError::InvalidTransition {
                from: format!("{:?}", self.state),
                to: format!("{:?}", new_state),
            }),
        }?;

        self.state = new_state;

        // Set finished_at for terminal states
        if matches!(
            self.state,
            QueryState::Completed | QueryState::Failed { .. } | QueryState::Denied { .. }
        ) {
            self.finished_at = Some(Utc::now());
        }

        Ok(())
    }

    /// Convenience: mark as completed with outcome.
    ///
    /// Only sets outcome AFTER transition succeeds to maintain atomicity.
    pub fn complete(&mut self, outcome: QueryOutcome) -> Result<(), QueryError> {
        // Validate and perform transition FIRST
        match &self.state {
            QueryState::Acting => self.transition(QueryState::Completed),
            QueryState::Processing => self.transition(QueryState::Completed),
            _ => Err(QueryError::InvalidTransition {
                from: format!("{:?}", self.state),
                to: "Completed".to_string(),
            }),
        }?;

        // Only set outcome AFTER successful transition
        // If transition succeeded but we're in an unexpected state, rollback not needed
        // because the state machine guarantees we're now in Completed
        self.outcome = Some(outcome);
        Ok(())
    }

    /// Convenience: mark as failed (operational error).
    ///
    /// The `Failed` state with its `reason` and `phase` fields is the
    /// authoritative record of failure. Does NOT set `outcome`.
    ///
    /// For governance denials (risk gate, approval gate), use `deny()` instead.
    pub fn fail(&mut self, reason: String, phase: String) {
        // Transition is best-effort for failure
        // The Failed state captures reason/phase - no need to pollute outcome
        let _ = self.transition(QueryState::Failed { reason, phase });
    }

    /// Convenience: mark as governance-denied (Phase 2+).
    ///
    /// Called when the risk gate or a future approval gate rejects the action.
    /// This is NOT an operational failure — it is an intentional governed outcome.
    ///
    /// Sets `outcome = Some(QueryOutcome::Denied)` for audit and projections.
    /// `check` identifies which governance rule triggered the denial (e.g. "max_open_positions").
    /// Query must be in `RiskChecked` state before calling this.
    pub fn deny(&mut self, reason: String, check: String) {
        // Set outcome BEFORE transition so it is recorded even if transition is already terminal
        self.outcome = Some(QueryOutcome::Denied { reason: reason.clone() });
        let _ = self.transition(QueryState::Denied { reason, check });
    }

    /// Duration of query execution (None if not yet completed).
    pub fn duration(&self) -> Option<Duration> {
        self.finished_at.map(|f| f - self.started_at)
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use robson_domain::Symbol;
    use rust_decimal_macros::dec;

    fn create_test_query() -> ExecutionQuery {
        ExecutionQuery::new(
            QueryKind::ProcessSignal {
                signal_id: Uuid::now_v7(),
                symbol: Symbol::from_pair("BTCUSDT").unwrap(),
                side: Side::Long,
                entry_price: Price::new(dec!(95000)).unwrap(),
                stop_loss: Price::new(dec!(93500)).unwrap(),
            },
            ActorKind::Detector,
        )
    }

    #[test]
    fn test_new_starts_accepted() {
        let query = create_test_query();

        assert_eq!(query.state, QueryState::Accepted);
        assert!(query.context_summary.is_none());
        assert!(query.finished_at.is_none());
        assert!(query.outcome.is_none());
    }

    #[test]
    fn test_valid_transitions() {
        // Accepted -> Processing -> Acting -> Completed
        let mut query = create_test_query();

        query.transition(QueryState::Processing).unwrap();
        assert_eq!(query.state, QueryState::Processing);

        query.transition(QueryState::Acting).unwrap();
        assert_eq!(query.state, QueryState::Acting);

        query.complete(QueryOutcome::ActionsExecuted { actions_count: 1 }).unwrap();
        assert_eq!(query.state, QueryState::Completed);
        assert!(query.finished_at.is_some());
        assert!(matches!(
            query.outcome,
            Some(QueryOutcome::ActionsExecuted { actions_count: 1 })
        ));
    }

    #[test]
    fn test_processing_to_completed_directly() {
        // Accepted -> Processing -> Completed (NoAction)
        let mut query = create_test_query();

        query.transition(QueryState::Processing).unwrap();

        query
            .complete(QueryOutcome::NoAction { reason: "No trigger".to_string() })
            .unwrap();

        assert_eq!(query.state, QueryState::Completed);
    }

    #[test]
    fn test_invalid_transition_accepted_to_acting() {
        let mut query = create_test_query();

        let result = query.transition(QueryState::Acting);
        assert!(result.is_err());
    }

    #[test]
    fn test_invalid_transition_completed_to_anything() {
        let mut query = create_test_query();

        query.transition(QueryState::Processing).unwrap();
        query.transition(QueryState::Acting).unwrap();
        query.complete(QueryOutcome::ActionsExecuted { actions_count: 1 }).unwrap();

        // Try to transition from Completed
        let result = query.transition(QueryState::Processing);
        assert!(result.is_err());
    }

    #[test]
    fn test_fail_from_processing() {
        let mut query = create_test_query();

        query.transition(QueryState::Processing).unwrap();
        query.fail("Test error".to_string(), "processing".to_string());

        match query.state {
            QueryState::Failed { reason, phase } => {
                assert_eq!(reason, "Test error");
                assert_eq!(phase, "processing");
            },
            _ => panic!("Expected Failed state"),
        }
        assert!(query.finished_at.is_some());
    }

    #[test]
    fn test_fail_from_acting() {
        let mut query = create_test_query();

        query.transition(QueryState::Processing).unwrap();
        query.transition(QueryState::Acting).unwrap();
        query.fail("Executor error".to_string(), "acting".to_string());

        match query.state {
            QueryState::Failed { reason, phase } => {
                assert_eq!(reason, "Executor error");
                assert_eq!(phase, "acting");
            },
            _ => panic!("Expected Failed state"),
        }
    }

    #[test]
    fn test_fail_from_accepted() {
        let mut query = create_test_query();

        query.fail("Validation error".to_string(), "accepted".to_string());

        match query.state {
            QueryState::Failed { reason, phase } => {
                assert_eq!(reason, "Validation error");
                assert_eq!(phase, "accepted");
            },
            _ => panic!("Expected Failed state"),
        }
    }

    #[test]
    fn test_double_complete_fails() {
        let mut query = create_test_query();

        query.transition(QueryState::Processing).unwrap();
        query.complete(QueryOutcome::NoAction { reason: "First".to_string() }).unwrap();

        // Try to complete again
        let result = query.complete(QueryOutcome::ActionsExecuted { actions_count: 1 });
        assert!(result.is_err());
    }

    #[test]
    fn test_double_complete_preserves_first_outcome() {
        // Verify atomicity: failed complete should not mutate outcome
        let mut query = create_test_query();

        // First, transition to a state where complete() is valid
        query.transition(QueryState::Processing).unwrap();
        query
            .complete(QueryOutcome::NoAction { reason: "First outcome".to_string() })
            .unwrap();

        // Verify first outcome was set
        assert!(query.outcome.is_some());

        // Try to complete with different outcome - should fail (already Completed)
        let result = query.complete(QueryOutcome::ActionsExecuted { actions_count: 999 });
        assert!(result.is_err());

        // Original outcome should be preserved
        match &query.outcome {
            Some(QueryOutcome::NoAction { reason }) => {
                assert_eq!(reason, "First outcome");
            },
            _ => panic!("Expected original NoAction outcome to be preserved"),
        }
    }

    #[test]
    fn test_fail_does_not_set_outcome() {
        // Phase 1: fail() should NOT set outcome - Failed state is authoritative
        let mut query = create_test_query();

        query.transition(QueryState::Processing).unwrap();
        query.fail("Test error".to_string(), "processing".to_string());

        // Outcome should remain None
        assert!(
            query.outcome.is_none(),
            "fail() should not set outcome in Phase 1 - Failed state is authoritative"
        );

        // But state should be Failed
        assert!(matches!(query.state, QueryState::Failed { .. }));
    }

    #[test]
    fn test_deny_sets_outcome_and_terminal_state() {
        // Phase 2: deny() must set outcome for audit/projections
        let mut query = create_test_query();

        query.transition(QueryState::Processing).unwrap();
        query.transition(QueryState::RiskChecked).unwrap();
        query.deny("Too many positions".to_string(), "max_open_positions".to_string());

        // State must be Denied (terminal)
        assert!(
            matches!(query.state, QueryState::Denied { .. }),
            "deny() must transition to Denied state"
        );

        // outcome must be Some(Denied) for audit and projections
        assert!(
            matches!(query.outcome, Some(QueryOutcome::Denied { .. })),
            "deny() must set outcome = Some(QueryOutcome::Denied)"
        );

        // finished_at must be set (Denied is terminal)
        assert!(query.finished_at.is_some(), "Denied is a terminal state — finished_at must be set");
    }

    #[test]
    fn test_duration_set_on_complete() {
        let mut query = create_test_query();

        query.transition(QueryState::Processing).unwrap();
        query.complete(QueryOutcome::ActionsExecuted { actions_count: 1 }).unwrap();

        assert!(query.duration().is_some());
    }

    #[test]
    fn test_duration_none_before_complete() {
        let query = create_test_query();

        assert!(query.duration().is_none());
    }

    #[test]
    fn test_arm_position_kind() {
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let entry = Price::new(dec!(95000)).unwrap();
        let stop = Price::new(dec!(93500)).unwrap();
        let tech_stop = TechnicalStopDistance::from_entry_and_stop(entry, stop);

        let query = ExecutionQuery::new(
            QueryKind::ArmPosition {
                symbol: symbol.clone(),
                side: Side::Long,
                tech_stop_distance: tech_stop,
                account_id: Uuid::now_v7(),
            },
            ActorKind::Operator { source: CommandSource::Api },
        );

        assert!(matches!(query.kind, QueryKind::ArmPosition { .. }));
        assert!(matches!(query.actor, ActorKind::Operator { source: CommandSource::Api }));
    }

    #[test]
    fn test_disarm_position_kind() {
        let position_id = Uuid::now_v7();

        let query = ExecutionQuery::new(
            QueryKind::DisarmPosition { position_id },
            ActorKind::Operator { source: CommandSource::Api },
        );

        assert!(matches!(query.kind, QueryKind::DisarmPosition { .. }));
    }

    #[test]
    fn test_panic_close_position_kind() {
        let position_id = Uuid::now_v7();

        let query = ExecutionQuery::new(
            QueryKind::PanicClosePosition { position_id },
            ActorKind::Operator { source: CommandSource::Api },
        );

        assert!(matches!(query.kind, QueryKind::PanicClosePosition { .. }));
    }

    #[test]
    fn test_process_market_tick_kind() {
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();

        let query = ExecutionQuery::new(
            QueryKind::ProcessMarketTick {
                symbol: symbol.clone(),
                price: Price::new(dec!(96000)).unwrap(),
            },
            ActorKind::MarketData,
        );

        assert!(matches!(query.kind, QueryKind::ProcessMarketTick { .. }));
        assert!(matches!(query.actor, ActorKind::MarketData));
    }

    #[test]
    fn test_health_check_kind() {
        let query = ExecutionQuery::new(
            QueryKind::HealthCheck,
            ActorKind::System { subsystem: "scheduler".to_string() },
        );

        assert!(matches!(query.kind, QueryKind::HealthCheck));
    }

    #[test]
    fn test_scaffolding_enums() {
        // These are just to ensure they compile and are accessible
        let _class = ActionClass::ExchangeWrite;
        let _approval = ApprovalRequirement::NotRequired;
        let _source = CommandSource::Cli;
        let _context = ContextSummary { active_positions_count: 3 };
        let _permission = PermissionDecision::Granted;

        let _approval_required = ApprovalRequirement::Required {
            reason: "High risk".to_string(),
            ttl_seconds: 300,
        };

        let _denied = PermissionDecision::Denied { reason: "Over limit".to_string() };

        let _elevation = PermissionDecision::RequiresElevation { scope: "admin".to_string() };
    }
}
