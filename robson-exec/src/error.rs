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

    /// The venue refused an order because of a value in its trading rules
    /// (lot step, minimum or maximum quantity, tick, minimum notional,
    /// precision). Metadata may be stale, so risk reduction refreshes once and
    /// resubmits (ADR-0056 §5). What counts is a narrow per-venue allow-list in
    /// the adapter. This is not finality evidence: whether an order can still
    /// surface is answered separately by `VenueFinalityRule`. Displays as
    /// [`ExecError::Exchange`]: the type is for ADR-0056 §5, and to a caller of
    /// the API a venue refusal answers what it always has.
    #[error("Exchange error: {0}")]
    RulesRejected(String),

    /// The venue refused a reduce-only order. On a position account that means
    /// nothing is left to reduce, or the order conflicts with others resting on
    /// the same position; which one takes an account observation to tell.
    /// Displays as [`ExecError::Exchange`], for the same reason.
    #[error("Exchange error: {0}")]
    ReduceOnlyRejected(String),

    /// The order journal would not authorize this send, so no request left
    /// under this client id (ADR-0056 step 7). The caller mints a new id on its
    /// next try.
    #[error("Exchange error: {0}")]
    SendRefused(String),

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

    /// Futures safety check failed (critical safety violation)
    ///
    /// This error is returned when the account is not in the expected
    /// position mode (One-way) or leverage setting.
    /// **DO NOT proceed with orders when this error occurs.**
    #[error("FUTURES SAFETY VIOLATION: expected {expected}, got {actual}. {advice}")]
    FuturesSafetyViolation {
        /// What was expected (e.g., "One-way mode", "1x leverage")
        expected: String,
        /// What was found (e.g., "Hedge mode", "5x leverage")
        actual: String,
        /// Advice on how to fix
        advice: String,
    },
}

/// Result type for execution operations.
pub type ExecResult<T> = Result<T, ExecError>;
