//! Isolated native source-audit witnesses. No exchange or production
//! connection.
use std::{cell::RefCell, time::Duration};

use anyhow::{anyhow, ensure, Result};
use tokio::sync::oneshot;

use super::*;

const SCOPE: &str = "spot-cold-local-audit";
const BUDGET: PageBudget = PageBudget {
    intents: 2,
    attempts: 2,
    orders: 2,
    routes: 2,
};

type Pause = (oneshot::Sender<()>, oneshot::Receiver<()>);

tokio::task_local! {
    static PAUSE_AFTER_INTENTS: RefCell<Option<Pause>>;
}

pub(super) async fn after_intents() {
    if let Ok(Some((reached, resume))) =
        PAUSE_AFTER_INTENTS.try_with(|slot| slot.borrow_mut().take())
    {
        let _ = reached.send(());
        let _ = resume.await;
    }
}

#[derive(Default)]
struct Keys {
    intents: Vec<String>,
    attempts: Vec<String>,
    orders: Vec<String>,
    routes: Vec<Uuid>,
}

impl ColdSourceSink for Keys {
    fn intents(&mut self, rows: &[IntentSource]) -> Result<(), SinkFull> {
        bounded(&mut self.intents, rows.iter().map(|r| r.client_order_id.clone()))
    }
    fn attempts(&mut self, rows: &[AttemptSource]) -> Result<(), SinkFull> {
        bounded(&mut self.attempts, rows.iter().map(|r| r.client_order_id.clone()))
    }
    fn orders(&mut self, rows: &[OrderSource]) -> Result<(), SinkFull> {
        bounded(&mut self.orders, rows.iter().map(|r| r.exchange_order_id.clone()))
    }
    fn routes(&mut self, rows: &[RouteSource]) -> Result<(), SinkFull> {
        bounded(&mut self.routes, rows.iter().map(|r| r.position_id))
    }
}

fn bounded<T>(
    target: &mut Vec<T>,
    incoming: impl ExactSizeIterator<Item = T>,
) -> Result<(), SinkFull> {
    if target.len() + incoming.len() > 16 {
        return Err(SinkFull);
    }
    target.extend(incoming);
    Ok(())
}

fn scope(tenant_id: Uuid) -> ColdAuditScope<'static> {
    ColdAuditScope { tenant_id, account_scope: SCOPE }
}

fn deadline() -> Instant {
    Instant::now() + Duration::from_secs(3)
}

async fn setup(pool: &PgPool) -> Result<Uuid> {
    let options = pool.connect_options();
    let mut addresses = tokio::net::lookup_host((options.get_host(), options.get_port())).await?;
    ensure!(addresses.next().is_some_and(|address| address.ip().is_loopback()));
    robson_db::migrate(pool).await?;
    let tenant = Uuid::now_v7();
    sqlx::query("INSERT INTO accounting_state(tenant_id,venue_id,budget_model) VALUES($1,'binance:spot','hwm_v1')")
        .bind(tenant).execute(pool).await?;
    sqlx::query(
        "INSERT INTO venue_accounts(venue_id,account_scope,tenant_id,uid,bound_at,cutover_id)
         VALUES('binance:spot',$1,$2,'local-fixture',now(),$3)",
    )
    .bind(SCOPE)
    .bind(tenant)
    .bind(Uuid::now_v7())
    .execute(pool)
    .await?;
    sqlx::query(
        "CREATE INDEX idx_leg_routes_spot_scope_position
          ON public.leg_routes(account_scope,position_id) WHERE venue_id='binance:spot'",
    )
    .execute(pool)
    .await?;
    Ok(tenant)
}

async fn seed(pool: &PgPool, tenant: Uuid, start: i64, end: i64) -> Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query(
        "INSERT INTO order_intents(intent_id,tenant_id,venue_id,account_scope,client_order_id,kind,params,created_at)
         SELECT md5($2||'j'||n)::uuid,$1,'binance:spot',$2,lpad(n::text,3,'0'),'exit','{}',now()
         FROM generate_series($3::bigint,$4::bigint)n",
    ).bind(tenant).bind(SCOPE).bind(start).bind(end).execute(&mut *tx).await?;
    sqlx::query(
        "INSERT INTO submit_attempts(attempt_id,venue_id,account_scope,client_order_id,state,created_at)
         SELECT md5($1||'a'||n)::uuid,'binance:spot',$1,lpad(n::text,3,'0'),'started',now()
         FROM generate_series($2::bigint,$3::bigint)n",
    ).bind(SCOPE).bind(start).bind(end).execute(&mut *tx).await?;
    sqlx::query(
        "INSERT INTO venue_orders(tenant_id,venue_id,account_scope,symbol,exchange_order_id,
         client_order_id,last_client_order_id,owner,side,order_type,orig_qty,status,phase,last_checked_at,live_seen_at)
         SELECT $1,'binance:spot',$2,'BTCUSDT',(10000+n)::text,'c-'||n,'c-'||n,'discovered',
          'SELL','LIMIT',1,'NEW','live',now(),now()
         FROM generate_series($3::bigint,$4::bigint)n",
    ).bind(tenant).bind(SCOPE).bind(start).bind(end).execute(&mut *tx).await?;
    sqlx::query(
        "INSERT INTO leg_routes(position_id,tenant_id,venue_id,account_scope,routed_at)
         SELECT lpad(to_hex(100+n),32,'0')::uuid,$1,'binance:spot',$2,now()
         FROM generate_series($3::bigint,$4::bigint)n",
    )
    .bind(tenant)
    .bind(SCOPE)
    .bind(start)
    .bind(end)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

fn expected(keys: &Keys, n: u64) -> Result<()> {
    let upstream: Vec<_> = (1..=n).map(|i| format!("{i:03}")).collect();
    ensure!(keys.intents == upstream && keys.attempts == upstream);
    ensure!(keys.orders == (1..=n).map(|i| (10000 + i).to_string()).collect::<Vec<_>>());
    ensure!(
        keys.routes == (1..=n).map(|i| Uuid::from_u128(u128::from(100 + i))).collect::<Vec<_>>()
    );
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn cold_spot_audit_four_sources_require_real_terminal_pages(pool: PgPool) -> Result<()> {
    let tenant = setup(&pool).await?;
    for size in [0, 8, 9, 10] {
        if size > 0 {
            seed(&pool, tenant, if size == 8 { 1 } else { size }, size).await?;
        }
        let mut keys = Keys::default();
        let received = audit_cold_sources(&pool, scope(tenant), BUDGET, deadline(), &mut keys)
            .await
            .map_err(|failure| anyhow!("complete source audit at {size}: {failure:?}"))?;
        let pages = if size >= 8 { 2 } else { 1 };
        ensure!(received.tenant_id == tenant && received.account_scope == SCOPE);
        ensure!(received.statements == 6 + 4 * u64::from(pages));
        ensure!(received.families.iter().all(|f| f.pages == pages && f.rows == size as u64));
        expected(&keys, size as u64)?;
    }
    let stopped = audit_cold_sources(
        &pool,
        scope(tenant),
        PageBudget { intents: 1, ..BUDGET },
        deadline(),
        &mut Keys::default(),
    )
    .await;
    ensure!(matches!(stopped,Err(ColdAuditRefusal::PageBudget{
        family:Family::Intents,last_key:Some(SourceKey::ClientOrder(key))
    }) if key=="008"));
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn cold_spot_audit_snapshot_survives_real_publication_across_families(
    pool: PgPool,
) -> Result<()> {
    let tenant = setup(&pool).await?;
    seed(&pool, tenant, 1, 8).await?;
    // A real READ COMMITTED control keeps the first native page at eight,
    // then can mix in the two later rows on another family.
    let mut control = pool.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL READ COMMITTED, READ ONLY")
        .execute(&mut *control)
        .await?;
    let first_control = read_intent_page(&mut control, SCOPE, tenant, None).await?;
    ensure!(first_control.full && first_control.rows.len() == 8);
    let (observed, wait) = oneshot::channel();
    let (release, resume) = oneshot::channel();
    let mut keys = Keys::default();
    let old = PAUSE_AFTER_INTENTS.scope(
        RefCell::new(Some((observed, resume))),
        audit_cold_sources(&pool, scope(tenant), BUDGET, deadline(), &mut keys),
    );
    let next = async {
        wait.await?;
        seed(&pool, tenant, 9, 10).await?;
        release
            .send(())
            .map_err(|_| anyhow!("original audit exited before source publication"))?;
        Ok::<_, anyhow::Error>(())
    };
    let (receipt, published) = tokio::join!(old, next);
    published?;
    let receipt = receipt.map_err(|error| anyhow!("original audit: {error:?}"))?;
    ensure!(receipt.families.iter().all(|f| f.rows == 8));
    expected(&keys, 8)?;
    let control_last = read_attempt_page(&mut control, SCOPE, tenant, Some("008")).await?;
    ensure!(control_last.rows.len() == 2 && control_last.rows[0].client_order_id == "009");
    control.rollback().await?;
    let mut fresh = Keys::default();
    let receipt = audit_cold_sources(&pool, scope(tenant), BUDGET, deadline(), &mut fresh)
        .await
        .map_err(|error| anyhow!("fresh audit: {error:?}"))?;
    ensure!(receipt.families.iter().all(|f| f.rows == 10));
    expected(&fresh, 10)
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn cold_spot_audit_refuses_foreign_page_nine_orphan_and_catalog(pool: PgPool) -> Result<()> {
    let tenant = setup(&pool).await?;
    let incorrect =
        audit_cold_sources(&pool, scope(Uuid::now_v7()), BUDGET, deadline(), &mut Keys::default())
            .await;
    ensure!(matches!(incorrect, Err(ColdAuditRefusal::AccountTenant)));
    let absent = audit_cold_sources(
        &pool,
        ColdAuditScope {
            tenant_id: tenant,
            account_scope: "unused-cold-scope",
        },
        BUDGET,
        deadline(),
        &mut Keys::default(),
    )
    .await;
    ensure!(matches!(absent, Err(ColdAuditRefusal::AccountMissing)));
    seed(&pool, tenant, 1, 8).await?;
    sqlx::query(
        "INSERT INTO public.order_intents(intent_id,tenant_id,venue_id,account_scope,client_order_id,
              kind,params,created_at) VALUES($1,$2,'binance:spot',$3,'009','exit','{}',now())",
    ).bind(Uuid::now_v7()).bind(Uuid::now_v7()).bind(SCOPE).execute(&pool).await?;
    let result =
        audit_cold_sources(&pool, scope(tenant), BUDGET, deadline(), &mut Keys::default()).await;
    ensure!(
        matches!(result,Err(ColdAuditRefusal::Row{family:Family::Intents,reason:RowDefect::ForeignOwner,
        key:SourceKey::ClientOrder(key),last_verified:Some(SourceKey::ClientOrder(last))})
        if key=="009" && last=="008")
    );
    sqlx::query("DELETE FROM order_intents WHERE account_scope=$1 AND client_order_id='009'")
        .bind(SCOPE)
        .execute(&pool)
        .await?;
    let mut seeded = pool.begin().await?;
    sqlx::query("SET LOCAL session_replication_role='replica'")
        .execute(&mut *seeded)
        .await?;
    sqlx::query(
        "INSERT INTO submit_attempts(attempt_id,venue_id,account_scope,client_order_id,state,created_at)
         VALUES($1,'binance:spot',$2,'009','started',now())",
    ).bind(Uuid::now_v7()).bind(SCOPE).execute(&mut *seeded).await?;
    seeded.commit().await?;
    let result =
        audit_cold_sources(&pool, scope(tenant), BUDGET, deadline(), &mut Keys::default()).await;
    ensure!(
        matches!(result,Err(ColdAuditRefusal::Row{family:Family::Attempts,reason:RowDefect::MissingParent,
        key:SourceKey::ClientOrder(key),last_verified:Some(SourceKey::ClientOrder(last))})
        if key=="009" && last=="008")
    );

    sqlx::query("ALTER TABLE public.balance_leg_admissions DISABLE TRIGGER ALL")
        .execute(&pool)
        .await?;
    let result =
        audit_cold_sources(&pool, scope(tenant), BUDGET, deadline(), &mut Keys::default()).await;
    ensure!(
        matches!(result,Err(ColdAuditRefusal::Catalog(label)) if label=="balance_leg_admissions_route"
        || label=="balance_legs_admission")
    );
    sqlx::query("ALTER TABLE public.balance_leg_admissions ENABLE TRIGGER ALL")
        .execute(&pool)
        .await?;
    sqlx::query("DROP INDEX public.idx_leg_routes_spot_scope_position")
        .execute(&pool)
        .await?;
    let result =
        audit_cold_sources(&pool, scope(tenant), BUDGET, deadline(), &mut Keys::default()).await;
    ensure!(matches!(result,Err(ColdAuditRefusal::Catalog(label)) if label=="route_key"));
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn cold_spot_audit_cancelled_source_query_retires_original_backend(
    pool: PgPool,
) -> Result<()> {
    use robson_testkit::db_proxy::DbProxy;
    use sqlx::postgres::{PgPoolOptions, PgSslMode};
    let tenant = setup(&pool).await?;
    let config = pool.connect_options();
    let upstream = tokio::net::lookup_host((config.get_host(), config.get_port()))
        .await?
        .next()
        .ok_or_else(|| anyhow!("fixture endpoint"))?;
    ensure!(upstream.ip().is_loopback());
    let proxy = DbProxy::start(upstream).await?;
    let proxied = PgPoolOptions::new()
        .max_connections(1)
        .test_before_acquire(false)
        .connect_with(
            config
                .as_ref()
                .clone()
                .host("127.0.0.1")
                .port(proxy.addr().port())
                .ssl_mode(PgSslMode::Disable),
        )
        .await?;
    let (pid,birth):(i32,String)=sqlx::query_as(
        "SELECT pg_backend_pid(),backend_start::text FROM pg_stat_activity WHERE pid=pg_backend_pid()",
    ).fetch_one(&proxied).await?;
    let (observed, wait) = oneshot::channel();
    let (release, resume) = oneshot::channel();
    proxy.enable_exact_query_trace(32);
    let task_pool = proxied.clone();
    let job = tokio::spawn(async move {
        PAUSE_AFTER_INTENTS
            .scope(
                RefCell::new(Some((observed, resume))),
                audit_cold_sources(
                    &task_pool,
                    scope(tenant),
                    BUDGET,
                    deadline(),
                    &mut Keys::default(),
                ),
            )
            .await
    });
    tokio::time::timeout(Duration::from_secs(1), wait).await??;
    let mut lock = pool.begin().await?;
    sqlx::query("LOCK TABLE public.submit_attempts IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *lock)
        .await?;
    release.send(()).map_err(|_| anyhow!("owned snapshot stopped early"))?;
    tokio::time::timeout(Duration::from_millis(300),async {
        loop {
            let waiting:bool=sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE pid=$1 AND backend_start::text=$2
                  AND query LIKE '%FROM public.submit_attempts%' AND wait_event_type='Lock')",
            ).bind(pid).bind(&birth).fetch_one(&pool).await?;
            if waiting {break;}
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        Ok::<_,sqlx::Error>(())
    }).await??;
    job.abort();
    match job.await {
        Err(error) if error.is_cancelled() => {},
        other => anyhow::bail!("expected caller cancellation: {other:?}"),
    }
    tokio::time::timeout(Duration::from_secs(2),async {
        loop {
            let alive:bool=sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE pid=$1 AND backend_start::text=$2)",
            ).bind(pid).bind(&birth).fetch_one(&pool).await?;
            if !alive {break;}
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        Ok::<_,sqlx::Error>(())
    }).await??;
    let replacement: (i32,String) = sqlx::query_as(
        "SELECT pg_backend_pid(),backend_start::text FROM pg_stat_activity WHERE pid=pg_backend_pid()",
    ).fetch_one(&proxied).await?;
    ensure!(replacement != (pid, birth));
    ensure!(proxied.size() == 1 && proxy.stats().connections >= 2);
    ensure!(proxy.query_trace().iter().all(|q| !q.trim_start().starts_with("ROLLBACK")));
    lock.rollback().await?;
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn cold_spot_audit_seeded_external_publication_never_mixes_scopes(
    pool: PgPool,
) -> Result<()> {
    use proptest::{
        strategy::{Strategy, ValueTree},
        test_runner::{Config, RngSeed, TestRunner},
    };
    let tenant = setup(&pool).await?;
    let mut runner = TestRunner::new(Config {
        rng_seed: RngSeed::Fixed(0xbc2510b7),
        failure_persistence: None,
        ..Config::default()
    });
    let scenarios = (1_i64..=2, 0_u64..=3500);
    let mut previous = 0_i64;
    for _ in 0..7 {
        let (additional, delay) =
            scenarios.new_tree(&mut runner).expect("bounded publication schedule").current();
        let (observed, wait) = oneshot::channel();
        let (release, resume) = oneshot::channel();
        let mut keys = Keys::default();
        let old = PAUSE_AFTER_INTENTS.scope(
            RefCell::new(Some((observed, resume))),
            audit_cold_sources(&pool, scope(tenant), BUDGET, deadline(), &mut keys),
        );
        let next = async {
            wait.await?;
            tokio::time::sleep(Duration::from_micros(delay)).await;
            seed(&pool, tenant, previous + 1, previous + additional).await?;
            release.send(()).map_err(|_| anyhow!("old audit stopped"))?;
            Ok::<_, anyhow::Error>(())
        };
        let (receipt, published) = tokio::join!(old, next);
        published?;
        let observed = receipt.map_err(|error| anyhow!("generated audit: {error:?}"))?;
        ensure!(observed.tenant_id == tenant && observed.account_scope == SCOPE);
        ensure!(observed.families.iter().all(|family| family.rows == previous as u64));
        expected(&keys, previous as u64)?;
        previous += additional;
    }
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn cold_spot_audit_typed_deadline_on_running_snapshot(pool: PgPool) -> Result<()> {
    let tenant = setup(&pool).await?;
    let (observed, wait) = oneshot::channel();
    let (_keep_blocked, resume) = oneshot::channel();
    let mut keys = Keys::default();
    let start = Instant::now();
    let pending = PAUSE_AFTER_INTENTS.scope(
        RefCell::new(Some((observed, resume))),
        audit_cold_sources(
            &pool,
            scope(tenant),
            BUDGET,
            start + Duration::from_millis(250),
            &mut keys,
        ),
    );
    let (outcome, reached) = tokio::join!(pending, wait);
    reached?;
    ensure!(matches!(outcome, Err(ColdAuditRefusal::Deadline(Stage::Page(Family::Intents)))));
    ensure!(Instant::now().saturating_duration_since(start) >= Duration::from_millis(250));
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn cold_spot_audit_refuses_stale_and_missing_current_admission(pool: PgPool) -> Result<()> {
    let tenant = setup(&pool).await?;
    seed(&pool, tenant, 1, 1).await?;
    let position = Uuid::from_u128(101);
    let old = Uuid::now_v7();
    let next = Uuid::now_v7();
    let mut tx = pool.begin().await?;
    sqlx::query("UPDATE accounting_state SET version=version+1 WHERE tenant_id=$1 AND venue_id='binance:spot'")
        .bind(tenant).execute(&mut *tx).await?;
    sqlx::query(
        "SELECT balance_leg_admit($1,'binance:spot',$2,'BTCUSDT',$3,$4,'2026-11-01',
            10,10,2,10,0.001,'base','fixture',now(),'fixture',1,1,100,0.1,10,90,now(),false)",
    )
    .bind(tenant)
    .bind(SCOPE)
    .bind(position)
    .bind(old)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    let matched =
        audit_cold_sources(&pool, scope(tenant), BUDGET, deadline(), &mut Keys::default()).await;
    ensure!(matched.is_ok(), "{matched:?}");

    let mut changed = pool.begin().await?;
    sqlx::query(
        "UPDATE balance_leg_admissions SET superseded_at=now(),superseded_by=$2 WHERE entry_intent_id=$1",
    ).bind(old).bind(next).execute(&mut *changed).await?;
    sqlx::query(
        "INSERT INTO balance_leg_admissions
         SELECT (jsonb_populate_record(NULL::public.balance_leg_admissions,
           to_jsonb(a)||jsonb_build_object('entry_intent_id',$2::uuid,
                                      'superseded_at',NULL,'superseded_by',NULL))).*
         FROM public.balance_leg_admissions a WHERE a.entry_intent_id=$1",
    )
    .bind(old)
    .bind(next)
    .execute(&mut *changed)
    .await?;
    changed.commit().await?;
    let stale =
        audit_cold_sources(&pool, scope(tenant), BUDGET, deadline(), &mut Keys::default()).await;
    ensure!(matches!(stale,Err(ColdAuditRefusal::Row{family:Family::Routes,
        reason:RowDefect::Route(RouteDefect::StaleLegPointer),key:SourceKey::Route(p),..}) if p==position));

    // The native FK still validates its historical row. A malformed
    // supersession chain leaves no independently discoverable current row.
    sqlx::query(
        "UPDATE balance_leg_admissions SET superseded_at=now(),superseded_by=$2 WHERE entry_intent_id=$1",
    ).bind(next).bind(old).execute(&pool).await?;
    let missing =
        audit_cold_sources(&pool, scope(tenant), BUDGET, deadline(), &mut Keys::default()).await;
    ensure!(matches!(missing,Err(ColdAuditRefusal::Row{family:Family::Routes,
        reason:RowDefect::Route(RouteDefect::LegWithoutAdmission),key:SourceKey::Route(p),..}) if p==position));
    Ok(())
}
