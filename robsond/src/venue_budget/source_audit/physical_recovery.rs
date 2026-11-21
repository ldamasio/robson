//! Native empty-budget evidence for the disposable physical restore test.
use std::{
    net::{IpAddr, SocketAddr},
    time::Duration,
};

use anyhow::{anyhow, ensure, Result};
use chrono::{DateTime, Utc};
use robson_exec::venue_clock::{NativeClock, TickCapture};
use robson_testkit::db_proxy::DbProxy;
use sqlx::{
    postgres::{PgPoolOptions, PgSslMode},
    PgPool,
};
use tokio::time::{timeout_at, Instant};
use uuid::Uuid;

use super::run::{finish_audit, pass, start, Envelope, Pass, Terminal, Witness};
use crate::{
    process_epoch::EpochStart,
    venue_budget::{
        bootstrap::{self, BootstrapInput},
        BudgetError, Trace,
    },
};

const VENUE: &str = "binance:spot";
const NATIVE_TRANSACTION: Duration = Duration::from_secs(2);

#[derive(Debug)]
pub(crate) struct BudgetAuditEvidence {
    pub(crate) copied_audit: Uuid,
    pub(crate) copied_generation: i64,
    pub(crate) audit: Uuid,
    pub(crate) generation: i64,
    pub(crate) started_at: DateTime<Utc>,
    pub(crate) completed_at: DateTime<Utc>,
    pub(crate) start_commands: usize,
    pub(crate) pass_commands: usize,
    pub(crate) terminal_commands: usize,
    pub(crate) terminal_keys: (i64, i64, i64, i64),
}

#[derive(Debug)]
pub(crate) struct BudgetBootstrapEvidence {
    pub(crate) audit: Uuid,
    pub(crate) generation: i64,
    pub(crate) process_epoch: Uuid,
    pub(crate) predecessor_epoch: Option<Uuid>,
    pub(crate) predecessor_clean: Option<bool>,
    pub(crate) start: TickCapture,
    pub(crate) committed_by: TickCapture,
    pub(crate) recorded_at: DateTime<Utc>,
    pub(crate) stale_commands: usize,
    pub(crate) bootstrap_commands: usize,
}

pub(crate) fn copied_budget_binding(input: &BootstrapInput) -> (Uuid, i64) {
    (input.audit, input.generation)
}

/// The caller owns the restored database identity and both writer-role fences.
pub(crate) async fn audit_empty_budget(
    pool: &PgPool,
    copied: &BootstrapInput,
    deadline: Instant,
) -> Result<BudgetAuditEvidence> {
    timeout_at(deadline, async {
        let (proxy, native) = wire(pool).await?;
        let (copied_audit, copied_generation) = copied_budget_binding(copied);
        let copied_started =
            current_audit_started(pool, copied_audit, copied_generation, None).await?;
        let audit = Uuid::now_v7();
        ensure!(audit != copied_audit);
        // The public fixture deadline is stricter than the private 60s envelope cap.
        let envelope = Envelope::new(deadline.min(Instant::now() + Duration::from_secs(5)))
            .map_err(|err| anyhow!("budget audit envelope: {err:?}"))?;
        let trace = Trace::default();
        let run = start(&native, audit, envelope, &trace)
            .await
            .map_err(|err| anyhow!("budget start: {err:?}"))?;
        ensure!(
            run.audit == audit
                && run.generation
                    == copied_generation
                        .checked_add(1)
                        .ok_or_else(|| anyhow!("budget generation overflow"))?
                && run.high_water.is_none()
                && run.cursor.is_none()
                && run.examined == 0,
            "fresh budget audit did not bind the empty attempt cut"
        );
        let start_commands = command_count(&proxy, &trace, 5, true)?;
        let empty = pass(&native, run, &trace)
            .await
            .map_err(|err| anyhow!("budget pass: {err:?}"))?;
        ensure!(empty == Pass::Exhausted);
        let pass_commands = command_count(&proxy, &trace, 5, true)? - start_commands;

        let terminal = finish_audit(&native, run, &trace)
            .await
            .map_err(|err| anyhow!("budget terminal: {err:?}"))?;
        let Terminal::Complete(receipt) = terminal else {
            anyhow::bail!("fresh empty budget terminal: {terminal:?}");
        };
        ensure!(
            receipt.audit == audit
                && receipt.generation == run.generation
                && receipt.high_water.is_none()
                && receipt.examined == 0
                && receipt.witness
                    == (Witness {
                        source: 0,
                        spot: 0,
                        clocks: 0,
                        charges: 0
                    }),
            "budget terminal receipt did not seal the exact empty cut"
        );
        let terminal_commands = command_count(&proxy, &trace, 10, true)? - start_commands;
        ensure!(proxy.query_trace()[start_commands].contains("BEGIN ISOLATION LEVEL"));
        let started_at =
            current_audit_started(pool, audit, run.generation, Some(receipt.completed_at)).await?;
        ensure!(started_at <= receipt.completed_at);
        let old_still_complete: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM budget_source_audits
                            WHERE audit_id=$1 AND quiescence_generation=$2
                              AND started_at=$3 AND status='complete')",
        )
        .bind(copied_audit)
        .bind(copied_generation)
        .bind(copied_started)
        .fetch_one(pool)
        .await?;
        ensure!(
            old_still_complete,
            "superseding the copied binding changed its historical audit"
        );

        Ok(BudgetAuditEvidence {
            copied_audit,
            copied_generation,
            audit,
            generation: run.generation,
            started_at,
            completed_at: receipt.completed_at,
            start_commands,
            pass_commands,
            terminal_commands,
            terminal_keys: (
                receipt.witness.source,
                receipt.witness.spot,
                receipt.witness.clocks,
                receipt.witness.charges,
            ),
        })
    })
    .await?
}

/// Use the restored admin pool after the native audit phase has been accepted.
/// The predecessor and candidate roles stay denied by the external controller.
pub(crate) async fn bootstrap_empty_budget(
    pool: &PgPool,
    tenant: Uuid,
    epoch: &EpochStart,
    copied: &BootstrapInput,
    fresh: &BudgetAuditEvidence,
    clock: &NativeClock,
    deadline: Instant,
) -> Result<BudgetBootstrapEvidence> {
    timeout_at(deadline, async {
        let (stale_proxy, stale_pool) = wire(pool).await?;
        ensure!(copied_budget_binding(copied) == (fresh.copied_audit, fresh.copied_generation));
        ensure!(fresh.audit != fresh.copied_audit);
        ensure!(
            current_audit_started(pool, fresh.audit, fresh.generation, Some(fresh.completed_at))
                .await?
                == fresh.started_at
        );
        let process_epoch = epoch.epoch.epoch_id;
        let predecessor_epoch = epoch.predecessor_epoch_id;
        ensure!(predecessor_epoch.is_some_and(|old| old != process_epoch));
        let predecessor_clean =
            predecessor_epoch.map(|_| epoch.predecessor.unclean_reason().is_none());

        ensure!(
            predecessor_clean == Some(false),
            "physical restore must see the unclean original epoch"
        );

        let count_sql = "SELECT count(*) FROM budget_epochs";
        ensure!(sqlx::query_scalar::<_, i64>(count_sql).fetch_one(pool).await? == 0);

        let stale_trace = Trace::default();
        let stale = bootstrap::observe_bootstrap(
            &stale_pool,
            tenant,
            epoch,
            copied,
            native_deadline(deadline),
            &stale_trace,
        )
        .await;
        ensure!(
            matches!(stale, Err(BudgetError::Invalid)),
            "copied budget binding did not refuse"
        );
        let stale_commands = command_count(&stale_proxy, &stale_trace, 3, false)?;
        ensure!(stale_proxy.query_trace()[2].contains("ON CONFLICT(venue_id)"));
        drop(stale_pool);
        drop(stale_proxy);
        ensure!(sqlx::query_scalar::<_, i64>(count_sql).fetch_one(pool).await? == 0);

        let (proxy, native) = wire(pool).await?;
        let fresh_trace = Trace::default();
        let start = clock.capture()?;
        let input = BootstrapInput {
            audit: fresh.audit,
            generation: fresh.generation,
            start,
        };
        let rows = bootstrap::observe_bootstrap(
            &native,
            tenant,
            epoch,
            &input,
            native_deadline(deadline),
            &fresh_trace,
        )
        .await
        .map_err(|err| anyhow!("fresh budget bootstrap: {err:?}"))?;
        let committed_by = clock.capture()?;
        committed_by.elapsed_since(&start)?;
        ensure!(rows.start == start && rows.active.is_empty() && rows.pause.is_none());
        let bootstrap_commands = command_count(&proxy, &fresh_trace, 6, true)?;

        let sql = "SELECT process_epoch, venue_id, (clock_domain).kind,
                          (clock_domain).boot_uuid, (clock_domain).offset_seconds,
                          (clock_domain).offset_nanoseconds, start_tick::text,
                          predecessor_epoch, predecessor_clean, recorded_at
                   FROM budget_epochs ORDER BY process_epoch LIMIT 2";
        let epochs = sqlx::query_as::<
            _,
            (
                Uuid,
                String,
                String,
                Uuid,
                i64,
                i32,
                String,
                Option<Uuid>,
                Option<bool>,
                DateTime<Utc>,
            ),
        >(sql)
        .fetch_all(pool)
        .await?;
        let [(id, venue, kind, boot, seconds, nanos, tick, predecessor, clean, recorded_at)] =
            epochs.as_slice()
        else {
            anyhow::bail!("physical budget bootstrap did not commit exactly one durable epoch");
        };
        let domain = start.domain();
        let (offset_seconds, offset_ns) = domain.offset();
        ensure!(
            (*id, venue.as_str(), kind.as_str(), *boot, *seconds, *nanos)
                == (
                    process_epoch,
                    VENUE,
                    "boottime",
                    domain.boot(),
                    offset_seconds,
                    i32::try_from(offset_ns)?
                )
                && tick.parse::<u128>()? == start.nanoseconds()
                && (*predecessor, *clean) == (predecessor_epoch, predecessor_clean),
            "budget epoch readback differs from the exact committed epoch and BOOTTIME input"
        );

        ensure!(
            current_audit_started(pool, fresh.audit, fresh.generation, Some(fresh.completed_at))
                .await?
                == fresh.started_at
        );
        ensure!(matches!(bootstrap::production_input(), Err(BudgetError::PrerequisiteRefusal)));
        Ok(BudgetBootstrapEvidence {
            audit: fresh.audit,
            generation: fresh.generation,
            process_epoch,
            predecessor_epoch,
            predecessor_clean,
            start,
            committed_by,
            recorded_at: *recorded_at,
            stale_commands,
            bootstrap_commands,
        })
    })
    .await?
}

async fn current_audit_started(
    pool: &PgPool,
    audit: Uuid,
    generation: i64,
    completed_at: Option<DateTime<Utc>>,
) -> Result<DateTime<Utc>> {
    let (started_at, durable_completion): (DateTime<Utc>, DateTime<Utc>) = sqlx::query_as(
        "SELECT a.started_at, a.completed_at
         FROM budget_source_audits a
         JOIN budget_venue_state v
           ON v.venue_id=a.venue_id AND v.audit_id=a.audit_id
          AND v.quiescence_generation=a.quiescence_generation
         WHERE a.venue_id=$1 AND a.audit_id=$2 AND a.quiescence_generation=$3
           AND v.initialization_state='complete' AND v.source_high_water IS NULL
           AND a.status='complete' AND a.high_water IS NULL AND a.cursor IS NULL
           AND a.examined_count=0 AND a.failure_reason IS NULL
           AND NOT EXISTS (SELECT 1 FROM submit_attempts)
           AND NOT EXISTS (SELECT 1 FROM spot_intent_clock)
           AND NOT EXISTS (SELECT 1 FROM budget_daily_charges)",
    )
    .bind(VENUE)
    .bind(audit)
    .bind(generation)
    .fetch_one(pool)
    .await?;
    ensure!(completed_at.is_none_or(|expected| expected == durable_completion));
    ensure!(started_at <= durable_completion);
    Ok(started_at)
}

fn native_deadline(deadline: Instant) -> Instant {
    deadline.min(Instant::now() + NATIVE_TRANSACTION)
}

async fn wire(pool: &PgPool) -> Result<(DbProxy, PgPool)> {
    let options = pool.connect_options();
    let address: IpAddr = options.get_host().parse()?;
    ensure!(
        address.is_loopback(),
        "native budget fixture requires a literal loopback upstream"
    );
    let proxy = DbProxy::start(SocketAddr::new(address, options.get_port())).await?;
    proxy.enable_exact_query_trace(12);
    let native = PgPoolOptions::new().max_connections(1).connect_lazy_with(
        (*options)
            .clone()
            .host("127.0.0.1")
            .port(proxy.addr().port())
            .ssl_mode(PgSslMode::Disable)
            .application_name("physical-budget-recovery"),
    );
    Ok((proxy, native))
}

fn command_count(
    proxy: &DbProxy,
    trace: &Trace,
    expected: usize,
    committed: bool,
) -> Result<usize> {
    let statements = proxy.query_trace();
    ensure!(
        proxy.query_trace_dropped() == 0
            && statements.len() == expected
            && proxy.stats().statements == expected as u64
            && trace.issued() == expected as u64,
        "budget wire count mismatch at {expected}: {:?} / {}",
        statements,
        trace.issued(),
    );
    ensure!(statements[0].contains("BEGIN ISOLATION LEVEL"));
    ensure!(
        statements.last().is_some_and(|sql| sql
            .split_whitespace()
            .last()
            .is_some_and(|word| word.trim_end_matches(';') == "COMMIT"))
            == committed
    );
    Ok(statements.len())
}
