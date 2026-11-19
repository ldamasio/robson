//! Native M40 controls. A local test database is created for each case.
use std::{borrow::Cow, time::Instant};

use anyhow::Result;
use robson_testkit::db_proxy::{DbProxy, ProxyMode};
use serde_json::{json, Value};
use sqlx::{migrate::Migrator, postgres::PgSslMode, PgPool};

use super::{
    super::stages::{self, Edge, Stage},
    helpers::refuses_after_retirement,
    *,
};

const DO: &str = "DO $migration40$";
const ELAPSED: &str = "UPDATE _sqlx_migrations";
const RESTORE: &str = "SELECT set_config('lock_timeout',$1,false)";
const SETTINGS: &str = "SELECT pg_backend_pid(),current_setting('lock_timeout'),current_setting('statement_timeout'),current_setting('idle_in_transaction_session_timeout')";
const EXACT_BACKEND: &str = "SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE pid=$1 AND backend_start::text=$2 AND datname=current_database())";
#[path = "migration40_constraints.rs"]
mod constraints;
const CATALOG: &str = r#"
SELECT jsonb_build_object(
    'table', to_regclass('public.balance_transitions')::text,
    'columns', (SELECT jsonb_agg(jsonb_build_array(attname,format_type(atttypid,atttypmod),attnotnull,atthasdef) ORDER BY attnum)
        FROM pg_attribute WHERE attrelid=to_regclass('public.balance_transitions') AND attnum>0 AND NOT attisdropped),
    'constraints', (SELECT jsonb_object_agg(conname,pg_get_constraintdef(oid))
        FROM pg_constraint WHERE conrelid=to_regclass('public.balance_transitions')),
    'open_index', (SELECT jsonb_build_array(indisunique,indisvalid,indnkeyatts,
                  pg_get_indexdef(indexrelid,1,false),pg_get_expr(indpred,indrelid))
        FROM pg_index WHERE indexrelid=to_regclass('public.idx_balance_transitions_open')),
    'guard', (SELECT jsonb_build_array(pg_get_function_result(p.oid),pg_get_function_identity_arguments(p.oid),
                  l.lanname,md5(prosrc),proconfig)
        FROM pg_proc p JOIN pg_language l ON l.oid=p.prolang
        WHERE p.oid=to_regprocedure('public.balance_transitions_move_forward()')),
    'trigger', (SELECT jsonb_build_array(tgname,tgfoid::regprocedure::text,tgtype,tgenabled::text)
        FROM pg_trigger WHERE tgrelid=to_regclass('public.balance_transitions') AND NOT tgisinternal)
)
"#;

const HISTORY: &str = "SELECT version,description,installed_on::text,success,checksum,execution_time FROM _sqlx_migrations ORDER BY version";
type MigrationRow = (i64, String, String, bool, Vec<u8>, i64);
type AdvisoryKey = (i64, i64);

struct Fixture {
    adapter: BoundedMigration,
    observer: PgConnection,
    proxy: DbProxy,
    pid: i32,
    birth: String,
    key: AdvisoryKey,
    catalog: Value,
    history: Vec<MigrationRow>,
    baseline_statements: u64,
}

fn migration() -> &'static Migration {
    contract40().unwrap()
}

async fn through39(pool: &PgPool) -> Result<()> {
    let mut inventory = sqlx::migrate!("../migrations");
    inventory.migrations =
        Cow::Owned(inventory.iter().filter(|m| m.version < VERSION40).cloned().collect());
    inventory.run(pool).await?;
    Ok(())
}

async fn acquired_key(connection: &mut PgConnection, pid: i32, birth: &str) -> Result<AdvisoryKey> {
    let keys: Vec<AdvisoryKey> = sqlx::query_as(
        "SELECT l.classid::bigint,l.objid::bigint FROM pg_locks l JOIN pg_stat_activity a USING(pid) WHERE l.pid=$1 AND a.backend_start::text=$2 AND a.datname=current_database() AND a.state='idle' AND a.xact_start IS NULL AND l.locktype='advisory' AND l.granted AND l.objsubid=1",
    )
    .bind(pid)
    .bind(birth)
    .fetch_all(connection)
    .await?;
    anyhow::ensure!(keys.len() == 1, "expected one granted advisory lock: {keys:?}");
    Ok(keys[0])
}

impl Fixture {
    async fn new(pool: &PgPool) -> Result<Self> {
        let options = pool.connect_options();
        let upstream = tokio::net::lookup_host((options.get_host(), options.get_port()))
            .await?
            .next()
            .ok_or_else(|| anyhow::anyhow!("no local test address"))?;
        anyhow::ensure!(upstream.ip().is_loopback(), "M40 fixture requires local PostgreSQL");
        through39(pool).await?;
        let proxy = DbProxy::start(upstream).await?;
        let proxied = options
            .as_ref()
            .clone()
            .host("127.0.0.1")
            .port(proxy.addr().port())
            .ssl_mode(PgSslMode::Disable);
        let mut adapter = BoundedMigration::connect(&proxied).await?;
        let (pid, birth): (i32, String) = sqlx::query_as("SELECT pg_backend_pid(),backend_start::text FROM pg_stat_activity WHERE pid=pg_backend_pid()")
            .fetch_one(adapter.connection()?).await?;
        sqlx::query("SELECT set_config('lock_timeout','310ms',false),set_config('statement_timeout','410ms',false),set_config('idle_in_transaction_session_timeout','510ms',false)")
            .execute(adapter.connection()?).await?;
        adapter.lock().await?;
        let mut observer = options.connect().await?;
        let key = acquired_key(&mut observer, pid, &birth).await?;
        let catalog: Value = sqlx::query_scalar(CATALOG).fetch_one(&mut observer).await?;
        assert_eq!(
            catalog,
            json!({
                "table": null, "columns": null, "constraints": null,
                "open_index": null, "guard": null, "trigger": null
            })
        );
        let history: Vec<MigrationRow> = sqlx::query_as(HISTORY).fetch_all(&mut observer).await?;
        let versions: Vec<_> = history.iter().map(|row| row.0).collect();
        let expected: Vec<_> = crate::MIGRATOR
            .iter()
            .filter(|m| m.version < VERSION40)
            .map(|m| m.version)
            .collect();
        assert_eq!(versions, expected, "real previous migrations installed");
        for (row, installed) in history.iter().zip(crate::MIGRATOR.iter()) {
            assert_eq!(
                (row.1.as_str(), row.3, row.4.as_slice()),
                (installed.description.as_ref(), true, installed.checksum.as_ref())
            );
        }
        let baseline_statements = proxy.stats().statements;
        proxy.enable_exact_query_trace(16);
        Ok(Self {
            adapter,
            observer,
            proxy,
            pid,
            birth,
            key,
            catalog,
            history,
            baseline_statements,
        })
    }

    fn assert_wire_prefix(&self, count: usize) {
        let trace = self.proxy.query_trace();
        let sql: Vec<_> = trace.iter().map(|s| s.trim()).collect();
        assert_eq!(sql.len(), count, "M40 exact wire prefix length: {sql:?}");
        assert_eq!(self.proxy.stats().statements - self.baseline_statements, count as u64);
        let expected = [
            "WITH old AS MATERIALIZED",
            "BEGIN",
            migration().sql.trim(),
            public_apply::metadata_insert_sql_for_tests().trim(),
            "COMMIT",
            ELAPSED,
            RESTORE,
        ];
        for &index in &[0, 5, 6] {
            if index < count {
                assert!(
                    sql[index].starts_with(expected[index]),
                    "M40 wire slot {index}: {:?}",
                    sql[index]
                );
            }
        }
        for index in 1..count.min(5) {
            assert_eq!(sql[index], expected[index], "M40 wire slot {index}");
        }
    }

    async fn finish(self, landed: Option<bool>) -> Result<()> {
        let Self {
            adapter,
            mut observer,
            proxy,
            pid,
            birth,
            key,
            catalog,
            history,
            ..
        } = self;
        let before = proxy.stats().statements;
        drop(adapter);
        timeout(Duration::from_millis(350), async {
            loop {
                let alive: bool = sqlx::query_scalar(EXACT_BACKEND)
                    .bind(pid)
                    .bind(&birth)
                    .fetch_one(&mut observer)
                    .await?;
                if !alive {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            Ok::<_, sqlx::Error>(())
        })
        .await??;
        assert_eq!(proxy.stats().statements, before, "retirement flushed SQL");
        timeout(ACQUIRE_BOUND, observer.lock()).await??;
        let (observer_pid, observer_birth): (i32, String) = sqlx::query_as("SELECT pg_backend_pid(),backend_start::text FROM pg_stat_activity WHERE pid=pg_backend_pid()")
            .fetch_one(&mut observer).await?;
        assert_eq!(acquired_key(&mut observer, observer_pid, &observer_birth).await?, key);
        let actual_catalog: Value = sqlx::query_scalar(CATALOG).fetch_one(&mut observer).await?;
        let mut actual_history: Vec<MigrationRow> =
            sqlx::query_as(HISTORY).fetch_all(&mut observer).await?;
        if let Some(has_elapsed) = landed {
            assert_catalog(actual_catalog);
            let (version, description, _, success, checksum, elapsed) =
                actual_history.pop().unwrap();
            assert_eq!(
                (version, description.as_str(), success, checksum.as_slice()),
                (VERSION40, migration().description.as_ref(), true, migration().checksum.as_ref())
            );
            if has_elapsed {
                assert!(elapsed >= 0, "elapsed acknowledgement outcome: {elapsed}");
            } else {
                assert_eq!(elapsed, -1, "unwritten elapsed value");
            }
        } else {
            assert_eq!(actual_catalog, catalog, "schema escaped a failed M40 transaction");
        }
        assert_eq!(actual_history, history, "M40 changed prior migration history");
        observer.unlock().await?;
        Ok(())
    }
}

fn assert_catalog(actual: Value) {
    let expected_columns = json!([
        ["transition_id", "uuid", true, false],
        ["position_id", "uuid", true, false],
        ["tenant_id", "uuid", true, false],
        ["venue_id", "text", true, false],
        ["account_scope", "text", true, false],
        ["symbol", "text", true, false],
        ["kind", "text", true, false],
        ["target_trigger", "numeric", true, false],
        ["reason", "text", true, false],
        ["started_at", "timestamp with time zone", true, false],
        ["completed_at", "timestamp with time zone", false, false],
        ["outcome", "text", false, false],
        ["first_cancel_intent_id", "uuid", false, false],
        ["original_target_order_id", "text", false, false]
    ]);
    assert_eq!(actual["table"], "balance_transitions");
    assert_eq!(actual["columns"], expected_columns);
    assert_eq!(
        actual["constraints"],
        json!({
            "balance_transitions_pkey": "PRIMARY KEY (transition_id)",
            "balance_transitions_kind": "CHECK ((kind = ANY (ARRAY['replace'::text, 'reprotect'::text, 'flatten'::text, 'exit'::text])))",
            "balance_transitions_target": "CHECK ((ledger_amount_fits(target_trigger) AND (target_trigger > (0)::numeric)))",
            "balance_transitions_reason": "CHECK ((btrim(reason) <> ''::text))",
            "balance_transitions_completion_pair": "CHECK (((completed_at IS NULL) = (outcome IS NULL)))",
            "balance_transitions_first_cancel_pair": "CHECK (((first_cancel_intent_id IS NULL) = (original_target_order_id IS NULL)))",
            "balance_transitions_leg": "FOREIGN KEY (position_id, tenant_id, venue_id, account_scope, symbol) REFERENCES balance_legs(position_id, tenant_id, venue_id, account_scope, symbol)",
            "balance_transitions_first_cancel": "FOREIGN KEY (first_cancel_intent_id) REFERENCES order_intents(intent_id)",
            "balance_transitions_original_order": "FOREIGN KEY (venue_id, account_scope, symbol, original_target_order_id) REFERENCES venue_orders(venue_id, account_scope, symbol, exchange_order_id)"
        })
    );
    assert_eq!(
        actual["open_index"],
        json!([true, true, 1, "position_id", "(completed_at IS NULL)"])
    );
    assert_eq!(
        actual["guard"],
        json!(["trigger", "", "plpgsql", GUARD_SOURCE_MD5, [
            "search_path=pg_catalog, public"
        ]])
    );
    assert_eq!(
        actual["trigger"],
        json!([
            "balance_transitions_move_forward",
            "balance_transitions_move_forward()",
            31,
            "O"
        ])
    );
}

const GUARD_SOURCE_MD5: &str = "e3c024c9e73334c2d940da9a36e5d478";

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn migration40_healthy_restores_settings_and_keeps_exact_history(pool: PgPool) -> Result<()> {
    let mut f = Fixture::new(&pool).await?;
    let start = Instant::now();
    f.adapter.apply(migration()).await?;
    let duration = start.elapsed();
    assert_eq!(f.adapter.status(), Status { phase: Phase::Restored, failure: None });
    f.assert_wire_prefix(7);
    let settings: (i32, String, String, String) =
        sqlx::query_as(SETTINGS).fetch_one(f.adapter.connection()?).await?;
    assert_eq!(settings, (f.pid, "310ms".into(), "410ms".into(), "510ms".into()));
    eprintln!("M40 healthy apply duration={duration:?}, exact SQL count=7");
    assert!(duration < APPLY_BOUND);
    f.finish(Some(true)).await
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Fault {
    ParentLock,
    RejectCommit,
    DoAckLoss,
    CommitAckLoss,
    RejectElapsed,
    ElapsedAckLoss,
    RestoreSever,
    RestoreAckLoss,
}

async fn reject_metadata(observer: &mut PgConnection, commit: bool) -> Result<()> {
    sqlx::query(
        "CREATE FUNCTION reject_m40() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN
         IF NEW.version=20240101000040 THEN
           RAISE EXCEPTION 'native M40 metadata rejected' USING ERRCODE='23514';
         END IF; RETURN NEW; END $$",
    )
    .execute(&mut *observer)
    .await?;
    let trigger = if commit {
        "CREATE CONSTRAINT TRIGGER reject_m40 AFTER INSERT ON _sqlx_migrations DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION reject_m40()"
    } else {
        "CREATE TRIGGER reject_m40 BEFORE UPDATE ON _sqlx_migrations FOR EACH ROW EXECUTE FUNCTION reject_m40()"
    };
    sqlx::query(trigger).execute(observer).await?;
    Ok(())
}

async fn fault(pool: PgPool, case: Fault) -> Result<()> {
    let mut f = Fixture::new(&pool).await?;
    let mut holder = if case == Fault::ParentLock {
        let mut tx = pool.begin().await?;
        sqlx::query("LOCK TABLE balance_legs IN ACCESS EXCLUSIVE MODE")
            .execute(&mut *tx)
            .await?;
        let (pid, birth): (i32, String) = sqlx::query_as("SELECT pg_backend_pid(),backend_start::text FROM pg_stat_activity WHERE pid=pg_backend_pid()")
            .fetch_one(&mut *tx).await?;
        let holds: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_locks l JOIN pg_stat_activity a USING(pid)
            WHERE l.pid=$1 AND a.backend_start::text=$2 AND a.datname=current_database() AND l.granted
            AND l.relation='public.balance_legs'::regclass AND l.mode='AccessExclusiveLock')")
            .bind(pid).bind(&birth).fetch_one(&mut f.observer).await?;
        assert!(holds, "fixture must positively hold the exact referenced lock");
        Some(tx)
    } else {
        None
    };
    match case {
        Fault::RejectCommit => reject_metadata(&mut f.observer, true).await?,
        Fault::RejectElapsed => reject_metadata(&mut f.observer, false).await?,
        Fault::DoAckLoss => f.proxy.set_mode(ProxyMode::DropSimpleInTransactionAckOf(DO)),
        Fault::CommitAckLoss => f.proxy.set_mode(ProxyMode::DropCommitAckOf(DO)),
        Fault::ElapsedAckLoss => f.proxy.set_mode(ProxyMode::DropAutocommitAckOf(ELAPSED)),
        _ => {},
    }

    let restore = matches!(case, Fault::RestoreSever | Fault::RestoreAckLoss);
    let started = Instant::now();
    let result = if restore {
        let (hit, release) = stages::arm(&f.adapter.gate, Stage::Restore, Edge::BeforeCall)?;
        let proxy = &f.proxy;
        let baseline = f.baseline_statements;
        let apply = f.adapter.apply(migration());
        let alter_restore = async {
            hit.await?;
            assert_eq!(proxy.stats().statements - baseline, 6);
            if case == Fault::RestoreSever {
                let closed = proxy.closed_relays();
                proxy.set_mode(ProxyMode::Sever);
                timeout(Duration::from_millis(80), async {
                    while proxy.closed_relays() == closed {
                        tokio::task::yield_now().await;
                    }
                })
                .await?;
            } else {
                proxy.set_mode(ProxyMode::DropAutocommitAckOf(RESTORE));
            }
            release.send(()).map_err(|_| anyhow::anyhow!("restore gate expired"))?;
            Ok::<_, anyhow::Error>(())
        };
        let (result, fault) = tokio::join!(apply, alter_restore);
        fault?;
        result
    } else {
        f.adapter.apply(migration()).await
    };
    let duration = started.elapsed();
    if let Some(holder) = holder.take() {
        holder.rollback().await?;
    }
    eprintln!("M40 {case:?} duration={duration:?} result={result:?}");
    let error = result.expect_err("fixture fault did not interrupt the actual apply");
    assert!(
        duration < APPLY_BOUND,
        "{case:?} did not complete within the fixed whole-apply limit"
    );
    let status = f.adapter.status();
    let expected = if restore {
        Failure {
            at: Phase::RestoreStarted,
            outcome: Outcome::AppliedButSessionUnrestored,
        }
    } else {
        Failure {
            at: Phase::DelegatedApply,
            outcome: Outcome::MayHaveCommitted,
        }
    };
    assert_eq!(status, Status {
        phase: Phase::Retired,
        failure: Some(expected)
    });
    assert!(refuses_after_retirement(&mut f.adapter).await);

    let (prefix, landed, native_rejection) = match case {
        Fault::ParentLock => (3, None, Some("55P03")),
        Fault::RejectCommit => (5, None, Some("23514")),
        Fault::DoAckLoss => (3, None, None),
        Fault::CommitAckLoss => (5, Some(false), None),
        Fault::RejectElapsed => (6, Some(false), Some("23514")),
        Fault::ElapsedAckLoss | Fault::RestoreSever => (6, Some(true), None),
        Fault::RestoreAckLoss => (7, Some(true), None),
    };
    if let Some(expected) = native_rejection {
        let db = match &error {
            MigrateError::Execute(e) => e.as_database_error(),
            MigrateError::ExecuteMigration(e, VERSION40) => e.as_database_error(),
            _ => None,
        }
        .expect("native PostgreSQL error");
        assert_eq!(db.code().as_deref(), Some(expected));
        if case != Fault::ParentLock {
            assert_eq!(db.message(), "native M40 metadata rejected");
        }
    }
    assert_eq!(
        f.proxy.simple_in_transaction_acks_dropped(),
        u64::from(case == Fault::DoAckLoss)
    );
    assert_eq!(f.proxy.stats().commit_acks_dropped, u64::from(case == Fault::CommitAckLoss));
    assert_eq!(
        f.proxy.autocommit_acks_dropped(),
        u64::from(matches!(case, Fault::ElapsedAckLoss | Fault::RestoreAckLoss))
    );
    f.assert_wire_prefix(prefix);
    f.finish(landed).await
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn migration40_contended_parent_blocks_the_do(pool: PgPool) -> Result<()> {
    fault(pool, Fault::ParentLock).await
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn migration40_native_commit_rejection_rolls_back(pool: PgPool) -> Result<()> {
    fault(pool, Fault::RejectCommit).await
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn migration40_lost_do_ack_has_no_commit(pool: PgPool) -> Result<()> {
    fault(pool, Fault::DoAckLoss).await
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn migration40_lost_commit_ack_leaves_catalog_and_metadata(pool: PgPool) -> Result<()> {
    fault(pool, Fault::CommitAckLoss).await
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn migration40_native_elapsed_rejection_leaves_committed_catalog(pool: PgPool) -> Result<()> {
    fault(pool, Fault::RejectElapsed).await
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn migration40_lost_elapsed_ack_leaves_landed_elapsed(pool: PgPool) -> Result<()> {
    fault(pool, Fault::ElapsedAckLoss).await
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn migration40_sever_before_restore_never_sends_restore(pool: PgPool) -> Result<()> {
    fault(pool, Fault::RestoreSever).await
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn migration40_lost_restore_ack_retires_same_backend(pool: PgPool) -> Result<()> {
    fault(pool, Fault::RestoreAckLoss).await
}
