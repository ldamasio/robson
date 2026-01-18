//! Event Querying

use crate::types::{EventEnvelope, EventLogError, Result};
use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

/// Query options for reading events
#[derive(Debug, Clone)]
pub struct QueryOptions {
    /// Filter by tenant ID (required for multi-tenancy)
    pub tenant_id: Uuid,

    /// Filter by stream key
    pub stream_key: Option<String>,

    /// Filter by event type
    pub event_type: Option<String>,

    /// Start time (inclusive)
    pub from_time: Option<DateTime<Utc>>,

    /// End time (exclusive)
    pub to_time: Option<DateTime<Utc>>,

    /// Start sequence number (inclusive)
    pub from_seq: Option<i64>,

    /// End sequence number (exclusive)
    pub to_seq: Option<i64>,

    /// Trace ID for correlation
    pub trace_id: Option<Uuid>,

    /// Command ID
    pub command_id: Option<Uuid>,

    /// Workflow ID
    pub workflow_id: Option<Uuid>,

    /// Limit results
    pub limit: Option<i64>,

    /// Ascending (true) or descending (false) by occurred_at
    pub ascending: bool,
}

impl QueryOptions {
    /// Create new query options for tenant
    pub fn new(tenant_id: Uuid) -> Self {
        Self {
            tenant_id,
            stream_key: None,
            event_type: None,
            from_time: None,
            to_time: None,
            from_seq: None,
            to_seq: None,
            trace_id: None,
            command_id: None,
            workflow_id: None,
            limit: None,
            ascending: true,
        }
    }

    /// Filter by stream key
    pub fn stream(mut self, stream_key: impl Into<String>) -> Self {
        self.stream_key = Some(stream_key.into());
        self
    }

    /// Filter by event type
    pub fn event_type(mut self, event_type: impl Into<String>) -> Self {
        self.event_type = Some(event_type.into());
        self
    }

    /// Filter by time range
    pub fn time_range(mut self, from: DateTime<Utc>, to: DateTime<Utc>) -> Self {
        self.from_time = Some(from);
        self.to_time = Some(to);
        self
    }

    /// Filter by sequence range
    pub fn seq_range(mut self, from: i64, to: i64) -> Self {
        self.from_seq = Some(from);
        self.to_seq = Some(to);
        self
    }

    /// Filter by trace ID
    pub fn trace(mut self, trace_id: Uuid) -> Self {
        self.trace_id = Some(trace_id);
        self
    }

    /// Limit results
    pub fn limit(mut self, limit: i64) -> Self {
        self.limit = Some(limit);
        self
    }

    /// Sort descending by time
    pub fn descending(mut self) -> Self {
        self.ascending = false;
        self
    }
}

/// Query events from log
pub async fn query_events(pool: &PgPool, options: QueryOptions) -> Result<Vec<EventEnvelope>> {
    let mut query = String::from("SELECT * FROM event_log WHERE tenant_id = $1");
    let mut bind_count = 1;

    // Build dynamic query based on filters
    if options.stream_key.is_some() {
        bind_count += 1;
        query.push_str(&format!(" AND stream_key = ${}", bind_count));
    }

    if options.event_type.is_some() {
        bind_count += 1;
        query.push_str(&format!(" AND event_type = ${}", bind_count));
    }

    if options.from_time.is_some() {
        bind_count += 1;
        query.push_str(&format!(" AND occurred_at >= ${}", bind_count));
    }

    if options.to_time.is_some() {
        bind_count += 1;
        query.push_str(&format!(" AND occurred_at < ${}", bind_count));
    }

    if options.from_seq.is_some() {
        bind_count += 1;
        query.push_str(&format!(" AND seq >= ${}", bind_count));
    }

    if options.to_seq.is_some() {
        bind_count += 1;
        query.push_str(&format!(" AND seq < ${}", bind_count));
    }

    if options.trace_id.is_some() {
        bind_count += 1;
        query.push_str(&format!(" AND trace_id = ${}", bind_count));
    }

    if options.command_id.is_some() {
        bind_count += 1;
        query.push_str(&format!(" AND command_id = ${}", bind_count));
    }

    if options.workflow_id.is_some() {
        bind_count += 1;
        query.push_str(&format!(" AND workflow_id = ${}", bind_count));
    }

    // Order by
    let order = if options.ascending { "ASC" } else { "DESC" };
    query.push_str(&format!(" ORDER BY occurred_at {}, seq {}", order, order));

    // Limit
    if let Some(limit) = options.limit {
        bind_count += 1;
        query.push_str(&format!(" LIMIT ${}", bind_count));
    }

    // Build query with bindings
    let mut q = sqlx::query_as::<_, EventEnvelopeRow>(&query).bind(options.tenant_id);

    if let Some(ref stream_key) = options.stream_key {
        q = q.bind(stream_key);
    }
    if let Some(ref event_type) = options.event_type {
        q = q.bind(event_type);
    }
    if let Some(from_time) = options.from_time {
        q = q.bind(from_time);
    }
    if let Some(to_time) = options.to_time {
        q = q.bind(to_time);
    }
    if let Some(from_seq) = options.from_seq {
        q = q.bind(from_seq);
    }
    if let Some(to_seq) = options.to_seq {
        q = q.bind(to_seq);
    }
    if let Some(trace_id) = options.trace_id {
        q = q.bind(trace_id);
    }
    if let Some(command_id) = options.command_id {
        q = q.bind(command_id);
    }
    if let Some(workflow_id) = options.workflow_id {
        q = q.bind(workflow_id);
    }
    if let Some(limit) = options.limit {
        q = q.bind(limit);
    }

    let rows = q.fetch_all(pool).await?;

    Ok(rows.into_iter().map(Into::into).collect())
}

/// Database row mapping
#[derive(sqlx::FromRow)]
struct EventEnvelopeRow {
    event_id: Uuid,
    tenant_id: Uuid,
    stream_key: String,
    seq: i64,
    event_type: String,
    payload: serde_json::Value,
    payload_schema_version: i32,
    occurred_at: DateTime<Utc>,
    ingested_at: DateTime<Utc>,
    idempotency_key: String,
    trace_id: Option<Uuid>,
    causation_id: Option<Uuid>,
    command_id: Option<Uuid>,
    workflow_id: Option<Uuid>,
    actor_type: Option<String>,
    actor_id: Option<String>,
    prev_hash: Option<String>,
    hash: Option<String>,
}

impl From<EventEnvelopeRow> for EventEnvelope {
    fn from(row: EventEnvelopeRow) -> Self {
        use crate::types::ActorType;

        let actor_type = row.actor_type.and_then(|s| match s.as_str() {
            "CLI" => Some(ActorType::CLI),
            "Daemon" => Some(ActorType::Daemon),
            "System" => Some(ActorType::System),
            "Exchange" => Some(ActorType::Exchange),
            _ => None,
        });

        Self {
            event_id: row.event_id,
            tenant_id: row.tenant_id,
            stream_key: row.stream_key,
            seq: row.seq,
            event_type: row.event_type,
            payload: row.payload,
            payload_schema_version: row.payload_schema_version,
            occurred_at: row.occurred_at,
            ingested_at: row.ingested_at,
            idempotency_key: row.idempotency_key,
            trace_id: row.trace_id,
            causation_id: row.causation_id,
            command_id: row.command_id,
            workflow_id: row.workflow_id,
            actor_type,
            actor_id: row.actor_id,
            prev_hash: row.prev_hash,
            hash: row.hash,
        }
    }
}

// TODO: Add helper functions for common queries
// - get_stream_events(stream_key, from_seq, limit)
// - get_position_events(position_id)
// - get_order_events(order_id)
// - get_trace_events(trace_id)

#[cfg(test)]
mod tests {
    use super::*;

    // TODO: Add integration tests
    // - Test query by stream
    // - Test query by time range
    // - Test query by trace ID
    // - Test pagination with limit
}
