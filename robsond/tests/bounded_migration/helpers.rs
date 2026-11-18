use std::{borrow::Cow, time::Duration};

use robson_testkit::db_proxy::{DbProxy, ProxyMode, ProxyStats};
use serde_json::Value;
use sqlx::{postgres::PgSslMode, PgConnection, PgPool};
use uuid::Uuid;

use super::super::*;

const CATALOG: &str = r#"
SELECT jsonb_build_object(
 'relations',(SELECT jsonb_agg(ROW(c.relname,c.relkind)::text ORDER BY c.relname)
   FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace
   WHERE n.nspname='public' AND c.relkind IN ('r','p','i','S','v')),
 'columns',(SELECT jsonb_agg(ROW(c.relname,a.attname,a.atttypid::regtype::text,
   a.attnotnull,a.attidentity,pg_get_expr(d.adbin,d.adrelid))::text ORDER BY c.relname,a.attnum)
   FROM pg_attribute a JOIN pg_class c ON c.oid=a.attrelid
   JOIN pg_namespace n ON n.oid=c.relnamespace
   LEFT JOIN pg_attrdef d ON d.adrelid=c.oid AND d.adnum=a.attnum
   WHERE n.nspname='public' AND a.attnum>0 AND NOT a.attisdropped),
 'constraints',(SELECT jsonb_agg(ROW(c.conname,t.relname,c.contype,
   c.convalidated,c.condeferrable,c.condeferred,pg_get_constraintdef(c.oid))::text
   ORDER BY t.relname,c.conname)
   FROM pg_constraint c JOIN pg_class t ON t.oid=c.conrelid
   JOIN pg_namespace n ON n.oid=t.relnamespace WHERE n.nspname='public'),
 'functions',(SELECT jsonb_agg(ROW(p.proname,pg_get_function_identity_arguments(p.oid),
   pg_get_functiondef(p.oid))::text ORDER BY p.proname,pg_get_function_identity_arguments(p.oid))
   FROM pg_proc p JOIN pg_namespace n ON n.oid=p.pronamespace
   WHERE n.nspname='public' AND p.prokind IN ('f','p')),
 'triggers',(SELECT jsonb_agg(ROW(c.relname,t.tgname,pg_get_triggerdef(t.oid))::text
   ORDER BY c.relname,t.tgname) FROM pg_trigger t JOIN pg_class c ON c.oid=t.tgrelid
   JOIN pg_namespace n ON n.oid=c.relnamespace
   WHERE n.nspname='public' AND NOT t.tgisinternal))
"#;
const LEGACY: &str = "SELECT jsonb_build_object('intents',(SELECT jsonb_agg(to_jsonb(i) ORDER BY intent_id) FROM order_intents i),'attempts',(SELECT jsonb_agg(to_jsonb(a) ORDER BY attempt_id) FROM submit_attempts a),'history',(SELECT jsonb_agg(to_jsonb(m) ORDER BY version) FROM _sqlx_migrations m WHERE version<20240101000038))";

pub(super) struct Fixture {
    pub adapter: BoundedMigration,
    pub proxy: DbProxy,
    pub observer: PgConnection,
    pub pid: i32,
    pub catalog_before: Value,
    pub legacy_before: Value,
    pub baseline: ProxyStats,
}

pub(super) struct Finished {
    pub retired: bool,
    pub status: Status,
    pub stats: ProxyStats,
    pub tail: Vec<String>,
    pub dropped: u64,
    pub catalog: Value,
    pub legacy: Value,
    pub metadata: Option<(String, bool, Vec<u8>, i64)>,
}

pub(super) fn migration() -> &'static Migration {
    crate::MIGRATOR
        .iter()
        .find(|m| m.version == VERSION)
        .expect("embedded original38")
}

impl Fixture {
    pub async fn new(pool: &PgPool) -> anyhow::Result<Self> {
        Self::new_with_diagnostics(pool, false).await
    }

    pub(super) async fn new_with_diagnostics(
        pool: &PgPool,
        diagnostics: bool,
    ) -> anyhow::Result<Self> {
        let mut through37 = sqlx::migrate!("../migrations");
        through37.migrations =
            Cow::Owned(through37.iter().filter(|m| m.version < VERSION).cloned().collect());
        through37.run(pool).await?;
        let intent = Uuid::now_v7();
        sqlx::query("INSERT INTO order_intents (intent_id,tenant_id,venue_id,account_scope,client_order_id,kind,params,created_at) VALUES ($1,$2,'binance:usdm','scope','legacy-public52','exit','{}',now())")
            .bind(intent).bind(Uuid::now_v7()).execute(pool).await?;
        sqlx::query("INSERT INTO submit_attempts (attempt_id,venue_id,account_scope,client_order_id,state,created_at) VALUES ($1,'binance:usdm','scope','legacy-public52','started',now())")
            .bind(Uuid::now_v7()).execute(pool).await?;
        let options = pool.connect_options();
        let upstream = tokio::net::lookup_host((options.get_host(), options.get_port()))
            .await?
            .next()
            .ok_or_else(|| std::io::Error::other("no supplied PG address"))?;
        let proxy = DbProxy::start(upstream).await?;
        if diagnostics {
            proxy.enable_diagnostics();
        }
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
        // Warm observer queries before the measured owner operation.
        let _: bool = alive(&mut observer, pid).await?;
        let _: Option<(String, bool, Vec<u8>, i64)> = metadata(&mut observer).await?;
        let catalog_before = catalog(&mut observer).await?;
        let legacy_before = legacy(&mut observer).await?;
        let baseline = proxy.stats();
        proxy.enable_query_trace(32);
        Ok(Self {
            adapter,
            proxy,
            observer,
            pid,
            catalog_before,
            legacy_before,
            baseline,
        })
    }

    pub async fn refreshed_catalog(&mut self) -> anyhow::Result<()> {
        self.catalog_before = catalog(&mut self.observer).await?;
        Ok(())
    }

    pub async fn finish(self) -> anyhow::Result<Finished> {
        let Self { adapter, proxy, mut observer, pid, .. } = self;
        let cleanup_deadline = tokio::time::Instant::now() + ACQUIRE_BOUND;
        let status = adapter.status();
        drop(adapter); // Whole raw owner, never graceful-close or pool-return.
        let retirement = tokio::time::timeout(Duration::from_millis(300), async {
            for _ in 0..60 {
                if !alive(&mut observer, pid).await? {
                    return Ok::<_, sqlx::Error>(true);
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            Ok(false)
        })
        .await;
        // Capture end state and dispose the proxy even if a polling query hung.
        let stats = proxy.stats();
        let tail = proxy.query_trace();
        let dropped = proxy.query_trace_dropped();
        proxy.set_mode(ProxyMode::Sever);
        drop(proxy);
        // One absolute existing5s cleanup ceiling, shared by barrier, all three
        // snapshots and unlock; it is not a fresh5s allowance per query.
        let snapshots = tokio::time::timeout_at(cleanup_deadline, async {
            observer.lock().await?;
            let catalog = catalog(&mut observer).await?;
            let legacy = legacy(&mut observer).await?;
            let metadata = metadata(&mut observer).await?;
            observer.unlock().await?;
            Ok::<_, anyhow::Error>((catalog, legacy, metadata))
        })
        .await;
        // A cancelled/failed observer is raw-dropped, releasing any advisory lock.
        drop(observer);
        eprintln!("cleanup status={status:?} received_statements={} retirement={retirement:?} snapshots_complete={}",
            stats.statements, matches!(&snapshots, Ok(Ok(_))));
        // No observation error is propagated before all owned resources drop.
        let retired = retirement??;
        let (catalog, legacy, metadata) = snapshots??;
        Ok(Finished {
            retired,
            status,
            stats,
            tail,
            dropped,
            catalog,
            legacy,
            metadata,
        })
    }
}

pub(super) async fn alive(c: &mut PgConnection, pid: i32) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE pid=$1)")
        .bind(pid)
        .fetch_one(c)
        .await
}

pub(super) async fn catalog(c: &mut PgConnection) -> Result<Value, sqlx::Error> {
    sqlx::query_scalar(CATALOG).fetch_one(c).await
}

pub(super) async fn legacy(c: &mut PgConnection) -> Result<Value, sqlx::Error> {
    sqlx::query_scalar(LEGACY).fetch_one(c).await
}

pub(super) async fn metadata(
    c: &mut PgConnection,
) -> Result<Option<(String, bool, Vec<u8>, i64)>, sqlx::Error> {
    sqlx::query_as("SELECT description,success,checksum,execution_time FROM _sqlx_migrations WHERE version=20240101000038")
        .fetch_optional(c).await
}

pub(super) async fn refuses_after_retirement(adapter: &mut BoundedMigration) -> bool {
    let mut next = migration().clone();
    next.version += 1;
    let mut refused = adapter.ensure_migrations_table().await.is_err();
    refused &= adapter.dirty_version().await.is_err();
    refused &= adapter.list_applied_migrations().await.is_err();
    refused &= adapter.lock().await.is_err();
    refused &= adapter.unlock().await.is_err();
    refused &= adapter.apply(migration()).await.is_err();
    refused &= adapter.apply(&next).await.is_err();
    refused &= adapter.revert(&next).await.is_err();
    refused
}

/// Four bounded RECEIVED-text windows. Not Forwarded, held response or C3
/// proof.
pub(super) struct Window {
    pub text: Vec<String>,
    pub dropped: u64,
    pub statements: u64,
}

pub(super) async fn apply_in_windows(
    f: &mut Fixture,
) -> anyhow::Result<(Result<Duration, MigrateError>, Result<Vec<Window>, MigrateError>, Duration)> {
    use super::super::stages::{self, Edge, Stage};
    let slot = f.adapter.gate.clone();
    let points = [
        Stage::Ddl(16),
        Stage::Ddl(32),
        Stage::Ddl(48),
        Stage::Restore,
    ];
    let (first_hit, first_release) = stages::arm(&slot, points[0], Edge::AfterReturn)?;
    let proxy = &f.proxy;
    let baseline = f.baseline.statements;
    let controller = async {
        let mut current = (first_hit, first_release);
        let mut windows = Vec::with_capacity(4);
        for n in 0..4 {
            current.0.await.map_err(|_| refusal("fixture stage hit missing"))?;
            windows.push(Window {
                text: proxy.query_trace(),
                dropped: proxy.query_trace_dropped(),
                statements: proxy.stats().statements - baseline,
            });
            let next = if n + 1 < 4 {
                Some(stages::arm(&slot, points[n + 1], Edge::AfterReturn)?)
            } else {
                None
            };
            proxy.enable_query_trace(32);
            current.1.send(()).map_err(|_| refusal("fixture stage release lost"))?;
            if let Some(next) = next {
                current = next;
            } else {
                break;
            }
        }
        Ok::<_, MigrateError>(windows)
    };
    // Observer ceiling is the approved60x5ms fixture bound. It never replaces
    // the independent absolute200ms production apply timer (which includes gates).
    let producer = async {
        let started = std::time::Instant::now();
        let result = f.adapter.apply(migration()).await;
        (result, started.elapsed())
    };
    let ((result, producer_duration), windows) =
        tokio::join!(producer, tokio::time::timeout(Duration::from_millis(300), controller));
    let windows = match windows {
        Ok(observed) => observed,
        Err(_) => Err(refusal("fixture stage observer expired")),
    };
    Ok((result, windows, producer_duration))
}
