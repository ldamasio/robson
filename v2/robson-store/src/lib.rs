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
//!     // Find active positions
//!     let active = store.find_active().await.unwrap();
//!     println!("Active positions: {}", active.len());
//! }
//! ```

#![warn(clippy::all)]

// Modules
mod error;
mod memory;
mod repository;

// Re-exports
pub use error::StoreError;
pub use memory::MemoryStore;
pub use repository::{EventRepository, OrderRepository, PositionRepository, Store};
