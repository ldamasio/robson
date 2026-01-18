//! DB-backed integration test for trailing stop projections
//!
//! Tests that trailing stop events are correctly projected to positions_current table.

use chrono::Utc;
use robson_eventlog::{append_event, ActorType, Event};
use rust_decimal_macros::dec;
use sqlx::PgPool;
use uuid::Uuid;

/// Helper to create and append an event, then apply projections
async fn append_and_project(pool: &PgPool, event: Event) {
    let stream_key = event.stream_key.clone();
    let event_id = append_event(pool, &stream_key, None, event).await.unwrap();

    // Fetch envelope for projection
    let envelope: robson_eventlog::EventEnvelope = sqlx::query_as(
        r#"SELECT event_id, tenant_id, stream_key, seq, event_type, payload, payload_schema_version,
                  occurred_at, ingested_at, idempotency_key, trace_id, causation_id, command_id, workflow_id,
                  actor_type, actor_id, prev_hash, hash
           FROM event_log WHERE event_id = $1"#
    )
    .bind(event_id)
    .fetch_one(pool)
    .await
    .unwrap();

    robson_projector::apply_event_to_projections(pool, &envelope).await.unwrap();
}

/// Test that EntryFilled initializes trailing_stop_price and favorable_extreme
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn test_entry_filled_initializes_trailing_stop(pool: PgPool) {
    let tenant_id = Uuid::now_v7();
    let account_id = Uuid::now_v7();
    let position_id = Uuid::now_v7();
    let strategy_id = Uuid::now_v7();

    // First: Create position via POSITION_OPENED
    let opened_event = Event::new(
        tenant_id,
        format!("position:{}", position_id),
        "POSITION_OPENED",
        serde_json::json!({
            "position_id": position_id,
            "tenant_id": tenant_id,
            "account_id": account_id,
            "strategy_id": strategy_id,
            "symbol": "BTCUSDT",
            "side": "long",
            "entry_price": null,
            "entry_quantity": null,
            "entry_filled_at": null,
            "technical_stop_price": "93500.0",
            "technical_stop_distance": "1500.0",
            "entry_order_id": null,
            "stop_loss_order_id": null
        }),
    )
    .with_actor(ActorType::Daemon, Some("test".to_string()));

    append_and_project(&pool, opened_event).await;

    // Second: Fill entry
    let filled_event = Event::new(
        tenant_id,
        format!("position:{}", position_id),
        "entry_filled",
        serde_json::json!({
            "position_id": position_id,
            "order_id": Uuid::now_v7(),
            "fill_price": "95000.0",
            "filled_quantity": "0.1",
            "fee": "0.001",
            "initial_stop": "93500.0",
            "timestamp": Utc::now()
        }),
    )
    .with_actor(ActorType::Daemon, Some("test".to_string()));

    append_and_project(&pool, filled_event).await;

    // Verify: trailing_stop_price = initial_stop, favorable_extreme = fill_price
    let row: (String, Option<rust_decimal::Decimal>, Option<rust_decimal::Decimal>) = sqlx::query_as(
        "SELECT state, trailing_stop_price, favorable_extreme FROM positions_current WHERE position_id = $1"
    )
    .bind(position_id)
    .fetch_one(&pool)
    .await
    .unwrap();

    assert_eq!(row.0, "active");
    assert_eq!(row.1, Some(dec!(93500.0)));
    assert_eq!(row.2, Some(dec!(95000.0)));
}

/// Test that TrailingStopUpdated updates stop price and favorable_extreme
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn test_trailing_stop_updated_monotonic(pool: PgPool) {
    let tenant_id = Uuid::now_v7();
    let account_id = Uuid::now_v7();
    let position_id = Uuid::now_v7();
    let strategy_id = Uuid::now_v7();

    // Setup: Create position and fill entry
    let opened_event = Event::new(
        tenant_id,
        format!("position:{}", position_id),
        "POSITION_OPENED",
        serde_json::json!({
            "position_id": position_id,
            "tenant_id": tenant_id,
            "account_id": account_id,
            "strategy_id": strategy_id,
            "symbol": "BTCUSDT",
            "side": "long",
            "entry_price": null,
            "entry_quantity": null,
            "entry_filled_at": null,
            "technical_stop_price": "93500.0",
            "technical_stop_distance": "1500.0",
            "entry_order_id": null,
            "stop_loss_order_id": null
        }),
    )
    .with_actor(ActorType::Daemon, Some("test".to_string()));

    append_and_project(&pool, opened_event).await;

    let filled_event = Event::new(
        tenant_id,
        format!("position:{}", position_id),
        "entry_filled",
        serde_json::json!({
            "position_id": position_id,
            "order_id": Uuid::now_v7(),
            "fill_price": "95000.0",
            "filled_quantity": "0.1",
            "fee": "0.001",
            "initial_stop": "93500.0",
            "timestamp": Utc::now()
        }),
    )
    .with_actor(ActorType::Daemon, Some("test".to_string()));

    append_and_project(&pool, filled_event).await;

    // Event 1: TrailingStopUpdated (price moved to 96500, stop to 95000)
    let update1 = Event::new(
        tenant_id,
        format!("position:{}", position_id),
        "trailing_stop_updated",
        serde_json::json!({
            "position_id": position_id,
            "previous_stop": "93500.0",
            "new_stop": "95000.0",
            "trigger_price": "96500.0",
            "timestamp": Utc::now()
        }),
    )
    .with_actor(ActorType::Daemon, Some("test".to_string()));

    append_and_project(&pool, update1).await;

    // Verify monotonic update (all 3 fields)
    let row1: (Option<rust_decimal::Decimal>, Option<rust_decimal::Decimal>, Option<chrono::DateTime<chrono::Utc>>) = sqlx::query_as(
        "SELECT trailing_stop_price, favorable_extreme, extreme_at FROM positions_current WHERE position_id = $1"
    )
    .bind(position_id)
    .fetch_one(&pool)
    .await
    .unwrap();

    assert_eq!(row1.0, Some(dec!(95000.0))); // trailing_stop_price
    assert_eq!(row1.1, Some(dec!(96500.0))); // favorable_extreme
    assert!(row1.2.is_some()); // extreme_at is set

    // Event 2: Another update (price to 97000, stop to 95500)
    let update2 = Event::new(
        tenant_id,
        format!("position:{}", position_id),
        "trailing_stop_updated",
        serde_json::json!({
            "position_id": position_id,
            "previous_stop": "95000.0",
            "new_stop": "95500.0",
            "trigger_price": "97000.0",
            "timestamp": Utc::now()
        }),
    )
    .with_actor(ActorType::Daemon, Some("test".to_string()));

    append_and_project(&pool, update2).await;

    // Verify second update
    let row2: (Option<rust_decimal::Decimal>, Option<rust_decimal::Decimal>) = sqlx::query_as(
        "SELECT trailing_stop_price, favorable_extreme FROM positions_current WHERE position_id = $1"
    )
    .bind(position_id)
    .fetch_one(&pool)
    .await
    .unwrap();

    assert_eq!(row2.0, Some(dec!(95500.0)));
    assert_eq!(row2.1, Some(dec!(97000.0)));
}

/// Test that ExitTriggered marks position as exiting
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn test_exit_triggered_marks_exiting(pool: PgPool) {
    let tenant_id = Uuid::now_v7();
    let account_id = Uuid::now_v7();
    let position_id = Uuid::now_v7();
    let strategy_id = Uuid::now_v7();

    // Setup: Create position and fill entry
    let opened_event = Event::new(
        tenant_id,
        format!("position:{}", position_id),
        "POSITION_OPENED",
        serde_json::json!({
            "position_id": position_id,
            "tenant_id": tenant_id,
            "account_id": account_id,
            "strategy_id": strategy_id,
            "symbol": "BTCUSDT",
            "side": "long",
            "entry_price": null,
            "entry_quantity": null,
            "entry_filled_at": null,
            "technical_stop_price": "93500.0",
            "technical_stop_distance": "1500.0",
            "entry_order_id": null,
            "stop_loss_order_id": null
        }),
    )
    .with_actor(ActorType::Daemon, Some("test".to_string()));

    append_and_project(&pool, opened_event).await;

    let filled_event = Event::new(
        tenant_id,
        format!("position:{}", position_id),
        "entry_filled",
        serde_json::json!({
            "position_id": position_id,
            "order_id": Uuid::now_v7(),
            "fill_price": "95000.0",
            "filled_quantity": "0.1",
            "fee": "0.001",
            "initial_stop": "93500.0",
            "timestamp": Utc::now()
        }),
    )
    .with_actor(ActorType::Daemon, Some("test".to_string()));

    append_and_project(&pool, filled_event).await;

    // Event: ExitTriggered
    let exit_event = Event::new(
        tenant_id,
        format!("position:{}", position_id),
        "exit_triggered",
        serde_json::json!({
            "position_id": position_id,
            "reason": "trailing_stop",
            "trigger_price": "93500.0",
            "stop_price": "93500.0",
            "timestamp": Utc::now()
        }),
    )
    .with_actor(ActorType::Daemon, Some("test".to_string()));

    append_and_project(&pool, exit_event).await;

    // Verify state changed to exiting
    let state: String = sqlx::query_scalar(
        "SELECT state FROM positions_current WHERE position_id = $1"
    )
    .bind(position_id)
    .fetch_one(&pool)
    .await
    .unwrap();

    assert_eq!(state, "exiting");
}
