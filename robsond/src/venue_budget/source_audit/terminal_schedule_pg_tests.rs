//! The private terminal audit's native wire and concurrent successor controls.
use super::*;
use crate::venue_budget::source_audit::run::{finish_audit, Terminal};

async fn lock_original_venue(
    pool: &PgPool,
) -> anyhow::Result<sqlx::Transaction<'_, sqlx::Postgres>> {
    let mut lock = pool.begin().await?;
    sqlx::query("SELECT 1 FROM budget_venue_state WHERE venue_id='binance:spot' FOR UPDATE")
        .fetch_one(&mut *lock)
        .await?;
    Ok(lock)
}

async fn terminal_status(
    pool: &PgPool,
    original: Run,
    status: &str,
) -> anyhow::Result<Option<chrono::DateTime<Utc>>> {
    let row = sqlx::query(
        "SELECT audit_id,venue_id,quiescence_generation,high_water,cursor,examined_count,
                status,started_at,completed_at,failure_reason
         FROM budget_source_audits WHERE audit_id=$1",
    )
    .bind(original.audit)
    .fetch_one(pool)
    .await?;
    let identity: (Uuid, String, i64, Option<Uuid>, Option<Uuid>, i64, String) = (
        row.try_get(0)?,
        row.try_get(1)?,
        row.try_get(2)?,
        row.try_get(3)?,
        row.try_get(4)?,
        row.try_get(5)?,
        row.try_get(6)?,
    );
    ensure!(
        identity
            == (
                original.audit,
                "binance:spot".to_owned(),
                original.generation,
                original.high_water,
                original.cursor,
                original.examined,
                status.to_owned(),
            ),
        "durable audit tuple: {identity:?}"
    );
    let started: chrono::DateTime<Utc> = row.try_get(7)?;
    let completed: Option<chrono::DateTime<Utc>> = row.try_get(8)?;
    ensure!(row.try_get::<Option<String>, _>(9)?.is_none());
    ensure!(completed.is_some() == (status == "complete"));
    ensure!(completed.is_none_or(|at| at >= started));
    Ok(completed)
}

async fn populate_terminal_cut(pool: &PgPool) -> anyhow::Result<()> {
    let tenant = seed(pool, &[
        (1, "binance:spot"),
        (3, "binance:spot"),
        (5, "binance:usdm"),
    ])
    .await?;
    project_healthy_spot(pool, tenant).await
}

async fn last_checkpoint(pool: &PgPool, trace: &Trace) -> anyhow::Result<Run> {
    let initial = start(pool, Uuid::now_v7(), envelope()?, trace)
        .await
        .map_err(|err| anyhow!("{err:?}"))?;
    let result = pass(pool, initial, trace).await;
    let Ok(Pass::Checkpointed(run)) = result else {
        anyhow::bail!("expected a native populated checkpoint: {result:?}");
    };
    ensure!(
        run.high_water == Some(Uuid::from_u128(5))
            && run.cursor == run.high_water
            && run.examined == 3
    );
    Ok(run)
}

fn commit_sent(trace: &[String]) -> bool {
    trace
        .iter()
        .any(|s| s.lines().any(|sql| sql.trim().trim_end_matches(';') == "COMMIT"))
}

async fn cut_terminal(pool: &PgPool, cancel: bool) -> anyhow::Result<()> {
    initialize(pool).await?;
    populate_terminal_cut(pool).await?;
    let trace = Trace::default();
    let old = last_checkpoint(pool, &trace).await?;
    let wire = Wire::new(pool, "audit-terminal-cut").await?;
    let hold = lock_original_venue(pool).await?;
    let mut hold = hold;
    let holder = holder_pid(&mut hold).await?;
    let mut work = Box::pin(finish_audit(&wire.pool, old, &trace));
    let original = tokio::select! {
        result = &mut work => anyhow::bail!("terminal ended before held venue lock: {result:?}"),
        result = wire.blocked(pool, holder, 3) => result?,
    };
    let mut control = wire.proxy.control(original.identity, false).map_err(|e| anyhow!("{e:?}"))?;
    let target = if cancel { 4 } else { 5 };
    let mut gate = control
        .arm(target, ControlBoundary::BeforeCommandComplete)
        .map_err(|e| anyhow!("{e:?}"))?;
    hold.rollback().await?;
    let hit = tokio::select! {
        result = &mut work => anyhow::bail!("terminal escaped ordinal {target}: {result:?}"),
        result = timeout(Duration::from_millis(350), gate.held()) => result?.map_err(|e| anyhow!("{e:?}"))?,
    };
    ensure!(
        hit.record.identity == original.identity
            && hit.record.ordinal == Some(target)
            && hit.record.event == DiagnosticEvent::Complete
    );
    let expected_status = if cancel { "running" } else { "complete" };
    let expected_completion = terminal_status(pool, old, expected_status).await?;
    if cancel {
        drop(work);
    } else {
        let outcome = work.await;
        ensure!(
            matches!(outcome, Err(Fault { phase: Phase::Commit, audit: Some(id), .. }) if id == old.audit),
            "lost terminal COMMIT must not publish an acknowledgement: {outcome:?}"
        );
    }
    wire.retires(pool, &original).await?;
    let snapshot = closed_by_client(&gate).await?;
    ensure!(terminal_status(pool, old, expected_status).await? == expected_completion);
    let mut free = pool.begin().await?;
    sqlx::query("SELECT 1 FROM budget_venue_state WHERE venue_id='binance:spot' FOR UPDATE NOWAIT")
        .fetch_one(&mut *free)
        .await?;
    sqlx::query("SELECT 1 FROM budget_source_audits WHERE audit_id=$1 FOR UPDATE NOWAIT")
        .bind(old.audit)
        .fetch_one(&mut *free)
        .await?;
    free.rollback().await?;
    let issued = wire.proxy.query_trace();
    ensure!(wire.proxy.query_trace_dropped() == 0 && issued.len() == target as usize);
    ensure!(commit_sent(&issued) != cancel, "no cleanup or replay after cut: {issued:?}");
    drop(gate);
    ensure!(control.close().await.map_err(|e| anyhow!("{e:?}"))? == snapshot);
    Ok(())
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn cancelled_terminal_before_commit_preserves_running_audit(
    pool: PgPool,
) -> anyhow::Result<()> {
    cut_terminal(&pool, true).await
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn withheld_terminal_commit_ack_does_not_produce_completion_receipt(
    pool: PgPool,
) -> anyhow::Result<()> {
    cut_terminal(&pool, false).await
}

async fn generation_cut(pool: &PgPool, replacement_first: bool, jitter: u64) -> anyhow::Result<()> {
    let trace = Trace::default();
    let old = last_checkpoint(pool, &trace).await?;
    let successor_id = Uuid::now_v7();
    let wire = Wire::new(pool, "audit-terminal-race").await?;
    let successor_wire = Wire::new(pool, "audit-terminal-successor").await?;
    let mut hold = if replacement_first {
        let mut hold = pool.begin().await?;
        sqlx::raw_sql("LOCK TABLE budget_venue_state IN ACCESS EXCLUSIVE MODE")
            .execute(&mut *hold)
            .await?;
        hold
    } else {
        lock_original_venue(pool).await?
    };
    let holder = holder_pid(&mut hold).await?;
    let mut work = Box::pin(finish_audit(&wire.pool, old, &trace));
    let blocked_ordinal = if replacement_first { 2 } else { 3 };
    let original = tokio::select! {
        result = &mut work => anyhow::bail!("terminal finished before race setup: {result:?}"),
        result = wire.blocked(pool, holder, blocked_ordinal) => result?,
    };
    let mut control = wire.proxy.control(original.identity, false).map_err(|e| anyhow!("{e:?}"))?;
    let (target, boundary) = if replacement_first {
        (3, ControlBoundary::BeforeForward)
    } else {
        (4, ControlBoundary::BeforeCommandComplete)
    };
    let mut gate = control.arm(target, boundary).map_err(|e| anyhow!("{e:?}"))?;
    hold.rollback().await?;
    let hit = tokio::select! {
        result = &mut work => anyhow::bail!("terminal escaped race gate: {result:?}"),
        result = timeout(Duration::from_millis(350), gate.held()) => result?.map_err(|e| anyhow!("{e:?}"))?,
    };
    ensure!(hit.record.identity == original.identity && hit.record.ordinal == Some(target));
    tokio::time::sleep(Duration::from_millis(jitter)).await;

    let (terminal, new) = if replacement_first {
        let new = start(&successor_wire.pool, successor_id, envelope()?, &trace)
            .await
            .map_err(|e| anyhow!("{e:?}"))?;
        let release = gate.release();
        let (terminal, released) = tokio::join!(work, release);
        released.map_err(|e| anyhow!("{e:?}"))?;
        (terminal, new)
    } else {
        let mut next = Box::pin(start(&successor_wire.pool, successor_id, envelope()?, &trace));
        let observed = tokio::select! {
            result = &mut next => anyhow::bail!("successor won while old terminal locked venue: {result:?}"),
            result = successor_wire.blocked(pool, original.pid, 3) => result?,
        };
        ensure!(observed.pid != original.pid);
        let gate = async { gate.release().await.map_err(|e| anyhow!("{e:?}")) };
        let (terminal, new, released) = tokio::join!(work, next, gate);
        released?;
        (terminal, new.map_err(|e| anyhow!("{e:?}"))?)
    };
    wire.retires(pool, &original).await?;
    if replacement_first {
        ensure!(matches!(terminal, Ok(Terminal::Stale)), "superseded terminal: {terminal:?}");
        ensure!(terminal_status(pool, old, "running").await?.is_none());
    } else {
        let Ok(Terminal::Complete(receipt)) = terminal else {
            anyhow::bail!("initial terminal winner: {terminal:?}");
        };
        ensure!(terminal_status(pool, old, "complete").await? == Some(receipt.completed_at));
        ensure!(
            receipt.audit == old.audit
                && receipt.high_water == old.high_water
                && receipt.generation == old.generation
                && receipt.examined == old.examined
        );
    }
    ensure!(terminal_status(pool, new, "running").await?.is_none());
    let (_, active_audit, active_generation) = venue(pool).await?;
    ensure!(active_audit == Some(successor_id) && active_generation == old.generation + 1);
    ensure!(new.audit == successor_id && new.generation == active_generation);
    Ok(())
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn generated_terminal_and_generation_replacement_obey_the_durable_oracle(
    pool: PgPool,
) -> anyhow::Result<()> {
    initialize(&pool).await?;
    populate_terminal_cut(&pool).await?;
    let mut rng = TestRunner::new(Config {
        rng_seed: RngSeed::Fixed(2026111910),
        failure_persistence: None,
        ..Config::default()
    });
    let jitter = proptest::collection::vec(0_u64..9, 8)
        .new_tree(&mut rng)
        .map_err(|error| anyhow!("terminal schedule: {error:?}"))?
        .current();
    for (index, millis) in jitter.into_iter().enumerate() {
        let replacement_first = index % 2 == 0;
        generation_cut(&pool, replacement_first, millis).await?;
        eprintln!(
            "C2_TERMINAL_RACE case={index} replacement_first={replacement_first} delay_ms={millis}"
        );
    }
    Ok(())
}
