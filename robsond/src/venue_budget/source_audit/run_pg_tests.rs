//! Native running/failed audit controls on the isolated fixture database only.
//! Fixture ownership authorizes these exercises; it is not the C2 barrier.
use std::time::Duration;

use anyhow::{anyhow, ensure};
use chrono::Utc;
use robson_exec::venue_clock::NativeClock;

use super::{
    run::{pass, start, Discrepancy, Envelope, Pass, Run},
    *,
};

async fn initialize(pool: &PgPool) -> anyhow::Result<()> {
    let setup = Trace::default();
    let clock = NativeClock::initialize()?;
    super::super::fixture::prepare(pool, &clock, Instant::now() + Duration::from_secs(2), &setup)
        .await
        .map_err(|e| anyhow!("isolated initialization: {e:?}"))?;
    ensure!(setup.issued() == 7);
    Ok(())
}

/// Actual constrained intent/attempt rows only; no clock or charge rows.
async fn seed(pool: &PgPool, rows: &[(u128, &str)]) -> anyhow::Result<()> {
    let tenant = Uuid::now_v7();
    let at = Utc::now();
    for chunk in rows.chunks(1000) {
        let ids: Vec<_> = chunk.iter().map(|r| Uuid::from_u128(r.0)).collect();
        let intents: Vec<_> = chunk.iter().map(|r| Uuid::from_u128(100_000 + r.0)).collect();
        let venues: Vec<_> = chunk.iter().map(|r| r.1).collect();
        let clients: Vec<_> = chunk.iter().map(|r| format!("audit-run-{}", r.0)).collect();
        let written = sqlx::query("INSERT INTO order_intents(intent_id,tenant_id,venue_id,account_scope,client_order_id,kind,params,created_at) SELECT id,$2,venue,'audit-run',client,'exit','{}'::jsonb,$5 FROM unnest($1::uuid[],$3::text[],$4::text[]) AS x(id,venue,client)")
            .bind(&intents).bind(tenant).bind(&venues).bind(&clients).bind(at)
            .execute(pool).await?.rows_affected();
        ensure!(written == u64::try_from(chunk.len())?);
        let written = sqlx::query("INSERT INTO submit_attempts(attempt_id,venue_id,account_scope,client_order_id,state,created_at) SELECT id,venue,'audit-run',client,'started',$4 FROM unnest($1::uuid[],$2::text[],$3::text[]) AS x(id,venue,client)")
            .bind(&ids).bind(&venues).bind(&clients).bind(at).execute(pool).await?.rows_affected();
        ensure!(written == u64::try_from(chunk.len())?);
    }
    Ok(())
}

fn envelope() -> anyhow::Result<Envelope> {
    Envelope::new(Instant::now() + Duration::from_secs(20)).map_err(|e| anyhow!("{e:?}"))
}

async fn durable(pool: &PgPool, audit: Uuid) -> anyhow::Result<(i64, Option<Uuid>, i64, String)> {
    let row = sqlx::query("SELECT quiescence_generation,cursor,examined_count,status FROM budget_source_audits WHERE audit_id=$1")
        .bind(audit)
        .fetch_one(pool)
        .await?;
    Ok((row.try_get(0)?, row.try_get(1)?, row.try_get(2)?, row.try_get(3)?))
}

async fn venue(pool: &PgPool) -> anyhow::Result<(String, Option<Uuid>, i64)> {
    let row = sqlx::query("SELECT initialization_state,audit_id,quiescence_generation FROM budget_venue_state WHERE venue_id='binance:spot'")
        .fetch_one(pool)
        .await?;
    Ok((row.try_get(0)?, row.try_get(1)?, row.try_get(2)?))
}

async fn finish(
    pool: PgPool,
    primary: Result<anyhow::Result<()>, tokio::time::error::Elapsed>,
) -> anyhow::Result<()> {
    let cleanup = tokio::time::timeout(Duration::from_secs(3), pool.close()).await;
    eprintln!("AUDIT_RUN_FINAL primary={primary:?} cleanup={cleanup:?}");
    match (primary, cleanup) {
        (Ok(Ok(())), Ok(())) => Ok(()),
        (primary, cleanup) => Err(anyhow!("primary={primary:?}; pool_cleanup={cleanup:?}")),
    }
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn global_checkpoints_advance_once_per_acknowledged_pass(pool: PgPool) -> anyhow::Result<()> {
    let primary = tokio::time::timeout(Duration::from_secs(30), async {
        initialize(&pool).await?;
        let (_, prior, generation) = venue(&pool).await?;
        let source: Vec<_> = (0..2001u128).map(|n| (n, "binance:usdm")).collect();
        seed(&pool, &source).await?;
        let trace = Trace::default();
        let envelope = envelope()?;
        let audit = Uuid::now_v7();
        let run = start(&pool, audit, envelope, &trace).await.map_err(|e| anyhow!("{e:?}"))?;
        ensure!(trace.issued() == 5, "setup is exactly five executions");
        ensure!(run.generation == generation + 1 && run.high_water == Some(Uuid::from_u128(2000)));
        let (state, bound, _) = venue(&pool).await?;
        ensure!(state == "complete" && bound == Some(audit) && prior != Some(audit));
        ensure!(durable(&pool, prior.ok_or(anyhow!("fixture audit"))?).await?.3 == "complete");
        let Pass::Checkpointed(first) =
            pass(&pool, run, &trace).await.map_err(|e| anyhow!("{e:?}"))?
        else {
            return Err(anyhow!("first page must checkpoint"));
        };
        ensure!(first.cursor == Some(Uuid::from_u128(999)) && first.examined == 1000);
        ensure!(trace.issued() == 10, "one pass is exactly five executions");
        // A wrong expected cursor is refused without a write.
        let wrong = Run {
            cursor: Some(Uuid::from_u128(500)),
            examined: 501,
            ..first
        };
        ensure!(pass(&pool, wrong, &trace).await.map_err(|e| anyhow!("{e:?}"))? == Pass::Stale);
        ensure!(
            durable(&pool, audit).await? == (run.generation, first.cursor, 1000, "running".into())
        );
        let Pass::Checkpointed(second) =
            pass(&pool, first, &trace).await.map_err(|e| anyhow!("{e:?}"))?
        else {
            return Err(anyhow!("second page must checkpoint"));
        };
        ensure!(second.cursor == Some(Uuid::from_u128(1999)) && second.examined == 2000);
        let Pass::Checkpointed(last) =
            pass(&pool, second, &trace).await.map_err(|e| anyhow!("{e:?}"))?
        else {
            return Err(anyhow!("last page must checkpoint"));
        };
        ensure!(last.cursor == last.high_water && last.examined == 2001);
        let before = trace.issued();
        ensure!(pass(&pool, last, &trace).await.map_err(|e| anyhow!("{e:?}"))? == Pass::Exhausted);
        ensure!(trace.issued() == before, "exhausted issues no SQL");
        ensure!(durable(&pool, audit).await?.3 == "running", "never completion here");
        let usdm: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM submit_attempts WHERE venue_id='binance:usdm'",
        )
        .fetch_one(&pool)
        .await?;
        ensure!(usdm == 2001, "non-Spot source preserved");
        Ok(())
    })
    .await;
    finish(pool, primary).await
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn fresh_generation_makes_old_run_stale(pool: PgPool) -> anyhow::Result<()> {
    let primary = tokio::time::timeout(Duration::from_secs(30), async {
        initialize(&pool).await?;
        let source: Vec<_> = (0..1500u128).map(|n| (n, "binance:usdm")).collect();
        seed(&pool, &source).await?;
        let trace = Trace::default();
        let envelope = envelope()?;
        let old = start(&pool, Uuid::now_v7(), envelope, &trace)
            .await
            .map_err(|e| anyhow!("{e:?}"))?;
        let new = start(&pool, Uuid::now_v7(), envelope, &trace)
            .await
            .map_err(|e| anyhow!("{e:?}"))?;
        ensure!(new.generation == old.generation + 1);
        ensure!(pass(&pool, old, &trace).await.map_err(|e| anyhow!("{e:?}"))? == Pass::Stale);
        ensure!(durable(&pool, old.audit).await? == (old.generation, None, 0, "running".into()));
        let Pass::Checkpointed(next) =
            pass(&pool, new, &trace).await.map_err(|e| anyhow!("{e:?}"))?
        else {
            return Err(anyhow!("current run must checkpoint"));
        };
        ensure!(next.examined == 1000);
        Ok(())
    })
    .await;
    finish(pool, primary).await
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn initialized_marker_with_missing_spot_records_fails_audit(
    pool: PgPool,
) -> anyhow::Result<()> {
    let primary = tokio::time::timeout(Duration::from_secs(30), async {
        initialize(&pool).await?;
        let source: Vec<_> = (0..1200u128)
            .map(|n| {
                (
                    n,
                    if n == 1100 {
                        "binance:spot"
                    } else {
                        "binance:usdm"
                    },
                )
            })
            .collect();
        seed(&pool, &source).await?;
        let trace = Trace::default();
        let envelope = envelope()?;
        let run = start(&pool, Uuid::now_v7(), envelope, &trace)
            .await
            .map_err(|e| anyhow!("{e:?}"))?;
        let Pass::Checkpointed(first) =
            pass(&pool, run, &trace).await.map_err(|e| anyhow!("{e:?}"))?
        else {
            return Err(anyhow!("futures-only first page must checkpoint"));
        };
        let failed = pass(&pool, first, &trace).await.map_err(|e| anyhow!("{e:?}"))?;
        ensure!(failed == Pass::Failed(Discrepancy::MissingClock), "{failed:?}");
        let (generation, cursor, count, status) = durable(&pool, run.audit).await?;
        ensure!((generation, cursor, count) == (run.generation, first.cursor, 1000));
        ensure!(status == "failed", "retains the last committed checkpoint");
        ensure!(pass(&pool, first, &trace).await.map_err(|e| anyhow!("{e:?}"))? == Pass::Stale);
        let clocks: i64 = sqlx::query_scalar("SELECT count(*) FROM spot_intent_clock")
            .fetch_one(&pool)
            .await?;
        let charges: i64 = sqlx::query_scalar("SELECT count(*) FROM budget_daily_charges")
            .fetch_one(&pool)
            .await?;
        ensure!(clocks == 0 && charges == 0, "no repair or enrichment");
        Ok(())
    })
    .await;
    finish(pool, primary).await
}
