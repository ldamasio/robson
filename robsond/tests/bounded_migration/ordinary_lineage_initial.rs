//! Original38 applied by the actual upstream Migrator, four bounded windows.
use super::{
    super::super::stages::{self, Edge, Stage},
    *,
};

pub(super) async fn original38(f: &mut Fixture) -> Result<()> {
    let mut source = sqlx::migrate!("../migrations");
    source.migrations =
        Cow::Owned(source.iter().filter(|m| m.version <= VERSION).cloned().collect());
    let slot = f.adapter.gate.clone();
    let points = [
        Stage::Ddl(16),
        Stage::Ddl(32),
        Stage::Ddl(48),
        Stage::Restore,
    ];
    let first = stages::arm(&slot, points[0], Edge::AfterReturn)?;
    let baseline = f.baseline;
    let proxy = &f.proxy;
    let controller = async {
        let mut current = first;
        let mut windows = Vec::with_capacity(4);
        for n in 0..4 {
            current
                .0
                .await
                .map_err(|_| anyhow::anyhow!("initial real run_direct gate absent"))?;
            windows.push(Window {
                text: proxy.query_trace(),
                dropped: proxy.query_trace_dropped(),
                statements: proxy.stats().statements - baseline.statements,
            });
            let next = if n < 3 {
                Some(stages::arm(&slot, points[n + 1], Edge::AfterReturn)?)
            } else {
                None
            };
            proxy.enable_query_trace(32);
            current
                .1
                .send(())
                .map_err(|_| anyhow::anyhow!("initial real run_direct gate release lost"))?;
            if let Some(next) = next {
                current = next;
            } else {
                break;
            }
        }
        Ok::<_, anyhow::Error>(windows)
    };
    let began = std::time::Instant::now();
    let (result, windows) = tokio::join!(
        source.run_direct(&mut f.adapter),
        tokio::time::timeout(CONTROLLER_CEILING, controller)
    );
    eprintln!(
        "P9_INITIAL_REAL_RUN_DIRECT total_including_housekeeping={:?} result={}",
        began.elapsed(),
        if result.is_ok() { "Ok" } else { "Err" }
    );
    let windows = windows.context("initial stage observer ceiling")?;
    result?;
    ensure!(
        f.adapter.status() == (Status { phase: Phase::Restored, failure: None }),
        "initial real run_direct did not restore"
    );
    let windows = windows?;
    let mut sql = Vec::with_capacity(63);
    ensure!(windows.len() == 4, "original38 trace windows absent");
    for (n, w) in windows.iter().enumerate() {
        ensure!(
            w.dropped == 0
                && w.statements == [23, 39, 55, 63][n]
                && w.text.len() == [23, 16, 16, 8][n],
            "original38 received trace incomplete"
        );
        sql.extend(w.text.iter().map(|s| s.trim().trim_end_matches(';').trim()));
    }
    let chunks = fixed52::chunks(migration())?;
    let tail = f.proxy.query_trace();
    ensure!(
        f.proxy.query_trace_dropped() == 0
            && tail.len() == 2
            && tail[0].trim() == "SELECT current_database()"
            && tail[1].trim() == "SELECT pg_advisory_unlock($1)"
            && f.proxy.stats().statements - baseline.statements == 65
            && f.proxy.stats().commits - baseline.commits == 1,
        "initial run_direct final unlock/65/COMMIT"
    );
    ensure!(sql[0]=="SELECT current_database()" && sql[1]=="SELECT pg_advisory_lock($1)" && sql[2].starts_with("CREATE TABLE IF NOT EXISTS _sqlx_migrations")
        && sql[3]=="SELECT version FROM _sqlx_migrations WHERE success = false ORDER BY version LIMIT 1" && sql[4]=="SELECT version, checksum FROM _sqlx_migrations ORDER BY version","initial upstream housekeeping provenance");
    let sql = &sql[5..];
    let expected: Vec<_> = chunks.iter().map(|s| s.trim().trim_end_matches(';').trim()).collect();
    ensure!(
        sql.len() == 58
            && sql[2..54] == expected
            && sql[1] == "BEGIN"
            && sql[54] == public_apply::metadata_insert_sql_for_tests().trim()
            && sql[55] == "COMMIT"
            && sql[56].starts_with("UPDATE _sqlx_migrations")
            && sql[57].starts_with("SELECT set_config("),
        "original38 source/order changed"
    );
    ensure!(
        profile(f.adapter.connection()?).await?
            == (f.pid, "310ms".into(), "410ms".into(), "510ms".into()),
        "original38 PID/settings not restored"
    );
    let m = metadata(&mut f.observer).await?.context("original38 metadata absent")?;
    ensure!(
        m.0 == migration().description.as_ref()
            && m.1
            && m.2 == migration().checksum.as_ref()
            && m.3 >= 0,
        "original38 real checksum/history not applied"
    );
    contract::source_hash()?;
    Ok(())
}
