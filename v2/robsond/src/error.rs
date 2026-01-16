//! Daemon error types.

use robson_domain::DomainError;
use robson_engine::EngineError;
use robson_exec::ExecError;
use robson_store::StoreError;
use thiserror::Error;
use uuid::Uuid;

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

    /// Position not found
    #[error("Position not found: {0}")]
    PositionNotFound(Uuid),

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

    /// Shutdown requested
    #[error("Shutdown requested")]
    Shutdown,
}

/// Result type for daemon operations.
pub type DaemonResult<T> = Result<T, DaemonError>;
