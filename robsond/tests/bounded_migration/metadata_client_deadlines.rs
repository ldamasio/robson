//! Client deadline/postcommit fixture support; no new runtime owner or timer.
#[path = "metadata_postcommit_advisory.rs"]
mod advisory;
#[path = "metadata_global_deadlines.rs"]
mod global;

use super::*;

const OUTSIDE: &str = "SELECT EXISTS(SELECT 1 FROM pg_stat_activity a WHERE a.pid=$1 AND a.backend_start::text=$2 AND a.state='idle' AND a.xact_start IS NULL AND EXISTS(SELECT 1 FROM pg_locks WHERE pid=$1 AND locktype='advisory' AND granted) AND NOT EXISTS(SELECT 1 FROM pg_locks l JOIN pg_class c ON c.oid=l.relation JOIN pg_namespace n ON n.oid=c.relnamespace WHERE l.pid=$1 AND n.nspname='public'))";
const RETIRED: &str = "SELECT NOT EXISTS(SELECT 1 FROM pg_stat_activity WHERE pid=$1 OR pid=$2) AND NOT EXISTS(SELECT 1 FROM pg_locks WHERE pid=$1 OR pid=$2)";
const PROFILE: &str = "SELECT pg_backend_pid(),current_setting('lock_timeout'),current_setting('statement_timeout'),current_setting('idle_in_transaction_session_timeout'),current_setting('idle_session_timeout',true)";

fn deadline<T>(r: &Result<T>, message: &str) -> bool {
    r.as_ref().err().is_some_and(|e| {
        e.chain().any(|e| {
            e.downcast_ref::<std::io::Error>()
                .is_some_and(|e| e.kind() == std::io::ErrorKind::Other && e.to_string() == message)
        })
    })
}

async fn backend(f: &mut Fixture) -> Result<String> {
    Ok(
        sqlx::query_scalar("SELECT backend_start::text FROM pg_stat_activity WHERE pid=$1")
            .bind(f.pid)
            .fetch_one(&mut f.observer)
            .await?,
    )
}

async fn outside(c: &mut PgConnection, pid: i32, start: &str) -> Result<bool> {
    Ok(sqlx::query_scalar(OUTSIDE).bind(pid).bind(start).fetch_one(c).await?)
}

async fn profile(c: &mut PgConnection) -> Result<(i32, String, String, String, Option<String>)> {
    Ok(sqlx::query_as(PROFILE).fetch_one(c).await?)
}

fn exact_metadata(m: &Option<(String, bool, Vec<u8>, i64)>) -> Result<()> {
    let (description, success, checksum, elapsed) = m.as_ref().context("APPLIED38 absent")?;
    ensure!(
        description == migration().description.as_ref()
            && *success
            && checksum.as_slice() == migration().checksum.as_ref()
            && *elapsed >= -1,
        "APPLIED38 identity/elapsed mismatch"
    );
    Ok(())
}

struct Applied {
    full: Value,
    metadata: Option<(String, bool, Vec<u8>, i64)>,
    elapsed_may_advance: bool,
}

async fn applied(c: &mut PgConnection, elapsed_may_advance: bool) -> Result<Applied> {
    let full = capture(c, "C").await?;
    let metadata = metadata(c).await?;
    exact_metadata(&metadata)?;
    Ok(Applied { full, metadata, elapsed_may_advance })
}

async fn finish_applied(
    f: Fixture,
    waiter: Option<BoundedMigration>,
    waiter_pid: Option<i32>,
    expected: Option<&Applied>,
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
    let end = tokio::time::Instant::now() + ACQUIRE_BOUND;
    drop(adapter); // WHOLE actual holder, never just its unlock borrow.
    drop(waiter);
    let retired = tokio::time::timeout(NATIVE_RETIREMENT, async {
        for _ in 0..NATIVE_RETIREMENT_POLLS {
            let done: bool = sqlx::query_scalar(RETIRED)
                .bind(pid)
                .bind(waiter_pid)
                .fetch_one(&mut observer)
                .await?;
            if done {
                return Ok::<_, sqlx::Error>(true);
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        Ok(false)
    })
    .await
    .context("APPLIED dual retirement window")
    .and_then(|r| r.map_err(Into::into));
    let after = proxy.stats();
    let dropped = proxy.query_trace_dropped();
    proxy.set_mode(ProxyMode::Sever);
    drop(proxy);
    let snapshots = tokio::time::timeout_at(end, async {
        observer.lock().await?;
        let full = capture(&mut observer, "B1").await?;
        let m = metadata(&mut observer).await?;
        let old = legacy(&mut observer).await?;
        observer.unlock().await?;
        Ok::<_, anyhow::Error>((full, m, old))
    })
    .await
    .context("APPLIED fresh barrier/snapshots5s")
    .and_then(|r| r);
    drop(observer);
    eprintln!(
        "CLIENT_APPLIED_CLEANUP retired={:?} retirement={} snapshots={}",
        retired.as_ref().ok(),
        cause(&retired),
        cause(&snapshots)
    );
    let retired = retired?;
    eprintln!("CLIENT_APPLIED_RETIREMENT retired={retired} counters_before={before:?} counters_after={after:?} dropped={dropped}");
    ensure!(retired, "APPLIED native retirement absent");
    ensure!(after == before, "APPLIED hidden SQL counters changed during retirement");
    ensure!(dropped == 0, "APPLIED retirement trace dropped");
    let (full, m, old) = snapshots?;
    eprintln!("CLIENT_APPLIED_BASELINE expected_present={}", expected.is_some());
    let expected = expected.context("APPLIED baseline not observed")?;
    let (a, ai) = contract::maps(&expected.full)?;
    let (b, bi) = contract::maps(&full)?;
    eprintln!("CLIENT_APPLIED_MAPS semantic_equal={} native_equal={} legacy_equal={} expected_sem={} actual_sem={} expected_native={} actual_native={}",a==b,ai==bi,old==legacy_before,a.len(),b.len(),ai.len(),bi.len());
    ensure!(a == b, "APPLIED full semantic drift");
    ensure!(ai == bi, "APPLIED full native identity drift");
    ensure!(old == legacy_before, "APPLIED legacy drift");
    eprintln!("CLIENT_APPLIED_METADATA actual_present={} expected_present={} description_equal={} success_equal={} checksum_equal={} elapsed_may_advance={} elapsed_equal={} elapsed_nonregressing={}",m.is_some(),expected.metadata.is_some(),m.as_ref().map(|m| &m.0)==expected.metadata.as_ref().map(|m| &m.0),m.as_ref().map(|m| m.1)==expected.metadata.as_ref().map(|m| m.1),m.as_ref().map(|m| &m.2)==expected.metadata.as_ref().map(|m| &m.2),expected.elapsed_may_advance,m.as_ref().map(|m|m.3)==expected.metadata.as_ref().map(|m|m.3),m.as_ref().map(|m|m.3)>=expected.metadata.as_ref().map(|m|m.3));
    exact_metadata(&m)?;
    if expected.elapsed_may_advance {
        ensure!(
            m.as_ref().map(|m| m.3) >= expected.metadata.as_ref().map(|m| m.3),
            "APPLIED elapsed regressed"
        );
    } else {
        ensure!(m == expected.metadata, "APPLIED metadata changed after success");
    }
    contract::source_hash()?;
    Ok(())
}

async fn healthy58(f: &mut Fixture) -> Result<()> {
    let run = apply_in_windows(f).await?;
    run.0.map_err(anyhow::Error::from)?;
    let windows = run.1.context("healthy58 windows absent")?;
    let mut sql = Vec::with_capacity(58);
    ensure!(windows.len() == 4, "healthy58 window count");
    for (n, w) in windows.iter().enumerate() {
        ensure!(
            w.dropped == 0
                && w.statements == [18, 34, 50, 58][n]
                && w.text.len() == [18, 16, 16, 8][n],
            "healthy58 incomplete received trace"
        );
        sql.extend(w.text.iter().map(|s| s.trim().trim_end_matches(';').trim()));
    }
    let chunks = fixed52::chunks(migration())?;
    let expected: Vec<_> = chunks.iter().map(|s| s.trim().trim_end_matches(';').trim()).collect();
    ensure!(
        sql.len() == 58
            && sql[2..54] == expected
            && sql[1] == "BEGIN"
            && sql[54] == public_apply::metadata_insert_sql_for_tests().trim()
            && sql[55] == "COMMIT"
            && sql[56].contains("UPDATE _sqlx_migrations")
            && sql[57].starts_with("SELECT set_config("),
        "healthy58 source order"
    );
    ensure!(
        f.adapter.status() == (Status { phase: Phase::Restored, failure: None }),
        "healthy58 not restored"
    );
    Ok(())
}
