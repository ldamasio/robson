//! Exact local M41 bounded apply and native generation shape controls.
use std::time::Instant;

use anyhow::{ensure, Context, Result};
use robson_testkit::db_proxy::{DbProxy, ProxyMode};
use sqlx::{postgres::PgSslMode, PgPool};
use uuid::Uuid;

use super::{
    super::stages::{self, Edge, Stage},
    helpers::refuses_after_retirement,
    *,
};

const TABLE: &str = "public.spot_order_set_generation";
const SETTINGS: &str = "SELECT pg_backend_pid(),current_setting('lock_timeout'),
    current_setting('statement_timeout'),current_setting('idle_in_transaction_session_timeout')";
const DELEGATED_RETIREMENT: Status = Status {
    phase: Phase::Retired,
    failure: Some(Failure {
        at: Phase::DelegatedApply,
        outcome: Outcome::MayHaveCommitted,
    }),
};

#[path = "migration41_projection.rs"]
mod projection;

struct Fixture {
    adapter: BoundedMigration,
    migration: &'static Migration,
    proxy: DbProxy,
    observer: PgConnection,
    pid: i32,
    birth: String,
    wire: u64,
}

fn migration() -> &'static Migration {
    contract41().expect("exact embedded M41")
}

impl Fixture {
    async fn new(pool: &PgPool) -> Result<Self> {
        Self::for_migration(pool, migration()).await
    }

    async fn for_migration(pool: &PgPool, migration: &'static Migration) -> Result<Self> {
        let predecessor = migration.version - 1;
        through(predecessor).run(pool).await?;
        let options = pool.connect_options();
        let upstream = tokio::net::lookup_host((options.get_host(), options.get_port()))
            .await?
            .next()
            .context("local native migration database")?;
        ensure!(upstream.ip().is_loopback(), "migration41 must use an isolated local fixture");
        let proxy = DbProxy::start(upstream).await?;
        let proxied = options
            .as_ref()
            .clone()
            .host("127.0.0.1")
            .port(proxy.addr().port())
            .ssl_mode(PgSslMode::Disable);
        let mut adapter = BoundedMigration::connect(&proxied).await?;
        let (pid, birth) = sqlx::query_as(
            "SELECT pid,backend_start::text FROM pg_stat_activity WHERE pid=pg_backend_pid()",
        )
        .fetch_one(adapter.connection()?)
        .await?;
        sqlx::query(
            "SELECT set_config('lock_timeout','310ms',false),
                            set_config('statement_timeout','410ms',false),
                            set_config('idle_in_transaction_session_timeout','510ms',false)",
        )
        .execute(adapter.connection()?)
        .await?;
        adapter.lock().await?;
        let observer = options.connect().await?;
        let wire = proxy.stats().statements;
        proxy.enable_exact_query_trace(16);
        let mut fixture = Self {
            adapter,
            migration,
            proxy,
            observer,
            pid,
            birth,
            wire,
        };
        ensure!(!fixture.table_exists().await? && fixture.metadata().await?.is_none());
        let present: Vec<i64> =
            sqlx::query_scalar("SELECT version FROM _sqlx_migrations ORDER BY version")
                .fetch_all(&mut fixture.observer)
                .await?;
        let embedded: Vec<_> = crate::MIGRATOR
            .iter()
            .filter(|m| m.version <= predecessor)
            .map(|m| m.version)
            .collect();
        ensure!(present == embedded, "actual immutable predecessor inventory");
        Ok(fixture)
    }

    fn first_create(&self) -> &'static str {
        match self.migration.version {
            VERSION41 => "CREATE TABLE public.spot_order_set_generation",
            VERSION42 => "CREATE TABLE public.spot_order_set_sell_legs",
            VERSION43 => "CREATE TABLE public.spot_order_set_cancel_members",
            VERSION44 => "CREATE TABLE public.spot_order_set_cancel_unbound",
            unexpected => panic!("unexpected fixture migration {unexpected}"),
        }
    }

    async fn table_exists(&mut self) -> Result<bool> {
        let table = self.first_create().trim_start_matches("CREATE TABLE ");
        Ok(sqlx::query_scalar("SELECT to_regclass($1) IS NOT NULL")
            .bind(table)
            .fetch_one(&mut self.observer)
            .await?)
    }

    async fn metadata(&mut self) -> Result<Option<(String, bool, Vec<u8>, i64)>> {
        Ok(sqlx::query_as(
            "SELECT description,success,checksum,execution_time
             FROM _sqlx_migrations WHERE version=$1",
        )
        .bind(self.migration.version)
        .fetch_optional(&mut self.observer)
        .await?)
    }

    fn assert_wire(&self, count: usize) {
        let actual = self.proxy.query_trace();
        assert_eq!(actual.len(), count, "physical M41 wire at prefix {count}: {actual:?}");
        assert_eq!(self.proxy.stats().statements - self.wire, count as u64);
        assert_eq!(self.proxy.query_trace_dropped(), 0);
        let expected = [
            "WITH old AS MATERIALIZED",
            "BEGIN",
            self.migration.sql.trim(),
            public_apply::metadata_insert_sql_for_tests().trim(),
            "COMMIT",
            "UPDATE _sqlx_migrations",
            "SELECT set_config('lock_timeout',$1,false)",
        ];
        for (i, sql) in actual.iter().enumerate() {
            if matches!(i, 0 | 5 | 6) {
                assert!(sql.trim().starts_with(expected[i]), "M41 wire slot {i}: {sql:?}");
            } else {
                assert_eq!(sql.trim(), expected[i], "M41 wire slot {i}");
            }
        }
    }

    async fn retired(&mut self) -> Result<()> {
        timeout(Duration::from_millis(350), async {
            loop {
                let live: bool = sqlx::query_scalar(
                    "SELECT EXISTS(SELECT 1 FROM pg_stat_activity
                     WHERE pid=$1 AND backend_start::text=$2 AND datname=current_database())",
                )
                .bind(self.pid)
                .bind(&self.birth)
                .fetch_one(&mut self.observer)
                .await?;
                if !live {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(3)).await;
            }
            Ok::<(), sqlx::Error>(())
        })
        .await
        .context("original M41 connection still present")??;
        Ok(())
    }

    async fn assert_landed(&mut self, timely: bool) -> Result<()> {
        ensure!(self.table_exists().await?);
        let (description, success, checksum, elapsed) =
            self.metadata().await?.context("same exact M41 metadata missing")?;
        ensure!(
            description == self.migration.description
                && success
                && checksum.as_slice() == self.migration.checksum.as_ref()
        );
        if timely {
            ensure!(elapsed >= 0, "attested elapsed metadata");
        } else {
            ensure!(elapsed == -1, "lost COMMIT ACK must precede elapsed update");
        }
        let leaves: &[&str] = match self.migration.version {
            VERSION42 => &[
                "sell_legs",
                "sell_symbols",
                "cancel_members",
                "cancel_groups",
            ],
            VERSION43 => &["cancel_members", "cancel_groups", "cancel_legs"],
            VERSION44 => &["cancel_unbound", "cancel_ungroupable"],
            _ => &[],
        };
        if !leaves.is_empty() {
            for suffix in leaves {
                // The suffix derives from the fixture's fixed per-migration inventory.
                let sql = format!("SELECT count(*) FROM public.spot_order_set_{suffix}");
                let rows: i64 = sqlx::query_scalar(&sql).fetch_one(&mut self.observer).await?;
                ensure!(rows == 0, "new table {suffix} must remain empty");
            }
            return Ok(());
        }
        let columns: Vec<(String, String, bool)> = sqlx::query_as(
            "SELECT attname::text,format_type(atttypid,atttypmod),attnotnull
             FROM pg_attribute WHERE attrelid=$1::regclass AND attnum>0 AND NOT attisdropped
             ORDER BY attnum",
        )
        .bind(TABLE)
        .fetch_all(&mut self.observer)
        .await?;
        let expected = [
            ("tenant_id", "uuid", true),
            ("venue_id", "text", true),
            ("account_scope", "text", true),
            ("generation_id", "uuid", true),
            ("process_epoch", "uuid", false),
            ("source_audit_id", "uuid", false),
            ("accounting_version", "bigint", false),
        ]
        .map(|(n, t, r)| (n.to_owned(), t.to_owned(), r));
        ensure!(columns == expected);
        let constraints: (i64, i64, i64) = sqlx::query_as(
            "SELECT count(*) FILTER (WHERE contype='p'),
                    count(*) FILTER (WHERE contype='c' AND convalidated),
                    count(*) FILTER (WHERE contype='f')
             FROM pg_constraint WHERE conrelid=$1::regclass",
        )
        .bind(TABLE)
        .fetch_one(&mut self.observer)
        .await?;
        ensure!(constraints == (1, 8, 0), "scoped primary key and eight exact guard roles");
        let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM public.spot_order_set_generation")
            .fetch_one(&mut self.observer)
            .await?;
        ensure!(rows == 0, "migration must create no generation row");
        Ok(())
    }
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn migration41_healthy_is_exact_bounded_and_preserves_existing_locks(
    pool: PgPool,
) -> Result<()> {
    let mut f = Fixture::new(&pool).await?;
    let mut holder = pool.begin().await?;
    sqlx::raw_sql("LOCK TABLE public.accounting_state IN ROW EXCLUSIVE MODE")
        .execute(&mut *holder)
        .await?;
    let started = Instant::now();
    f.adapter.apply(migration()).await?;
    let elapsed = started.elapsed();
    assert_eq!(f.adapter.status(), Status { phase: Phase::Restored, failure: None });
    f.assert_wire(7);
    ensure!(elapsed < APPLY_BOUND);
    let settings: (i32, String, String, String) =
        sqlx::query_as(SETTINGS).fetch_one(f.adapter.connection()?).await?;
    ensure!(settings == (f.pid, "310ms".into(), "410ms".into(), "510ms".into()));
    f.assert_landed(true).await?;
    holder.rollback().await?;

    let tenant = Uuid::now_v7();
    let id = Uuid::now_v7();
    let wrong = sqlx::query(
        "INSERT INTO public.spot_order_set_generation
         (tenant_id,venue_id,account_scope,generation_id,process_epoch)
         VALUES($1,'binance:spot','fixture',$2,$3)",
    )
    .bind(tenant)
    .bind(id)
    .bind(Uuid::now_v7())
    .execute(&pool)
    .await
    .expect_err("incomplete nonnull tuple");
    ensure!(wrong.as_database_error().is_some_and(|db| {
        db.code().as_deref() == Some("23514")
            && db.constraint() == Some("spot_order_set_generation_shape")
    }));
    f.adapter.unlock().await?;
    eprintln!("M41 bounded native commit physical_sql=7 legacy_lock_held=true elapsed={elapsed:?}");
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn migration41_wrong_binding_refuses_without_sql(pool: PgPool) -> Result<()> {
    wrong_binding(Fixture::new(&pool).await?).await
}

async fn wrong_binding(mut f: Fixture) -> Result<()> {
    let migration = f.migration;
    for wrong in [
        Migration { no_tx: true, ..migration.clone() },
        Migration {
            checksum: Cow::Owned(vec![0; 48]),
            ..migration.clone()
        },
        Migration {
            sql: Cow::Borrowed("SELECT 1"),
            ..migration.clone()
        },
    ] {
        let denied = f.adapter.apply(&wrong).await.expect_err("unembedded M41 must refuse");
        let expected = format!("migration{} contract mismatch", migration.version % 100);
        ensure!(denied.to_string().contains(&expected));
        f.assert_wire(0);
    }
    ensure!(!f.table_exists().await? && f.metadata().await?.is_none());
    f.adapter.unlock().await?;
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn migration41_contended_catalog_rolls_back_with_native_timeout(pool: PgPool) -> Result<()> {
    contended_catalog(&pool, Fixture::new(&pool).await?).await
}

async fn contended_catalog(pool: &PgPool, mut f: Fixture) -> Result<()> {
    let migration = f.migration;
    let mut holder = pool.begin().await?;
    sqlx::raw_sql(&format!("{} (held integer)", f.first_create()))
        .execute(&mut *holder)
        .await?;
    ensure!(!f.table_exists().await?, "original cannot see the uncommitted holder");

    let started = Instant::now();
    let denied = f.adapter.apply(migration).await.expect_err("catalog conflict must time out");
    let elapsed = started.elapsed();
    let native = match &denied {
        MigrateError::Execute(error) => error.as_database_error(),
        MigrateError::ExecuteMigration(error, version) if *version == migration.version => {
            error.as_database_error()
        },
        _ => None,
    }
    .context("native PostgreSQL catalog lock rejection")?;
    ensure!(native.code().as_deref() == Some("55P03"));
    ensure!(elapsed < APPLY_BOUND);
    ensure!(f.adapter.status() == DELEGATED_RETIREMENT);
    f.assert_wire(3);
    ensure!(refuses_after_retirement(&mut f.adapter).await);
    f.retired().await?;
    f.assert_wire(3);
    ensure!(!f.table_exists().await? && f.metadata().await?.is_none());
    holder.rollback().await?;
    ensure!(!f.table_exists().await? && f.metadata().await?.is_none());
    eprintln!(
        "M{} bounded native contention sqlstate=55P03 physical_sql=3 elapsed={elapsed:?}",
        migration.version % 100
    );
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn migration41_transport_blackhole_fires_whole_apply_deadline(pool: PgPool) -> Result<()> {
    transport_blackhole(Fixture::new(&pool).await?).await
}

async fn transport_blackhole(mut f: Fixture) -> Result<()> {
    let migration = f.migration;
    let (hit, release) = stages::arm(&f.adapter.gate, Stage::Install, Edge::AfterReturn)?;
    let watchdog = Duration::from_secs(1);
    let started = Instant::now();
    let (result, armed) = {
        let producer = f.adapter.apply(migration);
        let controller = async {
            timeout(Duration::from_millis(250), hit).await??;
            ensure!(f.proxy.stats().statements - f.wire == 1);
            f.proxy.set_mode(ProxyMode::Blackhole);
            release.send(()).map_err(|_| anyhow::anyhow!("install gate closed"))?;
            Ok::<(), anyhow::Error>(())
        };
        tokio::join!(timeout(watchdog, producer), controller)
    };
    let elapsed = started.elapsed();
    armed?;
    let error = result
        .context("M41 blackhole did not return at its own 200 ms deadline")?
        .expect_err("M41 blackhole must refuse");
    let expected = format!("migration{} whole-apply deadline exceeded", migration.version % 100);
    ensure!(matches!(error, MigrateError::Source(source) if source.to_string() == expected));
    ensure!((APPLY_BOUND..watchdog).contains(&elapsed));
    ensure!(f.adapter.status() == DELEGATED_RETIREMENT);
    ensure!(refuses_after_retirement(&mut f.adapter).await);
    f.retired().await?;
    f.assert_wire(1);
    ensure!(!f.table_exists().await? && f.metadata().await?.is_none());
    eprintln!(
        "M{} bounded native blackhole physical_sql=1 elapsed={elapsed:?}",
        migration.version % 100
    );
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn migration41_caller_cancellation_after_commit_retires_original(pool: PgPool) -> Result<()> {
    caller_cancel(Fixture::new(&pool).await?).await
}

async fn caller_cancel(mut f: Fixture) -> Result<()> {
    let migration = f.migration;
    let (hit, release) = stages::arm(&f.adapter.gate, Stage::Restore, Edge::BeforeCall)?;
    let started = Instant::now();
    {
        let mut producer = f.adapter.apply(migration);
        tokio::select! {
            result = &mut producer => anyhow::bail!("M41 returned before the cancellation cut: {result:?}"),
            reached = hit => reached?,
        }
        drop(producer);
        drop(release);
    }
    let elapsed = started.elapsed();
    ensure!(elapsed < APPLY_BOUND);
    ensure!(
        f.adapter.status()
            == Status {
                phase: Phase::Retired,
                failure: Some(Failure {
                    at: Phase::RestoreStarted,
                    outcome: Outcome::AppliedButSessionUnrestored,
                }),
            }
    );
    ensure!(f.proxy.stats().commit_acks_dropped == 0);
    ensure!(refuses_after_retirement(&mut f.adapter).await);
    f.retired().await?;
    f.assert_wire(6);
    f.assert_landed(true).await?;
    eprintln!(
        "M{} bounded native caller cancellation physical_sql=6 elapsed={elapsed:?}",
        migration.version % 100
    );
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn migration41_original_holder_bounds_real_advisory_waiter(pool: PgPool) -> Result<()> {
    let mut f = Fixture::new(&pool).await?;
    f.adapter.apply(migration()).await?;
    f.assert_wire(7);
    let mut waiter = BoundedMigration::connect(pool.connect_options().as_ref()).await?;
    let (pid, birth): (i32, String) = sqlx::query_as(
        "SELECT pid,backend_start::text FROM pg_stat_activity WHERE pid=pg_backend_pid()",
    )
    .fetch_one(waiter.connection()?)
    .await?;
    let started = Instant::now();
    let watchdog = ACQUIRE_BOUND + Duration::from_secs(1);
    let denied = {
        let mut lock = waiter.lock();
        let witness = timeout(Duration::from_millis(300), async {
            loop {
                let waiting: bool = sqlx::query_scalar(
                    "SELECT EXISTS (SELECT 1 FROM pg_stat_activity a
                     JOIN pg_locks w ON w.pid=a.pid
                     JOIN pg_locks h ON (h.locktype,h.database,h.classid,h.objid,h.objsubid,h.mode)
                                       =(w.locktype,w.database,w.classid,w.objid,w.objsubid,w.mode)
                     WHERE a.pid=$1 AND a.backend_start::text=$2
                       AND w.locktype='advisory' AND NOT w.granted
                       AND h.granted AND h.pid=$3 AND $3=ANY(pg_blocking_pids(a.pid)))",
                )
                .bind(pid)
                .bind(&birth)
                .bind(f.pid)
                .fetch_one(&mut f.observer)
                .await?;
                if waiting {
                    return Ok::<(), sqlx::Error>(());
                }
                tokio::time::sleep(Duration::from_millis(3)).await;
            }
        });
        tokio::select! {
            result = &mut lock => anyhow::bail!("M41 waiter returned before same-key contention: {result:?}"),
            seen = witness => seen.context("exact PostgreSQL blocking lock")??,
        }
        timeout(watchdog, lock).await.context("M41 advisory waiter exceeded watchdog")?
    }
    .expect_err("M41 advisory waiter must expire while holder is live");
    let elapsed = started.elapsed();
    ensure!(
        matches!(denied, MigrateError::Source(source) if source.to_string() == "migration advisory-lock deadline exceeded")
    );
    ensure!((ACQUIRE_BOUND..watchdog).contains(&elapsed));
    ensure!(
        waiter.status()
            == Status {
                phase: Phase::Retired,
                failure: Some(Failure {
                    at: Phase::Preflight,
                    outcome: Outcome::ApplyNotStarted,
                }),
            }
    );
    ensure!(refuses_after_retirement(&mut waiter).await);
    f.assert_wire(7);
    f.assert_landed(true).await?;
    // A server running with client_connection_check_interval=0 can observe
    // the closed waiter only after it leaves its native lock wait.
    f.adapter.unlock().await?;
    timeout(Duration::from_millis(350), async {
        while sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE pid=$1 AND backend_start::text=$2)",
        )
        .bind(pid)
        .bind(&birth)
        .fetch_one(&mut f.observer)
        .await?
        {
            tokio::time::sleep(Duration::from_millis(3)).await;
        }
        Ok::<(), sqlx::Error>(())
    })
    .await
    .context("original advisory waiter backend still present")??;
    eprintln!("M41 bounded native advisory same_key_waiter_retired=true elapsed={elapsed:?}");
    Ok(())
}

async fn lost(mut f: Fixture, commit: bool) -> Result<()> {
    let migration = f.migration;
    f.proxy.set_mode(if commit {
        ProxyMode::DropCommitAck
    } else {
        ProxyMode::DropSimpleInTransactionAckOf(f.first_create())
    });
    let started = Instant::now();
    let denied = f.adapter.apply(migration).await;
    ensure!(denied.is_err() && started.elapsed() < APPLY_BOUND);
    ensure!(f.adapter.status() == DELEGATED_RETIREMENT);
    f.assert_wire(if commit { 5 } else { 3 });
    ensure!(f.proxy.stats().commit_acks_dropped == u64::from(commit));
    ensure!(f.proxy.simple_in_transaction_acks_dropped() == u64::from(!commit));
    ensure!(refuses_after_retirement(&mut f.adapter).await);
    f.retired().await?;
    f.assert_wire(if commit { 5 } else { 3 });
    if commit {
        f.assert_landed(false).await?;
    } else {
        ensure!(!f.table_exists().await? && f.metadata().await?.is_none());
    }
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn migration41_lost_commit_ack_leaves_original_native_state(pool: PgPool) -> Result<()> {
    lost(Fixture::new(&pool).await?, true).await
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn migration41_lost_ddl_ack_rolls_back_original_before_commit(pool: PgPool) -> Result<()> {
    lost(Fixture::new(&pool).await?, false).await
}

fn projection_migration() -> &'static Migration {
    contract42().expect("exact embedded M42")
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn migration42_wrong_binding_refuses_without_sql(pool: PgPool) -> Result<()> {
    wrong_binding(Fixture::for_migration(&pool, projection_migration()).await?).await
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn migration42_contended_catalog_rolls_back_with_native_timeout(pool: PgPool) -> Result<()> {
    contended_catalog(&pool, Fixture::for_migration(&pool, projection_migration()).await?).await
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn migration42_transport_blackhole_fires_whole_apply_deadline(pool: PgPool) -> Result<()> {
    transport_blackhole(Fixture::for_migration(&pool, projection_migration()).await?).await
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn migration42_caller_cancellation_after_commit_retires_original(pool: PgPool) -> Result<()> {
    caller_cancel(Fixture::for_migration(&pool, projection_migration()).await?).await
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn migration42_lost_commit_ack_leaves_original_native_state(pool: PgPool) -> Result<()> {
    lost(Fixture::for_migration(&pool, projection_migration()).await?, true).await
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn migration42_lost_ddl_ack_leaves_only_first_migration(pool: PgPool) -> Result<()> {
    lost(Fixture::for_migration(&pool, projection_migration()).await?, false).await?;
    let present: bool = sqlx::query_scalar(
        "SELECT to_regclass('public.spot_order_set_generation') IS NOT NULL
            AND EXISTS(SELECT 1 FROM _sqlx_migrations WHERE version=$1 AND success)",
    )
    .bind(VERSION41)
    .fetch_one(&pool)
    .await?;
    ensure!(present, "the committed M41 remains intact after M42 rolls back");
    Ok(())
}

fn cancel_migration() -> &'static Migration {
    contract43().expect("exact embedded M43")
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn migration43_wrong_binding_refuses_without_sql(pool: PgPool) -> Result<()> {
    wrong_binding(Fixture::for_migration(&pool, cancel_migration()).await?).await
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn migration43_contended_catalog_rolls_back_with_native_timeout(pool: PgPool) -> Result<()> {
    contended_catalog(&pool, Fixture::for_migration(&pool, cancel_migration()).await?).await
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn migration43_transport_blackhole_fires_whole_apply_deadline(pool: PgPool) -> Result<()> {
    transport_blackhole(Fixture::for_migration(&pool, cancel_migration()).await?).await
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn migration43_caller_cancellation_after_commit_retires_original(pool: PgPool) -> Result<()> {
    caller_cancel(Fixture::for_migration(&pool, cancel_migration()).await?).await
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn migration43_lost_commit_ack_leaves_original_native_state(pool: PgPool) -> Result<()> {
    lost(Fixture::for_migration(&pool, cancel_migration()).await?, true).await
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn migration43_lost_ddl_ack_leaves_both_preceding_migrations(pool: PgPool) -> Result<()> {
    lost(Fixture::for_migration(&pool, cancel_migration()).await?, false).await?;
    let present: bool = sqlx::query_scalar(
        "SELECT to_regclass('public.spot_order_set_generation') IS NOT NULL
            AND (SELECT count(*) FROM _sqlx_migrations WHERE version IN ($1,$2) AND success)=2",
    )
    .bind(VERSION41)
    .bind(VERSION42)
    .fetch_one(&pool)
    .await?;
    ensure!(present, "both preceding exact migrations remain after M43 rolls back");
    Ok(())
}

fn cancel_unbound_migration() -> &'static Migration {
    contract44().expect("exact embedded M44")
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn migration44_wrong_binding_refuses_without_sql(pool: PgPool) -> Result<()> {
    wrong_binding(Fixture::for_migration(&pool, cancel_unbound_migration()).await?).await
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn migration44_contended_catalog_rolls_back_with_native_timeout(pool: PgPool) -> Result<()> {
    contended_catalog(&pool, Fixture::for_migration(&pool, cancel_unbound_migration()).await?).await
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn migration44_transport_blackhole_fires_whole_apply_deadline(pool: PgPool) -> Result<()> {
    transport_blackhole(Fixture::for_migration(&pool, cancel_unbound_migration()).await?).await
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn migration44_caller_cancellation_after_commit_retires_original(pool: PgPool) -> Result<()> {
    caller_cancel(Fixture::for_migration(&pool, cancel_unbound_migration()).await?).await
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn migration44_lost_commit_ack_leaves_original_native_state(pool: PgPool) -> Result<()> {
    lost(Fixture::for_migration(&pool, cancel_unbound_migration()).await?, true).await
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn migration44_lost_ddl_ack_leaves_three_preceding_migrations(pool: PgPool) -> Result<()> {
    lost(Fixture::for_migration(&pool, cancel_unbound_migration()).await?, false).await?;
    let present: bool = sqlx::query_scalar(
        "SELECT to_regclass('public.spot_order_set_generation') IS NOT NULL
            AND (SELECT count(*) FROM _sqlx_migrations WHERE version IN ($1,$2,$3) AND success)=3",
    )
    .bind(VERSION41)
    .bind(VERSION42)
    .bind(VERSION43)
    .fetch_one(&pool)
    .await?;
    ensure!(present, "three preceding exact migrations remain after M44 rolls back");
    Ok(())
}
