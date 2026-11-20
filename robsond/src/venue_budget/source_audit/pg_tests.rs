//! Real schema/reader consistency, NOT J2b production writer/fencing proof.
use std::time::Duration;

use anyhow::{anyhow, ensure};
use chrono::Utc;
use robson_exec::venue_clock::NativeClock;
use sqlx::Row;

use super::*;

async fn initialize(pool: &PgPool, setup: &Trace) -> anyhow::Result<()> {
    // Existing sealed seven-command EMPTY macro fixture only. Its completed
    // marker must never suppress an actual later normative-source read.
    let clock = NativeClock::initialize()?;
    super::super::fixture::prepare(pool, &clock, Instant::now() + Duration::from_secs(2), setup)
        .await
        .map_err(|e| anyhow!("isolated initialization: {e:?}"))?;
    ensure!(setup.issued() == 7);
    Ok(())
}

async fn seed(pool: &PgPool, rows: &[(u128, &str, &str)], setup: &Trace) -> anyhow::Result<Uuid> {
    ensure!(rows.len() <= 2001, "finite fixture, not an operating limit");
    let tenant = Uuid::now_v7();
    let at = Utc::now();
    // Same descriptive identity fields as IntentRow/AttemptRow; these are
    // actual constrained rows, not a claimed governed order or J2b receipt.
    for chunk in rows.chunks(1000) {
        let ids: Vec<_> = chunk.iter().map(|r| Uuid::from_u128(r.0)).collect();
        let intents: Vec<_> = chunk.iter().map(|r| Uuid::from_u128(100_000 + r.0)).collect();
        let venues: Vec<_> = chunk.iter().map(|r| r.1).collect();
        let clients: Vec<_> = chunk.iter().map(|r| format!("source-page-{}", r.0)).collect();
        let kinds: Vec<_> = chunk.iter().map(|r| r.2).collect();
        setup.issue();
        let written = sqlx::query("INSERT INTO order_intents(intent_id,tenant_id,venue_id,account_scope,client_order_id,kind,params,created_at) SELECT id,$2,venue,'source-page',client,kind,'{}'::jsonb,$6 FROM unnest($1::uuid[],$3::text[],$4::text[],$5::text[]) AS x(id,venue,client,kind)")
            .bind(&intents).bind(tenant).bind(&venues).bind(&clients).bind(&kinds).bind(at)
            .execute(pool).await?.rows_affected();
        ensure!(written == u64::try_from(chunk.len())?);
        setup.issue();
        let written = sqlx::query("INSERT INTO submit_attempts(attempt_id,venue_id,account_scope,client_order_id,state,created_at) SELECT id,venue,'source-page',client,'started',$4 FROM unnest($1::uuid[],$2::text[],$3::text[]) AS x(id,venue,client)")
            .bind(&ids).bind(&venues).bind(&clients).bind(at).execute(pool).await?.rows_affected();
        ensure!(written == u64::try_from(chunk.len())?);
    }
    Ok(tenant)
}

async fn page(
    pool: &PgPool,
    cursor: Option<Uuid>,
    high: Uuid,
    trace: &Trace,
) -> anyhow::Result<Page> {
    observe(pool, cursor, high, Instant::now() + Duration::from_secs(2), trace)
        .await
        .map_err(|e| anyhow!("source page: {e:?}"))
}

async fn finish(
    pool: PgPool,
    primary: Result<anyhow::Result<()>, tokio::time::error::Elapsed>,
) -> anyhow::Result<()> {
    let cleanup = tokio::time::timeout(Duration::from_secs(3), pool.close()).await;
    eprintln!("SOURCE_PAGE_FINAL primary={primary:?} cleanup={cleanup:?}");
    match (primary, cleanup) {
        (Ok(Ok(())), Ok(())) => Ok(()),
        (primary, cleanup) => Err(anyhow!("primary={primary:?}; pool_cleanup={cleanup:?}")),
    }
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn global_pages_bound_before_venue_filter_and_include_nil(
    pool: PgPool,
) -> anyhow::Result<()> {
    let primary = tokio::time::timeout(Duration::from_secs(20), async {
        let setup = Trace::default();
        initialize(&pool, &setup).await?;
        let source: Vec<_> = (0..2001u128)
            .map(|n| {
                (
                    n,
                    if n == 1500 {
                        "binance:spot"
                    } else {
                        "binance:usdm"
                    },
                    "exit",
                )
            })
            .collect();
        seed(&pool, &source, &setup).await?;
        ensure!(setup.issued() == 13, "7 fixture + 3 bounded pairs of source writes");
        let reads = Trace::default();
        let high = Uuid::from_u128(2000);
        let first = page(&pool, None, high, &reads).await?;
        let first_ids: Vec<_> = (0..1000u128).map(Uuid::from_u128).collect();
        ensure!(first.examined == first_ids, "first GLOBAL page includes real nil UUID");
        ensure!(first.spot.is_empty() && first.cursor() == Some(Uuid::from_u128(999)));
        let second = page(&pool, first.cursor(), high, &reads).await?;
        let second_ids: Vec<_> = (1000..2000u128).map(Uuid::from_u128).collect();
        ensure!(second.examined == second_ids, "nonspot rows still consume page cardinality");
        ensure!(second.spot == [Uuid::from_u128(1500)]);
        ensure!(second.missing_clock == second.spot && second.missing_charge == second.spot);
        let last = page(&pool, second.cursor(), high, &reads).await?;
        ensure!(last.examined == [high] && last.spot.is_empty());
        ensure!(reads.issued() == 3, "three actual one-SELECT page operations");
        Ok(())
    })
    .await;
    finish(pool, primary).await
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn actual_clock_projection_preserves_identity_and_legacy_null(
    pool: PgPool,
) -> anyhow::Result<()> {
    let primary = tokio::time::timeout(Duration::from_secs(20), async {
        let setup = Trace::default();
        initialize(&pool, &setup).await?;
        let tenant = seed(&pool, &[(3, "binance:spot", "cancel"), (9, "binance:spot", "exit")], &setup).await?;
        // Actual migration36 FK parent, not readiness/cutover certification.
        // version and created_at retain their existing schema defaults.
        setup.issue();
        let parent = sqlx::query("INSERT INTO accounting_state(tenant_id,venue_id,budget_model) VALUES($1,'binance:spot','net_from_start_non_expanding_v1')")
            .bind(tenant).execute(&pool).await?.rows_affected();
        ensure!(parent == 1);
        let ids = [Uuid::from_u128(3), Uuid::from_u128(9)];
        setup.issue();
        let inserted = sqlx::query("INSERT INTO spot_intent_clock(attempt_id,intent_id,tenant_id,venue_id,attempt_created_at) SELECT a.attempt_id,i.intent_id,i.tenant_id,a.venue_id,a.created_at FROM submit_attempts a JOIN order_intents i ON (i.venue_id,i.account_scope,i.client_order_id)=(a.venue_id,a.account_scope,a.client_order_id) WHERE a.attempt_id=ANY($1::uuid[])")
            .bind(ids.as_slice()).execute(&pool).await?.rows_affected();
        ensure!(inserted == 2 && setup.issued() == 11, "7 fixture + 2 source + accounting parent + actual clock/trigger");
        let reads = Trace::default();
        let actual = page(&pool, None, ids[1], &reads).await?;
        ensure!(actual.examined == ids && actual.spot == ids);
        ensure!(actual.missing_intent.is_empty() && actual.missing_clock.is_empty()
            && actual.missing_charge.is_empty() && actual.mismatched.is_empty());
        ensure!(actual.cancels == [ids[0]] && actual.legacy_null == ids,
            "cancel identity and legacy NULL survive without manufactured time evidence");
        // One separately counted observer, independent known fixture values.
        let observer = sqlx::query("SELECT ARRAY(SELECT daily_units FROM budget_daily_charges ORDER BY attempt_id) AS units, (SELECT count(*) FROM budget_daily_charges)=2 AND NOT EXISTS(SELECT 1 FROM budget_daily_charges WHERE budget_clock_domain IS DISTINCT FROM NULL::budget_clock_domain_v38 OR budget_created_tick IS NOT NULL) AS actual_null")
            .fetch_one(&pool).await?;
        ensure!(observer.try_get::<Vec<i16>, _>("units")? == [0, 1]);
        ensure!(observer.try_get::<bool, _>("actual_null")?);
        ensure!(reads.issued() == 1);
        Ok(())
    }).await;
    finish(pool, primary).await
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn initialized_marker_cannot_hide_unclocked_source(pool: PgPool) -> anyhow::Result<()> {
    let primary = tokio::time::timeout(Duration::from_secs(20), async {
        let setup = Trace::default();
        initialize(&pool, &setup).await?;
        // Deliberately source-only/unflagged DATA fixture. No live sender,
        // restore operation, constraint disabling, DELETE or fabricated clock.
        seed(&pool, &[(17, "binance:spot", "exit")], &setup).await?;
        ensure!(setup.issued() == 9);
        let observer = sqlx::query("SELECT s.initialization_state='complete' AND a.status='complete' AS marker_complete, (SELECT count(*) FROM submit_attempts) AS sources, (SELECT count(*) FROM spot_intent_clock) AS clocks, (SELECT count(*) FROM budget_daily_charges) AS charges FROM budget_venue_state s JOIN budget_source_audits a ON a.audit_id=s.audit_id WHERE s.venue_id='binance:spot'")
            .fetch_one(&pool).await?;
        ensure!(observer.try_get::<bool, _>("marker_complete")?);
        ensure!(observer.try_get::<i64, _>("sources")? == 1);
        ensure!(observer.try_get::<i64, _>("clocks")? == 0 && observer.try_get::<i64, _>("charges")? == 0);
        let reads = Trace::default();
        let id = Uuid::from_u128(17);
        let actual = page(&pool, None, id, &reads).await?;
        ensure!(actual.examined == [id] && actual.spot == [id], "normative source is not derived rows/marker");
        ensure!(actual.missing_intent.is_empty());
        ensure!(actual.missing_clock == [id] && actual.missing_charge == [id],
            "missing history stays visible even beside a complete marker");
        ensure!(reads.issued() == 1);
        ensure!(matches!(super::super::bootstrap::production_input(), Err(BudgetError::PrerequisiteRefusal)));
        Ok(())
    }).await;
    finish(pool, primary).await
}
