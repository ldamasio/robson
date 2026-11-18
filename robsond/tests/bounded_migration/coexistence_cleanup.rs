//! Moved private coexistence cleanup/readback; no new owner or authority.
use super::*;

pub(super) async fn finish(
    f: Fixture,
    journal_proxy: Option<robson_testkit::db_proxy::DbProxy>,
    mut owned_pids: Vec<i32>,
    snapshots: (Option<Value>, Option<Value>),
    expected: (Uuid, String, Option<Uuid>, Option<Incident>),
    work: Work,
) -> Result<()> {
    // Setup failures still retire every PID actually captured by the owned proxy.
    if let Some(d) = journal_proxy.as_ref().and_then(|p| p.diagnostics()) {
        owned_pids.extend(d.connections.iter().filter_map(|c| c.pid));
        owned_pids.sort_unstable();
        owned_pids.dedup();
    }
    let (baseline, old_rows) = snapshots;
    let (tenant, client, expected_intent, expected_incident) = expected;
    let before_cleanup = f.proxy.stats();
    let journal_before_cleanup = journal_proxy.as_ref().map(|p| p.stats());
    let Fixture { adapter, proxy, mut observer, pid: _, .. } = f;
    drop(adapter);
    let cleanup_deadline = tokio::time::Instant::now() + ACQUIRE_BOUND;
    let cleanup = async {
        let mut gone = false;
        let mut polls = 0;
        tokio::time::timeout(Duration::from_millis(300), async {
            for n in 1..=60 {
                polls = n;
                gone = sqlx::query_scalar("SELECT NOT EXISTS(SELECT 1 FROM pg_stat_activity WHERE pid=ANY($1)) AND NOT EXISTS(SELECT 1 FROM pg_locks WHERE pid=ANY($1))")
                    .bind(&owned_pids).fetch_one(&mut observer).await?;
                eprintln!("LEGACY_COEXIST_RETIRE poll={n} gone={gone}");
                if gone { break; } tokio::time::sleep(Duration::from_millis(5)).await;
            }
            Ok::<_, anyhow::Error>(())
        }).await.context("coexistence retirement300")??;
        ensure!(gone, "coexistence owned backend/locks survived");
        observer.lock().await?;
        let full = capture(&mut observer, "B1").await?;
        let after = rows(&mut observer, tenant, &client).await?;
        let m = metadata(&mut observer).await?;
        let incident_exact: bool = if let Some(i) = expected_incident.as_ref() {
            sqlx::query_scalar("SELECT count(*)=1 AND bool_and(reason=$2 AND first_at=$3 AND last_at=$4 AND count=$5 AND first_generation=$6 AND last_generation=$7 AND released_at IS NULL) FROM recovery_incidents WHERE tenant_id=$1")
                .bind(tenant).bind(i.reason.as_str()).bind(i.first_at).bind(i.last_at).bind(i.count).bind(i.first_generation).bind(i.last_generation)
                .fetch_one(&mut observer).await?
        } else {
            true
        };
        observer.unlock().await?;
        let old = old_rows.as_ref().context("coexistence baseline rows missing")?;
        ensure!(
            old["intents"].is_null()
                && old["attempts"].is_null()
                && old["incidents"].is_null()
                && old["old"] == after["old"],
            "coexistence changed prior historical rows"
        );
        let (a, ai) =
            contract::maps(baseline.as_ref().context("coexistence full baseline absent")?)?;
        let (b, bi) = contract::maps(&full)?;
        ensure!(
            a == b && ai == bi && m.is_none(),
            "coexistence rollback/catalog/native identity/no38"
        );
        match work {
            Work::Intent => {
                let i = after["intents"].as_array().context("legacy intent absent")?;
                let a = after["attempts"].as_array().context("legacy attempt absent")?;
                ensure!(
                    i.len() == 1 && a.len() == 1 && after["incidents"].is_null(),
                    "legacy batch extra/missing rows"
                );
                ensure!(
                    i[0]["intent_id"]
                        == expected_intent.context("intent identity missing")?.to_string()
                        && i[0]["tenant_id"] == tenant.to_string()
                        && i[0]["venue_id"] == "binance:usdm"
                        && i[0]["account_scope"] == DEFAULT_ACCOUNT_SCOPE
                        && i[0]["client_order_id"] == client
                        && i[0]["kind"] == "exit"
                        && i[0]["params"] == serde_json::json!({"symbol":"BTCUSDT"})
                        && i[0]["position_id"].is_null()
                        && i[0]["cancels_order_id"].is_null()
                        && i[0]["venue_outcome"].is_null()
                        && i[0]["venue_evidence"].is_null()
                        && a[0]["state"] == "started"
                        && a[0]["rejected_at"].is_null(),
                    "legacy exact committed business fields"
                );
            },
            Work::Incident => ensure!(
                incident_exact
                    && after["intents"].is_null()
                    && after["attempts"].is_null()
                    && after["incidents"].as_array().is_some_and(|v| v.len() == 1),
                "legacy incident did not persist alone/exactly"
            ),
        }
        ensure!(
            proxy.stats() == before_cleanup
                && proxy.query_trace_dropped() == 0
                && journal_proxy.as_ref().map(|p| p.stats()) == journal_before_cleanup,
            "cleanup flushed hidden SQL"
        );
        eprintln!("LEGACY_COEXIST_CLEANUP owned_pids={} retirement_polls={polls} native_locks_zero=true serialization_sql=4 snapshots_sql=3 incident_exact_sql={} journal_no_worker=true", owned_pids.len(), usize::from(expected_incident.is_some()));
        Ok::<_, anyhow::Error>(())
    };
    let cleanup = tokio::time::timeout_at(cleanup_deadline, cleanup)
        .await
        .context("coexistence cleanup5s")
        .and_then(|v| v);
    drop(observer);
    proxy.set_mode(robson_testkit::db_proxy::ProxyMode::Sever);
    drop(proxy);
    if let Some(p) = journal_proxy {
        p.set_mode(robson_testkit::db_proxy::ProxyMode::Sever);
        drop(p);
    }
    cleanup
}
