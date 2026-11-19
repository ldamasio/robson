//! Native-protocol gates on the private clocked journal transaction (J2b):
//! the one M38 helper held after its DataRow and after its CommandComplete,
//! and COMMIT's acknowledgement held. No cleanup SQL, no replay. TEST-ONLY.
use std::time::Duration;

use anyhow::{anyhow, ensure, Result};
use robson_exec::order_journal::{CommitFailure, JournalBatch};
use robson_testkit::db_proxy::{
    ClientLoss, ControlBoundary, ControlHit, DbProxy, DiagnosticEvent, DiagnosticIdentity,
    DiagnosticPhase, SqlCategory,
};
use sqlx::{
    postgres::{PgPoolOptions, PgSslMode},
    PgPool,
};
use tokio::time::{timeout, Instant};
use uuid::Uuid;

use super::{one, scope, PgJournalBackend, Scope};

#[path = "order_journal_clocked_work_pg_tests.rs"]
mod work;

const HELPER: &str = "SELECT * FROM public.budget_insert_clocks(";

/// One proxied, one-slot writer pool whose connection was opened after
/// diagnostics were enabled, so its actual backend PID is attributed.
struct Wire {
    proxy: DbProxy,
    store: PgJournalBackend,
    connection: u64,
    pid: i32,
    birth: String,
}

/// Actual statement positions of the clocked route, learned from one
/// committed calibration batch on the same connection.
struct Route {
    helper: u64,
    commit: u64,
    data_rows: u64,
}

impl Wire {
    async fn new(pool: &PgPool, tenant: Uuid) -> Result<Self> {
        let options = pool.connect_options();
        let upstream = tokio::net::lookup_host((options.get_host(), options.get_port()))
            .await?
            .next()
            .ok_or_else(|| anyhow!("no fixture PostgreSQL address"))?;
        ensure!(upstream.ip().is_loopback(), "fixture must be loopback");
        let proxy = DbProxy::start(upstream).await?;
        proxy.enable_diagnostics();
        let writer = PgPoolOptions::new()
            .max_connections(1)
            .acquire_timeout(super::super::JOURNAL_ACQUIRE_BOUND)
            .test_before_acquire(false)
            .connect_with(
                options
                    .as_ref()
                    .clone()
                    .host("127.0.0.1")
                    .port(proxy.addr().port())
                    .ssl_mode(PgSslMode::Disable)
                    .application_name("j2b-clocked-wire"),
            )
            .await?;
        let mut connection = writer.acquire().await?;
        let (pid, birth): (i32, String) = sqlx::query_as(
            "SELECT pg_backend_pid(),backend_start::text
            FROM pg_stat_activity WHERE pid=pg_backend_pid()",
        )
        .fetch_one(&mut *connection)
        .await?;
        drop(connection);
        timeout(Duration::from_secs(1), async {
            while writer.num_idle() != 1 {
                tokio::task::yield_now().await;
            }
        })
        .await?;
        let identities = proxy.diagnostic_identities().ok_or_else(|| anyhow!("diagnostics"))?;
        ensure!(identities.len() == 1, "one attributed writer connection: {identities:?}");
        ensure!(
            identities[0].pid == Some(pid) && identities[0].live,
            "writer identity carries its actual PID"
        );
        let store = PgJournalBackend {
            pool: writer,
            daemon_pool: pool.clone(),
            tenant_id: tenant,
            ready_wait: super::super::JOURNAL_READY_WAIT,
        };
        Ok(Self {
            proxy,
            store,
            connection: identities[0].connection,
            pid,
            birth,
        })
    }

    /// The live registry identity of the original writer connection.
    fn identity(&self) -> Result<DiagnosticIdentity> {
        let identities =
            self.proxy.diagnostic_identities().ok_or_else(|| anyhow!("diagnostics"))?;
        identities
            .into_iter()
            .find(|i| i.connection == self.connection && i.pid == Some(self.pid) && i.live)
            .ok_or_else(|| anyhow!("original writer identity is not live"))
    }

    /// The highest Simple/Execute ordinal issued on the writer connection.
    fn last_ordinal(&self) -> Result<u64> {
        let snapshot = self.proxy.diagnostics().ok_or_else(|| anyhow!("diagnostics"))?;
        ensure!(
            snapshot.dropped == 0
                && snapshot.connection_overflow == 0
                && snapshot.phase_overflow == 0,
            "diagnostic loss"
        );
        Ok(snapshot
            .records
            .iter()
            .filter(|r| r.identity.connection == self.connection)
            .filter_map(|r| r.ordinal)
            .max()
            .unwrap_or(0))
    }

    /// Commit one healthy batch and locate the unique helper and COMMIT.
    async fn calibrate(&self, scope: &Scope) -> Result<Route> {
        let base = self.last_ordinal()?;
        let rows_before = self.proxy.data_rows();
        self.proxy.enable_exact_query_trace(32);
        let (batch, input) = one(scope, "j2b-wire-calibrate")?;
        let receipts = self.store.commit_clocked(&batch, &input).await;
        ensure!(receipts.as_ref().is_ok_and(|r| r.len() == 1), "calibration: {receipts:?}");
        let trace = self.proxy.query_trace();
        ensure!(self.proxy.query_trace_dropped() == 0, "calibration trace complete");
        ensure!(
            self.last_ordinal()? == base + trace.len() as u64,
            "every traced statement is one writer ordinal: {trace:?}"
        );
        let unique = |matches: &dyn Fn(&str) -> bool| -> Result<u64> {
            let found: Vec<_> = trace
                .iter()
                .enumerate()
                .filter(|(_, sql)| matches(sql.as_str()))
                .map(|(n, _)| n as u64 + 1)
                .collect();
            ensure!(found.len() == 1, "not unique in {trace:?}");
            Ok(found[0])
        };
        let helper = unique(&|sql: &str| sql.trim_start().starts_with(HELPER))?;
        let commit = unique(&|sql: &str| is_commit(sql))?;
        ensure!(
            helper < commit && commit == trace.len() as u64,
            "helper precedes the final COMMIT: {trace:?}"
        );
        ensure!(self.identity().is_ok(), "calibration kept the original connection");
        Ok(Route {
            helper,
            commit,
            data_rows: self.proxy.data_rows() - rows_before,
        })
    }

    async fn retires(&self, observer: &PgPool) -> Result<()> {
        timeout(Duration::from_secs(2), async {
            loop {
                let present: bool = sqlx::query_scalar(
                    "SELECT EXISTS(SELECT 1 FROM pg_stat_activity
                    WHERE pid=$1 AND backend_start::text=$2 AND datname=current_database())",
                )
                .bind(self.pid)
                .bind(&self.birth)
                .fetch_one(observer)
                .await?;
                if !present {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            Ok::<_, sqlx::Error>(())
        })
        .await??;
        Ok(())
    }
}

fn is_commit(sql: &str) -> bool {
    sql.trim().trim_end_matches(';').trim().eq_ignore_ascii_case("COMMIT")
}

#[derive(Clone, Copy, Debug)]
enum Gate {
    /// Helper DataRow forwarded, its CommandComplete held.
    HelperRow,
    /// Helper CommandComplete forwarded, ReadyForQuery held.
    HelperComplete,
    /// COMMIT applied by the server, its CommandComplete held.
    CommitAck,
}

/// Intent, attempt, clock and charge rows of one batch, read by the healthy
/// independent owner.
async fn durable(pool: &PgPool, batch: &JournalBatch) -> Result<(i64, i64, i64, i64)> {
    Ok(sqlx::query_as(
        "SELECT (SELECT count(*) FROM order_intents WHERE intent_id=$1)::bigint,
            (SELECT count(*) FROM submit_attempts WHERE attempt_id=$2)::bigint,
            (SELECT count(*) FROM spot_intent_clock WHERE attempt_id=$2)::bigint,
            (SELECT count(*) FROM budget_daily_charges WHERE attempt_id=$2)::bigint",
    )
    .bind(batch.intent.intent_id)
    .bind(batch.attempts[0].attempt_id)
    .fetch_one(pool)
    .await?)
}

async fn version(pool: &PgPool, tenant: Uuid) -> Result<i64> {
    Ok(sqlx::query_scalar(
        "SELECT version FROM accounting_state WHERE tenant_id=$1 AND venue_id='binance:spot'",
    )
    .bind(tenant)
    .fetch_one(pool)
    .await?)
}

/// An independent write-lock probe on the original owner's two row locks.
async fn locks_free(pool: &PgPool, tenant: Uuid) -> Result<(bool, bool)> {
    let accounting = sqlx::query(
        "SELECT 1 FROM accounting_state WHERE tenant_id=$1 AND venue_id='binance:spot'
        FOR UPDATE NOWAIT",
    )
    .bind(tenant)
    .fetch_one(pool)
    .await;
    let venue = sqlx::query(
        "SELECT 1 FROM budget_venue_state WHERE venue_id='binance:spot' FOR UPDATE NOWAIT",
    )
    .fetch_one(pool)
    .await;
    Ok((lock_free(accounting)?, lock_free(venue)?))
}

fn lock_free(probe: Result<sqlx::postgres::PgRow, sqlx::Error>) -> Result<bool> {
    match probe {
        Ok(_) => Ok(true),
        Err(sqlx::Error::Database(e)) if e.code().as_deref() == Some("55P03") => Ok(false),
        Err(e) => Err(e.into()),
    }
}

struct Observed {
    hit: ControlHit,
    rows_at_hit: u64,
    rows_before: u64,
    locks_free_while_held: (bool, bool),
    durable_while_held: (i64, i64, i64, i64),
    result: Result<Vec<super::ClockReceipt>, CommitFailure>,
    elapsed: Duration,
    batch: JournalBatch,
    trace: Vec<String>,
}

/// One full clocked route through the gate. Observe the writer's own socket
/// close, retired backend, freed locks and replacement before any gate drop.
async fn gated(
    pool: &PgPool,
    scope: &Scope,
    wire: &Wire,
    route: &Route,
    gate: Gate,
) -> Result<Observed> {
    let start = wire.last_ordinal()?;
    let (ordinal, boundary) = match gate {
        Gate::HelperRow => (start + route.helper, ControlBoundary::BeforeCommandComplete),
        Gate::HelperComplete => (start + route.helper, ControlBoundary::BeforeReady),
        Gate::CommitAck => (start + route.commit, ControlBoundary::BeforeCommandComplete),
    };
    wire.proxy.enable_exact_query_trace(32);
    let rows_before = wire.proxy.data_rows();
    let mut lease = wire
        .proxy
        .control(wire.identity()?, false)
        .map_err(|e| anyhow!("control: {e:?}"))?;
    let (batch, input) = one(scope, &format!("j2b-wire-{gate:?}"))?;
    let mut armed = lease.arm(ordinal, boundary).map_err(|e| anyhow!("arm: {e:?}"))?;
    let started = Instant::now();
    let observe = async {
        let hit = timeout(Duration::from_millis(450), armed.held())
            .await
            .map_err(|_| anyhow!("{gate:?} target never reached"))?
            .map_err(|e| anyhow!("held: {e:?}"))?;
        let rows_at_hit = wire.proxy.data_rows();
        let locks_free_while_held = locks_free(pool, scope.tenant).await?;
        let durable_while_held = durable(pool, &batch).await?;
        Ok::<_, anyhow::Error>((hit, rows_at_hit, locks_free_while_held, durable_while_held))
    };
    let (result, observed) = tokio::join!(wire.store.commit_clocked(&batch, &input), observe);
    let elapsed = started.elapsed();
    let (hit, rows_at_hit, locks_free_while_held, durable_while_held) = observed?;
    wire.retires(pool).await?;
    let retired = timeout(Duration::from_secs(1), async {
        loop {
            let snapshot = armed.snapshot();
            if snapshot.closed {
                break snapshot;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await?;
    ensure!(
        retired.client_loss == Some(ClientLoss::Read) && retired.proof_valid && !retired.released,
        "{gate:?} the client's socket closed with a valid unreleased gate: {retired:?}"
    );
    ensure!(wire.proxy.query_trace_dropped() == 0, "fault trace complete before replacement");
    let trace = wire.proxy.query_trace();
    ensure!(
        locks_free(pool, scope.tenant).await? == (true, true),
        "{gate:?} both locks released before gate teardown"
    );
    slot_available(wire).await?;
    drop(armed);
    let closed = timeout(Duration::from_secs(1), lease.close())
        .await?
        .map_err(|e| anyhow!("close: {e:?}"))?;
    ensure!(closed == retired, "{gate:?} teardown did not cause retirement");
    Ok(Observed {
        hit,
        rows_at_hit,
        rows_before,
        locks_free_while_held,
        durable_while_held,
        result,
        elapsed,
        batch,
        trace,
    })
}

/// The writer target is the original connection's helper or COMMIT, never a
/// startup, BEGIN or timeout statement.
fn targeted(wire: &Wire, route: &Route, observed: &Observed, gate: Gate) -> Result<()> {
    let hit = &observed.hit;
    let (statement, expected) = match gate {
        Gate::HelperRow | Gate::HelperComplete => {
            (hit.complete.unwrap_or(hit.record), route.helper)
        },
        Gate::CommitAck => (hit.record, route.commit),
    };
    ensure!(
        statement.identity.connection == wire.connection
            && statement.identity.pid == Some(wire.pid),
        "{gate:?} gate on the original writer identity"
    );
    ensure!(
        statement.event == DiagnosticEvent::Complete,
        "{gate:?} completion: {statement:?}"
    );
    ensure!(
        observed.trace.len() as u64 == expected,
        "{gate:?} no statement after the gate: {:?}",
        observed.trace
    );
    let last = observed.trace.last().ok_or_else(|| anyhow!("empty trace"))?;
    match gate {
        Gate::HelperRow | Gate::HelperComplete => {
            ensure!(
                statement.category == SqlCategory::Other
                    && statement.phase == DiagnosticPhase::Execute,
                "{gate:?} helper is one extended Execute: {statement:?}"
            );
            ensure!(last.trim_start().starts_with(HELPER), "{gate:?} helper SQL: {last:?}");
            ensure!(
                observed.trace.iter().filter(|s| s.trim_start().starts_with(HELPER)).count() == 1,
                "{gate:?} unique helper"
            );
        },
        Gate::CommitAck => {
            ensure!(
                statement.category == SqlCategory::Commit
                    && statement.phase == DiagnosticPhase::Simple,
                "COMMIT is one simple query: {statement:?}"
            );
            ensure!(is_commit(last), "COMMIT SQL: {last:?}");
        },
    }
    match gate {
        Gate::HelperRow => ensure!(
            hit.boundary == ControlBoundary::BeforeCommandComplete
                && hit.complete.is_none()
                && observed.rows_at_hit - observed.rows_before == route.data_rows,
            "DataRow forwarded, CommandComplete held: {hit:?}"
        ),
        Gate::HelperComplete => ensure!(
            hit.boundary == ControlBoundary::BeforeReady
                && hit.record.event == DiagnosticEvent::Ready
                && hit.record.transaction == Some(b'T')
                && observed.rows_at_hit - observed.rows_before == route.data_rows,
            "CommandComplete forwarded, in-transaction Ready held: {hit:?}"
        ),
        Gate::CommitAck => ensure!(
            hit.boundary == ControlBoundary::BeforeCommandComplete && hit.complete.is_none(),
            "COMMIT CommandComplete held: {hit:?}"
        ),
    }
    Ok(())
}

/// The one-slot writer pool hands out a fresh backend at once.
async fn slot_available(wire: &Wire) -> Result<()> {
    let replacement: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&wire.store.pool)
        .await?;
    ensure!(replacement != wire.pid, "a new backend, not the retired one");
    ensure!(wire.store.pool.size() == 1, "at most one writer slot");
    Ok(())
}

async fn pre_commit(pool: PgPool, gate: Gate) -> Result<()> {
    let scope = scope(&pool).await?;
    let wire = Wire::new(&pool, scope.tenant).await?;
    let route = wire.calibrate(&scope).await?;
    let before = version(&pool, scope.tenant).await?;
    let observed = gated(&pool, &scope, &wire, &route, gate).await?;
    targeted(&wire, &route, &observed, gate)?;
    ensure!(
        matches!(&observed.result, Err(CommitFailure::Known(why))
            if why == "clocked journal deadline before COMMIT"),
        "{gate:?} is a known pre-COMMIT failure: {:?}",
        observed.result
    );
    ensure!(
        observed.elapsed >= Duration::from_millis(500)
            && observed.elapsed < Duration::from_millis(900),
        "{gate:?} bounded by the clocked deadline: {:?}",
        observed.elapsed
    );
    ensure!(
        observed.locks_free_while_held == (false, false),
        "{gate:?} the open transaction held both accounting and global venue rows"
    );
    ensure!(
        observed.durable_while_held == (0, 0, 0, 0),
        "{gate:?} nothing visible while held"
    );
    ensure!(
        durable(&pool, &observed.batch).await? == (0, 0, 0, 0),
        "{gate:?} no durable intent, attempt, clock or charge"
    );
    ensure!(
        version(&pool, scope.tenant).await? == before,
        "{gate:?} accounting version unchanged"
    );
    Ok(())
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn helper_held_after_data_row_is_known_before_commit(pool: PgPool) -> Result<()> {
    pre_commit(pool, Gate::HelperRow).await
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn helper_held_after_command_complete_is_known_before_commit(pool: PgPool) -> Result<()> {
    pre_commit(pool, Gate::HelperComplete).await
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn lost_commit_acknowledgement_is_uncertain_and_durable(pool: PgPool) -> Result<()> {
    let scope = scope(&pool).await?;
    let wire = Wire::new(&pool, scope.tenant).await?;
    let route = wire.calibrate(&scope).await?;
    let before = version(&pool, scope.tenant).await?;
    let observed = gated(&pool, &scope, &wire, &route, Gate::CommitAck).await?;
    targeted(&wire, &route, &observed, Gate::CommitAck)?;
    ensure!(
        matches!(&observed.result, Err(CommitFailure::Uncertain(why))
            if why == "clocked journal COMMIT unanswered at its deadline"),
        "a held COMMIT acknowledgement is uncertain, never a receipt: {:?}",
        observed.result
    );
    ensure!(
        observed.elapsed >= Duration::from_millis(500)
            && observed.elapsed < Duration::from_millis(900),
        "bounded by the clocked deadline: {:?}",
        observed.elapsed
    );
    // The server applied COMMIT before its CommandComplete was held.
    ensure!(
        observed.locks_free_while_held == (true, true),
        "COMMIT released both rows before its ack"
    );
    ensure!(observed.durable_while_held == (1, 1, 1, 1), "durable while the ack was held");
    ensure!(
        durable(&pool, &observed.batch).await? == (1, 1, 1, 1),
        "the independent owner observes one intent, attempt, clock and charge"
    );
    ensure!(version(&pool, scope.tenant).await? == before + 1, "one accounting version");
    Ok(())
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn clocked_transaction_declares_read_committed_despite_session_default(
    pool: PgPool,
) -> Result<()> {
    let scope = scope(&pool).await?;
    let wire = Wire::new(&pool, scope.tenant).await?;
    let writer = &wire.store.pool;
    sqlx::query("SET default_transaction_isolation TO 'repeatable read'")
        .execute(writer)
        .await?;
    let session_default: String = sqlx::query_scalar("SHOW default_transaction_isolation")
        .fetch_one(writer)
        .await?;
    ensure!(session_default == "repeatable read");
    wire.proxy.enable_exact_query_trace(12);
    let (batch, input) = one(&scope, "j2b-isolation")?;
    let result = wire.store.commit_clocked(&batch, &input).await;
    ensure!(
        result.as_ref().is_ok_and(|r| r.len() == 1 && r[0].daily_units == 1),
        "the real M38 projection accepted the clocked transaction: {result:?}"
    );
    let trace = wire.proxy.query_trace();
    ensure!(
        wire.proxy.query_trace_dropped() == 0
            && trace.len() == 7
            && trace[0] == "BEGIN ISOLATION LEVEL READ COMMITTED"
            && trace[5].trim_start().starts_with(HELPER)
            && is_commit(&trace[6]),
        "explicit transaction isolation replaced BEGIN in the same seven statements: {trace:?}"
    );
    ensure!(wire.identity().is_ok(), "the original session was retained");
    Ok(())
}
