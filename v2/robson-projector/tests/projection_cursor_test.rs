//! Testcontainers-based integration test for projection cursor behavior.
//!
//! This test verifies that the projection worker:
//! 1. Successfully applies valid events
//! 2. Blocks on invalid events (invariant violations)
//! 3. Does NOT advance cursor past failing events

use testcontainers::{Container, GenericImage, ImageExt, clients::Cli};
use tokio::time::{Duration, sleep};

use robson_eventlog::ActorType;
use robson_projector::apply_event_to_projections;
use robson_testkit::{seed_balance_sampled_event, seed_position_opened_event, setup_test_db};

/// Helper: Start PostgreSQL container and apply migrations
async fn setup_db() -> (sqlx::PgPool, Container<'static, GenericImage>) {
    let docker = Cli::default();
    let image = GenericImage::new("postgres", "16-alpine")
        .with_env_var("POSTGRES_USER", "test")
        .with_env_var("POSTGRES_PASSWORD", "test")
        .with_env_var("POSTGRES_DB", "test")
        .with_exposed_port(5432.tcp());

    let container = docker.run(image);
    let port = container.get_host_port_ipv4(5432).await;
    let database_url = format!("postgresql://test:test@localhost:{}/test", port);

    // Wait for Postgres to be ready
    sleep(Duration::from_secs(2)).await;

    let pool = sqlx::PgPool::connect(&database_url)
        .await
        .expect("Failed to connect to test database");

    // Apply migrations
    setup_test_db(&pool).await.expect("Failed to run migrations");

    (pool, container)
}

#[tokio::test]
async fn test_projection_cursor_blocks_on_invariant_failure() {
    // Setup: Start Postgres and apply migrations
    let (pool, _container) = setup_db().await;

    // Arrange: Generate test IDs
    let tenant_id = uuid::Uuid::new_v4();
    let account_id = uuid::Uuid::new_v4();

    // Act 1: Insert valid BALANCE_SAMPLED event (seq=1)
    let event1 = seed_balance_sampled_event(&pool, tenant_id, account_id, "USDT", "10000", "0")
        .await
        .expect("Failed to seed balance event");

    let result1 = apply_event_to_projections(&pool, &event1).await;
    assert!(result1.is_ok(), "Event 1 should succeed");

    // Assert 1: Check balances_current has the row
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM balances_current WHERE tenant_id = $1")
            .bind(tenant_id)
            .fetch_one(&pool)
            .await
            .expect("Failed to query balances_current");

    assert_eq!(count, 1, "balances_current should have 1 row after event 1");

    // Act 2: Insert invalid POSITION_OPENED event (stop_distance=0, seq=2)
    let event2 = seed_position_opened_event(
        &pool, tenant_id, account_id, "BTCUSDT", "95000", "0.1", "94000",
        "0", // INVARIANT VIOLATION: technical_stop_distance is 0
    )
    .await
    .expect("Failed to seed position event");

    let result2 = apply_event_to_projections(&pool, &event2).await;

    // Assert 2: Event 2 should fail with invariant error
    assert!(result2.is_err(), "Event 2 should fail invariant check");

    let err = result2.unwrap_err();
    let err_msg = err.to_string();
    assert!(
        err_msg.contains("technical_stop_distance") && err_msg.contains("non-zero"),
        "Error should mention technical_stop_distance: {}",
        err_msg
    );

    // Assert 3: Check positions_current has 0 rows (event 2 was NOT applied)
    let pos_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM positions_current WHERE tenant_id = $1")
            .bind(tenant_id)
            .fetch_one(&pool)
            .await
            .expect("Failed to query positions_current");

    assert_eq!(pos_count, 0, "positions_current should have 0 rows (event 2 failed)");

    // Assert 4: Verify cursor behavior - simulate worker loop
    let mut last_seq = 0i64;
    let mut applied_count = 0;

    // Simulate: Fetch events where seq > last_seq
    let stream_key = format!("account:{}", account_id);
    let events = sqlx::query(
        r#"
        SELECT event_id, tenant_id, stream_key, seq, event_type, payload,
               payload_schema_version, occurred_at, ingested_at, idempotency_key,
               trace_id, causation_id, command_id, workflow_id,
               actor_type, actor_id, prev_hash, hash
        FROM event_log
        WHERE tenant_id = $1
          AND stream_key = $2
          AND seq > $3
        ORDER BY seq ASC
        LIMIT 10
        "#,
    )
    .bind(tenant_id)
    .bind(&stream_key)
    .bind(last_seq)
    .fetch_all(&pool)
    .await
    .expect("Failed to fetch events");

    // Process events in order
    for event in events {
        let actor_type_str: Option<String> = event.try_get("actor_type").unwrap_or(None);
        let actor_type = match actor_type_str.as_deref() {
            Some("CLI") => Some(ActorType::CLI),
            Some("Daemon") => Some(ActorType::Daemon),
            Some("System") => Some(ActorType::System),
            Some("Exchange") => Some(ActorType::Exchange),
            _ => None,
        };

        let envelope = robson_eventlog::EventEnvelope {
            event_id: event.try_get("event_id").unwrap(),
            tenant_id: event.try_get("tenant_id").unwrap(),
            stream_key: event.try_get("stream_key").unwrap(),
            seq: event.try_get("seq").unwrap(),
            event_type: event.try_get("event_type").unwrap(),
            payload: event.try_get("payload").unwrap(),
            payload_schema_version: event.try_get("payload_schema_version").unwrap(),
            occurred_at: event.try_get("occurred_at").unwrap(),
            ingested_at: event.try_get("ingested_at").unwrap(),
            idempotency_key: event.try_get("idempotency_key").unwrap(),
            trace_id: event.try_get("trace_id").ok(),
            causation_id: event.try_get("causation_id").ok(),
            command_id: event.try_get("command_id").ok(),
            workflow_id: event.try_get("workflow_id").ok(),
            actor_type,
            actor_id: event.try_get("actor_id").unwrap(),
            prev_hash: event.try_get("prev_hash").ok(),
            hash: event.try_get("hash").ok(),
        };

        match apply_event_to_projections(&pool, &envelope).await {
            Ok(()) => {
                // Only advance cursor on success
                last_seq = envelope.seq;
                applied_count += 1;
            },
            Err(_) => {
                // Stop processing on error - cursor does NOT advance
                break;
            },
        }
    }

    // Final assertions about cursor behavior
    assert_eq!(last_seq, 1, "Cursor should stay at seq=1 (event 1 succeeded, event 2 failed)");
    assert_eq!(applied_count, 1, "Should have applied exactly 1 event");

    // Verify final database state
    let final_balance_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM balances_current WHERE tenant_id = $1")
            .bind(tenant_id)
            .fetch_one(&pool)
            .await
            .expect("Failed to query final balances");

    let final_position_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM positions_current WHERE tenant_id = $1")
            .bind(tenant_id)
            .fetch_one(&pool)
            .await
            .expect("Failed to query final positions");

    assert_eq!(final_balance_count, 1, "Final: 1 balance row");
    assert_eq!(final_position_count, 0, "Final: 0 position rows (invariant blocked event 2)");
}
