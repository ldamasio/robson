//! Actual upstream run_direct lineage. Faults exist only in fresh macro DBs.
#[path = "ordinary_lineage_initial.rs"]
mod initial;
use std::{borrow::Cow, time::Duration};

use anyhow::{ensure, Context, Result};
use serde_json::Value;
use sqlx::{
    migrate::{MigrationType, Migrator},
    PgConnection, PgPool,
};

use super::{
    super::{helpers::*, *},
    capture, cause, contract,
};

const HISTORY: &str = "SELECT COALESCE(jsonb_agg((to_jsonb(m)||jsonb_build_object('checksum',encode(m.checksum,'hex'))) ORDER BY version),'[]'::jsonb) FROM _sqlx_migrations m";
const SETTINGS: &str = "SELECT pg_backend_pid(),current_setting('lock_timeout'),current_setting('statement_timeout'),current_setting('idle_in_transaction_session_timeout')";
const CREATE_WITNESS: &str = "CREATE TABLE p9_lineage_witness (actual_pid integer NOT NULL, lock_limit text NOT NULL, statement_limit text NOT NULL, idle_limit text NOT NULL)";
// A real NONEMPTY ordinary39 fixture; normal Migration::new checksum, no
// override.
const SQL39: &str = "INSERT INTO p9_lineage_witness SELECT pg_backend_pid(),current_setting('lock_timeout'),current_setting('statement_timeout'),current_setting('idle_in_transaction_session_timeout')";
const ROWS: &str =
    "SELECT actual_pid,lock_limit,statement_limit,idle_limit FROM p9_lineage_witness";
const GONE: &str = "SELECT NOT EXISTS(SELECT 1 FROM pg_stat_activity WHERE pid=$1) AND NOT EXISTS(SELECT 1 FROM pg_locks WHERE pid=$1)";

#[derive(Clone, Copy, Debug)]
enum Kind {
    Matching,
    Mismatch,
    Dirty,
    Missing,
}

fn next() -> Migration {
    Migration::new(
        VERSION + 1,
        "ordinary real run_direct lineage witness".into(),
        MigrationType::Simple,
        SQL39.into(),
        false,
    )
}

fn migrator(kind: Kind) -> Migrator {
    let mut m = sqlx::migrate!("../migrations");
    let mut source: Vec<_> = m
        .iter()
        .filter(|m| {
            m.version <= VERSION && !(matches!(kind, Kind::Missing) && m.version == VERSION)
        })
        .cloned()
        .collect();
    source.push(next());
    m.migrations = Cow::Owned(source);
    m
}

async fn history(c: &mut PgConnection) -> Result<Value> {
    let value: Value = sqlx::query_scalar(HISTORY).fetch_one(c).await?;
    ensure!(
        value.as_array().is_some_and(|v| v.len() <= 64)
            && serde_json::to_vec(&value)?.len() <= 65536,
        "P9 isolated history64rows/64KiB bound"
    );
    Ok(value)
}

async fn profile(c: &mut PgConnection) -> Result<(i32, String, String, String)> {
    Ok(sqlx::query_as(SETTINGS).fetch_one(c).await?)
}


struct Baseline {
    full: Value,
    history: Value,
}

async fn finish(
    f: Fixture,
    baseline: Option<&Baseline>,
    kind: Kind,
    before: robson_testkit::db_proxy::ProxyStats,
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
    drop(adapter); // Upstream early lineage refusal does NOT poison the adapter.
    let retired = tokio::time::timeout(Duration::from_millis(300), async {
        for _ in 0..60 {
            let gone: bool = sqlx::query_scalar(GONE).bind(pid).fetch_one(&mut observer).await?;
            if gone {
                return Ok::<_, sqlx::Error>(true);
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        Ok(false)
    })
    .await
    .context("P9 original raw retirement300ms")
    .and_then(|r| r.map_err(Into::into));
    let after = proxy.stats();
    let dropped = proxy.query_trace_dropped();
    proxy.set_mode(robson_testkit::db_proxy::ProxyMode::Sever);
    drop(proxy);
    let snapshots = tokio::time::timeout_at(end, async {
        observer.lock().await?;
        let full = capture(&mut observer, "B1").await?;
        let history = history(&mut observer).await?;
        let legacy = legacy(&mut observer).await?;
        let rows: Vec<(i32, String, String, String)> =
            sqlx::query_as(ROWS).fetch_all(&mut observer).await?;
        observer.unlock().await?;
        Ok::<_, anyhow::Error>((full, history, legacy, rows))
    })
    .await
    .context("P9 fresh actual barrier/readbacks5s")
    .and_then(|r| r);
    drop(observer);
    eprintln!(
        "P9_CLEANUP kind={kind:?} retirement={} snapshots={}",
        cause(&retired),
        cause(&snapshots)
    );
    ensure!(
        retired? && after == before && dropped == 0,
        "P9 retirement/hiddenSQL/trace loss"
    );
    let (full, history, legacy, rows) = snapshots?;
    let b = baseline.context("P9 fault baseline not captured")?;
    let (a, ai) = contract::maps(&b.full)?;
    let (z, zi) = contract::maps(&full)?;
    ensure!(
        a == z && ai == zi && legacy == legacy_before,
        "P9 full semantic/native/legacy drift"
    );
    if matches!(kind, Kind::Matching) {
        ensure!(
            rows == vec![(pid, "310ms".into(), "410ms".into(), "510ms".into())],
            "ordinary39 did not execute once on restored SAME PID"
        );
        let all = history.as_array().context("P9 history not array")?;
        let old: Vec<_> = all
            .iter()
            .filter(|r| r["version"].as_i64() != Some(VERSION + 1))
            .cloned()
            .collect();
        ensure!(
            Value::Array(old) == b.history
                && all.len() == b.history.as_array().context("baseline history")?.len() + 1,
            "P9 earlier history rewritten"
        );
        let row = all
            .iter()
            .find(|r| r["version"].as_i64() == Some(VERSION + 1))
            .context("ordinary39 metadata absent")?;
        let checksum = next().checksum.iter().map(|v| format!("{v:02x}")).collect::<String>();
        ensure!(
            row["success"] == true
                && row["description"] == next().description.as_ref()
                && row["checksum"] == checksum
                && row["execution_time"].as_i64().is_some_and(|n| n >= 0)
                && row["installed_on"].is_string(),
            "ordinary39 normal upstream metadata mismatch"
        );
    } else {
        ensure!(
            history == b.history && rows.is_empty(),
            "P9 refused lineage mutated history/executed39"
        );
    }
    contract::source_hash()?;
    Ok(())
}

async fn run(pool: PgPool, kind: Kind) -> Result<()> {
    let mut f = Fixture::new(&pool).await?;
    let mut baseline = None;
    let primary = tokio::time::timeout(ACQUIRE_BOUND, async {
        initial::original38(&mut f).await?;
        sqlx::query(CREATE_WITNESS).execute(&mut f.observer).await?;
        // Deliberate TEST-ONLY isolated database faults; never a repair path.
        match kind {
            Kind::Mismatch => {
                let mut checksum = migration().checksum.to_vec();
                checksum[0] ^= 1;
                let n = sqlx::query("UPDATE _sqlx_migrations SET checksum=$1 WHERE version=$2")
                    .bind(checksum)
                    .bind(VERSION)
                    .execute(&mut f.observer)
                    .await?
                    .rows_affected();
                ensure!(n == 1, "checksum fault did not affect exactly38");
            },
            Kind::Dirty => {
                let n = sqlx::query("UPDATE _sqlx_migrations SET success=false WHERE version=$1")
                    .bind(VERSION)
                    .execute(&mut f.observer)
                    .await?
                    .rows_affected();
                ensure!(n == 1, "dirty fault did not affect exactly38");
            },
            _ => (),
        }
        baseline = Some(Baseline {
            full: capture(&mut f.observer, "C").await?,
            history: history(&mut f.observer).await?,
        });
        let source = migrator(kind);
        ensure!(
            source.locking && !source.ignore_missing && !source.no_tx,
            "P9 upstream safeguards disabled"
        );
        f.proxy.enable_query_trace(32);
        let before = f.proxy.stats();
        let result = source.run_direct(&mut f.adapter).await;
        let after = f.proxy.stats();
        let trace = f.proxy.query_trace();
        let expected_count = match kind {
            Kind::Matching => 12,
            Kind::Dirty => 4,
            _ => 5,
        };
        eprintln!(
            "P9_RUN kind={kind:?} outcome={} statements={} commits={} expected={expected_count}",
            if result.is_ok() { "Ok" } else { "Err" },
            after.statements - before.statements,
            after.commits - before.commits
        );
        let typed = match (&kind, &result) {
            (Kind::Matching, Ok(())) => true,
            (Kind::Mismatch, Err(MigrateError::VersionMismatch(v)))
            | (Kind::Dirty, Err(MigrateError::Dirty(v)))
            | (Kind::Missing, Err(MigrateError::VersionMissing(v))) => *v == VERSION,
            _ => false,
        };
        ensure!(typed, "ordinary run_direct returned wrong typed outcome: {result:?}");
        ensure!(
            after.statements - before.statements == expected_count
                && trace.len() as u64 == expected_count
                && f.proxy.query_trace_dropped() == 0
                && after.connections == before.connections
                && after.commits - before.commits == u64::from(matches!(kind, Kind::Matching)),
            "P9 housekeeping/apply39 received count"
        );
        ensure!(
            !trace
                .iter()
                .any(|s| s.starts_with("WITH old AS MATERIALIZED") || s.contains("set_config(")),
            "matching38 was reapplied or session settings changed"
        );
        let q: Vec<_> = trace.iter().map(|s| s.trim().trim_end_matches(';').trim()).collect();
        ensure!(q[0]=="SELECT current_database()" && q[1]=="SELECT pg_advisory_lock($1)"
            && q[2].starts_with("CREATE TABLE IF NOT EXISTS _sqlx_migrations")
            && q[3]=="SELECT version FROM _sqlx_migrations WHERE success = false ORDER BY version LIMIT 1"
            && (matches!(kind,Kind::Dirty) || q[4]=="SELECT version, checksum FROM _sqlx_migrations ORDER BY version"), "P9 upstream housekeeping source provenance");
        if matches!(kind,Kind::Matching) {
            ensure!(q[5]=="BEGIN" && q[6]==SQL39 && q[7]==public_apply::metadata_insert_sql_for_tests().trim() && q[8]=="COMMIT"
                && q[9].starts_with("UPDATE _sqlx_migrations") && q[10]=="SELECT current_database()" && q[11]=="SELECT pg_advisory_unlock($1)", "P9 actual ordinary39/upstream metadata/unlock source order");
        }
        let ordinary = trace.iter().filter(|s| s.trim() == SQL39).count();
        ensure!(
            ordinary == usize::from(matches!(kind, Kind::Matching)),
            "ordinary39 wrong actual SQL provenance/count"
        );
        if matches!(kind, Kind::Matching) {
            ensure!(
                profile(f.adapter.connection()?).await?
                    == (f.pid, "310ms".into(), "410ms".into(), "510ms".into()),
                "run_direct changed restored SAME PID/GUCs"
            );
        }
        Ok::<_, anyhow::Error>(())
    })
    .await
    .context("P9 fixture5s")
    .and_then(|r| r);
    let before = f.proxy.stats();
    let cleanup = finish(f, baseline.as_ref(), kind, before).await;
    eprintln!(
        "P9_FINISH kind={kind:?} primary={} cleanup={}",
        cause(&primary),
        cause(&cleanup)
    );
    primary?;
    cleanup?;
    Ok(())
}

macro_rules! lineage {
    ($name:ident,$kind:ident) => {
        #[sqlx::test(migrations = false)]
        #[ignore = "requires DATABASE_URL"]
        async fn $name(pool: PgPool) -> Result<()> {
            run(pool, Kind::$kind).await
        }
    };
}
lineage!(ordinary_matching38_skips_and_runs39_on_restored_same_pid, Matching);
lineage!(ordinary_checksum_mismatch38_refuses39_without_history_change, Mismatch);
lineage!(ordinary_dirty38_refuses39_without_history_change, Dirty);
lineage!(ordinary_missing38_source_refuses39_without_history_change, Missing);

// Upstream stop at original38 when restore fails; ordinary39 never runs.
#[path = "ordinary_restore_stop.rs"]
mod restore_stop;
