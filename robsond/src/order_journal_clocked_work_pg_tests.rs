//! Local native accounting. Server-side plans are separated from fault timing.
use robson_exec::order_journal::JournalBackend;

use super::*;

async fn mark(wire: &Wire, phase: &str) -> Result<()> {
    sqlx::query("SELECT set_config('robson_test.j2b_phase',$1,false)")
        .bind(phase)
        .fetch_one(&wire.store.pool)
        .await?;
    sqlx::raw_sql(
        "DO $$ BEGIN RAISE LOG 'ROBSON_J2B phase=% db=%',
            current_setting('robson_test.j2b_phase'),current_database(); END $$",
    )
    .execute(&wire.store.pool)
    .await?;
    Ok(())
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires isolated superuser PostgreSQL with native auto_explain"]
async fn journal_wire_and_nested_work_are_separately_measured(pool: PgPool) -> Result<()> {
    let scope = scope(&pool).await?;
    let wire = Wire::new(&pool, scope.tenant).await?;
    sqlx::raw_sql(
        "LOAD 'auto_explain';
         SET auto_explain.log_min_duration='0';
         SET auto_explain.log_analyze=on;
         SET auto_explain.log_buffers=on;
         SET auto_explain.log_triggers=on;
         SET auto_explain.log_nested_statements=on;
         SET auto_explain.log_format=json;",
    )
    .execute(&wire.store.pool)
    .await?;
    let (exact, input) = one(&scope, "j2b-measure-exact")?;
    let (legacy, _) = one(&scope, "j2b-measure-legacy")?;
    for (phase, clocked) in [("cold", true), ("warm", true), ("legacy", false)] {
        mark(&wire, phase).await?;
        let before = wire.proxy.stats().statements;
        let before_parses = wire.proxy.prepared_statement_parses();
        wire.proxy.enable_exact_query_trace(12);
        if clocked {
            let receipts = wire.store.commit_clocked(&exact, &input).await;
            ensure!(receipts.as_ref().is_ok_and(|r| r.len() == 1), "{phase} {receipts:?}");
        } else {
            wire.store.commit(&legacy).await.map_err(|error| anyhow!("{error:?}"))?;
        }
        let trace = wire.proxy.query_trace();
        let executions = wire.proxy.stats().statements - before;
        let parses = wire.proxy.prepared_statement_parses() - before_parses;
        ensure!(wire.proxy.query_trace_dropped() == 0 && executions == 7);
        let begin = if clocked {
            "BEGIN ISOLATION LEVEL READ COMMITTED"
        } else {
            "BEGIN"
        };
        ensure!(trace.len() == 7 && trace[0] == begin && is_commit(&trace[6]));
        ensure!(trace[5].trim_start().starts_with(if clocked {
            HELPER
        } else {
            "INSERT INTO spot_intent_clock"
        }));
        if phase == "warm" {
            ensure!(parses == 0, "warm repeat reused prepared execution");
        }
        ensure!(wire.identity().is_ok(), "all samples used the same backend");
        eprintln!(
            "J2B_WORK phase={phase} db={} pid={} wire_executions={executions} parses={parses} sql={}",
            pool.connect_options().get_database().unwrap(),
            wire.pid,
            serde_json::to_string(&trace)?,
        );
    }
    mark(&wire, "end").await?;
    Ok(())
}
