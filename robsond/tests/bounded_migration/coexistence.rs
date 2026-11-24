//! Actual unflagged journal writes blocked by original52, never a Spot grant.
#[path = "coexistence_cleanup.rs"]
mod cleanup;
use std::{io::Write, sync::Arc, time::Instant};

use robson_exec::{
    gate::{Incident, IncidentReason},
    order_journal::{
        IntentKind, IntentSpec, OrderJournal, WriteClass, COMMIT_BOUND, DEFAULT_ACCOUNT_SCOPE,
    },
    venue::VenueId,
};
use robsond::order_journal_store::PgJournalBackend;
use sqlx::{pool::PoolOptions, postgres::PgSslMode};

use super::{
    super::{
        helpers::*,
        stages::{self, Edge, Stage},
        *,
    },
    capture, contract, *,
};

const WARM: &str =
    "SELECT pid,backend_start::text,state FROM pg_stat_activity WHERE pid=ANY($1) ORDER BY pid";
const WAIT: &str = "SELECT a.pid FROM pg_stat_activity a JOIN unnest($1::int[],$2::text[]) w(pid,backend) ON a.pid=w.pid AND a.backend_start::text=w.backend WHERE a.state='active' AND a.wait_event_type='Lock' AND $3=ANY(pg_blocking_pids(a.pid)) AND EXISTS(SELECT 1 FROM pg_locks l WHERE l.pid=a.pid AND l.relation::bigint=$5 AND l.mode='RowExclusiveLock' AND NOT l.granted) AND EXISTS(SELECT 1 FROM pg_stat_activity m WHERE m.pid=$3 AND m.backend_start::text=$4 AND m.state='idle in transaction' AND m.backend_xid IS NOT NULL AND EXISTS(SELECT 1 FROM pg_locks l WHERE l.pid=m.pid AND l.relation::bigint=$5 AND l.granted))";
const ROWS: &str = "SELECT jsonb_build_object('old',jsonb_build_object('intents',(SELECT jsonb_agg(to_jsonb(i) ORDER BY intent_id) FROM order_intents i WHERE NOT(tenant_id=$1 AND client_order_id=$2)),'attempts',(SELECT jsonb_agg(to_jsonb(a) ORDER BY attempt_id) FROM submit_attempts a WHERE NOT(venue_id='binance:usdm' AND account_scope='operated' AND client_order_id=$2)),'incidents',(SELECT jsonb_agg(to_jsonb(r) ORDER BY incident_id) FROM recovery_incidents r WHERE tenant_id<>$1),'history',(SELECT jsonb_agg(to_jsonb(m) ORDER BY version) FROM _sqlx_migrations m WHERE version<20240101000038)),'intents',(SELECT jsonb_agg(to_jsonb(i)) FROM order_intents i WHERE tenant_id=$1 AND client_order_id=$2),'attempts',(SELECT jsonb_agg(to_jsonb(a)) FROM submit_attempts a WHERE venue_id='binance:usdm' AND account_scope='operated' AND client_order_id=$2),'incidents',(SELECT jsonb_agg(to_jsonb(r)) FROM recovery_incidents r WHERE tenant_id=$1))";

#[derive(Clone, Copy, Debug)]
enum Work {
    Intent,
    Incident,
}

fn identities(proxy: &robson_testkit::db_proxy::DbProxy) -> Result<Vec<i32>> {
    let d = proxy.diagnostics().context("coexistence diagnostics absent")?;
    ensure!(
        d.dropped == 0 && d.connection_overflow == 0 && d.phase_overflow == 0,
        "coexistence diagnostic loss"
    );
    let mut pids: Vec<_> = d
        .connections
        .iter()
        .filter(|c| c.live)
        .map(|c| c.pid.context("journal startup incomplete"))
        .collect::<Result<_>>()?;
    pids.sort_unstable();
    ensure!(!pids.is_empty() && pids.len() <= 2, "journal actual pool identity bound");
    Ok(pids)
}

async fn rows(c: &mut PgConnection, tenant: Uuid, client: &str) -> Result<Value> {
    Ok(sqlx::query_scalar(ROWS).bind(tenant).bind(client).fetch_one(c).await?)
}

async fn contend(
    f: &mut Fixture,
    journal_proxy: &robson_testkit::db_proxy::DbProxy,
    work: Work,
    job: impl std::future::Future<Output = Result<usize>>,
    warm: &[(i32, String)],
    actor_backend: &str,
    relation: i64,
) -> Result<()> {
    let slot = f.adapter.gate.clone();
    let points = [16, 32, 48, 52];
    let first = stages::arm(&slot, Stage::Ddl(16), Edge::AfterReturn)?;
    let pids: Vec<_> = warm.iter().map(|v| v.0).collect();
    let backends: Vec<_> = warm.iter().map(|v| v.1.clone()).collect();
    let before = f.baseline;
    let journal_before = journal_proxy.stats();
    let proxy = &f.proxy;
    let observer = &mut f.observer;
    let actor = f.pid;
    let mut early = None;
    let mut job = Box::pin(async {
        let start = Instant::now();
        (job.await, start.elapsed())
    });
    let mut controller = Box::pin(async {
        let mut current = first;
        let mut windows = Vec::with_capacity(4);
        for n in 0..4 {
            current.0.await.context("coexistence stage absent")?;
            windows.push(Window {
                text: proxy.query_trace(),
                dropped: proxy.query_trace_dropped(),
                statements: proxy.stats().statements - before.statements,
            });
            if n < 3 {
                let next = stages::arm(&slot, Stage::Ddl(points[n + 1]), Edge::AfterReturn)?;
                proxy.enable_query_trace(32);
                current.1.send(()).map_err(|_| anyhow::anyhow!("coexistence release lost"))?;
                current = next;
            } else {
                ensure!(
                    proxy.stats().statements - before.statements == 54,
                    "coexistence did not follow original52"
                );
                let wait = async {
                    for poll in 1..=4 {
                        let waiting: Vec<i32> = sqlx::query_scalar(WAIT)
                            .bind(&pids)
                            .bind(&backends)
                            .bind(actor)
                            .bind(actor_backend)
                            .bind(relation)
                            .fetch_all(&mut *observer)
                            .await?;
                        if waiting.len() == 1 {
                            return Ok::<_, anyhow::Error>((waiting[0], poll));
                        }
                        ensure!(waiting.is_empty(), "more than one journal operation waiting");
                        if poll < 4 {
                            tokio::time::sleep(Duration::from_millis(5)).await;
                        }
                    }
                    anyhow::bail!("actual journal wait/blocker not observed")
                };
                let seen = tokio::select! {
                    result = &mut job => { early = Some(result); anyhow::bail!("journal returned before native wait"); },
                    result = tokio::time::timeout(Duration::from_millis(20), wait) => result.context("coexistence 20ms window inside idle 1s")??,
                };
                return Ok::<_, anyhow::Error>((windows, current.1, seen));
            }
        }
        anyhow::bail!("coexistence final stage missing")
    });
    let started = Instant::now();
    let observed = {
        let mut original = f.adapter.apply(migration());
        let seen = tokio::select! {
            _ = &mut original => Err(anyhow::anyhow!("migration returned before coexistence cancellation")),
            result = &mut controller => result,
        };
        drop(original); // Borrowed transaction/future, then OwnedApply's raw connection.
        seen
    };
    drop(controller); // Release the borrow of the SAME journal future, never replace it.
    let observed = observed.map(|(windows, release, seen)| {
        drop(release);
        (windows, seen)
    });
    let cancelled_at = started.elapsed();
    let cut_stats = f.proxy.stats();
    let refused = refuses_after_retirement(&mut f.adapter).await;
    let after_refusals = f.proxy.stats();
    let (job_result, job_duration) = match early {
        Some(v) => v,
        None => job.await,
    };
    let after = f.proxy.stats();
    // Diagnostic only: the measured values behind the cancellation/outcome and
    // actor checks below, written once straight to stderr (not
    // captured by libtest) after the job has finished.
    writeln!(
        std::io::stderr().lock(),
        "COEXIST_DIAG work={work:?} cancelled_at={cancelled_at:?} apply_bound={APPLY_BOUND:?} status={:?} observed_ok={} refused={refused} actor_statements={} actor_commits={} job_ok={:?} job_err={} job_duration={job_duration:?}",
        f.adapter.status(),
        observed.is_ok(),
        cut_stats.statements - before.statements,
        cut_stats.commits - before.commits,
        job_result.as_ref().ok(),
        job_result.is_err(),
    )?;
    let (windows, (waiting_pid, polls)) = observed?;
    ensure!(
        slot.lock().map_err(|_| anyhow::anyhow!("gate poisoned"))?.is_none(),
        "app gate remains"
    );
    ensure!(
        cancelled_at < APPLY_BOUND
            && f.adapter.status().phase == Phase::Retired
            && f.adapter.status().failure
                == Some(Failure {
                    at: Phase::DelegatedApply,
                    outcome: Outcome::MayHaveCommitted
                }),
        "coexistence original owner cancellation/outcome"
    );
    ensure!(
        refused
            && after_refusals == cut_stats
            && cut_stats.statements - before.statements == 54
            && cut_stats.commits == before.commits,
        "coexistence exact actor54, no39/noCOMMIT"
    );
    ensure!(
        job_result? == 1 && job_duration < COMMIT_BOUND,
        "legacy500 operation did not commit"
    );
    // Each journal transaction takes the certificate's key after BEGIN (8m-2a).
    let expected = match work {
        Work::Intent => 5,
        Work::Incident => 4,
    };
    let journal_after = journal_proxy.stats();
    ensure!(
        after == cut_stats
            && journal_after.statements - journal_before.statements == expected
            && journal_after.commits - journal_before.commits == 1
            && journal_after.connections == journal_before.connections
            && journal_after.commit_acks_dropped == journal_before.commit_acks_dropped
            && journal_proxy.autocommit_acks_dropped() == 0
            && journal_proxy.query_trace_dropped() == 0
            && f.proxy.query_trace_dropped() == 0,
        "coexistence separate actual actor54/journal5or4 inventory"
    );
    let trace = journal_proxy.query_trace();
    ensure!(
        trace.len() as u64 == expected
            && trace[0] == "BEGIN"
            && trace.last().is_some_and(|v| v == "COMMIT")
            && trace[1].starts_with("SELECT pg_advisory_xact_lock_shared(")
            && trace[2].contains(match work {
                Work::Intent => "INSERT INTO order_intents",
                Work::Incident => "INSERT INTO recovery_incidents",
            })
            && (!matches!(work, Work::Intent) || trace[3].contains("INSERT INTO submit_attempts")),
        "actual journal statement identities/order"
    );
    let mut received = Vec::with_capacity(52);
    for (n, w) in windows.iter().enumerate() {
        ensure!(
            w.dropped == 0
                && w.statements == [18, 34, 50, 54][n]
                && w.text.len() == [18, 16, 16, 4][n],
            "coexistence actor window"
        );
        received.extend(
            w.text[if n == 0 { 2 } else { 0 }..]
                .iter()
                .map(|s| s.trim().trim_end_matches(';').trim()),
        );
    }
    let chunks = fixed52::chunks(migration())?;
    ensure!(
        received
            == chunks.iter().map(|s| s.trim().trim_end_matches(';').trim()).collect::<Vec<_>>(),
        "coexistence original52 bytes/order"
    );
    ensure!(identities(journal_proxy)? == pids, "journal warm identity changed");
    eprintln!("LEGACY_COEXIST work={work:?} actor_prefix=54 actor_commit=0 journal_sql={expected} journal_commit=1 job_ms={:?} actor_drop_ms={cancelled_at:?} waiting_pid={waiting_pid} observer_wait_polls={polls} gates_clear=true", job_duration.as_millis());
    Ok(())
}

async fn run(pool: PgPool, work: Work) -> Result<()> {
    let mut f = Fixture::new_with_diagnostics(&pool, true).await?;
    let tenant = Uuid::now_v7();
    let client = format!("legacy-coexist-{tenant}");
    let mut journal = None;
    let mut journal_proxy = None;
    let mut operation = None;
    let mut intent = None;
    let mut expected_incident = None;
    let mut baseline = None;
    let mut old_rows = None;
    let mut owned_pids = vec![f.pid];
    let primary = tokio::time::timeout(APPLY_FIXTURE, async {
        // Second INSTANCE of the unchanged proxy isolates actual journal wire counts.
        let supplied = pool.connect_options();
        let upstream = tokio::net::lookup_host((supplied.get_host(), supplied.get_port())).await?
            .next().context("supplied journal PG address absent")?;
        journal_proxy = Some(robson_testkit::db_proxy::DbProxy::start(upstream).await?);
        let jp = journal_proxy.as_ref().unwrap(); jp.enable_diagnostics();
        let options = supplied.as_ref().clone().host("127.0.0.1")
            .port(jp.addr().port()).ssl_mode(PgSslMode::Disable);
        let carrier = PoolOptions::new().connect_lazy_with(options);
        let j = Arc::new(OrderJournal::new(Arc::new(PgJournalBackend::new(&carrier, tenant)), &VenueId::new("binance:usdm")));
        drop(carrier);
        journal = Some(Arc::clone(&j));
        let ready_before = jp.stats();
        j.check_ready().await.map_err(|_| anyhow::anyhow!("legacy readiness failed"))?;
        let ready_sql = jp.stats().statements - ready_before.statements;
        let pids = identities(jp)?;
        owned_pids.extend(pids.iter());
        let mut all = pids.clone(); all.push(f.pid);
        let actual: Vec<(i32,String,String)> = sqlx::query_as(WARM).bind(&all).fetch_all(&mut f.observer).await?;
        ensure!(actual.len() == all.len() && actual.iter().all(|v| v.2 == "idle"), "warm actual backend identities/state");
        let actor_backend = actual.iter().find(|v| v.0 == f.pid).context("actor backend identity absent")?.1.clone();
        let warm: Vec<_> = actual.into_iter().filter(|v| v.0 != f.pid).map(|v| (v.0,v.1)).collect();
        let relation: i64 = sqlx::query_scalar("SELECT CASE WHEN $1 THEN 'public.order_intents'::regclass::oid::bigint ELSE 'public.recovery_incidents'::regclass::oid::bigint END")
            .bind(matches!(work, Work::Intent)).fetch_one(&mut f.observer).await?;
        let wp: Vec<_> = warm.iter().map(|v| v.0).collect(); let wb: Vec<_> = warm.iter().map(|v| v.1.clone()).collect();
        let empty: Vec<i32> = sqlx::query_scalar(WAIT).bind(&wp).bind(&wb).bind(f.pid).bind(&actor_backend).bind(relation).fetch_all(&mut f.observer).await?;
        ensure!(empty.is_empty(), "warm query unexpectedly sees contention");
        let advisory_before = f.proxy.stats();
        f.adapter.lock().await?; // Original public advisory lock, separately counted2SQL.
        ensure!(f.proxy.stats().statements - advisory_before.statements == 2, "actual advisory setup inventory");
        baseline = Some(capture(&mut f.observer, "B0").await?);
        old_rows = Some(rows(&mut f.observer, tenant, &client).await?);
        ensure!(metadata(&mut f.observer).await?.is_none(), "38 already applied");
        match work {
            Work::Intent => {
                operation = Some(j.begin_operation());
                intent = Some(operation.as_ref().unwrap().scope(async { j.open_intent(IntentSpec {
                    position_id: None, kind: IntentKind::Exit, client_order_id: client.clone(), cancels_order_id: None,
                    params: serde_json::json!({"symbol":"BTCUSDT"}),
                }) }).await);
            },
            Work::Incident => {
                let occurrence = j.gate().record(IncidentReason::PersistenceFailure);
                ensure!(occurrence.opened && !occurrence.exhausted, "negative incident fixture did not open once");
                let pending = j.gate().open_incidents(); ensure!(pending.len() == 1, "incident fixture has extra work");
                expected_incident = Some(pending[0].clone());
            },
        }
        let generation = j.gate().generation(); let latch = j.latch_set();
        f.baseline = f.proxy.stats(); f.proxy.enable_query_trace(32); jp.enable_query_trace(32);
        let job = async {
            match work {
                Work::Intent => { operation.as_ref().unwrap().scope(j.before_send(intent.as_ref().unwrap(), WriteClass::Strict)).await
                    .map_err(|_| anyhow::anyhow!("legacy before_send refused"))?; Ok(1) },
                Work::Incident => Ok(j.flush_pending().await),
            }
        };
        eprintln!("LEGACY_COEXIST_SETUP readiness_actual_sql={ready_sql} actual_warm_connections={} identity_queries=1 relation_queries=1 wait_warm_queries=1 advisory_sql=2 baseline_catalog=1 baseline_rows=1 metadata_reads=1 fixture_macro_through37_and8explicit_base_setup_separate=true", warm.len());
        contend(&mut f, jp, work, job, &warm, &actor_backend, relation).await?;
        ensure!(j.gate().generation() == generation && j.latch_set() == latch, "coexistence introduced a journal failure/latch");
        Ok::<_, anyhow::Error>(())
    }).await.context("coexistence primary ceiling").and_then(|v| v);
    let expected_intent = intent.as_ref().map(|i| i.intent_id());
    drop(intent);
    drop(operation);
    drop(journal); // No background journal worker was started.
    let cleanup = cleanup::finish(
        f,
        journal_proxy,
        owned_pids,
        (baseline, old_rows),
        (tenant, client, expected_intent, expected_incident),
        work,
    )
    .await;
    eprintln!("LEGACY_COEXIST_FINAL primary={} cleanup={}", cause(&primary), cause(&cleanup));
    primary?;
    cleanup?;
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires DATABASE_URL"]
async fn actual52_cancel_preserves_legacy_before_send_500ms(pool: PgPool) -> Result<()> {
    run(pool, Work::Intent).await
}

#[sqlx::test(migrations = false)]
#[ignore = "requires DATABASE_URL"]
async fn actual52_cancel_preserves_legacy_incident_flush_500ms(pool: PgPool) -> Result<()> {
    run(pool, Work::Incident).await
}
