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

    /// Find positions eligible for lifecycle management.
    ///
    /// Returns positions still participating in the core trading lifecycle:
    /// Armed, Entering, Active, Exiting.
    ///
    /// Excludes terminal states such as Closed and Error.
    /// Used by position managers that need to act on every open core position
    /// (e.g. process market ticks, panic close, shutdown cleanup).
    ///
    /// Note: does NOT filter to "Active only" despite historic naming. Use
    /// `find_risk_open()` when computing portfolio exposure for risk gates.
    async fn find_active(&self) -> Result<Vec<Position>, StoreError>;

    /// Find positions with committed exchange exposure for risk context computation.
    ///
    /// Returns only `Entering` and `Active` positions:
    /// - `Entering`: entry order submitted to exchange, waiting for fill.
    ///   Notional is committed on the exchange even before fill confirmation.
    /// - `Active`: position open on exchange with trailing stop monitoring.
    ///
    /// Excludes Armed (no order yet) and Exiting (reducing, not expanding exposure).
    ///
    /// Used exclusively by `build_risk_context()` to ensure concurrent entries
    /// cannot bypass exposure limits during the order-fill window.
    ///
    /// Default implementation uses two `find_by_state` calls. Override for
    /// a single-pass implementation.
    async fn find_risk_open(&self) -> Result<Vec<Position>, StoreError> {
        let mut result = self.find_by_state("entering").await?;
        result.extend(self.find_by_state("active").await?);
        Ok(result)
    }

    /// Find positions by state
    async fn find_by_state(&self, state: &str) -> Result<Vec<Position>, StoreError>;

    /// Find active Core Trading position by symbol and side.
    ///
    /// Returns Some(position) if found in Entering, Active, or Exiting state.
    /// Used by Safety Net to exclude Core-managed positions.
    async fn find_active_by_symbol_and_side(
        &self,
        symbol: &robson_domain::Symbol,
        side: robson_domain::Side,
    ) -> Result<Option<Position>, StoreError>;

    /// Delete a position (soft delete - marks as closed)
    async fn delete(&self, id: PositionId) -> Result<(), StoreError>;

    /// Find positions closed in a given month.
    ///
    /// Returns positions with `closed_at` in the specified year/month that
    /// are in `Closed` state. Used to compute monthly realized PnL.
    async fn find_closed_in_month(
        &self,
        year: i32,
        month: u32,
    ) -> Result<Vec<Position>, StoreError>;
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

    /// Get all stored events in order (for crash recovery/rebuild).
    ///
    /// Returns all events from the event log in sequence order.
    /// Used by MemoryStore to rebuild the in-memory projection on startup.
    /// For PostgreSQL stores, this may return an empty Vec (no-op).
    async fn get_all_events(&self) -> Result<Vec<Event>, StoreError>;
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

    /// Apply an event to update the in-memory projection.
    ///
    /// This is called AFTER the event is appended to the EventLog.
    /// Order is critical: append FIRST, apply AFTER.
    ///
    /// Default implementation is a no-op (for PostgreSQL stores).
    /// MemoryStore overrides this to update positions synchronously.
    fn apply_event(&self, _event: &robson_domain::Event) -> Result<(), StoreError> {
        Ok(()) // Default no-op for non-in-memory stores
    }

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
