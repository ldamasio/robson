//! PostgreSQL projection reader for crash recovery.
//!
//! This module provides:
//! - `ProjectionRecovery` trait for reading positions from projection
//! - `PgProjectionReader` adapter implementing the trait
//! - `find_active_from_projection` function for direct access
//!
//! This module uses dynamic queries (sqlx::query) instead of compile-time
//! checked macros (sqlx::query!) to allow compilation without DATABASE_URL.

use crate::error::StoreError;
use async_trait::async_trait;
use robson_domain::{Position, Price, Quantity, Side, Symbol, TechnicalStopDistance};
use rust_decimal::Decimal;
use sqlx::{PgPool, Row};
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

/// Row struct for positions_current query results.
///
/// Derived from the positions_current table schema in migrations.
#[derive(Debug)]
struct PositionCurrentRow {
    position_id: Uuid,
    account_id: Uuid,
    symbol: String,
    side: String,
    state: String,
    entry_price: Option<Decimal>,
    entry_quantity: Option<Decimal>,
    trailing_stop_price: Option<Decimal>,
    favorable_extreme: Option<Decimal>,
    extreme_at: Option<chrono::DateTime<chrono::Utc>>,
    technical_stop_distance: Option<Decimal>,
    technical_stop_price: Option<Decimal>,
    created_at: chrono::DateTime<chrono::Utc>,
    updated_at: chrono::DateTime<chrono::Utc>,
}

/// Helper function to parse a row from positions_current query.
///
/// Uses sqlx::Row trait with rust_decimal feature enabled.
fn parse_position_row(row: &sqlx::postgres::PgRow) -> Result<PositionCurrentRow, sqlx::Error> {
    // Helper to get optional decimal values
    let try_get_decimal = |column: &str| -> Option<Decimal> {
        row.try_get::<Decimal, _>(column).ok()
    };

    Ok(PositionCurrentRow {
        position_id: row.try_get("position_id")?,
        account_id: row.try_get("account_id")?,
        symbol: row.try_get("symbol")?,
        side: row.try_get("side")?,
        state: row.try_get("state")?,
        entry_price: try_get_decimal("entry_price"),
        entry_quantity: try_get_decimal("entry_quantity"),
        trailing_stop_price: try_get_decimal("trailing_stop_price"),
        favorable_extreme: try_get_decimal("favorable_extreme"),
        extreme_at: row.try_get("extreme_at").ok(),
        technical_stop_distance: try_get_decimal("technical_stop_distance"),
        technical_stop_price: try_get_decimal("technical_stop_price"),
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
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
    let rows = sqlx::query(
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
    )
    .bind(tenant_id)
    .fetch_all(pool)
    .await
    .map_err(|e| StoreError::Database(format!("Failed to read projection: {}", e)))?;

    let mut positions = Vec::new();

    for row in rows {
        let row_data = parse_position_row(&row)
            .map_err(|e| StoreError::Database(format!("Failed to parse row: {}", e)))?;

        // Parse symbol
        let symbol = Symbol::from_pair(&row_data.symbol)
            .map_err(|e| StoreError::Deserialization(format!("Invalid symbol {}: {}", row_data.symbol, e)))?;

        // Parse side
        let side = match row_data.side.as_str() {
            "long" => Side::Long,
            "short" => Side::Short,
            _ => return Err(StoreError::Deserialization(format!("Invalid side: {}", row_data.side))),
        };

        // Create base position
        let mut position = Position::new(row_data.account_id, symbol, side);
        position.id = row_data.position_id;

        // Set entry data
        if let Some(entry_price) = row_data.entry_price {
            position.entry_price = Some(Price::new(entry_price).map_err(|e| {
                StoreError::Deserialization(format!("Invalid entry_price {}: {}", entry_price, e))
            })?);
        }

        if let Some(entry_quantity) = row_data.entry_quantity {
            position.quantity = Quantity::new(entry_quantity).map_err(|e| {
                StoreError::Deserialization(format!("Invalid entry_quantity {}: {}", entry_quantity, e))
            })?;
        }

        // Set tech stop distance
        if let (Some(_distance), Some(stop_price)) =
            (row_data.technical_stop_distance, row_data.technical_stop_price)
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
        match row_data.state.as_str() {
            "armed" => {
                position.state = robson_domain::PositionState::Armed;
            }
            "active" => {
                // For Active positions, we need to reconstruct the full Active state
                // This requires: current_price, trailing_stop, favorable_extreme, extreme_at
                let current_price = row_data
                    .entry_price
                    .ok_or_else(|| StoreError::Deserialization("Missing entry_price for active position".to_string()))?;

                let trailing_stop = row_data
                    .trailing_stop_price
                    .ok_or_else(|| StoreError::Deserialization("Missing trailing_stop_price for active position".to_string()))?;

                let favorable_extreme = row_data.favorable_extreme.ok_or_else(|| {
                    StoreError::Deserialization("Missing favorable_extreme for active position".to_string())
                })?;

                let extreme_at = row_data.extreme_at.ok_or_else(|| {
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
                    last_emitted_stop: row_data.trailing_stop_price.map(|p| Price::new(p).unwrap()),
                };
            }
            _ => {
                // Skip other states (exiting, closed, error) as they are not "active" for recovery
                continue;
            }
        }

        // Set timestamps
        position.created_at = row_data.created_at;
        position.updated_at = row_data.updated_at;

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
    /// Run with: `cargo test -p robson-store --features postgres`
    #[sqlx::test(migrations = "../../../migrations")]
    async fn test_projection_recovery_restores_active_position(pool: PgPool) {
        // 1. Insert a test ACTIVE position directly into positions_current
        let tenant_id = Uuid::now_v7();
        let position_id = Uuid::now_v7();
        let account_id = Uuid::now_v7();
        let strategy_id = Uuid::now_v7();
        let now = Utc::now();

        // Note: We use dynamic query to avoid sqlx::query! macro requiring DATABASE_URL
        sqlx::query(
            r#"
            INSERT INTO positions_current (
                position_id, tenant_id, account_id, strategy_id,
                symbol, side, state,
                entry_price, entry_quantity,
                trailing_stop_price, favorable_extreme, extreme_at,
                technical_stop_distance, technical_stop_price,
                current_quantity,
                last_event_id, last_seq,
                created_at, updated_at
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19)
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
        .bind(dec!(0.01))
        .bind(Uuid::now_v7())
        .bind(1i64)
        .bind(now)
        .bind(now)
        .execute(&pool)
        .await
        .expect("Failed to insert test position");

        // 2. Create PgProjectionReader and restore positions
        let reader = PgProjectionReader::new(Arc::new(pool));
        let restored = reader
            .find_active_from_projection(tenant_id)
            .await
            .expect("Failed to restore positions");

        // 3. Verify the position was restored
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

    #[sqlx::test(migrations = "../../../migrations")]
    async fn test_projection_recovery_skips_closed_positions(pool: PgPool) {
        let tenant_id = Uuid::now_v7();
        let account_id = Uuid::now_v7();
        let strategy_id = Uuid::now_v7();
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
                current_quantity,
                last_event_id, last_seq,
                created_at, updated_at
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, NULL, NULL, NULL, NULL, NULL, $10, $11, $12, $13, $14)
            "#,
        )
        .bind(Uuid::now_v7())
        .bind(tenant_id)
        .bind(account_id)
        .bind(strategy_id)
        .bind("BTCUSDT")
        .bind("long")
        .bind("armed")
        .bind(dec!(95000))
        .bind(dec!(0.01))
        .bind(dec!(0.01))
        .bind(Uuid::now_v7())
        .bind(1i64)
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
                current_quantity,
                last_event_id, last_seq,
                created_at, updated_at
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, NULL, NULL, NULL, NULL, NULL, $10, $11, $12, $13, $14)
            "#,
        )
        .bind(Uuid::now_v7())
        .bind(tenant_id)
        .bind(account_id)
        .bind(strategy_id)
        .bind("BTCUSDT")
        .bind("long")
        .bind("closed")
        .bind(dec!(95000))
        .bind(dec!(0.01))
        .bind(dec!(0.01))
        .bind(Uuid::now_v7())
        .bind(2i64)
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
