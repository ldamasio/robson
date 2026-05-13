//! Daemon error types.

use robson_domain::DomainError;
use robson_engine::EngineError;
use robson_exec::ExecError;
use robson_store::StoreError;
use rust_decimal::Decimal;
use thiserror::Error;
use uuid::Uuid;

use crate::{position_monitor::MonitorError, query_engine::QueryRecorderError};

/// Minimal payload describing one stale-Active position detected at startup.
#[derive(Debug, Clone)]
pub struct StartupStaleActiveInfo {
    pub position_id: Uuid,
    pub symbol: String,
    pub side: String,
    pub quantity: Decimal,
    pub entry_price: Option<Decimal>,
}

/// Daemon-level errors.
#[derive(Debug, Error)]
pub enum DaemonError {
    /// Domain error
    #[error("Domain error: {0}")]
    Domain(#[from] DomainError),

    /// Engine error
    #[error("Engine error: {0}")]
    Engine(#[from] EngineError),

    /// Execution error
    #[error("Execution error: {0}")]
    Exec(#[from] ExecError),

    /// Store error
    #[error("Store error: {0}")]
    Store(#[from] StoreError),

    /// Database error (only available with postgres feature)
    #[cfg(feature = "postgres")]
    #[error("Database error: {0}")]
    Database(#[from] sqlx::Error),

    /// Projection error (only available with postgres feature)
    #[cfg(feature = "postgres")]
    #[error("Projection error: {0}")]
    Projection(#[from] robson_projector::ProjectionError),

    /// Position not found
    #[error("Position not found: {0}")]
    PositionNotFound(Uuid),

    /// Query not found
    #[error("Query not found: {0}")]
    QueryNotFound(Uuid),

    /// Query audit persistence failed
    #[error("Query audit error: {0}")]
    QueryAudit(#[from] QueryRecorderError),

    /// Position already exists
    #[error("Position already exists: {0}")]
    PositionAlreadyExists(Uuid),

    /// Invalid position state for operation
    #[error("Invalid position state: expected {expected}, got {actual}")]
    InvalidPositionState { expected: String, actual: String },

    /// Detector error
    #[error("Detector error: {0}")]
    Detector(String),

    /// Event bus error
    #[error("Event bus error: {0}")]
    EventBus(String),

    /// Configuration error
    #[error("Configuration error: {0}")]
    Config(String),

    /// Approval expired before the operator authorized the query
    #[error("Approval expired for query: {0}")]
    ApprovalExpired(Uuid),

    /// Approval was requested, but current risk no longer allows execution
    #[error("Approval denied for query {query_id}: {reason}")]
    ApprovalDenied { query_id: Uuid, reason: String },

    /// Monitor error
    #[error("Monitor error: {0}")]
    Monitor(#[from] MonitorError),

    /// EventLog persistence failure (append or projection apply).
    ///
    /// Raised when `event_log_pool` is configured and a domain event fails
    /// to be appended or applied. This is a hard error — callers must not
    /// silently continue, as doing so would leave `positions_current` stale.
    #[error("EventLog error: {0}")]
    EventLog(String),

    /// MonthlyHalt is active — all new entries blocked (v3 policy).
    ///
    /// Returned when `arm_position`, `handle_signal`, or `approve_query` is
    /// called while the system is in MonthlyHalt (4% monthly drawdown reached).
    /// MonthlyHalt persists until next calendar month or operator
    /// acknowledgment.
    #[error("MonthlyHalt active: {reason}")]
    MonthlyHaltActive { reason: String },

    /// Shutdown requested
    #[error("Shutdown requested")]
    Shutdown,

    /// One or more Robson-Active positions are absent from the exchange at
    /// startup.
    ///
    /// Exit code 78 (EX_CONFIG). Daemon refuses to enter the control loop.
    /// Operator must resolve each position manually before restarting.
    /// See runbook: docs/runbooks/td-2026-05-05-001-stale-active-recovery.md
    #[error(
        "Startup aborted: {count} stale-active position(s) detected (exit 78 — \
         see runbook td-2026-05-05-001-stale-active-recovery.md)"
    )]
    StartupStaleActiveDetected {
        count: usize,
        positions: Vec<StartupStaleActiveInfo>,
    },
}

/// Result type for daemon operations.
pub type DaemonResult<T> = Result<T, DaemonError>;

/// Maps a `DaemonError` to a process exit code.
///
/// Used by `main` to convert typed errors to exit codes without scattering
/// `std::process::exit` throughout the call sites.
pub fn exit_code_for_daemon_error(err: &DaemonError) -> i32 {
    match err {
        DaemonError::StartupStaleActiveDetected { .. } => 78,
        _ => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_startup_stale_active_exit_code_is_78() {
        let err = DaemonError::StartupStaleActiveDetected { count: 1, positions: vec![] };
        assert_eq!(exit_code_for_daemon_error(&err), 78);
    }

    #[test]
    fn test_other_errors_exit_code_is_1() {
        let err = DaemonError::Config("bad config".to_string());
        assert_eq!(exit_code_for_daemon_error(&err), 1);
    }
}
