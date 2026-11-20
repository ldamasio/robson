//! An absence snapshot precedes the SAME original transaction's late COMMIT.
//! Existing ACK loss is transport failure, never C3/Ready or retry authority.
use std::time::{Duration, Instant};

use anyhow::{ensure, Context, Result};
use robson_testkit::db_proxy::{ProxyMode, ProxyStats};
use serde_json::Value;
use sqlx::{Connection, PgPool};

use super::{
    super::{
        helpers::*,
        stages::{self, Edge, Stage},
        *,
    },
    capture, cause, contract,
};

const SNAPSHOT: &str = "SELECT jsonb_build_object('metadata',(SELECT version FROM _sqlx_migrations WHERE version=20240101000038),'activity',(SELECT jsonb_build_object('pid',pid,'backend',backend_start::text,'transaction',xact_start::text,'xid',backend_xid::text,'state',state) FROM pg_stat_activity WHERE pid=$1),'locks',(SELECT count(*) FROM pg_locks WHERE pid=$1),'advisory',EXISTS(SELECT 1 FROM pg_locks WHERE pid=$1 AND locktype='advisory' AND granted),'metadata_lock',EXISTS(SELECT 1 FROM pg_locks WHERE pid=$1 AND relation='public._sqlx_migrations'::regclass AND mode='RowExclusiveLock' AND granted))";
const RETIRED: &str = "SELECT NOT EXISTS(SELECT 1 FROM pg_stat_activity WHERE pid=$1) AND NOT EXISTS(SELECT 1 FROM pg_locks WHERE pid=$1)";

async fn snapshot(c: &mut sqlx::PgConnection, pid: i32) -> Result<Value> {
    let value: Value = sqlx::query_scalar(SNAPSHOT).bind(pid).fetch_one(c).await?;
    ensure!(serde_json::to_vec(&value)?.len() <= 16384, "absence snapshot bound");
    Ok(value)
}

async fn exercise(f: &mut Fixture, warm: &Value) -> Result<()> {
    let slot = f.adapter.gate.clone();
    let points = [
        Stage::Ddl(16),
        Stage::Ddl(32),
        Stage::Ddl(48),
        Stage::Commit,
    ];
    let first = stages::arm(&slot, points[0], Edge::AfterReturn)?;
    let baseline = f.baseline;
    let pid = f.pid;
    let proxy = &f.proxy;
    let observer = &mut f.observer;
    let controller = async {
        let mut current = first;
        let mut windows = Vec::with_capacity(4);
        for n in 0..4 {
            current.0.await.map_err(|_| anyhow::anyhow!("absence stage hit missing"))?;
            windows.push(Window {
                text: proxy.query_trace(),
                dropped: proxy.query_trace_dropped(),
                statements: proxy.stats().statements - baseline.statements,
            });
            if n < 3 {
                let edge = if n == 2 {
                    Edge::BeforeCall
                } else {
                    Edge::AfterReturn
                };
                let next = stages::arm(&slot, points[n + 1], edge)?;
                proxy.enable_exact_query_trace(32);
                current
                    .1
                    .send(())
                    .map_err(|_| anyhow::anyhow!("absence milestone release lost"))?;
                current = next;
            } else {
                // One warmed, cheap statement only: no full capture under idle/apply.
                let observed_at = Instant::now();
                let absent = snapshot(observer, pid).await?;
                let a = &absent["activity"];
                ensure!(
                    absent["metadata"].is_null()
                        && a["pid"] == pid
                        && a["backend"] == warm["activity"]["backend"]
                        && a["backend"].is_string()
                        && a["transaction"].is_string()
                        && a["xid"].is_string()
                        && a["state"] == "idle in transaction"
                        && absent["locks"].as_u64().is_some_and(|n| n > 0)
                        && absent["advisory"] == true
                        && absent["metadata_lock"] == true,
                    "absence did not coexist with the same live original transaction"
                );
                ensure!(
                    proxy.stats().statements - baseline.statements == 55
                        && proxy.stats().commits == baseline.commits,
                    "absence observed after original COMMIT or wrong metadata prefix"
                );
                ensure!(
                    slot.lock().map_err(|_| anyhow::anyhow!("gate poisoned"))?.is_none(),
                    "app gate remains armed"
                );
                ensure!(proxy.mode() == ProxyMode::Pass, "preexisting transport fault");
                proxy.set_mode(ProxyMode::DropCommitAck);
                current
                    .1
                    .send(())
                    .map_err(|_| anyhow::anyhow!("same original COMMIT release lost"))?;
                eprintln!("LATE_COMMIT_ABSENCE pid={pid} metadata_absent=true same_original_idleTX=true prefix=55 observer_queries=1 observation_us={}", observed_at.elapsed().as_micros());
                return Ok::<_, anyhow::Error>(windows);
            }
        }
        anyhow::bail!("absence cut not reached")
    };
    let started = Instant::now();
    let (result, observed) = tokio::join!(
        f.adapter.apply(migration()),
        tokio::time::timeout(CONTROLLER_CEILING, controller)
    );
    let duration = started.elapsed();
    let status = f.adapter.status();
    let stats = f.proxy.stats();
    // Do not call a probe that can execute SQL against an unexpectedly live owner.
    let refused = if status.phase == Phase::Retired {
        refuses_after_retirement(&mut f.adapter).await
    } else {
        false
    };
    let after_refusals = f.proxy.stats();
    eprintln!("LATE_COMMIT_RESULT apply_ok={} duration={duration:?} status={status:?} controller_ok={} prefix={} commits={} ackdrops={}", result.is_ok(), observed.as_ref().is_ok_and(|r| r.is_ok()), stats.statements-baseline.statements, stats.commits-baseline.commits, stats.commit_acks_dropped-baseline.commit_acks_dropped);
    ensure!(
        result.is_err() && duration < APPLY_BOUND,
        "counterexample lost to success/idle/global deadline; no timing adjustment"
    );
    let mut windows = observed.context("absence controller ceiling")??;
    // The final gate window contains DDL49..52+metadata; COMMIT follows its
    // release.
    ensure!(
        windows.len() == 4 && windows.iter().map(|w| w.statements).eq([18, 34, 50, 55]),
        "absence four actual prefix windows"
    );
    ensure!(
        windows
            .iter()
            .zip([18, 16, 16, 5])
            .all(|(w, n)| w.dropped == 0 && w.text.len() == n),
        "absence bounded trace dropped or incomplete"
    );
    let tail = f.proxy.query_trace();
    ensure!(
        f.proxy.query_trace_dropped() == 0 && tail.len() == 6 && tail[5].trim() == "COMMIT",
        "same original COMMIT trace missing or extra SQL"
    );
    windows[3].text.push(tail[5].clone());
    let received: Vec<_> = windows.iter().flat_map(|w| &w.text).collect();
    ensure!(
        received.len() == 56
            && received[0].starts_with("WITH old AS MATERIALIZED")
            && received[1].trim() == "BEGIN",
        "absence full original inventory"
    );
    for (actual, expected) in received[2..54].iter().zip(fixed52::chunks(migration())?) {
        ensure!(actual.trim() == expected.trim(), "absence original52 byte/order drift");
    }
    ensure!(
        received[54].trim() == public_apply::metadata_insert_sql_for_tests().trim(),
        "absence original metadata literal drift"
    );
    ensure!(
        stats.statements - baseline.statements == 56
            && stats.commits - baseline.commits == 1
            && stats.commit_acks_dropped - baseline.commit_acks_dropped == 1
            && f.proxy.autocommit_acks_dropped() == 0,
        "actual original COMMIT/ACK loss did not occur"
    );
    ensure!(
        status.phase == Phase::Retired
            && status.failure
                == Some(Failure {
                    at: Phase::DelegatedApply,
                    outcome: Outcome::MayHaveCommitted,
                })
            && refused
            && after_refusals == stats,
        "absence granted success/reuse or retired entry sent SQL/39"
    );
    Ok(())
}

async fn finish(f: Fixture, reference: Option<&Value>, end_stats: ProxyStats) -> Result<()> {
    let Fixture {
        adapter,
        proxy,
        mut observer,
        pid,
        legacy_before,
        ..
    } = f;
    let deadline = tokio::time::Instant::now() + ACQUIRE_BOUND;
    drop(adapter); // Whole original raw owner before any retirement/barrier query.
    let retired = tokio::time::timeout(NATIVE_RETIREMENT, async {
        for n in 1..=NATIVE_RETIREMENT_POLLS {
            let gone: bool = sqlx::query_scalar(RETIRED).bind(pid).fetch_one(&mut observer).await?;
            if gone {
                return Ok::<_, sqlx::Error>(n);
            }
            if n < NATIVE_RETIREMENT_POLLS {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        }
        Err(sqlx::Error::Protocol("original PID/locks did not retire".into()))
    })
    .await
    .context("absence retirement window")
    .and_then(|r| r.map_err(Into::into));
    let snapshots = if retired.is_ok() {
        tokio::time::timeout_at(deadline, async {
            // Fresh actual public advisory acquisition, then NEW statement snapshots.
            observer.lock().await?;
            let full = capture(&mut observer, "C").await?;
            let old = legacy(&mut observer).await?;
            let m = metadata(&mut observer).await?;
            observer.unlock().await?;
            Ok::<_, anyhow::Error>((full, old, m))
        })
        .await
        .context("absence fresh public barrier/full snapshots5s")
        .and_then(|r| r)
    } else {
        Err(anyhow::anyhow!("retirement unproved; no final diagnosis snapshot"))
    };
    drop(observer);
    let after = proxy.stats();
    let dropped = proxy.query_trace_dropped();
    proxy.set_mode(ProxyMode::Sever);
    drop(proxy);
    eprintln!(
        "LATE_COMMIT_CLEANUP retirement={} snapshots={} polls={:?}",
        cause(&retired),
        cause(&snapshots),
        retired.as_ref().ok()
    );
    retired?;
    let (full, old, m) = snapshots?;
    ensure!(after == end_stats && dropped == 0, "owner retirement flushed hidden SQL");
    ensure!(old == legacy_before, "late original COMMIT changed legacy/history");
    let reference = reference.context("reference not completed")?;
    contract::declared(&full)?;
    let (expected, _) = contract::maps(reference)?;
    let (actual, _) = contract::maps(&full)?;
    ensure!(expected == actual, "late original COMMIT catalog differs from original DO");
    let (description, success, checksum, elapsed) =
        m.context("late original COMMIT was not durable")?;
    ensure!(
        description == migration().description.as_ref()
            && success
            && checksum.as_slice() == migration().checksum.as_ref()
            && elapsed == -1,
        "late original checksum/history/elapsed boundary mismatch"
    );
    contract::source_hash()?;
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires DATABASE_URL"]
async fn actual52_absence_precedes_same_original_late_commit(pool: PgPool) -> Result<()> {
    let mut f = Fixture::new(&pool).await?;
    let mut reference = None;
    let primary = tokio::time::timeout(APPLY_FIXTURE, async {
        contract::source_hash()?;
        f.adapter.lock().await?;
        // Original DO reference and rollback are outside the real APPLY_BOUND apply.
        let r = {
            let mut tx = f.adapter.connection()?.begin().await?;
            contract::run_original(&mut tx).await?;
            let r = capture(&mut tx, "R").await?;
            tx.rollback().await?;
            r
        };
        contract::declared(&r)?;
        reference = Some(r);
        let warm = snapshot(&mut f.observer, f.pid).await?;
        ensure!(
            warm["metadata"].is_null()
                && warm["activity"]["pid"] == f.pid
                && warm["activity"]["backend"].is_string()
                && warm["activity"]["state"] == "idle"
                && warm["activity"]["transaction"].is_null()
                && warm["activity"]["xid"].is_null()
                && warm["advisory"] == true,
            "warm observer/original identity not established"
        );
        f.baseline = f.proxy.stats();
        f.proxy.enable_exact_query_trace(32);
        exercise(&mut f, &warm).await
    })
    .await
    .context("absence setup/experiment ceiling; apply still original")
    .and_then(|r| r);
    let end_stats = f.proxy.stats();
    let cleanup = finish(f, reference.as_ref(), end_stats).await;
    eprintln!("LATE_COMMIT_FINAL primary={} cleanup={}", cause(&primary), cause(&cleanup));
    primary?;
    cleanup?;
    Ok(())
}
