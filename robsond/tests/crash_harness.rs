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

use std::{future::Future, os::unix::process::ExitStatusExt, path::PathBuf, time::Duration};

use robson_exec::crash_points::{
    ENTRY_AFTER_ACK, ENTRY_BEFORE_SEND, PROTECTION_BEFORE_SEND, REPAIR_AFTER_PAGE, STALL_ENV,
    STARTUP_BEFORE_SCAN, STOP_AFTER_MARKER, STOP_BEFORE_MARKER,
};
use robson_testkit::{
    daemon_process::{database_url_through, DaemonProcess, DaemonSpec},
    db_proxy::{DbProxy, ProxyMode},
    fake_venue::{FakeVenue, VenueCounts, FAKE_SYMBOL},
};
use robsond::stop::{HARD_EXIT, HARD_EXIT_CODE, STOP_DEADLINE};
use rust_decimal::Decimal;
use serde_json::{json, Value};
use sqlx::{Connection, PgConnection, PgPool};
use uuid::Uuid;

/// Hard limit for one daemon process; the watchdogs enforce it.
const DAEMON_DEADLINE: Duration = Duration::from_secs(90);
/// Time a step may take before the test fails with the daemon's log.
const STEP: Duration = Duration::from_secs(60);

/// Test-body limits: a stuck database or daemon wait inside the body fails the
/// test, and dropping the test future kills and reaps any daemon it still
/// holds. SQLx's per-test setup and cleanup sit outside them, and a failed test
/// can leave its database behind on the server.
const DAEMON_TEST_LIMIT: Duration = Duration::from_secs(240);
const PROXY_TEST_LIMIT: Duration = Duration::from_secs(30);

async fn within<T>(limit: Duration, test: impl Future<Output = T>) -> T {
    tokio::time::timeout(limit, test).await.expect("the test exceeded its limit")
}

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
        DaemonProcess::spawn(&self.spec(crash_point, extra_env)).expect("spawn robsond")
    }

    fn spec(&self, crash_point: Option<&str>, extra_env: &[(&str, &str)]) -> DaemonSpec {
        DaemonSpec {
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
        }
    }

    /// The journal's view of its intents of `kind`: client id, outcome,
    /// `uncertain_at` set, in creation order.
    async fn intents(&self, kind: &str) -> Vec<(String, Option<String>, bool)> {
        sqlx::query_as(
            "SELECT client_order_id, venue_outcome, uncertain_at IS NOT NULL
             FROM order_intents WHERE kind = $1 ORDER BY created_at",
        )
        .bind(kind)
        .fetch_all(&self.pool)
        .await
        .expect("read order_intents")
    }

    async fn blockers(&self) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*)::BIGINT FROM order_intents_blocking_rollback")
            .fetch_one(&self.pool)
            .await
            .expect("read the boundary view")
    }

    /// Wait until `done` holds on the database, for writes that land through
    /// the journal's retry loop.
    async fn until<F, Fut>(&self, what: &str, mut done: F)
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = bool>,
    {
        let waiting = async {
            while !done().await {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        };
        tokio::time::timeout(STEP, waiting)
            .await
            .unwrap_or_else(|_| panic!("timed out: {what}"));
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
    within(DAEMON_TEST_LIMIT, async move {
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

        assert_eq!(harness.venue.counts(), VenueCounts {
            accepted_orders: 1,
            fills: 1,
            cancels: 0
        });
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
        // The intent is on record and nothing says how it ended. It blocks a
        // rollback across this schema until an operator decides on it.
        let intents = harness.intents("entry").await;
        assert_eq!(intents.len(), 1, "{intents:?}");
        assert_eq!((intents[0].1.clone(), intents[0].2), (None, false));
        assert_eq!(harness.blockers().await, 1);
        drop(daemon);

        // The projection still reads Armed, so the restart stops at the startup
        // gate (exit 78) before any sweep. Nothing is closed: the fill stays open
        // on the venue with no stop.
        let restarted = harness.daemon(None);
        let status = restarted.wait_exit(STEP).await.expect("the restart stops by itself");
        assert_eq!(status.code(), Some(78), "startup gate\n{}", restarted.log_tail());
        assert_eq!(harness.venue.counts(), VenueCounts {
            accepted_orders: 1,
            fills: 1,
            cancels: 0
        });
        assert!(harness.venue.position(FAKE_SYMBOL) > Decimal::ZERO);
        assert_eq!(harness.intents("entry").await, intents, "a restart invents no outcome");
    })
    .await;
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
    within(DAEMON_TEST_LIMIT, async move {
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

        // The commit came first: the row outlives the process.
        let intents = harness.intents("entry").await;
        assert_eq!(intents.len(), 1, "{intents:?}");
        assert_eq!(intents[0].1, None);
        drop(daemon);

        // A later process finds the id spent: whatever it does next, it never
        // sends that id again.
        let restarted = harness.daemon(None);
        restarted.wait_ready(STEP).await.expect("daemon ready");
        tokio::time::sleep(Duration::from_secs(3)).await;
        assert!(
            harness.venue.orders().iter().all(|order| order.client_id != intents[0].0),
            "{:?}",
            harness.venue.orders()
        );
    })
    .await;
}

/// SIGKILL with a stop's intent committed and its request not sent. The entry
/// is on record as a success and the stop is outstanding, across the process's
/// death. Whatever the next process sends, it does not use the journaled id.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn killed_with_a_committed_stop_intent_the_row_outlives_the_process(pool: PgPool) {
    within(DAEMON_TEST_LIMIT, async move {
        let harness = Harness::start(pool).await;
        let daemon = harness.daemon(Some(PROTECTION_BEFORE_SEND));
        daemon.wait_ready(STEP).await.expect("daemon ready");
        let request = arm_immediate(daemon.api_url());
        daemon
            .wait_killed_at(PROTECTION_BEFORE_SEND, STEP)
            .await
            .expect("killed at the crash point");
        assert!(request.await.unwrap().is_err(), "the caller never got an answer");

        let entries = harness.intents("entry").await;
        assert_eq!(entries[0].1.as_deref(), Some("success"), "{entries:?}");
        let stops = harness.intents("protection").await;
        assert_eq!(stops.len(), 1, "{stops:?}");
        assert_eq!(stops[0].1, None);
        assert_eq!(harness.blockers().await, 1);
        assert_eq!(harness.venue.counts().accepted_orders, 1, "the stop never left");
        drop(daemon);

        let restarted = harness.daemon(None);
        restarted.wait_ready(STEP).await.expect("daemon ready");
        tokio::time::sleep(Duration::from_secs(3)).await;
        let orders = harness.venue.orders();
        assert!(
            orders.iter().all(|order| order.client_id != stops[0].0),
            "{orders:?}\n{}",
            restarted.log_tail()
        );
        assert_eq!(harness.intents("protection").await, stops, "no outcome is invented");
    })
    .await;
}

/// The acknowledgement of a real entry commit is lost in the real daemon. The
/// row landed and nothing tells it, so nothing is sent and the detector is not
/// handed back. Aimed at the journal's commit; every other write of the daemon
/// passes.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_lost_ack_on_the_entrys_commit_sends_nothing(pool: PgPool) {
    within(DAEMON_TEST_LIMIT, async move {
        let harness = Harness::start(pool).await;
        let daemon = harness.daemon(None);
        daemon.wait_ready(STEP).await.expect("daemon ready");

        harness.proxy.set_mode(ProxyMode::DropCommitAckOf("order_intents"));
        let answer = arm_immediate(daemon.api_url()).await.unwrap();

        assert!(answer.is_err(), "{answer:?}");
        assert_eq!(harness.proxy.stats().commit_acks_dropped, 1);
        assert_eq!(harness.venue.counts(), VenueCounts::default(), "{}", daemon.log_tail());
        // The journal's retry writes the stamp on the row the lost commit left.
        harness
            .until("the uncertain stamp", || async {
                harness.intents("entry").await.first().is_some_and(|intent| intent.2)
            })
            .await;
        let intents = harness.intents("entry").await;
        assert_eq!((intents.len(), intents[0].1.clone()), (1, None));
        assert_eq!(harness.blockers().await, 1);
    })
    .await;
}

/// A stalled journal: the entry's commit times out at its bound and nothing is
/// sent. Once the journal recovers the row lands as uncertain.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_timeout_on_the_entrys_commit_sends_nothing(pool: PgPool) {
    within(DAEMON_TEST_LIMIT, async move {
        let harness = Harness::start(pool).await;
        let daemon = harness.daemon(None);
        daemon.wait_ready(STEP).await.expect("daemon ready");
        for statement in [
            "CREATE FUNCTION stall_journal() RETURNS trigger LANGUAGE plpgsql AS $$
             BEGIN PERFORM pg_sleep(2); RETURN NEW; END $$",
            "CREATE TRIGGER stall_journal BEFORE INSERT ON order_intents
             FOR EACH ROW EXECUTE FUNCTION stall_journal()",
        ] {
            sqlx::query(statement).execute(&harness.pool).await.expect("stall");
        }

        let answer = arm_immediate(daemon.api_url()).await.unwrap();
        assert!(answer.is_err(), "{answer:?}");
        assert_eq!(harness.venue.counts(), VenueCounts::default(), "{}", daemon.log_tail());

        sqlx::query("DROP TRIGGER stall_journal ON order_intents")
            .execute(&harness.pool)
            .await
            .expect("recover");
        harness
            .until("the uncertain row", || async {
                harness.intents("entry").await.first().is_some_and(|intent| intent.2)
            })
            .await;
    })
    .await;
}

/// A journal schema that is not there stops the daemon at startup, before it
/// could send anything.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_daemon_refuses_to_start_without_the_journal_tables(pool: PgPool) {
    within(DAEMON_TEST_LIMIT, async move {
        let harness = Harness::start(pool).await;
        for statement in [
            "DROP VIEW order_intents_blocking_rollback",
            "DROP TABLE submit_attempts",
        ] {
            sqlx::query(statement).execute(&harness.pool).await.expect("drop");
        }

        let daemon = harness.daemon(None);
        let status = daemon.wait_exit(STEP).await.expect("the start is refused");

        assert_ne!(status.code(), Some(0), "{}", daemon.log_tail());
        assert!(daemon.log_tail().contains("Order journal not ready"), "{}", daemon.log_tail());
        assert_eq!(harness.venue.counts(), VenueCounts::default());
    })
    .await;
}

/// An outstanding intent from a dead process is decided on through the real
/// endpoint, with the identity recorded, and the boundary then holds. No
/// outcome is invented.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn an_operator_disposition_clears_the_rollback_boundary(pool: PgPool) {
    within(DAEMON_TEST_LIMIT, async move {
        let harness = Harness::start(pool).await;
        let daemon = harness.daemon(Some(ENTRY_BEFORE_SEND));
        daemon.wait_ready(STEP).await.expect("daemon ready");
        let request = arm_immediate(daemon.api_url());
        daemon.wait_killed_at(ENTRY_BEFORE_SEND, STEP).await.expect("killed");
        let _ = request.await;
        drop(daemon);
        assert_eq!(harness.blockers().await, 1);

        let token = "harness-only-token";
        let restarted = harness.daemon_with_env(None, &[("ROBSON_LEGACY_API_TOKEN", token)]);
        restarted.wait_ready(STEP).await.expect("daemon ready");
        let intent_id: Uuid = sqlx::query_scalar("SELECT intent_id FROM order_intents LIMIT 1")
            .fetch_one(&harness.pool)
            .await
            .expect("the intent");

        let response = reqwest::Client::new()
            .post(format!("{}/recovery/disposition", restarted.api_url()))
            .bearer_auth(token)
            .timeout(STEP)
            .json(&json!({
                "intent_id": intent_id,
                "disposition_kind": "accepted_risk",
                "treatment": "unbooked_accepted",
                "note": "harness",
            }))
            .send()
            .await
            .expect("the endpoint answers");
        assert!(response.status().is_success(), "{}", restarted.log_tail());
        let body: Value = response.json().await.expect("json");
        assert_eq!(body["boundary_b_blockers"], 0, "{body}");
        let outcome: Option<String> =
            sqlx::query_scalar("SELECT venue_outcome FROM order_intents WHERE intent_id = $1")
                .bind(intent_id)
                .fetch_one(&harness.pool)
                .await
                .expect("the intent");
        assert_eq!(outcome, None);
    })
    .await;
}

/// The fake venue is good enough for the whole healthy path of the real
/// binary: entry, exchange stop and operator close are served and counted, and
/// a cancel from the reconciliation sweep reaches the venue. When the sweep
/// cancels relative to the close is not controlled here.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn with_no_crash_point_the_real_binary_runs_a_full_position(pool: PgPool) {
    within(DAEMON_TEST_LIMIT, async move {
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

        // A scan cancels the stop, at some moment since the arm.
        let swept = async {
            while harness.venue.counts().cancels == 0 {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        };
        tokio::time::timeout(STEP, swept)
            .await
            .expect("the sweep cancels the orphan stop");
        assert_eq!(harness.venue.counts(), VenueCounts {
            accepted_orders: 3,
            fills: 2,
            cancels: 1
        });
        assert!(harness.venue.position(FAKE_SYMBOL).is_zero());
        assert!(
            harness.venue.unserved_requests().is_empty(),
            "{:?}",
            harness.venue.unserved_requests()
        );
    })
    .await;
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
    within(PROXY_TEST_LIMIT, async move {
        let (harness, mut connection) = proxied_connection(pool).await;

        harness.proxy.set_mode(ProxyMode::DropCommitAck);
        let mut tx = connection.begin().await.unwrap();
        insert_probe(1).execute(&mut *tx).await.unwrap();
        assert!(tx.commit().await.is_err(), "the client must see no acknowledgement");

        assert_eq!(probe_rows(&harness.pool).await, 1, "the commit landed on the server");
        let stats = harness.proxy.stats();
        assert_eq!((stats.commits, stats.commit_acks_dropped), (1, 1));
        assert_eq!(harness.proxy.mode(), ProxyMode::Pass, "one-shot");
    })
    .await;
}

/// `sqlx::query("COMMIT")` uses the extended protocol, which the fault does not
/// cover: it is acknowledged, not counted, and leaves the fault armed.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn proxy_leaves_an_extended_protocol_commit_alone(pool: PgPool) {
    within(PROXY_TEST_LIMIT, async move {
        let (harness, mut connection) = proxied_connection(pool).await;

        harness.proxy.set_mode(ProxyMode::DropCommitAck);
        sqlx::query("BEGIN").execute(&mut connection).await.unwrap();
        insert_probe(1).execute(&mut connection).await.unwrap();
        sqlx::query("COMMIT").execute(&mut connection).await.expect("acknowledged");

        assert_eq!(probe_rows(&harness.pool).await, 1);
        let stats = harness.proxy.stats();
        assert_eq!((stats.commits, stats.commit_acks_dropped), (0, 0));
        assert_eq!(harness.proxy.mode(), ProxyMode::DropCommitAck, "still armed");
    })
    .await;
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn proxy_passes_a_commit_through(pool: PgPool) {
    within(PROXY_TEST_LIMIT, async move {
        let (harness, mut connection) = proxied_connection(pool).await;

        let mut tx = connection.begin().await.unwrap();
        insert_probe(1).execute(&mut *tx).await.unwrap();
        tx.commit().await.expect("acknowledged");

        assert_eq!(probe_rows(&harness.pool).await, 1);
        assert_eq!(harness.proxy.stats().commits, 1);
    })
    .await;
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn proxy_sever_closes_connections_until_passing_again(pool: PgPool) {
    within(PROXY_TEST_LIMIT, async move {
        let (harness, mut connection) = proxied_connection(pool).await;

        harness.proxy.set_mode(ProxyMode::Sever);
        assert!(insert_probe(1).execute(&mut connection).await.is_err());
        assert!(PgConnection::connect(&harness.database_url).await.is_err());
        assert_eq!(probe_rows(&harness.pool).await, 0);

        harness.proxy.set_mode(ProxyMode::Pass);
        let mut fresh = PgConnection::connect(&harness.database_url).await.expect("recovered");
        insert_probe(2).execute(&mut fresh).await.unwrap();
        assert_eq!(probe_rows(&harness.pool).await, 1);
    })
    .await;
}

/// A blackholed statement never reaches the server, and the client learns
/// nothing until its own timeout.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn proxy_blackhole_leaves_the_client_to_its_own_timeout(pool: PgPool) {
    within(PROXY_TEST_LIMIT, async move {
        let (harness, mut connection) = proxied_connection(pool).await;

        harness.proxy.set_mode(ProxyMode::Blackhole);
        let statement = insert_probe(1).execute(&mut connection);
        assert!(tokio::time::timeout(Duration::from_millis(500), statement).await.is_err());
        assert_eq!(probe_rows(&harness.pool).await, 0, "nothing reached the server");
    })
    .await;
}

// ---- Permits, epochs and the stop (ADR-0056 step 7, PR S) -------------------

/// The send start against a phase change, through the real adapter, ledger
/// and transport to the fake venue (design §8, interleaving 3, for the stop):
/// every entry that reached the venue was ticketed while the daemon was open,
/// none started its send after Draining began, and every one refused is a
/// typed no-send that left no ticket.
#[test]
fn every_entry_reaching_the_venue_was_ticketed_while_open() {
    use std::sync::Arc;

    use proptest::prelude::*;
    use robson_connectors::{binance::usdm::BinanceExchangeAdapter, BinanceRestClient};
    use robson_domain::{OrderSide, Quantity, Symbol};
    use robson_exec::{
        gate::OperationKind,
        order_journal::{IntentKind, IntentSpec, OrderJournal, StubJournalBackend, WriteClass},
        venue::{EntryOrReduce, TradingPort, Venue, VenueId},
        ExecError,
    };

    let config = ProptestConfig { cases: 24, ..ProptestConfig::default() };
    proptest!(config, |(delays in proptest::collection::vec(0usize..6, 1..8), flip in 0usize..8)| {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let venue = FakeVenue::start().await.expect("fake venue");
            let client = Arc::new(BinanceRestClient::with_harness_base_url(
                venue.url(),
                "fixture-key".to_string(),
                "fixture-secret".to_string(),
            ));
            let adapter = Arc::new(BinanceExchangeAdapter::new(client));
            let journal = Arc::new(OrderJournal::new(
                Arc::new(StubJournalBackend::default()),
                &VenueId::new("test:venue"),
            ));
            adapter.attach_order_journal(Arc::clone(&journal));
            adapter.attach_gate(Arc::clone(journal.gate()));

            let entries: Vec<_> = delays
                .iter()
                .enumerate()
                .map(|(i, &delay)| {
                    let permit = journal.acquire(OperationKind::Arm).unwrap();
                    let (journal, adapter) = (Arc::clone(&journal), Arc::clone(&adapter));
                    tokio::spawn(async move {
                        permit
                            .run(async {
                                let id = format!("e{i}");
                                let intent = journal.open_intent(IntentSpec {
                                    position_id: None,
                                    kind: IntentKind::Entry,
                                    client_order_id: id.clone(),
                                    cancels_order_id: None,
                                    params: serde_json::Value::Null,
                                });
                                journal.before_send(&intent, WriteClass::Strict).await.unwrap();
                                for _ in 0..delay {
                                    tokio::task::yield_now().await;
                                }
                                let sent = TradingPort::place_market_order(
                                    &*adapter,
                                    &Symbol::from_pair(FAKE_SYMBOL).unwrap(),
                                    OrderSide::Buy,
                                    Quantity::new(Decimal::new(1, 1)).unwrap(),
                                    &id,
                                    EntryOrReduce::Entry,
                                )
                                .await;
                                (sent, journal.ticket_issued(&intent))
                            })
                            .await
                    })
                })
                .collect();
            let flipped = {
                let journal = Arc::clone(&journal);
                tokio::spawn(async move {
                    for _ in 0..flip {
                        tokio::task::yield_now().await;
                    }
                    journal.gate().begin_draining();
                    journal.gate().snapshot().sends_started
                })
            };
            let started_while_open = flipped.await.unwrap();
            let mut ticketed = 0;
            for entry in entries {
                let (sent, ticket) = entry.await.unwrap();
                match sent {
                    Ok(_) => {
                        assert!(ticket, "a send that went out has its ticket");
                        ticketed += 1;
                    },
                    Err(ExecError::SendRefused(_)) => assert!(!ticket, "a refused entry has none"),
                    Err(other) => panic!("unexpected: {other:?}"),
                }
            }
            assert_eq!(ticketed, started_while_open, "every ticket was issued while open");
            assert_eq!(journal.gate().snapshot().sends_started, started_while_open);
            assert_eq!(venue.counts().accepted_orders, started_while_open);
        });
    });
}

/// A client that disconnects while its operation is in flight does not cancel
/// it: the API runs the operation in its own task, which owns the permit. A
/// real TCP client sends `/panic`, its exit is held at the venue, the client
/// closes its socket, then the venue answers: the panic completes, nothing is
/// aborted and the latch stays clear.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_client_disconnecting_mid_operation_leaves_it_to_complete(pool: PgPool) {
    use tokio::io::AsyncWriteExt;

    within(DAEMON_TEST_LIMIT, async move {
        let harness = Harness::start(pool).await;
        let daemon = harness.daemon(None);
        daemon.wait_ready(STEP).await.expect("daemon ready");
        arm_immediate(daemon.api_url()).await.unwrap().expect("arm succeeds");
        assert_ne!(harness.venue.position(FAKE_SYMBOL), Decimal::ZERO);

        harness.venue.hold_orders();
        let api = daemon.api_url();
        let mut socket = tokio::net::TcpStream::connect(api.trim_start_matches("http://"))
            .await
            .expect("connect to the API");
        socket
            .write_all(b"POST /panic HTTP/1.1\r\nHost: robsond\r\nContent-Length: 0\r\n\r\n")
            .await
            .expect("send /panic");
        let held = async {
            while harness.venue.held_orders() == 0 {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        };
        tokio::time::timeout(STEP, held)
            .await
            .expect("the exit order reaches the venue");
        drop(socket);
        // Long enough for the server to read the end of the stream.
        tokio::time::sleep(Duration::from_millis(500)).await;
        harness.venue.release_orders();

        harness
            .until("the exit lands", || async {
                harness.intents("exit").await.iter().any(|(_, outcome, _)| outcome.is_some())
            })
            .await;
        assert_eq!(harness.venue.position(FAKE_SYMBOL), Decimal::ZERO, "the leg is flat");
        let exits = harness.intents("exit").await;
        assert_eq!(exits.len(), 1, "{exits:?}");
        assert!(!exits[0].2, "an answered exit, not an uncertain one: {exits:?}");
        // The latch gauge is published once a second.
        tokio::time::sleep(Duration::from_millis(1_500)).await;
        let metrics = reqwest::Client::new()
            .get(format!("{api}/metrics"))
            .timeout(STEP)
            .send()
            .await
            .expect("metrics")
            .text()
            .await
            .expect("metrics body");
        for series in [
            "robsond_aborted_operations_total 0",
            "robsond_journal_latch 0",
        ] {
            assert!(metrics.lines().any(|line| line == series), "{series}\n{metrics}");
        }
    })
    .await;
}

impl Harness {
    /// This tenant's epochs, oldest first: marked clean, marker version.
    async fn epochs(&self) -> Vec<(bool, Option<i16>)> {
        sqlx::query_as(
            "SELECT clean_stop_at IS NOT NULL, marker_version FROM process_epochs
             WHERE tenant_id = $1 ORDER BY epoch_seq",
        )
        .bind(self.tenant_id)
        .fetch_all(&self.pool)
        .await
        .expect("read process_epochs")
    }
}

/// The log without its colour codes.
fn plain(log: String) -> String {
    let mut out = String::with_capacity(log.len());
    let mut chars = log.chars();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            for c in chars.by_ref() {
                if c.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// The epoch lands before any venue request: a start whose epoch the database
/// refuses exits having sent the venue nothing at all.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_start_that_cannot_record_its_epoch_sends_the_venue_nothing(pool: PgPool) {
    within(DAEMON_TEST_LIMIT, async move {
        let harness = Harness::start(pool).await;
        sqlx::query(
            "CREATE FUNCTION refuse_epochs() RETURNS trigger AS $$
             BEGIN RAISE EXCEPTION 'epochs refused by the test'; END $$ LANGUAGE plpgsql",
        )
        .execute(&harness.pool)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TRIGGER refuse_epochs BEFORE INSERT ON process_epochs
             FOR EACH ROW EXECUTE FUNCTION refuse_epochs()",
        )
        .execute(&harness.pool)
        .await
        .unwrap();

        let daemon = harness.daemon(None);
        let status = daemon.wait_exit(STEP).await.expect("it refuses to start");
        assert!(!status.success(), "{status}\n{}", daemon.log_tail());
        assert!(
            plain(daemon.log()).contains("epochs refused by the test"),
            "{}",
            daemon.log_tail()
        );
        assert_eq!(harness.venue.requests(), 0, "no venue request before the epoch");
        assert!(harness.epochs().await.is_empty());
    })
    .await;
}

/// A venue that can mutate never starts without its epoch: the real binary, on
/// USD-M with usable credentials and no `DATABASE_URL`, refuses to start
/// before any request reaches the venue. Only the stub runs without one.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_mutating_venue_without_a_database_does_not_start(pool: PgPool) {
    within(DAEMON_TEST_LIMIT, async move {
        let harness = Harness::start(pool).await;
        let daemon = DaemonProcess::spawn(&DaemonSpec {
            removed_env: vec!["DATABASE_URL".to_string()],
            ..harness.spec(None, &[])
        })
        .expect("spawn robsond");
        let status = daemon.wait_exit(STEP).await.expect("it refuses to start");
        assert!(!status.success(), "{status}\n{}", daemon.log_tail());
        assert!(
            plain(daemon.log()).contains("starts only on a process epoch"),
            "{}",
            daemon.log_tail()
        );
        assert_eq!(harness.venue.requests(), 0, "no venue request without an epoch");
        assert!(harness.epochs().await.is_empty());
    })
    .await;
}

/// The pod's grace period: Kubernetes SIGKILLs after it.
const GRACE: Duration = Duration::from_secs(30);

/// SIGTERM, and SIGINT on the next run, stop the real process: exit 0 well
/// inside the grace period, its epoch marked at version 1. The second start
/// finds that marked predecessor, which counts as unclean until the fence PR.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_signal_stops_the_real_process_clean_and_marks_its_epoch(pool: PgPool) {
    within(DAEMON_TEST_LIMIT, async move {
        let harness = Harness::start(pool).await;
        for (run, signal) in ["TERM", "INT"].into_iter().enumerate() {
            let daemon = harness.daemon(None);
            daemon.wait_ready(STEP).await.expect("daemon ready");
            let asked = std::time::Instant::now();
            daemon.send_signal(signal).expect("signal");
            let status = daemon.wait_exit(GRACE).await.expect("it stops by itself");
            let stopped_in = asked.elapsed();
            let log = plain(daemon.log());
            assert!(status.success(), "{signal}: {status}\n{}", daemon.log_tail());
            assert!(stopped_in < Duration::from_secs(25), "{signal}: {stopped_in:?}");
            assert!(log.contains(&format!("cause=\"SIG{signal}\"")), "{log}");
            assert!(log.contains("Stop: clean, epoch marked"), "{log}");
            if run == 1 {
                assert!(log.contains("counts as unclean until the fence PR"), "{log}");
            }
        }
        assert_eq!(harness.epochs().await, [(true, Some(1)), (true, Some(1))]);
        assert!(harness.venue.unserved_requests().is_empty());
    })
    .await;
}

/// Killed just before its marker, the epoch stays unmarked and the next start
/// finds an unclean predecessor; killed just after it, the marker stands.
/// Real deaths at named places, after a real SIGTERM.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn killed_around_its_marker_the_epoch_says_whether_it_was_written(pool: PgPool) {
    within(DAEMON_TEST_LIMIT, async move {
        let harness = Harness::start(pool).await;

        let daemon = harness.daemon(Some(STOP_BEFORE_MARKER));
        daemon.wait_ready(STEP).await.expect("daemon ready");
        daemon.send_signal("TERM").expect("signal");
        daemon.wait_killed_at(STOP_BEFORE_MARKER, GRACE).await.expect("killed before");
        assert_eq!(harness.epochs().await, [(false, None)]);

        let daemon = harness.daemon(Some(STOP_AFTER_MARKER));
        daemon.wait_ready(STEP).await.expect("daemon ready");
        let log = plain(daemon.log());
        assert!(log.contains("Process epoch started after an unclean stop"), "{log}");
        assert!(log.contains("no_marker"), "{log}");
        daemon.send_signal("TERM").expect("signal");
        daemon.wait_killed_at(STOP_AFTER_MARKER, GRACE).await.expect("killed after");
        assert_eq!(harness.epochs().await, [(false, None), (true, Some(1))]);
    })
    .await;
}

/// A stop with `/panic` in flight, its exit order held by the venue: Draining
/// waits for the panic, refusing an ordinary request meanwhile, and the stop is
/// clean once the order's 10 s bound ends it. The timed-out exit is uncertain
/// and recorded, which a clean marker allows before the fence PR.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_stop_waits_for_a_panic_in_flight_and_refuses_ordinary_requests_meanwhile(pool: PgPool) {
    within(DAEMON_TEST_LIMIT, async move {
        let harness = Harness::start(pool).await;
        let daemon = harness.daemon(None);
        daemon.wait_ready(STEP).await.expect("daemon ready");
        arm_immediate(daemon.api_url()).await.unwrap().expect("arm succeeds");
        assert_eq!(harness.venue.counts().fills, 1);

        harness.venue.hold_orders();
        let api = daemon.api_url();
        let panic = tokio::spawn(async move {
            reqwest::Client::new()
                .post(format!("{api}/panic"))
                .timeout(STEP)
                .send()
                .await
                .map(|response| response.status())
        });
        // The exit order reaches the venue and is held there, with the panic's
        // permit held.
        let held = async {
            while harness.venue.held_orders() == 0 {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        };
        tokio::time::timeout(STEP, held)
            .await
            .expect("the exit order reaches the venue");
        let asked = std::time::Instant::now();
        daemon.send_signal("TERM").expect("signal");
        tokio::time::sleep(Duration::from_millis(500)).await;

        let refused = reqwest::Client::new()
            .post(format!("{}/positions", daemon.api_url()))
            .timeout(STEP)
            .json(&json!({ "symbol": FAKE_SYMBOL, "side": "long" }))
            .send()
            .await
            .expect("the listener is open while draining");
        assert_eq!(refused.status(), reqwest::StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(refused.json::<Value>().await.unwrap()["error"], "stopping");

        let status = daemon.wait_exit(GRACE).await.expect("it stops by itself");
        let stopped_in = asked.elapsed();
        assert!(status.success(), "{status}\n{}", daemon.log_tail());
        assert!(panic.await.unwrap().is_ok(), "the panic got its answer");
        assert!(
            stopped_in > Duration::from_secs(5) && stopped_in < Duration::from_secs(25),
            "draining waited for the panic, within the deadline: {stopped_in:?}"
        );
        assert!(
            plain(daemon.log()).contains("Stop: clean, epoch marked"),
            "{}",
            daemon.log_tail()
        );
        assert_eq!(harness.epochs().await, [(true, Some(1))]);
        let exits = harness.intents("exit").await;
        assert_eq!(exits.len(), 1, "{exits:?}");
        assert_eq!((exits[0].1.clone(), exits[0].2), (None, true), "uncertain, on record");
    })
    .await;
}

/// An operation that never ends holds the process no longer than the stop's
/// deadline. The arm's thread stalls for good after the venue's ack, with its
/// permit held. On SIGTERM the stop waits for it until the deadline and ends
/// unclean; the daemon stops waiting, and the runtime's teardown abandons the
/// stalled thread: exit 0 after the deadline and before the hard bound, the
/// epoch unmarked.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_stuck_operation_holds_the_process_no_longer_than_the_deadline(pool: PgPool) {
    within(DAEMON_TEST_LIMIT, async move {
        let harness = Harness::start(pool).await;
        let daemon = harness.daemon_with_env(Some(ENTRY_AFTER_ACK), &[(STALL_ENV, "1")]);
        daemon.wait_ready(STEP).await.expect("daemon ready");
        let arm = arm_immediate(daemon.api_url());
        daemon.wait_stalled_at(ENTRY_AFTER_ACK, STEP).await.expect("the arm stalls");
        let asked = std::time::Instant::now();
        daemon.send_signal("TERM").expect("signal");

        let status = daemon.wait_exit(GRACE).await.expect("it exits by itself");
        let stopped_in = asked.elapsed();
        eprintln!("a_stuck_operation_holds_the_process_no_longer_than_the_deadline: exited {stopped_in:?} after the signal, {status}");
        assert!(status.success(), "{status}\n{}", daemon.log_tail());
        assert!(
            stopped_in >= STOP_DEADLINE && stopped_in < HARD_EXIT,
            "after the deadline, before the hard bound: {stopped_in:?}\n{}",
            daemon.log_tail()
        );
        let log = plain(daemon.log());
        assert!(log.contains("Stop: not clean, no marker written"), "{}", daemon.log_tail());
        assert!(log.contains("deadline"), "{}", daemon.log_tail());
        assert_eq!(harness.epochs().await, [(false, None)]);
        assert!(arm.await.unwrap().is_err(), "the stalled arm never answered");
    })
    .await;
}

/// The hard bound. The startup's own thread stalls for good, the startup
/// permit held, so nothing the async runtime does can end the process. On
/// SIGTERM the stop ends unclean at its deadline, and the thread started at
/// the signal exits the process at the hard bound with code 70, its only
/// record. No marker.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_process_that_cannot_finish_is_ended_by_the_hard_bound(pool: PgPool) {
    within(DAEMON_TEST_LIMIT, async move {
        let harness = Harness::start(pool).await;
        let daemon = harness.daemon_with_env(Some(STARTUP_BEFORE_SCAN), &[(STALL_ENV, "1")]);
        daemon
            .wait_stalled_at(STARTUP_BEFORE_SCAN, STEP)
            .await
            .expect("the startup stalls");
        let asked = std::time::Instant::now();
        daemon.send_signal("TERM").expect("signal");

        let status = daemon.wait_exit(GRACE).await.expect("it exits by the hard bound");
        let stopped_in = asked.elapsed();
        eprintln!("a_process_that_cannot_finish_is_ended_by_the_hard_bound: exited {stopped_in:?} after the signal, {status}");
        assert_eq!(status.code(), Some(HARD_EXIT_CODE), "{status}\n{}", daemon.log_tail());
        assert!(
            stopped_in >= HARD_EXIT && stopped_in < GRACE,
            "at the hard bound, inside the grace: {stopped_in:?}\n{}",
            daemon.log_tail()
        );
        let log = plain(daemon.log());
        assert!(log.contains("Stop: not clean, no marker written"), "{}", daemon.log_tail());
        assert!(!daemon.watchdog_fired());
        assert_eq!(harness.epochs().await, [(false, None)]);
    })
    .await;
}

/// A signal during the start, before anything was sent, ends the start: the
/// epoch's insert waits in the database, SIGTERM arrives, and the process
/// exits 0 at once, the venue sent nothing and no epoch recorded.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_signal_during_the_epoch_insert_ends_the_start_with_nothing_sent(pool: PgPool) {
    within(DAEMON_TEST_LIMIT, async move {
        let harness = Harness::start(pool).await;
        sqlx::query(
            "CREATE FUNCTION stall_epochs() RETURNS trigger AS $$
             BEGIN PERFORM pg_sleep(30); RETURN NEW; END $$ LANGUAGE plpgsql",
        )
        .execute(&harness.pool)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TRIGGER stall_epochs BEFORE INSERT ON process_epochs
             FOR EACH ROW EXECUTE FUNCTION stall_epochs()",
        )
        .execute(&harness.pool)
        .await
        .unwrap();
        let inserting = "SELECT COUNT(*)::BIGINT FROM pg_stat_activity
             WHERE datname = current_database() AND state = 'active'
             AND query LIKE 'INSERT INTO process_epochs%'";

        let daemon = harness.daemon(None);
        harness
            .until("the epoch insert waits in the database", || async {
                sqlx::query_scalar::<_, i64>(inserting).fetch_one(&harness.pool).await.unwrap() > 0
            })
            .await;
        let asked = std::time::Instant::now();
        daemon.send_signal("TERM").expect("signal");

        let status = daemon.wait_exit(STEP).await.expect("it stops by itself");
        let stopped_in = asked.elapsed();
        eprintln!("a_signal_during_the_epoch_insert_ends_the_start_with_nothing_sent: exited {stopped_in:?} after the signal, {status}");
        assert!(status.success(), "{status}\n{}", daemon.log_tail());
        assert!(stopped_in < Duration::from_secs(5), "{stopped_in:?}\n{}", daemon.log_tail());
        assert!(
            plain(daemon.log()).contains("Stop requested before the start completed"),
            "{}",
            daemon.log_tail()
        );
        assert_eq!(harness.venue.requests(), 0, "nothing sent");

        // End the insert the process left waiting; it never committed.
        sqlx::query(
            "SELECT pg_terminate_backend(pid) FROM pg_stat_activity
             WHERE datname = current_database() AND query LIKE 'INSERT INTO process_epochs%'",
        )
        .execute(&harness.pool)
        .await
        .unwrap();
        harness
            .until("the insert has ended", || async {
                sqlx::query_scalar::<_, i64>(inserting).fetch_one(&harness.pool).await.unwrap() == 0
            })
            .await;
        assert!(harness.epochs().await.is_empty(), "no epoch recorded");
    })
    .await;
}

/// The hard bound does not wait on logging. The daemon's stdout and stderr go
/// to one pipe that the test stops reading once the daemon is up and then
/// fills to the last byte, so every log write blocks, the stop's own included.
/// On SIGTERM the process still exits with code 70 at the hard bound, and the
/// stop, blocked at its first log line, writes no marker.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn the_hard_bound_does_not_wait_on_a_blocked_log(pool: PgPool) {
    use std::{
        io::{Read, Write},
        sync::{
            atomic::{AtomicBool, AtomicU64, Ordering},
            Arc,
        },
    };

    within(DAEMON_TEST_LIMIT, async move {
        let harness = Harness::start(pool).await;
        let (daemon, mut output, mut filler) =
            DaemonProcess::spawn_into_pipe(&harness.spec(None, &[])).expect("spawn robsond");
        // Read the daemon's output until it is up.
        let up = Arc::new(AtomicBool::new(false));
        let draining = {
            let up = Arc::clone(&up);
            std::thread::spawn(move || {
                let mut chunk = [0; 4096];
                while !up.load(Ordering::SeqCst) && output.read(&mut chunk).is_ok_and(|n| n > 0) {}
                output
            })
        };
        daemon.wait_ready(STEP).await.expect("daemon ready");
        up.store(true, Ordering::SeqCst);
        // Then fill the pipe one byte at a time: the filler stops only once
        // not one byte more fits, and from then on every log write blocks.
        let written = Arc::new(AtomicU64::new(0));
        let filling = {
            let written = Arc::clone(&written);
            std::thread::spawn(move || {
                while filler.write_all(b".").is_ok() {
                    written.fetch_add(1, Ordering::SeqCst);
                }
            })
        };
        let output = draining.join().expect("drain thread");
        let full = async {
            let mut last = written.load(Ordering::SeqCst);
            loop {
                tokio::time::sleep(Duration::from_millis(500)).await;
                let now = written.load(Ordering::SeqCst);
                if now == last && now > 0 {
                    break;
                }
                last = now;
            }
        };
        tokio::time::timeout(STEP, full).await.expect("the pipe fills");
        assert!(daemon.wait_exit(Duration::ZERO).await.is_err(), "still running");

        let asked = std::time::Instant::now();
        daemon.send_signal("TERM").expect("signal");
        let status = daemon.wait_exit(GRACE).await.expect("it exits by the hard bound");
        let stopped_in = asked.elapsed();
        eprintln!("the_hard_bound_does_not_wait_on_a_blocked_log: exited {stopped_in:?} after the signal, {status}");
        assert_eq!(status.code(), Some(HARD_EXIT_CODE), "{status}");
        assert!(
            stopped_in >= HARD_EXIT && stopped_in < HARD_EXIT + Duration::from_secs(2),
            "at the hard bound: {stopped_in:?}"
        );
        assert_eq!(harness.epochs().await, [(false, None)]);
        drop(output);
        filling.join().expect("filler thread");
    })
    .await;
}

/// A signal before the daemon runs. The database refuses every connection and
/// the start retries; SIGTERM during a backoff ends the start at once, exit 0.
/// The database comes back right after the signal, and the start does not go
/// on: no venue request, no epoch.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_signal_during_the_database_retries_ends_the_start(pool: PgPool) {
    within(DAEMON_TEST_LIMIT, async move {
        let harness = Harness::start(pool).await;
        harness.proxy.set_mode(ProxyMode::Sever);
        let daemon = harness.daemon(None);
        // The second attempt failed: the start sleeps 2 s before the third.
        let retrying = async {
            while !plain(daemon.log()).contains("retry_in_secs=2") {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        };
        tokio::time::timeout(STEP, retrying).await.expect("the start retries the database");
        let asked = std::time::Instant::now();
        daemon.send_signal("TERM").expect("signal");
        harness.proxy.set_mode(ProxyMode::Pass);

        let status = daemon.wait_exit(STEP).await.expect("it stops by itself");
        let stopped_in = asked.elapsed();
        eprintln!("a_signal_during_the_database_retries_ends_the_start: exited {stopped_in:?} after the signal, {status}");
        assert!(status.success(), "{status}\n{}", daemon.log_tail());
        assert!(stopped_in < Duration::from_secs(1), "{stopped_in:?}\n{}", daemon.log_tail());
        assert!(
            plain(daemon.log()).contains("Stop requested before the start completed"),
            "{}",
            daemon.log_tail()
        );
        assert_eq!(harness.venue.requests(), 0, "nothing sent");
        assert!(harness.epochs().await.is_empty(), "no epoch");
    })
    .await;
}

/// The leg's `last_seq` and its suspension's checkpoint and clearance.
async fn repair_progress(pool: &PgPool, position_id: Uuid) -> (i64, Option<i64>, bool) {
    let last_seq: i64 =
        sqlx::query_scalar("SELECT last_seq FROM positions_current WHERE position_id = $1")
            .bind(position_id)
            .fetch_one(pool)
            .await
            .expect("read the leg");
    let (through, cleared): (Option<i64>, bool) = sqlx::query_as(
        "SELECT repaired_through_seq, cleared_at IS NOT NULL FROM stream_suspensions \
         WHERE stream_key = $1",
    )
    .bind(format!("position:{position_id}"))
    .fetch_one(pool)
    .await
    .expect("read the suspension");
    (last_seq, through, cleared)
}

/// ADR-0056 step 7, PR P: the real process dies while repairing a suspended
/// position stream, its first page applied and checkpointed. The next start
/// resumes past the checkpoint, applies the rest and clears the suspension.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn killed_mid_repair_the_next_start_resumes_past_its_checkpoint(pool: PgPool) {
    within(DAEMON_TEST_LIMIT, async move {
        let harness = Harness::start(pool).await;
        let position_id = Uuid::now_v7();
        let stream_key = format!("position:{position_id}");
        let now = chrono::Utc::now();
        let append = |event: robson_domain::Event| {
            let pool = harness.pool.clone();
            let stream_key = stream_key.clone();
            let tenant_id = harness.tenant_id;
            async move {
                let payload = serde_json::to_value(&event).unwrap();
                let event = robson_eventlog::Event::new(
                    tenant_id,
                    &stream_key,
                    event.event_type(),
                    payload,
                );
                robson_eventlog::append_event(&pool, &stream_key, None, event).await.unwrap();
            }
        };
        // A disarmed leg, applied: nothing at startup acts on it.
        append(robson_domain::Event::PositionArmed {
            position_id,
            account_id: Uuid::now_v7(),
            symbol: robson_domain::Symbol::from_pair(FAKE_SYMBOL).unwrap(),
            side: robson_domain::Side::Long,
            tech_stop_distance: None,
            stop_policy: robson_domain::StopPolicy::ExecutableSpan,
            stop_buffer_bps_at_arm: None,
            timestamp: now,
        })
        .await;
        append(robson_domain::Event::PositionDisarmed {
            position_id,
            reason: "harness".to_string(),
            timestamp: now,
        })
        .await;
        let applied = robson_eventlog::query_stream_in_seq_order(
            &harness.pool,
            harness.tenant_id,
            &stream_key,
            &["position_armed", "position_disarmed"],
        )
        .await
        .unwrap();
        for envelope in &applied {
            robson_projector::apply_event_to_projections(&harness.pool, envelope)
                .await
                .unwrap();
        }
        // Two pages that landed and never applied, under an open suspension.
        for stop in 1000..1600 {
            let price = |value| robson_domain::Price::new(Decimal::from(value)).unwrap();
            append(robson_domain::Event::TrailingStopUpdated {
                position_id,
                previous_stop: price(stop),
                new_stop: price(stop + 1),
                trigger_price: price(stop + 1),
                timestamp: now,
            })
            .await;
        }
        sqlx::query(
            "INSERT INTO stream_suspensions (suspension_id, tenant_id, stream_key, stream_kind, \
             failed_seq, error, suspended_at) VALUES ($1, $2, $3, 'position', 3, 'harness', NOW())",
        )
        .bind(Uuid::now_v7())
        .bind(harness.tenant_id)
        .bind(&stream_key)
        .execute(&harness.pool)
        .await
        .unwrap();

        let daemon = harness.daemon(Some(REPAIR_AFTER_PAGE));
        daemon
            .wait_killed_at(REPAIR_AFTER_PAGE, STEP)
            .await
            .expect("killed after the first page");
        assert_eq!(repair_progress(&harness.pool, position_id).await, (502, Some(502), false));
        drop(daemon);

        let restarted = harness.daemon(None);
        restarted.wait_ready(STEP).await.expect("daemon ready");
        assert_eq!(
            repair_progress(&harness.pool, position_id).await,
            (602, Some(602), true),
            "{}",
            restarted.log_tail()
        );
        assert_eq!(harness.venue.counts(), VenueCounts::default());
    })
    .await;
}
