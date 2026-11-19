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

#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum Boundary {
    BeforeAcquire,
    AfterIntents,
    AfterAttempts,
    AfterOrders,
    AfterRoutes,
}

type Pause = (Boundary, oneshot::Sender<()>, oneshot::Receiver<()>);

tokio::task_local! {
    static SNAPSHOT_PAUSE: RefCell<Option<Pause>>;
}

pub(super) async fn pause_at(current: Boundary) {
    let pause = SNAPSHOT_PAUSE.try_with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.as_ref().is_some_and(|(boundary, _, _)| *boundary == current) {
            slot.take()
        } else {
            None
        }
    });
    if let Ok(Some((_, reached, resume))) = pause {
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
    let old = SNAPSHOT_PAUSE.scope(
        RefCell::new(Some((Boundary::AfterIntents, observed, resume))),
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
        SNAPSHOT_PAUSE
            .scope(
                RefCell::new(Some((Boundary::AfterIntents, observed, resume))),
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
    let order = (0_usize..5).new_tree(&mut runner).expect("bounded order").current();
    let mut boundaries = [
        Boundary::BeforeAcquire,
        Boundary::AfterIntents,
        Boundary::AfterAttempts,
        Boundary::AfterOrders,
        Boundary::AfterRoutes,
    ];
    boundaries.rotate_left(order);
    let mut previous = 0_i64;
    for boundary in boundaries {
        let (additional, delay) =
            scenarios.new_tree(&mut runner).expect("bounded publication schedule").current();
        let (observed, wait) = oneshot::channel();
        let (release, resume) = oneshot::channel();
        let mut keys = Keys::default();
        let old = SNAPSHOT_PAUSE.scope(
            RefCell::new(Some((boundary, observed, resume))),
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
        let expected_count = previous
            + if boundary == Boundary::BeforeAcquire {
                additional
            } else {
                0
            };
        ensure!(observed.families.iter().all(|family| family.rows == expected_count as u64));
        expected(&keys, expected_count as u64)?;
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
    let pending = SNAPSHOT_PAUSE.scope(
        RefCell::new(Some((Boundary::AfterIntents, observed, resume))),
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
// Append these helpers beneath the isolated cold_audit_pg_tests.rs fixtures.
// Use its setup(), SCOPE, Result, ensure!, PgPool and Uuid.

const R2_UID: &str = "local-fixture";
const R2_DECOY_SCOPE: &str = "r2-other-spot-account";
const R2_DECOY_UID: &str = "r2-other-spot-uid";
const R2_SYMBOL: &str = "BTCUSDT";

type R2PriorBinding = (Uuid, Uuid, Uuid, i64, bool);

#[derive(Clone, Debug, Eq, PartialEq)]
struct R2AuditAnchor {
    tenant: Uuid,
    current_epoch: Uuid,
    version: i64,
    // generation, process epoch, source audit, accounting version, hint marker
    prior: R2PriorBinding,
}

async fn r2_seed_decoy_account(pool: &PgPool, tenant: Uuid) -> Result<()> {
    sqlx::query(
        "INSERT INTO public.venue_accounts
          (venue_id,account_scope,tenant_id,uid,bound_at,cutover_id)
         VALUES ('binance:spot',$1,$2,$3,now(),$4)",
    )
    .bind(R2_DECOY_SCOPE)
    .bind(tenant)
    .bind(R2_DECOY_UID)
    .bind(Uuid::now_v7())
    .execute(pool)
    .await?;
    Ok(())
}

/// Call after setup() in a clean native fixture. The copied prior is already
/// stale at the exact current numeric accounting version.
async fn r2_seed_copied_anchor(pool: &PgPool, tenant: Uuid) -> Result<R2AuditAnchor> {
    r2_seed_decoy_account(pool, tenant).await?;
    let old = crate::process_epoch::start_epoch(pool, tenant, "r2-copied-prior").await?;
    let version: i64 = sqlx::query_scalar(
        "SELECT version FROM public.accounting_state WHERE tenant_id=$1 AND venue_id='binance:spot'",
    )
    .bind(tenant)
    .fetch_one(pool)
    .await?;
    let prior = (Uuid::now_v7(), old.epoch.epoch_id, Uuid::now_v7(), version, false);
    sqlx::query(
        "INSERT INTO public.spot_order_set_generation
          (tenant_id,venue_id,account_scope,generation_id,process_epoch,source_audit_id,
           accounting_version,position_hints_complete)
         VALUES ($1,'binance:spot',$2,$3,$4,$5,$6,$7)",
    )
    .bind(tenant)
    .bind(SCOPE)
    .bind(prior.0)
    .bind(prior.1)
    .bind(prior.2)
    .bind(prior.3)
    .bind(prior.4)
    .execute(pool)
    .await?;
    let current = crate::process_epoch::start_epoch(pool, tenant, "r2-before-audit").await?;
    ensure!(current.predecessor_epoch_id == Some(prior.1));
    let anchor = R2AuditAnchor {
        tenant,
        current_epoch: current.epoch.epoch_id,
        version,
        prior,
    };
    ensure!(anchor.current_epoch != anchor.prior.1);
    r2_assert_anchor_oracle(pool, &anchor).await?;
    Ok(anchor)
}

/// Call from a separate writer while the audit owner is held at AfterIntents.
/// The same-version drift includes a new epoch, new complete prior tuple and a
/// late armed-only route. The root coordinator must withhold install authority.
async fn r2_publish_after_anchor(
    pool: &PgPool,
    before: &R2AuditAnchor,
) -> Result<(R2AuditAnchor, Uuid)> {
    let current =
        crate::process_epoch::start_epoch(pool, before.tenant, "r2-paused-audit-race").await?;
    ensure!(current.predecessor_epoch_id == Some(before.current_epoch));
    let prior = (Uuid::now_v7(), before.current_epoch, Uuid::now_v7(), before.version, true);
    let position = Uuid::from_u128(0xc01d_4001);
    let mut tx = pool.begin().await?;
    let updated = sqlx::query(
        "UPDATE public.spot_order_set_generation
         SET generation_id=$3,process_epoch=$4,source_audit_id=$5,position_hints_complete=$6
         WHERE (tenant_id,venue_id,account_scope,generation_id,process_epoch,source_audit_id,
                accounting_version,position_hints_complete)
             = ($1,'binance:spot',$2,$7,$8,$9,$10,$11)",
    )
    .bind(before.tenant)
    .bind(SCOPE)
    .bind(prior.0)
    .bind(prior.1)
    .bind(prior.2)
    .bind(prior.4)
    .bind(before.prior.0)
    .bind(before.prior.1)
    .bind(before.prior.2)
    .bind(before.prior.3)
    .bind(before.prior.4)
    .execute(&mut *tx)
    .await?;
    ensure!(updated.rows_affected() == 1);
    sqlx::query(
        "INSERT INTO public.leg_routes(position_id,tenant_id,venue_id,account_scope,routed_at)
         VALUES ($1,$2,'binance:spot',$3,now())",
    )
    .bind(position)
    .bind(before.tenant)
    .bind(SCOPE)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    let after = R2AuditAnchor {
        tenant: before.tenant,
        current_epoch: current.epoch.epoch_id,
        version: before.version,
        prior,
    };
    r2_assert_anchor_oracle(pool, &after).await?;
    Ok((after, position))
}

async fn r2_assert_anchor_oracle(pool: &PgPool, expected: &R2AuditAnchor) -> Result<()> {
    let accounts: Vec<(String, String, Uuid)> = sqlx::query_as(
        "SELECT account_scope,uid,tenant_id FROM public.venue_accounts
         WHERE venue_id='binance:spot' AND account_scope IN ($1,$2) ORDER BY account_scope",
    )
    .bind(SCOPE)
    .bind(R2_DECOY_SCOPE)
    .fetch_all(pool)
    .await?;
    ensure!(accounts.len() == 2);
    ensure!(accounts.contains(&(SCOPE.into(), R2_UID.into(), expected.tenant)));
    ensure!(accounts.contains(&(R2_DECOY_SCOPE.into(), R2_DECOY_UID.into(), expected.tenant)));
    let (version, epoch): (i64, Uuid) = sqlx::query_as(
        "SELECT a.version,e.epoch_id FROM public.accounting_state a
         CROSS JOIN LATERAL (
             SELECT epoch_id,clean_stop_at FROM public.process_epochs
             WHERE tenant_id=a.tenant_id ORDER BY epoch_seq DESC LIMIT 1
         ) e WHERE a.tenant_id=$1 AND a.venue_id='binance:spot' AND e.clean_stop_at IS NULL",
    )
    .bind(expected.tenant)
    .fetch_one(pool)
    .await?;
    ensure!((version, epoch) == (expected.version, expected.current_epoch));
    let prior: R2PriorBinding = sqlx::query_as(
        "SELECT generation_id,process_epoch,source_audit_id,accounting_version,
                position_hints_complete FROM public.spot_order_set_generation
         WHERE (tenant_id,venue_id,account_scope)=($1,'binance:spot',$2)",
    )
    .bind(expected.tenant)
    .bind(SCOPE)
    .fetch_one(pool)
    .await?;
    ensure!(prior == expected.prior);
    Ok(())
}

#[derive(Clone, Copy, Debug)]
enum R2NativeEdge {
    NamedForeign,
    ReverseForeign,
    NamedMissing,
    ExactPair,
}

#[derive(Debug)]
struct R2EdgeWitness {
    intent: Uuid,
    client: &'static str,
    exchange: &'static str,
    position: Uuid,
    scoped_intents: i64,
    scoped_orders: i64,
    scoped_routes: i64,
    terminal: bool,
}

/// One variant per isolated DB after setup() and r2_seed_decoy_account().
/// Both accounts share the tenant and venue, isolating account-scope checking.
/// M36's actual leg and native order constraints stay active for every case.
async fn r2_seed_native_edge(
    pool: &PgPool,
    tenant: Uuid,
    edge: R2NativeEdge,
) -> Result<R2EdgeWitness> {
    let (salt, client, exchange, j_scope, o_scope, original_position) = match edge {
        R2NativeEdge::NamedForeign => {
            (1, "r2-named-foreign", "912301", Some(R2_DECOY_SCOPE), SCOPE, true)
        },
        R2NativeEdge::ReverseForeign => {
            (2, "r2-reverse-foreign", "912302", Some(SCOPE), R2_DECOY_SCOPE, false)
        },
        R2NativeEdge::NamedMissing => (3, "r2-named-missing", "912303", None, SCOPE, false),
        R2NativeEdge::ExactPair => (4, "r2-exact-pair", "912304", Some(SCOPE), SCOPE, true),
    };
    let intent = Uuid::from_u128(0xc01d_4100 + salt);
    let position = Uuid::from_u128(0xc01d_4200 + salt);
    let admission = Uuid::from_u128(0xc01d_4300 + salt);
    // The absent and exact J controls share a qualified native terminal
    // shape. Only the exact present J can supply a terminal group mask.
    let terminal = !matches!(edge, R2NativeEdge::ReverseForeign);

    let mut tx = pool.begin().await?;
    let _: i64 = sqlx::query_scalar(
        "UPDATE public.accounting_state SET version=version+1
         WHERE tenant_id=$1 AND venue_id='binance:spot' RETURNING version",
    )
    .bind(tenant)
    .fetch_one(&mut *tx)
    .await?;
    if let Some(account) = j_scope {
        sqlx::query(
            "INSERT INTO public.order_intents
              (intent_id,tenant_id,venue_id,account_scope,client_order_id,position_id,kind,params,created_at)
             VALUES ($1,$2,'binance:spot',$3,$4,$5,'exit','{}',now())",
        )
        .bind(intent)
        .bind(tenant)
        .bind(account)
        .bind(client)
        .bind(original_position.then_some(position))
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query(
        "INSERT INTO public.leg_routes(position_id,tenant_id,venue_id,account_scope,routed_at)
         VALUES ($1,$2,'binance:spot',$3,now())",
    )
    .bind(position)
    .bind(tenant)
    .bind(o_scope)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "SELECT public.balance_leg_admit($1,'binance:spot',$2,$3,$4,$5,'2026-11-01',
         10,10,2,10,0.001,'base','fixture',now(),'r2-edge',1,1,100,0.1,10,90,now(),false)",
    )
    .bind(tenant)
    .bind(o_scope)
    .bind(R2_SYMBOL)
    .bind(position)
    .bind(admission)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO public.venue_orders
          (tenant_id,venue_id,account_scope,symbol,exchange_order_id,client_order_id,
           last_client_order_id,owner,intent_id,position_id,kind,side,order_type,orig_qty,
           status,phase,last_checked_at,live_seen_at,terminal_seen_at)
         VALUES ($1,'binance:spot',$2,$3,$4,$5,$5,'leg',$6,$7,'sell','SELL','LIMIT',1,
                 CASE WHEN $8 THEN 'CANCELED' ELSE 'NEW' END,
                 CASE WHEN $8 THEN 'terminal' ELSE 'live' END,now(),now(),
                 CASE WHEN $8 THEN now() ELSE NULL END)",
    )
    .bind(tenant)
    .bind(o_scope)
    .bind(R2_SYMBOL)
    .bind(exchange)
    .bind(client)
    .bind(intent)
    .bind(position)
    .bind(terminal)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    let (found_j, found_o, found_terminal): (Option<String>, String, bool) = sqlx::query_as(
        "SELECT (SELECT account_scope FROM public.order_intents WHERE intent_id=$1),
                account_scope, terminal FROM public.venue_orders WHERE intent_id=$1",
    )
    .bind(intent)
    .fetch_one(pool)
    .await?;
    ensure!((found_j.as_deref(), found_o.as_str(), found_terminal) == (j_scope, o_scope, terminal));

    let (scoped_intents, scoped_orders, scoped_routes): (i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM public.order_intents WHERE venue_id='binance:spot' AND account_scope=$1),
                (SELECT count(*) FROM public.venue_orders WHERE venue_id='binance:spot' AND account_scope=$1),
                (SELECT count(*) FROM public.leg_routes WHERE venue_id='binance:spot' AND account_scope=$1)",
    )
    .bind(SCOPE)
    .fetch_one(pool)
    .await?;
    let local_order = i64::from(o_scope == SCOPE);
    ensure!(
        (scoped_intents, scoped_orders, scoped_routes)
            == (i64::from(j_scope == Some(SCOPE)), local_order, local_order)
    );
    Ok(R2EdgeWitness {
        intent,
        client,
        exchange,
        position,
        scoped_intents,
        scoped_orders,
        scoped_routes,
        terminal,
    })
}

use super::super::cold_owner::{
    capture, AnchorDefect, OwnerRequest, PriorGeneration, RestoreIdentity,
};
use super::super::cold_source::Edge;

fn r2_request<'a>(tenant: Uuid, epoch: Uuid, version: i64, uid: &'a str) -> OwnerRequest<'a> {
    OwnerRequest {
        scope: scope(tenant),
        expected_uid: uid,
        epoch,
        accounting_version: version,
        restore: RestoreIdentity {
            operation: Uuid::now_v7(),
            target: Uuid::now_v7(),
        },
    }
}

fn r2_prior(p: R2PriorBinding) -> PriorGeneration {
    PriorGeneration {
        generation_id: p.0,
        process_epoch: Some(p.1),
        source_audit_id: Some(p.2),
        accounting_version: Some(p.3),
        position_hints_complete: p.4,
    }
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn cold_generation_owner_anchor_stays_with_the_terminal_snapshot(pool: PgPool) -> Result<()> {
    let tenant = setup(&pool).await?;
    let before = r2_seed_copied_anchor(&pool, tenant).await?;
    let request = r2_request(tenant, before.current_epoch, before.version, R2_UID);
    let wrong = OwnerRequest { expected_uid: R2_DECOY_UID, ..request };
    ensure!(matches!(
        capture(&pool, &wrong, deadline()).await,
        Err(ColdAuditRefusal::AccountUid)
    ));
    let (seen, observed) = oneshot::channel();
    let (released, resume) = oneshot::channel();
    let cold = SNAPSHOT_PAUSE.scope(
        RefCell::new(Some((Boundary::AfterIntents, seen, resume))),
        capture(&pool, &request, deadline()),
    );
    let publication = async {
        observed.await?;
        let after = r2_publish_after_anchor(&pool, &before).await?;
        released.send(()).map_err(|_| anyhow!("old snapshot ended prematurely"))?;
        Ok::<_, anyhow::Error>(after)
    };
    let (cut, publication) = tokio::join!(cold, publication);
    let (after, late) = publication?;
    let cut = cut.map_err(|e| anyhow!("old snapshot: {e:?}"))?;
    ensure!(
        cut.anchor.tenant_id == tenant
            && cut.anchor.account_scope == SCOPE
            && cut.anchor.uid == R2_UID
    );
    ensure!(
        cut.anchor.process_epoch == before.current_epoch
            && cut.anchor.accounting_version == before.version
            && cut.anchor.prior == Some(r2_prior(before.prior))
            && cut.restore == request.restore
    );
    ensure!(cut.routes().is_empty() && cut.receipt.statements == 10);
    ensure!(matches!(
        capture(&pool, &request, deadline()).await,
        Err(ColdAuditRefusal::Anchor(AnchorDefect::Epoch))
    ));
    let later = r2_request(tenant, after.current_epoch, after.version, R2_UID);
    let current = capture(&pool, &later, deadline())
        .await
        .map_err(|e| anyhow!("new snapshot: {e:?}"))?;
    ensure!(
        current.anchor.prior == Some(r2_prior(after.prior))
            && current.anchor.accounting_version == cut.anchor.accounting_version
            && current.routes().iter().map(|r| r.position_id).collect::<Vec<_>>() == [late]
    );
    Ok(())
}

async fn run_r2_edge(pool: &PgPool, variant: R2NativeEdge) -> Result<()> {
    let tenant = setup(pool).await?;
    r2_seed_decoy_account(pool, tenant).await?;
    let seeded = r2_seed_native_edge(pool, tenant, variant).await?;
    let epoch = crate::process_epoch::start_epoch(pool, tenant, "named-reverse-edge")
        .await?
        .epoch
        .epoch_id;
    let version:i64=sqlx::query_scalar("SELECT version FROM public.accounting_state WHERE tenant_id=$1 AND venue_id='binance:spot'")
        .bind(tenant).fetch_one(pool).await?;
    let request = r2_request(tenant, epoch, version, R2_UID);
    let cut = capture(pool, &request, deadline()).await;
    match variant {
        R2NativeEdge::NamedForeign => ensure!(matches!(cut,
            Err(ColdAuditRefusal::Row {family:Family::Orders,reason:RowDefect::ForeignEdge,
                key:SourceKey::Order {symbol,exchange_order_id},..})
            if symbol==R2_SYMBOL && exchange_order_id==seeded.exchange)),
        R2NativeEdge::ReverseForeign => ensure!(matches!(cut,
            Err(ColdAuditRefusal::Row {family:Family::Intents,reason:RowDefect::ForeignEdge,
                key:SourceKey::ClientOrder(client),..}) if client==seeded.client)),
        R2NativeEdge::NamedMissing | R2NativeEdge::ExactPair => {
            let cut = cut.map_err(|e| anyhow!("native edge {variant:?}: {e:?}"))?;
            let paired = matches!(variant, R2NativeEdge::ExactPair);
            ensure!(
                cut.anchor.prior.is_none()
                    && cut.receipt.statements == 10
                    && cut.attempts().is_empty()
            );
            ensure!(
                cut.intents().len() as i64 == seeded.scoped_intents
                    && cut.routes().len() as i64 == seeded.scoped_routes
                    && cut.orders().len() as i64 == seeded.scoped_orders
            );
            let o = &cut.orders()[0];
            ensure!(
                o.exchange_order_id == seeded.exchange
                    && o.intent_id == Some(seeded.intent)
                    && o.position_id == Some(seeded.position)
                    && (o.phase == "terminal") == seeded.terminal
            );
            if paired {
                ensure!(
                    o.named_edge(tenant, SCOPE) == Edge::InScope
                        && cut.intents()[0].reverse_edge(tenant, SCOPE) == Edge::InScope
                );
            } else {
                ensure!(o.named_edge(tenant, SCOPE) == Edge::AbsentNamed);
            }
        },
    }
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn cold_generation_owner_refuses_cross_scope_named_j(pool: PgPool) -> Result<()> {
    run_r2_edge(&pool, R2NativeEdge::NamedForeign).await
}
#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn cold_generation_owner_refuses_cross_scope_reverse_order(pool: PgPool) -> Result<()> {
    run_r2_edge(&pool, R2NativeEdge::ReverseForeign).await
}
#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn cold_generation_owner_retains_native_order_with_original_j_absent(
    pool: PgPool,
) -> Result<()> {
    run_r2_edge(&pool, R2NativeEdge::NamedMissing).await
}
#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn cold_generation_owner_checks_both_halves_of_exact_native_pair(pool: PgPool) -> Result<()> {
    run_r2_edge(&pool, R2NativeEdge::ExactPair).await
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn cold_generation_owner_rejects_overwidth_raw_key_before_cursor(pool: PgPool) -> Result<()> {
    let tenant = setup(&pool).await?;
    let epoch = crate::process_epoch::start_epoch(&pool, tenant, "overwidth")
        .await?
        .epoch
        .epoch_id;
    sqlx::query(
        "INSERT INTO public.order_intents
        (tenant_id,venue_id,account_scope,intent_id,client_order_id,kind,params,created_at)
        VALUES ($1,'binance:spot',$2,$3,repeat('é',129),'exit','{}',now())",
    )
    .bind(tenant)
    .bind(SCOPE)
    .bind(Uuid::now_v7())
    .execute(&pool)
    .await?;
    let version:i64=sqlx::query_scalar("SELECT version FROM public.accounting_state WHERE tenant_id=$1 AND venue_id='binance:spot'")
        .bind(tenant).fetch_one(&pool).await?;
    let cut = capture(&pool, &r2_request(tenant, epoch, version, R2_UID), deadline()).await;
    ensure!(matches!(
        cut,
        Err(ColdAuditRefusal::Row {
            family: Family::Intents,
            reason: RowDefect::OverWidth,
            last_verified: None,
            ..
        })
    ));
    Ok(())
}

use super::super::cold_install::{
    install, ClosedWriterFence, FencePhase, FenceReceipt, FixtureFenceError, InstallRefusal,
    Stage as InstallStage,
};

/// Deterministically drive SQL failure cuts. Authentication and physical
/// session retirement have their own independently isolated fixture.
struct FixtureFence {
    expected: RestoreIdentity,
    owner: Uuid,
    refuse: Option<FencePhase>,
    seen: Vec<FencePhase>,
}
impl FixtureFence {
    fn matching(expected: RestoreIdentity) -> Self {
        Self {
            expected,
            owner: Uuid::now_v7(),
            refuse: None,
            seen: Vec::new(),
        }
    }
}
impl ClosedWriterFence for FixtureFence {
    async fn check(
        &mut self,
        restore: RestoreIdentity,
        phase: FencePhase,
        deadline: Instant,
    ) -> std::result::Result<FenceReceipt, FixtureFenceError> {
        self.seen.push(phase);
        if restore != self.expected || self.refuse == Some(phase) || Instant::now() >= deadline {
            return Err(FixtureFenceError);
        }
        Ok(FenceReceipt { restore, owner: self.owner })
    }
}

async fn r2_install_fixture(
    pool: &PgPool,
) -> Result<(OwnerRequest<'static>, R2PriorBinding, Uuid)> {
    let tenant = setup(pool).await?;
    let old = r2_seed_copied_anchor(pool, tenant).await?;
    let pair = r2_seed_native_edge(pool, tenant, R2NativeEdge::ExactPair).await?;
    let other = Uuid::now_v7();
    let inactive = Uuid::now_v7();
    let sources = [
        (Uuid::now_v7(), "exit", Some(pair.position), None, false),
        (Uuid::now_v7(), "protection", Some(other), None, false),
        (Uuid::now_v7(), "cancel", Some(pair.position), Some(pair.exchange), false),
        (inactive, "cancel", Some(pair.position), Some("912309"), true),
        (Uuid::now_v7(), "cancel", Some(pair.position), Some("912310"), false),
        (Uuid::now_v7(), "cancel", Some(pair.position), Some("00001"), false),
        (Uuid::now_v7(), "cancel", Some(other), Some("912311"), false),
        (Uuid::now_v7(), "cancel", Some(other), None, true),
    ];
    for (id, kind, position, target, not_sent) in sources {
        sqlx::query(
            "INSERT INTO public.order_intents
              (intent_id,tenant_id,venue_id,account_scope,client_order_id,position_id,kind,cancels_order_id,
               params,created_at,not_sent)
             VALUES($1,$2,'binance:spot',$3,$1::uuid::text,$4,$5,$6,'{}',now(),$7)",
        ).bind(id).bind(tenant).bind(SCOPE).bind(position).bind(kind).bind(target)
            .bind(not_sent).execute(pool).await?;
    }
    sqlx::query(
       "INSERT INTO public.venue_orders
        (tenant_id,venue_id,account_scope,symbol,exchange_order_id,client_order_id,last_client_order_id,
         owner,side,order_type,orig_qty,status,phase,last_checked_at,live_seen_at)
        VALUES ($1,'binance:spot',$2,'BTCUSDT','912312','r2-independent-native-sell',
          'r2-independent-native-sell','discovered','SELL','LIMIT',1,'NEW','live',now(),now())"
    ).bind(tenant).bind(SCOPE).execute(pool).await?;
    let version:i64=sqlx::query_scalar("SELECT version FROM public.accounting_state WHERE tenant_id=$1 AND venue_id='binance:spot'")
        .bind(tenant).fetch_one(pool).await?;
    Ok((r2_request(tenant, old.current_epoch, version, R2_UID), old.prior, inactive))
}

const R2_LEAVES: [&str; 10] = [
    "sell_intents",
    "sell_orders",
    "sell_legs",
    "sell_symbols",
    "sell_unbound",
    "cancel_members",
    "cancel_groups",
    "cancel_legs",
    "cancel_unbound",
    "cancel_ungroupable",
];

async fn r2_leaf_counts(pool: &PgPool, tenant: Uuid, generation: Option<Uuid>) -> Result<Vec<i64>> {
    let mut counts = Vec::with_capacity(R2_LEAVES.len());
    for leaf in R2_LEAVES {
        let sql=format!(
         "SELECT count(*) FROM public.spot_order_set_{leaf} WHERE tenant_id=$1
          AND venue_id='binance:spot' AND account_scope=$2 AND ($3::uuid IS NULL OR generation_id=$3)");
        counts.push(
            sqlx::query_scalar(&sql)
                .bind(tenant)
                .bind(SCOPE)
                .bind(generation)
                .fetch_one(pool)
                .await?,
        );
    }
    Ok(counts)
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn cold_generation_install_publishes_ten_complete_leaf_sets(pool: PgPool) -> Result<()> {
    let (request, old, inactive) = r2_install_fixture(&pool).await?;
    let cut = capture(&pool, &request, deadline())
        .await
        .map_err(|e| anyhow!("owner cut: {e:?}"))?;
    ensure!(cut.anchor.prior == Some(r2_prior(old)) && cut.receipt.statements == 11);
    let mut fence = FixtureFence::matching(request.restore);
    let installed = install(&pool, &cut, &mut fence, deadline())
        .await
        .map_err(|e| anyhow!("local install: {e:?}"))?;
    ensure!(
        fence.seen
            == [
                FencePhase::BeforeRead,
                FencePhase::BeforePublish,
                FencePhase::AfterReadback
            ]
    );
    let g = &installed.generation;
    ensure!(
        installed.restore == request.restore
            && installed.accounting_version == request.accounting_version
            && g.generation_id != old.0
            && g.source_audit_id != old.2
            && g.process_epoch == request.epoch
    );
    ensure!(
        r2_leaf_counts(&pool, g.tenant_id, Some(g.generation_id)).await?
            == [2, 1, 1, 1, 1, 6, 3, 1, 1, 1]
    );
    let marker:(bool,i64)=sqlx::query_as(
       "SELECT position_hints_complete,accounting_version FROM public.spot_order_set_generation WHERE generation_id=$1"
    ).bind(g.generation_id).fetch_one(&pool).await?;
    ensure!(marker == (true, request.accounting_version));
    let zero: (i64, i64, bool) = sqlx::query_as(
        "SELECT pending_count,conflict_count,terminal_mask FROM public.spot_order_set_cancel_groups
        WHERE generation_id=$1 AND target_id='912309'",
    )
    .bind(g.generation_id)
    .fetch_one(&pool)
    .await?;
    ensure!(zero == (0, 0, false));
    let exact:(Option<Uuid>,Option<Uuid>,bool,bool)=sqlx::query_as(
       "SELECT source_position,position_id,pending,conflict FROM public.spot_order_set_cancel_members
        WHERE generation_id=$1 AND intent_id=$2"
    ).bind(g.generation_id).bind(inactive).fetch_one(&pool).await?;
    ensure!(exact.0 == exact.1 && exact.0.is_some() && !exact.2 && !exact.3);
    let mask:bool=sqlx::query_scalar(
       "SELECT terminal_mask FROM public.spot_order_set_cancel_groups WHERE generation_id=$1 AND target_id='912304'"
    ).bind(g.generation_id).fetch_one(&pool).await?;
    ensure!(mask);
    ensure!(matches!(
        install(&pool, &cut, &mut fence, deadline()).await,
        Err(InstallRefusal::AnchorChanged)
    ));
    r2_reconstruction_reference(&pool, g).await?;
    r2_replay_inactive_cancel(&pool, g, inactive).await?;
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn cold_generation_install_blocked_before_publish_rolls_back_every_leaf(
    pool: PgPool,
) -> Result<()> {
    let (request, old, _) = r2_install_fixture(&pool).await?;
    let cut = capture(&pool, &request, deadline())
        .await
        .map_err(|e| anyhow!("owner cut: {e:?}"))?;
    let mut fence = FixtureFence::matching(request.restore);
    fence.refuse = Some(FencePhase::BeforePublish);
    ensure!(matches!(
        install(&pool, &cut, &mut fence, deadline()).await,
        Err(InstallRefusal::FixtureFence(InstallStage::Publish))
    ));
    let prior:bool=sqlx::query_scalar(
         "SELECT EXISTS(SELECT 1 FROM spot_order_set_generation WHERE generation_id=$1 AND NOT position_hints_complete)")
        .bind(old.0).fetch_one(&pool).await?;
    ensure!(prior && r2_leaf_counts(&pool, request.scope.tenant_id, None).await? == [0; 10]);
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn cold_generation_install_refuses_missing_physical_leaf_write(pool: PgPool) -> Result<()> {
    let (request, old, _) = r2_install_fixture(&pool).await?;
    let cut = capture(&pool, &request, deadline())
        .await
        .map_err(|e| anyhow!("owner cut: {e:?}"))?;
    sqlx::raw_sql("CREATE FUNCTION suppress_one_cancel_leaf() RETURNS trigger LANGUAGE plpgsql AS $$
        BEGIN RETURN NULL; END $$;
        CREATE TRIGGER suppress_one_cancel_leaf BEFORE INSERT ON public.spot_order_set_cancel_members
        FOR EACH ROW WHEN (NEW.source_position IS NOT NULL) EXECUTE FUNCTION suppress_one_cancel_leaf()")
        .execute(&pool).await?;
    let mut fence = FixtureFence::matching(request.restore);
    ensure!(matches!(
        install(&pool, &cut, &mut fence, deadline()).await,
        Err(InstallRefusal::Leaves(_))
    ));
    let prior:bool=sqlx::query_scalar(
         "SELECT EXISTS(SELECT 1 FROM spot_order_set_generation WHERE generation_id=$1 AND NOT position_hints_complete)")
        .bind(old.0).fetch_one(&pool).await?;
    ensure!(prior && r2_leaf_counts(&pool, request.scope.tenant_id, None).await? == [0; 10]);
    Ok(())
}
async fn r2_reconstruction_reference(
    pool: &PgPool,
    generation: &super::super::JournalGeneration,
) -> Result<()> {
    for order in ["members_first", "targets_first"] {
        let mut tx = pool.begin().await?;
        sqlx::raw_sql(include_str!("../../tests/spot_order_set_reference_pg/sell_shadow.sql"))
            .execute(&mut *tx)
            .await?;
        sqlx::raw_sql(include_str!("../../tests/spot_order_set_reference_pg/cancel_shadow.sql"))
            .execute(&mut *tx)
            .await?;
        sqlx::query("SELECT set_config('robson_test.replay_cancel_order',$1,true)")
            .bind(order)
            .execute(&mut *tx)
            .await?;
        sqlx::raw_sql(include_str!(
            "../../tests/spot_order_set_reference_pg/initialization_replay.sql"
        ))
        .execute(&mut *tx)
        .await?;

        let reference_id = Uuid::now_v7();
        let moved = sqlx::query(
            "UPDATE public.spot_order_set_generation SET generation_id=$4
            WHERE (tenant_id,venue_id,account_scope,generation_id)=($1,'binance:spot',$2,$3)",
        )
        .bind(generation.tenant_id)
        .bind(&generation.account_scope)
        .bind(generation.generation_id)
        .bind(reference_id)
        .execute(&mut *tx)
        .await?;
        ensure!(moved.rows_affected() == 1);
        let stored: Uuid =
            sqlx::query_scalar(include_str!("../../tests/venue_ledger_orders/oracle_seed.sql"))
                .bind(generation.tenant_id)
                .bind(&generation.account_scope)
                .bind(reference_id)
                .fetch_one(&mut *tx)
                .await?;
        ensure!(stored == reference_id);

        for leaf in R2_LEAVES {
            let normalize = match leaf {
                "sell_intents" | "sell_orders" => "AND (potential OR conflict)",
                "sell_legs" | "sell_symbols" | "sell_unbound" => {
                    "AND (potential_count<>0 OR conflict_count<>0)"
                },
                "cancel_groups" => "AND (pending_count<>0 OR conflict_count<>0 OR terminal_mask)",
                "cancel_legs" | "cancel_unbound" | "cancel_ungroupable" => {
                    "AND (active_count<>0 OR conflict_count<>0)"
                },
                "cancel_members" => "",
                _ => unreachable!("fixed independent ten-leaf witness"),
            };
            let sql=format!(
                "WITH actual AS MATERIALIZED (
                    SELECT to_jsonb(t)-'generation_id' AS row
                    FROM public.spot_order_set_{leaf} AS t
                    WHERE (tenant_id,venue_id,account_scope,generation_id)=($1,'binance:spot',$2,$3) {normalize}
                ), expected AS MATERIALIZED (
                    SELECT to_jsonb(t)-'generation_id' AS row
                    FROM public.spot_order_set_{leaf} AS t
                    WHERE (tenant_id,venue_id,account_scope,generation_id)=($1,'binance:spot',$2,$4) {normalize}
                )
                SELECT count(*) FROM (
                   (SELECT row FROM actual EXCEPT ALL SELECT row FROM expected)
                   UNION ALL
                   (SELECT row FROM expected EXCEPT ALL SELECT row FROM actual)
                ) differences"
            );
            let different: i64 = sqlx::query_scalar(&sql)
                .bind(generation.tenant_id)
                .bind(&generation.account_scope)
                .bind(generation.generation_id)
                .bind(reference_id)
                .fetch_one(&mut *tx)
                .await?;
            ensure!(different == 0, "{order} {leaf}: {different} differing scoped rows");
        }
        tx.rollback().await?;
    }
    Ok(())
}

async fn r2_replay_inactive_cancel(
    pool: &PgPool,
    g: &super::super::JournalGeneration,
    original: Uuid,
) -> Result<()> {
    use robson_exec::order_journal::{
        BalanceBatch, IntentKind, IntentRow, JournalBackend, JournalBatch, OutcomeRow,
    };
    let (position_id, created_at): (Option<Uuid>, chrono::DateTime<chrono::Utc>) = sqlx::query_as(
        "SELECT position_id,created_at FROM public.order_intents WHERE intent_id=$1",
    )
    .bind(original)
    .fetch_one(pool)
    .await?;
    let batch = JournalBatch {
        intent: IntentRow {
            intent_id: original,
            venue_id: "binance:spot".into(),
            account_scope: g.account_scope.clone(),
            client_order_id: original.to_string(),
            position_id,
            kind: IntentKind::Cancel,
            cancels_order_id: Some("912309".into()),
            params: serde_json::json!({}),
            created_at,
        },
        attempts: vec![],
        outcome: Some(OutcomeRow { not_sent: true, ..OutcomeRow::default() }),
        balance: Some(BalanceBatch {
            admission: None,
            handed_off: false,
            commit_bound: Duration::from_millis(500),
        }),
    };
    let store = crate::order_journal_store::PgJournalBackend::new(pool, g.tenant_id)
        .with_order_set_projection(g.clone());
    let before: (i64,i64,bool)=sqlx::query_as(
       "SELECT pending_count, conflict_count, terminal_mask FROM public.spot_order_set_cancel_groups
        WHERE generation_id=$1 AND target_id='912309'"
    ).bind(g.generation_id).fetch_one(pool).await?;
    store
        .commit(&batch)
        .await
        .map_err(|e| anyhow!("real grouped inactive replay: {e:?}"))?;
    let after: (i64,i64,bool)=sqlx::query_as(
       "SELECT pending_count, conflict_count, terminal_mask FROM public.spot_order_set_cancel_groups
        WHERE generation_id=$1 AND target_id='912309'"
    ).bind(g.generation_id).fetch_one(pool).await?;
    ensure!(before == after && after == (0, 0, false));
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn cold_generation_install_refuses_same_version_epoch_drift(pool: PgPool) -> Result<()> {
    let (request, old, _) = r2_install_fixture(&pool).await?;
    let cut = capture(&pool, &request, deadline())
        .await
        .map_err(|e| anyhow!("owner cut: {e:?}"))?;
    let epoch =
        crate::process_epoch::start_epoch(&pool, request.scope.tenant_id, "install-stale-source")
            .await?;
    ensure!(epoch.epoch.epoch_id != request.epoch);
    let mut fence = FixtureFence::matching(request.restore);
    ensure!(matches!(
        install(&pool, &cut, &mut fence, deadline()).await,
        Err(InstallRefusal::Anchor(ColdAuditRefusal::Anchor(AnchorDefect::Epoch)))
    ));
    let prior:bool=sqlx::query_scalar(
         "SELECT EXISTS(SELECT 1 FROM spot_order_set_generation WHERE generation_id=$1 AND NOT position_hints_complete)")
        .bind(old.0).fetch_one(&pool).await?;
    ensure!(prior && r2_leaf_counts(&pool, request.scope.tenant_id, None).await? == [0; 10]);
    Ok(())
}
#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn cold_generation_install_issues_sixteen_commands_and_fresh_readback(
    pool: PgPool,
) -> Result<()> {
    r2_install_wire(&pool, false).await
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn cold_generation_install_lost_ack_retires_and_withholds_handoff(
    pool: PgPool,
) -> Result<()> {
    r2_install_wire(&pool, true).await
}

async fn r2_install_wire(pool: &PgPool, lose_ack: bool) -> Result<()> {
    use robson_testkit::db_proxy::{DbProxy, ProxyMode};
    use sqlx::postgres::{PgPoolOptions, PgSslMode};
    let (request, old, _) = r2_install_fixture(pool).await?;
    let cut = capture(pool, &request, deadline())
        .await
        .map_err(|e| anyhow!("owner cut: {e:?}"))?;
    let opts = pool.connect_options();
    let upstream = tokio::net::lookup_host((opts.get_host(), opts.get_port()))
        .await?
        .next()
        .ok_or_else(|| anyhow!("local PG16 upstream"))?;
    ensure!(upstream.ip().is_loopback());
    let proxy = DbProxy::start(upstream).await?;
    let proxied = PgPoolOptions::new()
        .max_connections(1)
        .test_before_acquire(false)
        .connect_with(
            opts.as_ref()
                .clone()
                .host("127.0.0.1")
                .port(proxy.addr().port())
                .ssl_mode(PgSslMode::Disable),
        )
        .await?;
    let (pid, birth): (i32, String) = sqlx::query_as(
        "SELECT pid,backend_start::text FROM pg_stat_activity WHERE pid=pg_backend_pid()",
    )
    .fetch_one(&proxied)
    .await?;
    proxy.enable_exact_query_trace(24);
    let baseline = proxy.stats();
    if lose_ack {
        proxy.set_mode(ProxyMode::DropCommitAckOf("position_hints_complete = true"));
    }
    let mut fence = FixtureFence::matching(request.restore);
    let installed = install(&proxied, &cut, &mut fence, deadline()).await;
    let trace = proxy.query_trace();
    let after = proxy.stats();
    let expected = if lose_ack { 16 } else { 18 };
    ensure!(
        trace.len() == expected
            && proxy.query_trace_dropped() == 0
            && after.statements == baseline.statements + expected as u64,
        "native install wire: {trace:?}"
    );
    ensure!(
        trace[0] == "BEGIN"
            && trace[1].contains("set_config('lock_timeout','20ms',true)")
            && trace[2].contains("FOR UPDATE")
            && trace[3].contains("latest_epoch_live")
    );
    ensure!(trace[4..14]
        .iter()
        .zip(R2_LEAVES)
        .all(|(sql, table)| sql.contains(&format!("INSERT INTO public.spot_order_set_{table}"))));
    ensure!(trace[14].contains("position_hints_complete = true") && trace[15] == "COMMIT");
    if lose_ack {
        ensure!(matches!(installed, Err(InstallRefusal::CommitUncertain(_))));
        ensure!(
            after.commit_acks_dropped == baseline.commit_acks_dropped + 1
                && fence.seen == [FencePhase::BeforeRead, FencePhase::BeforePublish]
        );
        tokio::time::timeout(Duration::from_millis(500), async {
            loop {
                let original:bool=sqlx::query_scalar(
                    "SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE pid=$1 AND backend_start::text=$2)"
                ).bind(pid).bind(&birth).fetch_one(pool).await?;
                if !original { return Ok::<(),sqlx::Error>(()); }
                tokio::time::sleep(Duration::from_millis(3)).await;
            }
        }).await??;
        let (new, complete): (Uuid, bool) = sqlx::query_as(
            "SELECT generation_id,position_hints_complete FROM public.spot_order_set_generation
             WHERE (tenant_id,venue_id,account_scope)=($1,'binance:spot',$2)",
        )
        .bind(request.scope.tenant_id)
        .bind(request.scope.account_scope)
        .fetch_one(pool)
        .await?;
        ensure!(complete && new != old.0);
        ensure!(
            r2_leaf_counts(pool, request.scope.tenant_id, Some(new)).await?
                == [2, 1, 1, 1, 1, 6, 3, 1, 1, 1]
        );
    } else {
        installed.map_err(|e| anyhow!("local wire install: {e:?}"))?;
        ensure!(
            after.connections == baseline.connections + 1
                && trace[16].contains("latest_epoch_live")
                && trace[17].contains("jsonb_build_array")
                && fence.seen
                    == [
                        FencePhase::BeforeRead,
                        FencePhase::BeforePublish,
                        FencePhase::AfterReadback
                    ]
        );
    }
    proxied.close().await;
    eprintln!("Cold installer PG16 actual commands inside transaction=16 independent readback={} lost_ack={lose_ack}",expected-16);
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn cold_generation_install_refuses_changed_or_surplus_leaf_sets(pool: PgPool) -> Result<()> {
    let (request, old, _) = r2_install_fixture(&pool).await?;
    let cut = capture(&pool, &request, deadline())
        .await
        .map_err(|e| anyhow!("owner cut: {e:?}"))?;
    for (label, table, timing, body, compare_at_publish) in [
        (
            "fabricated original position",
            "cancel_members",
            "BEFORE",
            "NEW.source_position='99000000-0000-0000-0000-000000000001'; RETURN NEW;",
            false,
        ),
        (
            "missing native terminal mask",
            "cancel_groups",
            "BEFORE",
            "NEW.terminal_mask=false; RETURN NEW;",
            false,
        ),
        (
            "extra row inserted behind RETURNING",
            "cancel_groups",
            "AFTER",
            "IF NEW.target_id='912304' THEN
                INSERT INTO public.spot_order_set_cancel_groups
                    (tenant_id,venue_id,account_scope,generation_id,symbol,position_id,target_id,
                     pending_count,conflict_count,terminal_mask)
                  VALUES(NEW.tenant_id,NEW.venue_id,NEW.account_scope,NEW.generation_id,NEW.symbol,
                         NEW.position_id,'999',0,0,false);
             END IF; RETURN NEW;",
            true,
        ),
        (
            "older inserted leaf changed behind RETURNING",
            "cancel_groups",
            "AFTER",
            "UPDATE public.spot_order_set_sell_unbound SET potential_count=99
              WHERE (tenant_id,venue_id,account_scope,generation_id)
                =(NEW.tenant_id,NEW.venue_id,NEW.account_scope,NEW.generation_id);
             RETURN NEW;",
            true,
        ),
    ] {
        let trigger = format!(
            "CREATE FUNCTION cold_install_leaf_mutant() RETURNS trigger LANGUAGE plpgsql AS
             $$ BEGIN {body} END $$;
             CREATE TRIGGER cold_install_leaf_mutant {timing} INSERT ON public.spot_order_set_{table}
             FOR EACH ROW EXECUTE FUNCTION cold_install_leaf_mutant()"
        );
        sqlx::raw_sql(&trigger).execute(&pool).await?;
        let mut fence = FixtureFence::matching(request.restore);
        let refused = install(&pool, &cut, &mut fence, deadline()).await;
        if compare_at_publish {
            ensure!(matches!(refused, Err(InstallRefusal::Publish)), "{label}: {refused:?}");
        } else {
            ensure!(matches!(refused, Err(InstallRefusal::Leaves(_))), "{label}: {refused:?}");
        }
        let marker: bool = sqlx::query_scalar(
            "SELECT position_hints_complete FROM spot_order_set_generation WHERE generation_id=$1",
        )
        .bind(old.0)
        .fetch_one(&pool)
        .await?;
        ensure!(!marker && r2_leaf_counts(&pool, request.scope.tenant_id, None).await? == [0; 10]);
        sqlx::raw_sql(&format!(
            "DROP TRIGGER cold_install_leaf_mutant ON public.spot_order_set_{table};
              DROP FUNCTION cold_install_leaf_mutant()"
        ))
        .execute(&pool)
        .await?;
    }
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn cold_generation_owner_caps_full_pager_with_no_false_terminal(pool: PgPool) -> Result<()> {
    let tenant = setup(&pool).await?;
    let epoch = crate::process_epoch::start_epoch(&pool, tenant, "bounded-full-page")
        .await?
        .epoch
        .epoch_id;
    seed(&pool, tenant, 1, 63).await?;
    let version: i64 = sqlx::query_scalar(
        "SELECT version FROM accounting_state WHERE tenant_id=$1 AND venue_id='binance:spot'",
    )
    .bind(tenant)
    .fetch_one(&pool)
    .await?;
    let request = r2_request(tenant, epoch, version, R2_UID);
    let exact = capture(&pool, &request, deadline())
        .await
        .map_err(|e| anyhow!("63 eligible rows per family: {e:?}"))?;
    ensure!(
        exact.receipt.statements == 38
            && exact.receipt.families.iter().all(|f| f.rows == 63 && f.pages == 8)
    );
    seed(&pool, tenant, 64, 64).await?;
    let cut = capture(&pool, &request, deadline()).await;
    ensure!(matches!(
        cut,
        Err(ColdAuditRefusal::Sink { family: Family::Intents, .. })
            | Err(ColdAuditRefusal::PageBudget { family: Family::Intents, .. })
    ));
    Ok(())
}
#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn cold_generation_install_entry_uses_real_rebuilt_position_hints(
    pool: PgPool,
) -> Result<()> {
    use robson_exec::order_journal::{CommitFailure, JournalBackend};
    let (request, _, _) = r2_install_fixture(&pool).await?;
    let recoverable_hint = Uuid::now_v7();
    let old_cancel = Uuid::now_v7();
    sqlx::query(
      "INSERT INTO public.order_intents
       (intent_id,tenant_id,venue_id,account_scope,client_order_id,position_id,kind,params,created_at,not_sent)
       VALUES($1,$2,'binance:spot',$3,$1::uuid::text,$4,'cancel','{}',now(),true)"
    ).bind(old_cancel).bind(request.scope.tenant_id).bind(SCOPE).bind(recoverable_hint)
      .execute(&pool).await?;
    let cut = capture(&pool, &request, deadline())
        .await
        .map_err(|e| anyhow!("owner cut: {e:?}"))?;
    let mut fence = FixtureFence::matching(request.restore);
    let installed = install(&pool, &cut, &mut fence, deadline())
        .await
        .map_err(|e| anyhow!("install: {e:?}"))?;
    let g = installed.generation;
    let evidence:(Option<Uuid>,Option<Uuid>,bool,bool)=sqlx::query_as(
        "SELECT source_position,position_id,pending,conflict FROM public.spot_order_set_cancel_members
         WHERE (tenant_id,venue_id,account_scope,generation_id,intent_id)=($1,'binance:spot',$2,$3,$4)"
    ).bind(g.tenant_id).bind(SCOPE).bind(g.generation_id).bind(old_cancel).fetch_one(&pool).await?;
    ensure!(evidence == (Some(recoverable_hint), None, false, false));
    let writer = crate::order_journal_store::PgJournalBackend::new(&pool, g.tenant_id)
        .with_order_set_projection(g.clone());
    let fresh = r2_local_entry(&pool, &g, Uuid::now_v7()).await?;
    writer.commit(&fresh).await.map_err(|e| anyhow!("fresh native Entry: {e:?}"))?;
    let refused = r2_local_entry(&pool, &g, recoverable_hint).await?;
    ensure!(matches!(writer.commit(&refused).await, Err(CommitFailure::Known(_))));
    let observed:(i64,bool,bool,bool)=sqlx::query_as(
       "SELECT accounting_version,position_hints_complete,
            EXISTS(SELECT 1 FROM public.balance_legs WHERE admission_intent_id=$2 AND position_id=$3),
            EXISTS(SELECT 1 FROM public.order_intents WHERE intent_id=$4)
        FROM public.spot_order_set_generation WHERE generation_id=$1"
    ).bind(g.generation_id).bind(fresh.intent.intent_id).bind(fresh.intent.position_id)
     .bind(refused.intent.intent_id).fetch_one(&pool).await?;
    ensure!(observed == (request.accounting_version + 1, true, true, false), "{observed:?}");
    Ok(())
}

async fn r2_local_entry(
    pool: &PgPool,
    g: &super::super::JournalGeneration,
    position: Uuid,
) -> Result<robson_exec::order_journal::JournalBatch> {
    use chrono::{NaiveDate, Utc};
    use robson_exec::order_journal::{
        AdmissionRow, BalanceBatch, IntentKind, IntentRow, JournalBatch,
    };
    use rust_decimal::Decimal;
    use std::sync::Arc;
    let ledger = crate::venue_ledger_store::PgVenueLedger::new(pool, g.tenant_id);
    let venue = robson_exec::venue::VenueId::new("binance:spot");
    let id = Uuid::now_v7();
    ledger.insert_route(position, &venue, &g.account_scope, Utc::now()).await?;
    Ok(JournalBatch {
        intent: IntentRow {
            intent_id: id,
            venue_id: "binance:spot".into(),
            account_scope: g.account_scope.clone(),
            client_order_id: id.to_string(),
            position_id: Some(position),
            kind: IntentKind::Entry,
            cancels_order_id: None,
            params: serde_json::json!({}),
            created_at: Utc::now(),
        },
        attempts: vec![],
        outcome: None,
        balance: Some(BalanceBatch {
            admission: Some(Arc::new(AdmissionRow {
                position_id: position,
                symbol: R2_SYMBOL.to_owned(),
                decided_month: NaiveDate::from_ymd_opt(2026, 11, 1).unwrap(),
                taker_bps: Decimal::new(10, 0),
                gap_bps: Decimal::new(10, 0),
                notional_margin_bps: Decimal::new(2, 0),
                min_notional: Decimal::new(10, 0),
                lot_step: Decimal::new(1, 3),
                fee_asset_policy: "base".into(),
                envelope_source: "isolated-cold-entry".into(),
                envelope_as_of: Utc::now(),
                envelope_hash: "fixture".into(),
                planned_qty: Decimal::ONE,
                requested_qty: Decimal::ONE,
                reference_price: Decimal::new(100, 0),
                remainder_reserve: Decimal::new(1, 1),
                admission_charge: Decimal::new(10, 0),
                initial_trigger: Decimal::new(90, 0),
            })),
            handed_off: false,
            commit_bound: Duration::from_millis(500),
        }),
    })
}
