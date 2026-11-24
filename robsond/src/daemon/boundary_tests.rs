//! ADR-0056 step 8p-2 proofs (8p PLAN-r3 §8): a balance account's boundary
//! ownership on PostgreSQL 16. R1's routing and its USD-M control (the
//! zero-basis PLAN's test 6), the pending rows first at startup (its test 5)
//! and at the poll, none before the certified activation, several boundaries
//! in one write, the pre-certificate zero reset once, no version moved
//! without a row, a position account's legacy boundary, the command id
//! pinned, each transaction's statements by proxy trace, and a lost apply
//! `COMMIT` answer retiring its connection raw. 8p-O: each of those
//! statements counted under its operation's root.
//!
//! The balance daemon runs on the stub venue's identity (`stub`) with no
//! generation installed; the proxy test installs one on Binance Spot's.
use std::future::Future;

use chrono::{NaiveDate, TimeZone};
use robson_exec::{
    order_journal::OrderJournal,
    stub::STUB_VENUE_ID,
    venue::{AccountKind, AccountPort},
};
use robson_testkit::db_proxy::{DbProxy, ProxyMode};
use sqlx::{postgres::PgPoolOptions, PgPool};
use tracing::instrument::WithSubscriber as _;
use uuid::Uuid;

use super::*;
use crate::{
    balance_accounting::{BalanceAccounting, GenerationCell},
    boundary::{command_id, read_month, reset_command_id, write_pending, zero_reset},
    capability::worker_venue::WorkerVenue,
    circuit_breaker::HaltState,
    event_streams::EventStreams,
    operation_observation::{job, probe::Probe, Job, JOB_METRICS},
    spot_order_set_store::JournalGeneration,
};

const STUB: &str = STUB_VENUE_ID;
const SPOT: &str = robson_exec::venue::VenueId::BINANCE_SPOT;

fn at(year: i32, month: u32, day: u32, hour: u32, minute: u32, second: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(year, month, day, hour, minute, second).single().unwrap()
}

fn day(year: i32, month: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, 1).unwrap()
}

/// The accounting row of `tenant` on `venue`, at version 0.
async fn account(pool: &PgPool, tenant: Uuid, venue: &str) {
    sqlx::query(
        "INSERT INTO accounting_state (tenant_id, venue_id, budget_model) \
         VALUES ($1, $2, 'net_from_start_non_expanding_v1')",
    )
    .bind(tenant)
    .bind(venue)
    .execute(pool)
    .await
    .unwrap();
}

/// 8m's rows of `tenant` on `venue` (fixtures, as 8p PLAN-r3 §8 allows): a
/// cutover anchor and its activation basis of `month` at 0, pending, or
/// certified at `as_of` when one is given.
async fn activation(pool: &PgPool, tenant: Uuid, venue: &str, month: &str, as_of: Option<&str>) {
    let cutover = Uuid::now_v7();
    // Their cutover run (M56's foreign keys).
    sqlx::query(include_str!("../../tests/fixtures/cutover_run.sql"))
        .bind(venue)
        .bind(tenant)
        .bind(cutover)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO cash_anchors (anchor_id, tenant_id, venue_id, kind, cutover_id, quote) \
         VALUES ($1, $2, $3, 'cutover', $4, 1805)",
    )
    .bind(Uuid::now_v7())
    .bind(tenant)
    .bind(venue)
    .bind(cutover)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO activation_bases (tenant_id, venue_id, cutover_id, month, previous_basis, \
         quote_total, basis, state, created_at) \
         VALUES ($1, $2, $3, $4::date, 0, 1805, 0, 'pending', $4::date + INTERVAL '12 hours')",
    )
    .bind(tenant)
    .bind(venue)
    .bind(cutover)
    .bind(month)
    .execute(pool)
    .await
    .unwrap();
    if let Some(as_of) = as_of {
        sqlx::query(
            "UPDATE activation_bases SET state = 'certified', certified_at = $2::timestamptz, \
             as_of = $2::timestamptz WHERE cutover_id = $1",
        )
        .bind(cutover)
        .bind(as_of)
        .execute(pool)
        .await
        .unwrap();
    }
}

async fn version(pool: &PgPool, tenant: Uuid, venue: &str) -> i64 {
    sqlx::query_scalar(
        "SELECT version FROM accounting_state WHERE (tenant_id, venue_id) = ($1, $2) /* 8p-2 */",
    )
    .bind(tenant)
    .bind(venue)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// `tenant`'s snapshot rows by month: month, `as_of`, state, `pending_since`.
async fn snapshots(
    pool: &PgPool,
    tenant: Uuid,
) -> Vec<(NaiveDate, DateTime<Utc>, String, DateTime<Utc>)> {
    sqlx::query_as(
        "SELECT month, as_of, state, pending_since FROM boundary_snapshots \
         WHERE tenant_id = $1 ORDER BY month",
    )
    .bind(tenant)
    .fetch_all(pool)
    .await
    .unwrap()
}

/// `tenant`'s stored resets of a month: base, carried risk and command id.
async fn resets(
    pool: &PgPool,
    tenant: Uuid,
    year: i32,
    month: u32,
) -> Vec<(Decimal, Decimal, Option<Uuid>)> {
    sqlx::query_as(
        "SELECT (payload->>'capital_base')::numeric, (payload->>'carried_positions_risk')::numeric, \
         command_id FROM event_log WHERE tenant_id = $1 AND event_type = 'month_boundary_reset' \
         AND (payload->>'year')::int = $2 AND (payload->>'month')::int = $3 ORDER BY seq",
    )
    .bind(tenant)
    .bind(year)
    .bind(month as i32)
    .fetch_all(pool)
    .await
    .unwrap()
}

/// The month's row: base, reset time and realized loss.
async fn month_row(
    pool: &PgPool,
    year: i16,
    month: i16,
) -> Option<(Decimal, Option<DateTime<Utc>>, Decimal)> {
    sqlx::query_as(
        "SELECT capital_base, boundary_reset_at, realized_loss FROM monthly_state \
         WHERE year = $1 AND month = $2 /* 8p-2 */",
    )
    .bind(year)
    .bind(month)
    .fetch_optional(pool)
    .await
    .unwrap()
}

/// A balance-account daemon of `tenant` on `pool`, with its own pending gauge.
/// Its venue keeps the futures surface when `futures_surface`, so a call of the
/// legacy reset would read 12,000; Spot has none.
fn balance_daemon(
    pool: &PgPool,
    tenant: Uuid,
    futures_surface: bool,
) -> (Daemon<WorkerVenue, MemoryStore>, Arc<WorkerVenue>, prometheus::Gauge) {
    let venue = WorkerVenue::new(AccountKind::BalanceAccount, false);
    let venue = Arc::new(if futures_surface {
        venue
    } else {
        venue.without_position_account()
    });
    let mut config = Config::test();
    config.projection.tenant_id = Some(tenant);
    let mut daemon = Daemon::assemble_with_recovery(
        config,
        Arc::clone(&venue),
        Arc::new(StubTradeFeed::new()),
        Arc::new(robson_exec::StubOhlcv::default()),
        None,
        Some(Arc::new(pool.clone())),
        None,
        None,
    );
    let pending = prometheus::Gauge::new("test_month_boundary_pending", "test").unwrap();
    daemon.month_boundary_pending = pending.clone();
    (daemon, venue, pending)
}

/// Whether the poll's view of `daemon`'s month at `now` finds its snapshot
/// owed and not complete: what `robsond_boundary_snapshot_pending` shows.
async fn owed(
    pool: &PgPool,
    daemon: &Daemon<WorkerVenue, MemoryStore>,
    now: DateTime<Utc>,
) -> bool {
    let accounting = daemon.position_manager.read().await.balance_accounting().unwrap();
    read_month(pool, &accounting, now).await.unwrap().2
}

/// PLAN-r3 §6.6's mapping: the first 16 bytes of SHA-256 of v16's string,
/// with only the RFC 4122 variant bits set. The hash's own byte 8 is 0x9b
/// for January on Binance Spot (variant bits already set), 0xd0 for April
/// and 0x55 for the stub's January; each keeps its hashed version nibble.
#[test]
fn the_reset_command_id_is_pinned() {
    let january = reset_command_id(SPOT, 2027, 1);
    assert_eq!(january, command_id("month_reset:binance_spot:2027-01"));
    assert_eq!(january.to_string(), "694ffdf2-7165-826a-9bac-12bc14fd00be");
    let april = reset_command_id(SPOT, 2027, 4);
    assert_eq!(april.to_string(), "7346137c-100c-7cb7-9097-4568ab7d52bd");
    let stub = reset_command_id(STUB, 2027, 1);
    assert_eq!(stub.to_string(), "3a7e08d9-0eb4-32db-95e6-be1d39958197");
    for id in [january, april, stub] {
        assert_eq!(id.get_variant(), uuid::Variant::RFC4122);
    }
}

/// R1 without a database: a balance daemon's poll asks its venue nothing and
/// records no reset, where the legacy poll would read the futures wallet.
#[tokio::test]
async fn a_balance_daemon_without_a_database_never_runs_the_futures_reset() {
    let venue = Arc::new(WorkerVenue::new(AccountKind::BalanceAccount, false));
    let daemon = Daemon::with_venue(
        Config::test(),
        Arc::clone(&venue),
        Arc::new(StubTradeFeed::new()),
        Arc::new(robson_exec::StubOhlcv::default()),
    );
    assert!(!daemon.poll_month_boundary(at(2027, 1, 1, 0, 0, 30)).await.unwrap());
    assert_eq!(venue.requests(), 0);
    assert!(daemon.store.events().get_all_events().await.unwrap().is_empty());
}

/// R1 (zero-basis test 6): a balance account's boundary poll never reaches
/// the futures-wallet reset, even on a venue that could answer it. Before any
/// certificate the month gets only 8p's reset at 0, with its command id; the
/// legacy handler on a venue without the futures surface, as Spot's, fails
/// and appends nothing, which is what the routing keeps out.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn balance_boundary_never_calls_the_futures_reset(pool: PgPool) {
    let tenant = Uuid::now_v7();
    let (daemon, venue, pending) = balance_daemon(&pool, tenant, true);
    let january = at(2027, 1, 1, 0, 0, 30);

    daemon.poll_month_boundary(january).await.unwrap();

    assert_eq!(resets(&pool, tenant, 2027, 1).await, [(
        Decimal::ZERO,
        Decimal::ZERO,
        Some(reset_command_id(STUB, 2027, 1))
    )]);
    assert_eq!(venue.requests(), 0, "no venue request, the futures wallet's least of all");
    assert_eq!(
        month_row(&pool, 2027, 1).await,
        Some((Decimal::ZERO, Some(january), Decimal::ZERO))
    );
    assert_eq!(pending.get(), 0.0);
    let manager = daemon.position_manager.read().await;
    assert!(manager.load_monthly_state(january).await.unwrap().basis_invalid(), "base 0");
    drop(manager);

    let (spot, _, _) = balance_daemon(&pool, Uuid::now_v7(), false);
    let february = at(2027, 2, 1, 0, 0, 30);
    let refused = spot.handle_month_boundary(february).await;
    assert!(matches!(refused, Err(DaemonError::Exec(_))), "{refused:?}");
    let appended: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM event_log WHERE event_type = 'month_boundary_reset' \
         AND (payload->>'month')::int = 2",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!((appended, month_row(&pool, 2027, 2).await), (0, None));
}

/// A position account (USD-M) keeps today's boundary whatever 8p's tables
/// hold: the legacy reset from the futures wallet, with no command id, and no
/// snapshot row or version moved. The zero-basis test 6's USD-M control: the
/// empty wallet resets December at 0.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_position_account_keeps_the_legacy_boundary(pool: PgPool) {
    let tenant = Uuid::now_v7();
    account(&pool, tenant, STUB).await;
    activation(&pool, tenant, STUB, "2026-10-01", Some("2026-10-15 12:00:00+00")).await;
    let mut config = Config::test();
    config.projection.tenant_id = Some(tenant);
    let daemon = Daemon::new_stub_with_recovery(config, None, Some(Arc::new(pool.clone())));
    assert_eq!(daemon.exchange.capabilities().account_kind, AccountKind::PositionAccount);

    daemon.exchange.set_futures_balance(Decimal::ZERO);
    assert!(daemon.poll_month_boundary(at(2026, 12, 1, 0, 0, 30)).await.unwrap());
    daemon.exchange.set_futures_balance(dec!(7000));
    assert!(daemon.poll_month_boundary(at(2027, 1, 1, 0, 0, 30)).await.unwrap());

    assert_eq!(resets(&pool, tenant, 2026, 12).await, [(Decimal::ZERO, Decimal::ZERO, None)]);
    assert_eq!(resets(&pool, tenant, 2027, 1).await, [(dec!(7000), Decimal::ZERO, None)]);
    assert!(snapshots(&pool, tenant).await.is_empty());
    assert_eq!(version(&pool, tenant, STUB).await, 0);
}

/// Whether some session waits for a lock on `table`.
async fn waited_on(pool: &PgPool, table: &str) {
    loop {
        let waiting: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM pg_locks WHERE relation = $1::regclass AND NOT granted)",
        )
        .bind(table)
        .fetch_one(pool)
        .await
        .unwrap();
        if waiting {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// Zero-basis test 5: a restart across boundaries writes every crossed
/// boundary's pending row before anything else. Held at its stream repair
/// (step 0a, the first recovery step to touch the database), the startup has
/// already written them, in one write. The first poll after it then adds
/// nothing, moves no version and appends no reset of the month, legacy or
/// zero; the month shows its reset and its snapshot pending.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn startup_writes_pending_rows_before_any_recovery(pool: PgPool) {
    let tenant = Uuid::now_v7();
    account(&pool, tenant, STUB).await;
    activation(&pool, tenant, STUB, "2026-10-01", Some("2026-10-15 12:00:00+00")).await;
    let (daemon, venue, pending) = balance_daemon(&pool, tenant, true);
    let now = Utc::now();
    let mut held = pool.begin().await.unwrap();
    sqlx::query("LOCK TABLE stream_suspensions IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *held)
        .await
        .unwrap();

    let shutdown = tokio_util::sync::CancellationToken::new();
    let mut startup = Box::pin(daemon.start_up(&shutdown));
    tokio::select! {
        started = &mut startup => panic!("the startup passed its held stream repair: {started:?}"),
        waited = tokio::time::timeout(Duration::from_secs(20), waited_on(&pool, "stream_suspensions")) => {
            waited.expect("the startup reaches its stream repair");
        },
    }
    let rows = snapshots(&pool, tenant).await;
    let months: Vec<NaiveDate> = rows.iter().map(|row| row.0).collect();
    assert_eq!(months.first(), Some(&day(2026, 11)), "{rows:?}");
    assert_eq!(months.last(), Some(&day(now.year(), now.month())), "{rows:?}");
    assert!(rows.iter().all(|row| row.2 == "pending"), "{rows:?}");
    assert_eq!(version(&pool, tenant, STUB).await, 1, "one write for all of them");
    held.rollback().await.unwrap();
    drop(startup);

    daemon.poll_month_boundary(now).await.unwrap();
    assert_eq!(snapshots(&pool, tenant).await, rows);
    assert_eq!(version(&pool, tenant, STUB).await, 1);
    assert!(resets(&pool, tenant, now.year(), now.month()).await.is_empty());
    assert_eq!(venue.requests(), 0);
    assert_eq!(pending.get(), 1.0);
    assert!(owed(&pool, &daemon, now).await);
}

/// Several boundaries crossed during downtime are pending at once: from the
/// month after the certified basis's to the current one, each as of its
/// first instant, in one write. Both ends at exactly B: a basis certified at
/// its own month's B owns no row of it, and a poll at exactly B owns B's.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn several_crossed_boundaries_are_pending_in_one_write(pool: PgPool) {
    let tenant = Uuid::now_v7();
    account(&pool, tenant, STUB).await;
    activation(&pool, tenant, STUB, "2026-10-01", Some("2026-10-01 00:00:00+00")).await;
    let (daemon, _, _) = balance_daemon(&pool, tenant, false);
    let january = at(2027, 1, 1, 0, 0, 0);

    daemon.poll_month_boundary(january).await.unwrap();

    let pending = |year, month| {
        (day(year, month), at(year, month, 1, 0, 0, 0), "pending".to_string(), january)
    };
    assert_eq!(snapshots(&pool, tenant).await, [
        pending(2026, 11),
        pending(2026, 12),
        pending(2027, 1)
    ]);
    assert_eq!(version(&pool, tenant, STUB).await, 1);
    assert!(owed(&pool, &daemon, january).await);
}

/// No row exists for a boundary at or before the certified activation basis,
/// nor for a basis never certified (an abandoned certificate).
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn no_pending_row_before_the_certified_activation(pool: PgPool) {
    let (abandoned, certified) = (Uuid::now_v7(), Uuid::now_v7());
    for tenant in [abandoned, certified] {
        account(&pool, tenant, STUB).await;
    }
    activation(&pool, abandoned, STUB, "2026-12-01", None).await;
    activation(&pool, certified, STUB, "2026-12-01", Some("2026-12-15 12:00:00+00")).await;
    let (first, _, _) = balance_daemon(&pool, abandoned, false);
    let (second, _, _) = balance_daemon(&pool, certified, false);
    let (december, january) = (at(2026, 12, 20, 0, 0, 0), at(2027, 1, 1, 0, 0, 30));

    for daemon in [&first, &second] {
        daemon.poll_month_boundary(december).await.unwrap();
    }
    assert!(snapshots(&pool, abandoned).await.is_empty());
    assert!(snapshots(&pool, certified).await.is_empty());
    assert!(!owed(&pool, &second, december).await, "the cutover month owes none");

    for daemon in [&first, &second] {
        daemon.poll_month_boundary(january).await.unwrap();
    }
    assert!(snapshots(&pool, abandoned).await.is_empty());
    assert_eq!(snapshots(&pool, certified).await, [(
        day(2027, 1),
        at(2027, 1, 1, 0, 0, 0),
        "pending".to_string(),
        january
    )]);
    assert_eq!(version(&pool, certified, STUB).await, 1);
    assert!(owed(&pool, &second, january).await);
}

/// Repeated polls, in this process and after a restart, find nothing new and
/// move no version: the pending transaction rolls back.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn idempotent_pending_poll_moves_no_version(pool: PgPool) {
    let tenant = Uuid::now_v7();
    account(&pool, tenant, STUB).await;
    activation(&pool, tenant, STUB, "2026-12-01", Some("2026-12-15 12:00:00+00")).await;
    let (daemon, _, _) = balance_daemon(&pool, tenant, false);
    let january = at(2027, 1, 1, 0, 0, 30);
    let later = |seconds| january + chrono::Duration::seconds(seconds);

    daemon.poll_month_boundary(january).await.unwrap();
    assert_eq!(version(&pool, tenant, STUB).await, 1);
    daemon.poll_month_boundary(later(60)).await.unwrap();
    let (restarted, _, _) = balance_daemon(&pool, tenant, false);
    restarted.poll_month_boundary(later(120)).await.unwrap();
    let accounting = restarted.position_manager.read().await.balance_accounting().unwrap();
    assert_eq!(write_pending(&pool, &accounting, later(180)).await.unwrap(), 0);

    assert_eq!(snapshots(&pool, tenant).await.len(), 1);
    assert_eq!(version(&pool, tenant, STUB).await, 1, "nothing new moves no version");
}

/// Before any certificate, a month nobody reset gets 8p's reset at basis 0,
/// once: it clears the breaker and is broadcast, and neither a later poll nor
/// a restart applies it again over the month's activity. A month with an
/// activation row (a cutover running in it), or one another writer already
/// reset (R2), gets none.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn pre_certificate_boundary_resets_to_zero_once(pool: PgPool) {
    let tenant = Uuid::now_v7();
    account(&pool, tenant, STUB).await;
    let (daemon, venue, pending) = balance_daemon(&pool, tenant, true);
    let breaker = daemon.position_manager.read().await.circuit_breaker();
    breaker.trigger_halt("December's drawdown".to_string()).await.unwrap();
    let mut broadcast = daemon.event_bus.subscribe();
    let january = at(2027, 1, 1, 0, 0, 30);

    daemon.poll_month_boundary(january).await.unwrap();

    let reset = (Decimal::ZERO, Decimal::ZERO, Some(reset_command_id(STUB, 2027, 1)));
    assert_eq!(resets(&pool, tenant, 2027, 1).await, [reset]);
    assert_eq!(
        month_row(&pool, 2027, 1).await,
        Some((Decimal::ZERO, Some(january), Decimal::ZERO))
    );
    assert_eq!(breaker.level().await, HaltState::Active);
    assert!(matches!(
        broadcast.try_recv(),
        Some(Ok(DaemonEvent::MonthBoundaryProcessed { year: 2027, month: 1, capital_base }))
            if capital_base.is_zero()
    ));
    assert_eq!(pending.get(), 0.0);
    assert!(!owed(&pool, &daemon, january).await, "no certificate, no snapshot owed");
    assert_eq!(version(&pool, tenant, STUB).await, 1, "the apply's bump");

    sqlx::query("UPDATE monthly_state SET realized_loss = 5 WHERE year = 2027 AND month = 1")
        .execute(&pool)
        .await
        .unwrap();
    daemon
        .poll_month_boundary(january + chrono::Duration::seconds(60))
        .await
        .unwrap();
    let (restarted, _, _) = balance_daemon(&pool, tenant, true);
    restarted
        .poll_month_boundary(january + chrono::Duration::seconds(120))
        .await
        .unwrap();
    assert_eq!(resets(&pool, tenant, 2027, 1).await, [reset]);
    assert_eq!(month_row(&pool, 2027, 1).await, Some((Decimal::ZERO, Some(january), dec!(5))));

    activation(&pool, tenant, STUB, "2027-02-01", None).await;
    daemon.poll_month_boundary(at(2027, 2, 1, 0, 0, 30)).await.unwrap();
    assert!(resets(&pool, tenant, 2027, 2).await.is_empty(), "a cutover runs in February");

    let other = at(2027, 3, 1, 0, 0, 5);
    sqlx::query(
        "INSERT INTO monthly_state (year, month, capital_base, boundary_reset_at, created_at) \
         VALUES (2027, 3, 900, $1, $1)",
    )
    .bind(other)
    .execute(&pool)
    .await
    .unwrap();
    daemon.poll_month_boundary(at(2027, 3, 1, 0, 0, 30)).await.unwrap();
    assert!(resets(&pool, tenant, 2027, 3).await.is_empty(), "R2");
    assert_eq!(month_row(&pool, 2027, 3).await, Some((dec!(900), Some(other), Decimal::ZERO)));
    assert_eq!(pending.get(), 0.0);
    assert_eq!(venue.requests(), 0);
}

async fn through_proxy(pool: &PgPool) -> (DbProxy, PgPool) {
    let options = pool.connect_options();
    let upstream = tokio::net::lookup_host((options.get_host(), options.get_port()))
        .await
        .unwrap()
        .next()
        .unwrap();
    let proxy = DbProxy::start(upstream).await.unwrap();
    let url = robson_testkit::daemon_process::database_url_through(
        &std::env::var("DATABASE_URL").unwrap(),
        proxy.addr(),
        options.get_database().unwrap(),
    )
    .unwrap();
    let proxied = PgPoolOptions::new()
        .max_connections(2)
        .test_before_acquire(false)
        .connect(&url)
        .await
        .unwrap();
    (proxy, proxied)
}

/// Each statement `work` sent through `proxy`, by what it is.
async fn traced<T>(proxy: &DbProxy, work: impl Future<Output = T>) -> (T, Vec<&'static str>) {
    proxy.enable_query_trace(64);
    let result = work.await;
    let shape = proxy
        .query_trace()
        .iter()
        .map(|sql| match sql.trim() {
            "BEGIN" => "begin",
            "COMMIT" => "commit",
            "ROLLBACK" => "rollback",
            sql if sql.starts_with("SELECT set_config('lock_timeout'") => "bounds",
            sql if sql == robson_eventlog::certificate::PARTICIPATE => "key",
            sql if sql.starts_with("WITH locked AS (") => "bump",
            sql if sql.contains("UPDATE public.spot_order_set_generation") => "cas",
            "SELECT create_event_log_partitions(3)" => "event_partitions",
            "SELECT create_snapshot_partitions(3)" => "snapshot_partitions",
            sql if sql.starts_with("INSERT INTO public.boundary_snapshots") => "pending",
            sql if sql.starts_with("WITH m AS (") => "view",
            sql if sql.starts_with("SELECT next_seq") => "seq",
            sql if sql.starts_with("INSERT INTO event_idempotency") => "idempotency",
            sql if sql.starts_with("INSERT INTO event_log") => "event",
            sql if sql.starts_with("UPDATE stream_state") => "stream",
            sql if sql.starts_with("SELECT event_id, tenant_id") => "fetch",
            sql if sql.starts_with("INSERT INTO monthly_state") => "monthly",
            _ => "other",
        })
        .collect();
    proxy.disable_query_trace();
    (result, shape)
}

/// A Binance Spot context of a new tenant with an order-set generation
/// installed in its live epoch, so each bump carries the CAS.
async fn installed_spot(pool: &PgPool) -> BalanceAccounting {
    let tenant = Uuid::now_v7();
    account(pool, tenant, SPOT).await;
    let start = crate::process_epoch::start_epoch(pool, tenant, "8p-2").await.unwrap();
    let cell = GenerationCell::default();
    let accounting = BalanceAccounting::for_daemon(
        AccountKind::BalanceAccount,
        tenant,
        &robson_exec::venue::VenueId::new(SPOT),
        &cell,
    )
    .unwrap();
    let generation = JournalGeneration {
        tenant_id: tenant,
        account_scope: accounting.account_scope.clone(),
        generation_id: Uuid::now_v7(),
        process_epoch: start.epoch.epoch_id,
        source_audit_id: Uuid::now_v7(),
    };
    sqlx::query(
        "INSERT INTO spot_order_set_generation (tenant_id, venue_id, account_scope, \
         generation_id, process_epoch, source_audit_id, accounting_version) \
         VALUES ($1, $2, $3, $4, $5, $6, 0)",
    )
    .bind(tenant)
    .bind(SPOT)
    .bind(&generation.account_scope)
    .bind(generation.generation_id)
    .bind(generation.process_epoch)
    .bind(generation.source_audit_id)
    .execute(pool)
    .await
    .unwrap();
    cell.publish(generation);
    accounting
}

/// The planned statements (8p PLAN-r3 §7.2, each transaction with 8m-2a's
/// certificate key) on Binance Spot's identity with a generation installed:
/// the pending rows seven, committed with a row and rolled back without; the
/// month's view one; the zero reset nine and its apply eight (its guard is
/// the month's view), then five when the month is already reset. Each write
/// bumps once with the CAS.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn each_boundary_transaction_sends_its_planned_statements(pool: PgPool) {
    sqlx::query("SELECT create_event_log_partitions(3)")
        .execute(&pool)
        .await
        .unwrap();
    let (proxy, proxied) = through_proxy(&pool).await;
    let january = at(2027, 1, 1, 0, 0, 30);
    let certified = installed_spot(&pool).await;
    activation(&pool, certified.tenant_id, SPOT, "2026-12-01", Some("2026-12-15 12:00:00+00"))
        .await;

    let (written, shape) = traced(&proxy, write_pending(&proxied, &certified, january)).await;
    assert_eq!(written.unwrap(), 1);
    assert_eq!(shape, ["begin", "bounds", "key", "bump", "cas", "pending", "commit"]);
    let (written, shape) = traced(&proxy, write_pending(&proxied, &certified, january)).await;
    assert_eq!(written.unwrap(), 0);
    assert_eq!(shape, ["begin", "bounds", "key", "bump", "cas", "pending", "rollback"]);
    let (view, shape) = traced(&proxy, read_month(&proxied, &certified, january)).await;
    assert_eq!(view.unwrap(), (false, false, true), "not due, not landed, owed");
    assert_eq!(shape, ["view"]);
    assert_eq!(version(&pool, certified.tenant_id, SPOT).await, 1);

    let uncertified = installed_spot(&pool).await;
    let streams = EventStreams::default();
    let journal = OrderJournal::unpersisted();
    let reset = zero_reset(&proxied, &uncertified, &streams, &journal, january);
    let (applied, shape) = traced(&proxy, reset).await;
    assert!(applied.unwrap());
    assert_eq!(shape, [
        "begin",
        "bounds",
        "key",
        "view",
        "seq",
        "idempotency",
        "event",
        "stream",
        "commit",
        "fetch",
        "begin",
        "bounds",
        "key",
        "bump",
        "cas",
        "monthly",
        "commit"
    ]);
    let reset = zero_reset(&proxied, &uncertified, &streams, &journal, january);
    let (applied, shape) = traced(&proxy, reset).await;
    assert!(!applied.unwrap());
    assert_eq!(shape, ["begin", "bounds", "key", "view", "rollback"]);
    assert_eq!(version(&pool, uncertified.tenant_id, SPOT).await, 1);
}

/// A balance account's poll through the proxy: the partitions once per new
/// month, then each poll's pending transaction (six without a generation)
/// and view; the zero reset only while its month is due.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_balance_poll_creates_the_partitions_once_per_month(pool: PgPool) {
    let (proxy, proxied) = through_proxy(&pool).await;
    let tenant = Uuid::now_v7();
    account(&pool, tenant, STUB).await;
    let (daemon, _, _) = balance_daemon(&proxied, tenant, false);
    let january = at(2027, 1, 1, 0, 0, 30);
    let pending = [
        "begin", "bounds", "key", "bump", "pending", "rollback", "view",
    ];

    let (polled, shape) = traced(&proxy, daemon.poll_month_boundary(january)).await;
    assert!(polled.unwrap());
    let reset = [
        "begin",
        "bounds",
        "key",
        "view",
        "seq",
        "idempotency",
        "event",
        "stream",
        "commit",
    ];
    let apply = [
        "fetch", "begin", "bounds", "key", "bump", "monthly", "commit",
    ];
    let partitions = ["event_partitions", "snapshot_partitions"];
    assert_eq!(shape, [&partitions[..], &pending[..], &reset[..], &apply[..]].concat());
    let later = january + chrono::Duration::seconds(60);
    let (polled, shape) = traced(&proxy, daemon.poll_month_boundary(later)).await;
    assert!(!polled.unwrap());
    assert_eq!(shape, pending);
}

const JANUARY_STREAM: &str = "system:month_boundary:2027-01";

/// 8m-2a: the boundary's transactions take the cutover certificate's key right
/// after their bounds. While completion holds it, the pending rows and the
/// zero reset's append each refuse at their lock bound with nothing written
/// and nothing suspended; after completion both land.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn boundary_transactions_wait_for_completion(pool: PgPool) {
    use crate::balance_accounting::certificate_tests::Completion;
    sqlx::query("SELECT create_event_log_partitions(3)")
        .execute(&pool)
        .await
        .unwrap();
    let january = at(2027, 1, 1, 0, 0, 30);
    let certified = installed_spot(&pool).await;
    activation(&pool, certified.tenant_id, SPOT, "2026-12-01", Some("2026-12-15 12:00:00+00"))
        .await;
    let uncertified = installed_spot(&pool).await;
    let (streams, journal) = (EventStreams::default(), OrderJournal::unpersisted());

    let completion = Completion::hold(&pool).await.unwrap();
    assert!(write_pending(&pool, &certified, january).await.is_err());
    assert!(zero_reset(&pool, &uncertified, &streams, &journal, january).await.is_err());
    assert!(!streams.is_suspended(JANUARY_STREAM));
    assert_eq!(version(&pool, certified.tenant_id, SPOT).await, 0);
    assert_eq!(version(&pool, uncertified.tenant_id, SPOT).await, 0);
    assert!(resets(&pool, uncertified.tenant_id, 2027, 1).await.is_empty());
    completion.release().await.unwrap();

    assert_eq!(write_pending(&pool, &certified, january).await.unwrap(), 1);
    assert!(zero_reset(&pool, &uncertified, &streams, &journal, january).await.unwrap());
    assert_eq!(version(&pool, certified.tenant_id, SPOT).await, 1);
    assert_eq!(resets(&pool, uncertified.tenant_id, 2027, 1).await.len(), 1);
}

/// The zero reset of January 2027 as 8p stores it for `tenant` on the stub
/// venue, appended alone: a crash after the append and before the apply.
async fn reset_appended_alone(pool: &PgPool, tenant: Uuid, at: DateTime<Utc>) {
    let reset = robson_domain::Event::MonthBoundaryReset {
        capital_base: Decimal::ZERO,
        carried_positions_risk: Decimal::ZERO,
        month: 1,
        year: 2027,
        timestamp: at,
    };
    let payload = serde_json::to_value(&reset).unwrap();
    let stored = robson_eventlog::Event::new(tenant, JANUARY_STREAM, reset.event_type(), payload)
        .with_command_id(reset_command_id(STUB, 2027, 1));
    robson_eventlog::append_event(pool, JANUARY_STREAM, None, stored).await.unwrap();
}

/// A crash between the zero reset's append and its apply leaves the event
/// stored and the month unreset. With the event log held, the apply's fetch
/// stalls: the client deadline ends it within seconds, nothing is applied,
/// the stream is suspended and recorded, and a suspended stream takes no
/// further reset. After a restart the guard still finds the month due, the
/// append dedups by command id and the stored event is applied, once.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_lost_apply_heals_once_and_a_stalled_one_suspends_the_stream(pool: PgPool) {
    sqlx::query("SELECT create_event_log_partitions(3)")
        .execute(&pool)
        .await
        .unwrap();
    let tenant = Uuid::now_v7();
    let (daemon, _, _) = balance_daemon(&pool, tenant, false);
    let accounting = daemon.position_manager.read().await.balance_accounting().unwrap();
    let january = at(2027, 1, 1, 0, 0, 30);
    reset_appended_alone(&pool, tenant, january).await;
    let (streams, journal) = (EventStreams::default(), OrderJournal::unpersisted());

    let mut held = pool.begin().await.unwrap();
    sqlx::query("LOCK TABLE event_log IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *held)
        .await
        .unwrap();
    let started = std::time::Instant::now();
    let stalled = zero_reset(&pool, &accounting, &streams, &journal, january);
    let stalled = tokio::time::timeout(Duration::from_secs(10), stalled).await;
    let waited = started.elapsed();
    let stalled = stalled.expect("the client deadline ends the stalled apply");
    held.rollback().await.unwrap();
    assert!(stalled.is_err(), "{stalled:?}");
    assert!(
        waited >= Duration::from_secs(2) && waited < Duration::from_secs(5),
        "{waited:?}"
    );
    assert!(streams.is_suspended(JANUARY_STREAM));
    assert!(journal.retry_set_depth() > 0, "the suspension is recorded");
    let refused = zero_reset(&pool, &accounting, &streams, &journal, january).await;
    assert!(refused.unwrap_err().to_string().contains("is suspended"));
    assert_eq!(month_row(&pool, 2027, 1).await, None);

    let restarted = EventStreams::default();
    let later = january + chrono::Duration::seconds(60);
    assert!(zero_reset(&pool, &accounting, &restarted, &journal, later).await.unwrap());
    let reset = (Decimal::ZERO, Decimal::ZERO, Some(reset_command_id(STUB, 2027, 1)));
    assert_eq!(resets(&pool, tenant, 2027, 1).await, [reset]);
    assert_eq!(
        month_row(&pool, 2027, 1).await,
        Some((Decimal::ZERO, Some(january), Decimal::ZERO))
    );
    assert!(!zero_reset(&pool, &accounting, &restarted, &journal, later).await.unwrap());
}

/// Holds each `monthly_state` write's `COMMIT` on the server for half a
/// second, inside the apply's one-second statement bound.
const SLOW_COMMIT: [&str; 2] = [
    "CREATE FUNCTION slow_commit() RETURNS trigger LANGUAGE plpgsql AS \
     $$ BEGIN PERFORM pg_sleep(0.5); RETURN NULL; END $$",
    "CREATE CONSTRAINT TRIGGER slow_commit AFTER INSERT OR UPDATE ON monthly_state \
     DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION slow_commit()",
];

/// The zero reset's apply cancelled after its `COMMIT` was sent, its answer
/// lost (8p PLAN-r3 §7.1): the server holds the commit while the proxy starts
/// swallowing every answer, and the client deadline ends the apply. Its
/// connection is retired raw: closed at once and no longer the pool's, never
/// returned to the pool, where SQLx would flush the queued `ROLLBACK` and
/// wait on a ping nothing answers. The stream is suspended and recorded. The
/// reset did commit: a suspended stream takes no further reset, and after a
/// restart the guard finds it landed and appends nothing.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_lost_apply_commit_retires_its_connection_raw(pool: PgPool) {
    sqlx::query("SELECT create_event_log_partitions(3)")
        .execute(&pool)
        .await
        .unwrap();
    for sql in SLOW_COMMIT {
        sqlx::query(sql).execute(&pool).await.unwrap();
    }
    let (proxy, proxied) = through_proxy(&pool).await;
    let accounting = installed_spot(&pool).await;
    let tenant = accounting.tenant_id;
    let january = at(2027, 1, 1, 0, 0, 30);
    let (streams, journal) = (EventStreams::default(), OrderJournal::unpersisted());
    let (commits, closed) = (proxy.stats().commits, proxy.closed_relays());
    // The append's COMMIT, then the apply's: every answer from then on is lost.
    let lose_answers = async {
        while proxy.stats().commits < commits + 2 {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        proxy.set_mode(ProxyMode::Blackhole);
    };
    let started = std::time::Instant::now();
    let reset = zero_reset(&proxied, &accounting, &streams, &journal, january);
    let both = async { tokio::join!(lose_answers, reset) };
    let both = tokio::time::timeout(Duration::from_secs(10), both).await;
    let waited = started.elapsed();
    let ((), late) = both.expect("the client deadline ends the apply");
    let late = late.unwrap_err().to_string();
    assert!(late.contains("deadline has elapsed"), "{late}");
    assert!(
        waited >= Duration::from_secs(2) && waited < Duration::from_secs(5),
        "{waited:?}"
    );
    // The apply's connection is retired raw, closing its relay; whether any
    // connection went back to the pool is the idleness check below. This
    // bound (the apply's own 10 s above) only ends a hang.
    tokio::time::timeout(Duration::from_secs(10), async {
        while proxy.closed_relays() == closed {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the apply's connection is closed, not returned to the pool");
    assert_eq!(proxied.size() as usize, proxied.num_idle(), "none in use or being returned");
    assert!(streams.is_suspended(JANUARY_STREAM));
    assert!(journal.retry_set_depth() > 0, "the suspension is recorded");
    proxy.set_mode(ProxyMode::Pass);

    assert_eq!(
        month_row(&pool, 2027, 1).await,
        Some((Decimal::ZERO, Some(january), Decimal::ZERO))
    );
    assert_eq!(version(&pool, tenant, SPOT).await, 1, "the apply committed once");
    let refused = zero_reset(&pool, &accounting, &streams, &journal, january).await;
    assert!(refused.unwrap_err().to_string().contains("is suspended"));
    let restarted = EventStreams::default();
    let later = january + chrono::Duration::seconds(60);
    assert!(!zero_reset(&pool, &accounting, &restarted, &journal, later).await.unwrap());
    let reset = (Decimal::ZERO, Decimal::ZERO, Some(reset_command_id(SPOT, 2027, 1)));
    assert_eq!(resets(&pool, tenant, 2027, 1).await, [reset]);
}

/// Two owners of one venue, as when a successor starts beside an older live
/// process: the successor's start rotates the order-set generation in its own
/// epoch, so every bump of the older owner, its zero reset's apply included,
/// refuses (8l-4a's CAS) and only the successor applies the month's reset,
/// once, the event appended by either owner but stored once.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_superseded_owner_applies_no_reset(pool: PgPool) {
    sqlx::query("SELECT create_event_log_partitions(3)")
        .execute(&pool)
        .await
        .unwrap();
    let older = installed_spot(&pool).await;
    let tenant = older.tenant_id;
    let installed = older.generation.current().unwrap();
    let successor_epoch = crate::process_epoch::start_epoch(&pool, tenant, "8p-2 successor")
        .await
        .unwrap();
    let rotated = JournalGeneration {
        generation_id: Uuid::now_v7(),
        process_epoch: successor_epoch.epoch.epoch_id,
        source_audit_id: Uuid::now_v7(),
        ..installed.clone()
    };
    sqlx::query(
        "UPDATE public.spot_order_set_generation g SET generation_id = $3, process_epoch = $4, \
         source_audit_id = $5, accounting_version = a.version FROM public.accounting_state a \
         WHERE (a.tenant_id, a.venue_id) = (g.tenant_id, g.venue_id) \
         AND (g.tenant_id, g.venue_id, g.account_scope) = ($1, 'binance:spot', $2) \
         AND g.generation_id = $6",
    )
    .bind(tenant)
    .bind(&rotated.account_scope)
    .bind(rotated.generation_id)
    .bind(rotated.process_epoch)
    .bind(rotated.source_audit_id)
    .bind(installed.generation_id)
    .execute(&pool)
    .await
    .unwrap();
    let cell = GenerationCell::default();
    cell.publish(rotated);
    let successor = BalanceAccounting::for_daemon(
        AccountKind::BalanceAccount,
        tenant,
        &robson_exec::venue::VenueId::new(SPOT),
        &cell,
    )
    .unwrap();
    let january = at(2027, 1, 1, 0, 0, 30);
    let key = "system:month_boundary:2027-01";
    let journal = OrderJournal::unpersisted();

    let older_streams = EventStreams::default();
    let refused = zero_reset(&pool, &older, &older_streams, &journal, january).await;
    assert!(refused.unwrap_err().to_string().contains("stale"), "the older owner's apply");
    assert!(older_streams.is_suspended(key));
    assert_eq!(month_row(&pool, 2027, 1).await, None);
    assert_eq!(version(&pool, tenant, SPOT).await, 0);

    let streams = EventStreams::default();
    assert!(zero_reset(&pool, &successor, &streams, &journal, january).await.unwrap());
    let reset = (Decimal::ZERO, Decimal::ZERO, Some(reset_command_id(SPOT, 2027, 1)));
    assert_eq!(resets(&pool, tenant, 2027, 1).await, [reset]);
    assert_eq!(
        month_row(&pool, 2027, 1).await,
        Some((Decimal::ZERO, Some(january), Decimal::ZERO))
    );
    assert_eq!(version(&pool, tenant, SPOT).await, 1, "one apply, the successor's");
    assert!(!zero_reset(&pool, &successor, &streams, &journal, january).await.unwrap());
}

/// What a probe saw of one job root (8p-O): its label, its attempts by kind
/// (statement, `BEGIN`, `COMMIT`, `ROLLBACK`), its score (those plus one per
/// nested job operation), whether it crossed 15, whether it is incomplete,
/// and its outcome (1 success, 2 error).
type Observed = crate::operation_observation::probe::JobObservation;

/// `work` relayed through `proxy` and observed by `probe`: its result, each
/// job root it ran, and the statements the proxy relayed, once each relayed
/// statement, in order, is asserted to be one first-entered attempt of its
/// intended root, of its kind: `owners` gives each root's label and how
/// many consecutive relayed statements are its.
async fn observed<T>(
    proxy: &DbProxy,
    probe: &Probe,
    owners: &[(&'static str, usize)],
    work: impl Future<Output = T>,
) -> (T, Vec<Observed>, Vec<&'static str>) {
    let (result, shape) = traced(proxy, work.with_subscriber(probe.dispatch())).await;
    let mut labels = owners.iter().flat_map(|&(label, n)| std::iter::repeat_n(label, n));
    let expected: Vec<(&'static str, u64)> = shape
        .iter()
        .map(|relayed| (labels.next().expect("an owner"), io_kind(relayed)))
        .collect();
    assert_eq!(labels.next(), None, "{shape:?}");
    assert_eq!(probe.take_events(), expected, "{shape:?}");
    (result, probe.take_jobs(), shape)
}

/// The `io_kind` of a relayed statement: 2 `BEGIN`, 3 `COMMIT`, 4
/// `ROLLBACK`, 1 any other.
fn io_kind(relayed: &str) -> u64 {
    match relayed {
        "begin" => 2,
        "commit" => 3,
        "rollback" => 4,
        _ => 1,
    }
}

/// The attempts by kind (statement, `BEGIN`, `COMMIT`, `ROLLBACK`) that a
/// proxy trace relayed.
fn kinds(shape: &[&str]) -> [u64; 4] {
    let count = |kind| shape.iter().filter(|relayed| **relayed == kind).count() as u64;
    let [begin, commit, rollback] = ["begin", "commit", "rollback"].map(count);
    [
        shape.len() as u64 - begin - commit - rollback,
        begin,
        commit,
        rollback,
    ]
}

/// 8p-O: each statement of every boundary transaction, as the proxy relays
/// it, counts once under its operation's root, in order and by kind: the tick's
/// pending rows committed (seven) and rolled back (seven) and its view (one),
/// the zero reset's append (nine) and apply (eight), each its own root, and the
/// refused append (five), each with the certificate's key (8m-2a). None crosses
/// 15.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn each_boundary_statement_counts_under_its_operation(pool: PgPool) {
    let _serial = JOB_METRICS.lock().await;
    sqlx::query("SELECT create_event_log_partitions(3)")
        .execute(&pool)
        .await
        .unwrap();
    let (proxy, proxied) = through_proxy(&pool).await;
    let probe = Probe::new();
    let january = at(2027, 1, 1, 0, 0, 30);
    let certified = installed_spot(&pool).await;
    activation(&pool, certified.tenant_id, SPOT, "2026-12-01", Some("2026-12-15 12:00:00+00"))
        .await;

    for rows in [1, 0] {
        let pending = job(Job::BoundaryTick, write_pending(&proxied, &certified, january));
        let (written, jobs, shape) =
            observed(&proxy, &probe, &[("boundary_tick", 7)], pending).await;
        assert_eq!(written.unwrap(), rows);
        assert_eq!(shape.len(), 7, "{shape:?}");
        assert_eq!(jobs, [("boundary_tick", kinds(&shape), 7, false, false, 1)], "{shape:?}");
    }
    let view = job(Job::BoundaryTick, read_month(&proxied, &certified, january));
    let (view, jobs, shape) = observed(&proxy, &probe, &[("boundary_tick", 1)], view).await;
    assert_eq!(view.unwrap(), (false, false, true));
    assert_eq!(shape, ["view"]);
    assert_eq!(jobs, [("boundary_tick", [1, 0, 0, 0], 1, false, false, 1)]);

    let uncertified = installed_spot(&pool).await;
    let streams = EventStreams::default();
    let journal = OrderJournal::unpersisted();
    let reset = zero_reset(&proxied, &uncertified, &streams, &journal, january);
    let owners = [("boundary_zero_reset", 9), ("boundary_apply", 8)];
    let (applied, jobs, shape) = observed(&proxy, &probe, &owners, reset).await;
    assert!(applied.unwrap());
    assert_eq!(shape.len(), 17, "{shape:?}");
    assert_eq!(jobs, [
        ("boundary_zero_reset", kinds(&shape[..9]), 9, false, false, 1),
        ("boundary_apply", kinds(&shape[9..]), 8, false, false, 1),
    ]);
    let reset = zero_reset(&proxied, &uncertified, &streams, &journal, january);
    let owners = [("boundary_zero_reset", 5)];
    let (applied, jobs, shape) = observed(&proxy, &probe, &owners, reset).await;
    assert!(!applied.unwrap());
    assert_eq!(shape, ["begin", "bounds", "key", "view", "rollback"]);
    assert_eq!(jobs, [("boundary_zero_reset", [3, 1, 0, 1], 5, false, false, 1)]);
    probe.release();
}

/// 8p-O: a balance account's poll counts each statement the proxy relays
/// once: the tick's partitions, pending rows (six without a generation) and
/// view (nine at a new month, scored eleven with its two nested operations),
/// then the zero reset's append (nine) and apply (seven) under their own
/// roots; the next poll's tick seven.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_balance_poll_counts_each_statement_under_its_operation(pool: PgPool) {
    let _serial = JOB_METRICS.lock().await;
    let (proxy, proxied) = through_proxy(&pool).await;
    let probe = Probe::new();
    let tenant = Uuid::now_v7();
    account(&pool, tenant, STUB).await;
    let (daemon, _, _) = balance_daemon(&proxied, tenant, false);
    let january = at(2027, 1, 1, 0, 0, 30);

    let owners = [
        ("boundary_tick", 9),
        ("boundary_zero_reset", 9),
        ("boundary_apply", 7),
    ];
    let poll = daemon.poll_month_boundary(january);
    let (polled, jobs, shape) = observed(&proxy, &probe, &owners, poll).await;
    assert!(polled.unwrap());
    assert_eq!(shape.len(), 25, "{shape:?}");
    assert_eq!(&shape[..2], ["event_partitions", "snapshot_partitions"]);
    assert_eq!(jobs, [
        ("boundary_tick", kinds(&shape[..9]), 11, false, false, 1),
        ("boundary_zero_reset", kinds(&shape[9..18]), 9, false, false, 1),
        ("boundary_apply", kinds(&shape[18..]), 7, false, false, 1),
    ]);
    let later = january + chrono::Duration::seconds(60);
    let poll = daemon.poll_month_boundary(later);
    let (polled, jobs, shape) = observed(&proxy, &probe, &[("boundary_tick", 7)], poll).await;
    assert!(!polled.unwrap());
    assert_eq!(shape.len(), 7, "{shape:?}");
    assert_eq!(jobs, [("boundary_tick", kinds(&shape), 7, false, false, 1)]);
    probe.release();
}

/// 8p-O: the startup's pending rows count as a tick: the six statements a
/// pending write without a generation relays (begin, bounds, the
/// certificate's key, bump, the insert and commit), held at the stream repair
/// that follows.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn the_startup_pending_rows_count_as_a_tick(pool: PgPool) {
    let _serial = JOB_METRICS.lock().await;
    let probe = Probe::new();
    let tenant = Uuid::now_v7();
    account(&pool, tenant, STUB).await;
    activation(&pool, tenant, STUB, "2026-10-01", Some("2026-10-15 12:00:00+00")).await;
    let (daemon, _, _) = balance_daemon(&pool, tenant, true);
    let mut held = pool.begin().await.unwrap();
    sqlx::query("LOCK TABLE stream_suspensions IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *held)
        .await
        .unwrap();

    let shutdown = tokio_util::sync::CancellationToken::new();
    let mut startup = Box::pin(daemon.start_up(&shutdown).with_subscriber(probe.dispatch()));
    tokio::select! {
        started = &mut startup => panic!("the startup passed its held stream repair: {started:?}"),
        waited = tokio::time::timeout(Duration::from_secs(20), waited_on(&pool, "stream_suspensions")) => {
            waited.expect("the startup reaches its stream repair");
        },
    }
    assert_eq!(probe.take_jobs(), [("boundary_tick", [4, 1, 1, 0], 6, false, false, 1)]);
    let tick = |kind: u64| ("boundary_tick", kind);
    let in_order = [tick(2), tick(1), tick(1), tick(1), tick(1), tick(3)];
    assert_eq!(probe.take_events(), in_order, "in order");
    held.rollback().await.unwrap();
    drop(startup);
    probe.release();
}

/// 8p-O: a stalled apply's failure is counted where it happens (the apply's
/// fetch, an error), and the tick it ran in is incomplete: the stream
/// suspension it records is written by the journal's retry loop, uncounted.
/// The append found the stored event (eight, rolled back).
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_stalled_apply_leaves_its_tick_incomplete(pool: PgPool) {
    let _serial = JOB_METRICS.lock().await;
    sqlx::query("SELECT create_event_log_partitions(3)")
        .execute(&pool)
        .await
        .unwrap();
    let probe = Probe::new();
    let tenant = Uuid::now_v7();
    let (daemon, _, _) = balance_daemon(&pool, tenant, false);
    let accounting = daemon.position_manager.read().await.balance_accounting().unwrap();
    let january = at(2027, 1, 1, 0, 0, 30);
    reset_appended_alone(&pool, tenant, january).await;
    let (streams, journal) = (EventStreams::default(), OrderJournal::unpersisted());

    let mut held = pool.begin().await.unwrap();
    sqlx::query("LOCK TABLE event_log IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *held)
        .await
        .unwrap();
    let stalled =
        job(Job::BoundaryTick, zero_reset(&pool, &accounting, &streams, &journal, january));
    let stalled =
        tokio::time::timeout(Duration::from_secs(10), stalled.with_subscriber(probe.dispatch()));
    let stalled = stalled.await.expect("the client deadline ends the stalled apply");
    held.rollback().await.unwrap();
    assert!(stalled.is_err(), "{stalled:?}");
    assert!(streams.is_suspended(JANUARY_STREAM));
    assert_eq!(probe.take_jobs(), [
        ("boundary_tick", [0, 0, 0, 0], 2, false, true, 2),
        ("boundary_zero_reset", [6, 1, 0, 1], 8, false, false, 1),
        ("boundary_apply", [1, 0, 0, 0], 1, false, false, 2),
    ]);
    let append = |kind: u64| ("boundary_zero_reset", kind);
    assert_eq!(probe.take_events(), [
        append(2),
        append(1),
        append(1),
        append(1),
        append(1),
        append(1),
        append(1),
        append(4),
        ("boundary_apply", 1),
        ("boundary_tick", 7),
    ]);
    probe.release();
}
