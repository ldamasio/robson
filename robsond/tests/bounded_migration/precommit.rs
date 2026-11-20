//! Fresh macro-owned DB per cut. App gates and received text are distinct
//! facts.
#[path = "precommit_cut.rs"]
mod cut;
#[path = "late_partition.rs"]
mod late_partition;
#[path = "metadata_pending.rs"]
mod metadata_pending;

use std::time::{Duration, Instant};

use anyhow::{ensure, Context, Result};
use cut::*;
use robson_testkit::db_proxy::{ProxyMode, ProxyStats};
use serde_json::Value;
use sqlx::PgPool;

use super::{
    super::{
        helpers::*,
        stages::{self, Edge, Stage},
        *,
    },
    capture, cause, contract,
};

async fn finish(f: Fixture, baseline: Option<&Value>, before_cleanup: ProxyStats) -> Result<()> {
    let Fixture {
        adapter,
        proxy,
        mut observer,
        pid,
        legacy_before,
        ..
    } = f;
    let deadline = tokio::time::Instant::now() + ACQUIRE_BOUND;
    drop(adapter);
    let retirement = tokio::time::timeout(NATIVE_RETIREMENT, async {
        for _ in 0..NATIVE_RETIREMENT_POLLS {
            if !alive(&mut observer, pid).await? {
                return Ok::<_, sqlx::Error>(true);
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        Ok(false)
    })
    .await
    .context("precommit retirement deadline")
    .and_then(|r| r.map_err(Into::into));
    let after_retirement = proxy.stats();
    let trace_dropped = proxy.query_trace_dropped();
    proxy.set_mode(ProxyMode::Sever);
    drop(proxy);
    // The actual original adapter held this same public SQLx advisory lock.
    // All snapshots and unlock share the original absolute cleanup deadline.
    let snapshots = tokio::time::timeout_at(deadline, async {
        observer.lock().await?;
        let full = capture(&mut observer, "B1").await?;
        let legacy = legacy(&mut observer).await?;
        let metadata = metadata(&mut observer).await?;
        observer.unlock().await?;
        Ok::<_, anyhow::Error>((full, legacy, metadata))
    })
    .await
    .context("precommit serialized snapshot deadline")
    .and_then(|r| r);
    // Drop even an unfinished/failed observer before propagating any error.
    drop(observer);
    eprintln!(
        "PRECOMMIT_CLEANUP retired={:?} retirement_cause={} snapshots_cause={} statements={}",
        retirement.as_ref().ok(),
        cause(&retirement),
        cause(&snapshots),
        after_retirement.statements
    );
    ensure!(retirement?, "precommit original backend not retired");
    let (full, legacy, metadata) = snapshots?;
    ensure!(
        after_retirement == before_cleanup && trace_dropped == 0,
        "precommit retirement flushed hidden SQL or dropped trace"
    );
    let baseline = baseline.context("precommit baseline unavailable")?;
    let (before, before_ids) = contract::maps(baseline)?;
    let (after, after_ids) = contract::maps(&full)?;
    ensure!(
        before == after && before_ids == after_ids,
        "precommit rollback changed full semantic or native identity baseline"
    );
    ensure!(
        metadata.is_none() && legacy == legacy_before,
        "precommit metadata38 landed or legacy/history changed"
    );
    ensure!(
        contract::source_hash()? == contract::CHECKSUM,
        "precommit source identity drift"
    );
    Ok(())
}

async fn run_cut(pool: PgPool, cut: Cut) -> Result<()> {
    let mut f = Fixture::new(&pool).await?;
    let mut baseline = None;
    let primary = tokio::time::timeout(APPLY_FIXTURE, async {
        // Existing SQLx public lock includes current_database and lock: setup,
        // outside the measured apply prefix; never copy its key-generation algorithm.
        let setup = f.proxy.stats();
        f.adapter.lock().await?;
        let lock_statements = f.proxy.stats().statements - setup.statements;
        ensure!(lock_statements == 2, "precommit original advisory setup inventory");
        baseline = Some(capture(&mut f.observer, "B0").await?);
        ensure!(metadata(&mut f.observer).await?.is_none(), "precommit38 already applied");
        f.baseline = f.proxy.stats();
        f.proxy.enable_query_trace(32);
        eprintln!("PRECOMMIT_SETUP cut={cut:?} original_lock_statements={lock_statements} full_baseline_captures=1 metadata_reads=1");
        cancel_original(&mut f, cut).await
    }).await.context("precommit primary fixture deadline").and_then(|r| r);
    let before_cleanup = f.proxy.stats();
    let cleanup = finish(f, baseline.as_ref(), before_cleanup).await;
    eprintln!(
        "PRECOMMIT_RESULT primary_cause={} cleanup_cause={}",
        cause(&primary),
        cause(&cleanup)
    );
    primary?;
    cleanup?;
    Ok(())
}


// Distinct from Install/BeforeCall: this owner has NEVER been taken.
#[sqlx::test(migrations = false)]
#[ignore = "requires DATABASE_URL"]
async fn actual52_drop_unpolled_apply_preserves_owner(pool: PgPool) -> Result<()> {
    let mut f = Fixture::new(&pool).await?;
    let mut baseline = None;
    let primary = tokio::time::timeout(APPLY_FIXTURE, async {
        let setup = f.proxy.stats();
        f.adapter.lock().await?;
        ensure!(f.proxy.stats().statements - setup.statements == 2,
            "unpolled original advisory setup inventory");
        baseline = Some(capture(&mut f.observer, "B0").await?);
        ensure!(metadata(&mut f.observer).await?.is_none(), "unpolled38 already applied");
        f.proxy.enable_query_trace(32);
        let before = f.proxy.stats();
        let status = f.adapter.status();
        let original = f.adapter.apply(migration());
        drop(original); // Never poll, await, select or spawn this ORIGINAL future.
        ensure!(f.proxy.stats() == before && f.proxy.query_trace().is_empty() &&
            f.adapter.status() == status && status.failure.is_none(),
            "unpolled future performed work or changed owner status");
        // One separately counted probe proves same usable owner and old GUCs;
        // it is not charged to the zero-work dropped future.
        let row: (i32, String, String, String) = sqlx::query_as(
            "SELECT pg_backend_pid(),current_setting('lock_timeout'),current_setting('statement_timeout'),current_setting('idle_in_transaction_session_timeout')"
        ).persistent(false).fetch_one(f.adapter.connection()?).await?;
        ensure!(row == (f.pid, "310ms".into(), "410ms".into(), "510ms".into()),
            "unpolled drop changed actual owner PID/settings");
        ensure!(f.proxy.stats().statements - before.statements == 1 &&
            f.proxy.stats().commits == before.commits &&
            f.proxy.query_trace().len() == 1 && f.proxy.query_trace_dropped() == 0,
            "unpolled future added hidden SQL around readback probe");
        eprintln!("UNPOLLED_CUT apply_SQL=0 readback_SQL=1 owner_retained=true");
        Ok::<_, anyhow::Error>(())
    }).await.context("unpolled primary fixture deadline").and_then(|r| r);
    let before_cleanup = f.proxy.stats();
    let cleanup = finish(f, baseline.as_ref(), before_cleanup).await;
    eprintln!(
        "UNPOLLED_RESULT primary_cause={} cleanup_cause={}",
        cause(&primary),
        cause(&cleanup)
    );
    primary?;
    cleanup?;
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires DATABASE_URL"]
async fn actual52_cancel_before_install(pool: PgPool) -> Result<()> {
    run_cut(pool, Cut::BeforeInstall).await
}

#[sqlx::test(migrations = false)]
#[ignore = "requires DATABASE_URL"]
async fn actual52_cancel_after_install(pool: PgPool) -> Result<()> {
    run_cut(pool, Cut::AfterInstall).await
}

#[sqlx::test(migrations = false)]
#[ignore = "requires DATABASE_URL"]
async fn actual52_cancel_before_begin(pool: PgPool) -> Result<()> {
    run_cut(pool, Cut::BeforeBegin).await
}

#[sqlx::test(migrations = false)]
#[ignore = "requires DATABASE_URL"]
async fn actual52_cancel_after_begin(pool: PgPool) -> Result<()> {
    run_cut(pool, Cut::AfterBegin).await
}

#[sqlx::test(migrations = false)]
#[ignore = "requires DATABASE_URL"]
async fn actual52_cancel_before_metadata(pool: PgPool) -> Result<()> {
    run_cut(pool, Cut::BeforeMetadata).await
}

#[sqlx::test(migrations = false)]
#[ignore = "requires DATABASE_URL"]
async fn actual52_cancel_after_metadata(pool: PgPool) -> Result<()> {
    run_cut(pool, Cut::AfterMetadata).await
}

#[sqlx::test(migrations = false)]
#[ignore = "requires DATABASE_URL"]
async fn actual52_cancel_before_commit_preserves_full_baseline(pool: PgPool) -> Result<()> {
    run_cut(pool, Cut::BeforeCommit).await
}

// Each generated fn gets sqlx::test's DISTINCT fresh DB; no fixture reset or
// repeated seeded legacy-client identity in a shared DB, and no retry loop.
macro_rules! ddl_cuts {
    ($($before:ident, $after:ident => $n:literal);+ $(;)?) => {
        $(
        #[sqlx::test(migrations = false)]
        #[ignore = "requires DATABASE_URL"]
        async fn $before(pool: PgPool) -> Result<()> {
            run_cut(pool, Cut::BeforeDdl($n)).await
        }
        #[sqlx::test(migrations = false)]
        #[ignore = "requires DATABASE_URL"]
        async fn $after(pool: PgPool) -> Result<()> {
            run_cut(pool, Cut::AfterDdl($n)).await
        }
        )+
    };
}

ddl_cuts! {
    actual52_cancel_before_ddl01, actual52_cancel_after_ddl01 => 1;
    actual52_cancel_before_ddl02, actual52_cancel_after_ddl02 => 2;
    actual52_cancel_before_ddl03, actual52_cancel_after_ddl03 => 3;
    actual52_cancel_before_ddl04, actual52_cancel_after_ddl04 => 4;
    actual52_cancel_before_ddl05, actual52_cancel_after_ddl05 => 5;
    actual52_cancel_before_ddl06, actual52_cancel_after_ddl06 => 6;
    actual52_cancel_before_ddl07, actual52_cancel_after_ddl07 => 7;
    actual52_cancel_before_ddl08, actual52_cancel_after_ddl08 => 8;
    actual52_cancel_before_ddl09, actual52_cancel_after_ddl09 => 9;
    actual52_cancel_before_ddl10, actual52_cancel_after_ddl10 => 10;
    actual52_cancel_before_ddl11, actual52_cancel_after_ddl11 => 11;
    actual52_cancel_before_ddl12, actual52_cancel_after_ddl12 => 12;
    actual52_cancel_before_ddl13, actual52_cancel_after_ddl13 => 13;
    actual52_cancel_before_ddl14, actual52_cancel_after_ddl14 => 14;
    actual52_cancel_before_ddl15, actual52_cancel_after_ddl15 => 15;
    actual52_cancel_before_ddl16, actual52_cancel_after_ddl16 => 16;
    actual52_cancel_before_ddl17, actual52_cancel_after_ddl17 => 17;
    actual52_cancel_before_ddl18, actual52_cancel_after_ddl18 => 18;
    actual52_cancel_before_ddl19, actual52_cancel_after_ddl19 => 19;
    actual52_cancel_before_ddl20, actual52_cancel_after_ddl20 => 20;
    actual52_cancel_before_ddl21, actual52_cancel_after_ddl21 => 21;
    actual52_cancel_before_ddl22, actual52_cancel_after_ddl22 => 22;
    actual52_cancel_before_ddl23, actual52_cancel_after_ddl23 => 23;
    actual52_cancel_before_ddl24, actual52_cancel_after_ddl24 => 24;
    actual52_cancel_before_ddl25, actual52_cancel_after_ddl25 => 25;
    actual52_cancel_before_ddl26, actual52_cancel_after_ddl26 => 26;
    actual52_cancel_before_ddl27, actual52_cancel_after_ddl27 => 27;
    actual52_cancel_before_ddl28, actual52_cancel_after_ddl28 => 28;
    actual52_cancel_before_ddl29, actual52_cancel_after_ddl29 => 29;
    actual52_cancel_before_ddl30, actual52_cancel_after_ddl30 => 30;
    actual52_cancel_before_ddl31, actual52_cancel_after_ddl31 => 31;
    actual52_cancel_before_ddl32, actual52_cancel_after_ddl32 => 32;
    actual52_cancel_before_ddl33, actual52_cancel_after_ddl33 => 33;
    actual52_cancel_before_ddl34, actual52_cancel_after_ddl34 => 34;
    actual52_cancel_before_ddl35, actual52_cancel_after_ddl35 => 35;
    actual52_cancel_before_ddl36, actual52_cancel_after_ddl36 => 36;
    actual52_cancel_before_ddl37, actual52_cancel_after_ddl37 => 37;
    actual52_cancel_before_ddl38, actual52_cancel_after_ddl38 => 38;
    actual52_cancel_before_ddl39, actual52_cancel_after_ddl39 => 39;
    actual52_cancel_before_ddl40, actual52_cancel_after_ddl40 => 40;
    actual52_cancel_before_ddl41, actual52_cancel_after_ddl41 => 41;
    actual52_cancel_before_ddl42, actual52_cancel_after_ddl42 => 42;
    actual52_cancel_before_ddl43, actual52_cancel_after_ddl43 => 43;
    actual52_cancel_before_ddl44, actual52_cancel_after_ddl44 => 44;
    actual52_cancel_before_ddl45, actual52_cancel_after_ddl45 => 45;
    actual52_cancel_before_ddl46, actual52_cancel_after_ddl46 => 46;
    actual52_cancel_before_ddl47, actual52_cancel_after_ddl47 => 47;
    actual52_cancel_before_ddl48, actual52_cancel_after_ddl48 => 48;
    actual52_cancel_before_ddl49, actual52_cancel_after_ddl49 => 49;
    actual52_cancel_before_ddl50, actual52_cancel_after_ddl50 => 50;
    actual52_cancel_before_ddl51, actual52_cancel_after_ddl51 => 51;
    actual52_cancel_before_ddl52, actual52_cancel_after_ddl52 => 52;
}

// Stateful generated schedules, each in one fresh macro-owned database.
#[path = "generated_lifecycle.rs"]
mod generated_lifecycle;
