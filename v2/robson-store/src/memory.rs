//! In-memory store implementation
//!
//! Used for testing and development without a database.
//! Thread-safe using RwLock for concurrent access.

use crate::error::StoreError;
use crate::repository::{EventRepository, OrderRepository, PositionRepository, Store};
use async_trait::async_trait;
use robson_domain::{Event, Order, OrderId, OrderStatus, Position, PositionId};
use std::collections::HashMap;
use std::sync::RwLock;
use std::sync::atomic::{AtomicI64, Ordering};
use uuid::Uuid;

/// In-memory store for testing
pub struct MemoryStore {
    positions: RwLock<HashMap<PositionId, Position>>,
    orders: RwLock<HashMap<OrderId, Order>>,
    events: RwLock<Vec<StoredEvent>>,
    event_seq: AtomicI64,
}

/// Event with sequence number
struct StoredEvent {
    seq: i64,
    event: Event,
}

impl MemoryStore {
    /// Create a new empty in-memory store
    pub fn new() -> Self {
        Self {
            positions: RwLock::new(HashMap::new()),
            orders: RwLock::new(HashMap::new()),
            events: RwLock::new(Vec::new()),
            event_seq: AtomicI64::new(0),
        }
    }

    /// Get the number of positions
    pub fn position_count(&self) -> usize {
        self.positions.read().unwrap().len()
    }

    /// Get the number of orders
    pub fn order_count(&self) -> usize {
        self.orders.read().unwrap().len()
    }

    /// Get the number of events
    pub fn event_count(&self) -> usize {
        self.events.read().unwrap().len()
    }

    /// Clear all data (useful for test setup)
    pub fn clear(&self) {
        self.positions.write().unwrap().clear();
        self.orders.write().unwrap().clear();
        self.events.write().unwrap().clear();
        self.event_seq.store(0, Ordering::SeqCst);
    }

    /// Internal method to apply an event to the in-memory position projection.
    ///
    /// This is called by the Store trait's apply_event method.
    /// Order is critical: append FIRST, apply AFTER.
    ///
    /// If apply fails, we fail-fast (error is ok).
    /// The EventLog remains the source of truth for recovery.
    fn apply_event_internal(&self, event: &Event) -> Result<(), StoreError> {
        use robson_domain::{ExitReason, PositionState};

        match event {
            // EntryFilled: Position becomes Active with initial trailing stop
            Event::EntryFilled {
                position_id,
                fill_price,
                filled_quantity,
                initial_stop,
                ..
            } => {
                let mut positions = self.positions.write().unwrap();
                if let Some(mut position) = positions.get(position_id).cloned() {
                    // Update position fields from event
                    position.entry_price = Some(*fill_price);
                    position.entry_filled_at = Some(chrono::Utc::now());
                    position.quantity = *filled_quantity;

                    // Transition to Active state with initial trailing stop
                    position.state = PositionState::Active {
                        current_price: *fill_price,
                        trailing_stop: *initial_stop,
                        favorable_extreme: *fill_price,
                        extreme_at: chrono::Utc::now(),
                        insurance_stop_id: None,
                        last_emitted_stop: Some(*initial_stop),
                    };
                    position.updated_at = chrono::Utc::now();

                    positions.insert(*position_id, position);
                }
                // If position not found, this is idempotent - nothing to update
            },

            // TrailingStopUpdated: Update the trailing stop in Active state
            Event::TrailingStopUpdated {
                position_id,
                new_stop,
                trigger_price,
                ..
            } => {
                let mut positions = self.positions.write().unwrap();
                if let Some(mut position) = positions.get(position_id).cloned() {
                    if let PositionState::Active {
                        ref mut current_price,
                        ref mut trailing_stop,
                        ref mut favorable_extreme,
                        ref mut extreme_at,
                        ref mut last_emitted_stop,
                        ..
                    } = position.state
                    {
                        *current_price = *trigger_price;
                        *trailing_stop = *new_stop;
                        *favorable_extreme = *trigger_price;
                        *extreme_at = chrono::Utc::now();
                        *last_emitted_stop = Some(*new_stop);
                        position.updated_at = chrono::Utc::now();

                        positions.insert(*position_id, position);
                    }
                }
                // If position not found or not in Active state, idempotent
            },

            // PositionClosed: Mark position as closed
            Event::PositionClosed {
                position_id,
                exit_price,
                exit_reason,
                realized_pnl,
                ..
            } => {
                let mut positions = self.positions.write().unwrap();
                if let Some(mut position) = positions.get(position_id).cloned() {
                    position.state = PositionState::Closed {
                        exit_price: *exit_price,
                        realized_pnl: *realized_pnl,
                        exit_reason: match exit_reason {
                            robson_domain::ExitReason::TrailingStop => ExitReason::TrailingStop,
                            robson_domain::ExitReason::InsuranceStop => ExitReason::InsuranceStop,
                            robson_domain::ExitReason::UserPanic => ExitReason::UserPanic,
                            _ => ExitReason::PositionError,
                        },
                    };
                    position.closed_at = Some(chrono::Utc::now());
                    position.updated_at = chrono::Utc::now();

                    positions.insert(*position_id, position);
                }
                // If position not found, idempotent
            },

            // Other events don't affect the in-memory projection
            _ => {
                // No-op for events that don't change position state
            },
        }

        Ok(())
    }
}

impl Default for MemoryStore {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// Position Repository Implementation
// =============================================================================

#[async_trait]
impl PositionRepository for MemoryStore {
    async fn save(&self, position: &Position) -> Result<(), StoreError> {
        let mut positions = self.positions.write().unwrap();
        positions.insert(position.id, position.clone());
        Ok(())
    }

    async fn find_by_id(&self, id: PositionId) -> Result<Option<Position>, StoreError> {
        let positions = self.positions.read().unwrap();
        Ok(positions.get(&id).cloned())
    }

    async fn find_by_account(&self, account_id: Uuid) -> Result<Vec<Position>, StoreError> {
        let positions = self.positions.read().unwrap();
        Ok(positions.values().filter(|p| p.account_id == account_id).cloned().collect())
    }

    async fn find_active(&self) -> Result<Vec<Position>, StoreError> {
        let positions = self.positions.read().unwrap();
        Ok(positions.values().filter(|p| p.can_enter() || p.can_exit()).cloned().collect())
    }

    async fn find_by_state(&self, state: &str) -> Result<Vec<Position>, StoreError> {
        let positions = self.positions.read().unwrap();
        Ok(positions.values().filter(|p| p.state.name() == state).cloned().collect())
    }

    async fn delete(&self, id: PositionId) -> Result<(), StoreError> {
        let mut positions = self.positions.write().unwrap();
        if positions.remove(&id).is_some() {
            Ok(())
        } else {
            Err(StoreError::not_found("position", id.to_string()))
        }
    }
}

// =============================================================================
// Order Repository Implementation
// =============================================================================

#[async_trait]
impl OrderRepository for MemoryStore {
    async fn save(&self, order: &Order) -> Result<(), StoreError> {
        let mut orders = self.orders.write().unwrap();
        orders.insert(order.id, order.clone());
        Ok(())
    }

    async fn find_by_id(&self, id: OrderId) -> Result<Option<Order>, StoreError> {
        let orders = self.orders.read().unwrap();
        Ok(orders.get(&id).cloned())
    }

    async fn find_by_position(&self, position_id: PositionId) -> Result<Vec<Order>, StoreError> {
        let orders = self.orders.read().unwrap();
        Ok(orders.values().filter(|o| o.position_id == position_id).cloned().collect())
    }

    async fn find_by_exchange_id(&self, exchange_id: &str) -> Result<Option<Order>, StoreError> {
        let orders = self.orders.read().unwrap();
        Ok(orders
            .values()
            .find(|o| o.exchange_order_id.as_deref() == Some(exchange_id))
            .cloned())
    }

    async fn find_by_client_id(&self, client_id: &str) -> Result<Option<Order>, StoreError> {
        let orders = self.orders.read().unwrap();
        Ok(orders.values().find(|o| o.client_order_id == client_id).cloned())
    }

    async fn find_pending(&self) -> Result<Vec<Order>, StoreError> {
        let orders = self.orders.read().unwrap();
        Ok(orders
            .values()
            .filter(|o| matches!(o.status, OrderStatus::Pending | OrderStatus::Submitted))
            .cloned()
            .collect())
    }
}

// =============================================================================
// Event Repository Implementation
// =============================================================================

#[async_trait]
impl EventRepository for MemoryStore {
    async fn append(&self, event: &Event) -> Result<i64, StoreError> {
        let seq = self.event_seq.fetch_add(1, Ordering::SeqCst) + 1;
        let stored = StoredEvent { seq, event: event.clone() };
        let mut events = self.events.write().unwrap();
        events.push(stored);
        Ok(seq)
    }

    async fn find_by_position(&self, position_id: PositionId) -> Result<Vec<Event>, StoreError> {
        let events = self.events.read().unwrap();
        Ok(events
            .iter()
            .filter(|e| e.event.position_id() == position_id)
            .map(|e| e.event.clone())
            .collect())
    }

    async fn find_by_position_after(
        &self,
        position_id: PositionId,
        after_seq: i64,
    ) -> Result<Vec<Event>, StoreError> {
        let events = self.events.read().unwrap();
        Ok(events
            .iter()
            .filter(|e| e.event.position_id() == position_id && e.seq > after_seq)
            .map(|e| e.event.clone())
            .collect())
    }

    async fn get_latest_seq(&self, position_id: PositionId) -> Result<Option<i64>, StoreError> {
        let events = self.events.read().unwrap();
        Ok(events
            .iter()
            .filter(|e| e.event.position_id() == position_id)
            .map(|e| e.seq)
            .max())
    }
}

// =============================================================================
// Store Implementation
// =============================================================================

#[async_trait]
impl Store for MemoryStore {
    fn positions(&self) -> &dyn PositionRepository {
        self
    }

    fn orders(&self) -> &dyn OrderRepository {
        self
    }

    fn events(&self) -> &dyn EventRepository {
        self
    }

    /// Override to apply events to in-memory projection synchronously.
    fn apply_event(&self, event: &robson_domain::Event) -> Result<(), StoreError> {
        self.apply_event_internal(event)
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repository::{EventRepository, OrderRepository, PositionRepository};
    use chrono::Utc;
    use robson_domain::{OrderSide, Quantity, Side, Symbol};
    use rust_decimal_macros::dec;

    fn create_test_position() -> Position {
        Position::new(Uuid::now_v7(), Symbol::from_pair("BTCUSDT").unwrap(), Side::Long)
    }

    fn create_test_order(position_id: PositionId) -> Order {
        Order::new_market(
            position_id,
            Symbol::from_pair("BTCUSDT").unwrap(),
            OrderSide::Buy,
            Quantity::new(dec!(0.1)).unwrap(),
        )
    }

    fn create_test_event(position_id: PositionId) -> Event {
        Event::PositionArmed {
            position_id,
            account_id: Uuid::now_v7(),
            symbol: Symbol::from_pair("BTCUSDT").unwrap(),
            side: Side::Long,
            timestamp: Utc::now(),
        }
    }

    // Position Repository Tests
    #[tokio::test]
    async fn test_position_save_and_find() {
        let store = MemoryStore::new();
        let position = create_test_position();
        let id = position.id;

        PositionRepository::save(&store, &position).await.unwrap();

        let found = PositionRepository::find_by_id(&store, id).await.unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().id, id);
    }

    #[tokio::test]
    async fn test_position_find_active() {
        let store = MemoryStore::new();

        // Create armed position (active)
        let armed = create_test_position();
        PositionRepository::save(&store, &armed).await.unwrap();

        let active = PositionRepository::find_active(&store).await.unwrap();
        assert_eq!(active.len(), 1);
    }

    #[tokio::test]
    async fn test_position_find_by_account() {
        let store = MemoryStore::new();
        let account_id = Uuid::now_v7();

        let mut pos1 = create_test_position();
        pos1.account_id = account_id;
        PositionRepository::save(&store, &pos1).await.unwrap();

        let mut pos2 = create_test_position();
        pos2.account_id = account_id;
        PositionRepository::save(&store, &pos2).await.unwrap();

        // Different account
        let pos3 = create_test_position();
        PositionRepository::save(&store, &pos3).await.unwrap();

        let found = PositionRepository::find_by_account(&store, account_id).await.unwrap();
        assert_eq!(found.len(), 2);
    }

    #[tokio::test]
    async fn test_position_delete() {
        let store = MemoryStore::new();
        let position = create_test_position();
        let id = position.id;

        PositionRepository::save(&store, &position).await.unwrap();
        assert_eq!(store.position_count(), 1);

        PositionRepository::delete(&store, id).await.unwrap();
        assert_eq!(store.position_count(), 0);

        let found = PositionRepository::find_by_id(&store, id).await.unwrap();
        assert!(found.is_none());
    }

    #[tokio::test]
    async fn test_position_delete_not_found() {
        let store = MemoryStore::new();
        let result = PositionRepository::delete(&store, Uuid::now_v7()).await;
        assert!(result.is_err());
    }

    // Order Repository Tests
    #[tokio::test]
    async fn test_order_save_and_find() {
        let store = MemoryStore::new();
        let position = create_test_position();
        let order = create_test_order(position.id);
        let id = order.id;

        OrderRepository::save(&store, &order).await.unwrap();

        let found = OrderRepository::find_by_id(&store, id).await.unwrap();
        assert!(found.is_some());
    }

    #[tokio::test]
    async fn test_order_find_by_position() {
        let store = MemoryStore::new();
        let position_id = Uuid::now_v7();

        let order1 = create_test_order(position_id);
        let order2 = create_test_order(position_id);
        let order3 = create_test_order(Uuid::now_v7()); // Different position

        OrderRepository::save(&store, &order1).await.unwrap();
        OrderRepository::save(&store, &order2).await.unwrap();
        OrderRepository::save(&store, &order3).await.unwrap();

        let found = OrderRepository::find_by_position(&store, position_id).await.unwrap();
        assert_eq!(found.len(), 2);
    }

    #[tokio::test]
    async fn test_order_find_by_client_id() {
        let store = MemoryStore::new();
        let order = create_test_order(Uuid::now_v7());
        let client_id = order.client_order_id.clone();

        OrderRepository::save(&store, &order).await.unwrap();

        let found = OrderRepository::find_by_client_id(&store, &client_id).await.unwrap();
        assert!(found.is_some());
    }

    #[tokio::test]
    async fn test_order_find_pending() {
        let store = MemoryStore::new();

        let pending = create_test_order(Uuid::now_v7());
        OrderRepository::save(&store, &pending).await.unwrap();

        let found = OrderRepository::find_pending(&store).await.unwrap();
        assert_eq!(found.len(), 1);
    }

    // Event Repository Tests
    #[tokio::test]
    async fn test_event_append() {
        let store = MemoryStore::new();
        let position_id = Uuid::now_v7();
        let event = create_test_event(position_id);

        let seq = EventRepository::append(&store, &event).await.unwrap();
        assert_eq!(seq, 1);

        let seq2 = EventRepository::append(&store, &event).await.unwrap();
        assert_eq!(seq2, 2);
    }

    #[tokio::test]
    async fn test_event_find_by_position() {
        let store = MemoryStore::new();
        let position_id = Uuid::now_v7();

        let event1 = create_test_event(position_id);
        let event2 = create_test_event(position_id);
        let event3 = create_test_event(Uuid::now_v7()); // Different position

        EventRepository::append(&store, &event1).await.unwrap();
        EventRepository::append(&store, &event2).await.unwrap();
        EventRepository::append(&store, &event3).await.unwrap();

        let found = EventRepository::find_by_position(&store, position_id).await.unwrap();
        assert_eq!(found.len(), 2);
    }

    #[tokio::test]
    async fn test_event_find_after_seq() {
        let store = MemoryStore::new();
        let position_id = Uuid::now_v7();

        let event = create_test_event(position_id);
        EventRepository::append(&store, &event).await.unwrap(); // seq 1
        EventRepository::append(&store, &event).await.unwrap(); // seq 2
        EventRepository::append(&store, &event).await.unwrap(); // seq 3

        let found = EventRepository::find_by_position_after(&store, position_id, 1).await.unwrap();
        assert_eq!(found.len(), 2); // seq 2 and 3
    }

    #[tokio::test]
    async fn test_event_get_latest_seq() {
        let store = MemoryStore::new();
        let position_id = Uuid::now_v7();

        let seq = EventRepository::get_latest_seq(&store, position_id).await.unwrap();
        assert!(seq.is_none());

        let event = create_test_event(position_id);
        EventRepository::append(&store, &event).await.unwrap();
        EventRepository::append(&store, &event).await.unwrap();
        EventRepository::append(&store, &event).await.unwrap();

        let seq = EventRepository::get_latest_seq(&store, position_id).await.unwrap();
        assert_eq!(seq, Some(3));
    }

    // Store Tests
    #[tokio::test]
    async fn test_store_clear() {
        let store = MemoryStore::new();

        let position = create_test_position();
        PositionRepository::save(&store, &position).await.unwrap();

        let order = create_test_order(position.id);
        OrderRepository::save(&store, &order).await.unwrap();

        let event = create_test_event(position.id);
        EventRepository::append(&store, &event).await.unwrap();

        assert_eq!(store.position_count(), 1);
        assert_eq!(store.order_count(), 1);
        assert_eq!(store.event_count(), 1);

        store.clear();

        assert_eq!(store.position_count(), 0);
        assert_eq!(store.order_count(), 0);
        assert_eq!(store.event_count(), 0);
    }
}
