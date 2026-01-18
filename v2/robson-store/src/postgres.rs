//! PostgreSQL projection reader for crash recovery.
//!
//! This module provides functions to read from the `positions_current`
//! projection table for crash recovery purposes.

use crate::error::StoreError;
use robson_domain::{Position, Price, Quantity, Side, Symbol, TechnicalStopDistance};
use rust_decimal::Decimal;
use sqlx::PgPool;
use uuid::Uuid;

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

            let tech_stop = TechnicalStopDistance::new(distance, entry.as_decimal() - stop_price);
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

    // Note: These tests require a running PostgreSQL database.
    // In a real setup, use testcontainers or a separate test database.
}
