//! Repository trait definitions (Ports)
//!
//! These traits define the storage interface for the domain.
//! Implementations can be PostgreSQL, in-memory, or mock for testing.

use crate::error::StoreError;
use async_trait::async_trait;
use robson_domain::{Event, Order, OrderId, Position, PositionId};
use uuid::Uuid;

/// Repository for Position entities
#[async_trait]
pub trait PositionRepository: Send + Sync {
    /// Save a position (insert or update)
    async fn save(&self, position: &Position) -> Result<(), StoreError>;

    /// Find a position by ID
    async fn find_by_id(&self, id: PositionId) -> Result<Option<Position>, StoreError>;

    /// Find all positions for an account
    async fn find_by_account(&self, account_id: Uuid) -> Result<Vec<Position>, StoreError>;

    /// Find all active positions (state = Armed or Active)
    async fn find_active(&self) -> Result<Vec<Position>, StoreError>;

    /// Find positions by state
    async fn find_by_state(&self, state: &str) -> Result<Vec<Position>, StoreError>;

    /// Delete a position (soft delete - marks as closed)
    async fn delete(&self, id: PositionId) -> Result<(), StoreError>;
}

/// Repository for Order entities
#[async_trait]
pub trait OrderRepository: Send + Sync {
    /// Save an order (insert or update)
    async fn save(&self, order: &Order) -> Result<(), StoreError>;

    /// Find an order by ID
    async fn find_by_id(&self, id: OrderId) -> Result<Option<Order>, StoreError>;

    /// Find orders by position ID
    async fn find_by_position(&self, position_id: PositionId) -> Result<Vec<Order>, StoreError>;

    /// Find order by exchange order ID
    async fn find_by_exchange_id(&self, exchange_id: &str) -> Result<Option<Order>, StoreError>;

    /// Find order by client order ID
    async fn find_by_client_id(&self, client_id: &str) -> Result<Option<Order>, StoreError>;

    /// Find pending orders
    async fn find_pending(&self) -> Result<Vec<Order>, StoreError>;
}

/// Repository for Event entities (append-only)
#[async_trait]
pub trait EventRepository: Send + Sync {
    /// Append an event to the log
    async fn append(&self, event: &Event) -> Result<i64, StoreError>;

    /// Load all events for a position (in order)
    async fn find_by_position(&self, position_id: PositionId) -> Result<Vec<Event>, StoreError>;

    /// Load events for a position after a given sequence number
    async fn find_by_position_after(
        &self,
        position_id: PositionId,
        after_seq: i64,
    ) -> Result<Vec<Event>, StoreError>;

    /// Get the latest event sequence number for a position
    async fn get_latest_seq(&self, position_id: PositionId) -> Result<Option<i64>, StoreError>;
}

/// Combined store interface
#[async_trait]
pub trait Store: Send + Sync {
    /// Get position repository
    fn positions(&self) -> &dyn PositionRepository;

    /// Get order repository
    fn orders(&self) -> &dyn OrderRepository;

    /// Get event repository
    fn events(&self) -> &dyn EventRepository;

    /// Begin a transaction (for implementations that support it)
    async fn begin_transaction(&self) -> Result<(), StoreError> {
        Ok(()) // Default no-op for non-transactional stores
    }

    /// Commit the current transaction
    async fn commit(&self) -> Result<(), StoreError> {
        Ok(()) // Default no-op
    }

    /// Rollback the current transaction
    async fn rollback(&self) -> Result<(), StoreError> {
        Ok(()) // Default no-op
    }
}
