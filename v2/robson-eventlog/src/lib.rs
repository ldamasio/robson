//! Event Log Module
//!
//! Provides append-only event log with:
//! - Optimistic concurrency control via sequence numbers
//! - Idempotency via semantic payload hashing
//! - Multi-tenant isolation
//! - Stream-based partitioning
//!
//! # Usage
//!
//! ```rust,no_run
//! use robson_eventlog::{EventLog, Event, append_event};
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let pool = sqlx::PgPool::connect("postgresql://...").await?;
//! let stream_key = "position:01HN3XJQR6YQ...";
//!
//! let event = Event {
//!     event_type: "POSITION_OPENED".to_string(),
//!     payload: serde_json::json!({
//!         "position_id": "01HN3XJQR6YQ...",
//!         "symbol": "BTCUSDT",
//!         "side": "long"
//!     }),
//!     // ... other fields
//! };
//!
//! let event_id = append_event(&pool, stream_key, None, event).await?;
//! # Ok(())
//! # }
//! ```

pub mod append;
pub mod idempotency;
pub mod query;
pub mod types;

pub use append::append_event;
pub use idempotency::compute_idempotency_key;
pub use query::{query_events, QueryOptions};
pub use types::{ActorType, Event, EventEnvelope, EventLogError};
