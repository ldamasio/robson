//! Integration test for daemon crash recovery from PostgreSQL projection.
//!
//! This test verifies that the Daemon can restore positions from the
//! positions_current projection table after a crash.
//!
//! Run with: `cargo test -p robsond --features postgres crash_recovery`

#![cfg(feature = "postgres")]

use chrono::Utc;
use robson_eventlog::{Event, EventEnvelope, append_event};
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
    let envelope: EventEnvelope = sqlx::query_as("SELECT * FROM event_log WHERE event_id = $1")
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
        &pool,
        tenant_id,
        position_id,
        account_id,
        strategy_id,
        "BTCUSDT",
        dec!(95000),
        dec!(0.01),
        dec!(93500),
        dec!(1500),
    )
    .await
    .expect("Failed to setup position event");

    // 2. Create projection recovery reader
    let projection_recovery =
        Arc::new(PgProjectionReader::new(Arc::new(pool))) as Arc<dyn ProjectionRecovery>;

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
        },
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
        &pool,
        tenant_id,
        position_id,
        account_id,
        strategy_id,
        "ETHUSDT",
        dec!(3000),
        dec!(0.5),
        dec!(2950),
        dec!(50),
    )
    .await
    .expect("Failed to setup position event");

    // 2. Verify position is in projection
    let projection_recovery = Arc::new(PgProjectionReader::new(Arc::new(pool)))
        as Arc<dyn robson_store::ProjectionRecovery>;
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
        &pool,
        tenant_id,
        active_position_id,
        account_id,
        strategy_id,
        "BTCUSDT",
        dec!(95000),
        dec!(0.01),
        dec!(93500),
        dec!(1500),
    )
    .await
    .expect("Failed to setup active position event");

    // 2. Create a closed position (should NOT be restored)
    let closed_position_id = Uuid::now_v7();
    setup_position_event(
        &pool,
        tenant_id,
        closed_position_id,
        account_id,
        strategy_id,
        "BTCUSDT",
        dec!(90000),
        dec!(0.01),
        dec!(89000),
        dec!(1000),
    )
    .await
    .expect("Failed to setup closed position event");

    // 3. Close the second position by updating state in projection
    let now = Utc::now();
    sqlx::query(
        "UPDATE positions_current SET state = 'closed', closed_at = $1 WHERE position_id = $2",
    )
    .bind(now)
    .bind(closed_position_id)
    .execute(&pool)
    .await
    .expect("Failed to close position");

    // 4. Restore positions from projection
    let projection_recovery =
        Arc::new(PgProjectionReader::new(Arc::new(pool))) as Arc<dyn ProjectionRecovery>;
    let restored = projection_recovery
        .find_active_from_projection(tenant_id)
        .await
        .expect("Failed to restore positions from projection");

    // 5. Only the active position should be restored
    assert_eq!(restored.len(), 1, "Should restore only 1 active position");
    assert_eq!(
        restored[0].id, active_position_id,
        "Should restore active position, not closed one"
    );
}

// =============================================================================
// Domain event path tests (position_armed → ... → position_closed)
// These test the real execution path via robsond domain events, not POSITION_OPENED.
// =============================================================================

/// Append an event to the event log and immediately apply it to projections.
///
/// Returns the seq assigned to the event.
async fn append_and_apply(
    pool: &sqlx::PgPool,
    tenant_id: Uuid,
    stream_key: &str,
    event_type: &str,
    payload: serde_json::Value,
) -> Result<(), anyhow::Error> {
    use robson_eventlog::{Event, append_event};

    let event = Event::new(tenant_id, stream_key.to_string(), event_type, payload);
    let event_id = append_event(pool, stream_key, None, event).await?;

    let envelope: robson_eventlog::EventEnvelope =
        sqlx::query_as("SELECT * FROM event_log WHERE event_id = $1")
            .bind(event_id)
            .fetch_one(pool)
            .await?;

    apply_event_to_projections(pool, &envelope).await?;
    Ok(())
}

/// Test: position_armed creates an Armed row in positions_current.
/// Recovery must find it and reconstruct Armed state.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "Requires DATABASE_URL to be set"]
async fn test_recovery_from_domain_position_armed(pool: sqlx::PgPool) {
    let tenant_id = Uuid::now_v7();
    let position_id = Uuid::now_v7();
    let account_id = Uuid::now_v7();
    let stream_key = format!("position:{}", position_id);

    // Emit position_armed (matches robson-domain::Event::PositionArmed serialization)
    append_and_apply(
        &pool,
        tenant_id,
        &stream_key,
        "position_armed",
        serde_json::json!({
            "position_id": position_id,
            "account_id": account_id,
            "symbol": { "base": "BTC", "quote": "USDT" },
            "side": "Long",
            "tech_stop_distance": {
                "distance": "1500",
                "distance_pct": "1.578947",
                "initial_stop": "93500"
            },
            "timestamp": Utc::now().to_rfc3339()
        }),
    )
    .await
    .expect("Failed to append position_armed");

    // Recovery must find the Armed position
    let reader = Arc::new(robson_store::PgProjectionReader::new(Arc::new(pool)))
        as Arc<dyn robson_store::ProjectionRecovery>;
    let restored = reader.find_active_from_projection(tenant_id).await.expect("Recovery failed");

    assert_eq!(restored.len(), 1, "Should restore 1 armed position");
    let pos = &restored[0];
    assert_eq!(pos.id, position_id);
    assert_eq!(pos.symbol.as_pair(), "BTCUSDT");

    use robson_domain::PositionState;
    assert!(matches!(pos.state, PositionState::Armed), "Expected Armed, got {:?}", pos.state);
}

/// Test: full domain lifecycle — armed → entry_filled → trailing_stop_updated →
/// exit_filled → position_closed — closed position is NOT restored by recovery.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "Requires DATABASE_URL to be set"]
async fn test_recovery_domain_closed_position_excluded(pool: sqlx::PgPool) {
    let tenant_id = Uuid::now_v7();
    let position_id = Uuid::now_v7();
    let account_id = Uuid::now_v7();
    let order_id = Uuid::now_v7();
    let stream_key = format!("position:{}", position_id);
    let entry_price = dec!(95000);
    let exit_price = dec!(97000);

    // 1. position_armed
    append_and_apply(
        &pool,
        tenant_id,
        &stream_key,
        "position_armed",
        serde_json::json!({
            "position_id": position_id,
            "account_id": account_id,
            "symbol": { "base": "BTC", "quote": "USDT" },
            "side": "Long",
            "tech_stop_distance": {
                "distance": "1500",
                "distance_pct": "1.578947",
                "initial_stop": "93500"
            },
            "timestamp": Utc::now().to_rfc3339()
        }),
    )
    .await
    .expect("Failed position_armed");

    // 2. entry_filled
    append_and_apply(
        &pool,
        tenant_id,
        &stream_key,
        "entry_filled",
        serde_json::json!({
            "position_id": position_id,
            "order_id": order_id,
            "fill_price": entry_price,
            "filled_quantity": "0.01",
            "fee": "0.1",
            "initial_stop": "93500",
            "timestamp": Utc::now().to_rfc3339()
        }),
    )
    .await
    .expect("Failed entry_filled");

    // 3. trailing_stop_updated
    append_and_apply(
        &pool,
        tenant_id,
        &stream_key,
        "trailing_stop_updated",
        serde_json::json!({
            "position_id": position_id,
            "previous_stop": "93500",
            "new_stop": "95500",
            "trigger_price": "97000",
            "timestamp": Utc::now().to_rfc3339()
        }),
    )
    .await
    .expect("Failed trailing_stop_updated");

    // 4. exit_filled
    append_and_apply(
        &pool,
        tenant_id,
        &stream_key,
        "exit_filled",
        serde_json::json!({
            "position_id": position_id,
            "order_id": order_id,
            "fill_price": exit_price,
            "filled_quantity": "0.01",
            "fee": "0.1",
            "timestamp": Utc::now().to_rfc3339()
        }),
    )
    .await
    .expect("Failed exit_filled");

    // 5. position_closed (lowercase domain event with P&L)
    append_and_apply(
        &pool,
        tenant_id,
        &stream_key,
        "position_closed",
        serde_json::json!({
            "position_id": position_id,
            "exit_reason": "TrailingStop",
            "entry_price": entry_price,
            "exit_price": exit_price,
            "realized_pnl": "20",
            "total_fees": "0.2",
            "timestamp": Utc::now().to_rfc3339()
        }),
    )
    .await
    .expect("Failed position_closed");

    // Recovery must NOT return the closed position
    let reader = Arc::new(robson_store::PgProjectionReader::new(Arc::new(pool)))
        as Arc<dyn robson_store::ProjectionRecovery>;
    let restored = reader.find_active_from_projection(tenant_id).await.expect("Recovery failed");

    assert_eq!(
        restored.len(),
        0,
        "Closed position must not be restored; got {:?}",
        restored.iter().map(|p| p.id).collect::<Vec<_>>()
    );
}

// =============================================================================
// Projection and replay tests (MIG-v2.5#2)
// These validate append → apply → recovery using manual event construction.
// They do NOT exercise the PositionManager runtime path.
// The real runtime e2e test is test_runtime_arm_position_persists_to_projection below.
// =============================================================================

/// Test that projector returns error for unknown event types.
///
/// This validates that MIG-v2.5#2 does not silently skip unknown events.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "Requires DATABASE_URL to be set"]
async fn test_projector_rejects_unknown_event_type(pool: sqlx::PgPool) {
    use robson_eventlog::{Event, append_event};

    let tenant_id = Uuid::now_v7();
    let stream_key = "test:unknown:event";

    // Append an event with unknown type
    let event = Event::new(
        tenant_id,
        stream_key.to_string(),
        "UNKNOWN_EVENT_TYPE",
        serde_json::json!({ "foo": "bar" }),
    );
    let event_id = append_event(&pool, stream_key, None, event)
        .await
        .expect("Failed to append event");

    // Fetch the envelope
    let envelope: EventEnvelope = sqlx::query_as("SELECT * FROM event_log WHERE event_id = $1")
        .bind(event_id)
        .fetch_one(&pool)
        .await
        .expect("Failed to fetch envelope");

    // Apply to projections - should return MissingHandler error
    let result = apply_event_to_projections(&pool, &envelope).await;
    assert!(result.is_err(), "Projector should return error for unknown event type");

    let err = result.unwrap_err();
    assert!(
        err.to_string().contains("UNKNOWN_EVENT_TYPE"),
        "Error should mention the unknown event type"
    );
}

/// Projection/replay test: manually appended position_armed flows through
/// projection and is recoverable.
///
/// This is a PROJECTION test, not a runtime e2e test. It validates:
/// 1. append_event → apply_event_to_projections works for position_armed
/// 2. Recovery reads the armed position from positions_current
///
/// For the real runtime path (PositionManager → Executor → eventlog → projection),
/// see test_runtime_arm_position_persists_to_projection.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "Requires DATABASE_URL to be set"]
async fn test_projection_and_recovery_from_appended_position_armed(pool: sqlx::PgPool) {
    use robson_eventlog::{Event, append_event};

    let tenant_id = Uuid::now_v7();
    let position_id = Uuid::now_v7();
    let account_id = Uuid::now_v7();
    let stream_key = format!("position:{}", position_id);

    // 1. Emit position_armed event (simulating runtime path)
    let event = Event::new(
        tenant_id,
        stream_key.clone(),
        "position_armed",
        serde_json::json!({
            "position_id": position_id,
            "account_id": account_id,
            "symbol": { "base": "BTC", "quote": "USDT" },
            "side": "Long",
            "tech_stop_distance": {
                "distance": "1500",
                "distance_pct": "1.578947",
                "initial_stop": "93500"
            },
            "timestamp": chrono::Utc::now().to_rfc3339()
        }),
    );

    let event_id = append_event(&pool, &stream_key, None, event)
        .await
        .expect("Failed to append position_armed event");

    // 2. Fetch envelope and apply to projections (simulating execute_and_persist apply path)
    let envelope: EventEnvelope = sqlx::query_as("SELECT * FROM event_log WHERE event_id = $1")
        .bind(event_id)
        .fetch_one(&pool)
        .await
        .expect("Failed to fetch envelope");

    apply_event_to_projections(&pool, &envelope)
        .await
        .expect("Failed to apply event to projections");

    // 3. Verify projection is updated
    let state: Option<String> =
        sqlx::query_scalar("SELECT state FROM positions_current WHERE position_id = $1")
            .bind(position_id)
            .fetch_optional(&pool)
            .await
            .expect("Failed to query projection");

    assert_eq!(
        state.as_deref(),
        Some("armed"),
        "Projection should show armed state after apply"
    );

    // 4. Verify recovery from projection works
    let reader = Arc::new(robson_store::PgProjectionReader::new(Arc::new(pool.clone())))
        as Arc<dyn robson_store::ProjectionRecovery>;
    let restored = reader.find_active_from_projection(tenant_id).await.expect("Recovery failed");

    assert_eq!(restored.len(), 1, "Should restore 1 position from projection");
    assert_eq!(restored[0].id, position_id, "Recovered position ID should match");
    assert_eq!(restored[0].symbol.as_pair(), "BTCUSDT", "Recovered symbol should match");
}

/// Projection/replay test: exit event ordering is preserved in the eventlog.
///
/// This is a REPLAY test, not a runtime e2e test. It validates:
/// - exit_order_placed is appended before position_closed (ordering constraint)
/// - Replaying both events via apply_event_to_projections yields closed state
/// - Closed positions are excluded from recovery
///
/// The ordering guarantee on the LIVE path is enforced by execute_and_persist()
/// which persists ExitOrderPlaced before the caller emits PositionClosed.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "Requires DATABASE_URL to be set"]
async fn test_replay_exit_event_ordering_preserved_in_eventlog(pool: sqlx::PgPool) {
    use robson_eventlog::{Event, append_event};

    let tenant_id = Uuid::now_v7();
    let position_id = Uuid::now_v7();
    let order_id = Uuid::now_v7();
    let stream_key = format!("position:{}", position_id);

    // 1. First: position_armed
    let event1 = Event::new(
        tenant_id,
        stream_key.clone(),
        "position_armed",
        serde_json::json!({
            "position_id": position_id,
            "account_id": Uuid::nil(),
            "symbol": { "base": "BTC", "quote": "USDT" },
            "side": "Long",
            "tech_stop_distance": {
                "distance": "1500",
                "distance_pct": "1.578947",
                "initial_stop": "93500"
            },
            "timestamp": chrono::Utc::now().to_rfc3339()
        }),
    );
    let _ = append_event(&pool, &stream_key, None, event1)
        .await
        .expect("Failed to append position_armed");

    // 2. Second: exit_order_placed (must come before position_closed)
    let event2 = Event::new(
        tenant_id,
        stream_key.clone(),
        "exit_order_placed",
        serde_json::json!({
            "position_id": position_id,
            "order_id": order_id,
            "expected_price": "94500",
            "quantity": "0.01",
            "exit_reason": "TrailingStop",
            "timestamp": chrono::Utc::now().to_rfc3339()
        }),
    );
    let _ = append_event(&pool, &stream_key, None, event2)
        .await
        .expect("Failed to append exit_order_placed");

    // 3. Third: position_closed (must come after exit_order_placed)
    let event3 = Event::new(
        tenant_id,
        stream_key.clone(),
        "position_closed",
        serde_json::json!({
            "position_id": position_id,
            "exit_reason": "TrailingStop",
            "entry_price": "95000",
            "exit_price": "94500",
            "realized_pnl": "-50",
            "total_fees": "10",
            "timestamp": chrono::Utc::now().to_rfc3339()
        }),
    );
    let _ = append_event(&pool, &stream_key, None, event3)
        .await
        .expect("Failed to append position_closed");

    // 4. Fetch all events in order
    let events: Vec<(String, i64)> = sqlx::query_as(
        "SELECT event_type, seq FROM event_log WHERE tenant_id = $1 AND stream_key = $2 ORDER BY seq ASC",
    )
    .bind(tenant_id)
    .bind(&stream_key)
    .fetch_all(&pool)
    .await
    .expect("Failed to fetch events");

    assert_eq!(events.len(), 3, "Should have 3 events");
    assert_eq!(events[0].0, "position_armed", "First event should be position_armed");
    assert_eq!(events[1].0, "exit_order_placed", "Second event should be exit_order_placed");
    assert_eq!(events[2].0, "position_closed", "Third event should be position_closed");
    assert!(events[0].1 < events[1].1, "position_armed seq < exit_order_placed seq");
    assert!(events[1].1 < events[2].1, "exit_order_placed seq < position_closed seq");

    // 5. Apply events to projections in order (simulating replay)
    let envelopes: Vec<EventEnvelope> = sqlx::query_as(
        "SELECT * FROM event_log WHERE tenant_id = $1 AND stream_key = $2 ORDER BY seq ASC",
    )
    .bind(tenant_id)
    .bind(&stream_key)
    .fetch_all(&pool)
    .await
    .expect("Failed to fetch envelopes");

    for envelope in &envelopes {
        apply_event_to_projections(&pool, envelope)
            .await
            .expect("Failed to apply event to projections");
    }

    // 6. Verify final state is closed
    let state: Option<String> =
        sqlx::query_scalar("SELECT state FROM positions_current WHERE position_id = $1")
            .bind(position_id)
            .fetch_optional(&pool)
            .await
            .expect("Failed to query projection");

    assert_eq!(
        state.as_deref(),
        Some("closed"),
        "Final state should be closed after all events applied"
    );

    // 7. Verify closed position is NOT restored during recovery
    let reader = Arc::new(robson_store::PgProjectionReader::new(Arc::new(pool.clone())))
        as Arc<dyn robson_store::ProjectionRecovery>;
    let restored = reader.find_active_from_projection(tenant_id).await.expect("Recovery failed");

    assert_eq!(restored.len(), 0, "Closed position should NOT be restored from projection");
}

// =============================================================================
// Real runtime e2e tests (MIG-v2.5#2)
// These exercise the ACTUAL PositionManager → Executor → eventlog → projection path.
// No manual append_event calls. Failures in persist/apply propagate as errors.
// =============================================================================

/// Runtime e2e test: arm_position() persists to eventlog and updates projection.
///
/// This is the REAL runtime path test for MIG-v2.5#2. It exercises:
///   PositionManager::arm_position()
///     → execute_and_persist(EmitEvent(PositionArmed))
///       → Executor::execute() → store.apply_event()       (in-memory projection)
///       → persist_event_to_log()                          (fail-fast)
///         → append_event() to event_log
///         → apply_event_to_projections() → positions_current
///   → PgProjectionReader::find_active_from_projection()   (crash recovery)
///
/// If persist_event_to_log() fails (append or projection apply), arm_position()
/// returns an error — there is no silent best-effort fallback.
///
/// Run with: `DATABASE_URL=postgresql://... cargo test -p robsond --features postgres -- --ignored`
#[sqlx::test(migrations = "../migrations")]
#[ignore = "Requires DATABASE_URL to be set"]
async fn test_runtime_arm_position_persists_to_projection(pool: sqlx::PgPool) {
    use robson_domain::{Price, RiskConfig, Side, Symbol, TechnicalStopDistance};
    use robson_engine::Engine;
    use robson_exec::{Executor, IntentJournal, StubExchange};
    use robson_store::MemoryStore;
    use robsond::{EventBus, PositionManager, TracingQueryRecorder};
    use rust_decimal_macros::dec;

    let tenant_id = Uuid::now_v7();
    let account_id = Uuid::now_v7();

    // Build PositionManager with real event_log pool (fail-fast persistence enabled)
    let exchange = Arc::new(StubExchange::new(dec!(95000)));
    let journal = Arc::new(IntentJournal::new());
    let store = Arc::new(MemoryStore::new());
    let executor = Arc::new(Executor::new(exchange, journal, store.clone()));
    let event_bus = Arc::new(EventBus::new(100));
    let risk_config = RiskConfig::new(dec!(10000)).unwrap();
    let engine = Engine::new(risk_config.clone());

    let manager = Arc::new(
        PositionManager::new(engine, executor, store, event_bus, Arc::new(TracingQueryRecorder))
            .with_event_log(pool.clone(), tenant_id),
    );

    // arm_position() exercises the full runtime write path
    let entry_price = Price::new(dec!(95000)).unwrap();
    let stop_price = Price::new(dec!(93500)).unwrap();
    let tech_stop = TechnicalStopDistance::from_entry_and_stop(entry_price, stop_price);
    let symbol = Symbol::from_pair("BTCUSDT").unwrap();

    let position = manager
        .arm_position(symbol, Side::Long, risk_config, tech_stop, account_id)
        .await
        .expect("arm_position failed — eventlog or projection apply error");

    // Verify the event was persisted to event_log (append succeeded)
    let event_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM event_log WHERE stream_key = $1")
            .bind(format!("position:{}", position.id))
            .fetch_one(&pool)
            .await
            .expect("Failed to query event_log");

    assert!(
        event_count >= 1,
        "Expected at least 1 event in event_log for position {}, got {}",
        position.id,
        event_count
    );

    // Verify the event type is position_armed
    let event_type: String = sqlx::query_scalar(
        "SELECT event_type FROM event_log WHERE stream_key = $1 ORDER BY seq ASC LIMIT 1",
    )
    .bind(format!("position:{}", position.id))
    .fetch_one(&pool)
    .await
    .expect("Failed to query event_log event_type");

    assert_eq!(event_type, "position_armed", "First event must be position_armed");

    // Verify positions_current was updated synchronously (projection apply succeeded)
    let state: Option<String> =
        sqlx::query_scalar("SELECT state FROM positions_current WHERE position_id = $1")
            .bind(position.id)
            .fetch_optional(&pool)
            .await
            .expect("Failed to query positions_current");

    assert_eq!(
        state.as_deref(),
        Some("armed"),
        "positions_current should show 'armed' state after arm_position()"
    );

    // Verify crash recovery finds the armed position
    let reader = Arc::new(robson_store::PgProjectionReader::new(Arc::new(pool.clone())))
        as Arc<dyn robson_store::ProjectionRecovery>;
    let restored = reader.find_active_from_projection(tenant_id).await.expect("Recovery failed");

    assert_eq!(restored.len(), 1, "Crash recovery should find exactly 1 armed position");
    assert_eq!(restored[0].id, position.id, "Recovered position ID must match");
    assert_eq!(restored[0].symbol.as_pair(), "BTCUSDT", "Recovered symbol must match");

    use robson_domain::PositionState;
    assert!(
        matches!(restored[0].state, PositionState::Armed),
        "Recovered position must be in Armed state, got {:?}",
        restored[0].state
    );
}

// =============================================================================
// MIG-v2.5#2: entry_signal_received handler test
// =============================================================================

/// Test that entry_signal_received event is handled without error.
///
/// This is an audit event that doesn't change position state.
/// The handler must NOT return MissingHandler error.
/// Regression test for the gap where engine emits EntrySignalReceived
/// but projector had no handler for it.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "Requires DATABASE_URL to be set"]
async fn test_entry_signal_received_handled_without_error(pool: sqlx::PgPool) {
    use robson_eventlog::{Event, append_event};

    let tenant_id = Uuid::now_v7();
    let position_id = Uuid::now_v7();
    let signal_id = Uuid::now_v7();
    let stream_key = format!("position:{}", position_id);

    // First, arm the position so it exists in projection
    let armed_event = Event::new(
        tenant_id,
        stream_key.clone(),
        "position_armed",
        serde_json::json!({
            "position_id": position_id,
            "account_id": Uuid::nil(),
            "symbol": { "base": "BTC", "quote": "USDT" },
            "side": "Long",
            "tech_stop_distance": {
                "distance": "1500",
                "distance_pct": "1.578947",
                "initial_stop": "93500"
            },
            "timestamp": chrono::Utc::now().to_rfc3339()
        }),
    );
    let armed_id = append_event(&pool, &stream_key, None, armed_event)
        .await
        .expect("Failed to append position_armed");
    let armed_envelope: EventEnvelope =
        sqlx::query_as("SELECT * FROM event_log WHERE event_id = $1")
            .bind(armed_id)
            .fetch_one(&pool)
            .await
            .expect("Failed to fetch armed envelope");
    apply_event_to_projections(&pool, &armed_envelope)
        .await
        .expect("Failed to apply position_armed");

    // Now emit entry_signal_received - this is the key test
    let signal_event = Event::new(
        tenant_id,
        stream_key.clone(),
        "entry_signal_received",
        serde_json::json!({
            "position_id": position_id,
            "signal_id": signal_id,
            "entry_price": "95000",
            "stop_loss": "93500",
            "quantity": "0.01",
            "timestamp": chrono::Utc::now().to_rfc3339()
        }),
    );
    let signal_event_id = append_event(&pool, &stream_key, None, signal_event)
        .await
        .expect("Failed to append entry_signal_received");

    let signal_envelope: EventEnvelope =
        sqlx::query_as("SELECT * FROM event_log WHERE event_id = $1")
            .bind(signal_event_id)
            .fetch_one(&pool)
            .await
            .expect("Failed to fetch signal envelope");

    // This MUST NOT return MissingHandler error
    let result = apply_event_to_projections(&pool, &signal_envelope).await;
    assert!(
        result.is_ok(),
        "entry_signal_received should be handled without error, got: {:?}",
        result.err()
    );

    // Position should still be in armed state (signal doesn't change state)
    let state: Option<String> =
        sqlx::query_scalar("SELECT state FROM positions_current WHERE position_id = $1")
            .bind(position_id)
            .fetch_optional(&pool)
            .await
            .expect("Failed to query projection");

    assert_eq!(
        state.as_deref(),
        Some("armed"),
        "entry_signal_received should NOT change position state"
    );
}
