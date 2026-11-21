//! Native competitors and protocol gates for the local cold audit only.
use proptest::{
    strategy::{Strategy, ValueTree},
    test_runner::{Config, RngSeed, TestRunner},
};
use robson_testkit::db_proxy::{
    ClientLoss, ControlBoundary, ControlGate, ControlSnapshot, DbProxy, DiagnosticEvent,
    DiagnosticIdentity,
};
use sqlx::postgres::{PgPoolOptions, PgSslMode};
use tokio::time::timeout;

use super::{
    super::run::{Fault, Phase},
    *,
};

#[path = "terminal_schedule_pg_tests.rs"]
mod terminal;

struct Wire {
    proxy: DbProxy,
    pool: PgPool,
}

struct Blocked {
    identity: DiagnosticIdentity,
    pid: i32,
    backend_start: String,
    statement: u64,
}

impl Wire {
    async fn new(observer: &PgPool, label: &str) -> anyhow::Result<Self> {
        let options = observer.connect_options();
        let upstream = tokio::net::lookup_host((options.get_host(), options.get_port()))
            .await?
            .next()
            .ok_or_else(|| anyhow!("local fixture address"))?;
        ensure!(upstream.ip().is_loopback());
        let proxy = DbProxy::start(upstream).await?;
        proxy.enable_diagnostics();
        proxy.enable_exact_query_trace(12);
        let pool = PgPoolOptions::new().max_connections(1).connect_lazy_with(
            (*options)
                .clone()
                .host("127.0.0.1")
                .port(proxy.addr().port())
                .ssl_mode(PgSslMode::Disable)
                .application_name(label),
        );
        Ok(Self { proxy, pool })
    }

    async fn blocked(
        &self,
        observer: &PgPool,
        holder: i32,
        statement: u64,
    ) -> anyhow::Result<Blocked> {
        timeout(Duration::from_millis(175), async {
            loop {
                let identities = self.proxy.diagnostic_identities().unwrap_or_default();
                if let Some(identity) = identities.into_iter().find(|i| i.live && i.pid.is_some()) {
                    let pid = identity.pid.unwrap();
                    let start: Option<String> = sqlx::query_scalar(
                        "SELECT backend_start::text FROM pg_stat_activity
                         WHERE pid=$1 AND datname=current_database() AND $2=ANY(pg_blocking_pids(pid))",
                    )
                    .bind(pid)
                    .bind(holder)
                    .fetch_optional(observer)
                    .await?;
                    if let Some(backend_start) = start {
                        let events = self.proxy.diagnostics().ok_or_else(|| anyhow!("audit diagnostics"))?;
                        ensure!(events.dropped == 0 && events.phase_overflow == 0);
                        let latest = events.records.iter().rev().find(|e| {
                            e.identity.connection == identity.connection && e.event == DiagnosticEvent::Sent
                                && e.ordinal.is_some()
                        });
                        ensure!(
                            latest.and_then(|e| e.ordinal) == Some(statement)
                                && self.proxy.query_trace().len() == statement as usize,
                            "actual blocked statement {statement}: {:?}",
                            self.proxy.query_trace()
                        );
                        return Ok(Blocked { identity, pid, backend_start, statement });
                    }
                }
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        }).await?
    }

    async fn retires(&self, observer: &PgPool, original: &Blocked) -> anyhow::Result<()> {
        timeout(Duration::from_secs(2), async {
            loop {
                let alive: bool = sqlx::query_scalar(
                    "SELECT EXISTS(SELECT 1 FROM pg_stat_activity
                     WHERE pid=$1 AND backend_start::text=$2 AND datname=current_database())",
                )
                .bind(original.pid)
                .bind(&original.backend_start)
                .fetch_one(observer)
                .await?;
                if !alive {
                    return Ok::<_, sqlx::Error>(());
                }
                tokio::time::sleep(Duration::from_millis(3)).await;
            }
        })
        .await??;
        Ok(())
    }
}

async fn closed_by_client(gate: &ControlGate<'_>) -> anyhow::Result<ControlSnapshot> {
    let snapshot = timeout(Duration::from_secs(1), async {
        loop {
            let state = gate.snapshot();
            if state.closed {
                break state;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await?;
    ensure!(
        snapshot.client_loss == Some(ClientLoss::Read)
            && snapshot.proof_valid
            && !snapshot.released,
        "closed by the original client while the gate remained unreleased: {snapshot:?}"
    );
    Ok(snapshot)
}

async fn final_row(
    pool: &PgPool,
    run: Run,
    cursor: Option<Uuid>,
    examined: i64,
) -> anyhow::Result<()> {
    ensure!(
        durable(pool, run.audit).await? == (run.generation, cursor, examined, "running".into())
    );
    Ok(())
}

fn checkpoint_oracle(source: &[Uuid], prior: Option<Uuid>, examined: i64) -> (Option<Uuid>, i64) {
    let page: Vec<_> = source
        .iter()
        .copied()
        .filter(|id| prior.is_none_or(|cursor| *id > cursor))
        .take(1000)
        .collect();
    (page.last().copied(), examined + page.len() as i64)
}

async fn holder_pid(tx: &mut sqlx::Transaction<'_, sqlx::Postgres>) -> anyhow::Result<i32> {
    Ok(sqlx::query_scalar("SELECT pg_backend_pid()").fetch_one(&mut **tx).await?)
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn generated_same_cursor_competitors_commit_one_native_checkpoint(
    pool: PgPool,
) -> anyhow::Result<()> {
    initialize(&pool).await?;
    let source: Vec<_> = (0..1500_u128).map(|n| (n, "binance:usdm")).collect();
    seed(&pool, &source).await?;
    let ids: Vec<_> = source.iter().map(|r| Uuid::from_u128(r.0)).collect();
    let expected = checkpoint_oracle(&ids, None, 0);
    let mut rng = TestRunner::new(Config {
        rng_seed: RngSeed::Fixed(2026111909),
        failure_persistence: None,
        ..Config::default()
    });
    let jitters = proptest::collection::vec((0_u64..9, 0_u64..9), 8)
        .new_tree(&mut rng)
        .map_err(|err| anyhow!("schedule seed: {err:?}"))?
        .current();
    for (case, (peer_ms, release_ms)) in jitters.into_iter().enumerate() {
        let base = start(&pool, Uuid::now_v7(), envelope()?, &Trace::default())
            .await
            .map_err(|e| anyhow!("{e:?}"))?;
        let first = Wire::new(
            &pool,
            if case % 2 == 0 {
                "audit-left"
            } else {
                "audit-right"
            },
        )
        .await?;
        let second = Wire::new(
            &pool,
            if case % 2 == 0 {
                "audit-right"
            } else {
                "audit-left"
            },
        )
        .await?;
        let mut hold = pool.begin().await?;
        sqlx::query("SELECT 1 FROM budget_venue_state WHERE venue_id='binance:spot' FOR UPDATE")
            .fetch_one(&mut *hold)
            .await?;
        let holder = holder_pid(&mut hold).await?;
        let trace_a = Trace::default();
        let trace_b = Trace::default();
        let mut a = Box::pin(pass(&first.pool, base, &trace_a));
        let mut b = Box::pin(pass(&second.pool, base, &trace_b));
        let wait_a = tokio::select! {
            value = &mut a => anyhow::bail!("first finished before the venue lock released: {value:?}"),
            value = first.blocked(&pool, holder, 4) => value?,
        };
        tokio::time::sleep(Duration::from_millis(peer_ms)).await;
        let wait_b = tokio::select! {
            value = &mut a => anyhow::bail!("first left the held row: {value:?}"),
            value = &mut b => anyhow::bail!("second finished before row release: {value:?}"),
            value = second.blocked(&pool, holder, 4) => value?,
        };
        ensure!(wait_a.pid != wait_b.pid);
        tokio::time::sleep(Duration::from_millis(release_ms)).await;
        hold.rollback().await?;
        let (a, b) = tokio::join!(a, b);
        let winner = match (a, b) {
            (Ok(Pass::Checkpointed(winner)), Ok(Pass::Stale))
            | (Ok(Pass::Stale), Ok(Pass::Checkpointed(winner))) => winner,
            other => anyhow::bail!("same-cursor race must have one success: {other:?}"),
        };
        ensure!((winner.cursor, winner.examined) == expected);
        final_row(&pool, base, expected.0, expected.1).await?;
        ensure!(venue(&pool).await?.1 == Some(base.audit));
        if case % 2 == 1 {
            // Reverse of the held-old-page case: checkpoint wins before setup.
            let newer = start(&pool, Uuid::now_v7(), envelope()?, &Trace::default())
                .await
                .map_err(|e| anyhow!("{e:?}"))?;
            ensure!(newer.generation == base.generation + 1);
            ensure!(pass(&pool, winner, &Trace::default()).await == Ok(Pass::Stale));
            final_row(&pool, base, expected.0, expected.1).await?;
            final_row(&pool, newer, None, 0).await?;
            ensure!(venue(&pool).await?.1 == Some(newer.audit));
        }

        eprintln!("C2_CAS_GENERATED case={case} peer_ms={peer_ms} release_ms={release_ms}");
    }
    Ok(())
}

async fn held_old_page(pool: &PgPool, cancel: bool) -> anyhow::Result<()> {
    initialize(pool).await?;
    seed(pool, &[(5, "binance:usdm")]).await?;
    let envelope = envelope()?;
    let trace = Trace::default();
    let old = start(pool, Uuid::now_v7(), envelope, &trace)
        .await
        .map_err(|e| anyhow!("{e:?}"))?;
    let wire = Wire::new(pool, "audit-old-page").await?;
    let mut hold = pool.begin().await?;
    sqlx::raw_sql("LOCK TABLE submit_attempts IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *hold)
        .await?;
    let holder = holder_pid(&mut hold).await?;
    let mut work = Box::pin(pass(&wire.pool, old, &trace));
    // PostgreSQL takes the source-table lock while parsing the prepared page.
    // Its Execute ordinal is therefore one after the last issued execution.
    let blocked = tokio::select! {
        result = &mut work => anyhow::bail!("old page completed before gate: {result:?}"),
        result = wire.blocked(pool, holder, 2) => result?,
    };
    let mut control = wire.proxy.control(blocked.identity, false).map_err(|e| anyhow!("{e:?}"))?;
    let mut gate = control
        .arm(blocked.statement + 1, ControlBoundary::BeforeCommandComplete)
        .map_err(|e| anyhow!("{e:?}"))?;
    hold.rollback().await?;
    let hit = tokio::select! {
        result = &mut work => anyhow::bail!("audit ended before actual page gate: {result:?}"),
        result = timeout(Duration::from_millis(350), gate.held()) => result?.map_err(|e| anyhow!("{e:?}"))?,
    };
    ensure!(hit.record.ordinal == Some(3) && hit.record.event == DiagnosticEvent::Complete);
    ensure!(wire.proxy.data_rows() == 2, "settings and old bound page reached the wire");
    if cancel {
        drop(work);
        wire.retires(pool, &blocked).await?;
        let state = closed_by_client(&gate).await?;
        ensure!(wire.proxy.query_trace_dropped() == 0 && wire.proxy.query_trace().len() == 3);
        final_row(pool, old, None, 0).await?;
        drop(gate);
        ensure!(control.close().await.map_err(|e| anyhow!("{e:?}"))? == state);
        return Ok(());
    }
    let actor = async {
        let new = start(pool, Uuid::now_v7(), envelope, &trace)
            .await
            .map_err(|e| anyhow!("{e:?}"))?;
        ensure!(new.generation == old.generation + 1);
        gate.release().await.map_err(|e| anyhow!("{e:?}"))?;
        Ok::<_, anyhow::Error>(new)
    };
    let (outcome, successor) = tokio::join!(work, actor);
    ensure!(
        outcome == Ok(Pass::Stale),
        "old snapshot must lose CAS after generation swap: {outcome:?}"
    );
    let new = successor?;
    wire.retires(pool, &blocked).await?;
    final_row(pool, old, None, 0).await?;
    final_row(pool, new, None, 0).await?;
    ensure!(venue(pool).await?.1 == Some(new.audit));
    let Pass::Checkpointed(current) =
        pass(pool, new, &trace).await.map_err(|e| anyhow!("{e:?}"))?
    else {
        anyhow::bail!("successor did not checkpoint")
    };
    final_row(pool, current, Some(Uuid::from_u128(5)), 1).await?;
    Ok(())
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn real_generation_change_after_the_old_page_refuses_its_checkpoint(
    pool: PgPool,
) -> anyhow::Result<()> {
    held_old_page(&pool, false).await
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn cancellation_before_checkpoint_keeps_the_previous_audit(
    pool: PgPool,
) -> anyhow::Result<()> {
    held_old_page(&pool, true).await
}

async fn held_commit(pool: &PgPool, cancel: bool) -> anyhow::Result<()> {
    initialize(pool).await?;
    seed(pool, &[(5, "binance:usdm")]).await?;
    let trace = Trace::default();
    let run = start(pool, Uuid::now_v7(), envelope()?, &trace)
        .await
        .map_err(|e| anyhow!("{e:?}"))?;
    let wire = Wire::new(pool, "audit-held-commit").await?;
    let mut hold = pool.begin().await?;
    sqlx::query("SELECT 1 FROM budget_venue_state WHERE venue_id='binance:spot' FOR UPDATE")
        .fetch_one(&mut *hold)
        .await?;
    let holder = holder_pid(&mut hold).await?;
    let mut work = Box::pin(pass(&wire.pool, run, &trace));
    let original = tokio::select! {
        result = &mut work => anyhow::bail!("checkpoint ended before lock probe: {result:?}"),
        result = wire.blocked(pool, holder, 4) => result?,
    };
    let mut control = wire.proxy.control(original.identity, false).map_err(|e| anyhow!("{e:?}"))?;
    let target = original.statement + u64::from(!cancel);
    let mut gate = control
        .arm(target, ControlBoundary::BeforeCommandComplete)
        .map_err(|e| anyhow!("{e:?}"))?;
    hold.rollback().await?;
    let hit = tokio::select! {
        result = &mut work => anyhow::bail!("audit finished before COMMIT gate: {result:?}"),
        result = timeout(Duration::from_millis(350), gate.held()) => result?.map_err(|e| anyhow!("{e:?}"))?,
    };
    ensure!(
        hit.record.identity == original.identity
            && hit.record.ordinal == Some(target)
            && hit.record.event == DiagnosticEvent::Complete
    );
    let (cursor, count) = if cancel {
        (None, 0)
    } else {
        (Some(Uuid::from_u128(5)), 1)
    };
    final_row(pool, run, cursor, count).await?;
    if cancel {
        // The checkpoint executed, but COMMIT could not be sent before its completion.
        drop(work);
    } else {
        let actual = work.await;
        ensure!(
            matches!(actual, Err(Fault { phase: Phase::Commit, audit: Some(id), .. }) if id == run.audit),
            "COMMIT result was uncertain, with no usable cursor: {actual:?}"
        );
    }
    wire.retires(pool, &original).await?;
    let observed = closed_by_client(&gate).await?;
    final_row(pool, run, cursor, count).await?;
    let mut locks = pool.begin().await?;
    sqlx::query("SELECT 1 FROM budget_venue_state WHERE venue_id='binance:spot' FOR UPDATE NOWAIT")
        .fetch_one(&mut *locks)
        .await?;
    sqlx::query("SELECT 1 FROM budget_source_audits WHERE audit_id=$1 FOR UPDATE NOWAIT")
        .bind(run.audit)
        .fetch_one(&mut *locks)
        .await?;
    locks.rollback().await?;
    let trace = wire.proxy.query_trace();
    ensure!(wire.proxy.query_trace_dropped() == 0 && trace.len() == target as usize);
    let commit_sent = trace
        .iter()
        .any(|text| text.lines().any(|sql| sql.trim().trim_end_matches(';') == "COMMIT"));
    ensure!(commit_sent == !cancel, "audit trace through original boundary: {trace:?}");
    drop(gate);
    ensure!(control.close().await.map_err(|e| anyhow!("{e:?}"))? == observed);
    Ok(())
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn cancellation_after_checkpoint_before_commit_rolls_back_without_cleanup_sql(
    pool: PgPool,
) -> anyhow::Result<()> {
    held_commit(&pool, true).await
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn withheld_real_checkpoint_ack_is_durable_and_unpublished(
    pool: PgPool,
) -> anyhow::Result<()> {
    held_commit(&pool, false).await
}
