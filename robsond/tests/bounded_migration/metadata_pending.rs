//! One actual metadata I/O wait; no server-success/response-loss claim.
#[path = "metadata_client_deadlines.rs"]
mod client_deadlines;
#[path = "metadata_native_timeouts.rs"]
mod native_timeouts;
#[path = "metadata_response_loss.rs"]
mod server_success;
use sqlx::{ConnectOptions, Connection, Executor, PgConnection, PgPool, Statement};

use super::*;

const WAIT: &str = "SELECT EXISTS(SELECT 1 FROM pg_stat_activity a WHERE a.pid=$1 AND a.state='active' AND a.wait_event_type='Lock' AND EXISTS(SELECT 1 FROM pg_locks l WHERE l.pid=a.pid AND l.relation::bigint=$2 AND l.mode='RowExclusiveLock' AND NOT l.granted) AND $3=ANY(pg_blocking_pids(a.pid)))";
const GONE: &str = "SELECT NOT EXISTS(SELECT 1 FROM pg_stat_activity WHERE pid=$1 OR pid=$2)";

async fn waiting(c: &mut PgConnection, pid: i32, relation: i64, blocker: i32) -> Result<bool> {
    Ok(sqlx::query_scalar(WAIT)
        .bind(pid)
        .bind(relation)
        .bind(blocker)
        .fetch_one(c)
        .await?)
}

struct Witness {
    windows: Vec<Window>,
    released: Instant,
    polls: usize,
}

async fn cancel_in_io(f: &mut Fixture, relation: i64, blocker: i32) -> Result<()> {
    let slot = f.adapter.gate.clone();
    let points = [
        (Stage::Ddl(16), Edge::AfterReturn),
        (Stage::Ddl(32), Edge::AfterReturn),
        (Stage::Ddl(48), Edge::AfterReturn),
        (Stage::Metadata, Edge::BeforeCall),
    ];
    let first = stages::arm(&slot, points[0].0, points[0].1)?;
    let baseline = f.baseline;
    let pid = f.pid;
    let proxy = &f.proxy;
    let observer = &mut f.observer;
    let controller = async {
        let mut current = first;
        let mut windows = Vec::with_capacity(4);
        for n in 0..4 {
            current.0.await.map_err(|_| anyhow::anyhow!("lockwait stage hit missing"))?;
            if n < 3 {
                windows.push(Window {
                    text: proxy.query_trace(),
                    dropped: proxy.query_trace_dropped(),
                    statements: proxy.stats().statements - baseline.statements,
                });
                let next = stages::arm(&slot, points[n + 1].0, points[n + 1].1)?;
                proxy.enable_query_trace(32);
                current.1.send(()).map_err(|_| anyhow::anyhow!("lockwait stage release lost"))?;
                current = next;
            } else {
                ensure!(
                    proxy.stats().statements - baseline.statements == 54,
                    "metadata gate did not follow original52"
                );
                let clear = {
                    let guard = slot.lock().map_err(|_| anyhow::anyhow!("gate poisoned"))?;
                    guard.is_none()
                };
                ensure!(clear, "an app gate remained armed before metadata I/O");
                let released = Instant::now();
                current.1.send(()).map_err(|_| anyhow::anyhow!("metadata release lost"))?;
                // 20 ms observation window, kept strictly inside lock_timeout (200ms).
                let deadline = tokio::time::Instant::from_std(released) + Duration::from_millis(20);
                return tokio::time::timeout_at(deadline, async {
                    for polls in 1..=4 {
                        let live = waiting(observer, pid, relation, blocker).await?;
                        let statements = proxy.stats().statements - baseline.statements;
                        ensure!(statements <= 55, "metadata wait sent later SQL");
                        if live && statements == 55 {
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
                    anyhow::bail!("actual metadata lock wait not observed")
                })
                .await
                .context("metadata 20ms lock observation window expired")?;
            }
        }
        anyhow::bail!("metadata gate not reached")
    };
    let started = Instant::now();
    let (observed, probe, dropped_at) = {
        let mut original = f.adapter.apply(migration());
        tokio::pin!(controller);
        let observed = tokio::select! {
            result = &mut original => match result {
                Err(error) => Err(anyhow::Error::from(error).context("original returned before live lockwait")),
                Ok(_) => Err(anyhow::anyhow!("original unexpectedly applied before live lockwait")),
            },
            result = &mut controller => result,
        };
        let probe = if observed.is_ok() {
            std::future::poll_fn(|cx| {
                std::task::Poll::Ready(match std::future::Future::poll(original.as_mut(), cx) {
                    std::task::Poll::Pending => Ok(true),
                    std::task::Poll::Ready(Err(error)) => {
                        Err(anyhow::Error::from(error).context("lock timer won before Pending"))
                    },
                    std::task::Poll::Ready(Ok(_)) => {
                        Err(anyhow::anyhow!("metadata returned instead of Pending"))
                    },
                })
            })
            .await
        } else {
            Ok(false)
        };
        // ORIGINAL Box drops BEFORE this helper returns or blocker is released.
        drop(original);
        (observed, probe, Instant::now())
    };
    let status = f.adapter.status();
    let stats = f.proxy.stats();
    let refused = refuses_after_retirement(&mut f.adapter).await;
    let after = f.proxy.stats();
    eprintln!("LOCKWAIT_CUT observed={} probe_pending={} elapsed={:?} status={status:?} statements={} commits={}",
        observed.is_ok(), probe.as_ref().is_ok_and(|x| *x), started.elapsed(),
        stats.statements - baseline.statements, stats.commits - baseline.commits);
    let witness = observed?;
    ensure!(probe?, "original was not Pending after all app gates cleared");
    ensure!(
        dropped_at.duration_since(witness.released) < Duration::from_millis(20),
        "20ms window inside lock_timeout missed; no before-timeout cancellation claim"
    );
    ensure!(started.elapsed() < APPLY_BOUND, "APPLY_BOUND won before lockwait cancellation");
    let clear = {
        let guard = slot.lock().map_err(|_| anyhow::anyhow!("gate poisoned"))?;
        guard.is_none()
    };
    ensure!(
        clear && f.proxy.mode() == ProxyMode::Pass,
        "app/transport gate fabricated Pending"
    );
    ensure!(
        status.phase == Phase::Retired
            && status.failure
                == Some(Failure {
                    at: Phase::DelegatedApply,
                    outcome: Outcome::MayHaveCommitted,
                }),
        "lockwait cancellation outcome"
    );
    ensure!(
        refused
            && after == stats
            && stats.statements - baseline.statements == 55
            && stats.commits == baseline.commits
            && stats.commit_acks_dropped == baseline.commit_acks_dropped
            && f.proxy.autocommit_acks_dropped() == 0,
        "lockwait reuse/39/ACK fault or wrong prefix"
    );
    ensure!(witness.windows.len() == 4, "lockwait window count");
    let mut received = Vec::with_capacity(55);
    for (n, window) in witness.windows.iter().enumerate() {
        ensure!(
            window.dropped == 0
                && window.statements == [18, 34, 50, 55][n]
                && window.text.len() == [18, 16, 16, 5][n],
            "lockwait received window incomplete"
        );
        received.extend(window.text.iter().map(|s| s.trim().trim_end_matches(';').trim()));
    }
    ensure!(received.len() == 55, "lockwait received prefix length");
    let chunks = fixed52::chunks(migration())?;
    let expected: Vec<_> = chunks.iter().map(|s| s.trim().trim_end_matches(';').trim()).collect();
    ensure!(
        received[2..54] == expected
            && received[0].starts_with("WITH old AS MATERIALIZED")
            && received[1] == "BEGIN"
            && received[54] == public_apply::metadata_insert_sql_for_tests().trim(),
        "lockwait original SQL bytes/order changed"
    );
    eprintln!("LOCKWAIT_WITNESS native_wait=true app_gate=false pending=true polls={} released_to_drop={:?}",
        witness.polls, dropped_at.duration_since(witness.released));
    Ok(())
}

async fn finish_both(
    f: Fixture,
    blocker: Option<PgConnection>,
    blocker_pid: Option<i32>,
    baseline: Option<&Value>,
    before: ProxyStats,
) -> Result<()> {
    let Fixture {
        adapter,
        proxy,
        mut observer,
        pid,
        legacy_before,
        ..
    } = f;
    let deadline = tokio::time::Instant::now() + ACQUIRE_BOUND;
    // The original Box is already gone. No graceful close or queued ROLLBACK.
    drop(adapter);
    drop(blocker); // Releases fixture SHARE only AFTER the original owner drop.
    let retired = tokio::time::timeout(NATIVE_RETIREMENT, async {
        for _ in 0..NATIVE_RETIREMENT_POLLS {
            let gone: bool = sqlx::query_scalar(GONE)
                .bind(pid)
                .bind(blocker_pid)
                .fetch_one(&mut observer)
                .await?;
            if gone {
                return Ok::<_, sqlx::Error>(true);
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        Ok(false)
    })
    .await
    .context("lockwait dual retirement deadline")
    .and_then(|r| r.map_err(Into::into));
    let end = proxy.stats();
    let dropped = proxy.query_trace_dropped();
    proxy.set_mode(ProxyMode::Sever);
    drop(proxy);
    let snapshots = tokio::time::timeout_at(deadline, async {
        observer.lock().await?;
        let full = capture(&mut observer, "B1").await?;
        let legacy = legacy(&mut observer).await?;
        let metadata = metadata(&mut observer).await?;
        observer.unlock().await?;
        Ok::<_, anyhow::Error>((full, legacy, metadata))
    })
    .await
    .context("lockwait serialized snapshots deadline")
    .and_then(|r| r);
    drop(observer);
    eprintln!(
        "LOCKWAIT_CLEANUP retired_both={:?} retirement_cause={} snapshots_cause={} statements={}",
        retired.as_ref().ok(),
        cause(&retired),
        cause(&snapshots),
        end.statements
    );
    ensure!(retired? && blocker_pid.is_some(), "both actual backend retirements not proved");
    let (full, legacy, metadata) = snapshots?;
    ensure!(
        end == before && dropped == 0,
        "lockwait retirement flushed SQL or trace dropped"
    );
    let (old, old_ids) = contract::maps(baseline.context("lockwait baseline missing")?)?;
    let (now, now_ids) = contract::maps(&full)?;
    ensure!(
        old == now && old_ids == now_ids && legacy == legacy_before && metadata.is_none(),
        "lockwait rollback changed complete catalog/legacy or landed38"
    );
    contract::source_hash()?;
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires DATABASE_URL"]
async fn actual52_metadata_lock_wait_cancel_preserves_full_baseline(pool: PgPool) -> Result<()> {
    let mut f = Fixture::new(&pool).await?;
    let mut blocker = None;
    let mut blocker_pid = None;
    let mut baseline = None;
    let primary = tokio::time::timeout(APPLY_FIXTURE, async {
        let old = f.proxy.stats();
        f.adapter.lock().await?;
        ensure!(f.proxy.stats().statements - old.statements == 2, "original advisory setup inventory");
        let template = public_apply::metadata_insert_sql_for_tests();
        let old = f.proxy.stats();
        let c = f.adapter.connection()?;
        let cache = c.cached_statements_size();
        let prepared = (&mut *c).prepare(template).await?;
        let count = match prepared.parameters() {
            Some(sqlx::Either::Left(types)) => types.len(),
            Some(sqlx::Either::Right(count)) => count,
            None => anyhow::bail!("public metadata parameter description missing"),
        };
        ensure!(count == 3 && prepared.sql() == template && prepared.columns().is_empty(),
            "original metadata public preparation identity/parameters");
        drop(prepared);
        ensure!(c.cached_statements_size() == cache + 1 && f.proxy.stats() == old,
            "metadata preparation executed SQL or failed cache insertion");
        baseline = Some(capture(&mut f.observer, "B0").await?);
        ensure!(baseline.as_ref().is_some_and(|v| v["context"]["metadata"].is_null()), "38 already applied");
        let relation: i64 = sqlx::query_scalar("SELECT 'public._sqlx_migrations'::regclass::oid::bigint")
            .fetch_one(&mut f.observer).await?;
        blocker = Some(pool.connect_options().connect().await?);
        let c = blocker.as_mut().context("blocker unavailable")?;
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()").fetch_one(&mut *c).await?;
        blocker_pid = Some(pid);
        ensure!(!waiting(&mut f.observer, f.pid, relation, pid).await?, "unexpected initial lock wait");
        let gone: bool = sqlx::query_scalar(GONE).bind(f.pid).bind(pid).fetch_one(&mut f.observer).await?;
        ensure!(!gone, "warm live backend identity missing");
        sqlx::query("BEGIN").persistent(false).execute(&mut *c).await?;
        sqlx::query("LOCK TABLE public._sqlx_migrations IN SHARE MODE").persistent(false).execute(&mut *c).await?;
        f.baseline = f.proxy.stats();
        f.proxy.enable_query_trace(32);
        eprintln!("LOCKWAIT_SETUP original_advisory_SQL=2 prepare_QE=0 parameters=3 cache_delta=1 blocker_SQL=3 observer_setup_SQL=4");
        cancel_in_io(&mut f, relation, pid).await
    }).await.context("lockwait primary fixture deadline").and_then(|r| r);
    let before = f.proxy.stats();
    let cleanup = finish_both(f, blocker, blocker_pid, baseline.as_ref(), before).await;
    eprintln!(
        "LOCKWAIT_RESULT primary_cause={} cleanup_cause={}",
        cause(&primary),
        cause(&cleanup)
    );
    primary?;
    cleanup?;
    Ok(())
}
