//! Event Log Types

use chrono::{DateTime, Datelike, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

/// Actor type that emitted the event
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, sqlx::Type)]
#[sqlx(type_name = "varchar")]
pub enum ActorType {
    /// CLI command
    CLI,
    /// Daemon autonomous action
    Daemon,
    /// System scheduled job
    System,
    /// External exchange event
    Exchange,
}

impl ActorType {
    pub fn as_str(&self) -> &'static str {
        match self {
            ActorType::CLI => "CLI",
            ActorType::Daemon => "Daemon",
            ActorType::System => "System",
            ActorType::Exchange => "Exchange",
        }
    }
}

/// Event envelope with all metadata
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct EventEnvelope {
    // Identity
    pub event_id: Uuid,
    pub tenant_id: Uuid,

    // Stream Partitioning
    pub stream_key: String,
    pub seq: i64,

    // Event Type & Data
    pub event_type: String,
    pub payload: serde_json::Value,
    pub payload_schema_version: i32,

    // Temporal
    pub occurred_at: DateTime<Utc>,
    pub ingested_at: DateTime<Utc>,

    // Idempotency
    pub idempotency_key: String,

    // Correlation
    pub trace_id: Option<Uuid>,
    pub causation_id: Option<Uuid>,
    pub command_id: Option<Uuid>,
    pub workflow_id: Option<Uuid>,

    // Actor
    pub actor_type: Option<ActorType>,
    pub actor_id: Option<String>,

    // Audit (optional)
    pub prev_hash: Option<String>,
    pub hash: Option<String>,
}

/// Event builder for constructing events
#[derive(Debug, Clone)]
pub struct Event {
    pub tenant_id: Uuid,
    pub stream_key: String,
    pub event_type: String,
    pub payload: serde_json::Value,
    pub occurred_at: DateTime<Utc>,

    // Optional correlation
    pub trace_id: Option<Uuid>,
    pub causation_id: Option<Uuid>,
    pub command_id: Option<Uuid>,
    pub workflow_id: Option<Uuid>,

    // Optional actor
    pub actor_type: Option<ActorType>,
    pub actor_id: Option<String>,

    // Schema version
    pub payload_schema_version: i32,
}

impl Event {
    /// Create a new event
    pub fn new(
        tenant_id: Uuid,
        stream_key: impl Into<String>,
        event_type: impl Into<String>,
        payload: serde_json::Value,
    ) -> Self {
        Self {
            tenant_id,
            stream_key: stream_key.into(),
            event_type: event_type.into(),
            payload,
            occurred_at: Utc::now(),
            trace_id: None,
            causation_id: None,
            command_id: None,
            workflow_id: None,
            actor_type: None,
            actor_id: None,
            payload_schema_version: 1,
        }
    }

    /// Set trace ID
    pub fn with_trace_id(mut self, trace_id: Uuid) -> Self {
        self.trace_id = Some(trace_id);
        self
    }

    /// Set causation ID
    pub fn with_causation_id(mut self, causation_id: Uuid) -> Self {
        self.causation_id = Some(causation_id);
        self
    }

    /// Set command ID
    pub fn with_command_id(mut self, command_id: Uuid) -> Self {
        self.command_id = Some(command_id);
        self
    }

    /// Set workflow ID
    pub fn with_workflow_id(mut self, workflow_id: Uuid) -> Self {
        self.workflow_id = Some(workflow_id);
        self
    }

    /// Set actor
    pub fn with_actor(mut self, actor_type: ActorType, actor_id: Option<String>) -> Self {
        self.actor_type = Some(actor_type);
        self.actor_id = actor_id;
        self
    }
}

/// Event log errors
#[derive(Debug, thiserror::Error)]
pub enum EventLogError {
    #[error("Concurrent modification: expected seq {expected}, got {actual}")]
    ConcurrentModification { expected: i64, actual: i64 },

    #[error("Stream not found: {0}")]
    StreamNotFound(String),

    #[error("Idempotent event already exists: {0}")]
    IdempotentDuplicate(Uuid),

    #[error("Database error: {0}")]
    Database(#[from] sqlx::Error),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Invalid event: {0}")]
    InvalidEvent(String),

    /// The cutover certificate's completion holds or waits for its key (8m
    /// PLAN-r4 §6.7): the append took nothing, wrote nothing and may be
    /// retried. Certain, unlike a database error.
    #[error("the cutover certificate is completing; nothing was appended")]
    CertificateInProgress,
}

pub type Result<T> = std::result::Result<T, EventLogError>;

/// Untrusted event reference with explicit shape validation. Public fields may
/// contain unvalidated data; even successful validation proves no commit or
/// ownership. Original coordinates must not be replaced by a projection's
/// latest event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventCoordinates {
    pub event_id: Uuid,
    pub tenant_id: Uuid,
    pub stream_key: String,
    pub seq: i64,
    pub occurred_at: DateTime<Utc>,
    pub ingested_at: DateTime<Utc>,
}

impl EventCoordinates {
    pub fn validate(&self) -> Result<()> {
        if self.event_id.is_nil()
            || self.tenant_id.is_nil()
            || self.stream_key.trim().is_empty()
            || self.seq <= 0
        {
            return Err(EventLogError::InvalidEvent("invalid event coordinates".into()));
        }
        Ok(())
    }

    /// UTC ingestion month only; occurred_at may be in another partition.
    pub fn ingestion_month(&self) -> (i32, u32) {
        (self.ingested_at.year(), self.ingested_at.month())
    }
}

#[cfg(test)]
mod coordinate_tests {
    use super::*;

    fn coordinates() -> EventCoordinates {
        EventCoordinates {
            event_id: Uuid::from_u128(1),
            tenant_id: Uuid::from_u128(2),
            stream_key: "position:test".into(),
            seq: 1,
            occurred_at: "2026-01-31T23:59:59Z".parse().unwrap(),
            ingested_at: "2026-02-01T00:00:00Z".parse().unwrap(),
        }
    }

    #[test]
    fn partition_uses_original_ingestion_not_event_occurrence() {
        let mut c = coordinates();
        c.validate().unwrap();
        assert_eq!(c.ingestion_month(), (2026, 2));
        c.occurred_at = "2025-12-01T00:00:00Z".parse().unwrap();
        assert_eq!(c.ingestion_month(), (2026, 2));
    }

    #[test]
    fn empty_identity_and_nonpositive_sequence_are_rejected() {
        let original = coordinates();
        let mut bad = original.clone();
        bad.event_id = Uuid::nil();
        assert!(bad.validate().is_err());
        bad = original.clone();
        bad.tenant_id = Uuid::nil();
        assert!(bad.validate().is_err());
        for stream in ["", " ", "\n"] {
            bad = original.clone();
            bad.stream_key = stream.into();
            assert!(bad.validate().is_err());
        }
        for seq in [0, -1, i64::MIN] {
            bad = original.clone();
            bad.seq = seq;
            assert!(bad.validate().is_err());
        }
        bad = original;
        bad.seq = i64::MAX;
        bad.validate().unwrap();
    }
}
