//! C1 calibration for the balance-account session bounds (PLAN r4 §6). Not a
//! gate: built only with the `calibration` feature and run on demand against a
//! disposable PostgreSQL 16, in two runs (seed, then measure) so the server can
//! be restarted in between and the first measured pass starts with empty
//! shared buffers.

#![cfg(feature = "calibration")]

use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

use chrono::{Datelike, Utc};
use robson_domain::MonthlyBudgetModel;
use robson_eventlog::{append_event, query_events, query_stream_in_seq_order, Event, QueryOptions};
use robson_exec::venue::{AccountKind, VenueId};
use robsond::{db_session::connect_daemon_pool, venue_ledger_store::PgVenueLedger};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

const DATABASE: &str = "robson_c1_calibration";
const TENANT: Uuid = Uuid::from_u128(0x0000_0000_0000_7000_8000_0000_0000_c101);
const ACCOUNT_STREAM: &str = "account:c1";
const ACCOUNT_EVENTS: i64 = 200_000;

fn url_for(database: &str) -> String {
    let url = std::env::var("DATABASE_URL").unwrap();
    format!("{}/{database}", url.rsplit_once('/').unwrap().0)
}

/// Seed the representative volume of PLAN r4 §6 C1 into a fresh database.
#[tokio::test]
#[ignore = "calibration seed; requires DATABASE_URL"]
async fn c1_seed() {
    let admin = PgPool::connect(&std::env::var("DATABASE_URL").unwrap()).await.unwrap();
    sqlx::query(&format!("DROP DATABASE IF EXISTS {DATABASE} WITH (FORCE)"))
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query(&format!("CREATE DATABASE {DATABASE}"))
        .execute(&admin)
        .await
        .unwrap();
    let pool = PgPool::connect(&url_for(DATABASE)).await.unwrap();
    sqlx::migrate!("../migrations").run(&pool).await.unwrap();
    let started = Instant::now();
    let statements = [
        // Eleven past months, so 1,000,000 events spread over 12 partitions.
        r#"DO $$ DECLARE i int; m timestamp; BEGIN
           FOR i IN 1..11 LOOP
             m := date_trunc('month', (now() AT TIME ZONE 'UTC') - make_interval(months => i));
             EXECUTE format('CREATE TABLE IF NOT EXISTS %I PARTITION OF event_log FOR VALUES FROM (%L) TO (%L)',
               'event_log_' || to_char(m, 'YYYY_MM'), m AT TIME ZONE 'UTC', (m + interval '1 month') AT TIME ZONE 'UTC');
           END LOOP; END $$"#,
        r#"INSERT INTO event_log (tenant_id, stream_key, seq, event_type, payload, occurred_at, ingested_at, idempotency_key)
           SELECT $1, 'account:c1', g,
                  (ARRAY['query_state_changed','entry_signal_received','trailing_stop_updated'])[1 + g % 3],
                  jsonb_build_object('n', g), t.ts, t.ts, 'c1-acct-' || g
           FROM generate_series(1, 200000) g,
                LATERAL (SELECT (date_trunc('month', now() AT TIME ZONE 'UTC') - interval '11 months') AT TIME ZONE 'UTC'
                                + (g % 28800) * interval '15 minutes' AS ts) t"#,
        r#"INSERT INTO event_log (tenant_id, stream_key, seq, event_type, payload, occurred_at, ingested_at, idempotency_key)
           SELECT $1, 'position:' || md5('c1-position-' || (g % 10000))::uuid, g / 10000 + 1,
                  (ARRAY['entry_filled','trailing_stop_updated','favorable_extreme_advanced','position_closed'])[1 + (g / 10000) % 4],
                  jsonb_build_object('n', g), t.ts, t.ts, 'c1-pos-' || g
           FROM generate_series(0, 799999) g,
                LATERAL (SELECT (date_trunc('month', now() AT TIME ZONE 'UTC') - interval '11 months') AT TIME ZONE 'UTC'
                                + (g % 28800) * interval '15 minutes' AS ts) t"#,
        r#"INSERT INTO stream_state (stream_key, tenant_id, last_seq)
           SELECT stream_key, $1, max(seq) FROM event_log WHERE tenant_id = $1 GROUP BY stream_key"#,
        r#"INSERT INTO positions_current (position_id, tenant_id, account_id, symbol, side, state,
               current_quantity, entry_price, entry_quantity, current_price, trailing_stop_price,
               favorable_extreme, extreme_at, entry_mode, approval_mode, last_event_id, last_seq,
               created_at, updated_at)
           SELECT md5('c1-position-' || n)::uuid, $1, md5('c1-account')::uuid, 'BTCUSDT', 'long',
                  (ARRAY['armed','active','closed','closed','closed'])[1 + n % 5], 1, 100, 1, 100, 90,
                  100, now() - (n % 720) * interval '1 day',
                  CASE WHEN n % 5 = 0 THEN 'immediate' END, CASE WHEN n % 5 = 0 THEN 'auto' END,
                  md5('c1-last-' || n)::uuid, 80,
                  now() - (n % 720) * interval '1 day', now() - (n % 720) * interval '1 day'
           FROM generate_series(0, 9999) n"#,
        r#"INSERT INTO stream_suspensions (suspension_id, tenant_id, stream_key, stream_kind, failed_seq, error, suspended_at)
           SELECT md5('c1-susp-' || n)::uuid, $1, 'position:' || md5('c1-position-' || n)::uuid,
                  'position', 2, 'c1', now() - n * interval '1 minute'
           FROM generate_series(0, 1999) n"#,
        r#"INSERT INTO monthly_state (year, month, capital_base, month_peak_net, monthly_budget_model, created_at)
           SELECT EXTRACT(year FROM m)::smallint, EXTRACT(month FROM m)::smallint, 1000, 0, 'hwm_v1', m
           FROM (SELECT date_trunc('month', now()) - make_interval(months => i) AS m FROM generate_series(1, 24) i) s
           ON CONFLICT (year, month) DO NOTHING"#,
        r#"INSERT INTO projection_checkpoints (projection_name, tenant_id, stream_key, last_seq, updated_at)
           SELECT 'robson-projector', $1, stream_key, last_seq, now() FROM stream_state WHERE tenant_id = $1"#,
        r#"INSERT INTO order_intents (intent_id, tenant_id, venue_id, account_scope, client_order_id, kind, params,
               venue_outcome, created_at)
           SELECT md5('c1-intent-' || n)::uuid, $1, CASE WHEN n % 2 = 0 THEN 'binance_usdm' ELSE 'test:venue' END,
                  'operated', CASE WHEN n < 1000 THEN 'c1q-' ELSE 'c1-' END || n, 'exit', '{}',
                  CASE WHEN n < 1000 THEN NULL ELSE 'success' END, now() - n * interval '1 minute'
           FROM generate_series(0, 99999) n"#,
        r#"INSERT INTO income_ledger (exchange_income_id, symbol, income_type, amount, asset, income_time, matched_at)
           SELECT 'c1-' || n, 'BTCUSDT', 'COMMISSION', 0.1, 'USDT', now() - interval '2 days' - n * interval '1 second',
                  CASE WHEN n < 1000 THEN NULL ELSE now() END
           FROM generate_series(0, 99999) n"#,
        r#"INSERT INTO queries_current (query_id, tenant_id, stream_key, state, started_at, snapshot,
               last_event_id, last_seq, updated_at)
           SELECT md5('c1-query-' || n)::uuid, $1, 'account:c1',
                  CASE WHEN n < 500 THEN 'AwaitingApproval' ELSE 'Completed' END,
                  now() - n * interval '1 minute', '{}', md5('c1-qe-' || n)::uuid, n, now()
           FROM generate_series(0, 9999) n"#,
        r#"INSERT INTO funding_sagas (tenant_id, saga_id, state, quote, created_at, updated_at)
           SELECT $1, md5('c1-saga-' || n)::uuid, CASE WHEN n < 500 THEN 'TRANSFERRING' ELSE 'QUOTED' END,
                  '{}', now() - n * interval '1 minute', now()
           FROM generate_series(0, 9999) n"#,
        "ANALYZE",
    ];
    for sql in statements {
        let q = sqlx::query(sql);
        let q = if sql.contains("$1") {
            q.bind(TENANT)
        } else {
            q
        };
        q.execute(&pool).await.unwrap_or_else(|e| panic!("seed failed: {e}\n{sql}"));
    }
    // A predecessor that never stopped clean, so the measured start stamps.
    robsond::process_epoch::start_epoch(&pool, TENANT, "c1-seed").await.unwrap();
    let counts: (i64, i64, i64, i64, i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM event_log), (SELECT count(*) FROM positions_current), \
         (SELECT count(*) FROM order_intents), (SELECT count(*) FROM income_ledger), \
         (SELECT count(*) FROM queries_current), (SELECT count(*) FROM funding_sagas), \
         (SELECT count(*) FROM stream_suspensions)",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    eprintln!("C1_SEED counts={counts:?} seconds={:.1}", started.elapsed().as_secs_f64());
    assert_eq!(counts, (1_000_000, 10_000, 100_000, 100_000, 10_000, 10_000, 2_000));
}

struct Recorder {
    pass: &'static str,
}

impl Recorder {
    fn record(&self, op: &str, kind: &str, elapsed: Duration, rows: i64) {
        eprintln!(
            "C1_MEASURE {}",
            json!({"pass": self.pass, "op": op, "kind": kind,
                   "ms": elapsed.as_secs_f64() * 1000.0, "rows": rows})
        );
    }
}

/// Time `$body`, record it as `$op` of `$kind` with the row count `$rows`
/// computes from its value, and return the value.
macro_rules! timed {
    ($rec:expr, $op:expr, $kind:expr, $rows:expr, $body:expr) => {{
        let started = Instant::now();
        let value = $body;
        let elapsed = started.elapsed();
        let rows: i64 = $rows(&value);
        $rec.record($op, $kind, elapsed, rows);
        value
    }};
}

/// The partition name of the month `ahead` months from now, as the partition
/// functions name it.
fn month_partition(prefix: &str, ahead: i32) -> String {
    let now = Utc::now();
    let total = now.year() * 12 + now.month0() as i32 + ahead;
    format!("{prefix}_{:04}_{:02}", total / 12, total % 12 + 1)
}

/// Unstamp the qualifying intents and drop the partition the measured calls
/// must create; never measured.
async fn reset(plain: &PgPool) {
    sqlx::query(
        "UPDATE order_intents SET uncertain_generation = NULL, uncertain_at = NULL \
         WHERE client_order_id LIKE 'c1q-%'",
    )
    .execute(plain)
    .await
    .unwrap();
    for parent in ["event_log", "snapshots"] {
        let name = month_partition(parent, 3);
        sqlx::query(&format!("DROP TABLE IF EXISTS {name}"))
            .execute(plain)
            .await
            .unwrap();
    }
}

async fn measure_pass(root: &PgPool, plain: &PgPool, pass: &'static str, round: i32) {
    reset(plain).await;
    let rec = Recorder { pass };
    let month_start = Utc::now()
        .date_naive()
        .with_day(1)
        .unwrap()
        .and_hms_opt(0, 0, 0)
        .unwrap()
        .and_utc();
    let stream: String = sqlx::query_scalar(
        "SELECT stream_key FROM stream_state WHERE stream_key LIKE 'position:%' \
         ORDER BY stream_key LIMIT 1",
    )
    .fetch_one(plain)
    .await
    .unwrap();
    timed!(rec, "find_active_from_projection", "statement", |v: &Vec<_>| v.len() as i64, {
        robson_store::find_active_from_projection(root, TENANT).await.unwrap()
    });
    timed!(rec, "find_armed_entry_policies", "statement", |v: &Vec<_>| v.len() as i64, {
        robson_store::find_armed_entry_policies(root, TENANT).await.unwrap()
    });
    timed!(
        rec,
        "find_positions_overlapping_month",
        "statement",
        |v: &Vec<_>| v.len() as i64,
        {
            robson_store::find_positions_overlapping_month(root, TENANT, month_start)
                .await
                .unwrap()
        }
    );
    timed!(
        rec,
        "query_events_tenant_limit_1000",
        "statement",
        |v: &Vec<_>| v.len() as i64,
        {
            let mut options = QueryOptions::new(TENANT);
            options.limit = Some(1000);
            query_events(root, options).await.unwrap()
        }
    );
    timed!(rec, "query_events_stream", "statement", |v: &Vec<_>| v.len() as i64, {
        let mut options = QueryOptions::new(TENANT);
        options.stream_key = Some(stream.clone());
        query_events(root, options).await.unwrap()
    });
    timed!(rec, "query_stream_in_seq_order", "statement", |v: &Vec<_>| v.len() as i64, {
        query_stream_in_seq_order(root, TENANT, &stream, &["entry_filled", "position_closed"])
            .await
            .unwrap()
    });
    timed!(
        rec,
        "suspensions_load",
        "statement",
        |v: &Vec<(Uuid, String, Option<i64>)>| v.len() as i64,
        {
            sqlx::query_as::<_, (Uuid, String, Option<i64>)>(
                "SELECT suspension_id, stream_key, repaired_through_seq FROM stream_suspensions \
             WHERE tenant_id = $1 AND cleared_at IS NULL ORDER BY suspended_at",
            )
            .bind(TENANT)
            .fetch_all(root)
            .await
            .unwrap()
        }
    );
    timed!(
        rec,
        "repair_page_types",
        "statement",
        |v: &Vec<(i64, String)>| v.len() as i64,
        {
            sqlx::query_as::<_, (i64, String)>(
                "SELECT seq, event_type FROM event_log \
             WHERE tenant_id = $1 AND stream_key = $2 AND seq > $3 ORDER BY seq LIMIT $4",
            )
            .bind(TENANT)
            .bind(&stream)
            .bind(0_i64)
            .bind(500_i64)
            .fetch_all(root)
            .await
            .unwrap()
        }
    );
    timed!(rec, "replay_page_account_from_0", "statement", |v: &Vec<_>| v.len() as i64, {
        sqlx::query(
            "SELECT event_id, tenant_id, stream_key, seq, event_type, payload, \
             payload_schema_version, occurred_at, ingested_at, idempotency_key, trace_id, \
             causation_id, command_id, workflow_id, actor_type, actor_id, prev_hash, hash \
             FROM event_log WHERE tenant_id = $1 AND stream_key = $2 AND seq > $3 \
             ORDER BY seq ASC LIMIT 1000",
        )
        .bind(TENANT)
        .bind(ACCOUNT_STREAM)
        .bind(0_i64)
        .fetch_all(root)
        .await
        .unwrap()
    });
    timed!(rec, "replay_page_account_from_mid", "statement", |v: &Vec<_>| v.len() as i64, {
        sqlx::query(
            "SELECT event_id, tenant_id, stream_key, seq, event_type, payload FROM event_log \
             WHERE tenant_id = $1 AND stream_key = $2 AND seq > $3 ORDER BY seq ASC LIMIT 1000",
        )
        .bind(TENANT)
        .bind(ACCOUNT_STREAM)
        .bind(ACCOUNT_EVENTS / 2)
        .fetch_all(root)
        .await
        .unwrap()
    });
    timed!(rec, "projection_cursor", "statement", |v: &Option<i64>| v.is_some() as i64, {
        sqlx::query_scalar::<_, i64>(
            "SELECT last_seq FROM projection_checkpoints \
             WHERE projection_name = $1 AND tenant_id = $2 AND stream_key = $3",
        )
        .bind("robson-projector")
        .bind(TENANT)
        .bind(ACCOUNT_STREAM)
        .fetch_optional(root)
        .await
        .unwrap()
    });
    let start = timed!(
        rec,
        "start_epoch_unclean",
        "transaction",
        |v: &robsond::process_epoch::EpochStart| v.stamped_intents as i64,
        { robsond::process_epoch::start_epoch(root, TENANT, "c1").await.unwrap() }
    );
    assert_eq!(start.stamped_intents, 1_000, "{pass}: the unclean start stamps 1,000 intents");
    for (op, parent) in [
        ("create_event_log_partitions", "event_log"),
        ("create_snapshot_partitions", "snapshots"),
    ] {
        timed!(rec, op, "transaction", |_: &()| 1, {
            sqlx::query(&format!("SELECT {op}(3)")).execute(root).await.unwrap();
        });
        let name = month_partition(parent, 3);
        let exists: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_class WHERE relname = $1)")
                .bind(&name)
                .fetch_one(plain)
                .await
                .unwrap();
        assert!(exists, "{pass}: {op} must create the missing {name}");
    }
    // The monthly projector's explicit-lock path, for a month with no row
    // (`robson-projector/src/handlers/monthly_state.rs`, add_to_month_counters).
    let year = (2031 + round) as i16;
    timed!(rec, "monthly_explicit_lock_path", "transaction", |_: &()| 1, {
        const ADD: &str = "UPDATE monthly_state SET trades_opened = trades_opened + $3, \
                           realized_loss = realized_loss + $4 WHERE year = $1 AND month = $2";
        let updated = sqlx::query(ADD)
            .bind(year)
            .bind(1_i16)
            .bind(1_i32)
            .bind(rust_decimal::Decimal::ZERO)
            .execute(root)
            .await
            .unwrap();
        assert_eq!(updated.rows_affected(), 0);
        let mut tx = root.begin().await.unwrap();
        sqlx::query("LOCK TABLE monthly_state IN SHARE ROW EXCLUSIVE MODE")
            .execute(&mut *tx)
            .await
            .unwrap();
        sqlx::query(ADD)
            .bind(year)
            .bind(1_i16)
            .bind(1_i32)
            .bind(rust_decimal::Decimal::ZERO)
            .execute(&mut *tx)
            .await
            .unwrap();
        tx.commit().await.unwrap();
    });
    timed!(rec, "schema_readiness", "statement", |v: &i64| *v, {
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(DISTINCT column_name)::BIGINT FROM information_schema.columns \
             WHERE table_schema = current_schema() AND table_name = 'positions_current' \
             AND column_name IN ('initial_executable_stop','executable_span','cap_basis_distance',\
             'tick_size_at_admission','stop_plan_entry_reference')",
        )
        .fetch_one(root)
        .await
        .unwrap()
    });
    let anomalies = timed!(rec, "income_anomaly_count", "statement", |v: &usize| *v as i64, {
        robsond::income_ledger::count_confirmed_anomalies(root, chrono::Duration::minutes(5))
            .await
            .unwrap()
    });
    assert_eq!(anomalies, 1_000);
    let pending =
        timed!(rec, "restart_pending_queries", "statement", |v: &Vec<_>| v.len() as i64, {
            sqlx::query(
                "SELECT snapshot FROM queries_current WHERE tenant_id = $1 AND stream_key = $2 \
                 AND state = 'AwaitingApproval' ORDER BY last_seq ASC",
            )
            .bind(TENANT)
            .bind(ACCOUNT_STREAM)
            .fetch_all(root)
            .await
            .unwrap()
        });
    assert_eq!(pending.len(), 500);
    let blocked = timed!(rec, "blocked_funding_count", "statement", |v: &i64| *v, {
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM funding_sagas WHERE tenant_id = $1 \
             AND state NOT IN ('QUOTED', 'REFRESHED', 'FAILED')",
        )
        .bind(TENANT)
        .fetch_one(root)
        .await
        .unwrap()
    });
    assert_eq!(blocked, 500);
    let ledger = PgVenueLedger::new(root, TENANT);
    let venue = VenueId::new(format!("test:c1-{pass}"));
    timed!(rec, "ledger_ensure_accounting_row", "transaction", |_: &()| 1, {
        ledger.ensure_accounting_row(&venue, MonthlyBudgetModel::HwmV1).await.unwrap();
    });
    // Eight appends racing on one stream: each holds its stream_state row
    // until COMMIT, so the slowest is the longest lock hold of the burst.
    let shared = format!("account:c1-race-{pass}");
    let mut tasks = Vec::new();
    for n in 0..8 {
        let root = root.clone();
        let shared = shared.clone();
        tasks.push(tokio::spawn(async move {
            let started = Instant::now();
            let event = Event::new(
                TENANT,
                shared.as_str(),
                "c1_race",
                json!({"n": n, "id": Uuid::now_v7()}),
            );
            append_event(&root, &shared, None, event).await.unwrap();
            started.elapsed()
        }));
    }
    for task in tasks {
        rec.record("append_event_contended", "transaction", task.await.unwrap(), 1);
    }
}

/// Measure every C1 operation through the bounded root pool: cold (the first
/// pass after the server restart), warm, then under four concurrent writers.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "calibration measurement; requires DATABASE_URL and a prior c1_seed"]
async fn c1_measure() {
    let url = url_for(DATABASE);
    let root = connect_daemon_pool(&url, AccountKind::BalanceAccount).await.unwrap();
    let plain = PgPool::connect(&url).await.unwrap();
    measure_pass(&root, &plain, "cold", 0).await;
    measure_pass(&root, &plain, "warm", 1).await;
    let stop = Arc::new(AtomicBool::new(false));
    let mut writers = Vec::new();
    for w in 0..4 {
        let writer_pool = connect_daemon_pool(&url, AccountKind::BalanceAccount).await.unwrap();
        let stop = stop.clone();
        writers.push(tokio::spawn(async move {
            let ledger = PgVenueLedger::new(&writer_pool, TENANT);
            let venue = VenueId::new("test:c1-writers");
            ledger.ensure_accounting_row(&venue, MonthlyBudgetModel::HwmV1).await.unwrap();
            let mut n = 0_u64;
            while !stop.load(Ordering::Relaxed) {
                let event = Event::new(
                    TENANT,
                    "account:c1-writers",
                    "c1_writer",
                    json!({"w": w, "n": n, "id": Uuid::now_v7()}),
                );
                append_event(&writer_pool, "account:c1-writers", None, event).await.unwrap();
                // A ledger-shaped write: local bounds, then the accounting bump.
                let mut tx = writer_pool.begin().await.unwrap();
                sqlx::query(
                    "SELECT set_config('lock_timeout', '300ms', true), \
                     set_config('statement_timeout', '1s', true)",
                )
                .execute(&mut *tx)
                .await
                .unwrap();
                sqlx::query(
                    "UPDATE accounting_state SET version = version + 1 \
                     WHERE tenant_id = $1 AND venue_id = $2",
                )
                .bind(TENANT)
                .bind(venue.as_str())
                .execute(&mut *tx)
                .await
                .unwrap();
                tx.commit().await.unwrap();
                n += 1;
            }
            n
        }));
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    measure_pass(&root, &plain, "concurrent", 2).await;
    stop.store(true, Ordering::Relaxed);
    let mut writes = 0;
    for writer in writers {
        writes += writer.await.unwrap();
    }
    eprintln!("C1_WRITERS iterations={writes}");
    assert!(writes > 0, "the concurrent pass needs real writers");
}
