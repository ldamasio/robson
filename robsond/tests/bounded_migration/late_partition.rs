//! Existing Blackhole after a real return; server cleanup, never EOF-orphan.
use robson_testkit::db_proxy::{DbProxy, DiagnosticDelivery, DiagnosticEvent};
use sqlx::PgConnection;

use super::*;

const OBSERVE: &str = "SELECT jsonb_build_object('activity',(SELECT jsonb_build_object('pid',a.pid,'backend',a.backend_start::text,'transaction',a.xact_start::text,'xid',a.backend_xid::text,'state',a.state) FROM pg_stat_activity a WHERE a.pid=$1),'locks',(SELECT count(*) FROM pg_locks WHERE pid=$1),'venue',EXISTS(SELECT 1 FROM pg_locks WHERE pid=$1 AND relation::bigint=$2 AND mode='AccessExclusiveLock' AND granted),'clock',EXISTS(SELECT 1 FROM pg_locks WHERE pid=$1 AND relation::bigint=$3 AND mode='AccessExclusiveLock' AND granted),'metadata',EXISTS(SELECT 1 FROM pg_locks WHERE pid=$1 AND relation::bigint=$4 AND mode='RowExclusiveLock' AND granted))";
const IDENTIFY: &str = "SELECT jsonb_build_object('backend',(SELECT backend_start::text FROM pg_stat_activity WHERE pid=$1),'venue','public.venue_orders'::regclass::oid::bigint,'clock','public.spot_intent_clock'::regclass::oid::bigint,'metadata','public._sqlx_migrations'::regclass::oid::bigint)";

async fn observe(c: &mut PgConnection, pid: i32, identity: &Value) -> Result<Value> {
    let value: Value = sqlx::query_scalar(OBSERVE)
        .bind(pid)
        .bind(identity["venue"].as_i64().context("venue OID missing")?)
        .bind(identity["clock"].as_i64().context("clock OID missing")?)
        .bind(identity["metadata"].as_i64().context("metadata OID missing")?)
        .fetch_one(c)
        .await?;
    ensure!(serde_json::to_vec(&value)?.len() <= 65536, "partition observer byte bound");
    Ok(value)
}

fn idle(value: &Value, identity: &Value, pid: i32, metadata: bool) -> Result<()> {
    let a = &value["activity"];
    ensure!(
        a["pid"] == pid
            && a["backend"] == identity["backend"]
            && a["backend"].is_string()
            && a["transaction"].is_string()
            && a["xid"].is_string()
            && a["state"] == "idle in transaction"
            && value["locks"].as_u64().is_some_and(|n| n > 0)
            && value["venue"] == true
            && value["clock"] == true
            && value["metadata"] == metadata,
        "partition native idle/identity/locks missing"
    );
    Ok(())
}

fn idle_report(
    value: &Value,
    identity: &Value,
    pid: i32,
    metadata: bool,
    phase: &'static str,
    poll: usize,
    elapsed: Duration,
) -> Result<()> {
    let result = idle(value, identity, pid, metadata);
    if result.is_err() {
        let a = &value["activity"];
        let payload = serde_json::json!({
            "phase": phase, "poll": poll, "elapsed_us": elapsed.as_micros(),
            "actual": {"pid": a["pid"], "backend": a["backend"],
                "transaction": a["transaction"], "xid": a["xid"], "state": a["state"],
                "locks": value["locks"], "venue": value["venue"],
                "clock": value["clock"], "metadata": value["metadata"]},
            "expected": {"pid": pid, "backend": identity["backend"],
                "venue_oid": identity["venue"], "clock_oid": identity["clock"],
                "metadata_oid": identity["metadata"], "metadata": metadata}
        });
        match serde_json::to_string(&payload) {
            Ok(encoded) if encoded.len() <= 4000 => eprintln!("PARTITION_OBSERVATION_FAILURE {encoded}"),
            _ => eprintln!("PARTITION_OBSERVATION_FAILURE phase={phase} poll={poll} diagnostic=unavailable_or_over4000"),
        }
    }
    result
}

fn retired(value: &Value) -> bool {
    value["activity"].is_null() && value["locks"] == 0
}

fn retiring(
    value: &Value,
    before: &Value,
    identity: &Value,
    pid: i32,
    poll: usize,
    elapsed: Duration,
) -> Result<()> {
    let a = &value["activity"];
    let old = &before["activity"];
    let valid = a["pid"] == pid
        && a["backend"] == identity["backend"]
        && (a["transaction"] == old["transaction"] || a["transaction"].is_null())
        && (a["xid"] == old["xid"] || a["xid"].is_null());
    if !valid {
        let payload = serde_json::json!({
            "phase": "retirement", "poll": poll, "elapsed_us": elapsed.as_micros(),
            "actual": {"pid": a["pid"], "backend": a["backend"],
                "transaction": a["transaction"], "xid": a["xid"], "state": a["state"],
                "locks": value["locks"], "venue": value["venue"],
                "clock": value["clock"], "metadata": value["metadata"]},
            "expected": {"pid": pid, "backend": identity["backend"],
                "transaction_original_or_null": old["transaction"],
                "xid_original_or_null": old["xid"]}
        });
        match serde_json::to_string(&payload) {
            Ok(encoded) if encoded.len() <= 4000 => {
                eprintln!("PARTITION_RETIREMENT_FAILURE {encoded}")
            },
            _ => eprintln!(
                "PARTITION_RETIREMENT_FAILURE poll={poll} diagnostic=unavailable_or_over4000"
            ),
        }
    }
    ensure!(valid, "partition retirement PID/backend/new-transaction drift");
    Ok(())
}

fn timeout_evidence(proxy: &DbProxy, pid: i32) -> Result<()> {
    let d = proxy.diagnostics().context("partition diagnostics missing")?;
    ensure!(
        d.dropped == 0
            && d.connection_overflow == 0
            && d.phase_overflow == 0
            && d.connections.len() == 1
            && d.connections[0].pid == Some(pid),
        "partition diagnostic overflow/identity"
    );
    // Fatal responses deliberately lose phase attribution, not actual PID.
    let records: Vec<_> = d
        .records
        .iter()
        .filter(|r| {
            r.identity.connection == d.connections[0].connection
                && r.identity.pid == Some(pid)
                && r.event == DiagnosticEvent::Error
                && r.sqlstate == Some(*b"25P03")
        })
        .collect();
    ensure!(
        records.iter().filter(|r| r.delivery == DiagnosticDelivery::Received).count() == 1
            && records.iter().filter(|r| r.delivery == DiagnosticDelivery::Dropped).count() == 1,
        "actual server idle25P03 Received+Dropped missing"
    );
    Ok(())
}

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
            for polls in 1..=60 {
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
                if polls < 60 {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            }
            anyhow::bail!("partition backend/locks survived bounded observation")
        };
        let producer = async {
            let result = (&mut original).await;
            (result.map_err(anyhow::Error::from), started.elapsed())
        };
        // The ORIGINAL global200 future continues to be polled, never reset.
        let ((apply, duration), server) = tokio::join!(
            producer,
            tokio::time::timeout_at(
                tokio::time::Instant::from_std(started) + APPLY_BOUND,
                watching
            )
        );
        drop(original);
        let server = server.context("partition server200 observation deadline").and_then(|r| r);
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
        "partition did not fail visibly/clean server within original200"
    );
    ensure!(
        status.phase == Phase::Retired
            && status.failure
                == Some(Failure {
                    at: Phase::DelegatedApply,
                    outcome: Outcome::MayHaveCommitted
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

async fn run_partition(pool: PgPool, cut: Cut) -> Result<()> {
    let mut f = Fixture::new_with_diagnostics(&pool, true).await?;
    let mut baseline = None;
    let primary = tokio::time::timeout(ACQUIRE_BOUND, async {
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
    .context("partition primary5s deadline")
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

#[sqlx::test(migrations = false)]
#[ignore = "requires DATABASE_URL"]
async fn actual52_partition_after_ddl43_releases_server_locks(pool: PgPool) -> Result<()> {
    run_partition(pool, Cut::AfterDdl(43)).await
}

#[sqlx::test(migrations = false)]
#[ignore = "requires DATABASE_URL"]
async fn actual52_partition_after_ddl52_releases_server_locks(pool: PgPool) -> Result<()> {
    run_partition(pool, Cut::AfterDdl(52)).await
}

#[sqlx::test(migrations = false)]
#[ignore = "requires DATABASE_URL"]
async fn actual52_partition_after_metadata_releases_server_locks(pool: PgPool) -> Result<()> {
    run_partition(pool, Cut::AfterMetadata).await
}

#[cfg(test)]
mod retirement_guards {
    use super::*;

    fn snapshot(pid: i32, xid: Option<&str>, locks: u64) -> Value {
        serde_json::json!({"activity": {"pid":pid,
            "backend":"2026-11-17 13:28:37.808715+00",
            "transaction":"2026-11-17 13:28:37.911301+00", "xid":xid,
            "state":"idle in transaction"}, "locks":locks,
            "venue":locks>1, "clock":locks>1, "metadata":false})
    }

    #[test]
    fn recorded_teardown_releases_resources_before_activity_exit() {
        // Recorded failing sample: same43/backend/transaction, xidNULL,
        // locks1 and both relation locks gone. Original xid is immaterial.
        let before = snapshot(43, Some("7"), 3);
        let identity = serde_json::json!({"backend":before["activity"]["backend"]});
        let middle = snapshot(43, None, 1);
        assert!(retiring(&middle, &before, &identity, 43, 4, Duration::ZERO).is_ok());
        assert!(!retired(&middle));
        let residual = serde_json::json!({"activity":null,"locks":1});
        assert!(!retired(&residual));
        assert!(retired(&serde_json::json!({"activity":null,"locks":0})));
    }

    proptest::proptest! {
        #![proptest_config(proptest::test_runner::Config::with_cases(64))]
        #[test]
        fn new_owner_or_transaction_is_never_accepted(
            pid in 1_i32..1_000_000, xid in 1_u64..1_000_000, field in 0_u8..4,
        ) {
            let xid = xid.to_string();
            let before = snapshot(pid, Some(&xid), 3);
            let identity = serde_json::json!({"backend":before["activity"]["backend"]});
            let mut changed = before.clone();
            match field {
                0 => changed["activity"]["pid"] = (pid + 1).into(),
                1 => changed["activity"]["backend"] = "reused backend".into(),
                2 => changed["activity"]["transaction"] = "new transaction".into(),
                _ => changed["activity"]["xid"] = format!("{xid}0").into(),
            }
            proptest::prop_assert!(retiring(&changed, &before, &identity, pid, 2, Duration::ZERO).is_err());
            proptest::prop_assert!(!retired(&changed));
        }

        #[test]
        fn trace_completes_only_after_all_native_resources_exit(
            states in proptest::collection::vec(0_u8..3, 1..32),
        ) {
            let before = snapshot(43, Some("7"), 3);
            let identity = serde_json::json!({"backend":before["activity"]["backend"]});
            let mut trace = Vec::with_capacity(states.len() + 1);
            for state in states {
                let value = match state {
                    0 => before.clone(),
                    1 => snapshot(43, None, 0),
                    _ => serde_json::json!({"activity":null,"locks":1}),
                };
                if !value["activity"].is_null() {
                    proptest::prop_assert!(retiring(&value, &before, &identity, 43, 2, Duration::ZERO).is_ok());
                }
                trace.push(value);
            }
            let terminal = trace.len();
            trace.push(serde_json::json!({"activity":null,"locks":0}));
            proptest::prop_assert_eq!(trace.iter().position(retired), Some(terminal));
        }
    }
}
