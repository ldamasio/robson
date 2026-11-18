//! Exact INSERT completion lost to existing Blackhole; never retained C/Z.
#[path = "metadata_transcript.rs"]
mod transcript;
use robson_testkit::db_proxy::{DbProxy, DiagnosticEvent as Event, DiagnosticSnapshot};
use tokio::sync::oneshot;

use super::*;

const IDENTITY: &str = "SELECT jsonb_build_object('backend',(SELECT backend_start::text FROM pg_stat_activity WHERE pid=$1),'metadata','public._sqlx_migrations'::regclass::oid::bigint,'venue','public.venue_orders'::regclass::oid::bigint,'clock','public.spot_intent_clock'::regclass::oid::bigint)";
const IDLE: &str = "SELECT EXISTS(SELECT 1 FROM pg_stat_activity a WHERE a.pid=$1 AND a.backend_start::text=$2 AND a.state='idle in transaction' AND a.xact_start IS NOT NULL AND a.backend_xid IS NOT NULL AND EXISTS(SELECT 1 FROM pg_locks WHERE pid=$1 AND relation::bigint=$3 AND mode='RowExclusiveLock' AND granted) AND EXISTS(SELECT 1 FROM pg_locks WHERE pid=$1 AND relation::bigint=$4 AND mode='AccessExclusiveLock' AND granted) AND EXISTS(SELECT 1 FROM pg_locks WHERE pid=$1 AND relation::bigint=$5 AND mode='AccessExclusiveLock' AND granted))";

async fn idle(c: &mut PgConnection, pid: i32, id: &Value) -> Result<bool> {
    Ok(sqlx::query_scalar(IDLE)
        .bind(pid)
        .bind(id["backend"].as_str().context("backend identity")?)
        .bind(id["metadata"].as_i64().context("metadata OID")?)
        .bind(id["venue"].as_i64().context("venue OID")?)
        .bind(id["clock"].as_i64().context("clock OID")?)
        .fetch_one(c)
        .await?)
}

fn trace(proxy: &DbProxy, pid: i32) -> Result<DiagnosticSnapshot> {
    let d = proxy.diagnostics().context("metadata diagnostics absent")?;
    // Physical retention/identity only. Sticky Unknown is not repaired,
    // relabeled Known, or used for ordinal/phase/control authority.
    let validity = transcript::physical(&d, pid);
    if validity.is_err() {
        let connections: Vec<_> = d
            .connections
            .iter()
            .take(4)
            .map(|c| {
                serde_json::json!({
            "connection":c.connection, "pid":c.pid, "live":c.live,
            "attribution_unknown":c.attribution_unknown})
            })
            .collect();
        let unknown: Vec<_> = d
            .records
            .iter()
            .filter(|r| r.identity.attribution_unknown || r.event == Event::AttributionLost)
            .take(4)
            .map(|r| {
                serde_json::json!({
                "micros":r.micros, "connection":r.identity.connection, "pid":r.identity.pid,
                "attribution_unknown":r.identity.attribution_unknown,
                "event":format!("{:?}",r.event), "phase":format!("{:?}",r.phase),
                "delivery":format!("{:?}",r.delivery), "ordinal":r.ordinal})
            })
            .collect();
        let payload = serde_json::json!({"expected_pid":pid,"dropped":d.dropped,
            "connection_overflow":d.connection_overflow,"phase_overflow":d.phase_overflow,
            "connection_count":d.connections.len(),"first_connections":connections,
            "retained_record_count":d.records.len(),"first_retained_unknown_events":unknown});
        match serde_json::to_string(&payload) {
            Ok(encoded) if encoded.len() <= 4000 => eprintln!("METADATA_TRACE_FAILURE {encoded}"),
            _ => eprintln!("METADATA_TRACE_FAILURE diagnostic=unavailable_or_over4000"),
        }
    }
    if let Some(r) = d.records.iter().find(|r| r.event == Event::Error) {
        eprintln!(
            "METADATA_SUCCESS_SERVER_ERROR pid={:?} phase={:?} sqlstate={:?}",
            r.identity.pid,
            r.phase,
            r.sqlstate.map(|s| String::from_utf8_lossy(&s).into_owned())
        );
    }
    validity?;
    Ok(d)
}

struct Success {
    windows: Vec<Window>,
    hit: oneshot::Receiver<()>,
    release: Option<oneshot::Sender<()>>,
    fence: transcript::Fence,
    released: Instant,
}

async fn lose_response(
    f: &mut Fixture,
    blocker: &mut Option<PgConnection>,
    blocker_pid: i32,
    id: &Value,
) -> Result<()> {
    let points = [
        (Stage::Ddl(16), Edge::AfterReturn),
        (Stage::Ddl(32), Edge::AfterReturn),
        (Stage::Ddl(48), Edge::AfterReturn),
        (Stage::Metadata, Edge::BeforeCall),
    ];
    let slot = f.adapter.gate.clone();
    let first = stages::arm(&slot, points[0].0, points[0].1)?;
    let proxy = &f.proxy;
    let observer = &mut f.observer;
    let pid = f.pid;
    let baseline = f.baseline;
    let relation = id["metadata"].as_i64().context("metadata OID missing")?;
    let controller = async {
        let mut current = first;
        let mut windows = Vec::with_capacity(4);
        for n in 0..4 {
            current.0.await.map_err(|_| anyhow::anyhow!("metadata stage hit missing"))?;
            if n < 3 {
                windows.push(Window {
                    text: proxy.query_trace(),
                    dropped: proxy.query_trace_dropped(),
                    statements: proxy.stats().statements - baseline.statements,
                });
                let next = stages::arm(&slot, points[n + 1].0, points[n + 1].1)?;
                proxy.enable_query_trace(32);
                current
                    .1
                    .send(())
                    .map_err(|_| anyhow::anyhow!("metadata milestone release lost"))?;
                current = next;
            } else {
                ensure!(
                    proxy.stats().statements - baseline.statements == 54,
                    "metadata before-call prefix"
                );
                let mut captured = None;
                for _ in 0..4 {
                    let d = trace(proxy, pid)?;
                    if let Some(fence) = transcript::Fence::capture(&d, pid)? {
                        captured = Some(fence);
                        break;
                    }
                    tokio::task::yield_now().await;
                }
                let fence = captured.context("all old Complete/Ready responses not drained")?;
                eprintln!(
                    "METADATA_SUCCESS_FENCE records={} physicalOnly=true KnownAuthority=false",
                    fence.len()
                );
                // Rejection sentinel only. It MUST remain unreached and be
                // removed before the final genuine Pending probe.
                let (hit, release) = stages::arm(&slot, Stage::Metadata, Edge::AfterReturn)?;
                let released = Instant::now();
                current.1.send(()).map_err(|_| anyhow::anyhow!("metadata release lost"))?;
                return tokio::time::timeout_at(
                    tokio::time::Instant::from_std(released) + Duration::from_millis(20),
                    async {
                        let mut locked = false;
                    for polls in 1..=4 {
                        let waiting = waiting(observer, pid, relation, blocker_pid).await?;
                        let d = trace(proxy, pid)?;
                        ensure!(!fence.complete(&d)?, "metadata completed before partition");
                        let stats = proxy.stats();
                        let received = stats.statements - baseline.statements;
                        ensure!(received <= 55 && stats.commits == baseline.commits, "metadata later SQL before partition");
                        if waiting && received == 55 {
                            locked = true;
                            break;
                        }
                        if polls < 4 { tokio::time::sleep(Duration::from_millis(5)).await; }
                    }
                    ensure!(locked, "metadata real55/native Lockwait absent");
                    eprintln!("METADATA_SUCCESS_PARTITION fenceRecords={} nativeLockwait=true received=55 KnownAuthority=false", fence.len());
                    proxy.set_mode(ProxyMode::Blackhole);
                        drop(blocker.take()); // Release real SHARE, no SQL/ROLLBACK.
                        for polls in 1..=4 {
                            let d = trace(proxy, pid)?;
                        let complete = fence.complete(&d)?;
                        let native = complete && idle(observer, pid, id).await?;
                        eprintln!("METADATA_SUCCESS_OBSERVE poll={polls} completeReadyDropped={complete} nativeIdle={native}");
                        if native {
                                windows.push(Window {
                                    text: proxy.query_trace(),
                                    dropped: proxy.query_trace_dropped(),
                                    statements: proxy.stats().statements - baseline.statements,
                                });
                                return Ok::<_, anyhow::Error>(Success {
                                    windows,
                                    hit,
                                    release: Some(release),
                                    fence,
                                    released,
                                });
                            }
                            if polls < 4 {
                                tokio::time::sleep(Duration::from_millis(5)).await;
                            }
                        }
                        anyhow::bail!("metadata actual completion/idleTX witness absent")
                    },
                )
                .await
                .context("metadata lock/response observation20 expired")?;
            }
        }
        anyhow::bail!("metadata gate absent")
    };
    let started = Instant::now();
    let witness = {
        let mut original = f.adapter.apply(migration());
        let mut controller = Box::pin(controller);
        let observed = tokio::select! {
            result = &mut original => match result {
                Err(error) => Err(anyhow::Error::from(error).context("original returned before lost-response witness")),
                Ok(_) => Err(anyhow::anyhow!("original unexpectedly succeeded before lost-response witness")),
            },
            result = &mut controller => result,
        };
        drop(controller);
        let mut witness = observed?;
        ensure!(
            witness.fence.complete(&trace(proxy, pid)?)?,
            "completion changed before final Pending"
        );
        ensure!(
            matches!(witness.hit.try_recv(), Err(oneshot::error::TryRecvError::Empty)),
            "AfterReturn55 was reached"
        );
        let sentinel = slot.lock().map_err(|_| anyhow::anyhow!("gate poisoned"))?.take();
        ensure!(sentinel.is_some(), "AfterReturn55 sentinel unexpectedly consumed");
        drop(sentinel);
        drop(witness.release.take());
        ensure!(
            slot.lock().map_err(|_| anyhow::anyhow!("gate poisoned"))?.is_none(),
            "app gate remains active"
        );
        let pending =
            std::future::poll_fn(|cx| {
                std::task::Poll::Ready(match std::future::Future::poll(original.as_mut(), cx) {
                    std::task::Poll::Pending => Ok(()),
                    std::task::Poll::Ready(Err(error)) => Err(anyhow::Error::from(error)
                        .context("original returned before final Pending")),
                    std::task::Poll::Ready(Ok(_)) => {
                        Err(anyhow::anyhow!("original returned success instead of Pending"))
                    },
                })
            })
            .await;
        let revalidated = trace(proxy, pid).and_then(|d| {
            ensure!(witness.fence.complete(&d)?, "completion changed at final original Pending");
            Ok(())
        });
        drop(original); // WHOLE original owner, after all gates are gone.
        pending?;
        revalidated?;
        witness
    };
    let status = f.adapter.status();
    let stats = proxy.stats();
    // No stored frames are released. Raw owner is gone before Pass makes
    // refusal/cleanup traffic visible; full serialized rollback is still required.
    proxy.set_mode(ProxyMode::Pass);
    let refused = refuses_after_retirement(&mut f.adapter).await;
    ensure!(
        refused
            && proxy.stats() == stats
            && stats.statements - baseline.statements == 55
            && stats.commits == baseline.commits
            && stats.commit_acks_dropped == baseline.commit_acks_dropped
            && proxy.autocommit_acks_dropped() == 0,
        "metadata refusal/39/laterSQL/ACK/prefix"
    );
    ensure!(
        status.phase == Phase::Retired
            && status.failure
                == Some(Failure {
                    at: Phase::DelegatedApply,
                    outcome: Outcome::MayHaveCommitted
                }),
        "metadata lost-response outcome"
    );
    ensure!(
        started.elapsed() < APPLY_BOUND && witness.released.elapsed() < Duration::from_millis(20),
        "metadata Pending/drop missed unchanged200/20"
    );
    let mut received = Vec::with_capacity(55);
    ensure!(witness.windows.len() == 4, "metadata received window count");
    for (n, window) in witness.windows.iter().enumerate() {
        ensure!(
            window.dropped == 0
                && window.statements == [18, 34, 50, 55][n]
                && window.text.len() == [18, 16, 16, 5][n],
            "metadata received windows incomplete"
        );
        received.extend(window.text.iter().map(|s| s.trim().trim_end_matches(';').trim()));
    }
    let chunks = fixed52::chunks(migration())?;
    let expected: Vec<_> = chunks.iter().map(|s| s.trim().trim_end_matches(';').trim()).collect();
    ensure!(
        received.len() == 55
            && received[2..54] == expected
            && received[1] == "BEGIN"
            && received[0].starts_with("WITH old AS MATERIALIZED")
            && received[54] == public_apply::metadata_insert_sql_for_tests().trim(),
        "metadata original bytes/order"
    );
    eprintln!("METADATA_SUCCESS fenceRecords={} KnownAuthority=false CompleteReceivedDropped=true ReadyTDropped=true idleTX=true AfterReturn55=false appgate=false originalPending=true elapsed={:?}", witness.fence.len(), started.elapsed());
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires DATABASE_URL"]
async fn actual52_metadata_server_success_lost_response_rolls_back(pool: PgPool) -> Result<()> {
    let mut f = Fixture::new_with_diagnostics(&pool, true).await?;
    let (mut blocker, mut blocker_pid, mut baseline) = (None, None, None);
    let primary = tokio::time::timeout(ACQUIRE_BOUND, async {
        let old = f.proxy.stats();
        f.adapter.lock().await?;
        ensure!(f.proxy.stats().statements - old.statements == 2, "metadata original advisory SQL");
        let template = public_apply::metadata_insert_sql_for_tests();
        let old = f.proxy.stats();
        let c = f.adapter.connection()?;
        let cache = c.cached_statements_size();
        let prepared = (&mut *c).prepare(template).await?;
        let parameters = match prepared.parameters() {
            Some(sqlx::Either::Left(types)) => types.len(), Some(sqlx::Either::Right(n)) => n,
            None => anyhow::bail!("metadata parameters absent"),
        };
        ensure!(parameters == 3 && prepared.sql() == template && prepared.columns().is_empty(), "metadata original public prepare");
        drop(prepared);
        ensure!(c.cached_statements_size() == cache + 1 && f.proxy.stats() == old, "metadata prepare Q/E or cache drift");
        baseline = Some(capture(&mut f.observer, "B0").await?);
        ensure!(baseline.as_ref().is_some_and(|v| v["context"]["metadata"].is_null()), "38 already present");
        let id: Value = sqlx::query_scalar(IDENTITY).bind(f.pid).fetch_one(&mut f.observer).await?;
        ensure!(id["backend"].is_string(), "metadata original backend identity absent");
        blocker = Some(pool.connect_options().connect().await?);
        let c = blocker.as_mut().context("metadata blocker absent")?;
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()").fetch_one(&mut *c).await?;
        blocker_pid = Some(pid);
        ensure!(!waiting(&mut f.observer, f.pid, id["metadata"].as_i64().context("metadata OID")?, pid).await?, "unexpected initial wait");
        let _ = idle(&mut f.observer, f.pid, &id).await?;
        let gone: bool = sqlx::query_scalar(GONE).bind(f.pid).bind(pid).fetch_one(&mut f.observer).await?;
        ensure!(!gone, "original/blocker identity absent");
        sqlx::query("BEGIN").persistent(false).execute(&mut *c).await?;
        sqlx::query("LOCK TABLE public._sqlx_migrations IN SHARE MODE").persistent(false).execute(&mut *c).await?;
        f.baseline = f.proxy.stats();
        f.proxy.enable_query_trace(32);
        eprintln!("METADATA_SUCCESS_SETUP advisorySQL=2 prepareQE=0 parameters=3 cache=1 blockerSQL=3 observerSetup=5");
        lose_response(&mut f, &mut blocker, pid, &id).await
    }).await.context("metadata success primary5s deadline").and_then(|r| r);
    let before = f.proxy.stats();
    let cleanup = finish_both(f, blocker, blocker_pid, baseline.as_ref(), before).await;
    eprintln!(
        "METADATA_SUCCESS_CLEANUP primary={} cleanup={}",
        cause(&primary),
        cause(&cleanup)
    );
    primary?;
    cleanup?;
    Ok(())
}
