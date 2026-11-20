//! One partition cut end to end: armed app gates, blackhole, bounded
//! observation of the server-side retirement.
use super::*;

// The server idle_in_transaction_session_timeout (1s) retires the partitioned
// backend. 400 polls of at least 5 ms cover at least 2 s, inside APPLY_BOUND
// (formerly 60 polls for the 20 ms idle timer).
const PARTITION_POLLS: usize = 400;

struct Witness {
    windows: Vec<Window>,
    before: Value,
    released: Instant,
}

async fn partition(f: &mut Fixture, identity: &Value, cut: Cut) -> Result<()> {
    ensure!(
        matches!(cut, Cut::AfterDdl(43 | 52) | Cut::AfterMetadata),
        "partition finite cut"
    );
    let spec = plan(cut)?;
    let first = stages::arm(&f.adapter.gate, spec.points[0].0, spec.points[0].1)?;
    let slot = f.adapter.gate.clone();
    let proxy = &f.proxy;
    let observer = &mut f.observer;
    let baseline = f.baseline;
    let pid = f.pid;
    let diagnostic_started = Instant::now(); // Diagnostic clock only; no deadline changes.
    let controller = async {
        let mut current = first;
        let mut windows = Vec::with_capacity(4);
        for n in 0..spec.points.len() {
            current.0.await.map_err(|_| anyhow::anyhow!("partition stage hit missing"))?;
            windows.push(Window {
                text: proxy.query_trace(),
                dropped: proxy.query_trace_dropped(),
                statements: proxy.stats().statements - baseline.statements,
            });
            if n + 1 < spec.points.len() {
                let next = stages::arm(&slot, spec.points[n + 1].0, spec.points[n + 1].1)?;
                proxy.enable_query_trace(32);
                current
                    .1
                    .send(())
                    .map_err(|_| anyhow::anyhow!("partition milestone release lost"))?;
                current = next;
            } else {
                let before = observe(observer, pid, identity).await?;
                idle_report(
                    &before,
                    identity,
                    pid,
                    spec.metadata,
                    "pre",
                    0,
                    diagnostic_started.elapsed(),
                )?;
                ensure!(
                    slot.lock().map_err(|_| anyhow::anyhow!("gate poisoned"))?.is_none(),
                    "partition gate remains armed"
                );
                ensure!(proxy.mode() == ProxyMode::Pass, "partition preexisting fault");
                proxy.set_mode(ProxyMode::Blackhole);
                let released = Instant::now();
                current
                    .1
                    .send(())
                    .map_err(|_| anyhow::anyhow!("partition final release lost"))?;
                return Ok::<_, anyhow::Error>(Witness { windows, before, released });
            }
        }
        anyhow::bail!("partition cut missing")
    };
    let started = Instant::now();
    let (apply, duration, server, witness) = {
        let mut original = f.adapter.apply(migration());
        let mut controller = Box::pin(controller);
        let witness = tokio::select! {
            result = &mut original => {
                drop(original);
                return result.map_err(anyhow::Error::from)
                    .and_then(|_| Err(anyhow::anyhow!("partition returned success before cut")))
                    .context("original returned before partition witness");
            },
            result = &mut controller => result,
        }?;
        drop(controller);
        // No armed or in-flight app gate remains after its release is consumed.
        let early = std::future::poll_fn(|cx| {
            std::task::Poll::Ready(match std::future::Future::poll(original.as_mut(), cx) {
                std::task::Poll::Pending => None,
                std::task::Poll::Ready(result) => Some(result),
            })
        })
        .await;
        eprintln!(
            "PARTITION_PENDING cut={cut:?} observed={} elapsed={:?}",
            early.is_none(),
            started.elapsed()
        );
        if let Some(result) = early {
            drop(original);
            return result
                .map_err(anyhow::Error::from)
                .and_then(|_| Err(anyhow::anyhow!("partition returned success instead of Pending")))
                .context("Pending observation lost; no timing adjustment");
        }
        let watching = async {
            let post = observe(observer, pid, identity).await?;
            idle_report(
                &post,
                identity,
                pid,
                spec.metadata,
                "post",
                1,
                diagnostic_started.elapsed(),
            )?;
            ensure!(
                post["activity"] == witness.before["activity"],
                "partition transaction identity drift"
            );
            for polls in 1..=PARTITION_POLLS {
                let value = if polls == 1 {
                    post.clone()
                } else {
                    observe(observer, pid, identity).await?
                };
                if retired(&value) {
                    return Ok::<_, anyhow::Error>((
                        polls,
                        started.elapsed(),
                        witness.released.elapsed(),
                    ));
                }
                if !value["activity"].is_null() {
                    retiring(
                        &value,
                        &witness.before,
                        identity,
                        pid,
                        polls,
                        diagnostic_started.elapsed(),
                    )?;
                }
                if polls < PARTITION_POLLS {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            }
            anyhow::bail!("partition backend/locks survived bounded observation")
        };
        let producer = async {
            let result = (&mut original).await;
            (result.map_err(anyhow::Error::from), started.elapsed())
        };
        // The ORIGINAL APPLY_BOUND future continues to be polled, never reset.
        let ((apply, duration), server) = tokio::join!(
            producer,
            tokio::time::timeout_at(
                tokio::time::Instant::from_std(started) + APPLY_BOUND,
                watching
            )
        );
        drop(original);
        let server = server.context("partition server observation deadline").and_then(|r| r);
        (apply, duration, server, witness)
    };
    let status = f.adapter.status();
    let stats = proxy.stats();
    let partition_mode = proxy.mode();
    let diagnostic = timeout_evidence(proxy, pid);
    // Swallowed frames are gone. Restore visibility only AFTER independent
    // server timeout/retirement evidence; never release a live transaction.
    if server.is_ok() && diagnostic.is_ok() {
        proxy.set_mode(ProxyMode::Pass);
    }
    let refused = refuses_after_retirement(&mut f.adapter).await;
    let after = proxy.stats();
    eprintln!("PARTITION_RESULT cut={cut:?} apply_cause={} server_cause={} diagnostic_cause={} duration={duration:?} status={status:?} prefix={} commits={}",
        cause(&apply), cause(&server), cause(&diagnostic), stats.statements - baseline.statements, stats.commits - baseline.commits);
    let (polls, gone_at, after_release) = server?;
    diagnostic?;
    ensure!(
        apply.is_err() && duration < APPLY_BOUND && gone_at < APPLY_BOUND,
        "partition did not fail visibly/clean server within original APPLY_BOUND"
    );
    ensure!(
        status.phase == Phase::Retired
            && status.failure
                == Some(Failure {
                    at: Phase::DelegatedApply,
                    outcome: Outcome::MayHaveCommitted,
                }),
        "partition outcome"
    );
    ensure!(
        refused
            && after == stats
            && partition_mode == ProxyMode::Blackhole
            && proxy.mode() == ProxyMode::Pass
            && stats.statements - baseline.statements == spec.prefix as u64
            && stats.commits == baseline.commits
            && stats.commit_acks_dropped == baseline.commit_acks_dropped
            && proxy.autocommit_acks_dropped() == 0,
        "partition reuse/laterSQL/39/ACK/prefix"
    );
    ensure!(
        slot.lock().map_err(|_| anyhow::anyhow!("gate poisoned"))?.is_none(),
        "partition app gate survived"
    );
    ensure!(witness.windows.len() == spec.points.len(), "partition window count");
    let mut received = Vec::with_capacity(spec.prefix);
    for (n, window) in witness.windows.iter().enumerate() {
        let previous = if n == 0 { 0 } else { spec.points[n - 1].2 };
        ensure!(
            window.dropped == 0
                && window.statements == spec.points[n].2
                && window.text.len() as u64 == spec.points[n].2 - previous,
            "partition trace window incomplete"
        );
        received.extend(window.text.iter().map(|s| s.trim().trim_end_matches(';').trim()));
    }
    let chunks = fixed52::chunks(migration())?;
    let expected: Vec<_> = chunks[..spec.ddl_count]
        .iter()
        .map(|s| s.trim().trim_end_matches(';').trim())
        .collect();
    ensure!(
        received.len() == spec.prefix
            && received.len() >= 2
            && received[0].starts_with("WITH old AS MATERIALIZED")
            && received[1] == "BEGIN"
            && received[2..2 + spec.ddl_count] == expected,
        "partition original bytes/order"
    );
    if spec.metadata {
        ensure!(
            received.last() == Some(&public_apply::metadata_insert_sql_for_tests().trim()),
            "partition metadata bytes"
        );
    }
    eprintln!("PARTITION_WITNESS cut={cut:?} pending=true same_pid_idle_before_after=true idle25P03=true polls={polls} gone_at={gone_at:?} after_release={after_release:?}");
    Ok(())
}

pub(super) async fn run_partition(pool: PgPool, cut: Cut) -> Result<()> {
    let mut f = Fixture::new_with_diagnostics(&pool, true).await?;
    let mut baseline = None;
    let primary = tokio::time::timeout(APPLY_FIXTURE, async {
        let old = f.proxy.stats();
        f.adapter.lock().await?;
        ensure!(
            f.proxy.stats().statements - old.statements == 2,
            "partition advisory setup inventory"
        );
        let identity: Value =
            sqlx::query_scalar(IDENTIFY).bind(f.pid).fetch_one(&mut f.observer).await?;
        ensure!(identity["backend"].is_string(), "partition original backend identity missing");
        let _ = observe(&mut f.observer, f.pid, &identity).await?;
        baseline = Some(capture(&mut f.observer, "B0").await?);
        ensure!(
            baseline.as_ref().is_some_and(|v| v["context"]["metadata"].is_null()),
            "38 already applied"
        );
        f.baseline = f.proxy.stats();
        f.proxy.enable_query_trace(32);
        eprintln!("PARTITION_SETUP cut={cut:?} original_advisory_SQL=2 observer_setup_SQL=3");
        partition(&mut f, &identity, cut).await
    })
    .await
    .context("partition primary deadline")
    .and_then(|r| r);
    let before = f.proxy.stats();
    let cleanup = finish(f, baseline.as_ref(), before).await;
    eprintln!(
        "PARTITION_CLEANUP cut={cut:?} primary_cause={} cleanup_cause={}",
        cause(&primary),
        cause(&cleanup)
    );
    primary?;
    cleanup?;
    Ok(())
}
