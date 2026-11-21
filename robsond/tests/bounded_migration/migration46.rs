//! Native M46-M50 controls, the S8-4 Spot accounting schema (ADR-0056 step
//! 8l-1): each exact embedded file through the delegated bounded path with
//! migration 40's fault inventory, the official runner, the rollback boundary
//! for older images and, in `migration46_constraints.rs`, the schema's rules.
//! The same per-version suite covers M51-M54, the S8-5 boundary snapshot
//! schema (step 8p-1), whose own controls are in `migration51.rs`, and
//! M55-M56, the S8-6 cutover schema (step 8m-1), whose own controls are in
//! `migration55.rs`. A local test database is created for each case.
use std::time::Instant;

use anyhow::{ensure, Context, Result};
use robson_testkit::db_proxy::{DbProxy, ProxyMode};
use sqlx::{postgres::PgSslMode, PgPool};

use super::{
    super::stages::{self, Edge, Stage},
    helpers::refuses_after_retirement,
    *,
};

#[path = "migration46_constraints.rs"]
mod constraints;

#[path = "migration51.rs"]
mod boundary;

#[path = "migration55.rs"]
mod cutover;

const VERSIONS: [i64; 5] = [VERSION46, VERSION47, VERSION48, VERSION49, VERSION50];
/// Every bounded Spot schema version: S8-4 (46-50), then S8-5 (51-54).
const SPOT_SCHEMA: [i64; 9] = [
    VERSION46, VERSION47, VERSION48, VERSION49, VERSION50, VERSION51, VERSION52, VERSION53,
    VERSION54,
];
/// S8-6 (55-56), after every S8-4 and S8-5 version.
const CUTOVER_SCHEMA: [i64; 2] = [VERSION55, VERSION56];
const ELAPSED: &str = "UPDATE _sqlx_migrations";
const RESTORE: &str = "SELECT set_config('lock_timeout',$1,false)";
const SETTINGS: &str = "SELECT pg_backend_pid(),current_setting('lock_timeout'),current_setting('statement_timeout'),current_setting('idle_in_transaction_session_timeout')";
const EXACT_BACKEND: &str = "SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE pid=$1 AND backend_start::text=$2 AND datname=current_database())";
const HISTORY: &str = "SELECT version,description,installed_on::text,success,checksum,execution_time FROM _sqlx_migrations ORDER BY version";
/// Every public relation, constraint, function and trigger (internal foreign
/// key triggers on the referenced tables included), as one fingerprint.
const CATALOG: &str = "SELECT md5(string_agg(x, '|' ORDER BY x)) FROM (
    SELECT 'c:'||relname||':'||relkind::text FROM pg_class WHERE relnamespace='public'::regnamespace
    UNION ALL SELECT 'k:'||conrelid::regclass::text||':'||conname||':'||pg_get_constraintdef(oid)
        FROM pg_constraint WHERE connamespace='public'::regnamespace
    UNION ALL SELECT 'f:'||oid::regprocedure::text FROM pg_proc WHERE pronamespace='public'::regnamespace
    UNION ALL SELECT 't:'||tgrelid::regclass::text||':'||tgname FROM pg_trigger
        WHERE tgrelid IN (SELECT oid FROM pg_class WHERE relnamespace='public'::regnamespace)) s(x)";
const PRESENT: &str = "SELECT count(*) FILTER (WHERE CASE k
        WHEN 'relation' THEN to_regclass(n) IS NOT NULL
        WHEN 'function' THEN to_regprocedure(n) IS NOT NULL
        WHEN 'constraint' THEN EXISTS(SELECT 1 FROM pg_constraint WHERE conname=n AND connamespace='public'::regnamespace)
        ELSE EXISTS(SELECT 1 FROM pg_trigger WHERE tgname=n AND NOT tgisinternal) END)
    FROM unnest($1::text[],$2::text[]) AS o(k,n)";
type MigrationRow = (i64, String, String, bool, Vec<u8>, i64);
type AdvisoryKey = (i64, i64);

fn contract(version: i64) -> &'static Migration {
    match version {
        VERSION46 => contract46(),
        VERSION47 => contract47(),
        VERSION48 => contract48(),
        VERSION49 => contract49(),
        VERSION50 => contract50(),
        VERSION51 => contract51(),
        VERSION52 => contract52(),
        VERSION53 => contract53(),
        VERSION54 => contract54(),
        VERSION55 => contract55(),
        VERSION56 => contract56(),
        _ => None,
    }
    .expect("exact embedded S8-4, S8-5 or S8-6 migration")
}

/// The DO statement's leading text, for the proxy's ACK-loss selectors.
fn do_head(version: i64) -> &'static str {
    match version {
        VERSION46 => "DO $migration46$",
        VERSION47 => "DO $migration47$",
        VERSION48 => "DO $migration48$",
        VERSION49 => "DO $migration49$",
        VERSION50 => "DO $migration50$",
        VERSION51 => "DO $migration51$",
        VERSION52 => "DO $migration52$",
        VERSION53 => "DO $migration53$",
        VERSION54 => "DO $migration54$",
        VERSION55 => "DO $migration55$",
        VERSION56 => "DO $migration56$",
        unexpected => panic!("unexpected Spot schema version {unexpected}"),
    }
}

/// Each version's objects: they land all together or not at all.
fn objects(version: i64) -> &'static [(&'static str, &'static str)] {
    match version {
        VERSION46 => &[
            ("function", "public.cash_anchor_moves_forward()"),
            ("relation", "public.cash_anchors"),
            ("trigger", "cash_anchors_move_forward"),
            ("relation", "public.cash_checkpoints"),
        ],
        VERSION47 => &[
            ("function", "public.cash_discrepancy_moves_forward()"),
            ("relation", "public.cash_discrepancies"),
            ("trigger", "cash_discrepancies_move_forward"),
            ("relation", "public.idx_cash_discrepancies_open"),
        ],
        VERSION48 => &[
            ("relation", "public.cash_sightings"),
            ("trigger", "cash_sightings_are_immutable"),
            ("relation", "public.cash_reanchors"),
            ("trigger", "cash_reanchors_are_immutable"),
        ],
        VERSION49 => &[
            ("relation", "public.cash_attested_flows"),
            ("trigger", "cash_attested_flows_are_immutable"),
            ("relation", "public.dust_ledger"),
            ("trigger", "dust_ledger_is_immutable"),
        ],
        VERSION50 => &[
            ("relation", "public.accounting_invalidations"),
            ("trigger", "accounting_invalidations_are_immutable"),
        ],
        VERSION51 => &[
            ("function", "public.boundary_snapshot_moves_forward()"),
            ("relation", "public.boundary_snapshots"),
            ("trigger", "boundary_snapshots_move_forward"),
            ("relation", "public.boundary_reopenings"),
        ],
        VERSION52 => &[
            ("trigger", "boundary_reopenings_are_immutable"),
            ("relation", "public.boundary_snapshot_revisions"),
            ("trigger", "boundary_snapshot_revisions_are_immutable"),
        ],
        VERSION53 => &[
            ("relation", "public.boundary_carried_legs"),
            ("trigger", "boundary_carried_legs_are_immutable"),
            ("function", "public.activation_basis_moves_forward()"),
        ],
        VERSION54 => &[
            ("relation", "public.activation_bases"),
            ("trigger", "activation_bases_move_forward"),
            ("relation", "public.idx_activation_bases_certified"),
        ],
        VERSION55 => &[
            ("function", "public.venue_cutover_moves_forward()"),
            ("relation", "public.venue_cutovers"),
            ("trigger", "venue_cutovers_move_forward"),
            ("relation", "public.idx_venue_cutovers_complete"),
        ],
        VERSION56 => &[
            ("relation", "public.idx_venue_cutovers_pending"),
            ("constraint", "venue_accounts_venue_cutover"),
            ("constraint", "cash_anchors_venue_cutover"),
            ("constraint", "activation_bases_venue_cutover"),
        ],
        unexpected => panic!("unexpected Spot schema version {unexpected}"),
    }
}

/// The referenced table a holder locks so that the version's DO must wait:
/// existing tables and S8-4's own earlier ones, each locked by the DO's first
/// table, so the lock wait starts before the DO spends its statement bound
/// (49's dust_ledger locks balance_legs only after a whole table; the lock
/// wait then ends at the 200 ms lock timeout, inside the 1 s statement bound).
/// S8-5's: 51's first table references accounting_state, 52's first command
/// is the trigger on boundary_reopenings, 53's first table references
/// balance_legs and 54's references cash_anchors. S8-6's: 55's first table
/// references accounting_state; 56's is venue_accounts, the one table it
/// alters that is already in production (its first command, an index on the
/// new empty venue_cutovers, takes milliseconds of the statement bound).
fn parent(version: i64) -> &'static str {
    match version {
        VERSION46 => "clock_samples",
        VERSION47 => "accounting_state",
        VERSION48 => "cash_discrepancies",
        VERSION49 => "cash_reanchors",
        VERSION50 => "accounting_state",
        VERSION51 => "accounting_state",
        VERSION52 => "boundary_reopenings",
        VERSION53 => "balance_legs",
        VERSION54 => "cash_anchors",
        VERSION55 => "accounting_state",
        VERSION56 => "venue_accounts",
        unexpected => panic!("unexpected Spot schema version {unexpected}"),
    }
}

async fn landed(connection: &mut PgConnection, version: i64) -> Result<bool> {
    let (kinds, names): (Vec<&str>, Vec<&str>) = objects(version).iter().copied().unzip();
    let present: i64 = sqlx::query_scalar(PRESENT)
        .bind(&kinds)
        .bind(&names)
        .fetch_one(connection)
        .await?;
    ensure!(
        present == 0 || present == kinds.len() as i64,
        "partial M{} schema: {present} of {}",
        version % 100,
        kinds.len()
    );
    Ok(present > 0)
}

async fn acquired_key(connection: &mut PgConnection, pid: i32, birth: &str) -> Result<AdvisoryKey> {
    let keys: Vec<AdvisoryKey> = sqlx::query_as(
        "SELECT l.classid::bigint,l.objid::bigint FROM pg_locks l JOIN pg_stat_activity a USING(pid) WHERE l.pid=$1 AND a.backend_start::text=$2 AND a.datname=current_database() AND a.state='idle' AND a.xact_start IS NULL AND l.locktype='advisory' AND l.granted AND l.objsubid=1",
    )
    .bind(pid)
    .bind(birth)
    .fetch_all(connection)
    .await?;
    ensure!(keys.len() == 1, "expected one granted advisory lock: {keys:?}");
    Ok(keys[0])
}

/// Fixture installs follow the production route: the inventory through 45 by
/// SQLx's in-order runner, then every S8-4 version below `version` through
/// the bounded adapter and the official runner.
async fn install_through(pool: &PgPool, version: i64) -> Result<()> {
    through(VERSION45).run(pool).await?;
    if version > VERSION45 {
        bounded_install(pool, &through(version)).await?;
    }
    Ok(())
}

/// `run_direct` on a fresh bounded adapter. A fixture DO can still exceed
/// the 1 s statement bound under host load (57014, MayHaveCommitted); the
/// runbook's answer, a rerun that skips what landed, is applied at most twice
/// and reported. The subject of each test is never retried.
async fn bounded_install(pool: &PgPool, inventory: &Migrator) -> Result<()> {
    let options = pool.connect_options();
    for run in 1..=3 {
        let mut adapter = BoundedMigration::connect(options.as_ref()).await?;
        let error = match inventory.run_direct(&mut adapter).await {
            Ok(()) => return Ok(()),
            Err(error) => error,
        };
        let timed_out = match &error {
            MigrateError::ExecuteMigration(e, v) if *v >= VERSION46 => e.as_database_error(),
            _ => None,
        }
        .is_some_and(|db| db.code().as_deref() == Some("57014"));
        ensure!(timed_out && run < 3, "bounded fixture install failed: {error}");
        eprintln!("S84_FIXTURE_RERUN run={run} after {error}");
    }
    unreachable!("the third run returns or fails")
}

struct Fixture {
    version: i64,
    migration: &'static Migration,
    adapter: BoundedMigration,
    observer: PgConnection,
    proxy: DbProxy,
    pid: i32,
    birth: String,
    key: AdvisoryKey,
    catalog: String,
    history: Vec<MigrationRow>,
    baseline_statements: u64,
}

impl Fixture {
    /// The real predecessor inventory, applied in order, then the measured
    /// adapter behind the proxy holding SQLx's advisory lock, as run_direct.
    async fn new(pool: &PgPool, version: i64) -> Result<Self> {
        let options = pool.connect_options();
        let upstream = tokio::net::lookup_host((options.get_host(), options.get_port()))
            .await?
            .next()
            .context("no local test address")?;
        ensure!(upstream.ip().is_loopback(), "S8-4 fixture requires local PostgreSQL");
        install_through(pool, version - 1).await?;
        let proxy = DbProxy::start(upstream).await?;
        let proxied = options
            .as_ref()
            .clone()
            .host("127.0.0.1")
            .port(proxy.addr().port())
            .ssl_mode(PgSslMode::Disable);
        let mut adapter = BoundedMigration::connect(&proxied).await?;
        let (pid, birth): (i32, String) = sqlx::query_as(
            "SELECT pg_backend_pid(),backend_start::text FROM pg_stat_activity WHERE pid=pg_backend_pid()",
        )
        .fetch_one(adapter.connection()?)
        .await?;
        sqlx::query("SELECT set_config('lock_timeout','310ms',false),set_config('statement_timeout','410ms',false),set_config('idle_in_transaction_session_timeout','510ms',false)")
            .execute(adapter.connection()?).await?;
        adapter.lock().await?;
        let mut observer = options.connect().await?;
        let key = acquired_key(&mut observer, pid, &birth).await?;
        for later in SPOT_SCHEMA.into_iter().chain(CUTOVER_SCHEMA).filter(|later| *later >= version)
        {
            ensure!(!landed(&mut observer, later).await?, "M{} already present", later % 100);
        }
        let history: Vec<MigrationRow> = sqlx::query_as(HISTORY).fetch_all(&mut observer).await?;
        let embedded: Vec<_> = crate::MIGRATOR.iter().filter(|m| m.version < version).collect();
        ensure!(history.len() == embedded.len(), "actual immutable predecessor inventory");
        for (row, installed) in history.iter().zip(embedded) {
            ensure!(
                (row.0, row.1.as_str(), row.3, row.4.as_slice())
                    == (
                        installed.version,
                        installed.description.as_ref(),
                        true,
                        installed.checksum.as_ref()
                    ),
                "predecessor {} is not the embedded one",
                row.0
            );
        }
        let catalog: String = sqlx::query_scalar(CATALOG).fetch_one(&mut observer).await?;
        let baseline_statements = proxy.stats().statements;
        proxy.enable_exact_query_trace(16);
        Ok(Self {
            version,
            migration: contract(version),
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
        let m = self.version % 100;
        assert_eq!(sql.len(), count, "M{m} exact wire prefix length: {sql:?}");
        assert_eq!(self.proxy.stats().statements - self.baseline_statements, count as u64);
        assert_eq!(self.proxy.query_trace_dropped(), 0);
        let expected = [
            "WITH old AS MATERIALIZED",
            "BEGIN",
            self.migration.sql.trim(),
            public_apply::metadata_insert_sql_for_tests().trim(),
            "COMMIT",
            ELAPSED,
            RESTORE,
        ];
        for &index in &[0, 5, 6] {
            if index < count {
                assert!(
                    sql[index].starts_with(expected[index]),
                    "M{m} wire slot {index}: {:?}",
                    sql[index]
                );
            }
        }
        for index in 1..count.min(5) {
            assert_eq!(sql[index], expected[index], "M{m} wire slot {index}");
        }
    }

    /// Retire the adapter by raw drop, then prove from a fresh session that
    /// its backend and advisory key are gone and what landed, if anything.
    async fn finish(self, landed_with_elapsed: Option<bool>) -> Result<()> {
        let Self {
            version,
            migration,
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
        // The raw-dropped backend may still finish a statement under the 1 s
        // statement_timeout before it notices the closed socket.
        timeout(NATIVE_RETIREMENT, async {
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
        .await
        .context("original S8-4 connection still present")??;
        ensure!(proxy.stats().statements == before, "retirement flushed SQL");
        timeout(ACQUIRE_BOUND, observer.lock()).await??;
        let same: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM pg_locks
            WHERE pid=pg_backend_pid() AND locktype='advisory' AND granted AND objsubid=1
            AND classid::bigint=$1 AND objid::bigint=$2)",
        )
        .bind(key.0)
        .bind(key.1)
        .fetch_one(&mut observer)
        .await?;
        ensure!(same, "observer must acquire the exact retired adapter's advisory key");
        let mut actual: Vec<MigrationRow> =
            sqlx::query_as(HISTORY).fetch_all(&mut observer).await?;
        let schema = landed(&mut observer, version).await?;
        if let Some(has_elapsed) = landed_with_elapsed {
            ensure!(schema, "M{} schema missing", version % 100);
            let (row_version, description, _, success, checksum, elapsed) =
                actual.pop().context("S8-4 history row")?;
            ensure!(
                (row_version, description.as_str(), success, checksum.as_slice())
                    == (version, migration.description.as_ref(), true, migration.checksum.as_ref())
            );
            if has_elapsed {
                ensure!(elapsed >= 0, "elapsed acknowledgement outcome: {elapsed}");
            } else {
                ensure!(elapsed == -1, "unwritten elapsed value: {elapsed}");
            }
            for (kind, name) in objects(version) {
                if *kind == "relation" && !name.contains(".idx_") {
                    // The name comes from this file's fixed inventory.
                    let rows: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {name}"))
                        .fetch_one(&mut observer)
                        .await?;
                    ensure!(rows == 0, "M{} populated {name}", version % 100);
                }
            }
        } else {
            ensure!(!schema, "schema escaped a failed M{} transaction", version % 100);
            let after: String = sqlx::query_scalar(CATALOG).fetch_one(&mut observer).await?;
            ensure!(after == catalog, "M{} changed the catalog without landing", version % 100);
        }
        ensure!(actual == history, "M{} changed prior migration history", version % 100);
        observer.unlock().await?;
        Ok(())
    }
}

async fn healthy(pool: PgPool, version: i64) -> Result<()> {
    let mut f = Fixture::new(&pool, version).await?;
    let start = Instant::now();
    f.adapter.apply(f.migration).await?;
    let duration = start.elapsed();
    ensure!(f.adapter.status() == (Status { phase: Phase::Restored, failure: None }));
    f.assert_wire_prefix(7);
    let settings: (i32, String, String, String) =
        sqlx::query_as(SETTINGS).fetch_one(f.adapter.connection()?).await?;
    ensure!(settings == (f.pid, "310ms".into(), "410ms".into(), "510ms".into()));
    let elapsed_ns: i64 =
        sqlx::query_scalar("SELECT execution_time FROM _sqlx_migrations WHERE version=$1")
            .bind(version)
            .fetch_one(&mut f.observer)
            .await?;
    eprintln!(
        "M{} healthy apply duration={duration:?} begin_to_commit_ns={elapsed_ns} exact_sql=7",
        version % 100
    );
    ensure!(duration < APPLY_BOUND);
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

async fn reject_metadata(observer: &mut PgConnection, version: i64, commit: bool) -> Result<()> {
    // The version is one of this file's fixed constants.
    sqlx::raw_sql(&format!(
        "CREATE FUNCTION reject_s84() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN
         IF NEW.version={version} THEN
           RAISE EXCEPTION 'native M{} metadata rejected' USING ERRCODE='23514';
         END IF; RETURN NEW; END $$",
        version % 100
    ))
    .execute(&mut *observer)
    .await?;
    let trigger = if commit {
        "CREATE CONSTRAINT TRIGGER reject_s84 AFTER INSERT ON _sqlx_migrations DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION reject_s84()"
    } else {
        "CREATE TRIGGER reject_s84 BEFORE UPDATE ON _sqlx_migrations FOR EACH ROW EXECUTE FUNCTION reject_s84()"
    };
    sqlx::query(trigger).execute(observer).await?;
    Ok(())
}

async fn fault(pool: PgPool, version: i64, case: Fault) -> Result<()> {
    let mut f = Fixture::new(&pool, version).await?;
    let mut holder = if case == Fault::ParentLock {
        let parent = parent(version);
        let mut tx = pool.begin().await?;
        // The parent comes from this file's fixed inventory.
        sqlx::raw_sql(&format!("LOCK TABLE public.{parent} IN ACCESS EXCLUSIVE MODE"))
            .execute(&mut *tx)
            .await?;
        let (pid, birth): (i32, String) = sqlx::query_as(
            "SELECT pg_backend_pid(),backend_start::text FROM pg_stat_activity WHERE pid=pg_backend_pid()",
        )
        .fetch_one(&mut *tx)
        .await?;
        let holds: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM pg_locks l JOIN pg_stat_activity a USING(pid)
            WHERE l.pid=$1 AND a.backend_start::text=$2 AND a.datname=current_database() AND l.granted
            AND l.relation=to_regclass($3) AND l.mode='AccessExclusiveLock')",
        )
        .bind(pid)
        .bind(&birth)
        .bind(format!("public.{parent}"))
        .fetch_one(&mut f.observer)
        .await?;
        ensure!(holds, "fixture must positively hold the exact referenced lock");
        Some(tx)
    } else {
        None
    };
    match case {
        Fault::RejectCommit | Fault::RejectElapsed => {
            reject_metadata(&mut f.observer, version, case == Fault::RejectCommit).await?;
            // The rejecting trigger is the fixture's own catalog change.
            f.catalog = sqlx::query_scalar(CATALOG).fetch_one(&mut f.observer).await?;
        },
        Fault::DoAckLoss => {
            f.proxy.set_mode(ProxyMode::DropSimpleInTransactionAckOf(do_head(version)))
        },
        Fault::CommitAckLoss => f.proxy.set_mode(ProxyMode::DropCommitAckOf(do_head(version))),
        Fault::ElapsedAckLoss => f.proxy.set_mode(ProxyMode::DropAutocommitAckOf(ELAPSED)),
        _ => {},
    }

    let restore = matches!(case, Fault::RestoreSever | Fault::RestoreAckLoss);
    let started = Instant::now();
    let result = if restore {
        let (hit, release) = stages::arm(&f.adapter.gate, Stage::Restore, Edge::BeforeCall)?;
        let proxy = &f.proxy;
        let baseline = f.baseline_statements;
        let apply = f.adapter.apply(f.migration);
        let alter_restore = async {
            // An apply that fails before the restore stage (a DO over the
            // statement bound, say) fails this case instead of hanging it.
            timeout(CONTROLLER_CEILING, hit)
                .await
                .context("the apply never reached its restore stage")??;
            ensure!(proxy.stats().statements - baseline == 6);
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
        f.adapter.apply(f.migration).await
    };
    let duration = started.elapsed();
    if let Some(holder) = holder.take() {
        holder.rollback().await?;
    }
    eprintln!("M{} {case:?} duration={duration:?} result={result:?}", version % 100);
    let error = result.err().context("fixture fault did not interrupt the actual apply")?;
    ensure!(
        duration < APPLY_BOUND,
        "{case:?} did not complete within the fixed whole-apply limit"
    );
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
    ensure!(
        f.adapter.status()
            == (Status {
                phase: Phase::Retired,
                failure: Some(expected)
            })
    );
    ensure!(refuses_after_retirement(&mut f.adapter).await);

    let (prefix, landed, native_rejection) = match case {
        Fault::ParentLock => (3, None, Some("55P03")),
        Fault::RejectCommit => (5, None, Some("23514")),
        Fault::DoAckLoss => (3, None, None),
        Fault::CommitAckLoss => (5, Some(false), None),
        Fault::RejectElapsed => (6, Some(false), Some("23514")),
        Fault::ElapsedAckLoss | Fault::RestoreSever => (6, Some(true), None),
        Fault::RestoreAckLoss => (7, Some(true), None),
    };
    if let Some(code) = native_rejection {
        let db = match &error {
            MigrateError::Execute(e) => e.as_database_error(),
            MigrateError::ExecuteMigration(e, v) if *v == version => e.as_database_error(),
            _ => None,
        }
        .context("native PostgreSQL error")?;
        ensure!(db.code().as_deref() == Some(code), "{case:?}: {db:?}");
        if case != Fault::ParentLock {
            ensure!(db.message() == format!("native M{} metadata rejected", version % 100));
        }
    }
    ensure!(f.proxy.simple_in_transaction_acks_dropped() == u64::from(case == Fault::DoAckLoss));
    ensure!(f.proxy.stats().commit_acks_dropped == u64::from(case == Fault::CommitAckLoss));
    ensure!(
        f.proxy.autocommit_acks_dropped()
            == u64::from(matches!(case, Fault::ElapsedAckLoss | Fault::RestoreAckLoss))
    );
    f.assert_wire_prefix(prefix);
    f.finish(landed).await
}

/// One module per version: the healthy apply and migration 40's eight faults.
macro_rules! bounded_suite {
    ($($module:ident => $version:expr),* $(,)?) => {$(
        mod $module {
            use super::*;

            #[sqlx::test(migrations = false)]
            #[ignore = "requires isolated PostgreSQL DATABASE_URL"]
            async fn healthy_restores_settings_and_keeps_exact_history(pool: PgPool) -> Result<()> {
                healthy(pool, $version).await
            }

            #[sqlx::test(migrations = false)]
            #[ignore = "requires isolated PostgreSQL DATABASE_URL"]
            async fn contended_parent_blocks_the_do(pool: PgPool) -> Result<()> {
                fault(pool, $version, Fault::ParentLock).await
            }

            #[sqlx::test(migrations = false)]
            #[ignore = "requires isolated PostgreSQL DATABASE_URL"]
            async fn native_commit_rejection_rolls_back(pool: PgPool) -> Result<()> {
                fault(pool, $version, Fault::RejectCommit).await
            }

            #[sqlx::test(migrations = false)]
            #[ignore = "requires isolated PostgreSQL DATABASE_URL"]
            async fn lost_do_ack_has_no_commit(pool: PgPool) -> Result<()> {
                fault(pool, $version, Fault::DoAckLoss).await
            }

            #[sqlx::test(migrations = false)]
            #[ignore = "requires isolated PostgreSQL DATABASE_URL"]
            async fn lost_commit_ack_leaves_catalog_and_metadata(pool: PgPool) -> Result<()> {
                fault(pool, $version, Fault::CommitAckLoss).await
            }

            #[sqlx::test(migrations = false)]
            #[ignore = "requires isolated PostgreSQL DATABASE_URL"]
            async fn native_elapsed_rejection_leaves_committed_catalog(pool: PgPool) -> Result<()> {
                fault(pool, $version, Fault::RejectElapsed).await
            }

            #[sqlx::test(migrations = false)]
            #[ignore = "requires isolated PostgreSQL DATABASE_URL"]
            async fn lost_elapsed_ack_leaves_landed_elapsed(pool: PgPool) -> Result<()> {
                fault(pool, $version, Fault::ElapsedAckLoss).await
            }

            #[sqlx::test(migrations = false)]
            #[ignore = "requires isolated PostgreSQL DATABASE_URL"]
            async fn sever_before_restore_never_sends_restore(pool: PgPool) -> Result<()> {
                fault(pool, $version, Fault::RestoreSever).await
            }

            #[sqlx::test(migrations = false)]
            #[ignore = "requires isolated PostgreSQL DATABASE_URL"]
            async fn lost_restore_ack_retires_same_backend(pool: PgPool) -> Result<()> {
                fault(pool, $version, Fault::RestoreAckLoss).await
            }
        }
    )*};
}

bounded_suite!(
    m46 => VERSION46, m47 => VERSION47, m48 => VERSION48, m49 => VERSION49, m50 => VERSION50,
    m51 => VERSION51, m52 => VERSION52, m53 => VERSION53, m54 => VERSION54,
    m55 => VERSION55, m56 => VERSION56,
);

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn migration46_to_50_refuse_a_non_identical_source_before_sql(pool: PgPool) -> Result<()> {
    let mut f = Fixture::new(&pool, VERSION46).await?;
    for version in VERSIONS {
        let migration = contract(version);
        let foreign = contract(if version == VERSION46 {
            VERSION47
        } else {
            VERSION46
        });
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
            Migration {
                description: Cow::Borrowed("other"),
                ..migration.clone()
            },
            Migration {
                migration_type: MigrationType::ReversibleUp,
                ..migration.clone()
            },
            Migration {
                sql: foreign.sql.clone(),
                checksum: foreign.checksum.clone(),
                ..migration.clone()
            },
        ] {
            let denied =
                f.adapter.apply(&wrong).await.err().context("unembedded S8-4 source applied")?;
            let expected = format!("migration{} contract mismatch", version % 100);
            ensure!(
                matches!(&denied, MigrateError::Source(source) if source.to_string() == expected),
                "M{}: {denied}",
                version % 100
            );
            ensure!(f.adapter.status() == (Status { phase: Phase::Preflight, failure: None }));
            f.assert_wire_prefix(0);
        }
    }
    for version in VERSIONS {
        ensure!(!landed(&mut f.observer, version).await?);
    }
    f.adapter.unlock().await?;
    f.finish(None).await
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn migrations46_to_50_official_runner_applies_each_once_in_order(pool: PgPool) -> Result<()> {
    let mut f = Fixture::new(&pool, VERSION46).await?;
    // run_direct takes the same advisory lock itself. The image is frozen at
    // 50, the one that carries S8-4; S8-5's runner witness is migration51.rs's.
    let image = through(VERSION50);
    f.adapter.unlock().await?;
    f.proxy.enable_exact_query_trace(64);
    let before = f.proxy.stats().statements;
    let started = Instant::now();
    image.run_direct(&mut f.adapter).await?;
    let duration = started.elapsed();
    let trace: Vec<String> =
        f.proxy.query_trace().iter().map(|sql| sql.trim().to_owned()).collect();
    let statements = f.proxy.stats().statements - before;
    ensure!(
        statements == 42 && trace.len() == 42 && f.proxy.query_trace_dropped() == 0,
        "{trace:?}"
    );
    ensure!(trace[0] == "SELECT current_database()" && trace[1] == "SELECT pg_advisory_lock($1)");
    for (k, version) in VERSIONS.into_iter().enumerate() {
        let slot = 5 + 7 * k;
        ensure!(
            trace[slot].starts_with("WITH old AS MATERIALIZED"),
            "install before M{}",
            version % 100
        );
        ensure!(trace[slot + 1] == "BEGIN");
        ensure!(
            trace[slot + 2].trim() == contract(version).sql.trim(),
            "M{} out of order",
            version % 100
        );
        ensure!(trace[slot + 3].trim() == public_apply::metadata_insert_sql_for_tests().trim());
        ensure!(trace[slot + 4] == "COMMIT");
        ensure!(trace[slot + 5].starts_with(ELAPSED), "{:?}", trace[slot + 5]);
        ensure!(trace[slot + 6].starts_with(RESTORE), "{:?}", trace[slot + 6]);
    }
    ensure!(
        trace[40] == "SELECT current_database()" && trace[41] == "SELECT pg_advisory_unlock($1)"
    );
    ensure!(f.adapter.status() == (Status { phase: Phase::Restored, failure: None }));
    let history: Vec<MigrationRow> = sqlx::query_as(HISTORY).fetch_all(&mut f.observer).await?;
    ensure!(history[..f.history.len()] == f.history[..], "prior history changed");
    let added = &history[f.history.len()..];
    ensure!(added.len() == VERSIONS.len());
    for ((version, description, _, success, checksum, elapsed), expected) in
        added.iter().zip(VERSIONS)
    {
        let migration = contract(expected);
        ensure!(
            (*version, description.as_str(), *success, checksum.as_slice())
                == (expected, migration.description.as_ref(), true, migration.checksum.as_ref())
        );
        ensure!(*elapsed >= 0);
        ensure!(landed(&mut f.observer, expected).await?);
        eprintln!("M{} official begin_to_commit_ns={elapsed}", expected % 100);
    }
    eprintln!("M46-M50 official runner physical_sql={statements} duration={duration:?}");
    // A second official run skips every applied version.
    let again = f.proxy.stats().statements;
    image.run_direct(&mut f.adapter).await?;
    ensure!(f.proxy.stats().statements - again == 7, "a repeated run applied something");
    ensure!(
        sqlx::query_as::<_, MigrationRow>(HISTORY).fetch_all(&mut f.observer).await? == history
    );
    Ok(())
}

/// An older image's embedded inventory, with this binary's own runner
/// safeguards (a frozen predecessor of the production `MIGRATOR`).
fn image_through(version: i64) -> Migrator {
    Migrator {
        migrations: Cow::Owned(
            crate::MIGRATOR.iter().filter(|m| m.version <= version).cloned().collect(),
        ),
        ignore_missing: crate::MIGRATOR.ignore_missing,
        locking: crate::MIGRATOR.locking,
        no_tx: crate::MIGRATOR.no_tx,
    }
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn migrations46_to_50_older_images_refuse_with_version_missing(pool: PgPool) -> Result<()> {
    install_through(&pool, VERSION50).await?;
    let options = pool.connect_options();
    let mut observer = options.connect().await?;
    let history: Vec<MigrationRow> = sqlx::query_as(HISTORY).fetch_all(&mut observer).await?;
    let catalog: String = sqlx::query_scalar(CATALOG).fetch_one(&mut observer).await?;
    for version in VERSIONS {
        ensure!(landed(&mut observer, version).await?);
    }
    // Each older image, from the one before 46 to the one before 50, stops
    // at the first applied version missing from its embedded set, before any
    // apply: the hook fails and the sync stops.
    for missing in VERSIONS {
        let mut older = BoundedMigration::connect(options.as_ref()).await?;
        let refused = timeout(ACQUIRE_BOUND, image_through(missing - 1).run_direct(&mut older))
            .await
            .context("older image did not answer")?;
        ensure!(
            matches!(refused, Err(MigrateError::VersionMissing(v)) if v == missing),
            "image through M{} did not refuse with VersionMissing(M{}): {refused:?}",
            (missing - 1) % 100,
            missing % 100
        );
        ensure!(
            older.status() == (Status { phase: Phase::Preflight, failure: None }),
            "older image applied"
        );
        // SQLx returns before its unlock: only the raw drop releases the key.
        drop(older);
    }
    ensure!(sqlx::query_as::<_, MigrationRow>(HISTORY).fetch_all(&mut observer).await? == history);
    ensure!(sqlx::query_scalar::<_, String>(CATALOG).fetch_one(&mut observer).await? == catalog);
    // The canonical image of S8-4 (frozen at 50; later files are their own
    // witnesses') skips every applied version and unlocks.
    let mut canonical = BoundedMigration::connect(options.as_ref()).await?;
    timeout(ACQUIRE_BOUND, image_through(VERSION50).run_direct(&mut canonical))
        .await
        .context("canonical image did not answer")??;
    ensure!(sqlx::query_as::<_, MigrationRow>(HISTORY).fetch_all(&mut observer).await? == history);
    eprintln!("M46-M50 rollback boundary: images through M45..M49 refused with VersionMissing; canonical skipped");
    Ok(())
}
