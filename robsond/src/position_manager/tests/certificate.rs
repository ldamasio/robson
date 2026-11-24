//! ADR-0056 step 8m-2a (8m PLAN-r4 §6.7, §8) on the manager's write paths:
//! while a session holds the cutover certificate's key exclusive, as
//! completion will, an event's append refuses at once with nothing written
//! and its stream not suspended, and the funding saga's state and the
//! month-peak upsert wait for completion and land after it.

use std::time::{Duration, Instant};

use robson_eventlog::certificate::{EXCLUSIVE, PARTICIPATE};
use sqlx::Connection as _;

use super::*;

/// Completion's stand-in: the key exclusive until `COMMIT`.
#[cfg(feature = "postgres")]
async fn hold(pool: &sqlx::PgPool) -> sqlx::PgConnection {
    let mut connection = sqlx::PgConnection::connect_with(&pool.connect_options()).await.unwrap();
    sqlx::query("BEGIN").execute(&mut connection).await.unwrap();
    sqlx::query(EXCLUSIVE).execute(&mut connection).await.unwrap();
    connection
}

#[cfg(feature = "postgres")]
async fn release(mut completion: sqlx::PgConnection) {
    sqlx::query("COMMIT").execute(&mut completion).await.unwrap();
    completion.close().await.unwrap();
}

/// Sessions waiting on an advisory lock in a statement containing `query`.
#[cfg(feature = "postgres")]
async fn waiting_in(pool: &sqlx::PgPool, query: &str) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM pg_stat_activity WHERE datname = current_database() \
         AND wait_event_type = 'Lock' AND wait_event = 'advisory' AND strpos(query, $1) > 0",
    )
    .bind(query)
    .fetch_one(pool)
    .await
    .unwrap()
}

#[cfg(feature = "postgres")]
async fn until_waiting_in(pool: &sqlx::PgPool, query: &str) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while waiting_in(pool, query).await < 1 {
        assert!(Instant::now() < deadline, "no session waits in {query}");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

#[cfg(feature = "postgres")]
fn manager_of(
    pool: &sqlx::PgPool,
    tenant_id: Uuid,
) -> Arc<PositionManager<StubExchange, MemoryStore>> {
    let order_journal = Arc::new(robson_exec::order_journal::OrderJournal::new(
        Arc::new(crate::order_journal_store::PgJournalBackend::new(pool, tenant_id)),
        &VenueId::new("test:venue"),
    ));
    let store = Arc::new(MemoryStore::new());
    let executor = Executor::new(
        Arc::new(StubExchange::new(dec!(95000))),
        Arc::new(IntentJournal::new()),
        store.clone(),
    )
    .with_order_journal(order_journal);
    Arc::new(
        PositionManager::with_approval_policy(
            Engine::new(RiskConfig::new(dec!(10000)).unwrap()),
            Arc::new(executor),
            store,
            Arc::new(EventBus::new(100)),
            Arc::new(TracingQueryRecorder),
            ApprovalPolicy::new(Decimal::from(100u32), 300),
            TradingPolicy::default(),
        )
        .with_event_log(pool.clone(), tenant_id),
    )
}

#[cfg(feature = "postgres")]
fn insurance_stop_failed(position_id: PositionId, error: &str) -> Event {
    Event::InsuranceStopFailed {
        position_id,
        stop_price: Price::new(dec!(94000)).unwrap(),
        error: error.to_string(),
        timestamp: chrono::Utc::now(),
    }
}

#[cfg(feature = "postgres")]
async fn count(pool: &sqlx::PgPool, sql: &str, key: Uuid) -> i64 {
    sqlx::query_scalar(sql).bind(key).fetch_one(pool).await.unwrap()
}

/// A refused append not retried (mutant) → this test. While completion holds
/// the key, a domain event's persistence keeps retrying its append under the
/// stream's lock: nothing in the log, the stream not suspended in memory or on
/// record, no latch, nothing queued for the journal. After completion the same
/// event lands, once, with no venue action repeated.
#[cfg(feature = "postgres")]
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_refused_append_lands_after_completion_without_suspending(pool: sqlx::PgPool) {
    let tenant_id = Uuid::now_v7();
    let manager = manager_of(&pool, tenant_id);
    let position_id = Uuid::now_v7();
    let rows = "SELECT count(*) FROM event_log WHERE stream_key = concat('position:', $1::uuid)";

    let completion = hold(&pool).await;
    let event = insurance_stop_failed(position_id, "during");
    let (persisted, ()) = tokio::join!(manager.persist_event_to_log(&event), async {
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert_eq!(count(&pool, rows, position_id).await, 0);
        assert!(!manager.event_streams.position_suspended(position_id));
        assert!(!manager.executor.order_journal().latch_set());
        assert_eq!(manager.executor.order_journal().retry_set_depth(), 0);
        release(completion).await;
    });
    persisted.unwrap();
    assert_eq!(count(&pool, rows, position_id).await, 1);
    let landed: String = sqlx::query_scalar(
        "SELECT payload->>'error' FROM event_log WHERE stream_key = concat('position:', $1::uuid)",
    )
    .bind(position_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(landed, "during", "the original event, not another");
    let suspensions = "SELECT count(*) FROM stream_suspensions WHERE tenant_id = $1";
    assert_eq!(count(&pool, suspensions, tenant_id).await, 0);
    assert!(!manager.event_streams.position_suspended(position_id));
}

/// A refusal past [`CERTIFICATE_RETRY_BOUND`], longer than completion may
/// hold the key, is a failure like any other: nothing appended, the stream
/// suspended and the latch set (fail closed).
#[cfg(feature = "postgres")]
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn an_append_refused_past_its_bound_fails_closed(pool: sqlx::PgPool) {
    let tenant_id = Uuid::now_v7();
    let manager = manager_of(&pool, tenant_id);
    let position_id = Uuid::now_v7();
    let completion = hold(&pool).await;
    let started = Instant::now();
    let failed = manager.persist_event_to_log(&insurance_stop_failed(position_id, "held")).await;
    assert!(matches!(failed, Err(DaemonError::EventLog(_))), "{failed:?}");
    assert!(started.elapsed() >= CERTIFICATE_RETRY_BOUND);
    assert!(manager.event_streams.position_suspended(position_id));
    assert!(manager.executor.order_journal().latch_set());
    release(completion).await;
    let rows = "SELECT count(*) FROM event_log WHERE stream_key = concat('position:', $1::uuid)";
    assert_eq!(count(&pool, rows, position_id).await, 0);
}

/// The saga's state upsert or the month-peak upsert left autocommit (mutants)
/// → this test. A saga whose append landed before completion's grant waits
/// for completion before its state row, which lands after the `COMMIT`; the
/// month-peak upsert waits the same way, and the peak lands after it.
#[cfg(feature = "postgres")]
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn saga_and_peak_writes_wait_for_completion(pool: sqlx::PgPool) {
    const HELD: i64 = 8_022_001;
    // The saga: its append pauses inside its insert, holding the key shared,
    // until completion has asked for the key.
    let exchange = Arc::new(StubExchange::new(dec!(50000)));
    exchange.set_futures_balance(dec!(10000));
    exchange.set_spot_balance("BTC", dec!(0.01), Decimal::ZERO);
    exchange.set_price("BTCUSDT", dec!(50000));
    let store = Arc::new(MemoryStore::new());
    let saga_manager = Arc::new(tokio::sync::RwLock::new(PositionManager::new(
        Engine::new(RiskConfig::new(dec!(10000)).unwrap()),
        Arc::new(Executor::new(exchange.clone(), Arc::new(IntentJournal::new()), store.clone())),
        store,
        Arc::new(EventBus::new(100)),
        Arc::new(TracingQueryRecorder),
        TradingPolicy::default(),
    )));
    let saga_tenant = Uuid::now_v7();
    let service = crate::funding::FundingService::new(
        Arc::new(pool.clone()),
        saga_tenant,
        exchange,
        saga_manager,
        crate::config::FundingConfig::default(),
    );
    inject_event_log_fault(
        &pool,
        "NEW.event_type = 'FundingQuoted'",
        &format!("PERFORM pg_advisory_lock({HELD}); PERFORM pg_advisory_unlock({HELD});"),
    )
    .await;
    let mut holder = sqlx::PgConnection::connect_with(&pool.connect_options()).await.unwrap();
    sqlx::query("SELECT pg_advisory_lock($1)")
        .bind(HELD)
        .execute(&mut holder)
        .await
        .unwrap();
    let sagas = "SELECT count(*) FROM funding_sagas WHERE tenant_id = $1";
    let driver = async {
        until_waiting_in(&pool, "INSERT INTO event_log").await;
        // Completion asks for the key behind the append's shared hold.
        let (completion, ()) = tokio::join!(hold(&pool), async {
            until_waiting_in(&pool, EXCLUSIVE).await;
            sqlx::query("SELECT pg_advisory_unlock($1)")
                .bind(HELD)
                .execute(&mut holder)
                .await
                .unwrap();
        });
        // Granted after the append's COMMIT: the state upsert waits.
        until_waiting_in(&pool, PARTICIPATE).await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(count(&pool, sagas, saga_tenant).await, 0, "the saga's state landed inside");
        release(completion).await;
    };
    let (quoted, ()) = tokio::join!(service.quote(), driver);
    restore_event_log_writes(&pool).await;
    quoted.unwrap();
    assert_eq!(count(&pool, sagas, saga_tenant).await, 1);

    // The month's peak, through its upsert (as the budget-model test drives
    // it): a manager with a pool reads equity from the database.
    let manager = create_postgres_test_manager(pool.clone());
    let now = chrono::Utc::now();
    let monthly = manager.load_monthly_state(now).await.unwrap();
    let peak = || async {
        sqlx::query_scalar::<_, Option<Decimal>>(
            "SELECT max(month_peak_net) FROM monthly_state WHERE year = $1 AND month = $2",
        )
        .bind(now.year())
        .bind(now.month() as i16)
        .fetch_one(&pool)
        .await
        .unwrap()
    };
    let before = peak().await;
    let completion = hold(&pool).await;
    let refresh = manager.refresh_month_peak_net_from_snapshot(now, &monthly, dec!(300));
    let (refreshed, ()) = tokio::join!(refresh, async {
        until_waiting_in(&pool, PARTICIPATE).await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(peak().await, before, "the peak landed inside completion's window");
        release(completion).await;
    });
    assert_eq!(refreshed.unwrap(), dec!(300));
    assert_eq!(peak().await, Some(dec!(300)));
}
