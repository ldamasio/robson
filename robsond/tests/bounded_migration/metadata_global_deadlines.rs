//! Real original global200 firing; scheduled gaps are never server idleTX.
use super::*;

#[sqlx::test(migrations = false)]
#[ignore = "requires DATABASE_URL"]
async fn actual52_global200_blackhole_before_install_retires_without_server_work(
    pool: PgPool,
) -> Result<()> {
    let mut f = Fixture::new(&pool).await?;
    let mut baseline = None;
    let primary = tokio::time::timeout(ACQUIRE_BOUND, async {
        f.adapter.lock().await?;
        let p = profile(f.adapter.connection()?).await?;
        ensure!(p.0 == f.pid && p.4.as_deref().is_none_or(|v| v == "0"), "outsideTX server idle limit could mask200");
        let start = backend(&mut f).await?;
        ensure!(outside(&mut f.observer, f.pid, &start).await?, "initial owner not idle outsideTX/advisory");
        baseline = Some(capture(&mut f.observer, "B0").await?);
        f.baseline = f.proxy.stats();
        let slot = f.adapter.gate.clone();
        let (hit, release) = stages::arm(&slot, Stage::Install, Edge::BeforeCall)?;
        f.proxy.set_mode(ProxyMode::Blackhole);
        let proxy = &f.proxy;
        let observer = &mut f.observer;
        let pid = f.pid;
        let began = Instant::now();
        let (result, upper, ended) = {
            let mut original = f.adapter.apply(migration());
            let mut controller = Box::pin(async {
                hit.await.map_err(|_| anyhow::anyhow!("global200 BeforeInstall absent"))?;
                let upper = Instant::now();
                release.send(()).map_err(|_| anyhow::anyhow!("global200 Install release lost"))?;
                ensure!(outside(observer, pid, &start).await?, "global200 server enteredTX/businesslocks");
                Ok::<_, anyhow::Error>(upper)
            });
            let (early, seen) = tokio::select! {
                r = &mut original => (Some(r), None),
                s = &mut controller => (None, Some(s)),
            };
            drop(controller);
            let upper = seen.context("global200 returned before released native witness")??;
            ensure!(early.is_none() && slot.lock().map_err(|_| anyhow::anyhow!("gate poisoned"))?.is_none(), "global200 appgate/early return");
            let probe = std::future::poll_fn(|cx| std::task::Poll::Ready(match std::future::Future::poll(original.as_mut(), cx) {
                std::task::Poll::Pending => Ok(()),
                std::task::Poll::Ready(_) => Err(anyhow::anyhow!("global200 genuine I/O Pending absent")),
            })).await;
            probe?;
            let r = (&mut original).await.map_err(anyhow::Error::from);
            let ended = Instant::now();
            drop(original);
            (r, upper, ended)
        };
        let stats = proxy.stats();
        let status = f.adapter.status();
        proxy.set_mode(ProxyMode::Pass);
        let refused = if status.phase == Phase::Retired { refuses_after_retirement(&mut f.adapter).await } else { false };
        eprintln!("GLOBAL200_IO result={} elapsed={:?} preflight_hit_bracket={:?} excess_lower={:?} excess_upper={:?} status={status:?}",
            cause(&result), ended.duration_since(began), upper.duration_since(began),
            ended.saturating_duration_since(upper + APPLY_BOUND), ended.saturating_duration_since(began + APPLY_BOUND));
        ensure!(deadline(&result, "migration38 whole-apply deadline exceeded") && ended.duration_since(began) >= APPLY_BOUND,
            "global200 original timeout did not fire");
        ensure!(status == (Status { phase: Phase::Retired, failure: Some(Failure { at: Phase::SetupStarted, outcome: Outcome::ApplyNotStarted }) })
            && refused && proxy.stats() == stats && stats == f.baseline, "global200 work/reuse/39/outcome");
        Ok::<_, anyhow::Error>(())
    }).await.context("global200 fixture5s").and_then(|r| r);
    let before = f.proxy.stats();
    let cleanup = super::super::super::finish(f, baseline.as_ref(), before).await;
    eprintln!("GLOBAL200_IO_CLEANUP primary={} cleanup={}", cause(&primary), cause(&cleanup));
    primary?;
    cleanup?;
    Ok(())
}

// One shared 200 ms budget must expire within this measured excess after its
// preflight bracket; per-stage renewed budgets end about 100 ms later.
const CUMULATIVE_EXCESS_BOUND: Duration = Duration::from_millis(50);

#[sqlx::test(migrations = false)]
#[ignore = "requires DATABASE_URL"]
async fn actual52_global200_is_cumulative_across_three_outside_tx_gaps(pool: PgPool) -> Result<()> {
    let mut f = Fixture::new(&pool).await?;
    let mut applied_baseline = None;
    let mut unapplied_baseline = None;
    let mut hits = Vec::with_capacity(6);
    let mut completed_delays = 0;
    let mut windows = Vec::with_capacity(6);
    let mut committed_at_gate = false;
    let mut landed_outcome = None;
    let primary = tokio::time::timeout(ACQUIRE_BOUND, async {
        f.adapter.lock().await?;
        let start = backend(&mut f).await?;
        ensure!(outside(&mut f.observer, f.pid, &start).await?, "cumulative initial owner not outsideTX/advisory");
        let p = profile(f.adapter.connection()?).await?;
        ensure!(p.4.as_deref().is_none_or(|s| s == "0"), "cumulative idle_session timeout could mask200");
        // DDL speed decides whether the shared deadline lands before or after
        // COMMIT; both are exact outcomes, so keep the full B0 for the former.
        unapplied_baseline = Some(capture(&mut f.observer, "B0").await?);
        ensure!(metadata(&mut f.observer).await?.is_none(), "cumulative38 already applied");
        f.baseline = f.proxy.stats();
        f.proxy.enable_query_trace(32);
        let points = [Stage::Install, Stage::Ddl(16), Stage::Ddl(32), Stage::Ddl(48), Stage::Elapsed, Stage::Restore];
        let slot = f.adapter.gate.clone();
        let first = stages::arm(&slot, points[0], Edge::BeforeCall)?;
        let proxy = &f.proxy;
        let baseline = f.baseline;
        let observer = &mut f.observer;
        let pid = f.pid;
        let began = Instant::now();
        let (result,ended) = {
            let mut original = f.adapter.apply(migration());
            let mut controller = Box::pin(async {
                let mut current = first;
                for (n,stage) in points.iter().copied().enumerate() {
                    current.0.await.map_err(|_| anyhow::anyhow!("cumulative stage absent"))?;
                    hits.push((stage, began.elapsed(), proxy.stats()));
                    windows.push((stage, Window { text: proxy.query_trace(), dropped: proxy.query_trace_dropped(), statements: proxy.stats().statements - baseline.statements }));
                    proxy.enable_query_trace(32);
                    if matches!(stage, Stage::Install | Stage::Elapsed | Stage::Restore) {
                        ensure!(outside(observer, pid, &start).await?, "cumulative hold was not outsideTX/advisory");
                    }
                    if matches!(stage, Stage::Elapsed | Stage::Restore) {
                        let m = metadata(observer).await?;
                        exact_metadata(&m)?;
                        let elapsed = m.as_ref().context("cumulative gate metadata absent")?.3;
                        ensure!(if stage==Stage::Elapsed {elapsed == -1} else {elapsed>=0},"cumulative gate metadata durability boundary");
                        committed_at_gate = true;
                    }
                    let next = if n + 1 < points.len() {
                        Some(stages::arm(&slot, points[n+1], if matches!(points[n+1],Stage::Ddl(_)) { Edge::AfterReturn } else { Edge::BeforeCall })?)
                    } else { None };
                    if matches!(stage, Stage::Install | Stage::Elapsed | Stage::Restore) {
                        tokio::time::sleep(Duration::from_millis(90)).await;
                        completed_delays += 1;
                    }
                    current.1.send(()).map_err(|_| anyhow::anyhow!("cumulative release lost"))?;
                    if let Some(next) = next { current = next; } else { break; }
                }
                Ok::<_, anyhow::Error>(())
            });
            let result = tokio::select! {
                r = &mut original => (r.map_err(anyhow::Error::from),Instant::now()),
                c = &mut controller => { c?; let r = (&mut original).await.map_err(anyhow::Error::from); (r,Instant::now()) },
            };
            drop(controller);
            drop(original);
            result
        };
        let status = f.adapter.status();
        let bracket = hits.first().context("initial global bracket absent")?.1;
        let committed = hits.iter().any(|h| h.0 == Stage::Elapsed);
        // Preserve the original return-time measurement before doing any slow
        // resolution. A flushed COMMIT may still be unread by the proxy here.
        eprintln!("GLOBAL200_CUMULATIVE_RETURN actual_apply_return={:?} original_global200_error={} elapsed_seen={committed} gate_verified={committed_at_gate}",ended.duration_since(began),deadline(&result,"migration38 whole-apply deadline exceeded"));
        ensure!(status.phase == Phase::Retired, "cumulative owner not retired");
        let gone = tokio::time::timeout(Duration::from_millis(300), async {
            for _ in 0..60 {
                let native_gone = sqlx::query_scalar::<_, bool>("SELECT NOT EXISTS(SELECT 1 FROM pg_stat_activity WHERE pid=$1) AND NOT EXISTS(SELECT 1 FROM pg_locks WHERE pid=$1)")
                    .bind(pid).fetch_one(&mut *observer).await?;
                if native_gone && proxy.closed_relays() == baseline.connections {
                    return Ok::<_, sqlx::Error>(true);
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            Ok(false)
        }).await.context("cumulative native/proxy retirement300ms")??;
        ensure!(gone && baseline.connections > 0, "cumulative native/proxy retirement absent");
        // Only native catalog evidence behind the public advisory barrier
        // decides durability and cleanup. Sent COMMIT is not committed SQL.
        observer.lock().await?;
        let post = async {
            let landed = metadata(observer).await?.is_some();
            if landed {
                applied_baseline = Some(applied(observer, true).await?);
            }
            Ok::<_, anyhow::Error>(landed)
        }.await;
        let unlocked = observer.unlock().await;
        eprintln!("GLOBAL200_CUMULATIVE_ORACLE point=AFTER_NATIVE_PROXY_RETIREMENT_AND_ADVISORY_BARRIER actual_apply_return={:?} original_global200_error={} full={} closed_relays={}",ended.duration_since(began),deadline(&result,"migration38 whole-apply deadline exceeded"),cause(&post),proxy.closed_relays());
        let landed = post?;
        unlocked?;
        landed_outcome = Some(landed);
        let before_refusals = proxy.stats();
        let refused = if status.phase == Phase::Retired { refuses_after_retirement(&mut f.adapter).await } else { false };
        let before_tail = proxy.stats();
        let tail = proxy.query_trace();
        let dropped = proxy.query_trace_dropped();
        let stats = proxy.stats();
        let drained = windows.last().context("cumulative drained windows absent")?.1.statements;
        // This count is final: the real forwarding scope has exited. It is
        // trace classification only; `landed` above is durability authority.
        let commit_sent = !committed && stats.commits - f.baseline.commits == 1;
        let received = stats.statements.checked_sub(baseline.statements).context("cumulative counter regressed")?;
        eprintln!("GLOBAL200_CUMULATIVE_TRACE drained={drained} tail_len={} dropped={dropped} received={received} before_refusals={before_refusals:?} before_tail={before_tail:?} after_tail={stats:?} planned90=3 completed90={completed_delays} hits={hits:?}",tail.len());
        ensure!(before_refusals == before_tail,"cumulative refusal interval received SQL");
        ensure!(before_tail == stats,"cumulative tail snapshot counters moved");
        ensure!(stats.connections == baseline.connections && proxy.closed_relays() == stats.connections, "cumulative relay identity/count changed");
        ensure!(!committed || landed, "cumulative Elapsed observed without durable38");
        ensure!(!landed || stats.commits - baseline.commits == 1, "cumulative durable38 without original COMMIT trace");
        ensure!(dropped == 0,"cumulative tail trace dropped");
        ensure!(if committed { (56..=57).contains(&drained) && (56..=57).contains(&received) } else if commit_sent { received == 56 && drained <= received } else { (2..=55).contains(&received) && drained <= received },"cumulative drained/final prefix outside committed56or57/uncertain56/uncommitted2to55");
        ensure!(received.checked_sub(drained) == Some(u64::try_from(tail.len())?),"cumulative tail/counter coverage mismatch");
        let mut sql = Vec::with_capacity(57);
        for (stage, w) in &windows {
            let (received, count) = match stage { Stage::Install => (0,0), Stage::Ddl(16) => (18,18), Stage::Ddl(32) => (34,16), Stage::Ddl(48) => (50,16), Stage::Elapsed => (56,6), Stage::Restore => (57,1), _ => anyhow::bail!("unexpected cumulative trace stage") };
            ensure!(w.dropped == 0 && w.statements == received && w.text.len() == count, "cumulative received window incomplete");
            sql.extend(w.text.iter().map(|s| s.trim().trim_end_matches(';').trim()));
        }
        sql.extend(tail.iter().map(|s| s.trim().trim_end_matches(';').trim()));
        ensure!(u64::try_from(sql.len())? == received,"cumulative full received prefix incomplete");
        let source = include_str!("../../../robson-db/src/bounded_migration/public_apply.rs");
        let elapsed_sql = source.split_once("const ELAPSED: &str = r#\"").context("original elapsed source start")?.1
            .split_once("\"#;").context("original elapsed source end")?.0.trim();
        let chunks = fixed52::chunks(migration())?;
        let expected: Vec<_> = chunks.iter().map(|s| s.trim().trim_end_matches(';').trim()).collect();
        let n = sql.len();
        ensure!(n >= 2 && sql[1] == "BEGIN" && sql[2..n.min(54)] == expected[..n.min(54) - 2]
            && (n < 55 || sql[54] == public_apply::metadata_insert_sql_for_tests().trim())
            && if committed { sql[55] == "COMMIT" && (n == 56 || sql[56] == elapsed_sql) } else if commit_sent { n == 56 && sql[55] == "COMMIT" } else { n <= 55 }, "cumulative fixed52/metadata/source order");
        eprintln!("GLOBAL200_CUMULATIVE cause={} elapsed={:?} preflight_hit_bracket={bracket:?} excess_lower={:?} excess_upper={:?} planned90=3 completed90={completed_delays} hits={hits:?} status={status:?}", cause(&result), ended.duration_since(began), ended.saturating_duration_since(began + bracket + APPLY_BOUND), ended.saturating_duration_since(began + APPLY_BOUND));
        ensure!(ended.saturating_duration_since(began + bracket + APPLY_BOUND) <= CUMULATIVE_EXCESS_BOUND, "global200 cumulative deadline later than one shared budget");
        ensure!(deadline(&result, "migration38 whole-apply deadline exceeded") && (1..3).contains(&completed_delays)
            && hits.iter().any(|h| h.0 == Stage::Install), "global200 reset/cumulative witness absent");
        let statements = stats.statements - f.baseline.statements;
        ensure!(refused && f.proxy.stats() == stats && stats.commits - f.baseline.commits == u64::from(committed || commit_sent)
            && if committed { (56..=57).contains(&statements) } else if commit_sent { statements == 56 } else { (2..=55).contains(&statements) }, "cumulative laterSQL/39/COMMIT/prefix");
        let expected_failure = if hits.iter().any(|h| h.0 == Stage::Restore) { Failure { at: Phase::RestoreStarted, outcome: Outcome::AppliedButSessionUnrestored } } else { Failure { at: Phase::DelegatedApply, outcome: Outcome::MayHaveCommitted } };
        ensure!(status == (Status { phase: Phase::Retired, failure: Some(expected_failure) }), "cumulative conservative exact outcome");
        eprintln!("GLOBAL200_CUMULATIVE_UNCERTAIN_COMMIT landed={landed} elapsed_seen={committed}");
        Ok::<_, anyhow::Error>(())
    }).await.context("cumulative fixture5s").and_then(|r| r);
    let before = f.proxy.stats();
    let cleanup: Result<()> = match landed_outcome {
        Some(true) => finish_applied(f, None, None, applied_baseline.as_ref(), before).await,
        Some(false) => super::super::super::finish(f, unapplied_baseline.as_ref(), before).await,
        None => {
            // Dispose the whole owner even if resolution failed. Unknown must
            // never silently select or claim the unapplied B0 outcome.
            let disposed = f.finish().await;
            match disposed {
                Ok(end) => Err(anyhow::anyhow!(
                    "cumulative durability unresolved; disposal retired={} metadata_present={}",
                    end.retired,
                    end.metadata.is_some()
                )),
                Err(error) => Err(error),
            }
        },
    };
    eprintln!(
        "GLOBAL200_CUMULATIVE_CLEANUP primary={} cleanup={}",
        cause(&primary),
        cause(&cleanup)
    );
    primary?;
    cleanup?;
    Ok(())
}
