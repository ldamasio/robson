//! Robson v2 Storage Layer
//!
//! Provides persistence for positions, orders, and events.
//!
//! # Architecture
//!
//! - **Repository traits**: Define the storage interface (ports)
//! - **In-memory store**: Fast implementation for testing
//! - **PostgreSQL store**: Production implementation (feature `postgres`)
//!
//! # Usage
//!
//! ```rust
//! use robson_store::{MemoryStore, Store, PositionRepository};
//! use robson_domain::{Position, Symbol, Side};
//! use uuid::Uuid;
//!
//! #[tokio::main]
//! async fn main() {
//!     let store = MemoryStore::new();
//!
//!     // Create and save a position
//!     let position = Position::new(
//!         Uuid::now_v7(),
//!         Symbol::from_pair("BTCUSDT").unwrap(),
//!         Side::Long,
//!     );
//!     store.save(&position).await.unwrap();
//!
//!     // Find open core positions (historically named "active")
//!     let open = store.find_active().await.unwrap();
//!     println!("Open positions: {}", open.len());
//! }
//! ```

#![warn(clippy::all)]

// Modules
mod credential_store;
mod detected_position;
mod error;
mod memory;
#[cfg(feature = "postgres")]
mod postgres;
mod repository;

// Re-exports
#[cfg(feature = "postgres")]
pub use credential_store::PgCredentialStore;
pub use credential_store::{CredentialStore, MemoryCredentialStore};
#[cfg(feature = "postgres")]
pub use detected_position::PgDetectedPositionRepository;
pub use detected_position::{
    DetectedPositionDto, DetectedPositionRepository, MemoryDetectedPositionRepository,
    SafetyExecutionDto,
};
pub use error::StoreError;
pub use memory::MemoryStore;
#[cfg(feature = "postgres")]
pub use postgres::{PgProjectionReader, ProjectionRecovery, find_active_from_projection};
pub use repository::{EventRepository, OrderRepository, PositionRepository, Store};
