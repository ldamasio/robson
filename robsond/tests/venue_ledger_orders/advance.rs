//! Isolated PG16 advance of the restarted linked SELL from durable 0.25 to a
//! governed 0.30. The successor installs its own generation through the
//! native cold owner, retains the newer record with its original marks and
//! writes it once through the projected writer. Only an independent readback
//! plus a fresh exact governed receipt resolves the local blocker; the
//! native fence stays held throughout. No live loop selects any of it, and
//! nothing here transfers funds or recognizes a trade.
use std::{sync::Arc, time::Duration};

use anyhow::{anyhow, ensure, Context, Result};
use robson_connectors::inventory_loopback::LOOPBACK_UID;
use robson_exec::{gate::OperationGate, venue_ledger::LedgerError};
use robson_testkit::db_proxy::DbProxy;
use rust_decimal::Decimal;
use sqlx::PgPool;
use tokio::time::{timeout, timeout_at, Instant};
use uuid::Uuid;

use super::{
    exact_reference, key, order_body_at, read_only, restart_after_lost_ack, support, version,
    Restart, APP, ORDER, UPDATE_MS,
};
use crate::{
    spot_order_set_store::{
        cold_install::{install, ClosedWriterFence, FencePhase, FenceReceipt, FixtureFenceError},
        cold_owner::{capture, OwnerRequest, RestoreIdentity},
        physical_fixture::ColdAuditScope,
        JournalGeneration,
    },
    venue_clock_owner::RestartLookup,
    venue_ledger_store::{
        pending_execution::{receive_mark, OrderExecutionReceiver},
        restart_execution::{RestartExecutionBlocker, RestartState},
    },
};

/// The venue's later update for the 0.30 execution, after durable 0.25.
const ADVANCED_MS: i64 = UPDATE_MS + 2_000;

/// Test-only custody of the isolated fixture at every installer phase: the
/// exact restore operation, the successor's held native gate and no
/// surviving old-owner session. It is not a supervisor or role admission.
struct ClosedFixture<'a> {
    pool: &'a PgPool,
    wire: &'a DbProxy,
    sql: u64,
    gate: &'a Arc<OperationGate>,
    expected: RestoreIdentity,
    owner: Uuid,
    seen: Vec<FencePhase>,
}

impl ClosedWriterFence for ClosedFixture<'_> {
    async fn check(
        &mut self,
        restore: RestoreIdentity,
        phase: FencePhase,
        deadline: Instant,
    ) -> std::result::Result<FenceReceipt, FixtureFenceError> {
        self.seen.push(phase);
        let before = self.wire.stats().statements;
        let old = timeout_at(
            deadline,
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM pg_stat_activity
                 WHERE datname = current_database() AND application_name = $1",
            )
            .bind(APP)
            .fetch_one(self.pool),
        )
        .await
        .map_err(|_| FixtureFenceError)?
        .map_err(|_| FixtureFenceError)?;
        self.sql += self.wire.stats().statements - before;
        if restore != self.expected
            || !matches!(
                phase,
                FencePhase::BeforeRead | FencePhase::BeforePublish | FencePhase::AfterReadback
            )
            || !self.gate.fenced()
            || old != 0
            || Instant::now() >= deadline
        {
            return Err(FixtureFenceError);
        }
        Ok(FenceReceipt { restore, owner: self.owner })
    }
}

/// Measured facts, printed only after every assertion and retirement.
struct Advanced {
    observed: Decimal,
    final_durable: Decimal,
    consumed_delta: Decimal,
    version_delta: i64,
    write_sql: u64,
    duplicate_sql: u64,
    duplicate_commits: usize,
    stale_sql: u64,
    fill_rows: i64,
    held_before: bool,
    held_after_commit: bool,
    reconciled_exact: bool,
    late_old_held: bool,
    current_projection: i64,
    cold_capture_sql: u64,
    capture_wire: u64,
    install_sql: u64,
    fence_sql: u64,
    rollback_poll_sql: u64,
}

async fn fill_rows(pool: &PgPool) -> Result<i64> {
    Ok(sqlx::query_scalar("SELECT count(*) FROM public.order_fills")
        .fetch_one(pool)
        .await?)
}

/// A refused native write detaches its connection and the server rolls it
/// back asynchronously. Wait, bounded, until no client session of this
/// database is left inside a transaction.
async fn wait_rolled_back(pool: &PgPool) -> Result<()> {
    timeout(Duration::from_secs(3), async {
        loop {
            let open: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM pg_stat_activity
                 WHERE datname = current_database() AND backend_type = 'client backend'
                   AND state IN ('idle in transaction', 'idle in transaction (aborted)')",
            )
            .fetch_one(pool)
            .await?;
            if open == 0 {
                return Ok::<_, anyhow::Error>(());
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .context("refused native writer rolled back")?
}

async fn generations(pool: &PgPool, g: &JournalGeneration) -> Result<Vec<(Uuid, Uuid, Uuid)>> {
    Ok(sqlx::query_as(
        "SELECT generation_id, source_audit_id, process_epoch FROM public.spot_order_set_generation
         WHERE tenant_id=$1 AND venue_id='binance:spot' AND account_scope=$2",
    )
    .bind(g.tenant_id)
    .bind(&g.account_scope)
    .fetch_all(pool)
    .await?)
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn projected_native_order_advances_execution_after_restart(pool: PgPool) -> Result<()> {
    let Restart {
        f,
        clock,
        input,
        intent,
        binding,
        order_link,
        seeded,
        successor,
        journal,
        upstream,
        child_pid,
        durable,
    } = restart_after_lost_ack(&pool).await?;
    let gate = journal.gate();
    // The audited cold owner's scoped route index, as its catalogue requires.
    sqlx::query(
        "CREATE INDEX idx_leg_routes_spot_scope_position
          ON public.leg_routes(account_scope,position_id) WHERE venue_id='binance:spot'",
    )
    .execute(&pool)
    .await?;
    // Every later phase sends its SQL through its own tracing proxy, measured
    // separately and in total through pool retirement. The C2 clock lookup
    // keeps the accepted restart harness's direct pool and its own receipts.
    let (fixture_wire, fixture) = support::through_proxy(&pool, 1).await;
    let (install_wire, installer) = support::through_proxy(&pool, 2).await;
    let (writer_wire, writers) = support::through_proxy(&pool, 1).await;
    let old_projection = f.ledger(&writers).with_native_order_set_projection(binding.clone());
    let frozen = support::snapshot(&fixture).await;
    let accounting = f.version(&fixture).await;
    let consumed = f.consumed(&fixture).await;
    ensure!(accounting == seeded + 1 && fill_rows(&fixture).await? == 0);

    let deadline = Instant::now() + Duration::from_secs(148);
    let proxy = DbProxy::start(upstream).await?;
    proxy.enable_exact_query_trace(24);
    let readonly = read_only(&pool, &proxy).await?;
    let mut lookup = RestartLookup::new(clock, deadline);
    let outcomes = timeout_at(deadline, async {
        let mut blocker = RestartExecutionBlocker::reconstruct(
            &readonly,
            &binding,
            key(&f.scope)?,
            order_link.clone(),
            &successor,
        )
        .await?;
        ensure!(blocker.state() == RestartState::Blocked && gate.fenced());

        eprintln!(
            "C8_ADVANCE_QUARANTINE predecessor=no_marker old_killed=SIGKILL backend_retired=1"
        );
        lookup
            .arm(&pool, f.tenant, &successor, &input, [
                (400, r#"{"code":-2013,"msg":"synthetic unresolved order"}"#.into()),
                (200, order_body_at("0.30", "30", ADVANCED_MS)),
                (200, order_body_at("0.25", "25", UPDATE_MS + 1_000)),
                (200, order_body_at("0.30", "30", ADVANCED_MS)),
            ])
            .await?;
        // An unresolved lookup on the obsolete projection stays held.
        let checked = blocker.precheck().await?;
        let receipt = lookup.read(&proxy).await?;
        ensure!(checked.apply(receipt).await? == RestartState::Blocked, "unresolved order");
        ensure!(blocker.retained_execution().is_none() && gate.fenced());

        // The successor's own generation through the native cold owner, under
        // the test-only closed fixture fence. Accounting does not move.
        let restore = RestoreIdentity {
            operation: Uuid::now_v7(),
            target: Uuid::now_v7(),
        };
        let uid = LOOPBACK_UID.to_string();
        let request = OwnerRequest {
            scope: ColdAuditScope {
                tenant_id: f.tenant,
                account_scope: &f.scope,
            },
            expected_uid: &uid,
            epoch: successor.epoch.epoch_id,
            accounting_version: accounting,
            restore,
        };
        let install_deadline = deadline.min(Instant::now() + Duration::from_secs(5));
        let before = install_wire.stats().statements;
        let cut = capture(&installer, &request, install_deadline)
            .await
            .map_err(|e| anyhow!("successor cold cut: {e:?}"))?;
        let capture_wire = install_wire.stats().statements - before;
        // Six fixed statements plus one terminal page for each family below
        // its eight-row page: two journal intents, no attempt, one order and
        // one route.
        let cold_capture_sql = cut.receipt.statements;
        let families: Vec<(u64, u32)> =
            cut.receipt.families.iter().map(|family| (family.rows, family.pages)).collect();
        ensure!(
            cold_capture_sql == 10 && families == [(2, 1), (0, 1), (1, 1), (1, 1)],
            "cold source terminals {families:?}"
        );
        let mut fence = ClosedFixture {
            pool: &fixture,
            wire: &fixture_wire,
            sql: 0,
            gate,
            expected: restore,
            owner: Uuid::now_v7(),
            seen: Vec::new(),
        };
        let before = install_wire.stats().statements;
        let installed = install(&installer, &cut, &mut fence, install_deadline)
            .await
            .map_err(|e| anyhow!("successor install: {e:?}"))?;
        let install_sql = install_wire.stats().statements - before;
        ensure!(install_wire.stats().commits == 2, "one audit and one publication COMMIT");
        ensure!(
            fence.seen
                == [FencePhase::BeforeRead, FencePhase::BeforePublish, FencePhase::AfterReadback]
                && fence.sql == 3
        );
        let current = installed.generation;
        ensure!(
            installed.restore == restore
                && installed.accounting_version == accounting
                && current.tenant_id == binding.tenant_id
                && current.account_scope == binding.account_scope
                && current.generation_id != binding.generation_id
                && current.source_audit_id != binding.source_audit_id
                && current.process_epoch == successor.epoch.epoch_id
        );
        // Independently, the installed tuple is the only current one.
        ensure!(
            generations(&fixture, &current).await?
                == [(current.generation_id, current.source_audit_id, current.process_epoch)]
        );
        ensure!(version(&fixture, &current).await? == accounting);
        exact_reference(&fixture, &current).await?;
        ensure!(
            support::snapshot(&fixture).await == frozen
                && f.version(&fixture).await == accounting
        );
        // The predecessor's binding no longer reconstructs; the held blocker
        // follows only the exact installed generation.
        ensure!(matches!(
            RestartExecutionBlocker::reconstruct(
                &readonly,
                &binding,
                key(&f.scope)?,
                order_link.clone(),
                &successor,
            )
            .await,
            Err(LedgerError::IdentityConflict)
        ));
        blocker.adopt_installed(&current).await?;
        ensure!(blocker.state() == RestartState::Blocked && gate.fenced());

        // The newer governed record is retained with its original marks.
        let checked = blocker.precheck().await?;
        let receipt = lookup.read(&proxy).await?;
        ensure!(checked.apply(receipt).await? == RestartState::Blocked, "newer execution");
        let (record, evidence) = blocker.retained_advance()?;
        ensure!(
            blocker.retained_execution() == Some(Decimal::new(30, 2))
                && record.executed_qty == Decimal::new(30, 2)
                && record.cumulative_quote_qty == Some(Decimal::new(30, 0))
                && record.update_time_ms == Some(ADVANCED_MS)
                && record.exchange_order_id == ORDER.to_string()
        );

        // The predecessor's writer is stale: a changing write of that record
        // is refused and rolled back, leaving every native row unchanged.
        let before = old_projection.statements_total();
        ensure!(matches!(
            old_projection.record_order(&record, &order_link, receive_mark(evidence)?).await,
            Err(LedgerError::Invariant(reason)) if reason.contains("projection membership refused")
        ));
        let stale_sql = old_projection.statements_total() - before;
        let before = fixture_wire.stats().statements;
        wait_rolled_back(&fixture).await?;
        let rollback_poll_sql = fixture_wire.stats().statements - before;
        ensure!(
            support::snapshot(&fixture).await == frozen
                && f.version(&fixture).await == accounting
        );

        // A late exact 0.25 cannot lower the retained high water or clear.
        let checked = blocker.precheck().await?;
        let receipt = lookup.read(&proxy).await?;
        let late_old_held = checked.apply(receipt).await? == RestartState::Blocked
            && blocker.retained_advance()? == (record.clone(), evidence);
        ensure!(late_old_held && gate.fenced(), "late old record");

        // One projected write under the installed generation. The pending
        // observation stays held through the commit and its readback.
        let current_ledger = f.ledger(&writers).with_native_order_set_projection(current.clone());
        let link = order_link.clone();
        let receiver = OrderExecutionReceiver::bind(gate, &current_ledger, key(&f.scope)?, link)?;
        let work = receiver.register(record.clone(), evidence)?.context("advance ticket")?;
        let held_before = receiver.pending() == Some(evidence);
        ensure!(held_before, "registered before any persistence await");
        let before = current_ledger.statements_total();
        let ack = work.persist().await?.context("one changed projected commit")?;
        let write_sql = current_ledger.statements_total() - before;
        let version_delta = ack.accounting_version - accounting;
        ensure!(write_sql == 9 && version_delta == 1);
        let held_after_commit = receiver.pending() == Some(evidence);
        ensure!(held_after_commit, "a commit alone retires nothing");
        blocker.adopt_commit(ack.accounting_version).await?;
        ensure!(blocker.state() == RestartState::Blocked && gate.fenced());
        let consumed_delta = f.consumed(&fixture).await - consumed;
        ensure!(consumed_delta == Decimal::new(5, 2));
        ensure!(version(&fixture, &current).await? == accounting + 1);
        exact_reference(&fixture, &current).await?;
        let committed = support::snapshot(&fixture).await;
        let projected = super::super::projection_snapshot(&fixture, &current).await?;

        // The same original record through the same projected writer, at its
        // original receive mark: no commit, still pending. Four statements
        // (BEGIN, the server bounds, one read, ROLLBACK): main's bounded
        // pre-read (d8e002e1).
        let before = current_ledger.statements_total();
        let (changed, commit) = receiver.replay_original(&record, evidence).await?;
        let duplicate_sql = current_ledger.statements_total() - before;
        let duplicate_commits = usize::from(commit);
        ensure!(!changed && duplicate_commits == 0 && duplicate_sql == 4);
        ensure!(receiver.pending() == Some(evidence));
        ensure!(
            support::snapshot(&fixture).await == committed
                && super::super::projection_snapshot(&fixture, &current).await? == projected
                && f.version(&fixture).await == accounting + 1
        );

        // Only a fresh exact governed receipt of the committed facts resolves.
        let checked = blocker.precheck().await?;
        let receipt = lookup.read(&proxy).await?;
        let (fresh, marks) = receipt.execution().context("fresh exact record")?;
        ensure!(fresh == &record && marks.send.before_ns > evidence.receive.after_ns);
        let reconciled_exact = checked.apply(receipt).await? == RestartState::Resolved;
        ensure!(reconciled_exact, "fresh exact corroboration");
        ensure!(gate.fenced(), "local resolution leaves the native fence held");
        ensure!(ack.apply() && receiver.pending().is_none() && gate.fenced());

        let final_row: (Decimal, Option<Decimal>, Option<i64>, Uuid) = sqlx::query_as(
            "SELECT executed_cum, cumulative_quote, venue_update_time_ms, intent_id FROM venue_orders
             WHERE tenant_id=$1 AND venue_id='binance:spot' AND account_scope=$2
               AND symbol=$3 AND exchange_order_id=$4 AND owner='leg'",
        )
        .bind(f.tenant)
        .bind(&f.scope)
        .bind(support::SYMBOL)
        .bind(ORDER.to_string())
        .fetch_one(&fixture)
        .await?;
        ensure!(
            final_row
                == (
                    record.executed_qty,
                    record.cumulative_quote_qty,
                    record.update_time_ms,
                    intent
                )
        );
        let fill_rows = fill_rows(&fixture).await?;
        ensure!(fill_rows == 0, "no trade facts recorded");
        let current_projection = i64::try_from(generations(&fixture, &current).await?.len())?;
        ensure!(current_projection == 1 && support::snapshot(&fixture).await == committed);
        lookup.verify_weight()?;
        Ok::<_, anyhow::Error>(Advanced {
            observed: record.executed_qty,
            final_durable: final_row.0,
            consumed_delta,
            version_delta,
            write_sql,
            duplicate_sql,
            duplicate_commits,
            stale_sql,
            fill_rows,
            held_before,
            held_after_commit,
            reconciled_exact,
            late_old_held,
            current_projection,
            cold_capture_sql,
            capture_wire,
            install_sql,
            fence_sql: fence.sql,
            rollback_poll_sql,
        })
    })
    .await
    .context("148s advance rehearsal ceiling")
    .and_then(|inner| inner);
    let reads = lookup.reads();
    let retired = lookup.finish(&successor, outcomes.is_err()).await;
    let readonly_stopped = timeout(Duration::from_secs(2), readonly.close()).await;
    let measured_stopped = timeout(Duration::from_secs(2), async {
        fixture.close().await;
        installer.close().await;
        writers.close().await;
    })
    .await;
    retired?;
    readonly_stopped.context("reader pool retirement")?;
    measured_stopped.context("measured pool retirement")?;
    let a = outcomes?;
    let trace = proxy.query_trace();
    let recovery_selects = trace
        .iter()
        .filter(|query| query.trim_start().starts_with("SELECT o.client_order_id"))
        .count();
    ensure!(
        proxy.query_trace_dropped() == 0
            && trace.len() == 10
            && recovery_selects == 9
            && trace.iter().all(|query| query.trim_start().starts_with("SELECT ")
                || query == "SHOW transaction_read_only"),
        "bounded read-only trace: {trace:?}"
    );
    ensure!(reads == 4 && gate.fenced());
    let recovery_wire = proxy.stats().statements;
    ensure!(recovery_wire == u64::try_from(trace.len())?);
    let fixture_sql = fixture_wire.stats().statements;
    let readback_sql = fixture_sql - a.fence_sql - a.rollback_poll_sql;
    let writer_sql = writer_wire.stats().statements;
    ensure!(install_wire.stats().statements == a.capture_wire + a.install_sql);
    let total_sql = recovery_wire + a.capture_wire + a.install_sql + fixture_sql + writer_sql;
    let flag = |held: bool| u8::from(held);
    eprintln!(
        "C8_ADVANCE_RESTART old_durable={} observed={} final_durable={} consumed_delta={} \
         version_delta={} write_sql={} duplicate_sql={} duplicate_commits={} fill_rows={} \
         held_before={} held_after_commit={} reconciled_exact={} native_fence=held old_retired=1 \
         current_projection={} late_old_held={} quarantine_seconds=120 get_handoffs={reads} \
         total_weight={} get_sql=0 stale_writer_refused=1 stale_sql={} \
         recovery_selects={recovery_selects} recovery_wire={recovery_wire} \
         capture_sql={} capture_wire={} install_sql={} install_commits=2 fence_sql={} \
         readback_sql={readback_sql} rollback_poll_sql={} writer_sql={writer_sql} \
         total_sql={total_sql} old_pid={child_pid} successor_pid={}",
        durable.0,
        a.observed,
        a.final_durable,
        a.consumed_delta,
        a.version_delta,
        a.write_sql,
        a.duplicate_sql,
        a.duplicate_commits,
        a.fill_rows,
        flag(a.held_before),
        flag(a.held_after_commit),
        flag(a.reconciled_exact),
        a.current_projection,
        flag(a.late_old_held),
        1 + 4 * reads,
        a.stale_sql,
        a.cold_capture_sql,
        a.capture_wire,
        a.install_sql,
        a.fence_sql,
        a.rollback_poll_sql,
        std::process::id(),
    );
    Ok(())
}
