//! Intent journal for idempotent execution.
//!
//! The intent journal ensures that each action is executed exactly once,
//! even if the system crashes and restarts mid-execution.
//!
//! # Flow
//!
//! 1. Record intent (before execution)
//! 2. Execute action (place order, etc.)
//! 3. Complete intent (with result)
//!
//! On restart, incomplete intents are either retried or marked failed.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::RwLock;
use uuid::Uuid;

use robson_domain::{ExitReason, OrderSide, PositionId, Quantity, Symbol};

use crate::error::{ExecError, ExecResult};
use crate::ports::OrderResult;

// =============================================================================
// Intent Types
// =============================================================================

/// An intent represents a planned action before execution.
///
/// Intents are recorded before execution and completed after.
/// This ensures idempotent execution even across crashes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Intent {
    /// Unique intent identifier (often same as signal_id)
    pub id: Uuid,
    /// Position this intent belongs to
    pub position_id: PositionId,
    /// The action to execute
    pub action: IntentAction,
    /// Current status
    pub status: IntentStatus,
    /// When the intent was created
    pub created_at: DateTime<Utc>,
    /// When the intent was completed (if completed)
    pub completed_at: Option<DateTime<Utc>>,
    /// Result of execution (if completed)
    pub result: Option<IntentResult>,
}

impl Intent {
    /// Create a new pending intent.
    pub fn new(id: Uuid, position_id: PositionId, action: IntentAction) -> Self {
        Self {
            id,
            position_id,
            action,
            status: IntentStatus::Pending,
            created_at: Utc::now(),
            completed_at: None,
            result: None,
        }
    }

    /// Check if intent is still pending.
    pub fn is_pending(&self) -> bool {
        matches!(self.status, IntentStatus::Pending)
    }

    /// Check if intent was successful.
    pub fn is_success(&self) -> bool {
        matches!(self.status, IntentStatus::Completed)
            && matches!(self.result, Some(IntentResult::Success(_)))
    }
}

/// Actions that can be recorded as intents.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum IntentAction {
    /// Place entry order
    PlaceEntryOrder {
        symbol: Symbol,
        side: OrderSide,
        quantity: Quantity,
    },

    /// Place exit order
    PlaceExitOrder {
        symbol: Symbol,
        side: OrderSide,
        quantity: Quantity,
        reason: ExitReason,
    },

    /// Cancel an order
    CancelOrder {
        symbol: Symbol,
        order_id: String,
    },
}

/// Status of an intent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum IntentStatus {
    /// Intent recorded, not yet executed
    Pending,
    /// Execution in progress
    Executing,
    /// Execution completed (check result for success/failure)
    Completed,
}

/// Result of intent execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum IntentResult {
    /// Execution succeeded
    Success(OrderResult),
    /// Execution failed
    Failed(String),
    /// Intent was skipped (e.g., duplicate)
    Skipped(String),
}

// =============================================================================
// Intent Journal
// =============================================================================

/// Journal for tracking intents and ensuring idempotent execution.
///
/// In production, this would be backed by PostgreSQL.
/// For now, we use an in-memory implementation.
pub struct IntentJournal {
    intents: RwLock<HashMap<Uuid, Intent>>,
}

impl IntentJournal {
    /// Create a new intent journal.
    pub fn new() -> Self {
        Self {
            intents: RwLock::new(HashMap::new()),
        }
    }

    /// Record a new intent before execution.
    ///
    /// Returns error if intent with same ID already exists.
    pub fn record(&self, intent: Intent) -> ExecResult<()> {
        let mut intents = self.intents.write().map_err(|e| {
            ExecError::IntentJournal(format!("Failed to acquire write lock: {}", e))
        })?;

        if intents.contains_key(&intent.id) {
            return Err(ExecError::AlreadyProcessed(intent.id));
        }

        intents.insert(intent.id, intent);
        Ok(())
    }

    /// Check if intent exists and get its current state.
    pub fn get(&self, intent_id: Uuid) -> ExecResult<Option<Intent>> {
        let intents = self.intents.read().map_err(|e| {
            ExecError::IntentJournal(format!("Failed to acquire read lock: {}", e))
        })?;

        Ok(intents.get(&intent_id).cloned())
    }

    /// Check if intent was already processed (completed or executing).
    pub fn is_processed(&self, intent_id: Uuid) -> ExecResult<bool> {
        let intents = self.intents.read().map_err(|e| {
            ExecError::IntentJournal(format!("Failed to acquire read lock: {}", e))
        })?;

        Ok(intents
            .get(&intent_id)
            .map(|i| !i.is_pending())
            .unwrap_or(false))
    }

    /// Mark intent as executing.
    pub fn mark_executing(&self, intent_id: Uuid) -> ExecResult<()> {
        let mut intents = self.intents.write().map_err(|e| {
            ExecError::IntentJournal(format!("Failed to acquire write lock: {}", e))
        })?;

        let intent = intents.get_mut(&intent_id).ok_or_else(|| {
            ExecError::IntentJournal(format!("Intent not found: {}", intent_id))
        })?;

        intent.status = IntentStatus::Executing;
        Ok(())
    }

    /// Complete an intent with result.
    pub fn complete(&self, intent_id: Uuid, result: IntentResult) -> ExecResult<()> {
        let mut intents = self.intents.write().map_err(|e| {
            ExecError::IntentJournal(format!("Failed to acquire write lock: {}", e))
        })?;

        let intent = intents.get_mut(&intent_id).ok_or_else(|| {
            ExecError::IntentJournal(format!("Intent not found: {}", intent_id))
        })?;

        intent.status = IntentStatus::Completed;
        intent.completed_at = Some(Utc::now());
        intent.result = Some(result);

        Ok(())
    }

    /// Get all pending intents (for recovery on restart).
    pub fn get_pending(&self) -> ExecResult<Vec<Intent>> {
        let intents = self.intents.read().map_err(|e| {
            ExecError::IntentJournal(format!("Failed to acquire read lock: {}", e))
        })?;

        Ok(intents.values().filter(|i| i.is_pending()).cloned().collect())
    }

    /// Get intents for a specific position.
    pub fn get_by_position(&self, position_id: PositionId) -> ExecResult<Vec<Intent>> {
        let intents = self.intents.read().map_err(|e| {
            ExecError::IntentJournal(format!("Failed to acquire read lock: {}", e))
        })?;

        Ok(intents
            .values()
            .filter(|i| i.position_id == position_id)
            .cloned()
            .collect())
    }

    /// Clear all intents (for testing).
    #[cfg(test)]
    pub fn clear(&self) {
        let mut intents = self.intents.write().unwrap();
        intents.clear();
    }
}

impl Default for IntentJournal {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    fn create_test_intent() -> Intent {
        Intent::new(
            Uuid::now_v7(),
            Uuid::now_v7(),
            IntentAction::PlaceEntryOrder {
                symbol: robson_domain::Symbol::from_pair("BTCUSDT").unwrap(),
                side: OrderSide::Buy,
                quantity: Quantity::new(dec!(0.1)).unwrap(),
            },
        )
    }

    #[test]
    fn test_record_and_get() {
        let journal = IntentJournal::new();
        let intent = create_test_intent();
        let id = intent.id;

        journal.record(intent.clone()).unwrap();

        let retrieved = journal.get(id).unwrap().unwrap();
        assert_eq!(retrieved.id, id);
        assert!(retrieved.is_pending());
    }

    #[test]
    fn test_duplicate_intent_rejected() {
        let journal = IntentJournal::new();
        let intent = create_test_intent();
        let id = intent.id;

        journal.record(intent.clone()).unwrap();

        // Second record should fail
        let result = journal.record(Intent::new(
            id, // Same ID
            Uuid::now_v7(),
            IntentAction::PlaceEntryOrder {
                symbol: robson_domain::Symbol::from_pair("ETHUSDT").unwrap(),
                side: OrderSide::Buy,
                quantity: Quantity::new(dec!(1.0)).unwrap(),
            },
        ));

        assert!(matches!(result, Err(ExecError::AlreadyProcessed(_))));
    }

    #[test]
    fn test_complete_intent() {
        let journal = IntentJournal::new();
        let intent = create_test_intent();
        let id = intent.id;

        journal.record(intent).unwrap();
        journal.mark_executing(id).unwrap();

        let order_result = OrderResult {
            exchange_order_id: "12345".to_string(),
            client_order_id: id.to_string(),
            fill_price: robson_domain::Price::new(dec!(95000)).unwrap(),
            filled_quantity: robson_domain::Quantity::new(dec!(0.1)).unwrap(),
            fee: dec!(0.001),
            fee_asset: "BNB".to_string(),
            filled_at: Utc::now(),
        };

        journal
            .complete(id, IntentResult::Success(order_result))
            .unwrap();

        let completed = journal.get(id).unwrap().unwrap();
        assert!(!completed.is_pending());
        assert!(completed.is_success());
        assert!(completed.completed_at.is_some());
    }

    #[test]
    fn test_is_processed() {
        let journal = IntentJournal::new();
        let intent = create_test_intent();
        let id = intent.id;

        // Not recorded yet
        assert!(!journal.is_processed(id).unwrap());

        // Recorded but pending
        journal.record(intent).unwrap();
        assert!(!journal.is_processed(id).unwrap());

        // Executing
        journal.mark_executing(id).unwrap();
        assert!(journal.is_processed(id).unwrap());
    }

    #[test]
    fn test_get_pending() {
        let journal = IntentJournal::new();

        let intent1 = create_test_intent();
        let intent2 = create_test_intent();
        let id1 = intent1.id;

        journal.record(intent1).unwrap();
        journal.record(intent2).unwrap();

        // Both pending
        assert_eq!(journal.get_pending().unwrap().len(), 2);

        // Complete one
        journal.mark_executing(id1).unwrap();
        journal
            .complete(id1, IntentResult::Skipped("test".to_string()))
            .unwrap();

        // Only one pending
        assert_eq!(journal.get_pending().unwrap().len(), 1);
    }
}
