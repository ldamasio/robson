//! Embedded migration39 through the delegated bounded path: SQLx's own apply
//! between install and restore on the same owned session. App gates are
//! application facts only; contention is a real lock holder on the same server.
use std::{
    borrow::Cow,
    time::{Duration, Instant},
};

use robson_testkit::db_proxy::{DbProxy, ProxyMode, ProxyStats};
use sqlx::{migrate::Migrator, postgres::PgSslMode, PgConnection, PgPool};

use super::{
    super::stages::{self, Edge, Stage},
    helpers::{alive, refuses_after_retirement},
    *,
};

const SETTINGS: &str = "SELECT pg_backend_pid(),current_setting('lock_timeout'),current_setting('statement_timeout'),current_setting('idle_in_transaction_session_timeout')";
// Column present; exactly nullable unconstrained NUMERIC without default;
// named checks present; named checks that are CHECK and still NOT VALID.
const SCHEMA: &str = "SELECT EXISTS(SELECT 1 FROM pg_attribute WHERE attrelid='public.venue_orders'::regclass AND attname='quote_floor' AND NOT attisdropped),EXISTS(SELECT 1 FROM pg_attribute WHERE attrelid='public.venue_orders'::regclass AND attname='quote_floor' AND NOT attisdropped AND atttypid='numeric'::regtype AND atttypmod=-1 AND NOT attnotnull AND NOT atthasdef),(SELECT count(*) FROM pg_constraint WHERE conrelid='public.venue_orders'::regclass AND conname IN ('venue_orders_quote_floor_fit','venue_orders_quote_floor_bound')),(SELECT count(*) FROM pg_constraint WHERE conrelid='public.venue_orders'::regclass AND conname IN ('venue_orders_quote_floor_fit','venue_orders_quote_floor_bound') AND contype='c' AND NOT convalidated)";
const METADATA: &str =
    "SELECT success,checksum,execution_time FROM _sqlx_migrations WHERE version=20240101000039";
const RESTORE_NEEDLE: &str = "SELECT set_config('lock_timeout',$1,false)";
// The identified adapter backend, idle outside any transaction, holding
// exactly one granted advisory lock (SQLx's bigint key: objsubid=1).
const HELD: &str = "SELECT l.classid::bigint,l.objid::bigint FROM pg_locks l JOIN pg_stat_activity a ON a.pid=l.pid WHERE l.pid=$1 AND a.backend_start::text=$2 AND a.state='idle' AND a.xact_start IS NULL AND l.locktype='advisory' AND l.granted AND l.objsubid=1";
const SAME_KEY: &str = "SELECT EXISTS(SELECT 1 FROM pg_locks WHERE pid=pg_backend_pid() AND locktype='advisory' AND granted AND objsubid=1 AND classid::bigint=$1 AND objid::bigint=$2)";
const HOLDS: &str = "SELECT EXISTS(SELECT 1 FROM pg_locks WHERE pid=$1 AND relation='public.venue_orders'::regclass AND mode='AccessShareLock' AND granted)";
// The given backend holds SQLx's advisory key (granted, bigint form).
const LOCKED: &str = "SELECT EXISTS(SELECT 1 FROM pg_locks WHERE pid=$1 AND locktype='advisory' AND granted AND objsubid=1 AND classid::bigint=$2 AND objid::bigint=$3)";
// The whole durable 39 row plus the history size: any rewrite or insert moves
// it.
const HISTORY: &str = "SELECT description,installed_on::text,success,checksum,execution_time,(SELECT count(*) FROM _sqlx_migrations) FROM _sqlx_migrations WHERE version=20240101000039";
// Outer test watchdog, past the production APPLY_BOUND by the apply-owning
// fixture margin (formerly 1 s over 200 ms): a build without that bound
// reaches a named assertion instead of hanging on the blackhole.
const WATCHDOG: Duration = APPLY_FIXTURE;
type Schema = (bool, bool, i64, i64);
type History = (String, String, bool, Vec<u8>, i64, i64);
const ABSENT: Schema = (false, false, 0, 0);
const APPLIED: Schema = (true, true, 2, 2);

fn migration39() -> &'static Migration {
    crate::MIGRATOR
        .iter()
        .find(|m| m.version == VERSION39)
        .expect("embedded original39")
}

/// The test-only through38 inventory: the binary before migration39 existed.
fn through38() -> Migrator {
    let mut through38 = sqlx::migrate!("../migrations");
    through38.migrations =
        Cow::Owned(through38.iter().filter(|m| m.version <= VERSION).cloned().collect());
    through38
}

/// Bounded 5 ms poll over the native retirement window, as in `finish`, for
/// one raw-dropped backend.
async fn retires(observer: &mut PgConnection, pid: i32) -> Result<bool, sqlx::Error> {
    tokio::time::timeout(NATIVE_RETIREMENT, async {
        for _ in 0..NATIVE_RETIREMENT_POLLS {
            if !alive(observer, pid).await? {
                return Ok(true);
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        Ok(false)
    })
    .await
    .unwrap_or(Ok(false))
}

struct Fixture39 {
    adapter: BoundedMigration,
    proxy: DbProxy,
    observer: PgConnection,
    pid: i32,
    key: (i64, i64),
    baseline: ProxyStats,
}

struct Finished39 {
    retired: bool,
    status: Status,
    stats: ProxyStats,
    schema: Schema,
    metadata: Option<(bool, Vec<u8>, i64)>,
}

async fn schema(c: &mut PgConnection) -> Result<Schema, sqlx::Error> {
    sqlx::query_as(SCHEMA).fetch_one(c).await
}

async fn metadata(c: &mut PgConnection) -> Result<Option<(bool, Vec<u8>, i64)>, sqlx::Error> {
    sqlx::query_as(METADATA).fetch_optional(c).await
}

impl Fixture39 {
    async fn new(pool: &PgPool) -> anyhow::Result<Self> {
        through38().run(pool).await?;
        let options = pool.connect_options();
        let upstream = tokio::net::lookup_host((options.get_host(), options.get_port()))
            .await?
            .next()
            .ok_or_else(|| std::io::Error::other("no supplied PG address"))?;
        let proxy = DbProxy::start(upstream).await?;
        let proxied = options
            .as_ref()
            .clone()
            .host("127.0.0.1")
            .port(proxy.addr().port())
            .ssl_mode(PgSslMode::Disable);
        let mut adapter = BoundedMigration::connect(&proxied).await?;
        let pid = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(adapter.connection()?)
            .await?;
        sqlx::query("SELECT set_config('lock_timeout','310ms',false),set_config('statement_timeout','410ms',false),set_config('idle_in_transaction_session_timeout','510ms',false)")
            .execute(adapter.connection()?).await?;
        let mut observer = options.connect().await?;
        let start: String =
            sqlx::query_scalar("SELECT backend_start::text FROM pg_stat_activity WHERE pid=$1")
                .bind(pid)
                .fetch_one(&mut observer)
                .await?;
        // The production SQLx advisory lock on this same adapter, as run_direct
        // takes it before 39, so finish's fresh lock serializes behind it.
        let before = proxy.stats();
        proxy.enable_query_trace(4);
        adapter.lock().await?;
        let lock_sql: Vec<_> = proxy.query_trace().iter().map(|s| s.trim().to_owned()).collect();
        anyhow::ensure!(
            proxy.stats().statements - before.statements == 2
                && lock_sql == ["SELECT current_database()", "SELECT pg_advisory_lock($1)"],
            "fixture39 advisory setup inventory: {lock_sql:?}"
        );
        let held: Vec<(i64, i64)> =
            sqlx::query_as(HELD).bind(pid).bind(&start).fetch_all(&mut observer).await?;
        anyhow::ensure!(
            held.len() == 1,
            "fixture39 adapter backend does not hold the advisory lock"
        );
        // Warm observer queries; 39 must be absent before the measured apply.
        let _: bool = alive(&mut observer, pid).await?;
        anyhow::ensure!(
            schema(&mut observer).await? == ABSENT && metadata(&mut observer).await?.is_none(),
            "fixture39 already applied before the measured apply"
        );
        let baseline = proxy.stats();
        proxy.enable_query_trace(32);
        Ok(Self {
            adapter,
            proxy,
            observer,
            pid,
            key: held[0],
            baseline,
        })
    }

    async fn finish(self) -> anyhow::Result<Finished39> {
        let Self {
            adapter, proxy, mut observer, pid, key, ..
        } = self;
        let cleanup_deadline = tokio::time::Instant::now() + ACQUIRE_BOUND;
        let status = adapter.status();
        drop(adapter); // Whole raw owner, never graceful-close or pool-return.
        let retirement = tokio::time::timeout(NATIVE_RETIREMENT, async {
            for _ in 0..NATIVE_RETIREMENT_POLLS {
                if !alive(&mut observer, pid).await? {
                    return Ok::<_, sqlx::Error>(true);
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            Ok(false)
        })
        .await;
        let stats = proxy.stats();
        proxy.set_mode(ProxyMode::Sever);
        drop(proxy);
        // Serialized behind any surviving migrator, as the runbook requires:
        // the same advisory key the adapter backend held during the apply.
        let snapshots = tokio::time::timeout_at(cleanup_deadline, async {
            observer.lock().await?;
            let same: bool = sqlx::query_scalar(SAME_KEY)
                .bind(key.0)
                .bind(key.1)
                .fetch_one(&mut observer)
                .await?;
            anyhow::ensure!(same, "fixture39 finish lock is not the adapter's advisory key");
            let schema = schema(&mut observer).await?;
            let metadata = metadata(&mut observer).await?;
            observer.unlock().await?;
            Ok::<_, anyhow::Error>((schema, metadata))
        })
        .await;
        drop(observer);
        eprintln!(
            "cleanup39 status={status:?} received_statements={} retirement={retirement:?} snapshots_complete={}",
            stats.statements,
            matches!(&snapshots, Ok(Ok(_)))
        );
        let retired = retirement??;
        let (schema, metadata) = snapshots??;
        Ok(Finished39 { retired, status, stats, schema, metadata })
    }
}

/// SQLx 0.8.6 maps only the migration's own SQL to `ExecuteMigration`; BEGIN,
/// metadata INSERT, COMMIT and the advisory lock surface as other variants.
fn migration_sql_code(error: &MigrateError) -> Option<(i64, String)> {
    match error {
        MigrateError::ExecuteMigration(e, version) => e
            .as_database_error()
            .and_then(|d| d.code().map(|code| (*version, code.into_owned()))),
        _ => None,
    }
}

#[sqlx::test(migrations = false)]
#[ignore = "requires DATABASE_URL"]
async fn migration39_bounded_apply_restores_same_session(pool: PgPool) {
    let mut f = Fixture39::new(&pool).await.unwrap();
    let started = Instant::now();
    let result = f.adapter.apply(migration39()).await;
    let duration = started.elapsed();
    let status = f.adapter.status();
    let stats = f.proxy.stats();
    let trace = f.proxy.query_trace();
    let after = match (&result, f.adapter.connection()) {
        (Ok(_), Ok(connection)) => Some(
            sqlx::query_as::<_, (i32, String, String, String)>(SETTINGS)
                .fetch_one(connection)
                .await,
        ),
        _ => None,
    };
    let pid = f.pid;
    let baseline = f.baseline;
    let finished = f.finish().await;
    eprintln!("primary apply={result:?} duration={duration:?} status={status:?}");
    result.expect("embedded39 must complete under the production APPLY_BOUND");
    let finished = finished.expect("migration39 cleanup observation");
    assert!(duration < APPLY_BOUND, "migration39_global_apply_bound");
    assert_eq!(status, Status { phase: Phase::Restored, failure: None });
    assert_eq!(stats.statements - baseline.statements, 7, "migration39_apply_inventory");
    assert_eq!(stats.commits - baseline.commits, 1);
    let q: Vec<_> = trace.iter().map(|s| s.trim().trim_end_matches(';').trim()).collect();
    assert_eq!(q.len(), 7, "migration39_received_trace");
    assert!(q[0].starts_with("WITH old AS MATERIALIZED"), "install before ALTER TABLE");
    assert_eq!(q[1], "BEGIN");
    assert_eq!(q[2], migration39().sql.trim().trim_end_matches(';').trim());
    assert_eq!(q[3], public_apply::metadata_insert_sql_for_tests().trim());
    assert_eq!(q[4], "COMMIT");
    assert!(q[5].starts_with("UPDATE _sqlx_migrations"));
    assert!(q[6].starts_with(RESTORE_NEEDLE), "restore after the elapsed UPDATE");
    assert_eq!(
        after.unwrap().unwrap(),
        (pid, "310ms".into(), "410ms".into(), "510ms".into()),
        "migration39_same_session_settings_restored"
    );
    assert!(finished.retired);
    assert_eq!(finished.schema, APPLIED, "migration39_schema_effect_missing");
    let (success, checksum, elapsed) = finished.metadata.unwrap();
    assert!(success);
    assert_eq!(checksum.as_slice(), migration39().checksum.as_ref());
    assert!(elapsed >= 0);
}

#[sqlx::test(migrations = false)]
#[ignore = "requires DATABASE_URL"]
async fn migration39_contention_fails_at_lock_timeout_and_retires(pool: PgPool) {
    let mut f = Fixture39::new(&pool).await.unwrap();
    // A real conflicting holder, proven granted before the apply starts: any
    // table lock blocks ACCESS EXCLUSIVE. The adapter's advisory lock is
    // already held, so the apply itself takes no advisory lock.
    let holder = async {
        let mut holder = pool.begin().await?;
        sqlx::query("LOCK TABLE venue_orders IN ACCESS SHARE MODE")
            .execute(&mut *holder)
            .await?;
        let pid: i32 =
            sqlx::query_scalar("SELECT pg_backend_pid()").fetch_one(&mut *holder).await?;
        let holds: bool = sqlx::query_scalar(HOLDS).bind(pid).fetch_one(&mut f.observer).await?;
        Ok::<_, sqlx::Error>((holder, holds))
    }
    .await;
    let holder = match holder {
        Ok((holder, true)) => holder,
        other => {
            let setup = other.map(|(_, holds)| holds);
            let cleanup = f.finish().await;
            panic!("contention holder setup failed: {setup:?}; cleanup_error={:?}", cleanup.err());
        },
    };
    let baseline = f.baseline;
    let started = Instant::now();
    let result = f.adapter.apply(migration39()).await;
    let duration = started.elapsed();
    let status = f.adapter.status();
    let stats = f.proxy.stats();
    let trace = f.proxy.query_trace();
    let refused = refuses_after_retirement(&mut f.adapter).await;
    let after_refusals = f.proxy.stats();
    let released = holder.commit().await;
    let finished = f.finish().await;
    eprintln!("primary apply={result:?} duration={duration:?} status={status:?}");
    let error = result.expect_err("contended39_reported_success");
    assert_eq!(
        migration_sql_code(&error),
        Some((VERSION39, "55P03".to_owned())),
        "lock_timeout_of_the_migration39_statement_not_the_cause"
    );
    assert!(duration < APPLY_BOUND, "contended39_global_apply_bound");
    let q: Vec<_> = trace.iter().map(|s| s.trim().trim_end_matches(';').trim()).collect();
    assert_eq!(q.len(), 3, "contended39_received_trace");
    assert!(q[0].starts_with("WITH old AS MATERIALIZED"), "install before ALTER TABLE");
    assert_eq!(q[1], "BEGIN");
    assert_eq!(
        q[2],
        migration39().sql.trim().trim_end_matches(';').trim(),
        "last sent is ALTER"
    );
    assert_eq!(
        status,
        Status {
            phase: Phase::Retired,
            failure: Some(Failure {
                at: Phase::DelegatedApply,
                outcome: Outcome::MayHaveCommitted,
            }),
        },
        "contended39_wrong_conservative_outcome"
    );
    assert_eq!(stats.statements - baseline.statements, 3, "install/BEGIN/ALTER prefix only");
    assert_eq!(stats.commits - baseline.commits, 0);
    assert!(refused, "contended39_retired_adapter_reused");
    assert_eq!(after_refusals, stats, "retired_entry_sent_sql");
    released.expect("conflicting holder must be unaffected");
    let finished = finished.expect("contention cleanup observation");
    assert!(finished.retired, "contended39_backend_not_retired");
    assert_eq!(finished.status, status);
    assert_eq!(finished.stats, stats, "retirement_flushed_rollback_or_cleanup_sql");
    assert_eq!(finished.schema, ABSENT, "contended39_partial_schema");
    assert_eq!(finished.metadata, None, "contended39_history_written");
}

#[sqlx::test(migrations = false)]
#[ignore = "requires DATABASE_URL"]
async fn migration39_lost_commit_ack_is_uncertain_and_durable(pool: PgPool) {
    let mut f = Fixture39::new(&pool).await.unwrap();
    let baseline = f.baseline;
    f.proxy.set_mode(ProxyMode::DropCommitAck);
    let started = Instant::now();
    let result = f.adapter.apply(migration39()).await;
    let duration = started.elapsed();
    let status = f.adapter.status();
    let stats = f.proxy.stats();
    let refused = refuses_after_retirement(&mut f.adapter).await;
    let after_refusals = f.proxy.stats();
    let key = f.key;
    let finished = f.finish().await;
    eprintln!("primary apply={result:?} duration={duration:?} status={status:?}");
    assert!(result.is_err(), "lost_commit_ack39_reported_success");
    assert_eq!(
        status.failure,
        Some(Failure {
            at: Phase::DelegatedApply,
            outcome: Outcome::MayHaveCommitted,
        })
    );
    assert_eq!(status.phase, Phase::Retired);
    assert_eq!(
        stats.statements - baseline.statements,
        5,
        "lost_commit39_sent_elapsed_or_restore"
    );
    assert_eq!(stats.commit_acks_dropped - baseline.commit_acks_dropped, 1);
    assert!(refused);
    assert_eq!(after_refusals, stats);
    let finished = finished.expect("lost-ACK39 cleanup observation");
    assert!(finished.retired, "lost_commit39_backend_not_retired");
    assert_eq!(finished.schema, APPLIED, "lost_commit39_commit_not_durable");
    let (success, checksum, elapsed) = finished.metadata.unwrap();
    assert!(success);
    assert_eq!(checksum.as_slice(), migration39().checksum.as_ref());
    assert_eq!(elapsed, -1, "lost_commit39_ran_postcommit_elapsed");

    // Binary rollback boundary over the proven durable 39: SQLx 0.8.6
    // run_direct takes its advisory lock, then returns VersionMissing before
    // its unlock, so only retiring that raw connection releases the key.
    let options = pool.connect_options();
    let mut watcher = options.connect().await.expect("rollback39 watcher");
    let before: History = sqlx::query_as(HISTORY)
        .fetch_one(&mut watcher)
        .await
        .expect("durable39 history");
    let mut old = options.connect().await.expect("rollback39 through38 connection");
    let old_pid: i32 =
        sqlx::query_scalar("SELECT pg_backend_pid()").fetch_one(&mut old).await.unwrap();
    let rollback = tokio::time::timeout(ACQUIRE_BOUND, through38().run_direct(&mut old)).await;
    let leaked: bool = sqlx::query_scalar(LOCKED)
        .bind(old_pid)
        .bind(key.0)
        .bind(key.1)
        .fetch_one(&mut watcher)
        .await
        .unwrap();
    drop(old); // Raw drop: the lock retires with its backend, never unlock SQL.
    let old_retired = retires(&mut watcher, old_pid).await;
    // The canonical migrator then acquires the same key, skips the durable 39
    // by matching checksum, and unlocks.
    let mut current = options.connect().await.expect("canonical39 connection");
    let current_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut current)
        .await
        .unwrap();
    let canonical =
        tokio::time::timeout(ACQUIRE_BOUND, crate::MIGRATOR.run_direct(&mut current)).await;
    let kept: bool = sqlx::query_scalar(LOCKED)
        .bind(current_pid)
        .bind(key.0)
        .bind(key.1)
        .fetch_one(&mut watcher)
        .await
        .unwrap();
    let after: History = sqlx::query_as(HISTORY)
        .fetch_one(&mut watcher)
        .await
        .expect("canonical39 history");
    drop(current);
    drop(watcher);
    eprintln!("rollback39={rollback:?} leaked={leaked} old_retired={old_retired:?} canonical39={canonical:?} kept={kept}");
    assert!(
        matches!(rollback, Ok(Err(MigrateError::VersionMissing(v))) if v == VERSION39),
        "rollback39_through38_did_not_refuse_missing39"
    );
    assert!(leaked, "rollback39_sqlx_refusal_boundary_changed");
    assert!(old_retired.unwrap(), "rollback39_refusing_backend_survived_with_lock");
    assert!(matches!(canonical, Ok(Ok(()))), "canonical39_did_not_skip_durable39");
    assert!(!kept, "canonical39_preserved_advisory_lock");
    assert_eq!(after, before, "canonical39_rewrote_history");
}

#[sqlx::test(migrations = false)]
#[ignore = "requires DATABASE_URL"]
async fn migration39_blackhole_after_install_fires_global200_and_retires(pool: PgPool) {
    let mut f = Fixture39::new(&pool).await.unwrap();
    let baseline = f.baseline;
    let slot = f.adapter.gate.clone();
    let (hit, release) = match stages::arm(&slot, Stage::Install, Edge::AfterReturn) {
        Ok(gate) => gate,
        Err(error) => {
            let cleanup = f.finish().await;
            panic!("blackhole39 gate setup failed: {error}; cleanup_error={:?}", cleanup.err());
        },
    };
    // Settings are installed and confirmed; SQLx's BEGIN has not been sent.
    let (watched, gate) = {
        let adapter = &mut f.adapter;
        let proxy = &f.proxy;
        let producer = async {
            let started = Instant::now();
            let result = adapter.apply(migration39()).await;
            (result, started.elapsed())
        };
        let controller = async {
            hit.await.map_err(|_| refusal("blackhole39 install return not reached"))?;
            let seen = (proxy.mode(), proxy.stats().statements - baseline.statements);
            proxy.set_mode(ProxyMode::Blackhole);
            release.send(()).map_err(|_| refusal("blackhole39 stage release lost"))?;
            Ok::<_, MigrateError>(seen)
        };
        tokio::join!(
            tokio::time::timeout(WATCHDOG, producer),
            tokio::time::timeout(CONTROLLER_CEILING, controller)
        )
    };
    let status = f.adapter.status();
    let stats = f.proxy.stats();
    let trace = f.proxy.query_trace();
    let mode = f.proxy.mode();
    let disarmed = slot.lock().is_ok_and(|gate| gate.is_none());
    let refused = refuses_after_retirement(&mut f.adapter).await;
    let after_refusals = f.proxy.stats();
    // Physical close: the raw-dropped client socket ends its relay scope.
    let closed = tokio::time::timeout(NATIVE_RETIREMENT, async {
        while f.proxy.closed_relays() < stats.connections {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .is_ok();
    let closed_relays = f.proxy.closed_relays();
    let finished = f.finish().await;
    eprintln!(
        "primary watched={watched:?} status={status:?} gate={gate:?} closed_relays={closed_relays}"
    );
    let (result, duration) = match watched {
        Ok(returned) => returned,
        Err(_) => panic!("migration39_blackhole_global_apply_bound_missing"),
    };
    let error = result.expect_err("blackhole39_reported_success");
    assert!(
        matches!(&error, MigrateError::Source(e) if e.to_string() == "migration39 whole-apply deadline exceeded"),
        "blackhole39_not_the_global_apply_bound_branch"
    );
    assert!(
        duration >= APPLY_BOUND && duration < WATCHDOG,
        "blackhole39_global_apply_bound_timing"
    );
    let seen = gate.expect("blackhole39 observer deadline").expect("actual install return");
    assert_eq!(seen, (ProxyMode::Pass, 1), "blackhole39_injected_at_wrong_call");
    assert_eq!(mode, ProxyMode::Blackhole, "blackhole39_mode_not_in_effect");
    assert_eq!(
        status,
        Status {
            phase: Phase::Retired,
            failure: Some(Failure {
                at: Phase::DelegatedApply,
                outcome: Outcome::MayHaveCommitted,
            }),
        },
        "blackhole39_wrong_conservative_outcome"
    );
    assert!(disarmed, "blackhole39_app_gate_survived");
    assert_eq!(stats.statements - baseline.statements, 1, "install only reached the server");
    assert_eq!(stats.commits - baseline.commits, 0, "blackhole39_commit_sent");
    assert_eq!(trace.len(), 1, "blackhole39_received_trace");
    assert!(trace[0].trim().starts_with("WITH old AS MATERIALIZED"));
    assert!(refused, "blackhole39_retired_adapter_reused");
    assert_eq!(after_refusals, stats);
    assert!(
        closed && stats.connections == baseline.connections && closed_relays == stats.connections,
        "blackhole39_relay_not_physically_closed"
    );
    let finished = finished.expect("blackhole39 cleanup observation");
    assert!(finished.retired, "blackhole39_backend_not_retired");
    assert_eq!(finished.status, status);
    assert_eq!(finished.stats, stats, "blackhole39_retirement_flushed_sql");
    assert_eq!(finished.schema, ABSENT, "blackhole39_partial_schema");
    assert_eq!(finished.metadata, None, "blackhole39_history_written");
}

#[sqlx::test(migrations = false)]
#[ignore = "requires DATABASE_URL"]
async fn migration39_restore_ack_loss_is_applied_but_unrestored(pool: PgPool) {
    let mut f = Fixture39::new(&pool).await.unwrap();
    let baseline = f.baseline;
    let (hit, release) = match stages::arm(&f.adapter.gate, Stage::Restore, Edge::BeforeCall) {
        Ok(gate) => gate,
        Err(error) => {
            let cleanup = f.finish().await;
            panic!("restore gate setup failed: {error}; cleanup_error={:?}", cleanup.err());
        },
    };
    let ((result, duration), gate) = {
        let adapter = &mut f.adapter;
        let proxy = &f.proxy;
        let producer = async {
            let started = Instant::now();
            let result = adapter.apply(migration39()).await;
            (result, started.elapsed())
        };
        let controller = async {
            hit.await.map_err(|_| refusal("restore39 stage not reached"))?;
            let before = proxy.stats().statements - baseline.statements;
            proxy.set_mode(ProxyMode::DropAutocommitAckOf(RESTORE_NEEDLE));
            release.send(()).map_err(|_| refusal("restore39 stage release lost"))?;
            Ok::<_, MigrateError>(before)
        };
        tokio::join!(producer, tokio::time::timeout(CONTROLLER_CEILING, controller))
    };
    let status = f.adapter.status();
    let stats = f.proxy.stats();
    let drops = f.proxy.autocommit_acks_dropped();
    let refused = refuses_after_retirement(&mut f.adapter).await;
    let after_refusals = f.proxy.stats();
    let finished = f.finish().await;
    eprintln!("primary apply={result:?} duration={duration:?} status={status:?} gate={gate:?}");
    assert!(result.is_err(), "restore_ack39_reported_success");
    let before = gate.expect("restore39 observer deadline").expect("actual restore39 stage");
    assert_eq!(before, 6, "restore39_mode_armed_at_wrong_call");
    assert_eq!(drops, 1, "restore39_ack_loss_did_not_fire");
    assert_eq!(stats.statements - baseline.statements, 7);
    assert_eq!(stats.commits - baseline.commits, 1);
    assert_eq!(status, Status {
        phase: Phase::Retired,
        failure: Some(Failure {
            at: Phase::RestoreStarted,
            outcome: Outcome::AppliedButSessionUnrestored,
        }),
    });
    assert!(refused, "restore39_retired_adapter_reused");
    assert_eq!(after_refusals, stats);
    let finished = finished.expect("restore-ACK39 cleanup observation");
    assert!(finished.retired, "restore39_backend_not_retired");
    assert_eq!(finished.stats, stats, "restore39_retirement_flushed_sql");
    assert_eq!(finished.schema, APPLIED);
    let (success, checksum, elapsed) = finished.metadata.unwrap();
    assert!(success);
    assert_eq!(checksum.as_slice(), migration39().checksum.as_ref());
    assert!(elapsed >= 0, "restore39_durable_elapsed_missing");
}
