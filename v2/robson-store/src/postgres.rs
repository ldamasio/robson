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
use robson_domain::{
    ExitReason, Position, PositionState, Price, Quantity, Side, Symbol, TechnicalStopDistance,
};
use rust_decimal::Decimal;
use sqlx::{PgPool, Row};
use std::sync::Arc;
use uuid::Uuid;

/// Trait for reading open core positions from a projection table.
///
/// This trait allows the Daemon to restore positions from the database
/// projection after a crash, without coupling the Daemon directly to PostgreSQL.
#[async_trait]
pub trait ProjectionRecovery: Send + Sync {
    /// Find open core positions from the projection for a given tenant.
    ///
    /// Returns positions participating in the core trading lifecycle:
    /// 'armed', 'entering', 'active', 'exiting'.
    ///
    /// Excludes terminal/manual states such as 'closed' and 'error'.
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
    current_quantity: Decimal,
    current_price: Option<Decimal>,
    trailing_stop_price: Option<Decimal>,
    favorable_extreme: Option<Decimal>,
    extreme_at: Option<chrono::DateTime<chrono::Utc>>,
    technical_stop_distance: Option<Decimal>,
    technical_stop_price: Option<Decimal>,
    entry_order_id: Option<Uuid>,
    exit_order_id: Option<Uuid>,
    entry_signal_id: Option<Uuid>,
    exit_reason: Option<String>,
    created_at: chrono::DateTime<chrono::Utc>,
    updated_at: chrono::DateTime<chrono::Utc>,
}

/// Helper function to parse a row from positions_current query.
///
/// Uses sqlx::Row trait with rust_decimal feature enabled.
/// For nullable columns, we use try_get::<Option<Decimal>, _>() to handle NULL values correctly.
fn parse_position_row(row: &sqlx::postgres::PgRow) -> Result<PositionCurrentRow, sqlx::Error> {
    // Helper to get optional decimal values (handles NULL correctly)
    let try_get_decimal = |column: &str| -> Option<Decimal> {
        row.try_get::<Option<Decimal>, _>(column).ok().flatten()
    };

    Ok(PositionCurrentRow {
        position_id: row.try_get("position_id")?,
        account_id: row.try_get("account_id")?,
        symbol: row.try_get("symbol")?,
        side: row.try_get("side")?,
        state: row.try_get("state")?,
        entry_price: try_get_decimal("entry_price"),
        entry_quantity: try_get_decimal("entry_quantity"),
        current_quantity: row.try_get("current_quantity")?,
        current_price: try_get_decimal("current_price"),
        trailing_stop_price: try_get_decimal("trailing_stop_price"),
        favorable_extreme: try_get_decimal("favorable_extreme"),
        extreme_at: row.try_get("extreme_at").ok(),
        technical_stop_distance: try_get_decimal("technical_stop_distance"),
        technical_stop_price: try_get_decimal("technical_stop_price"),
        entry_order_id: row.try_get("entry_order_id").ok(),
        exit_order_id: row.try_get("exit_order_id").ok(),
        entry_signal_id: row.try_get("entry_signal_id").ok(),
        exit_reason: row.try_get("exit_reason").ok(),
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn parse_exit_reason(reason: &str) -> Option<ExitReason> {
    match reason {
        "TrailingStop" | "trailing_stop" => Some(ExitReason::TrailingStop),
        "InsuranceStop" | "insurance_stop" => Some(ExitReason::InsuranceStop),
        "UserPanic" | "user_panic" => Some(ExitReason::UserPanic),
        "DegradedMode" | "degraded_mode" => Some(ExitReason::DegradedMode),
        "PositionError" | "position_error" => Some(ExitReason::PositionError),
        "DisarmedByUser" | "disarmed_by_user" => Some(ExitReason::DisarmedByUser),
        _ => None,
    }
}

/// Read open core positions from the `positions_current` projection table.
///
/// This function is used during crash recovery to restore open core positions
/// from the PostgreSQL projection. It reads positions with states:
/// - 'armed'
/// - 'entering'
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
/// * `Ok(Vec<Position>)` - List of open positions
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
/// - state: String ("armed", "entering", "active", "exiting", "closed", "error")
/// - entry_price: Decimal?
/// - entry_quantity: Decimal?
/// - current_quantity: Decimal
/// - current_price: Decimal?
/// - trailing_stop_price: Decimal?
/// - favorable_extreme: Decimal?
/// - extreme_at: Timestamp?
/// - technical_stop_distance: Decimal?
/// - entry_signal_id: UUID?
/// - exit_reason: TEXT?
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
            current_quantity,
            current_price,
            trailing_stop_price,
            favorable_extreme,
            extreme_at,
            technical_stop_distance,
            technical_stop_price,
            entry_order_id,
            exit_order_id,
            entry_signal_id,
            exit_reason,
            created_at,
            updated_at
        FROM positions_current
        WHERE tenant_id = $1
          AND state IN ('armed', 'entering', 'active', 'exiting')
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
        let symbol = Symbol::from_pair(&row_data.symbol).map_err(|e| {
            StoreError::Deserialization(format!("Invalid symbol {}: {}", row_data.symbol, e))
        })?;

        // Parse side
        let side = match row_data.side.as_str() {
            "long" => Side::Long,
            "short" => Side::Short,
            _ => {
                return Err(StoreError::Deserialization(format!(
                    "Invalid side: {}",
                    row_data.side
                )));
            },
        };

        // Create base position
        let mut position = Position::new(row_data.account_id, symbol, side);
        position.id = row_data.position_id;
        position.entry_order_id = row_data.entry_order_id;
        position.exit_order_id = row_data.exit_order_id;

        // Set entry data
        if let Some(entry_price) = row_data.entry_price {
            position.entry_price = Some(Price::new(entry_price).map_err(|e| {
                StoreError::Deserialization(format!("Invalid entry_price {}: {}", entry_price, e))
            })?);
        }

        if let Some(entry_quantity) = row_data.entry_quantity {
            position.quantity = Quantity::new(entry_quantity).map_err(|e| {
                StoreError::Deserialization(format!(
                    "Invalid entry_quantity {}: {}",
                    entry_quantity, e
                ))
            })?;
        }

        if row_data.current_quantity > Decimal::ZERO {
            position.quantity = Quantity::new(row_data.current_quantity).map_err(|e| {
                StoreError::Deserialization(format!(
                    "Invalid current_quantity {}: {}",
                    row_data.current_quantity, e
                ))
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
                position.state = PositionState::Armed;
            },
            "entering" => {
                let Some(entry_order_id) = row_data.entry_order_id else {
                    tracing::warn!(
                        position_id = %row_data.position_id,
                        "Skipping entering projection row without entry_order_id"
                    );
                    continue;
                };
                let Some(expected_entry) = row_data.entry_price else {
                    tracing::warn!(
                        position_id = %row_data.position_id,
                        "Skipping entering projection row without entry_price"
                    );
                    continue;
                };

                let signal_id = row_data.entry_signal_id.unwrap_or_else(|| {
                    tracing::warn!(
                        position_id = %row_data.position_id,
                        "Recovering entering projection row without signal_id; using nil UUID sentinel"
                    );
                    Uuid::nil()
                });

                position.state = PositionState::Entering {
                    entry_order_id,
                    expected_entry: Price::new(expected_entry).map_err(|e| {
                        StoreError::Deserialization(format!(
                            "Invalid expected_entry {}: {}",
                            expected_entry, e
                        ))
                    })?,
                    signal_id,
                };
            },
            "active" => {
                // For Active positions, we need to reconstruct the full Active state
                // This requires: current_price, trailing_stop, favorable_extreme, extreme_at
                let current_price =
                    row_data.current_price.or(row_data.entry_price).ok_or_else(|| {
                        StoreError::Deserialization(
                            "Missing current_price/entry_price for active position".to_string(),
                        )
                    })?;

                let trailing_stop = row_data.trailing_stop_price.ok_or_else(|| {
                    StoreError::Deserialization(
                        "Missing trailing_stop_price for active position".to_string(),
                    )
                })?;

                let favorable_extreme = row_data.favorable_extreme.ok_or_else(|| {
                    StoreError::Deserialization(
                        "Missing favorable_extreme for active position".to_string(),
                    )
                })?;

                let extreme_at = row_data.extreme_at.ok_or_else(|| {
                    StoreError::Deserialization(
                        "Missing extreme_at for active position".to_string(),
                    )
                })?;

                position.state = PositionState::Active {
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
            },
            "exiting" => {
                let Some(exit_reason_str) = row_data.exit_reason.as_deref() else {
                    tracing::warn!(
                        position_id = %row_data.position_id,
                        "Skipping exiting projection row without exit_reason"
                    );
                    continue;
                };
                let Some(exit_reason) = parse_exit_reason(exit_reason_str) else {
                    tracing::warn!(
                        position_id = %row_data.position_id,
                        exit_reason = %exit_reason_str,
                        "Skipping exiting projection row with unknown exit_reason"
                    );
                    continue;
                };

                let exit_order_id = row_data.exit_order_id.unwrap_or_else(|| {
                    tracing::warn!(
                        position_id = %row_data.position_id,
                        "Recovering exiting projection row without exit_order_id; using nil UUID sentinel"
                    );
                    Uuid::nil()
                });

                position.state = PositionState::Exiting { exit_order_id, exit_reason };
            },
            _ => {
                // Skip terminal/manual states not covered by crash recovery.
                continue;
            },
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
    /// Run with: `DATABASE_URL=postgresql://localhost/test cargo test -p robson-store --features postgres -- --ignored`
    #[sqlx::test(migrations = "../migrations")]
    #[ignore = "Requires DATABASE_URL to be set"]
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
                current_price, trailing_stop_price, favorable_extreme, extreme_at,
                technical_stop_distance, technical_stop_price,
                current_quantity,
                last_event_id, last_seq,
                created_at, updated_at
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20)
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
        .bind(dec!(96000))
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
                assert_eq!(current_price.as_decimal(), dec!(96000));
                assert_eq!(trailing_stop.as_decimal(), dec!(93500));
                assert_eq!(favorable_extreme.as_decimal(), dec!(97000));
                assert!(insurance_stop_id.is_none());
                assert_eq!(last_emitted_stop.as_ref().map(|p| p.as_decimal()), Some(dec!(93500)));
            },
            _ => panic!("Expected Active state, got {:?}", pos.state),
        }

        // Verify technical stop distance
        assert!(pos.tech_stop_distance.is_some());
        let tech_stop = pos.tech_stop_distance.as_ref().unwrap();
        assert_eq!(tech_stop.distance, dec!(1500));
        assert_eq!(tech_stop.distance_pct, dec!(1.57894736842105260000)); // ~1.58%
    }

    #[sqlx::test(migrations = "../migrations")]
    #[ignore = "Requires DATABASE_URL to be set"]
    async fn test_projection_recovery_restores_entering_and_exiting_positions(pool: PgPool) {
        let tenant_id = Uuid::now_v7();
        let account_id = Uuid::now_v7();
        let strategy_id = Uuid::now_v7();
        let now = Utc::now();

        let entering_id = Uuid::now_v7();
        let entering_order_id = Uuid::now_v7();
        let entering_signal_id = Uuid::now_v7();
        sqlx::query(
            r#"
            INSERT INTO positions_current (
                position_id, tenant_id, account_id, strategy_id,
                symbol, side, state,
                entry_price, entry_quantity,
                current_quantity,
                entry_order_id, entry_signal_id,
                last_event_id, last_seq,
                created_at, updated_at
            ) VALUES ($1, $2, $3, $4, $5, $6, 'entering', $7, $8, $8, $9, $10, $11, $12, $13, $13)
            "#,
        )
        .bind(entering_id)
        .bind(tenant_id)
        .bind(account_id)
        .bind(strategy_id)
        .bind("BTCUSDT")
        .bind("long")
        .bind(dec!(95100))
        .bind(dec!(0.02))
        .bind(entering_order_id)
        .bind(entering_signal_id)
        .bind(Uuid::now_v7())
        .bind(1i64)
        .bind(now)
        .execute(&pool)
        .await
        .expect("Failed to insert entering position");

        let exiting_id = Uuid::now_v7();
        let exiting_order_id = Uuid::now_v7();
        sqlx::query(
            r#"
            INSERT INTO positions_current (
                position_id, tenant_id, account_id, strategy_id,
                symbol, side, state,
                entry_price, entry_quantity,
                current_quantity,
                exit_order_id, exit_reason,
                last_event_id, last_seq,
                created_at, updated_at
            ) VALUES ($1, $2, $3, $4, $5, $6, 'exiting', $7, $8, $8, $9, $10, $11, $12, $13, $13)
            "#,
        )
        .bind(exiting_id)
        .bind(tenant_id)
        .bind(account_id)
        .bind(strategy_id)
        .bind("ETHUSDT")
        .bind("short")
        .bind(dec!(3000))
        .bind(dec!(0.5))
        .bind(exiting_order_id)
        .bind("TrailingStop")
        .bind(Uuid::now_v7())
        .bind(2i64)
        .bind(now)
        .execute(&pool)
        .await
        .expect("Failed to insert exiting position");

        let reader = PgProjectionReader::new(Arc::new(pool));
        let restored = reader
            .find_active_from_projection(tenant_id)
            .await
            .expect("Failed to restore positions");

        assert_eq!(restored.len(), 2, "Should restore entering + exiting positions");

        let entering = restored.iter().find(|p| p.id == entering_id).expect("Missing entering");
        match &entering.state {
            PositionState::Entering {
                entry_order_id,
                expected_entry,
                signal_id,
            } => {
                assert_eq!(*entry_order_id, entering_order_id);
                assert_eq!(expected_entry.as_decimal(), dec!(95100));
                assert_eq!(*signal_id, entering_signal_id);
            },
            _ => panic!("Expected Entering state, got {:?}", entering.state),
        }

        let exiting = restored.iter().find(|p| p.id == exiting_id).expect("Missing exiting");
        match &exiting.state {
            PositionState::Exiting { exit_order_id, exit_reason } => {
                assert_eq!(*exit_order_id, exiting_order_id);
                assert_eq!(*exit_reason, ExitReason::TrailingStop);
            },
            _ => panic!("Expected Exiting state, got {:?}", exiting.state),
        }
    }

    #[sqlx::test(migrations = "../migrations")]
    #[ignore = "Requires DATABASE_URL to be set"]
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

        // Insert an ERROR position (should NOT be restored)
        sqlx::query(
            r#"
            INSERT INTO positions_current (
                position_id, tenant_id, account_id, strategy_id,
                symbol, side, state,
                entry_price, entry_quantity,
                current_quantity,
                last_event_id, last_seq,
                created_at, updated_at
            ) VALUES ($1, $2, $3, $4, $5, $6, 'error', $7, $8, $8, $9, $10, $11, $11)
            "#,
        )
        .bind(Uuid::now_v7())
        .bind(tenant_id)
        .bind(account_id)
        .bind(strategy_id)
        .bind("ETHUSDT")
        .bind("long")
        .bind(dec!(3000))
        .bind(dec!(0.5))
        .bind(Uuid::now_v7())
        .bind(3i64)
        .bind(now)
        .execute(&pool)
        .await
        .expect("Failed to insert error position");

        let reader = PgProjectionReader::new(Arc::new(pool));
        let restored = reader
            .find_active_from_projection(tenant_id)
            .await
            .expect("Failed to restore positions");

        // Only armed position should be restored (closed + error are skipped)
        assert_eq!(restored.len(), 1);
        match &restored[0].state {
            robson_domain::PositionState::Armed => {},
            _ => panic!("Expected Armed state"),
        }
    }
}
