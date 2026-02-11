//! Repository for safety net detected positions.
//!
//! Provides persistence for rogue positions detected by the safety net,
//! allowing the position monitor to recover its state after restarts.

use crate::error::StoreError;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use robson_domain::{DetectedPosition, Price, Quantity, Side, Symbol};
use rust_decimal::Decimal;
use std::collections::HashMap;
use uuid::Uuid;

// =============================================================================
// DTO (Data Transfer Object) for serialization
// =============================================================================

/// DTO for serializing DetectedPosition to/from database.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DetectedPositionDto {
    /// Composite ID: "{symbol}:{side}" e.g., "BTCUSDT:long"
    pub position_id: String,
    /// Binance-assigned position ID
    pub binance_position_id: String,
    pub symbol: String,
    pub side: String,
    pub entry_price: Decimal,
    pub quantity: Decimal,
    pub stop_price: Decimal,
    pub stop_distance: Decimal,
    pub stop_distance_pct: Decimal,
    pub detected_at: DateTime<Utc>,
    pub last_verified_at: DateTime<Utc>,
    pub closed_at: Option<DateTime<Utc>>,
    pub is_active: bool,
    pub last_execution_attempt_at: Option<DateTime<Utc>>,
    pub consecutive_failures: i32,
    pub is_panic_mode: bool,
    pub last_error: Option<String>,
    pub stop_method: String,
}

impl DetectedPositionDto {
    /// Convert from domain DetectedPosition to DTO.
    pub fn from_domain(pos: &DetectedPosition) -> Self {
        let position_id = format!("{}:{}", pos.symbol.as_pair(),
            if pos.side == Side::Long { "long" } else { "short" });

        Self {
            position_id,
            binance_position_id: pos.binance_position_id.clone(),
            symbol: pos.symbol.as_pair(),
            side: format!("{:?}", pos.side).to_lowercase(),
            entry_price: pos.entry_price.as_decimal(),
            quantity: pos.quantity.as_decimal(),
            stop_price: pos.calculated_stop.as_ref().map(|s| s.stop_price.as_decimal()).unwrap_or_else(|| {
                // Calculate if not set (shouldn't happen with proper flow)
                match pos.side {
                    Side::Long => pos.entry_price.as_decimal() * Decimal::from(98u32) / Decimal::from(100u32),
                    Side::Short => pos.entry_price.as_decimal() * Decimal::from(102u32) / Decimal::from(100u32),
                }
            }),
            stop_distance: pos.calculated_stop.as_ref().map(|s| s.distance).unwrap_or_else(|| {
                match pos.side {
                    Side::Long => pos.entry_price.as_decimal() - (pos.entry_price.as_decimal() * Decimal::from(98u32) / Decimal::from(100u32)),
                    Side::Short => (pos.entry_price.as_decimal() * Decimal::from(102u32) / Decimal::from(100u32)) - pos.entry_price.as_decimal(),
                }
            }),
            stop_distance_pct: pos.calculated_stop.as_ref().map(|s| s.distance_pct).unwrap_or_else(|| Decimal::from(2)),
            detected_at: pos.detected_at,
            last_verified_at: pos.last_verified_at,
            closed_at: None, // Not tracked in domain, managed by repository
            is_active: true,
            last_execution_attempt_at: None,
            consecutive_failures: 0,
            is_panic_mode: false,
            last_error: None,
            stop_method: "fixed_2pct".to_string(),
        }
    }

    /// Convert from DTO to domain DetectedPosition.
    pub fn to_domain(&self) -> Result<DetectedPosition, StoreError> {
        let symbol = Symbol::from_pair(&self.symbol)
            .map_err(|e| StoreError::Deserialization(format!("Invalid symbol {}: {}", self.symbol, e)))?;

        let side = match self.side.as_str() {
            "long" => Side::Long,
            "short" => Side::Short,
            _ => return Err(StoreError::Deserialization(format!("Invalid side: {}", self.side))),
        };

        let entry_price = Price::new(self.entry_price)
            .map_err(|e| StoreError::Deserialization(format!("Invalid entry_price {}: {}", self.entry_price, e)))?;

        let quantity = Quantity::new(self.quantity)
            .map_err(|e| StoreError::Deserialization(format!("Invalid quantity {}: {}", self.quantity, e)))?;

        let mut pos = DetectedPosition::new(
            self.binance_position_id.clone(),
            symbol,
            side,
            entry_price,
            quantity,
        );
        pos.detected_at = self.detected_at;
        pos.last_verified_at = self.last_verified_at;

        // Re-calculate the stop (idempotent)
        if self.is_active {
            pos.calculate_safety_stop();
        }

        Ok(pos)
    }
}

// =============================================================================
// Execution Log DTO
// =============================================================================

/// DTO for safety net execution log entries.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SafetyExecutionDto {
    pub execution_id: Uuid,
    pub position_id: String,
    pub symbol: String,
    pub side: String,
    pub exit_side: String,
    pub quantity: Decimal,
    pub expected_stop_price: Decimal,
    pub execution_price: Option<Decimal>,
    pub exchange_order_id: Option<String>,
    pub status: String,
    pub error_message: Option<String>,
    pub attempt_number: i32,
    pub is_retry: bool,
    pub attempted_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

// =============================================================================
// Repository Trait
// =============================================================================

/// Repository for detected positions (safety net).
#[async_trait]
pub trait DetectedPositionRepository: Send + Sync {
    /// Save or update a detected position.
    async fn save(&self, position: &DetectedPosition) -> Result<(), StoreError>;

    /// Find a detected position by ID.
    async fn find_by_id(&self, id: &str) -> Result<Option<DetectedPosition>, StoreError>;

    /// Find all active detected positions.
    async fn find_active(&self) -> Result<Vec<DetectedPosition>, StoreError>;

    /// Find detected positions for a specific symbol.
    async fn find_by_symbol(&self, symbol: &str) -> Result<Vec<DetectedPosition>, StoreError>;

    /// Mark a position as closed.
    async fn mark_closed(&self, id: &str, closed_at: DateTime<Utc>) -> Result<(), StoreError>;

    /// Update execution attempt tracking.
    async fn update_execution_attempt(
        &self,
        id: &str,
        attempted_at: DateTime<Utc>,
        failures: i32,
        is_panic: bool,
        error: Option<String>,
    ) -> Result<(), StoreError>;

    /// Clear execution attempts (after successful execution).
    async fn clear_execution_attempts(&self, id: &str) -> Result<(), StoreError>;

    /// Log a safety net execution.
    async fn log_execution(&self, execution: &SafetyExecutionDto) -> Result<(), StoreError>;

    /// Get execution history for a position.
    async fn get_executions(&self, position_id: &str) -> Result<Vec<SafetyExecutionDto>, StoreError>;

    /// Find positions in panic mode.
    async fn find_panic_mode(&self) -> Result<Vec<DetectedPosition>, StoreError>;

    /// Delete old closed positions (cleanup).
    async fn cleanup_old_positions(&self, older_than: DateTime<Utc>) -> Result<u64, StoreError>;
}

// =============================================================================
// In-Memory Implementation
// =============================================================================

/// In-memory implementation of DetectedPositionRepository for testing.
pub struct MemoryDetectedPositionRepository {
    positions: tokio::sync::RwLock<HashMap<String, DetectedPositionDto>>,
    executions: tokio::sync::RwLock<Vec<SafetyExecutionDto>>,
}

impl MemoryDetectedPositionRepository {
    /// Create a new in-memory repository.
    pub fn new() -> Self {
        Self {
            positions: tokio::sync::RwLock::new(HashMap::new()),
            executions: tokio::sync::RwLock::new(Vec::new()),
        }
    }
}

impl Default for MemoryDetectedPositionRepository {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl DetectedPositionRepository for MemoryDetectedPositionRepository {
    async fn save(&self, position: &DetectedPosition) -> Result<(), StoreError> {
        let dto = DetectedPositionDto::from_domain(position);
        let mut positions = self.positions.write().await;
        positions.insert(dto.position_id.clone(), dto);
        Ok(())
    }

    async fn find_by_id(&self, id: &str) -> Result<Option<DetectedPosition>, StoreError> {
        let positions = self.positions.read().await;
        Ok(positions.get(id).and_then(|dto| dto.to_domain().ok()))
    }

    async fn find_active(&self) -> Result<Vec<DetectedPosition>, StoreError> {
        let positions = self.positions.read().await;
        let mut result = Vec::new();
        for dto in positions.values() {
            if dto.is_active {
                if let Ok(pos) = dto.to_domain() {
                    result.push(pos);
                }
            }
        }
        Ok(result)
    }

    async fn find_by_symbol(&self, symbol: &str) -> Result<Vec<DetectedPosition>, StoreError> {
        let positions = self.positions.read().await;
        let mut result = Vec::new();
        for dto in positions.values().filter(|d| d.symbol == symbol) {
            if let Ok(pos) = dto.to_domain() {
                result.push(pos);
            }
        }
        Ok(result)
    }

    async fn mark_closed(&self, id: &str, closed_at: DateTime<Utc>) -> Result<(), StoreError> {
        let mut positions = self.positions.write().await;
        if let Some(dto) = positions.get_mut(id) {
            dto.is_active = false;
            dto.closed_at = Some(closed_at);
        }
        Ok(())
    }

    async fn update_execution_attempt(
        &self,
        id: &str,
        attempted_at: DateTime<Utc>,
        failures: i32,
        is_panic: bool,
        error: Option<String>,
    ) -> Result<(), StoreError> {
        let mut positions = self.positions.write().await;
        if let Some(dto) = positions.get_mut(id) {
            dto.last_execution_attempt_at = Some(attempted_at);
            dto.consecutive_failures = failures;
            dto.is_panic_mode = is_panic;
            dto.last_error = error;
        }
        Ok(())
    }

    async fn clear_execution_attempts(&self, id: &str) -> Result<(), StoreError> {
        let mut positions = self.positions.write().await;
        if let Some(dto) = positions.get_mut(id) {
            dto.last_execution_attempt_at = None;
            dto.consecutive_failures = 0;
            dto.is_panic_mode = false;
            dto.last_error = None;
        }
        Ok(())
    }

    async fn log_execution(&self, execution: &SafetyExecutionDto) -> Result<(), StoreError> {
        let mut executions = self.executions.write().await;
        executions.push(execution.clone());
        Ok(())
    }

    async fn get_executions(&self, position_id: &str) -> Result<Vec<SafetyExecutionDto>, StoreError> {
        let executions = self.executions.read().await;
        Ok(executions.iter().filter(|e| e.position_id == position_id).cloned().collect())
    }

    async fn find_panic_mode(&self) -> Result<Vec<DetectedPosition>, StoreError> {
        let positions = self.positions.read().await;
        let mut result = Vec::new();
        for dto in positions.values().filter(|d| d.is_panic_mode && d.is_active) {
            if let Ok(pos) = dto.to_domain() {
                result.push(pos);
            }
        }
        Ok(result)
    }

    async fn cleanup_old_positions(&self, older_than: DateTime<Utc>) -> Result<u64, StoreError> {
        let mut positions = self.positions.write().await;
        let before = positions.len();
        positions.retain(|_, dto| {
            if let Some(closed_at) = dto.closed_at {
                closed_at > older_than
            } else {
                true // Keep active positions
            }
        });
        Ok((before - positions.len()) as u64)
    }
}

// =============================================================================
// PostgreSQL Implementation
// =============================================================================

/// PostgreSQL implementation of DetectedPositionRepository.
#[cfg(feature = "postgres")]
pub struct PgDetectedPositionRepository {
    pool: Arc<sqlx::PgPool>,
}

#[cfg(feature = "postgres")]
impl PgDetectedPositionRepository {
    /// Create a new PostgreSQL repository.
    pub fn new(pool: Arc<sqlx::PgPool>) -> Self {
        Self { pool }
    }
}

#[cfg(feature = "postgres")]
#[async_trait]
impl DetectedPositionRepository for PgDetectedPositionRepository {
    async fn save(&self, position: &DetectedPosition) -> Result<(), StoreError> {
        let dto = DetectedPositionDto::from_domain(position);

        sqlx::query(
            r#"
            INSERT INTO detected_positions (
                position_id, symbol, side, entry_price, quantity,
                stop_price, stop_distance, stop_distance_pct,
                detected_at, verified_at, closed_at, is_active,
                stop_method
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
            ON CONFLICT (position_id) DO UPDATE SET
                verified_at = EXCLUDED.verified_at,
                closed_at = EXCLUDED.closed_at,
                is_active = EXCLUDED.is_active
            "#
        )
        .bind(&dto.position_id)
        .bind(&dto.symbol)
        .bind(&dto.side)
        .bind(dto.entry_price)
        .bind(dto.quantity)
        .bind(dto.stop_price)
        .bind(dto.stop_distance)
        .bind(dto.stop_distance_pct)
        .bind(dto.detected_at)
        .bind(dto.verified_at)
        .bind(dto.closed_at)
        .bind(dto.is_active)
        .bind(&dto.stop_method)
        .execute(&*self.pool)
        .await?;

        Ok(())
    }

    async fn find_by_id(&self, id: &str) -> Result<Option<DetectedPosition>, StoreError> {
        let row = sqlx::query_as::<_, DetectedPositionDto>(
            "SELECT * FROM detected_positions WHERE position_id = $1"
        )
        .bind(id)
        .fetch_optional(&*self.pool)
        .await?;

        Ok(row.and_then(|dto| dto.to_domain().ok()))
    }

    async fn find_active(&self) -> Result<Vec<DetectedPosition>, StoreError> {
        let rows = sqlx::query_as::<_, DetectedPositionDto>(
            "SELECT * FROM detected_positions WHERE is_active = TRUE ORDER BY detected_at ASC"
        )
        .fetch_all(&*self.pool)
        .await?;

        let mut result = Vec::new();
        for dto in rows {
            if let Ok(pos) = dto.to_domain() {
                result.push(pos);
            }
        }
        Ok(result)
    }

    async fn find_by_symbol(&self, symbol: &str) -> Result<Vec<DetectedPosition>, StoreError> {
        let rows = sqlx::query_as::<_, DetectedPositionDto>(
            "SELECT * FROM detected_positions WHERE symbol = $1 ORDER BY detected_at DESC"
        )
        .bind(symbol)
        .fetch_all(&*self.pool)
        .await?;

        let mut result = Vec::new();
        for dto in rows {
            if let Ok(pos) = dto.to_domain() {
                result.push(pos);
            }
        }
        Ok(result)
    }

    async fn mark_closed(&self, id: &str, closed_at: DateTime<Utc>) -> Result<(), StoreError> {
        sqlx::query(
            "UPDATE detected_positions SET is_active = FALSE, closed_at = $1 WHERE position_id = $2"
        )
        .bind(closed_at)
        .bind(id)
        .execute(&*self.pool)
        .await?;

        Ok(())
    }

    async fn update_execution_attempt(
        &self,
        id: &str,
        attempted_at: DateTime<Utc>,
        failures: i32,
        is_panic: bool,
        error: Option<String>,
    ) -> Result<(), StoreError> {
        sqlx::query(
            r#"
            UPDATE detected_positions
            SET last_execution_attempt_at = $1,
                consecutive_failures = $2,
                is_panic_mode = $3,
                last_error = $4
            WHERE position_id = $5
            "#
        )
        .bind(attempted_at)
        .bind(failures)
        .bind(is_panic)
        .bind(&error)
        .bind(id)
        .execute(&*self.pool)
        .await?;

        Ok(())
    }

    async fn clear_execution_attempts(&self, id: &str) -> Result<(), StoreError> {
        sqlx::query(
            r#"
            UPDATE detected_positions
            SET last_execution_attempt_at = NULL,
                consecutive_failures = 0,
                is_panic_mode = FALSE,
                last_error = NULL
            WHERE position_id = $1
            "#
        )
        .bind(id)
        .execute(&*self.pool)
        .await?;

        Ok(())
    }

    async fn log_execution(&self, execution: &SafetyExecutionDto) -> Result<(), StoreError> {
        sqlx::query(
            r#"
            INSERT INTO safety_net_executions (
                execution_id, position_id, symbol, side, exit_side,
                quantity, expected_stop_price, execution_price,
                exchange_order_id, status, error_message,
                attempt_number, is_retry, attempted_at, completed_at
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)
            "#
        )
        .bind(execution.execution_id)
        .bind(&execution.position_id)
        .bind(&execution.symbol)
        .bind(&execution.side)
        .bind(&execution.exit_side)
        .bind(execution.quantity)
        .bind(execution.expected_stop_price)
        .bind(execution.execution_price)
        .bind(&execution.exchange_order_id)
        .bind(&execution.status)
        .bind(&execution.error_message)
        .bind(execution.attempt_number)
        .bind(execution.is_retry)
        .bind(execution.attempted_at)
        .bind(execution.completed_at)
        .execute(&*self.pool)
        .await?;

        Ok(())
    }

    async fn get_executions(&self, position_id: &str) -> Result<Vec<SafetyExecutionDto>, StoreError> {
        let executions = sqlx::query_as::<_, SafetyExecutionDto>(
            "SELECT * FROM safety_net_executions WHERE position_id = $1 ORDER BY attempted_at DESC"
        )
        .bind(position_id)
        .fetch_all(&*self.pool)
        .await?;

        Ok(executions)
    }

    async fn find_panic_mode(&self) -> Result<Vec<DetectedPosition>, StoreError> {
        let rows = sqlx::query_as::<_, DetectedPositionDto>(
            "SELECT * FROM detected_positions WHERE is_panic_mode = TRUE AND is_active = TRUE"
        )
        .fetch_all(&*self.pool)
        .await?;

        let mut result = Vec::new();
        for dto in rows {
            if let Ok(pos) = dto.to_domain() {
                result.push(pos);
            }
        }
        Ok(result)
    }

    async fn cleanup_old_positions(&self, older_than: DateTime<Utc>) -> Result<u64, StoreError> {
        let result = sqlx::query(
            "DELETE FROM detected_positions WHERE closed_at < $1"
        )
        .bind(older_than)
        .execute(&*self.pool)
        .await?;

        Ok(result.rows_affected())
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn create_test_position() -> DetectedPosition {
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let entry_price = Price::new(rust_decimal_macros::dec!(95000)).unwrap();
        let quantity = Quantity::new(rust_decimal_macros::dec!(0.1)).unwrap();
        DetectedPosition::new("binance_123".to_string(), symbol, Side::Long, entry_price, quantity)
    }

    #[tokio::test]
    async fn test_memory_repository_save_and_find() {
        let repo = MemoryDetectedPositionRepository::new();
        let pos = create_test_position();

        repo.save(&pos).await.unwrap();

        // Position ID is constructed as "{symbol}:{side}"
        let found = repo.find_by_id("BTCUSDT:long").await.unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().binance_position_id, "binance_123");
    }

    #[tokio::test]
    async fn test_memory_repository_mark_closed() {
        let repo = MemoryDetectedPositionRepository::new();
        let pos = create_test_position();

        repo.save(&pos).await.unwrap();

        let closed_at = Utc::now();
        repo.mark_closed("BTCUSDT:long", closed_at).await.unwrap();

        let active = repo.find_active().await.unwrap();
        assert!(active.is_empty());

        let found = repo.find_by_id("BTCUSDT:long").await.unwrap();
        assert!(found.is_some());
        // Domain type doesn't have closed_at, but repository tracks it
    }

    #[tokio::test]
    async fn test_memory_repository_execution_tracking() {
        let repo = MemoryDetectedPositionRepository::new();
        let pos = create_test_position();

        repo.save(&pos).await.unwrap();

        let now = Utc::now();
        repo.update_execution_attempt("BTCUSDT:long", now, 2, false, Some("error".to_string()))
            .await.unwrap();

        let active = repo.find_active().await.unwrap();
        assert_eq!(active.len(), 1);

        repo.clear_execution_attempts("BTCUSDT:long").await.unwrap();

        // Should still be active, just cleared tracking
        let active = repo.find_active().await.unwrap();
        assert_eq!(active.len(), 1);
    }

    #[tokio::test]
    async fn test_dto_conversion() {
        let pos = create_test_position();
        let dto = DetectedPositionDto::from_domain(&pos);

        assert_eq!(dto.position_id, "BTCUSDT:long");
        assert_eq!(dto.symbol, "BTCUSDT");
        assert_eq!(dto.side, "long");
        assert_eq!(dto.stop_method, "fixed_2pct");

        let converted = dto.to_domain().unwrap();
        assert_eq!(converted.binance_position_id, pos.binance_position_id);
        assert_eq!(converted.symbol.as_pair(), pos.symbol.as_pair());
    }
}
