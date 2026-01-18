//! Integration tests for robson-projector
//!
//! Tests event handlers against a test database using sqlx-test fixtures.
//!
//! # Running these tests
//!
//! These tests require a PostgreSQL database with migration 002 applied:
//!
//! ```bash
//! # 1. Start PostgreSQL (example with docker)
//! docker run --rm -p 5432:5432 -e POSTGRES_PASSWORD=test postgres:16
//!
//! # 2. Apply migration 002
//! psql -h localhost -U postgres -d postgres -f v2/migrations/002_event_log_phase9.sql
//!
//! # 3. Run tests
//! DATABASE_URL="postgresql://postgres:test@localhost/postgres" \
//!   cargo test -p robson-projector --test integration_test -- --ignored
//! ```

use chrono::Utc;
use robson_eventlog::{ActorType, EventEnvelope};
use robson_projector::apply_event_to_projections;
use rust_decimal::Decimal;
use uuid::Uuid;

/// Test helper: Create an event envelope from an event
fn make_envelope(
    stream_key: &str,
    event_type: &str,
    payload: serde_json::Value,
    seq: i64,
) -> EventEnvelope {
    EventEnvelope {
        event_id: Uuid::new_v4(),
        tenant_id: Uuid::new_v4(),
        stream_key: stream_key.to_string(),
        seq,
        event_type: event_type.to_string(),
        payload,
        payload_schema_version: 1,
        occurred_at: Utc::now(),
        ingested_at: Utc::now(),
        idempotency_key: format!("{}:{}:{}", stream_key, seq, event_type),
        trace_id: None,
        causation_id: None,
        command_id: None,
        workflow_id: None,
        actor_type: Some(ActorType::CLI),
        actor_id: Some("test-user".to_string()),
        prev_hash: None,
        hash: None,
    }
}

// =============================================================================
// SQLX-TEST: Integration tests require DATABASE_URL
// =============================================================================

#[sqlx::test]
#[ignore = "requires DATABASE_URL (see file header for setup)"]
async fn test_order_submitted_creates_projection(pool: sqlx::PgPool) -> sqlx::Result<()> {
    // Arrange
    let order_id = Uuid::new_v4();
    let tenant_id = Uuid::new_v4();
    let account_id = Uuid::new_v4();

    let payload = serde_json::json!({
        "order_id": order_id,
        "tenant_id": tenant_id,
        "account_id": account_id,
        "position_id": null,
        "client_order_id": "client-123",
        "symbol": "BTCUSDT",
        "side": "buy",
        "order_type": "limit",
        "quantity": "0.1",
        "price": "50000",
        "stop_price": null
    });

    let envelope = make_envelope("order:test", "ORDER_SUBMITTED", payload, 1);

    // Act
    let result = apply_event_to_projections(&pool, &envelope).await;
    assert!(result.is_ok());

    // Assert
    let (status, last_seq): (String, i64) =
        sqlx::query_as("SELECT status, last_seq FROM orders_current WHERE order_id = $1")
            .bind(order_id)
            .fetch_one(&pool)
            .await?;

    assert_eq!(status, "pending");
    assert_eq!(last_seq, 1);

    Ok(())
}

#[sqlx::test]
#[ignore = "requires DATABASE_URL (see file header for setup)"]
async fn test_order_acked_updates_projection(pool: sqlx::PgPool) -> sqlx::Result<()> {
    // Arrange
    let order_id = Uuid::new_v4();
    let tenant_id = Uuid::new_v4();
    let account_id = Uuid::new_v4();

    // First submit
    let submit_payload = serde_json::json!({
        "order_id": order_id,
        "tenant_id": tenant_id,
        "account_id": account_id,
        "position_id": null,
        "client_order_id": "client-123",
        "symbol": "BTCUSDT",
        "side": "buy",
        "order_type": "limit",
        "quantity": "0.1",
        "price": "50000",
        "stop_price": null
    });
    let submit_env = make_envelope("order:test", "ORDER_SUBMITTED", submit_payload, 1);
    apply_event_to_projections(&pool, &submit_env).await.unwrap();

    // Act: Ack the order
    let ack_payload = serde_json::json!({
        "order_id": order_id,
        "exchange_order_id": "exchange-456"
    });
    let ack_env = make_envelope("order:test", "ORDER_ACKED", ack_payload, 2);
    apply_event_to_projections(&pool, &ack_env).await.unwrap();

    // Assert
    let (status, exchange_order_id, last_seq): (String, String, i64) = sqlx::query_as(
        "SELECT status, COALESCE(exchange_order_id, ''), last_seq FROM orders_current WHERE order_id = $1"
    )
    .bind(order_id)
    .fetch_one(&pool)
    .await?;

    assert_eq!(status, "acknowledged");
    assert_eq!(exchange_order_id, "exchange-456");
    assert_eq!(last_seq, 2);

    Ok(())
}

#[sqlx::test]
#[ignore = "requires DATABASE_URL (see file header for setup)"]
async fn test_position_opened_enforces_invariants(pool: sqlx::PgPool) -> sqlx::Result<()> {
    // Arrange
    let position_id = Uuid::new_v4();
    let tenant_id = Uuid::new_v4();
    let account_id = Uuid::new_v4();

    let payload = serde_json::json!({
        "position_id": position_id,
        "tenant_id": tenant_id,
        "account_id": account_id,
        "strategy_id": null,
        "symbol": "BTCUSDT",
        "side": "long",
        "entry_price": "50000",
        "entry_quantity": "0.1",
        "entry_filled_at": null,
        "technical_stop_price": "49000",
        "technical_stop_distance": "1000",
        "entry_order_id": null,
        "stop_loss_order_id": null
    });

    let envelope = make_envelope("position:test", "POSITION_OPENED", payload, 1);

    // Act
    let result = apply_event_to_projections(&pool, &envelope).await;

    // Assert - should succeed
    assert!(result.is_ok());

    let (state, trailing_stop): (String, Option<Decimal>) = sqlx::query_as(
        "SELECT state, trailing_stop_price FROM positions_current WHERE position_id = $1",
    )
    .bind(position_id)
    .fetch_one(&pool)
    .await?;

    assert_eq!(state, "armed");
    // INVARIANT: trailing_stop should be anchored to technical_stop_price
    assert_eq!(trailing_stop, Some(Decimal::from(49000)));

    Ok(())
}

#[sqlx::test]
#[ignore = "requires DATABASE_URL (see file header for setup)"]
async fn test_position_opened_rejects_zero_stop_distance(pool: sqlx::PgPool) -> sqlx::Result<()> {
    // Arrange - invalid payload with zero stop distance
    let position_id = Uuid::new_v4();
    let tenant_id = Uuid::new_v4();
    let account_id = Uuid::new_v4();

    let payload = serde_json::json!({
        "position_id": position_id,
        "tenant_id": tenant_id,
        "account_id": account_id,
        "strategy_id": null,
        "symbol": "BTCUSDT",
        "side": "long",
        "entry_price": "50000",
        "entry_quantity": "0.1",
        "entry_filled_at": null,
        "technical_stop_price": "50000",
        "technical_stop_distance": "0",
        "entry_order_id": null,
        "stop_loss_order_id": null
    });

    let envelope = make_envelope("position:test", "POSITION_OPENED", payload, 1);

    // Act
    let result = apply_event_to_projections(&pool, &envelope).await;

    // Assert - should fail invariant check
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(err.to_string().contains("InvariantViolated"));

    Ok(())
}

#[sqlx::test]
#[ignore = "requires DATABASE_URL (see file header for setup)"]
async fn test_fill_received_idempotent(pool: sqlx::PgPool) -> sqlx::Result<()> {
    // Arrange
    let order_id = Uuid::new_v4();
    let tenant_id = Uuid::new_v4();
    let account_id = Uuid::new_v4();

    // Submit order first
    let submit_payload = serde_json::json!({
        "order_id": order_id,
        "tenant_id": tenant_id,
        "account_id": account_id,
        "position_id": null,
        "client_order_id": "client-123",
        "symbol": "BTCUSDT",
        "side": "buy",
        "order_type": "limit",
        "quantity": "0.1",
        "price": "50000",
        "stop_price": null
    });
    let submit_env = make_envelope("order:test", "ORDER_SUBMITTED", submit_payload, 1);
    apply_event_to_projections(&pool, &submit_env).await.unwrap();

    let fill_payload = serde_json::json!({
        "fill_id": Uuid::new_v4(),
        "tenant_id": tenant_id,
        "account_id": account_id,
        "order_id": order_id,
        "exchange_order_id": "exchange-456",
        "exchange_trade_id": "trade-789",
        "symbol": "BTCUSDT",
        "side": "buy",
        "fill_price": "50000",
        "fill_quantity": "0.1",
        "fee": "0.001",
        "fee_asset": "BTC",
        "is_maker": true,
        "filled_at": Utc::now()
    });

    let fill_env = make_envelope("order:test", "FILL_RECEIVED", fill_payload.clone(), 2);

    // Act: Apply twice
    apply_event_to_projections(&pool, &fill_env).await.unwrap();
    apply_event_to_projections(&pool, &fill_env).await.unwrap();

    // Assert: filled_quantity should only be applied once
    let (filled_quantity,): (Decimal,) =
        sqlx::query_as("SELECT filled_quantity FROM orders_current WHERE order_id = $1")
            .bind(order_id)
            .fetch_one(&pool)
            .await?;

    assert_eq!(filled_quantity, Decimal::from(1)); // Not 2!

    Ok(())
}

#[sqlx::test]
#[ignore = "requires DATABASE_URL (see file header for setup)"]
async fn test_strategy_enabled(pool: sqlx::PgPool) -> sqlx::Result<()> {
    // Arrange
    let strategy_id = Uuid::new_v4();
    let tenant_id = Uuid::new_v4();
    let account_id = Uuid::new_v4();

    let payload = serde_json::json!({
        "strategy_id": strategy_id,
        "tenant_id": tenant_id,
        "account_id": account_id,
        "strategy_name": "Mean Reversion MA99",
        "strategy_type": "trend_following",
        "detector_config": null,
        "risk_config": {"max_risk_percent": "1.0"}
    });

    let envelope = make_envelope("strategy:test", "STRATEGY_ENABLED", payload, 1);

    // Act
    let result = apply_event_to_projections(&pool, &envelope).await;

    // Assert
    assert!(result.is_ok());

    let (is_enabled, strategy_name): (bool, String) = sqlx::query_as(
        "SELECT is_enabled, strategy_name FROM strategy_state_current WHERE strategy_id = $1",
    )
    .bind(strategy_id)
    .fetch_one(&pool)
    .await?;

    assert!(is_enabled);
    assert_eq!(strategy_name, "Mean Reversion MA99");

    Ok(())
}

#[sqlx::test]
#[ignore = "requires DATABASE_URL (see file header for setup)"]
async fn test_balance_sampled(pool: sqlx::PgPool) -> sqlx::Result<()> {
    // Arrange
    let balance_id = Uuid::new_v4();
    let tenant_id = Uuid::new_v4();
    let account_id = Uuid::new_v4();

    let payload = serde_json::json!({
        "balance_id": balance_id,
        "tenant_id": tenant_id,
        "account_id": account_id,
        "asset": "BTC",
        "free": "1.5",
        "locked": "0.5",
        "sampled_at": Utc::now()
    });

    let envelope = make_envelope("account:test", "BALANCE_SAMPLED", payload, 1);

    // Act
    let result = apply_event_to_projections(&pool, &envelope).await;

    // Assert
    assert!(result.is_ok());

    let (free, locked, total): (Decimal, Decimal, Decimal) =
        sqlx::query_as("SELECT free, locked, total FROM balances_current WHERE balance_id = $1")
            .bind(balance_id)
            .fetch_one(&pool)
            .await?;

    assert_eq!(free, Decimal::from(15) / Decimal::from(10)); // 1.5
    assert_eq!(locked, Decimal::from(5) / Decimal::from(10)); // 0.5
    assert_eq!(total, Decimal::from(2)); // 1.5 + 0.5

    Ok(())
}

#[sqlx::test]
#[ignore = "requires DATABASE_URL (see file header for setup)"]
async fn test_risk_check_failed(pool: sqlx::PgPool) -> sqlx::Result<()> {
    // Arrange
    let tenant_id = Uuid::new_v4();
    let account_id = Uuid::new_v4();

    let payload = serde_json::json!({
        "tenant_id": tenant_id,
        "account_id": account_id,
        "strategy_id": null,
        "violation_reason": "Daily loss limit exceeded"
    });

    let envelope = make_envelope("account:test", "RISK_CHECK_FAILED", payload, 1);

    // Act
    let result = apply_event_to_projections(&pool, &envelope).await;

    // Assert
    assert!(result.is_ok());

    let (is_violated, reason): (bool, Option<String>) = sqlx::query_as(
        "SELECT is_violated, violation_reason FROM risk_state_current WHERE account_id = $1",
    )
    .bind(account_id)
    .fetch_one(&pool)
    .await?;

    assert!(is_violated);
    assert_eq!(reason, Some("Daily loss limit exceeded".to_string()));

    Ok(())
}

#[sqlx::test]
#[ignore = "requires DATABASE_URL (see file header for setup)"]
async fn test_global_idempotency(pool: sqlx::PgPool) -> sqlx::Result<()> {
    // Regression test: inserting same event twice should not duplicate
    // Uses event_idempotency table for global idempotency by (tenant_id, idempotency_key)

    let tenant_id = Uuid::new_v4();
    let account_id = Uuid::new_v4();
    let idempotency_key = "global-idempotency-test";
    let stream_key = "account:global-test";

    // Helper to insert via event_idempotency
    let insert_via_idempotency = |stream_key: &str, seq: i64| -> sqlx::Result<Uuid> {
        let event_id = Uuid::new_v4();

        // First, insert into event_idempotency
        sqlx::query(
            r#"
            INSERT INTO event_idempotency (tenant_id, idempotency_key, event_id)
            VALUES ($1, $2, $3)
            ON CONFLICT (tenant_id, idempotency_key) DO NOTHING
            "#,
        )
        .bind(tenant_id)
        .bind(idempotency_key)
        .bind(event_id)
        .execute(&pool)
        .await?;

        // Check if this was a new insert
        let existing = sqlx::query_scalar::<_, Uuid>(
            "SELECT event_id FROM event_idempotency WHERE tenant_id = $1 AND idempotency_key = $2",
        )
        .bind(tenant_id)
        .bind(idempotency_key)
        .fetch_one(&pool)
        .await?;

        // Now insert into event_log
        let payload = serde_json::json!({
            "balance_id": Uuid::new_v4(),
            "tenant_id": tenant_id,
            "account_id": account_id,
            "asset": "USDT",
            "free": "2500.00",
            "locked": "0.00",
            "sampled_at": chrono::Utc::now()
        });

        sqlx::query(
            r#"
            INSERT INTO event_log (
                tenant_id, stream_key, seq, event_type, payload, payload_schema_version,
                occurred_at, ingested_at, idempotency_key, actor_type, actor_id, event_id
            ) VALUES ($1, $2, $3, 'BALANCE_SAMPLED', $4, 1,
                       NOW(), NOW(), $5, 'CLI', 'test-user', $6)
            "#,
        )
        .bind(tenant_id)
        .bind(stream_key)
        .bind(seq)
        .bind(payload)
        .bind(idempotency_key)
        .bind(event_id)
        .execute(&pool)
        .await?;

        Ok(existing)
    };

    // First insert - should succeed and create new row
    let event_id_1 = insert_via_idempotency(stream_key, 1).await?;
    assert!(event_id_1.is_some());

    // Verify event_log has 1 row
    let count_1: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM event_log WHERE tenant_id = $1 AND idempotency_key = $2",
    )
    .bind(tenant_id)
    .bind(idempotency_key)
    .fetch_one(&pool)
    .await?;
    assert_eq!(count_1, 1, "First insert should create 1 row");

    // Second insert - should be blocked by event_idempotency (ON CONFLICT DO NOTHING)
    let event_id_2 = insert_via_idempotency(stream_key, 2).await?;
    assert!(event_id_2.is_some());

    // Verify still only 1 row in event_log (second insert blocked by idempotency)
    let count_2: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM event_log WHERE tenant_id = $1 AND idempotency_key = $2",
    )
    .bind(tenant_id)
    .bind(idempotency_key)
    .fetch_one(&pool)
    .await?;
    assert_eq!(count_2, 1, "Second insert should NOT create duplicate; should still be 1 row");

    // Verify the event_id returned is the same (first insert)
    assert_eq!(event_id_1, event_id_2, "Both inserts should return same event_id");

    Ok(())
)
