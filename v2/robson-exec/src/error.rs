//! Execution layer error types.

use thiserror::Error;

/// Errors that can occur during execution operations.
#[derive(Debug, Error)]
pub enum ExecError {
    /// Exchange communication error
    #[error("Exchange error: {0}")]
    Exchange(String),

    /// Order was rejected by exchange
    #[error("Order rejected: {0}")]
    OrderRejected(String),

    /// Intent journal error
    #[error("Intent journal error: {0}")]
    IntentJournal(String),

    /// Intent already processed (idempotency check)
    #[error("Intent already processed: {0}")]
    AlreadyProcessed(uuid::Uuid),

    /// Store error
    #[error("Store error: {0}")]
    Store(#[from] robson_store::StoreError),

    /// Domain error
    #[error("Domain error: {0}")]
    Domain(#[from] robson_domain::DomainError),

    /// Engine error
    #[error("Engine error: {0}")]
    Engine(#[from] robson_engine::EngineError),

    /// Invalid state for operation
    #[error("Invalid state: {0}")]
    InvalidState(String),

    /// Configuration error
    #[error("Configuration error: {0}")]
    Config(String),

    /// Timeout waiting for operation
    #[error("Timeout: {0}")]
    Timeout(String),
}

/// Result type for execution operations.
pub type ExecResult<T> = Result<T, ExecError>;
