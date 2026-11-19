//! Terminal outcome and reverse charge closure on native M38/M40 fixtures.
use crate::venue_budget::source_audit::run::{
    finish_audit, Fault, Inconsistency, Phase, Terminal, Witness,
};

use super::*;

async fn final_run(pool: &PgPool, trace: &Trace) -> anyhow::Result<Run> {
    let mut run = start(pool, Uuid::now_v7(), envelope()?, trace)
        .await
        .map_err(|err| anyhow!("{err:?}"))?;
    while run.cursor != run.high_water {
        match pass(pool, run, trace).await {
            Ok(Pass::Checkpointed(next)) => run = next,
            other => anyhow::bail!("native forward page failed: {other:?}"),
        }
    }
    Ok(run)
}

async fn populate(pool: &PgPool) -> anyhow::Result<Uuid> {
    let tenant = seed(
        pool,
        &[
            (1, "binance:spot"),
            (3, "binance:spot"),
            (5, "binance:usdm"),
        ],
    )
    .await?;
    project_healthy_spot(pool, tenant).await?;
    Ok(tenant)
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn terminal_completes_empty_cut_and_refuses_replay(pool: PgPool) -> anyhow::Result<()> {
    initialize(&pool).await?;
    let trace = Trace::default();
    let run = final_run(&pool, &trace).await?;
    ensure!(run.high_water.is_none() && trace.issued() == 5);
    let result = finish_audit(&pool, run, &trace).await;
    let Ok(Terminal::Complete(receipt)) = result else {
        anyhow::bail!("empty cut completion: {result:?}");
    };
    ensure!(receipt.witness == (Witness { source: 0, clocks: 0, charges: 0 }));
    ensure!(
        (receipt.audit, receipt.generation, receipt.high_water, receipt.examined)
            == (run.audit, run.generation, run.high_water, run.examined)
    );
    ensure!(trace.issued() == 10);
    let durable_completed_at: chrono::DateTime<Utc> = sqlx::query_scalar(
        "SELECT completed_at FROM budget_source_audits WHERE audit_id=$1 AND status='complete'",
    )
    .bind(run.audit)
    .fetch_one(&pool)
    .await?;
    ensure!(receipt.completed_at == durable_completed_at);
    let replay = finish_audit(&pool, run, &trace).await;
    ensure!(matches!(replay, Ok(Terminal::Stale)));
    ensure!(durable(&pool, run.audit).await? == (run.generation, None, 0, "complete".into()));
    Ok(())
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn terminal_ignores_an_above_water_usdm_append_but_refuses_a_changed_cut(
    pool: PgPool,
) -> anyhow::Result<()> {
    initialize(&pool).await?;
    populate(&pool).await?;
    let trace = Trace::default();
    let old = final_run(&pool, &trace).await?;
    ensure!(old.high_water == Some(Uuid::from_u128(5)));
    seed(&pool, &[(9, "binance:usdm")]).await?;
    let result = finish_audit(&pool, old, &trace).await;
    let Ok(Terminal::Complete(receipt)) = result else {
        anyhow::bail!("USD-M append above the cut must be compatible: {result:?}");
    };
    ensure!(receipt.witness == (Witness { source: 3, clocks: 2, charges: 2 }));
    ensure!(trace.issued() == 15);

    let current = final_run(&pool, &trace).await?;
    seed(&pool, &[(2, "binance:usdm")]).await?;
    let changed = finish_audit(&pool, current, &trace).await;
    ensure!(matches!(
        changed,
        Err(Fault {
            phase: Phase::Terminal,
            error: BudgetError::Invalid,
            ..
        })
    ));
    ensure!(durable(&pool, current.audit).await?.3 == "running");
    ensure!(durable(&pool, old.audit).await?.3 == "complete");
    Ok(())
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn terminal_refuses_spot_clock_straddling_the_recorded_high_water(
    pool: PgPool,
) -> anyhow::Result<()> {
    initialize(&pool).await?;
    populate(&pool).await?;
    let trace = Trace::default();
    let run = final_run(&pool, &trace).await?;
    let late = seed(&pool, &[(7, "binance:spot"), (8, "binance:spot")]).await?;
    project_healthy_spot(&pool, late).await?;
    let result = finish_audit(&pool, run, &trace).await;
    ensure!(matches!(
        result,
        Err(Fault {
            phase: Phase::Terminal,
            error: BudgetError::Invalid,
            ..
        })
    ));
    ensure!(durable(&pool, run.audit).await?.3 == "running");
    Ok(())
}

/// Create one retained restored row with no source or clock, then restore the
/// exact NOT VALID FK. This runs only in this sqlx-owned disposable database.
async fn restore_orphan_charge(pool: &PgPool, tenant: Uuid, retired: bool) -> anyhow::Result<Uuid> {
    let attempt = Uuid::from_u128(120);
    let proof = retired.then(Uuid::now_v7);
    let mut restore = pool.begin().await?;
    if let Some(proof) = proof {
        sqlx::query(
            "INSERT INTO budget_expiry_proofs
               (proof_id,venue_id,proof_kind,source_attempt_id,clock_domain,start_tick,end_tick,recorded_at)
             VALUES($1,'binance:spot','same_domain',$2,
               ROW('boottime',$3,0,0)::budget_clock_domain_v38,0,86460000000000,NOW())",
        )
        .bind(proof).bind(attempt).bind(Uuid::now_v7()).execute(&mut *restore).await?;
    }
    sqlx::raw_sql("ALTER TABLE budget_daily_charges DROP CONSTRAINT budget_charge_clock_fk")
        .execute(&mut *restore)
        .await?;
    sqlx::query(
        "INSERT INTO budget_daily_charges
         (attempt_id,venue_id,tenant_id,intent_id,account_scope,client_order_id,kind,
          source_created_at,daily_units,expiry_proof_id,retired_at)
         VALUES($1,'binance:spot',$2,$3,'restored','orphan-charge',$4,NOW(),$5,$6,
          CASE WHEN $6::uuid IS NULL THEN NULL ELSE NOW() END)",
    )
    .bind(attempt)
    .bind(tenant)
    .bind(Uuid::now_v7())
    .bind(if retired { "entry" } else { "cancel" })
    .bind(i16::from(retired))
    .bind(proof)
    .execute(&mut *restore)
    .await?;
    sqlx::raw_sql(
        "ALTER TABLE budget_daily_charges ADD CONSTRAINT budget_charge_clock_fk
         FOREIGN KEY(attempt_id) REFERENCES spot_intent_clock(attempt_id) NOT VALID",
    )
    .execute(&mut *restore)
    .await?;
    restore.commit().await?;
    let restored: (bool, bool, i16, bool) = sqlx::query_as(
        "SELECT
            NOT EXISTS(SELECT 1 FROM submit_attempts a WHERE a.attempt_id=d.attempt_id)
            AND NOT EXISTS(SELECT 1 FROM spot_intent_clock c WHERE c.attempt_id=d.attempt_id),
            EXISTS(SELECT 1 FROM pg_constraint WHERE conname='budget_charge_clock_fk'
                   AND conrelid='budget_daily_charges'::regclass AND NOT convalidated),
            d.daily_units,d.retired_at IS NOT NULL
         FROM budget_daily_charges d WHERE d.attempt_id=$1",
    )
    .bind(attempt)
    .fetch_one(pool)
    .await?;
    ensure!(restored == (true, true, i16::from(retired), retired));
    Ok(attempt)
}

async fn orphan_refusal(pool: &PgPool, retired: bool) -> anyhow::Result<()> {
    initialize(pool).await?;
    let tenant = populate(pool).await?;
    let trace = Trace::default();
    let original = final_run(pool, &trace).await?;
    let healthy = finish_audit(pool, original, &trace).await;
    ensure!(matches!(healthy, Ok(Terminal::Complete(r))
        if r.witness == (Witness { source: 3, clocks: 2, charges: 2 })));

    let attempt = restore_orphan_charge(pool, tenant, retired).await?;
    let run = final_run(pool, &trace).await?;
    let result = finish_audit(pool, run, &trace).await;
    ensure!(
        matches!(&result, Ok(Terminal::Failed(Inconsistency::ChargeCount, receipt))
            if receipt.witness == (Witness { source: 3, clocks: 2, charges: 3 })),
        "orphan charge must fail the terminal parity gate: {result:?}"
    );
    ensure!(durable(pool, original.audit).await?.3 == "complete");
    ensure!(
        durable(pool, run.audit).await?
            == (run.generation, run.cursor, run.examined, "failed".into())
    );
    let retained: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM budget_daily_charges WHERE attempt_id=$1)")
            .bind(attempt)
            .fetch_one(pool)
            .await?;
    ensure!(retained && trace.issued() == 30);
    Ok(())
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn reverse_terminal_closure_rejects_a_charge_only_cancel(pool: PgPool) -> anyhow::Result<()> {
    orphan_refusal(&pool, false).await
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn reverse_terminal_closure_rejects_a_charge_only_tombstone(
    pool: PgPool,
) -> anyhow::Result<()> {
    orphan_refusal(&pool, true).await
}
