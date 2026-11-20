//! A balance-account daemon's sessions end their own lock waits, statements
//! and idle transactions on the server (PLAN r4 §6: T2–T6, T9). Every holder,
//! waiter and observer runs on its own connection with the server defaults;
//! nothing here changes a server-wide setting.

#![cfg(feature = "postgres")]

use std::{
    fmt::Debug,
    time::{Duration, Instant},
};

use chrono::{DateTime, Utc};
use proptest::{prelude::*, strategy::ValueTree, test_runner::TestRunner};
use robson_eventlog::{ActorType, Event, EventEnvelope, QueryOptions};
use robson_exec::{order_journal::JournalBackend, venue::AccountKind};
use robsond::{
    db_session::{balance_session_options, connect_daemon_pool, SessionBounds},
    order_journal_store::PgJournalBackend,
};
use sqlx::{
    postgres::{PgConnectOptions, PgPoolOptions},
    ConnectOptions, Connection, PgConnection, PgPool,
};
use uuid::Uuid;

const SHOW: &str = "SELECT current_setting('lock_timeout'), current_setting('statement_timeout'), \
                    current_setting('idle_in_transaction_session_timeout')";
const LOCK_BOUND: Duration = Duration::from_secs(2);
/// A client-side guard well past every server bound, so a missing bound fails
/// a test instead of hanging it.
const GUARD: Duration = Duration::from_secs(10);
/// Short bounds for the lifecycle and generated cases, so they stay fast.
const SHORT: SessionBounds = SessionBounds {
    lock_timeout_ms: 100,
    statement_timeout_ms: 300,
    idle_in_transaction_ms: 400,
};

/// The `sqlx::test` database, addressed through `DATABASE_URL`.
fn url(pool: &PgPool) -> String {
    let database = pool.connect_options().get_database().unwrap().to_string();
    let base = std::env::var("DATABASE_URL").unwrap();
    format!("{}/{database}", base.rsplit_once('/').unwrap().0)
}

fn options(pool: &PgPool) -> PgConnectOptions {
    url(pool).parse().unwrap()
}

/// The daemon's root pool for a balance account.
async fn root(pool: &PgPool) -> PgPool {
    connect_daemon_pool(&url(pool), AccountKind::BalanceAccount).await.unwrap()
}

/// A pool from the same option builder with one connection: every acquire
/// returns the same physical connection unless it was discarded.
async fn single(pool: &PgPool, bounds: SessionBounds) -> PgPool {
    PgPoolOptions::new()
        .max_connections(1)
        .connect_with(balance_session_options(options(pool), bounds))
        .await
        .unwrap()
}

async fn settings(connection: &mut PgConnection) -> (String, String, String, i32) {
    let (lock, statement, idle): (String, String, String) =
        sqlx::query_as(SHOW).fetch_one(&mut *connection).await.unwrap();
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(connection)
        .await
        .unwrap();
    (lock, statement, idle, pid)
}

fn shown(bounds: SessionBounds) -> (String, String, String) {
    let show = |ms: u64| {
        if ms % 1_000 == 0 {
            format!("{}s", ms / 1_000)
        } else {
            format!("{ms}ms")
        }
    };
    (
        show(bounds.lock_timeout_ms),
        show(bounds.statement_timeout_ms),
        show(bounds.idle_in_transaction_ms),
    )
}

/// The SQLSTATE of a server cancellation, from the code when the error keeps
/// it, or from the server's own message when a wrapper kept only the text.
fn sqlstate(error: &dyn Debug) -> String {
    let text = format!("{error:?}");
    for (code, message) in [
        ("55P03", "canceling statement due to lock timeout"),
        ("57014", "canceling statement due to statement timeout"),
        ("25P03", "idle-in-transaction timeout"),
    ] {
        if text.contains(code) || text.contains(message) {
            return code.to_string();
        }
    }
    text
}

/// `work` and how long it took, failing the test if no server bound ends it
/// within [`GUARD`].
async fn timed<T>(what: &str, work: impl std::future::Future<Output = T>) -> (T, Duration) {
    let started = Instant::now();
    match tokio::time::timeout(GUARD, work).await {
        Ok(value) => (value, started.elapsed()),
        Err(_) => panic!("{what}: no server bound ended it within {GUARD:?}"),
    }
}

/// A lock wait ended by the server's lock bound: `55P03`, after about 2 s.
fn assert_lock_bound(what: &str, error: &dyn Debug, elapsed: Duration) {
    assert_eq!(sqlstate(error), "55P03", "{what}: {error:?}");
    assert!(
        elapsed >= LOCK_BOUND - Duration::from_millis(200)
            && elapsed < LOCK_BOUND + Duration::from_secs(1),
        "{what}: {elapsed:?}"
    );
}

fn event(tenant: Uuid, stream: &str) -> Event {
    Event::new(
        tenant,
        stream,
        "session_bounds_test",
        serde_json::json!({ "id": Uuid::now_v7() }),
    )
}

fn envelope(
    tenant: Uuid,
    event_type: &str,
    payload: serde_json::Value,
    at: DateTime<Utc>,
) -> EventEnvelope {
    let stream = format!("test:session-bounds:{}", Uuid::now_v7());
    EventEnvelope {
        event_id: Uuid::now_v7(),
        tenant_id: tenant,
        stream_key: stream.clone(),
        seq: 1,
        event_type: event_type.to_string(),
        payload,
        payload_schema_version: 1,
        occurred_at: at,
        ingested_at: at,
        idempotency_key: format!("{stream}:1:{event_type}"),
        trace_id: None,
        causation_id: None,
        command_id: None,
        workflow_id: None,
        actor_type: Some(ActorType::CLI),
        actor_id: Some("session-bounds-test".to_string()),
        prev_hash: None,
        hash: None,
    }
}

fn entry_filled(tenant: Uuid, position_id: Uuid, at: DateTime<Utc>) -> EventEnvelope {
    let payload = serde_json::json!({
        "position_id": position_id,
        "order_id": Uuid::now_v7(),
        "fill_price": "100",
        "filled_quantity": "1",
        "fee": "0.1",
        "initial_stop": "90",
        "invalidation_guard_level": null,
        "timestamp": at,
    });
    envelope(tenant, "entry_filled", payload, at)
}

async fn insert_position(pool: &PgPool, tenant: Uuid) -> Uuid {
    let position_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO positions_current (position_id, tenant_id, account_id, symbol, side, state, \
         current_quantity, last_event_id, last_seq, created_at, updated_at) \
         VALUES ($1, $2, $3, 'BTCUSDT', 'long', 'entering', 1, $4, 1, NOW(), NOW())",
    )
    .bind(position_id)
    .bind(tenant)
    .bind(Uuid::now_v7())
    .bind(Uuid::now_v7())
    .execute(pool)
    .await
    .unwrap();
    position_id
}

// T2: writes under real contention end at the server's lock bound.

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn t2a_append_event_waits_end_at_the_lock_bound(pool: PgPool) {
    let tenant = Uuid::now_v7();
    let root = root(&pool).await;
    let stream = format!("position:{}", Uuid::now_v7());
    robson_eventlog::append_event(&pool, &stream, None, event(tenant, &stream))
        .await
        .unwrap();
    let mut holder = pool.begin().await.unwrap();
    sqlx::query("SELECT 1 FROM stream_state WHERE stream_key = $1 FOR UPDATE")
        .bind(&stream)
        .execute(&mut *holder)
        .await
        .unwrap();
    let (result, elapsed) = timed(
        "append_event",
        robson_eventlog::append_event(&root, &stream, None, event(tenant, &stream)),
    )
    .await;
    holder.rollback().await.unwrap();
    assert_lock_bound("append_event", &result.unwrap_err(), elapsed);
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM event_log WHERE stream_key = $1")
        .bind(&stream)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 1, "nothing written by the refused append");
    robson_eventlog::append_event(&root, &stream, None, event(tenant, &stream))
        .await
        .unwrap();
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn t2b_start_epoch_waits_end_at_the_lock_bound(pool: PgPool) {
    let root = root(&pool).await;
    let mut holder = pool.begin().await.unwrap();
    sqlx::query("LOCK TABLE process_epochs IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *holder)
        .await
        .unwrap();
    let (result, elapsed) =
        timed("start_epoch", robsond::process_epoch::start_epoch(&root, Uuid::now_v7(), "t2b"))
            .await;
    holder.rollback().await.unwrap();
    assert_lock_bound("start_epoch", &result.unwrap_err(), elapsed);
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM process_epochs")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

/// The partition `ahead` months from now, named as the partition functions name
/// it.
fn month_partition(parent: &str, ahead: i32) -> String {
    use chrono::Datelike;
    let now = Utc::now();
    let total = now.year() * 12 + now.month0() as i32 + ahead;
    format!("{parent}_{:04}_{:02}", total / 12, total % 12 + 1)
}

async fn partition_wait_ends_at_the_lock_bound(pool: PgPool, parent: &str, function: &str) {
    let root = root(&pool).await;
    let missing = month_partition(parent, 3);
    sqlx::query(&format!("DROP TABLE IF EXISTS {missing}"))
        .execute(&pool)
        .await
        .unwrap();
    let exists = |pool: PgPool, name: String| async move {
        sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM pg_class WHERE relname = $1)")
            .bind(name)
            .fetch_one(&pool)
            .await
            .unwrap()
    };
    assert!(!exists(pool.clone(), missing.clone()).await);
    let mut holder = pool.begin().await.unwrap();
    sqlx::query(&format!("LOCK TABLE {parent} IN ACCESS EXCLUSIVE MODE"))
        .execute(&mut *holder)
        .await
        .unwrap();
    let call = format!("SELECT {function}(3)");
    let (result, elapsed) = timed(function, sqlx::query(&call).execute(&root)).await;
    holder.rollback().await.unwrap();
    assert_lock_bound(function, &result.unwrap_err(), elapsed);
    assert!(
        !exists(pool.clone(), missing.clone()).await,
        "{missing} created despite the refusal"
    );
    sqlx::query(&format!("SELECT {function}(3)")).execute(&root).await.unwrap();
    assert!(exists(pool.clone(), missing).await);
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn t2c1_event_log_partition_waits_end_at_the_lock_bound(pool: PgPool) {
    partition_wait_ends_at_the_lock_bound(pool, "event_log", "create_event_log_partitions").await;
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn t2c2_snapshot_partition_waits_end_at_the_lock_bound(pool: PgPool) {
    partition_wait_ends_at_the_lock_bound(pool, "snapshots", "create_snapshot_partitions").await;
}

/// The month-peak write of `position_manager.rs`, verbatim.
const MONTH_PEAK: &str = r#"
    INSERT INTO monthly_state (
        year, month, capital_base, month_peak_net,
        monthly_budget_model, created_at
    )
    VALUES (
        $1, $2, $3, $4,
        COALESCE(
            (SELECT monthly_budget_model
             FROM monthly_state
             WHERE (year, month) < ($1::SMALLINT, $2::SMALLINT)
             ORDER BY year DESC, month DESC
             LIMIT 1),
            'hwm_v1'
        ),
        NOW()
    )
    ON CONFLICT (year, month) DO UPDATE SET
        month_peak_net = GREATEST(monthly_state.month_peak_net, EXCLUDED.month_peak_net)
"#;

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn t2d_monthly_peak_write_waits_end_at_the_lock_bound(pool: PgPool) {
    let root = root(&pool).await;
    let decimal = |n: i64| rust_decimal::Decimal::new(n, 0);
    sqlx::query(MONTH_PEAK)
        .bind(2030_i32)
        .bind(5_i16)
        .bind(decimal(1000))
        .bind(decimal(10))
        .execute(&pool)
        .await
        .unwrap();
    let mut holder = pool.begin().await.unwrap();
    sqlx::query("SELECT 1 FROM monthly_state WHERE year = 2030 AND month = 5 FOR UPDATE")
        .execute(&mut *holder)
        .await
        .unwrap();
    let (result, elapsed) = timed(
        "monthly peak write",
        sqlx::query(MONTH_PEAK)
            .bind(2030_i32)
            .bind(5_i16)
            .bind(decimal(1000))
            .bind(decimal(20))
            .execute(&root),
    )
    .await;
    holder.rollback().await.unwrap();
    assert_lock_bound("monthly peak write", &result.unwrap_err(), elapsed);
    let peak: rust_decimal::Decimal = sqlx::query_scalar(
        "SELECT month_peak_net FROM monthly_state WHERE year = 2030 AND month = 5",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(peak, decimal(10));
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn t2e_suspension_repair_write_waits_end_at_the_lock_bound(pool: PgPool) {
    let root = root(&pool).await;
    let suspension = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO stream_suspensions (suspension_id, tenant_id, stream_key, stream_kind, \
         failed_seq, error, suspended_at) VALUES ($1, $2, $3, 'position', 2, 't2e', NOW())",
    )
    .bind(suspension)
    .bind(Uuid::now_v7())
    .bind(format!("position:{}", Uuid::now_v7()))
    .execute(&pool)
    .await
    .unwrap();
    let mut holder = pool.begin().await.unwrap();
    sqlx::query("SELECT 1 FROM stream_suspensions WHERE suspension_id = $1 FOR UPDATE")
        .bind(suspension)
        .execute(&mut *holder)
        .await
        .unwrap();
    // The repair's clearing write of `event_streams.rs`, verbatim.
    let (result, elapsed) = timed(
        "suspension clear",
        sqlx::query(
            "UPDATE stream_suspensions SET cleared_at = NOW(), repair_error = NULL \
             WHERE suspension_id = $1",
        )
        .bind(suspension)
        .execute(&root),
    )
    .await;
    holder.rollback().await.unwrap();
    assert_lock_bound("suspension clear", &result.unwrap_err(), elapsed);
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn t2f_projection_checkpoint_waits_end_at_the_lock_bound(pool: PgPool) {
    let root = root(&pool).await;
    let tenant = Uuid::now_v7();
    // The checkpoint write of `projection_worker.rs`, verbatim.
    const CHECKPOINT: &str = "INSERT INTO projection_checkpoints (projection_name, tenant_id, \
         stream_key, last_seq, updated_at) VALUES ($1, $2, $3, $4, NOW()) \
         ON CONFLICT (projection_name, tenant_id, stream_key) DO UPDATE SET \
         last_seq = EXCLUDED.last_seq, updated_at = EXCLUDED.updated_at";
    let write = |pool: PgPool, seq: i64| async move {
        sqlx::query(CHECKPOINT)
            .bind("robson-projector")
            .bind(tenant)
            .bind("account:t2f")
            .bind(seq)
            .execute(&pool)
            .await
    };
    write(pool.clone(), 1).await.unwrap();
    let mut holder = pool.begin().await.unwrap();
    sqlx::query("SELECT 1 FROM projection_checkpoints WHERE tenant_id = $1 FOR UPDATE")
        .bind(tenant)
        .execute(&mut *holder)
        .await
        .unwrap();
    let (result, elapsed) = timed("projection checkpoint", write(root.clone(), 2)).await;
    holder.rollback().await.unwrap();
    assert_lock_bound("projection checkpoint", &result.unwrap_err(), elapsed);
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn t2g_projector_position_handler_waits_end_at_the_lock_bound(pool: PgPool) {
    let root = root(&pool).await;
    let tenant = Uuid::now_v7();
    let position_id = insert_position(&pool, tenant).await;
    let mut holder = pool.begin().await.unwrap();
    sqlx::query("SELECT 1 FROM positions_current WHERE position_id = $1 FOR UPDATE")
        .bind(position_id)
        .execute(&mut *holder)
        .await
        .unwrap();
    let (result, elapsed) = timed(
        "projector entry_filled",
        robson_projector::apply_event_to_projections(
            &root,
            &entry_filled(tenant, position_id, Utc::now()),
        ),
    )
    .await;
    holder.rollback().await.unwrap();
    assert_lock_bound("projector entry_filled", &result.unwrap_err(), elapsed);
    let state: String =
        sqlx::query_scalar("SELECT state FROM positions_current WHERE position_id = $1")
            .bind(position_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(state, "entering");
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn t2h_monthly_projector_explicit_lock_waits_end_at_the_lock_bound(pool: PgPool) {
    let root = root(&pool).await;
    // A month with no row takes the projector's explicit-lock path. ROW
    // EXCLUSIVE lets its first UPDATE through and blocks its LOCK TABLE.
    let at: DateTime<Utc> = "2031-03-15T12:00:00Z".parse().unwrap();
    let mut holder = pool.begin().await.unwrap();
    sqlx::query("LOCK TABLE monthly_state IN ROW EXCLUSIVE MODE")
        .execute(&mut *holder)
        .await
        .unwrap();
    let projector = {
        let root = root.clone();
        tokio::spawn(async move {
            timed(
                "monthly explicit lock",
                robson_projector::apply_event_to_projections(
                    &root,
                    &entry_filled(Uuid::now_v7(), Uuid::now_v7(), at),
                ),
            )
            .await
        })
    };
    // Observe the wait on the explicit SHARE ROW EXCLUSIVE request itself.
    let mut seen_waiting = false;
    for _ in 0..150 {
        seen_waiting = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM pg_locks WHERE relation = 'monthly_state'::regclass \
             AND mode = 'ShareRowExclusiveLock' AND NOT granted)",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        if seen_waiting {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let (result, elapsed) = projector.await.unwrap();
    holder.rollback().await.unwrap();
    assert!(seen_waiting, "the projector never reached its explicit LOCK TABLE");
    assert_lock_bound("monthly explicit lock", &result.unwrap_err(), elapsed);
}

// T3: reads under an ACCESS EXCLUSIVE holder end at the server's lock bound.

async fn read_under_holder<T: Debug>(
    pool: &PgPool,
    table: &str,
    read: impl std::future::Future<Output = T>,
) -> (T, Duration) {
    let mut holder = pool.begin().await.unwrap();
    sqlx::query(&format!("LOCK TABLE {table} IN ACCESS EXCLUSIVE MODE"))
        .execute(&mut *holder)
        .await
        .unwrap();
    let (value, elapsed) = timed(table, read).await;
    holder.rollback().await.unwrap();
    (value, elapsed)
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn t3_reads_under_an_exclusive_holder_end_at_the_lock_bound(pool: PgPool) {
    let root = root(&pool).await;
    let tenant = Uuid::now_v7();
    let (result, elapsed) = read_under_holder(
        &pool,
        "event_log",
        robson_eventlog::query_events(&root, QueryOptions::new(tenant)),
    )
    .await;
    assert_lock_bound("event-log query", &result.unwrap_err(), elapsed);
    let (result, elapsed) = read_under_holder(
        &pool,
        "positions_current",
        robson_store::find_active_from_projection(&root, tenant),
    )
    .await;
    assert_lock_bound("store recovery read", &result.unwrap_err(), elapsed);
    // The funding list of `funding/saga.rs`, verbatim.
    let (result, elapsed) = read_under_holder(
        &pool,
        "funding_sagas",
        sqlx::query(
            "SELECT saga_id, state, estimated_usdt, created_at FROM funding_sagas \
             WHERE tenant_id = $1 ORDER BY created_at DESC LIMIT 100",
        )
        .bind(tenant)
        .fetch_all(&root),
    )
    .await;
    assert_lock_bound("funding list", &result.unwrap_err(), elapsed);
    // The pending-recovery count of `api.rs`, verbatim.
    let (result, elapsed) = read_under_holder(
        &pool,
        "order_intents",
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM order_intents WHERE tenant_id = $1 \
             AND uncertain_generation IS NOT NULL AND venue_outcome IS NULL AND NOT not_sent \
             AND disposition_kind IS NULL",
        )
        .bind(tenant)
        .fetch_one(&root),
    )
    .await;
    assert_lock_bound("pending recovery count", &result.unwrap_err(), elapsed);
    // The journal's readiness probe runs on its own pool, cloned from the root
    // pool's options: unbounded, it would wait out its 10 s readiness window.
    let backend = PgJournalBackend::new(&root, tenant);
    let (result, elapsed) = read_under_holder(&pool, "order_intents", backend.check_ready()).await;
    let error = result.unwrap_err();
    assert!(error.contains("lock timeout"), "journal readiness: {error}");
    assert!(elapsed < LOCK_BOUND + Duration::from_secs(1), "journal readiness: {elapsed:?}");
}

// T4: lifecycle of the session bounds on reused and replacement connections.

#[sqlx::test(migrations = false)]
#[ignore = "requires DATABASE_URL"]
async fn t4a_a_local_override_reverts_on_the_same_connection(pool: PgPool) {
    let single = single(&pool, SessionBounds::BALANCE).await;
    let expected = shown(SessionBounds::BALANCE);
    for commit in [true, false] {
        let mut connection = single.acquire().await.unwrap();
        let (_, _, _, pid) = settings(&mut connection).await;
        let mut tx = connection.begin().await.unwrap();
        sqlx::query("SET LOCAL lock_timeout = '300ms'").execute(&mut *tx).await.unwrap();
        let local: String = sqlx::query_scalar("SELECT current_setting('lock_timeout')")
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        assert_eq!(local, "300ms");
        if commit {
            tx.commit().await.unwrap();
        } else {
            tx.rollback().await.unwrap();
        }
        drop(connection);
        let mut again = single.acquire().await.unwrap();
        let (lock, statement, idle, again_pid) = settings(&mut again).await;
        assert_eq!(again_pid, pid, "the same physical connection");
        assert_eq!((lock, statement, idle), expected.clone(), "commit={commit}");
    }
}

#[sqlx::test(migrations = false)]
#[ignore = "requires DATABASE_URL"]
async fn t4b_a_cancelled_statement_leaves_its_connection_reusable(pool: PgPool) {
    let single = single(&pool, SHORT).await;
    let mut connection = single.acquire().await.unwrap();
    let (_, _, _, pid) = settings(&mut connection).await;
    let error = sqlx::query("SELECT pg_sleep(1)").execute(&mut *connection).await.unwrap_err();
    assert_eq!(sqlstate(&error), "57014");
    drop(connection);
    let mut again = single.acquire().await.unwrap();
    let (lock, statement, idle, again_pid) = settings(&mut again).await;
    assert_eq!(again_pid, pid, "a cancelled statement does not cost the connection");
    assert_eq!((lock, statement, idle), shown(SHORT));
}

#[sqlx::test(migrations = false)]
#[ignore = "requires DATABASE_URL"]
async fn t4c_an_idle_terminated_session_is_replaced_bounded(pool: PgPool) {
    let single = single(&pool, SHORT).await;
    let mut connection = single.acquire().await.unwrap();
    let (_, _, _, pid) = settings(&mut connection).await;
    let mut tx = connection.begin().await.unwrap();
    sqlx::query("SELECT 1").execute(&mut *tx).await.unwrap();
    tokio::time::sleep(Duration::from_millis(SHORT.idle_in_transaction_ms + 500)).await;
    let error = sqlx::query("SELECT 1").execute(&mut *tx).await.unwrap_err();
    assert!(!format!("{error:?}").is_empty());
    drop(tx);
    drop(connection);
    let mut again = single.acquire().await.unwrap();
    let (lock, statement, idle, again_pid) = settings(&mut again).await;
    assert_ne!(again_pid, pid, "the terminated session is replaced");
    assert_eq!((lock, statement, idle), shown(SHORT));
}

// T5: a transaction abandoned while holding a business lock is ended by the
// server, with no client cleanup at all.

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn t5_an_abandoned_lock_holder_is_ended_by_the_server(pool: PgPool) {
    let stream = format!("position:{}", Uuid::now_v7());
    robson_eventlog::append_event(&pool, &stream, None, event(Uuid::now_v7(), &stream))
        .await
        .unwrap();
    let mut abandoned = balance_session_options(options(&pool), SessionBounds::BALANCE)
        .connect()
        .await
        .unwrap();
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut abandoned)
        .await
        .unwrap();
    sqlx::query("BEGIN").execute(&mut abandoned).await.unwrap();
    sqlx::query("UPDATE stream_state SET last_seq = last_seq WHERE stream_key = $1")
        .bind(&stream)
        .execute(&mut abandoned)
        .await
        .unwrap();
    let abandoned_at = Instant::now();
    // No ROLLBACK, no close: the client simply stops.
    std::mem::forget(abandoned);
    let waiter = {
        let pool = pool.clone();
        let stream = stream.clone();
        tokio::spawn(async move {
            sqlx::query("UPDATE stream_state SET last_seq = last_seq WHERE stream_key = $1")
                .bind(&stream)
                .execute(&pool)
                .await
                .unwrap();
            Instant::now()
        })
    };
    let deadline = Duration::from_millis(SessionBounds::BALANCE.idle_in_transaction_ms + 1_000);
    let gone = loop {
        let alive: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE pid = $1)")
                .bind(pid)
                .fetch_one(&pool)
                .await
                .unwrap();
        if !alive {
            break abandoned_at.elapsed();
        }
        assert!(
            abandoned_at.elapsed() < deadline,
            "the abandoned backend outlived its idle bound"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    let proceeded = waiter.await.unwrap().duration_since(abandoned_at);
    assert!(gone < deadline, "{gone:?}");
    assert!(proceeded < deadline, "the waiter proceeded only after {proceeded:?}");
}

// T6: a statement ends at the server's statement bound.

#[sqlx::test(migrations = false)]
#[ignore = "requires DATABASE_URL"]
async fn t6_a_statement_ends_at_the_statement_bound(pool: PgPool) {
    let root = root(&pool).await;
    let (result, elapsed) =
        timed("pg_sleep(6)", sqlx::query("SELECT pg_sleep(6)").execute(&root)).await;
    let error = result.unwrap_err();
    assert_eq!(sqlstate(&error), "57014");
    assert!(
        elapsed >= Duration::from_millis(4_900) && elapsed < Duration::from_secs(6),
        "{elapsed:?}"
    );
    let mut connection = root.acquire().await.unwrap();
    let (lock, statement, idle, _) = settings(&mut connection).await;
    assert_eq!((lock, statement, idle), shown(SessionBounds::BALANCE));
}

// T9: generated interleavings of the lifecycle on a two-connection pool.

#[derive(Debug, Clone, Copy)]
enum Step {
    Plain,
    Cancel,
    Abandon,
    Wait,
    Commit,
    Rollback,
}

fn step() -> impl Strategy<Value = Step> {
    prop_oneof![
        Just(Step::Plain),
        Just(Step::Cancel),
        Just(Step::Abandon),
        Just(Step::Wait),
        Just(Step::Commit),
        Just(Step::Rollback),
    ]
}

async fn run_step(
    step: Step,
    bounded: &PgPool,
    plain: &PgPool,
    url_options: &PgConnectOptions,
    stream: &str,
) {
    let slack = Duration::from_millis(500);
    match step {
        Step::Plain => {
            sqlx::query("SELECT 1").execute(bounded).await.unwrap();
        },
        Step::Cancel => {
            let (result, elapsed) =
                timed("pg_sleep(2)", sqlx::query("SELECT pg_sleep(2)").execute(bounded)).await;
            assert_eq!(sqlstate(&result.unwrap_err()), "57014");
            assert!(elapsed < Duration::from_millis(SHORT.statement_timeout_ms) + slack);
        },
        Step::Abandon => {
            let mut abandoned =
                balance_session_options(url_options.clone(), SHORT).connect().await.unwrap();
            sqlx::query("BEGIN").execute(&mut abandoned).await.unwrap();
            sqlx::query("UPDATE stream_state SET last_seq = last_seq WHERE stream_key = $1")
                .bind(stream)
                .execute(&mut abandoned)
                .await
                .unwrap();
            let abandoned_at = Instant::now();
            std::mem::forget(abandoned);
            timed(
                "waiter behind an abandoned holder",
                sqlx::query("UPDATE stream_state SET last_seq = last_seq WHERE stream_key = $1")
                    .bind(stream)
                    .execute(plain),
            )
            .await
            .0
            .unwrap();
            assert!(
                abandoned_at.elapsed()
                    < Duration::from_millis(SHORT.idle_in_transaction_ms) + slack,
                "an abandoned holder kept its lock for {:?}",
                abandoned_at.elapsed()
            );
        },
        Step::Wait => {
            let mut holder = plain.begin().await.unwrap();
            sqlx::query("SELECT 1 FROM stream_state WHERE stream_key = $1 FOR UPDATE")
                .bind(stream)
                .execute(&mut *holder)
                .await
                .unwrap();
            let (result, waited) = timed(
                "waiter",
                sqlx::query("UPDATE stream_state SET last_seq = last_seq WHERE stream_key = $1")
                    .bind(stream)
                    .execute(bounded),
            )
            .await;
            let error = result.unwrap_err();
            holder.rollback().await.unwrap();
            assert_eq!(sqlstate(&error), "55P03");
            assert!(waited < Duration::from_millis(SHORT.lock_timeout_ms) + slack, "{waited:?}");
        },
        Step::Commit | Step::Rollback => {
            let mut tx = bounded.begin().await.unwrap();
            sqlx::query("UPDATE stream_state SET last_seq = last_seq WHERE stream_key = $1")
                .bind(stream)
                .execute(&mut *tx)
                .await
                .unwrap();
            if matches!(step, Step::Commit) {
                tx.commit().await.unwrap();
            } else {
                tx.rollback().await.unwrap();
            }
        },
    }
    // Every connection of the pool still reports the bounds and still works.
    let (mut first, mut second) =
        (bounded.acquire().await.unwrap(), bounded.acquire().await.unwrap());
    for connection in [&mut first, &mut second] {
        let (lock, statement, idle, _) = settings(connection).await;
        assert_eq!((lock, statement, idle), shown(SHORT), "after {step:?}");
    }
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn t9_generated_lifecycle_keeps_every_session_bounded(pool: PgPool) {
    let url_options = options(&pool);
    let stream = format!("position:{}", Uuid::now_v7());
    robson_eventlog::append_event(&pool, &stream, None, event(Uuid::now_v7(), &stream))
        .await
        .unwrap();
    let mut runner = TestRunner::default();
    let sequences = proptest::collection::vec(step(), 1..7);
    for case in 0..12 {
        let sequence = sequences.new_tree(&mut runner).unwrap().current();
        eprintln!("T9 case {case}: {sequence:?}");
        let bounded = PgPoolOptions::new()
            .max_connections(2)
            .connect_with(balance_session_options(url_options.clone(), SHORT))
            .await
            .unwrap();
        for step in sequence {
            run_step(step, &bounded, &pool, &url_options, &stream).await;
        }
        bounded.close().await;
    }
}
