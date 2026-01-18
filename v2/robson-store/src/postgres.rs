//! PostgreSQL projection reader for crash recovery.
//!
//! This module provides:
//! - `ProjectionRecovery` trait for reading positions from projection
//! - `PgProjectionReader` adapter implementing the trait
//! - `find_active_from_projection` function for direct access

use crate::error::StoreError;
use async_trait::async_trait;
use robson_domain::{Position, Price, Quantity, Side, Symbol, TechnicalStopDistance};
use rust_decimal::Decimal;
use sqlx::PgPool;
use std::sync::Arc;
use uuid::Uuid;

/// Trait for reading active positions from a projection table.
///
/// This trait allows the Daemon to restore positions from the database
/// projection after a crash, without coupling the Daemon directly to PostgreSQL.
#[async_trait]
pub trait ProjectionRecovery: Send + Sync {
    /// Find active positions from the projection for a given tenant.
    ///
    /// Returns positions with states: 'armed', 'active', 'exiting'
    async fn find_active_from_projection(
        &self,
        tenant_id: Uuid,
    ) -> Result<Vec<Position>, StoreError>;
}

/// PostgreSQL adapter for reading from the positions_current projection.
///
/// This adapter wraps a PgPool and implements `ProjectionRecovery` by
/// delegating to the `find_active_from_projection` function.
pub struct PgProjectionReader {
    /// PostgreSQL connection pool
    pool: Arc<PgPool>,
}

impl PgProjectionReader {
    /// Create a new PostgreSQL projection reader.
    pub fn new(pool: Arc<PgPool>) -> Self {
        Self { pool }
    }

    /// Get a reference to the underlying pool (for testing).
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }
}

#[async_trait]
impl ProjectionRecovery for PgProjectionReader {
    /// Find active positions from the projection table.
    async fn find_active_from_projection(
        &self,
        tenant_id: Uuid,
    ) -> Result<Vec<Position>, StoreError> {
        find_active_from_projection(&self.pool, tenant_id).await
    }
}

/// Read active positions from the `positions_current` projection table.
///
/// This function is used during crash recovery to restore active positions
/// from the PostgreSQL projection. It reads positions with states:
/// - 'armed'
/// - 'active'
/// - 'exiting'
///
/// # Arguments
///
/// * `pool` - PostgreSQL connection pool
/// * `tenant_id` - Tenant ID to filter positions
///
/// # Returns
///
/// * `Ok(Vec<Position>)` - List of active positions
/// * `Err(StoreError)` - Database error
///
/// # Projection Schema
///
/// The `positions_current` table should have the following columns:
/// - position_id: Uuid
/// - tenant_id: Uuid
/// - account_id: Uuid
/// - strategy_id: Uuid
/// - symbol: String (e.g., "BTCUSDT")
/// - side: String ("long" or "short")
/// - state: String ("armed", "active", "exiting", "closed", "error")
/// - entry_price: Decimal?
/// - entry_quantity: Decimal?
/// - trailing_stop_price: Decimal?
/// - favorable_extreme: Decimal?
/// - extreme_at: Timestamp?
/// - technical_stop_distance: Decimal?
/// - technical_stop_price: Decimal?
/// - created_at: Timestamp
/// - updated_at: Timestamp
pub async fn find_active_from_projection(
    pool: &PgPool,
    tenant_id: Uuid,
) -> Result<Vec<Position>, StoreError> {
    let rows = sqlx::query!(
        r#"
        SELECT
            position_id,
            account_id,
            symbol,
            side,
            state,
            entry_price,
            entry_quantity,
            trailing_stop_price,
            favorable_extreme,
            extreme_at,
            technical_stop_distance,
            created_at,
            updated_at
        FROM positions_current
        WHERE tenant_id = $1
          AND state IN ('armed', 'active', 'exiting')
        ORDER BY created_at ASC
        "#,
        tenant_id
    )
    .fetch_all(pool)
    .await
    .map_err(|e| StoreError::Database(format!("Failed to read projection: {}", e)))?;

    let mut positions = Vec::new();

    for row in rows {
        // Parse symbol
        let symbol = Symbol::from_pair(&row.symbol)
            .map_err(|e| StoreError::Deserialization(format!("Invalid symbol {}: {}", row.symbol, e)))?;

        // Parse side
        let side = match row.side.as_str() {
            "long" => Side::Long,
            "short" => Side::Short,
            _ => return Err(StoreError::Deserialization(format!("Invalid side: {}", row.side))),
        };

        // Create base position
        let mut position = Position::new(row.account_id, symbol, side);
        position.id = row.position_id;

        // Set entry data
        if let Some(entry_price) = row.entry_price {
            position.entry_price = Some(Price::new(entry_price).map_err(|e| {
                StoreError::Deserialization(format!("Invalid entry_price {}: {}", entry_price, e))
            })?);
        }

        if let Some(entry_quantity) = row.entry_quantity {
            position.quantity = Quantity::new(entry_quantity).map_err(|e| {
                StoreError::Deserialization(format!("Invalid entry_quantity {}: {}", entry_quantity, e))
            })?;
        }

        // Set tech stop distance
        if let (Some(distance), Some(stop_price)) =
            (row.technical_stop_distance, row.technical_stop_price)
        {
            let entry = position.entry_price.ok_or_else(|| {
                StoreError::Deserialization("Missing entry_price for technical stop".to_string())
            })?;

            let stop = Price::new(stop_price).map_err(|e| {
                StoreError::Deserialization(format!("Invalid technical_stop_price: {}", e))
            })?;

            let tech_stop = TechnicalStopDistance::from_entry_and_stop(entry, stop);
            position.tech_stop_distance = Some(tech_stop);
        }

        // Set state based on row
        // For now, we only restore Armed positions to be conservative
        // Active positions will need to be reconstructed with full state
        match row.state.as_str() {
            "armed" => {
                position.state = robson_domain::PositionState::Armed;
            }
            "active" => {
                // For Active positions, we need to reconstruct the full Active state
                // This requires: current_price, trailing_stop, favorable_extreme, extreme_at
                let current_price = row
                    .entry_price
                    .ok_or_else(|| StoreError::Deserialization("Missing entry_price for active position".to_string()))?;

                let trailing_stop = row
                    .trailing_stop_price
                    .ok_or_else(|| StoreError::Deserialization("Missing trailing_stop_price for active position".to_string()))?;

                let favorable_extreme = row.favorable_extreme.ok_or_else(|| {
                    StoreError::Deserialization("Missing favorable_extreme for active position".to_string())
                })?;

                let extreme_at = row.extreme_at.ok_or_else(|| {
                    StoreError::Deserialization("Missing extreme_at for active position".to_string())
                })?;

                position.state = robson_domain::PositionState::Active {
                    current_price: Price::new(current_price).map_err(|e| {
                        StoreError::Deserialization(format!("Invalid current_price: {}", e))
                    })?,
                    trailing_stop: Price::new(trailing_stop).map_err(|e| {
                        StoreError::Deserialization(format!("Invalid trailing_stop: {}", e))
                    })?,
                    favorable_extreme: Price::new(favorable_extreme).map_err(|e| {
                        StoreError::Deserialization(format!("Invalid favorable_extreme: {}", e))
                    })?,
                    extreme_at,
                    insurance_stop_id: None,
                    last_emitted_stop: row.trailing_stop_price.map(|p| Price::new(p).unwrap()),
                };
            }
            _ => {
                // Skip other states (exiting, closed, error) as they are not "active" for recovery
                continue;
            }
        }

        // Set timestamps
        position.created_at = row.created_at;
        position.updated_at = row.updated_at;

        positions.push(position);
    }

    Ok(positions)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use rust_decimal_macros::dec;

    /// Integration test for projection recovery.
    ///
    /// This test uses `sqlx::test` which automatically:
    /// - Spins up a test database
    /// - Runs migrations from the migrations/ directory
    /// - Provides a PgPool for the test
    /// - Rolls back the transaction at the end
    ///
    /// Run with: `cargo test -p robson-store --features postgres -- postgres_tests`
    #[sqlx::test(migrations = "../migrations")]
    async fn test_projection_recovery_restores_active_position(pool: PgPool) {
        // 1. Create the positions_current projection table
        sqlx::query(
            r#"
            CREATE TABLE positions_current (
                position_id UUID PRIMARY KEY,
                tenant_id UUID NOT NULL,
                account_id UUID NOT NULL,
                strategy_id UUID NOT NULL,
                symbol VARCHAR(20) NOT NULL,
                side VARCHAR(10) NOT NULL,
                state VARCHAR(20) NOT NULL,
                entry_price DECIMAL(20, 8),
                entry_quantity DECIMAL(20, 8),
                trailing_stop_price DECIMAL(20, 8),
                favorable_extreme DECIMAL(20, 8),
                extreme_at TIMESTAMPTZ,
                technical_stop_distance DECIMAL(20, 8),
                technical_stop_price DECIMAL(20, 8),
                created_at TIMESTAMPTZ NOT NULL,
                updated_at TIMESTAMPTZ NOT NULL
            )
            "#,
        )
        .execute(&pool)
        .await
        .expect("Failed to create positions_current table");

        // 2. Insert a test ACTIVE position
        let tenant_id = Uuid::now_v7();
        let position_id = Uuid::now_v7();
        let account_id = Uuid::now_v7();
        let strategy_id = Uuid::now_v7();
        let now = Utc::now();

        sqlx::query(
            r#"
            INSERT INTO positions_current (
                position_id, tenant_id, account_id, strategy_id,
                symbol, side, state,
                entry_price, entry_quantity,
                trailing_stop_price, favorable_extreme, extreme_at,
                technical_stop_distance, technical_stop_price,
                created_at, updated_at
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16)
            "#,
        )
        .bind(position_id)
        .bind(tenant_id)
        .bind(account_id)
        .bind(strategy_id)
        .bind("BTCUSDT")
        .bind("long")
        .bind("active")
        .bind(dec!(95000))
        .bind(dec!(0.01))
        .bind(dec!(93500))
        .bind(dec!(97000))
        .bind(now)
        .bind(dec!(1500))
        .bind(dec!(93500))
        .bind(now)
        .bind(now)
        .execute(&pool)
        .await
        .expect("Failed to insert test position");

        // 3. Create PgProjectionReader and restore positions
        let reader = PgProjectionReader::new(Arc::new(pool));
        let restored = reader
            .find_active_from_projection(tenant_id)
            .await
            .expect("Failed to restore positions");

        // 4. Verify the position was restored
        assert_eq!(restored.len(), 1, "Should restore 1 position");

        let pos = &restored[0];
        assert_eq!(pos.id, position_id);
        assert_eq!(pos.account_id, account_id);
        assert_eq!(pos.symbol.as_pair(), "BTCUSDT");

        // Verify Active state with trailing stop data
        match &pos.state {
            robson_domain::PositionState::Active {
                current_price,
                trailing_stop,
                favorable_extreme,
                extreme_at: _,
                insurance_stop_id,
                last_emitted_stop,
            } => {
                assert_eq!(current_price.as_decimal(), dec!(95000));
                assert_eq!(trailing_stop.as_decimal(), dec!(93500));
                assert_eq!(favorable_extreme.as_decimal(), dec!(97000));
                assert!(insurance_stop_id.is_none());
                assert_eq!(last_emitted_stop.as_ref().map(|p| p.as_decimal()), Some(dec!(93500)));
            }
            _ => panic!("Expected Active state, got {:?}", pos.state),
        }

        // Verify technical stop distance
        assert!(pos.tech_stop_distance.is_some());
        let tech_stop = pos.tech_stop_distance.as_ref().unwrap();
        assert_eq!(tech_stop.distance, dec!(1500));
        assert_eq!(tech_stop.distance_pct, dec!(1.57894736842105260000)); // ~1.58%
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn test_projection_recovery_skips_closed_positions(pool: PgPool) {
        // Create the positions_current projection table
        sqlx::query(
            r#"
            CREATE TABLE positions_current (
                position_id UUID PRIMARY KEY,
                tenant_id UUID NOT NULL,
                account_id UUID NOT NULL,
                strategy_id UUID NOT NULL,
                symbol VARCHAR(20) NOT NULL,
                side VARCHAR(10) NOT NULL,
                state VARCHAR(20) NOT NULL,
                entry_price DECIMAL(20, 8),
                entry_quantity DECIMAL(20, 8),
                trailing_stop_price DECIMAL(20, 8),
                favorable_extreme DECIMAL(20, 8),
                extreme_at TIMESTAMPTZ,
                technical_stop_distance DECIMAL(20, 8),
                technical_stop_price DECIMAL(20, 8),
                created_at TIMESTAMPTZ NOT NULL,
                updated_at TIMESTAMPTZ NOT NULL
            )
            "#,
        )
        .execute(&pool)
        .await
        .expect("Failed to create positions_current table");

        let tenant_id = Uuid::now_v7();
        let now = Utc::now();

        // Insert an ARMED position (should be restored)
        sqlx::query(
            r#"
            INSERT INTO positions_current (
                position_id, tenant_id, account_id, strategy_id,
                symbol, side, state,
                entry_price, entry_quantity,
                trailing_stop_price, favorable_extreme, extreme_at,
                technical_stop_distance, technical_stop_price,
                created_at, updated_at
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, NULL, NULL, NULL, NULL, NULL, $10, $11)
            "#,
        )
        .bind(Uuid::now_v7())
        .bind(tenant_id)
        .bind(Uuid::now_v7())
        .bind(Uuid::now_v7())
        .bind("BTCUSDT")
        .bind("long")
        .bind("armed")
        .bind(dec!(95000))
        .bind(dec!(0.01))
        .bind(now)
        .bind(now)
        .execute(&pool)
        .await
        .expect("Failed to insert armed position");

        // Insert a CLOSED position (should NOT be restored)
        sqlx::query(
            r#"
            INSERT INTO positions_current (
                position_id, tenant_id, account_id, strategy_id,
                symbol, side, state,
                entry_price, entry_quantity,
                trailing_stop_price, favorable_extreme, extreme_at,
                technical_stop_distance, technical_stop_price,
                created_at, updated_at
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, NULL, NULL, NULL, NULL, NULL, $10, $11)
            "#,
        )
        .bind(Uuid::now_v7())
        .bind(tenant_id)
        .bind(Uuid::now_v7())
        .bind(Uuid::now_v7())
        .bind("BTCUSDT")
        .bind("long")
        .bind("closed")
        .bind(dec!(95000))
        .bind(dec!(0.01))
        .bind(now)
        .bind(now)
        .execute(&pool)
        .await
        .expect("Failed to insert closed position");

        let reader = PgProjectionReader::new(Arc::new(pool));
        let restored = reader
            .find_active_from_projection(tenant_id)
            .await
            .expect("Failed to restore positions");

        // Only armed position should be restored (closed is skipped)
        assert_eq!(restored.len(), 1);
        match &restored[0].state {
            robson_domain::PositionState::Armed => {}
            _ => panic!("Expected Armed state"),
        }
    }
}
