//! Same successful original holder, public unlock Pending, public waiter5s.
use super::*;

const SAME_KEY: &str = "SELECT EXISTS(SELECT 1 FROM pg_stat_activity a JOIN pg_locks w ON w.pid=a.pid JOIN pg_locks h ON h.locktype=w.locktype AND h.database IS NOT DISTINCT FROM w.database AND h.classid=w.classid AND h.objid=w.objid AND h.objsubid=w.objsubid WHERE a.pid=$1 AND a.backend_start::text=$2 AND a.state='active' AND a.wait_event_type='Lock' AND w.locktype='advisory' AND w.mode='ExclusiveLock' AND h.mode=w.mode AND NOT w.granted AND h.pid=$3 AND h.granted AND $3=ANY(pg_blocking_pids(a.pid)))";

#[sqlx::test(migrations = false)]
#[ignore = "requires DATABASE_URL"]
async fn actual52_postcommit_original_advisory_holder_forces_public_waiter5s(
    pool: PgPool,
) -> Result<()> {
    let mut f = Fixture::new(&pool).await?;
    let mut waiter = None;
    let mut waiter_pid = None;
    let mut applied_baseline = None;
    let primary = async {
        let (holder_start, waiter_start) = tokio::time::timeout(APPLY_FIXTURE, async {
            f.adapter.lock().await?;
            f.proxy.enable_query_trace(32);
            f.baseline = f.proxy.stats();
            healthy58(&mut f).await?;
            let holder_start = backend(&mut f).await?;
            ensure!(outside(&mut f.observer, f.pid, &holder_start).await?, "postcommit holder has TX/business locks");
            applied_baseline = Some(applied(&mut f.observer, false).await?);
            waiter = Some(BoundedMigration::connect(pool.connect_options().as_ref()).await?);
            let w = waiter.as_mut().context("waiter missing")?;
            let p = profile(w.connection()?).await?;
            ensure!(p.1 == "0" && p.2 == "0" && p.4.as_deref().is_none_or(|s| s == "0"),
                "waiter inherited server timeout could mask existing5s: lock={} statement={} idle_session={:?}", p.1, p.2, p.4);
            waiter_pid = Some(p.0);
            let waiter_start: String = sqlx::query_scalar("SELECT backend_start::text FROM pg_stat_activity WHERE pid=$1")
                .bind(p.0).fetch_one(&mut f.observer).await?;
            f.proxy.enable_query_trace(32);
            f.baseline = f.proxy.stats();
            Ok::<_, anyhow::Error>((holder_start, waiter_start))
        }).await.context("advisory setup ceiling (separate from real acquisition5s)")??;
        let w = waiter.as_mut().context("waiter missing after setup")?;
        let wp = waiter_pid.context("waiter PID absent")?;
        let proxy = &f.proxy;
        let observer = &mut f.observer;
        proxy.set_mode(ProxyMode::Blackhole);
        let began = Instant::now();
        let (result, elapsed) = {
            let mut unlock = f.adapter.unlock();
            let pending = std::future::poll_fn(|cx| std::task::Poll::Ready(match std::future::Future::poll(unlock.as_mut(), cx) {
                std::task::Poll::Pending => Ok(()),
                std::task::Poll::Ready(_) => Err(anyhow::anyhow!("original postcommit unlock was not Pending")),
            })).await;
            pending?;
            ensure!(tokio::time::timeout(Duration::from_millis(300), outside(observer, f.pid, &holder_start)).await.context("prewait holder native observation300ms")??, "unlock partition lost original outsideTX holder");
            let lock_began = Instant::now();
            let mut lock = w.lock();
            let mut witness = Box::pin(tokio::time::timeout(Duration::from_millis(300), async {
                for n in 1..=60 {
                    let same: bool = sqlx::query_scalar(SAME_KEY).bind(wp).bind(&waiter_start).bind(f.pid).fetch_one(&mut *observer).await?;
                    if same {
                        ensure!(outside(observer, f.pid, &holder_start).await?, "contended holder has business locks");
                        return Ok::<_, anyhow::Error>(n);
                    }
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
                anyhow::bail!("actual native same-key waiter contention not observed")
            }));
            let seen = tokio::select! {
                r = &mut lock => Err(anyhow::anyhow!("public waiter returned before actual same-key contention: {}", if r.is_ok() {"success"} else {"error"})),
                s = &mut witness => s.context("same-key native witness300ms").and_then(|r| r),
            };
            drop(witness);
            let polls = seen?;
            // No outer5s competes with the REAL original public lock's own5s.
            let result = (&mut lock).await.map_err(anyhow::Error::from);
            let elapsed = lock_began.elapsed();
            drop(lock);
            let pending = std::future::poll_fn(|cx| std::task::Poll::Ready(match std::future::Future::poll(unlock.as_mut(), cx) {
                std::task::Poll::Pending => Ok(()),
                std::task::Poll::Ready(_) => Err(anyhow::anyhow!("original unlock ceased Pending while holder retained")),
            })).await;
            // Dropping this borrow alone does NOT poison the successful holder.
            drop(unlock);
            pending?;
            eprintln!("ADVISORY5S_NATIVE polls={polls} waiter_pid={wp} holder_pid={} acquisition_first_poll_bracket={:?} elapsed={elapsed:?} total={:?}", f.pid, lock_began.duration_since(began), began.elapsed());
            (result, elapsed)
        };
        let status = w.status();
        let refused = if status.phase == Phase::Retired { refuses_after_retirement(w).await } else { false };
        ensure!(deadline(&result, "migration advisory-lock deadline exceeded") && elapsed >= ACQUIRE_BOUND,
            "public waiter acquisition5s did not actually fire");
        ensure!(status == (Status { phase: Phase::Retired, failure: Some(Failure { at: Phase::Preflight, outcome: Outcome::ApplyNotStarted }) }) && refused,
            "waiter deadline did not retire/refuse all8 entrypoints");
        ensure!(f.adapter.status() == (Status { phase: Phase::Restored, failure: None })
            && tokio::time::timeout(Duration::from_millis(300), outside(&mut f.observer, f.pid, &holder_start)).await.context("postwait holder native observation300ms")?? && f.proxy.stats() == f.baseline,
            "successful original holder changed or hidden unlock SQL arrived");
        eprintln!("ADVISORY5S_RESULT cause={} status={status:?}; dual retirement only after whole holder drop", cause(&result));
        Ok::<_, anyhow::Error>(())
    }.await;
    let before = f.proxy.stats();
    let cleanup = finish_applied(f, waiter, waiter_pid, applied_baseline.as_ref(), before).await;
    eprintln!("ADVISORY5S_CLEANUP primary={} cleanup={}", cause(&primary), cause(&cleanup));
    primary?;
    cleanup?;
    Ok(())
}
