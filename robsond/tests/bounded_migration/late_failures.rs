//! Real metadata/restore failures; no synthetic migration or wire authority.
use std::time::{Duration, Instant};

use anyhow::{ensure, Context, Result};
use robson_testkit::db_proxy::ProxyMode;
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

#[derive(Clone, Copy, Debug)]
enum Fault {
    DeferredCommit,
    ElapsedUpdate,
    RestoreSever,
}
impl Fault {
    fn prefix(self) -> u64 {
        if matches!(self, Self::DeferredCommit) {
            56
        } else {
            57
        }
    }
    fn stage(self) -> Stage {
        match self {
            Self::DeferredCommit => Stage::Commit,
            Self::ElapsedUpdate => Stage::Elapsed,
            Self::RestoreSever => Stage::Restore,
        }
    }
    fn applied(self) -> bool {
        !matches!(self, Self::DeferredCommit)
    }
}
#[path = "late_failure_oracles.rs"]
mod oracles;
use oracles::{cleanup, committed, database_code, error_class, post_fault, prepare, Oracle};

async fn exercise(f: &mut Fixture, fault: Fault, oracle: &mut Oracle) -> Result<()> {
    let slot = f.adapter.gate.clone();
    let points = [
        Stage::Ddl(16),
        Stage::Ddl(32),
        Stage::Ddl(48),
        fault.stage(),
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
            current.0.await.map_err(|_| anyhow::anyhow!("late fault stage missing"))?;
            if n < 3 {
                windows.push(Window {
                    text: proxy.query_trace(),
                    dropped: proxy.query_trace_dropped(),
                    statements: proxy.stats().statements - baseline.statements,
                });
                let edge = if n == 2 {
                    Edge::BeforeCall
                } else {
                    Edge::AfterReturn
                };
                let next = stages::arm(&slot, points[n + 1], edge)?;
                proxy.enable_query_trace(32);
                current.1.send(()).map_err(|_| anyhow::anyhow!("late window release lost"))?;
                current = next;
            } else {
                let expected = match fault {
                    Fault::DeferredCommit => 55,
                    Fault::ElapsedUpdate => 56,
                    Fault::RestoreSever => 57,
                };
                ensure!(
                    proxy.stats().statements - baseline.statements == expected,
                    "fault armed at wrong real call"
                );
                if fault.applied() {
                    oracle.committed_at_gate =
                        committed(observer, pid, &oracle.backend_start, fault).await?;
                    ensure!(
                        oracle.committed_at_gate,
                        "committed original38/PID/backend/outsideTX witness absent"
                    );
                }
                if matches!(fault, Fault::RestoreSever) {
                    proxy.set_mode(ProxyMode::Sever);
                    // Observe actual upstream backend death BEFORE polling restore.
                    tokio::time::timeout(Duration::from_millis(20), async {
                        for n in 0..4 {
                            if !alive(observer, pid).await? {
                                return Ok::<_, anyhow::Error>(());
                            }
                            if n < 3 {
                                tokio::time::sleep(Duration::from_millis(5)).await;
                            }
                        }
                        anyhow::bail!("Sever did not retire actual original PID")
                    })
                    .await
                    .context("restore Sever observation")??;
                }
                current.1.send(()).map_err(|_| anyhow::anyhow!("fault call release lost"))?;
                return Ok::<_, anyhow::Error>(windows);
            }
        }
        anyhow::bail!("fault stage unreachable")
    };
    let started = Instant::now();
    let producer = async {
        let result = f.adapter.apply(migration()).await;
        (result, started.elapsed())
    };
    let ((result, duration), observed) =
        tokio::join!(producer, tokio::time::timeout(Duration::from_millis(300), controller));
    let joined = started.elapsed();
    let status = f.adapter.status();
    let stats = f.proxy.stats();
    // Never issue refusal probes against an unexpectedly successful owner.
    let refused = if result.is_err() {
        refuses_after_retirement(&mut f.adapter).await
    } else {
        false
    };
    let after = f.proxy.stats();
    eprintln!("LATE_FAULT fault={fault:?} apply_ok={} actual_apply_return={duration:?} joined={joined:?} status={status:?} received={} commits={} observer_ok={}", result.is_ok(), stats.statements-baseline.statements, stats.commits-baseline.commits, observed.as_ref().is_ok_and(|v| v.is_ok()));
    let error = result.as_ref().err().context("fault unexpectedly succeeded")?;
    let code = database_code(error).filter(|s| {
        s.len() == 5 && s.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
    });
    eprintln!(
        "LATE_FAULT_ERROR class={} sqlstate={} actual_apply_return={duration:?} joined={joined:?}",
        error_class(error),
        code.as_deref().unwrap_or("none")
    );
    let mut windows = observed.context("fault controller ceiling")??;
    windows.push(Window {
        text: f.proxy.query_trace(),
        dropped: f.proxy.query_trace_dropped(),
        statements: stats.statements - baseline.statements,
    });
    ensure!(
        duration < APPLY_BOUND,
        "required real-fault apply return exceeded controlled200ms"
    );
    match fault {
        Fault::RestoreSever => ensure!(
            matches!(error, MigrateError::Execute(sqlx::Error::Io(_))),
            "restore did not return actual I/O error"
        ),
        _ => ensure!(
            database_code(error).as_deref() == Some("23514"),
            "expected actual trigger23514"
        ),
    }
    let failure = if matches!(fault, Fault::RestoreSever) {
        Failure {
            at: Phase::RestoreStarted,
            outcome: Outcome::AppliedButSessionUnrestored,
        }
    } else {
        Failure {
            at: Phase::DelegatedApply,
            outcome: Outcome::MayHaveCommitted,
        }
    };
    ensure!(
        status.phase == Phase::Retired && status.failure == Some(failure),
        "wrong conservative runtime outcome"
    );
    ensure!(refused && after == stats, "retired entry sent SQL or39");
    ensure!(
        stats.statements - baseline.statements == fault.prefix()
            && stats.commits - baseline.commits == 1,
        "wrong failure prefix/commit count"
    );
    ensure!(
        stats.commit_acks_dropped == baseline.commit_acks_dropped
            && f.proxy.autocommit_acks_dropped() == 0,
        "unexpected ACK fault"
    );
    ensure!(
        windows.iter().map(|w| w.statements).eq([18, 34, 50, fault.prefix()]),
        "cumulative failure windows drift"
    );
    let chunks = fixed52::chunks(migration())?;
    let all: Vec<_> = windows.iter().flat_map(|w| &w.text).collect();
    let lengths = [18, 16, 16, usize::try_from(fault.prefix() - 50)?];
    ensure!(
        windows.len() == 4
            && windows.iter().zip(lengths).all(|(w, n)| w.text.len() == n && w.dropped == 0),
        "bounded windows incomplete"
    );
    ensure!(
        all.len() == usize::try_from(fault.prefix())?
            && all[0].starts_with("WITH old AS MATERIALIZED")
            && all[1].trim() == "BEGIN",
        "full failure inventory missing BEGIN"
    );
    for (actual, expected) in all[2..54].iter().zip(chunks) {
        ensure!(actual.trim() == expected.trim(), "original52 bytes/order drift");
    }
    ensure!(
        all[54].trim() == public_apply::metadata_insert_sql_for_tests().trim(),
        "metadata literal drift"
    );
    ensure!(all[55].trim() == "COMMIT", "original COMMIT missing");
    if fault.applied() {
        ensure!(all[56].contains("UPDATE _sqlx_migrations"), "elapsed call missing");
    }
    Ok(())
}

async fn run(pool: PgPool, fault: Fault) -> Result<()> {
    let mut f = Fixture::new(&pool).await?;
    let mut oracle = None;
    let primary = tokio::time::timeout(ACQUIRE_BOUND, async {
        oracle = Some(prepare(&mut f, fault).await?);
        let exercised = exercise(&mut f, fault, oracle.as_mut().context("oracle missing")?).await;
        let post = post_fault(&mut f,oracle.as_mut().context("oracle missing")?,fault).await;
        eprintln!("LATE_POST_FAULT_ORACLE point=AFTER_APPLY_RETURN_BEFORE_CLEANUP_BARRIER exercised={} full={}",cause(&exercised),cause(&post));
        exercised?;
        post
    })
    .await
    .context("late failure primary ceiling")
    .and_then(|r| r);
    let cleanup = cleanup(f, oracle.as_ref(), fault).await;
    eprintln!(
        "LATE_FAULT_RESULT fault={fault:?} primary={} cleanup={}",
        cause(&primary),
        cause(&cleanup)
    );
    primary?;
    cleanup?;
    Ok(())
}
#[sqlx::test(migrations = false)]
#[ignore = "requires DATABASE_URL"]
async fn actual52_deferred_commit_rejection_rolls_back_full_baseline(pool: PgPool) -> Result<()> {
    run(pool, Fault::DeferredCommit).await
}
#[sqlx::test(migrations = false)]
#[ignore = "requires DATABASE_URL"]
async fn actual52_elapsed_update_error_keeps_committed_original38(pool: PgPool) -> Result<()> {
    run(pool, Fault::ElapsedUpdate).await
}
#[sqlx::test(migrations = false)]
#[ignore = "requires DATABASE_URL"]
async fn actual52_restore_sever_keeps_committed_original38(pool: PgPool) -> Result<()> {
    run(pool, Fault::RestoreSever).await
}
