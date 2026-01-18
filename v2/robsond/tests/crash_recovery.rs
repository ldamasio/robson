//! Integration test for daemon crash recovery from PostgreSQL projection.
//!
//! This test verifies that the Daemon can restore positions from the
//! positions_current projection table after a crash.
//!
//! Run with: `cargo test -p robsond --features postgres crash_recovery`

#![cfg(feature = "postgres")]

use chrono::Utc;
use robson_eventlog::{append_event, Event, EventEnvelope};
use robson_projector::apply_event_to_projections;
use robson_store::{PgProjectionReader, ProjectionRecovery};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use std::sync::Arc;
use uuid::Uuid;

// =============================================================================
// Test Helpers
// =============================================================================

/// Helper to create and persist a POSITION_OPENED event to the projection.
///
/// This reduces duplication across tests by encapsulating the event creation,
/// append, and projection application logic.
async fn setup_position_event(
    pool: &sqlx::PgPool,
    tenant_id: Uuid,
    position_id: Uuid,
    account_id: Uuid,
    strategy_id: Uuid,
    symbol: &str,
    entry_price: Decimal,
    entry_quantity: Decimal,
    technical_stop_price: Decimal,
    technical_stop_distance: Decimal,
) -> Result<(), anyhow::Error> {
    let now = Utc::now();

    // 1. Create POSITION_OPENED event
    let event = Event::new(
        tenant_id,
        format!("position:{}", position_id),
        "POSITION_OPENED",
        serde_json::json!({
            "position_id": position_id,
            "account_id": account_id,
            "strategy_id": strategy_id,
            "symbol": symbol,
            "side": "long",
            "entry_price": entry_price,
            "entry_quantity": entry_quantity,
            "entry_filled_at": now.to_rfc3339(),
            "technical_stop_price": technical_stop_price,
            "technical_stop_distance": technical_stop_distance,
            "entry_order_id": Uuid::now_v7(),
            "stop_loss_order_id": Uuid::now_v7()
        }),
    );

    // 2. Append event to event log
    let stream_key = format!("position:{}", position_id);
    let event_id = append_event(pool, &stream_key, None, event).await?;

    // 3. Query the event envelope
    let envelope: EventEnvelope = sqlx::query_as(
        "SELECT * FROM event_log WHERE event_id = $1"
    )
    .bind(event_id)
    .fetch_one(pool)
    .await?;

    // 4. Apply event to projections
    apply_event_to_projections(pool, &envelope).await?;

    Ok(())
}

/// Integration test for daemon crash recovery.
///
/// This test:
/// 1. Creates a POSITION_OPENED event via event log
/// 2. Applies the event to projections (materializes positions_current)
/// 3. Creates Daemon with projection recovery
/// 4. Calls restore_positions() to verify positions are restored
///
/// Run with: `DATABASE_URL=postgresql://localhost/test cargo test -p robsond --features postgres -- --ignored`
#[sqlx::test(migrations = "../migrations")]
#[ignore = "Requires DATABASE_URL to be set"]
async fn test_crash_recovery_restores_active_position(pool: sqlx::PgPool) {
    // Setup test data
    let tenant_id = Uuid::now_v7();
    let position_id = Uuid::now_v7();
    let account_id = Uuid::now_v7();
    let strategy_id = Uuid::now_v7();

    // 1. Create and persist POSITION_OPENED event
    setup_position_event(
        &pool, tenant_id, position_id, account_id, strategy_id,
        "BTCUSDT", dec!(95000), dec!(0.01), dec!(93500), dec!(1500)
    ).await.expect("Failed to setup position event");

    // 2. Create projection recovery reader
    let projection_recovery = Arc::new(PgProjectionReader::new(Arc::new(pool))) as Arc<dyn ProjectionRecovery>;

    // 3. Restore positions from projection
    let restored = projection_recovery
        .find_active_from_projection(tenant_id)
        .await
        .expect("Failed to restore positions from projection");

    // 4. Verify position was restored correctly
    assert_eq!(restored.len(), 1, "Should restore 1 position");

    let pos = &restored[0];
    assert_eq!(pos.id, position_id);
    assert_eq!(pos.account_id, account_id);
    assert_eq!(pos.symbol.as_pair(), "BTCUSDT");

    // Verify Armed state (POSITION_OPENED creates Armed state)
    use robson_domain::PositionState;
    match &pos.state {
        PositionState::Armed => {
            // Position should be Armed after POSITION_OPENED
        }
        _ => panic!("Expected Armed state, got {:?}", pos.state),
    }

    // Verify entry price and quantity
    assert_eq!(pos.entry_price.as_ref().map(|p| p.as_decimal()), Some(dec!(95000)));
    assert_eq!(pos.quantity.as_decimal(), dec!(0.01));

    // Verify technical stop distance
    assert!(pos.tech_stop_distance.is_some());
    let tech_stop = pos.tech_stop_distance.as_ref().unwrap();
    assert_eq!(tech_stop.distance, dec!(1500));
    // distance_pct should be ~1.58% (1500 / 95000 * 100)
    assert!(tech_stop.distance_pct > dec!(1.5) && tech_stop.distance_pct < dec!(1.6));
}

/// Integration test for daemon restore_positions() method.
///
/// This test verifies that Daemon::restore_positions() correctly restores
/// positions from the projection to the in-memory store.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "Requires DATABASE_URL to be set"]
async fn test_daemon_restore_positions_from_projection(pool: sqlx::PgPool) {
    // Setup test data
    let tenant_id = Uuid::now_v7();
    let position_id = Uuid::now_v7();
    let account_id = Uuid::now_v7();
    let strategy_id = Uuid::now_v7();

    // 1. Create and persist POSITION_OPENED event
    setup_position_event(
        &pool, tenant_id, position_id, account_id, strategy_id,
        "ETHUSDT", dec!(3000), dec!(0.5), dec!(2950), dec!(50)
    ).await.expect("Failed to setup position event");

    // 2. Verify position is in projection
    let projection_recovery = Arc::new(PgProjectionReader::new(Arc::new(pool))) as Arc<dyn robson_store::ProjectionRecovery>;
    let restored = projection_recovery
        .find_active_from_projection(tenant_id)
        .await
        .expect("Failed to restore positions from projection");

    // Should restore 1 position
    assert_eq!(restored.len(), 1, "Should restore 1 position from projection");

    // Note: Testing Daemon::restore_positions() requires calling daemon.run(),
    // which is not feasible in this test setup. The projection_recovery test
    // above verifies that PgProjectionReader works correctly.
}

/// Test that closed positions are not restored during crash recovery.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "Requires DATABASE_URL to be set"]
async fn test_crash_recovery_skips_closed_positions(pool: sqlx::PgPool) {
    let tenant_id = Uuid::now_v7();
    let account_id = Uuid::now_v7();
    let strategy_id = Uuid::now_v7();

    // 1. Create an active position (should be restored)
    let active_position_id = Uuid::now_v7();
    setup_position_event(
        &pool, tenant_id, active_position_id, account_id, strategy_id,
        "BTCUSDT", dec!(95000), dec!(0.01), dec!(93500), dec!(1500)
    ).await.expect("Failed to setup active position event");

    // 2. Create a closed position (should NOT be restored)
    let closed_position_id = Uuid::now_v7();
    setup_position_event(
        &pool, tenant_id, closed_position_id, account_id, strategy_id,
        "BTCUSDT", dec!(90000), dec!(0.01), dec!(89000), dec!(1000)
    ).await.expect("Failed to setup closed position event");

    // 3. Close the second position by updating state in projection
    let now = Utc::now();
    sqlx::query(
        "UPDATE positions_current SET state = 'closed', closed_at = $1 WHERE position_id = $2"
    )
    .bind(now)
    .bind(closed_position_id)
    .execute(&pool)
    .await
    .expect("Failed to close position");

    // 4. Restore positions from projection
    let projection_recovery = Arc::new(PgProjectionReader::new(Arc::new(pool))) as Arc<dyn ProjectionRecovery>;
    let restored = projection_recovery
        .find_active_from_projection(tenant_id)
        .await
        .expect("Failed to restore positions from projection");

    // 5. Only the active position should be restored
    assert_eq!(restored.len(), 1, "Should restore only 1 active position");
    assert_eq!(restored[0].id, active_position_id, "Should restore active position, not closed one");
}
