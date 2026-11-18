//! Upstream run_direct must stop at original38 when its restore fails: the
//! adapter retires conservatively and ordinary39 never runs. Fresh macro DB
//! only.
use robson_testkit::db_proxy::ProxyMode;

use super::{
    super::super::stages::{self, Edge, Stage},
    *,
};

#[sqlx::test(migrations = false)]
#[ignore = "requires DATABASE_URL"]
async fn ordinary39_not_run_after_original38_restore_failure(pool: PgPool) -> Result<()> {
    let mut f = Fixture::new(&pool).await?;
    let pid = f.pid;
    let primary = tokio::time::timeout(ACQUIRE_BOUND, async {
        sqlx::query(CREATE_WITNESS).execute(&mut f.observer).await?;
        let source = migrator(Kind::Matching);
        ensure!(
            source.locking && !source.ignore_missing && !source.no_tx,
            "restore-stop upstream safeguards disabled"
        );
        let slot = f.adapter.gate.clone();
        let (hit, release) = stages::arm(&slot, Stage::Restore, Edge::BeforeCall)?;
        let proxy = &f.proxy;
        let controller = async {
            hit.await.map_err(|_| anyhow::anyhow!("restore gate not reached"))?;
            // Committed original38; the real connection dies before restore.
            proxy.set_mode(ProxyMode::Sever);
            release.send(()).map_err(|_| anyhow::anyhow!("restore release lost"))?;
            Ok::<_, anyhow::Error>(())
        };
        let (result, gated) = tokio::join!(source.run_direct(&mut f.adapter), controller);
        let status = f.adapter.status();
        eprintln!(
            "RESTORE_STOP result={} status={status:?}",
            if result.is_ok() { "Ok" } else { "Err" }
        );
        gated?;
        ensure!(result.is_err(), "run_direct succeeded after original38 restore failure");
        ensure!(
            status
                == Status {
                    phase: Phase::Retired,
                    failure: Some(Failure {
                        at: Phase::RestoreStarted,
                        outcome: Outcome::AppliedButSessionUnrestored,
                    }),
                },
            "restore failure did not retire the adapter conservatively"
        );
        Ok::<_, anyhow::Error>(())
    })
    .await
    .context("restore-stop fixture5s")
    .and_then(|r| r);
    // Durable lineage after the original backend is gone: 38 committed, 39 absent.
    // One absolute 5 s cleanup deadline covers retirement and every readback.
    let end = tokio::time::Instant::now() + ACQUIRE_BOUND;
    let observed = tokio::time::timeout_at(end, async {
        let gone = tokio::time::timeout(Duration::from_millis(300), async {
            for _ in 0..60 {
                if sqlx::query_scalar::<_, bool>(GONE).bind(pid).fetch_one(&mut f.observer).await? {
                    return Ok::<_, sqlx::Error>(true);
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            Ok(false)
        })
        .await
        .context("restore-stop retirement300ms")??;
        ensure!(gone, "restore-stop original backend not retired");
        let versions: Vec<(i64, bool)> = sqlx::query_as(
            "SELECT version, success FROM _sqlx_migrations WHERE version >= $1 ORDER BY version",
        )
        .bind(VERSION)
        .fetch_all(&mut f.observer)
        .await?;
        let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM p9_lineage_witness")
            .fetch_one(&mut f.observer)
            .await?;
        eprintln!("RESTORE_STOP_LINEAGE versions={versions:?} witness_rows={rows}");
        ensure!(
            versions == vec![(VERSION, true)] && rows == 0,
            "ordinary39 ran or original38 not committed after restore failure"
        );
        Ok::<_, anyhow::Error>(())
    })
    .await
    .context("restore-stop shared cleanup5s")
    .and_then(|r| r);
    drop(f);
    primary?;
    observed
}
