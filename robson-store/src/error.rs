//! Storage layer errors

use thiserror::Error;

/// Errors that can occur in the storage layer
#[derive(Debug, Error)]
pub enum StoreError {
    /// Entity not found
    #[error("Entity not found: {entity_type} with id {id}")]
    NotFound {
        /// Type of entity (position, order, event)
        entity_type: String,
        /// Entity ID
        id: String,
    },

    /// Duplicate entity (idempotency violation)
    #[error("Duplicate entity: {entity_type} with id {id}")]
    Duplicate {
        /// Type of entity
        entity_type: String,
        /// Entity ID
        id: String,
    },

    /// Invalid state transition
    #[error("Invalid state transition: {message}")]
    InvalidState {
        /// Description of the invalid transition
        message: String,
    },

    /// Serialization error
    #[error("Serialization error: {0}")]
    Serialization(String),

    /// Database error
    #[error("Database error: {0}")]
    Database(String),

    /// Connection error
    #[error("Connection error: {0}")]
    Connection(String),

    /// Deserialization error (reading from projection)
    #[error("Deserialization error: {0}")]
    Deserialization(String),

    /// Domain error passthrough
    #[error("Domain error: {0}")]
    Domain(#[from] robson_domain::DomainError),
}

impl StoreError {
    /// Create a not found error
    pub fn not_found(entity_type: impl Into<String>, id: impl Into<String>) -> Self {
        Self::NotFound {
            entity_type: entity_type.into(),
            id: id.into(),
        }
    }

    /// Create a duplicate error
    pub fn duplicate(entity_type: impl Into<String>, id: impl Into<String>) -> Self {
        Self::Duplicate {
            entity_type: entity_type.into(),
            id: id.into(),
        }
    }
}

#[cfg(feature = "postgres")]
impl From<sqlx::Error> for StoreError {
    fn from(err: sqlx::Error) -> Self {
        match err {
            sqlx::Error::RowNotFound => StoreError::NotFound {
                entity_type: "unknown".to_string(),
                id: "unknown".to_string(),
            },
            sqlx::Error::Database(db_err) => {
                // Check for unique constraint violation
                if db_err.code().map(|c| c == "23505").unwrap_or(false) {
                    StoreError::Duplicate {
                        entity_type: "unknown".to_string(),
                        id: "unknown".to_string(),
                    }
                } else {
                    StoreError::Database(db_err.to_string())
                }
            },
            _ => StoreError::Database(err.to_string()),
        }
    }
}
