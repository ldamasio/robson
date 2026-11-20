//! Native server statement 1s / lock 200ms timers on the ORIGINAL metadata
//! await, no client fault timer. The test trigger sleeps 2s, twice the 1s
//! statement_timeout (the same 2x margin as the former 80ms/40ms).
use super::*;

const IDENTITY: &str = "SELECT jsonb_build_object('backend',(SELECT backend_start::text FROM pg_stat_activity WHERE pid=$1),'metadata','public._sqlx_migrations'::regclass::oid::bigint,'venue','public.venue_orders'::regclass::oid::bigint,'clock','public.spot_intent_clock'::regclass::oid::bigint)";
const OBSERVE: &str = "SELECT EXISTS(SELECT 1 FROM pg_stat_activity a WHERE a.pid=$1 AND a.backend_start::text=$2 AND a.state='active' AND EXISTS(SELECT 1 FROM pg_locks WHERE pid=$1 AND relation::bigint=$5 AND mode='AccessExclusiveLock' AND granted) AND EXISTS(SELECT 1 FROM pg_locks WHERE pid=$1 AND relation::bigint=$6 AND mode='AccessExclusiveLock' AND granted) AND (($4::integer IS NULL AND a.wait_event_type='Timeout' AND a.wait_event='PgSleep' AND EXISTS(SELECT 1 FROM pg_locks WHERE pid=$1 AND relation::bigint=$3 AND mode='RowExclusiveLock' AND granted)) OR ($4::integer IS NOT NULL AND a.wait_event_type='Lock' AND $4=ANY(pg_blocking_pids(a.pid)) AND EXISTS(SELECT 1 FROM pg_locks WHERE pid=$1 AND relation::bigint=$3 AND mode='RowExclusiveLock' AND NOT granted))))";
const SLEEP_FUNCTION: &str = "CREATE FUNCTION public.test_metadata_statement40() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.version=20240101000038 THEN PERFORM pg_sleep(2.0); END IF; RETURN NEW; END $$";
const SLEEP_TRIGGER: &str = "CREATE TRIGGER test_metadata_statement40 BEFORE INSERT ON public._sqlx_migrations FOR EACH ROW EXECUTE FUNCTION public.test_metadata_statement40()";

#[derive(Clone, Copy, Debug)]
enum Timer {
    Statement40,
    Lock20,
}
impl Timer {
    fn state(self) -> &'static str {
        match self {
            Self::Statement40 => "57014",
            Self::Lock20 => "55P03",
        }
    }
}

async fn native(c: &mut PgConnection, pid: i32, id: &Value, blocker: Option<i32>) -> Result<bool> {
    Ok(sqlx::query_scalar(OBSERVE)
        .bind(pid)
        .bind(id["backend"].as_str().context("native timer backend identity")?)
        .bind(id["metadata"].as_i64().context("native timer metadata OID")?)
        .bind(blocker)
        .bind(id["venue"].as_i64().context("native timer venue OID")?)
        .bind(id["clock"].as_i64().context("native timer clock OID")?)
        .fetch_one(c)
        .await?)
}

fn sqlstate(error: &anyhow::Error) -> Option<String> {
    error.chain().find_map(|e| match e.downcast_ref::<sqlx::Error>() {
        Some(sqlx::Error::Database(db)) => db.code().map(|s| s.into_owned()),
        _ => None,
    })
}

struct Witness {
    windows: Vec<Window>,
    released: Instant,
    polls: usize,
}

async fn measure(f: &mut Fixture, id: &Value, blocker: Option<i32>, timer: Timer) -> Result<()> {
    let points = [
        (Stage::Ddl(16), Edge::AfterReturn),
        (Stage::Ddl(32), Edge::AfterReturn),
        (Stage::Ddl(48), Edge::AfterReturn),
        (Stage::Metadata, Edge::BeforeCall),
    ];
    let slot = f.adapter.gate.clone();
    let first = stages::arm(&slot, points[0].0, points[0].1)?;
    let proxy = &f.proxy;
    let observer = &mut f.observer;
    let pid = f.pid;
    let baseline = f.baseline;
    let controller = async {
        let mut current = first;
        let mut windows = Vec::with_capacity(4);
        for n in 0..4 {
            current.0.await.map_err(|_| anyhow::anyhow!("native timer stage absent"))?;
            if n < 3 {
                windows.push(Window {
                    text: proxy.query_trace(),
                    dropped: proxy.query_trace_dropped(),
                    statements: proxy.stats().statements - baseline.statements,
                });
                let next = stages::arm(&slot, points[n + 1].0, points[n + 1].1)?;
                proxy.enable_query_trace(32);
                current
                    .1
                    .send(())
                    .map_err(|_| anyhow::anyhow!("native timer milestone release lost"))?;
                current = next;
            } else {
                ensure!(
                    proxy.stats().statements - baseline.statements == 54,
                    "native timer before-metadata prefix"
                );
                let released = Instant::now();
                current
                    .1
                    .send(())
                    .map_err(|_| anyhow::anyhow!("native timer metadata release lost"))?;
                for polls in 1..=4 {
                    let active = native(observer, pid, id, blocker).await?;
                    let statements = proxy.stats().statements - baseline.statements;
                    ensure!(statements <= 55, "native timer sent later SQL");
                    if active && statements == 55 {
                        windows.push(Window {
                            text: proxy.query_trace(),
                            dropped: proxy.query_trace_dropped(),
                            statements,
                        });
                        return Ok::<_, anyhow::Error>(Witness { windows, released, polls });
                    }
                    if polls < 4 {
                        tokio::time::sleep(Duration::from_millis(5)).await;
                    }
                }
                anyhow::bail!("native timer PgSleep/Lockwait witness absent")
            }
        }
        anyhow::bail!("native timer selected stage absent")
    };
    let started = Instant::now();
    let (apply, observed, returned) = {
        let mut original = f.adapter.apply(migration());
        let mut controller = Box::pin(controller);
        let (early, observed) = tokio::select! {
            result = &mut original => (Some(result), None),
            observed = &mut controller => (None, Some(observed)),
        };
        drop(controller);
        let observed = observed.unwrap_or_else(|| {
            Err(anyhow::anyhow!("original returned before native timer witness"))
        });
        let result = match early {
            Some(result) => result,
            None => (&mut original).await,
        };
        let returned = Instant::now();
        drop(original); // Normal SQLx error already retires the whole raw owner.
        (result.map_err(anyhow::Error::from), observed, returned)
    };
    let elapsed = returned.duration_since(started);
    let status = f.adapter.status();
    let stats = proxy.stats();
    let actual = apply.as_ref().err().and_then(sqlstate);
    let refused = if status.phase == Phase::Retired {
        refuses_after_retirement(&mut f.adapter).await
    } else {
        false // Unexpected success must not poll any later migration.
    };
    let after = proxy.stats();
    eprintln!("NATIVE_TIMER_RESULT timer={timer:?} sqlstate={actual:?} apply_cause={} witness_cause={} elapsed={elapsed:?} status={status:?} prefix={} commits={}",
        cause(&apply), cause(&observed), stats.statements - baseline.statements, stats.commits - baseline.commits);
    let witness = observed?;
    ensure!(actual.as_deref() == Some(timer.state()), "native timer wrong actual SQLSTATE");
    ensure!(elapsed < APPLY_BOUND, "global APPLY_BOUND masked native timer");
    ensure!(
        status.phase == Phase::Retired
            && status.failure
                == Some(Failure {
                    at: Phase::DelegatedApply,
                    outcome: Outcome::MayHaveCommitted
                }),
        "native timer outcome"
    );
    ensure!(
        refused
            && after == stats
            && proxy.mode() == ProxyMode::Pass
            && stats.statements - baseline.statements == 55
            && stats.commits == baseline.commits
            && stats.commit_acks_dropped == baseline.commit_acks_dropped
            && proxy.autocommit_acks_dropped() == 0,
        "native timer refusal/39/laterSQL/ACK/prefix"
    );
    ensure!(
        slot.lock().map_err(|_| anyhow::anyhow!("gate poisoned"))?.is_none(),
        "native timer app gate survived"
    );
    ensure!(witness.windows.len() == 4, "native timer window count");
    let mut received = Vec::with_capacity(55);
    for (n, w) in witness.windows.iter().enumerate() {
        ensure!(
            w.dropped == 0
                && w.statements == [18, 34, 50, 55][n]
                && w.text.len() == [18, 16, 16, 5][n],
            "native timer received trace incomplete"
        );
        received.extend(w.text.iter().map(|s| s.trim().trim_end_matches(';').trim()));
    }
    let chunks = fixed52::chunks(migration())?;
    let expected: Vec<_> = chunks.iter().map(|s| s.trim().trim_end_matches(';').trim()).collect();
    ensure!(
        received.len() == 55
            && received[2..54] == expected
            && received[1] == "BEGIN"
            && received[0].starts_with("WITH old AS MATERIALIZED")
            && received[54] == public_apply::metadata_insert_sql_for_tests().trim(),
        "native timer original bytes/order"
    );
    eprintln!("NATIVE_TIMER_WITNESS timer={timer:?} samePID_backend=true nativeActive=true polls={} metadata_to_error={:?} client_elapsed={elapsed:?}",
        witness.polls, returned.duration_since(witness.released));
    Ok(())
}

async fn run(pool: PgPool, timer: Timer) -> Result<()> {
    let mut f = Fixture::new(&pool).await?;
    let (mut blocker, mut blocker_pid, mut baseline) = (None, None, None);
    let primary = tokio::time::timeout(APPLY_FIXTURE, async {
        // Test-owned schema is in B0; it is not part of original38's DDL.
        if matches!(timer, Timer::Statement40) {
            sqlx::query(SLEEP_FUNCTION).persistent(false).execute(&mut f.observer).await?;
            sqlx::query(SLEEP_TRIGGER).persistent(false).execute(&mut f.observer).await?;
        }
        let old = f.proxy.stats();
        f.adapter.lock().await?;
        ensure!(f.proxy.stats().statements - old.statements == 2, "native timer advisory inventory");
        let template = public_apply::metadata_insert_sql_for_tests();
        let old = f.proxy.stats();
        let c = f.adapter.connection()?;
        let cache = c.cached_statements_size();
        let prepared = (&mut *c).prepare(template).await?;
        let parameters = match prepared.parameters() {
            Some(sqlx::Either::Left(types)) => types.len(), Some(sqlx::Either::Right(n)) => n,
            None => anyhow::bail!("native timer metadata parameters absent"),
        };
        ensure!(parameters == 3 && prepared.sql() == template && prepared.columns().is_empty(), "native timer original prepare identity");
        drop(prepared);
        ensure!(c.cached_statements_size() == cache + 1 && f.proxy.stats() == old, "native timer prepare Q/E/cache drift");
        baseline = Some(capture(&mut f.observer, "B0").await?);
        ensure!(baseline.as_ref().is_some_and(|v| v["context"]["metadata"].is_null()), "native timer38 already present");
        let id: Value = sqlx::query_scalar(IDENTITY).bind(f.pid).fetch_one(&mut f.observer).await?;
        ensure!(id["backend"].is_string(), "native timer backend identity absent");
        if matches!(timer, Timer::Lock20) {
            blocker = Some(pool.connect_options().connect().await?);
            let c = blocker.as_mut().context("native timer blocker absent")?;
            blocker_pid = Some(sqlx::query_scalar("SELECT pg_backend_pid()").fetch_one(&mut *c).await?);
        }
        ensure!(!native(&mut f.observer, f.pid, &id, blocker_pid).await?, "unexpected native timer activity before apply");
        let gone: bool = sqlx::query_scalar(GONE).bind(f.pid).bind(blocker_pid).fetch_one(&mut f.observer).await?;
        ensure!(!gone, "native timer original/blocker identity absent");
        if let Some(c) = blocker.as_mut() {
            sqlx::query("BEGIN").persistent(false).execute(&mut *c).await?;
            sqlx::query("LOCK TABLE public._sqlx_migrations IN SHARE MODE").persistent(false).execute(&mut *c).await?;
        }
        f.baseline = f.proxy.stats();
        f.proxy.enable_query_trace(32);
        eprintln!("NATIVE_TIMER_SETUP timer={timer:?} advisorySQL=2 prepareQE=0 parameters=3 cache=1 observerSetup=4 triggerSQL={} blockerSQL={}",
            usize::from(matches!(timer, Timer::Statement40)) * 2, usize::from(blocker_pid.is_some()) * 3);
        measure(&mut f, &id, blocker_pid, timer).await
    }).await.context("native timer primary deadline").and_then(|r| r);
    let before = f.proxy.stats();
    let cleanup = match timer {
        Timer::Statement40 => super::super::finish(f, baseline.as_ref(), before).await,
        Timer::Lock20 => finish_both(f, blocker, blocker_pid, baseline.as_ref(), before).await,
    };
    eprintln!(
        "NATIVE_TIMER_CLEANUP timer={timer:?} primary_cause={} cleanup_cause={}",
        cause(&primary),
        cause(&cleanup)
    );
    primary?;
    cleanup?;
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires DATABASE_URL"]
async fn actual52_metadata_statement40_retires_and_preserves_full_baseline(
    pool: PgPool,
) -> Result<()> {
    run(pool, Timer::Statement40).await
}

#[sqlx::test(migrations = false)]
#[ignore = "requires DATABASE_URL"]
async fn actual52_metadata_lock20_retires_and_preserves_full_baseline(pool: PgPool) -> Result<()> {
    run(pool, Timer::Lock20).await
}
