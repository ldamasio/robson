//! Crash harness self-tests (ADR-0056 step 7, PR H): the real `robsond`, with
//! the real Binance adapter, against a fake venue and a database proxy,
//! killed by SIGKILL at named crash points.
//!
//! What is asserted here is that the harness itself works. Behaviour tests for
//! later PRs add a `crash_point!` at the place and build on [`Harness`].
//!
//! The database comes from the environment, as for every `DATABASE_URL`-gated
//! test: `sqlx::test` creates one per test on the provisioned server and drops
//! it. The harness starts no database. Run it through the canonical wrapper,
//! whose guard refuses a production URL before SQLx connects:
//! `DATABASE_URL=… bash scripts/test-pg.sh --crash-harness`.

#![cfg(all(feature = "postgres", feature = "crash-points"))]

use std::{os::unix::process::ExitStatusExt, path::PathBuf, time::Duration};

use robson_exec::crash_points::{ENTRY_AFTER_ACK, ENTRY_BEFORE_SEND};
use robson_testkit::{
    daemon_process::{database_url_through, DaemonProcess, DaemonSpec},
    db_proxy::{DbProxy, ProxyMode},
    fake_venue::{FakeVenue, VenueCounts, FAKE_SYMBOL},
};
use rust_decimal::Decimal;
use serde_json::{json, Value};
use sqlx::{Connection, PgConnection, PgPool};
use uuid::Uuid;

/// Hard limit for one daemon process; the watchdogs enforce it.
const DAEMON_DEADLINE: Duration = Duration::from_secs(90);
/// Time a step may take before the test fails with the daemon's log.
const STEP: Duration = Duration::from_secs(60);

/// One test's world: the fake venue, the proxy in front of the test database,
/// and a direct connection for reading ground truth around the proxy.
struct Harness {
    pool: PgPool,
    venue: FakeVenue,
    proxy: DbProxy,
    database_url: String,
    tenant_id: Uuid,
    work_dir: PathBuf,
}

impl Harness {
    async fn start(pool: PgPool) -> Self {
        let venue = FakeVenue::start().await.expect("fake venue");
        let options = pool.connect_options();
        let upstream = tokio::net::lookup_host((options.get_host(), options.get_port()))
            .await
            .expect("resolve test database host")
            .next()
            .expect("the test database host has an address");
        let proxy = DbProxy::start(upstream).await.expect("database proxy");
        let database = options.get_database().expect("sqlx::test names its database");
        let base_url =
            std::env::var("DATABASE_URL").expect("the environment supplies DATABASE_URL");
        let database_url =
            database_url_through(&base_url, proxy.addr(), database).expect("proxied URL");
        let work_dir =
            PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("crash-{}", Uuid::now_v7()));
        std::fs::create_dir_all(&work_dir).expect("work dir");
        Self {
            pool,
            venue,
            proxy,
            database_url,
            tenant_id: Uuid::now_v7(),
            work_dir,
        }
    }

    fn daemon(&self, crash_point: Option<&str>) -> DaemonProcess {
        self.daemon_with_env(crash_point, &[])
    }

    fn daemon_with_env(
        &self,
        crash_point: Option<&str>,
        extra_env: &[(&str, &str)],
    ) -> DaemonProcess {
        DaemonProcess::spawn(&DaemonSpec {
            binary: PathBuf::from(env!("CARGO_BIN_EXE_robsond")),
            work_dir: self.work_dir.clone(),
            database_url: self.database_url.clone(),
            tenant_id: self.tenant_id,
            venue_url: self.venue.url(),
            crash_point: crash_point.map(str::to_string),
            deadline: DAEMON_DEADLINE,
            extra_env: extra_env.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
            removed_env: Vec::new(),
            dotenv: String::new(),
        })
        .expect("spawn robsond")
    }

    async fn event_types(&self) -> Vec<String> {
        sqlx::query_scalar("SELECT event_type FROM event_log WHERE stream_key LIKE 'position:%'")
            .fetch_all(&self.pool)
            .await
            .expect("read event_log")
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.work_dir);
    }
}

/// Arm an immediate long. Sent from a task: the daemon can die mid-request, so
/// the answer is often a closed connection.
fn arm_immediate(api_url: String) -> tokio::task::JoinHandle<Result<Value, String>> {
    tokio::spawn(async move {
        let response = reqwest::Client::new()
            .post(format!("{api_url}/positions"))
            .timeout(STEP)
            .json(&json!({
                "symbol": FAKE_SYMBOL,
                "side": "long",
                "entry_policy": { "mode": "immediate", "approval": "automatic" },
            }))
            .send()
            .await
            .map_err(|e| e.to_string())?;
        let status = response.status();
        let body = response.text().await.map_err(|e| e.to_string())?;
        if !status.is_success() {
            return Err(format!("arm answered {status}: {body}"));
        }
        serde_json::from_str(&body).map_err(|e| e.to_string())
    })
}

/// SIGKILL just after the venue accepted the entry, before anything recorded
/// it: the fill exists on the venue and nowhere else. A restart refuses to
/// start (exit 78) and leaves the fill open. The real binary is shown to use
/// the proxy, the fake venue and the named crash point.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn killed_after_the_venue_acked_the_fill_exists_only_on_the_venue(pool: PgPool) {
    let harness = Harness::start(pool).await;

    let daemon = harness.daemon(Some(ENTRY_AFTER_ACK));
    daemon.wait_ready(STEP).await.expect("daemon ready");
    assert!(
        harness.proxy.stats().connections > 0,
        "the daemon reaches the database via the proxy"
    );
    let request = arm_immediate(daemon.api_url());
    daemon
        .wait_killed_at(ENTRY_AFTER_ACK, STEP)
        .await
        .expect("killed at the crash point");
    assert!(request.await.unwrap().is_err(), "the caller never got an answer");

    assert_eq!(harness.venue.counts(), VenueCounts { accepted_orders: 1, fills: 1, cancels: 0 });
    assert!(
        harness.venue.position(FAKE_SYMBOL) > Decimal::ZERO,
        "the fill is open on the venue"
    );
    assert!(
        harness.venue.unserved_requests().is_empty(),
        "{:?}",
        harness.venue.unserved_requests()
    );
    let events = harness.event_types().await;
    assert!(events.iter().any(|e| e == "position_armed"), "{events:?}");
    assert!(!events.iter().any(|e| e == "entry_order_accepted"), "{events:?}");
    drop(daemon);

    // The projection still reads Armed, so the restart stops at the startup
    // gate (exit 78) before any sweep. Nothing is closed: the fill stays open
    // on the venue with no stop.
    let restarted = harness.daemon(None);
    let status = restarted.wait_exit(STEP).await.expect("the restart stops by itself");
    assert_eq!(status.code(), Some(78), "startup gate\n{}", restarted.log_tail());
    assert_eq!(harness.venue.counts(), VenueCounts { accepted_orders: 1, fills: 1, cancels: 0 });
    assert!(harness.venue.position(FAKE_SYMBOL) > Decimal::ZERO);
}

fn stuck_spec(work_dir: PathBuf) -> DaemonSpec {
    DaemonSpec {
        binary: PathBuf::from(env!("CARGO_BIN_EXE_robsond")),
        work_dir,
        // Nothing here dials a database or a venue before the checks under test.
        database_url: "postgresql://unused:unused@127.0.0.1:1/unused".to_string(),
        tenant_id: Uuid::now_v7(),
        venue_url: "http://127.0.0.1:1".to_string(),
        crash_point: None,
        deadline: DAEMON_DEADLINE,
        extra_env: Vec::new(),
        removed_env: Vec::new(),
        dotenv: String::new(),
    }
}

fn scratch_dir() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("crash-{}", Uuid::now_v7()));
    std::fs::create_dir_all(&dir).expect("work dir");
    dir
}

/// A crash-points build refuses production and a point it does not know, in
/// the real binary, before touching the database or the venue.
#[tokio::test]
async fn a_crash_points_build_refuses_production_and_unknown_points() {
    let dir = scratch_dir();

    let mut production = stuck_spec(dir.clone());
    production.extra_env.push(("ROBSON_ENV".to_string(), "production".to_string()));
    let daemon = DaemonProcess::spawn(&production).expect("spawn");
    let status = daemon.wait_exit(STEP).await.expect("refused at start");
    assert_eq!(status.code(), Some(101), "{}", daemon.log_tail());
    assert!(daemon.log_tail().contains("ROBSON_ENV=production"), "{}", daemon.log_tail());

    let mut unknown = stuck_spec(dir.clone());
    unknown.crash_point = Some("no_such_point".to_string());
    let daemon = DaemonProcess::spawn(&unknown).expect("spawn");
    let status = daemon.wait_exit(STEP).await.expect("refused at start");
    assert_eq!(status.code(), Some(101), "{}", daemon.log_tail());
    assert!(daemon.log_tail().contains("no_such_point"), "{}", daemon.log_tail());
    let _ = std::fs::remove_dir_all(dir);
}

/// The sequence the inherited-environment check alone would miss: `ROBSON_ENV`
/// is absent from the environment and `production` only in `.env`.
#[tokio::test]
async fn production_arriving_through_dotenv_is_refused() {
    let dir = scratch_dir();
    let mut spec = stuck_spec(dir.clone());
    spec.removed_env.push("ROBSON_ENV".to_string());
    spec.dotenv = "ROBSON_ENV=production\n".to_string();
    let daemon = DaemonProcess::spawn(&spec).expect("spawn");

    let status = daemon.wait_exit(STEP).await.expect("refused at start");
    assert_eq!(status.code(), Some(101), "{}", daemon.log_tail());
    assert!(daemon.log_tail().contains("ROBSON_ENV=production"), "{}", daemon.log_tail());
    let _ = std::fs::remove_dir_all(dir);
}

/// The child's empty local `.env` stops discovery, so a `.env` further up
/// (a checkout's, say) reaches it neither for production nor for anything else.
#[tokio::test]
async fn a_dotenv_in_a_parent_directory_does_not_reach_the_child() {
    let parent = scratch_dir();
    std::fs::write(parent.join(".env"), "ROBSON_ENV=production\n").expect("parent .env");
    let work_dir = parent.join("child");
    std::fs::create_dir_all(&work_dir).expect("child dir");
    let mut spec = stuck_spec(work_dir);
    spec.removed_env.push("ROBSON_ENV".to_string());
    // Lives until its own watchdog: it has an unreachable database to retry.
    spec.extra_env.push(("ROBSON_CRASH_WATCHDOG_SECS".to_string(), "1".to_string()));
    let daemon = DaemonProcess::spawn(&spec).expect("spawn");

    let status = daemon.wait_exit(STEP).await.expect("killed by its own watchdog");
    assert_eq!(status.signal(), Some(9), "not refused, so it ran: {}", daemon.log_tail());
    assert!(daemon.log_tail().contains("development"), "{}", daemon.log_tail());
    let _ = std::fs::remove_dir_all(parent);
}

/// The child-side watchdog: with no test-side deadline in play the child still
/// SIGKILLs itself, so it cannot outlive a test process that was killed.
#[tokio::test]
async fn a_stuck_daemon_kills_itself_at_its_own_deadline() {
    let dir = scratch_dir();
    let mut spec = stuck_spec(dir.clone());
    // Runs long enough to reach its retry loop on the unreachable database.
    spec.extra_env.push(("ROBSON_CRASH_WATCHDOG_SECS".to_string(), "1".to_string()));
    let daemon = DaemonProcess::spawn(&spec).expect("spawn");

    let status = daemon.wait_exit(STEP).await.expect("killed by its own watchdog");
    assert_eq!(status.signal(), Some(9), "SIGKILL expected, got {status}");
    assert!(!daemon.watchdog_fired(), "the test-side watchdog must not be what killed it");
    let _ = std::fs::remove_dir_all(dir);
}

/// SIGKILL just before the send: the same request, one line earlier, and the
/// venue saw nothing. Shows the crash point is at its named place, not merely
/// somewhere in the entry.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn killed_before_the_send_the_venue_saw_nothing(pool: PgPool) {
    let harness = Harness::start(pool).await;

    let daemon = harness.daemon(Some(ENTRY_BEFORE_SEND));
    daemon.wait_ready(STEP).await.expect("daemon ready");
    let request = arm_immediate(daemon.api_url());
    daemon
        .wait_killed_at(ENTRY_BEFORE_SEND, STEP)
        .await
        .expect("killed at the crash point");
    assert!(request.await.unwrap().is_err(), "the caller never got an answer");

    assert_eq!(harness.venue.counts(), VenueCounts::default());
    assert!(harness.venue.position(FAKE_SYMBOL).is_zero());
    let events = harness.event_types().await;
    assert!(events.iter().any(|e| e == "position_armed"), "{events:?}");
}

/// The fake venue is good enough for the whole healthy path of the real
/// binary: entry, exchange stop, operator close, and the reconciliation sweep
/// cancelling the stop that the close left behind.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn with_no_crash_point_the_real_binary_runs_a_full_position(pool: PgPool) {
    let harness = Harness::start(pool).await;

    let daemon = harness.daemon_with_env(None, &[("ROBSON_RECONCILIATION_INTERVAL_SECS", "1")]);
    daemon.wait_ready(STEP).await.expect("daemon ready");
    let armed = arm_immediate(daemon.api_url()).await.unwrap().expect("arm succeeds");
    // Cancels are asserted at the end: the 1 s scan can run at any moment.
    let after_arm = harness.venue.counts();
    assert_eq!((after_arm.accepted_orders, after_arm.fills), (2, 1), "{after_arm:?}");
    assert!(harness.venue.orders()[1].conditional, "{:?}", harness.venue.orders());

    let position_id = armed["position_id"].as_str().expect("position id");
    let response = reqwest::Client::new()
        .delete(format!("{}/positions/{position_id}", daemon.api_url()))
        .timeout(STEP)
        .send()
        .await
        .expect("close");
    assert!(response.status().is_success(), "{}", daemon.log_tail());
    let after_close = harness.venue.counts();
    assert_eq!((after_close.accepted_orders, after_close.fills), (3, 2), "{after_close:?}");

    // The next scan treats the now uncovered stop as an orphan and cancels it.
    let swept = async {
        while harness.venue.counts().cancels == 0 {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    };
    tokio::time::timeout(STEP, swept)
        .await
        .expect("the sweep cancels the orphan stop");
    assert_eq!(harness.venue.counts(), VenueCounts { accepted_orders: 3, fills: 2, cancels: 1 });
    assert!(harness.venue.position(FAKE_SYMBOL).is_zero());
    assert!(
        harness.venue.unserved_requests().is_empty(),
        "{:?}",
        harness.venue.unserved_requests()
    );
}

fn insert_probe(
    id: i32,
) -> sqlx::query::Query<'static, sqlx::Postgres, sqlx::postgres::PgArguments> {
    sqlx::query("INSERT INTO harness_probe (id) VALUES ($1)").bind(id)
}

async fn probe_rows(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM harness_probe")
        .fetch_one(pool)
        .await
        .unwrap()
}

/// Harness and a probe table on the test database, plus a connection through
/// the proxy.
async fn proxied_connection(pool: PgPool) -> (Harness, PgConnection) {
    let harness = Harness::start(pool).await;
    sqlx::query("CREATE TABLE harness_probe (id INT)")
        .execute(&harness.pool)
        .await
        .unwrap();
    let connection = PgConnection::connect(&harness.database_url).await.expect("through the proxy");
    (harness, connection)
}

/// A dropped acknowledgement is the case the journal cares about: the commit
/// landed and the client cannot know. A real SQLx transaction, real PostgreSQL.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn proxy_drops_the_commit_ack_after_the_commit_landed(pool: PgPool) {
    let (harness, mut connection) = proxied_connection(pool).await;

    harness.proxy.set_mode(ProxyMode::DropCommitAck);
    let mut tx = connection.begin().await.unwrap();
    insert_probe(1).execute(&mut *tx).await.unwrap();
    assert!(tx.commit().await.is_err(), "the client must see no acknowledgement");

    assert_eq!(probe_rows(&harness.pool).await, 1, "the commit landed on the server");
    let stats = harness.proxy.stats();
    assert_eq!((stats.commits, stats.commit_acks_dropped), (1, 1));
    assert_eq!(harness.proxy.mode(), ProxyMode::Pass, "one-shot");
}

/// `sqlx::query("COMMIT")` uses the extended protocol, which the fault does not
/// cover: it is acknowledged, not counted, and leaves the fault armed.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn proxy_leaves_an_extended_protocol_commit_alone(pool: PgPool) {
    let (harness, mut connection) = proxied_connection(pool).await;

    harness.proxy.set_mode(ProxyMode::DropCommitAck);
    sqlx::query("BEGIN").execute(&mut connection).await.unwrap();
    insert_probe(1).execute(&mut connection).await.unwrap();
    sqlx::query("COMMIT").execute(&mut connection).await.expect("acknowledged");

    assert_eq!(probe_rows(&harness.pool).await, 1);
    let stats = harness.proxy.stats();
    assert_eq!((stats.commits, stats.commit_acks_dropped), (0, 0));
    assert_eq!(harness.proxy.mode(), ProxyMode::DropCommitAck, "still armed");
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn proxy_passes_a_commit_through(pool: PgPool) {
    let (harness, mut connection) = proxied_connection(pool).await;

    let mut tx = connection.begin().await.unwrap();
    insert_probe(1).execute(&mut *tx).await.unwrap();
    tx.commit().await.expect("acknowledged");

    assert_eq!(probe_rows(&harness.pool).await, 1);
    assert_eq!(harness.proxy.stats().commits, 1);
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn proxy_sever_closes_connections_until_passing_again(pool: PgPool) {
    let (harness, mut connection) = proxied_connection(pool).await;

    harness.proxy.set_mode(ProxyMode::Sever);
    assert!(insert_probe(1).execute(&mut connection).await.is_err());
    assert!(PgConnection::connect(&harness.database_url).await.is_err());
    assert_eq!(probe_rows(&harness.pool).await, 0);

    harness.proxy.set_mode(ProxyMode::Pass);
    let mut fresh = PgConnection::connect(&harness.database_url).await.expect("recovered");
    insert_probe(2).execute(&mut fresh).await.unwrap();
    assert_eq!(probe_rows(&harness.pool).await, 1);
}

/// A blackholed statement never reaches the server, and the client learns
/// nothing until its own timeout.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn proxy_blackhole_leaves_the_client_to_its_own_timeout(pool: PgPool) {
    let (harness, mut connection) = proxied_connection(pool).await;

    harness.proxy.set_mode(ProxyMode::Blackhole);
    let statement = insert_probe(1).execute(&mut connection);
    assert!(tokio::time::timeout(Duration::from_millis(500), statement).await.is_err());
    assert_eq!(probe_rows(&harness.pool).await, 0, "nothing reached the server");
}
