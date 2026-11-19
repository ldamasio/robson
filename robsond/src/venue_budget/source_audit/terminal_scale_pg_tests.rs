//! End-to-end timing through the healthy historical envelope on disposable PG.
use crate::venue_budget::source_audit::run::{finish_audit, Terminal};

use super::*;

const SOURCE_ROWS: i64 = 1_000_000;

async fn seed_full_history(pool: &PgPool) -> anyhow::Result<()> {
    let tenant = Uuid::now_v7();
    let at = Utc::now();
    let mut connection = pool.acquire().await?;
    sqlx::query(
        "INSERT INTO accounting_state(tenant_id,venue_id,budget_model)
         VALUES($1,'binance:spot','net_from_start_non_expanding_v1')",
    )
    .bind(tenant)
    .execute(&mut *connection)
    .await?;
    let written = sqlx::query(
        "INSERT INTO order_intents(intent_id,tenant_id,venue_id,account_scope,client_order_id,kind,params,created_at)
         SELECT lpad(to_hex(n+1000000),32,'0')::uuid,$1,'binance:spot','terminal-scale',
                'terminal-scale-' || n,'cancel','{}'::jsonb,$2
         FROM generate_series(1::bigint,$3::bigint) AS n",
    )
    .bind(tenant)
    .bind(at)
    .bind(SOURCE_ROWS)
    .execute(&mut *connection)
    .await?;
    ensure!(written.rows_affected() == SOURCE_ROWS as u64);
    let written = sqlx::query(
        "INSERT INTO submit_attempts(attempt_id,venue_id,account_scope,client_order_id,state,created_at)
         SELECT lpad(to_hex(n),32,'0')::uuid,'binance:spot','terminal-scale',
                'terminal-scale-' || n,'started',$1
         FROM generate_series(1::bigint,$2::bigint) AS n",
    )
    .bind(at)
    .bind(SOURCE_ROWS)
    .execute(&mut *connection)
    .await?;
    ensure!(written.rows_affected() == SOURCE_ROWS as u64);
    for lower in (1..=SOURCE_ROWS).step_by(1000) {
        let written = sqlx::query(
            "INSERT INTO spot_intent_clock(attempt_id,intent_id,tenant_id,venue_id,attempt_created_at)
             SELECT lpad(to_hex(n),32,'0')::uuid,lpad(to_hex(n+1000000),32,'0')::uuid,
                    $1,'binance:spot',$2
             FROM generate_series($3::bigint,$4::bigint) AS n",
        )
        .bind(tenant)
        .bind(at)
        .bind(lower)
        .bind(lower + 999)
        .execute(&mut *connection)
        .await?;
        ensure!(written.rows_affected() == 1000);
    }
    for command in [
        "VACUUM (ANALYZE) order_intents",
        "VACUUM (ANALYZE) submit_attempts",
        "VACUUM (ANALYZE) spot_intent_clock",
        "VACUUM (ANALYZE) budget_daily_charges",
    ] {
        sqlx::raw_sql(command).execute(&mut *connection).await?;
    }
    Ok(())
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires isolated PG and a million-row measurement run"]
async fn measured_million_row_cold_terminal_envelope(pool: PgPool) -> anyhow::Result<()> {
    initialize(&pool).await?;
    let seed_started = Instant::now();
    seed_full_history(&pool).await?;
    eprintln!(
        "C2_FULL_SEED rows={SOURCE_ROWS} elapsed_ms={}",
        seed_started.elapsed().as_millis()
    );

    let audit = Uuid::now_v7();
    let trace = Trace::default();
    let started = Instant::now();
    let envelope =
        Envelope::new(started + Duration::from_secs(60)).map_err(|e| anyhow!("{e:?}"))?;
    let mut run = start(&pool, audit, envelope, &trace).await.map_err(|e| anyhow!("{e:?}"))?;
    let mut pages = 0;
    while run.cursor != run.high_water {
        let outcome = pass(&pool, run, &trace).await;
        let Ok(Pass::Checkpointed(next)) = outcome else {
            anyhow::bail!(
                "C2_FULL_PAGING pages={pages} wire={} elapsed_ms={} outcome={outcome:?} state={:?}",
                trace.issued(),
                started.elapsed().as_millis(),
                durable(&pool, audit).await?,
            );
        };
        run = next;
        pages += 1;
    }
    ensure!(pages == 1000 && run.examined == SOURCE_ROWS);
    let terminal_started = Instant::now();
    let result = finish_audit(&pool, run, &trace).await;
    let elapsed = started.elapsed();
    let terminal_elapsed = terminal_started.elapsed();
    eprintln!(
        "C2_FULL_TERMINAL rows={SOURCE_ROWS} pages={pages} wire={} total_ms={} terminal_ms={} outcome={result:?}",
        trace.issued(), elapsed.as_millis(), terminal_elapsed.as_millis(),
    );
    if !matches!(result, Ok(Terminal::Complete(_))) {
        anyhow::bail!("full envelope left durable {:?}", durable(&pool, audit).await?);
    }
    ensure!(trace.issued() == 5010 && elapsed < Duration::from_secs(60));
    ensure!(terminal_elapsed < Duration::from_secs(2));
    ensure!(durable(&pool, audit).await?.3 == "complete");
    Ok(())
}
