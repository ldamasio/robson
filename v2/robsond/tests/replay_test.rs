//! Integration tests for deterministic query audit replay.

#![cfg(feature = "postgres")]

use anyhow::Result;
use chrono::{DateTime, TimeZone, Utc};
use robson_domain::{Price, Side, Symbol};
use robson_eventlog::{
    ActorType, Event, EventEnvelope, QUERY_STATE_CHANGED_EVENT_TYPE, QueryOptions, append_event,
    query_events,
};
use robson_projector::apply_event_to_projections;
use robsond::{
    ActorKind, ExecutionQuery, QueryKind, QueryOutcome, QueryState, QueryStateChangedEvent,
};
use rust_decimal_macros::dec;
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, FromRow)]
struct ProjectedQueryRow {
    query_id: Uuid,
    tenant_id: Uuid,
    stream_key: String,
    position_id: Option<Uuid>,
    state: String,
    started_at: DateTime<Utc>,
    finished_at: Option<DateTime<Utc>>,
    snapshot: serde_json::Value,
    last_event_id: Uuid,
    last_seq: i64,
    updated_at: DateTime<Utc>,
}

fn ts(hour: u32, minute: u32, second: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 4, 5, hour, minute, second).single().unwrap()
}

fn signal_query(query_id: Uuid, position_id: Uuid, started_at: DateTime<Utc>) -> ExecutionQuery {
    let mut query = ExecutionQuery::new(
        QueryKind::ProcessSignal {
            signal_id: Uuid::from_u128(0xC1),
            symbol: Symbol::from_pair("BTCUSDT").unwrap(),
            side: Side::Long,
            entry_price: Price::new(dec!(95000)).unwrap(),
            stop_loss: Price::new(dec!(85500)).unwrap(),
        },
        ActorKind::Detector,
    );
    query.id = query_id;
    query.position_id = Some(position_id);
    query.started_at = started_at;
    query.finished_at = None;
    query.outcome = None;
    query.approval = None;
    query
}

fn market_tick_query(
    query_id: Uuid,
    position_id: Uuid,
    started_at: DateTime<Utc>,
) -> ExecutionQuery {
    let mut query = ExecutionQuery::new(
        QueryKind::ProcessMarketTick {
            symbol: Symbol::from_pair("BTCUSDT").unwrap(),
            price: Price::new(dec!(91000)).unwrap(),
        },
        ActorKind::MarketData,
    );
    query.id = query_id;
    query.position_id = Some(position_id);
    query.started_at = started_at;
    query.finished_at = None;
    query.outcome = None;
    query.approval = None;
    query
}

async fn append_and_project(
    pool: &sqlx::PgPool,
    tenant_id: Uuid,
    stream_key: &str,
    query: &ExecutionQuery,
    transition_cause: &str,
    occurred_at: DateTime<Utc>,
) -> Result<()> {
    let payload = QueryStateChangedEvent::from_query(query, transition_cause);
    let mut event = Event::new(
        tenant_id,
        stream_key,
        QUERY_STATE_CHANGED_EVENT_TYPE,
        serde_json::to_value(payload)?,
    )
    .with_actor(ActorType::Daemon, Some("replay-test".to_string()));
    event.occurred_at = occurred_at;

    let event_id = append_event(pool, stream_key, None, event).await?;
    let envelope: EventEnvelope = sqlx::query_as("SELECT * FROM event_log WHERE event_id = $1")
        .bind(event_id)
        .fetch_one(pool)
        .await?;
    apply_event_to_projections(pool, &envelope).await?;
    Ok(())
}

async fn load_queries_current(pool: &sqlx::PgPool) -> Result<Vec<ProjectedQueryRow>> {
    let rows = sqlx::query_as::<_, ProjectedQueryRow>(
        r#"
        SELECT
            query_id,
            tenant_id,
            stream_key,
            position_id,
            state,
            started_at,
            finished_at,
            snapshot,
            last_event_id,
            last_seq,
            updated_at
        FROM queries_current
        ORDER BY query_id ASC
        "#,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "Requires DATABASE_URL to be set"]
async fn test_query_replay_rebuilds_queries_current_byte_for_byte(pool: sqlx::PgPool) {
    let tenant_id = Uuid::from_u128(0x300);
    let stream_key = "robson:daemon:phase4:replay";

    let query_a_id = Uuid::from_u128(0x301);
    let query_b_id = Uuid::from_u128(0x302);
    let query_c_id = Uuid::from_u128(0x303);
    let position_a = Uuid::from_u128(0x311);
    let position_b = Uuid::from_u128(0x312);
    let position_c = Uuid::from_u128(0x313);

    let mut query_a = signal_query(query_a_id, position_a, ts(10, 0, 0));
    append_and_project(&pool, tenant_id, stream_key, &query_a, "accepted", ts(10, 0, 1))
        .await
        .unwrap();
    query_a.transition(QueryState::Processing).unwrap();
    append_and_project(&pool, tenant_id, stream_key, &query_a, "processing", ts(10, 0, 2))
        .await
        .unwrap();
    query_a.transition(QueryState::RiskChecked).unwrap();
    append_and_project(&pool, tenant_id, stream_key, &query_a, "risk_checked", ts(10, 0, 3))
        .await
        .unwrap();
    query_a.await_approval("manual approval".to_string(), 300).unwrap();
    query_a.approval.as_mut().unwrap().expires_at = ts(10, 5, 0);
    append_and_project(&pool, tenant_id, stream_key, &query_a, "awaiting_approval", ts(10, 0, 4))
        .await
        .unwrap();
    query_a.authorize().unwrap();
    query_a.approval.as_mut().unwrap().approved_at = Some(ts(10, 0, 5));
    append_and_project(&pool, tenant_id, stream_key, &query_a, "authorized", ts(10, 0, 5))
        .await
        .unwrap();
    query_a.transition(QueryState::Acting).unwrap();
    append_and_project(&pool, tenant_id, stream_key, &query_a, "acting", ts(10, 0, 6))
        .await
        .unwrap();
    query_a.complete(QueryOutcome::ActionsExecuted { actions_count: 1 }).unwrap();
    query_a.finished_at = Some(ts(10, 0, 7));
    append_and_project(&pool, tenant_id, stream_key, &query_a, "completed", ts(10, 0, 7))
        .await
        .unwrap();

    let mut query_b = signal_query(query_b_id, position_b, ts(11, 0, 0));
    append_and_project(&pool, tenant_id, stream_key, &query_b, "accepted", ts(11, 0, 1))
        .await
        .unwrap();
    query_b.transition(QueryState::Processing).unwrap();
    append_and_project(&pool, tenant_id, stream_key, &query_b, "processing", ts(11, 0, 2))
        .await
        .unwrap();
    query_b.transition(QueryState::RiskChecked).unwrap();
    append_and_project(&pool, tenant_id, stream_key, &query_b, "risk_checked", ts(11, 0, 3))
        .await
        .unwrap();
    query_b.deny("max open positions".to_string(), "max_open_positions".to_string());
    query_b.finished_at = Some(ts(11, 0, 4));
    append_and_project(&pool, tenant_id, stream_key, &query_b, "risk_denied", ts(11, 0, 4))
        .await
        .unwrap();

    let mut query_c = market_tick_query(query_c_id, position_c, ts(12, 0, 0));
    append_and_project(&pool, tenant_id, stream_key, &query_c, "accepted", ts(12, 0, 1))
        .await
        .unwrap();
    query_c.transition(QueryState::Processing).unwrap();
    append_and_project(&pool, tenant_id, stream_key, &query_c, "processing", ts(12, 0, 2))
        .await
        .unwrap();
    query_c.fail("exchange unavailable".to_string(), "acting".to_string());
    query_c.finished_at = Some(ts(12, 0, 3));
    append_and_project(&pool, tenant_id, stream_key, &query_c, "failed", ts(12, 0, 3))
        .await
        .unwrap();

    let baseline = load_queries_current(&pool).await.unwrap();
    assert_eq!(baseline.len(), 3);

    let events = query_events(
        &pool,
        QueryOptions::new(tenant_id)
            .stream(stream_key)
            .event_type(QUERY_STATE_CHANGED_EVENT_TYPE),
    )
    .await
    .unwrap();
    assert_eq!(events.len(), 14);

    sqlx::query("TRUNCATE TABLE queries_current").execute(&pool).await.unwrap();

    for envelope in &events {
        apply_event_to_projections(&pool, envelope).await.unwrap();
    }

    let replayed = load_queries_current(&pool).await.unwrap();
    assert_eq!(baseline, replayed);
}
