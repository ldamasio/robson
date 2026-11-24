//! Projection errors

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ProjectionError {
    #[error("Unknown event type: {0}")]
    UnknownEventType(String),

    #[error("Invalid payload for event {event_type}: {reason}")]
    InvalidPayload { event_type: String, reason: String },

    #[error("Invariant violated for event {event_type}: {reason}")]
    InvariantViolated { event_type: String, reason: String },

    #[error("Missing handler for event type {event_type} (seq={seq}, stream={stream_key})")]
    MissingHandler {
        event_type: String,
        seq: i64,
        stream_key: String,
    },

    /// A balance-account daemon's accounting step refused (ADR-0056 step 8l-4).
    #[error("Balance accounting refused: {0}")]
    Accounting(String),

    #[error("Database error: {0}")]
    Database(#[from] sqlx::Error),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, ProjectionError>;
