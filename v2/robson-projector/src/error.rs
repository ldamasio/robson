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

    #[error("Database error: {0}")]
    Database(#[from] sqlx::Error),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, ProjectionError>;
