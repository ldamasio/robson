//! Event Log Types

use chrono::{DateTime, Utc};
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
}

pub type Result<T> = std::result::Result<T, EventLogError>;
