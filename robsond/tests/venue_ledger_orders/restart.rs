//! Isolated PG16 process restart after a lost native COMMIT reply. An old
//! child owns the receiver and its pending ticket; the parent kills and reaps
//! it, proves its backend retired, starts an actual unclean successor epoch
//! and rebuilds only a typed local blocker from durable state. Nothing but a
//! sanitized marker crosses the process boundary.
use std::{
    os::unix::process::ExitStatusExt, process::Stdio, str::FromStr, sync::Arc, time::Duration,
};

use anyhow::{anyhow, ensure, Context, Result};
use robson_connectors::inventory_loopback::LOOPBACK_UID;
use robson_domain::OrderSide;
use robson_exec::{
    gate::{EpochView, PendingExecutionEvidence},
    order_journal::OrderJournal,
    venue::{OrderPhase, VenueOrderRecord},
    venue_clock::NativeClock,
    venue_ledger::{LedgerError, LegOrderKind, OrderKey, OrderLink, OrderOwner},
};
use robson_testkit::{
    daemon_process::database_url_through,
    db_proxy::{DbProxy, ProxyMode},
};
use rust_decimal::Decimal;
use sqlx::{
    postgres::{PgConnectOptions, PgPoolOptions, PgSslMode},
    PgPool,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, BufReader},
    process::Command,
    time::{timeout, timeout_at, Instant},
};
use uuid::Uuid;

use super::{
    exact_reference, generation_and_start, journal, setup_tables, support, version, write,
};
use crate::{
    order_journal_store::PgJournalBackend,
    process_epoch::{self, Predecessor},
    spot_order_set_store::JournalGeneration,
    venue_budget,
    venue_clock_owner::RestartLookup,
    venue_ledger_store::{
        pending_execution::OrderExecutionReceiver,
        restart_execution::{RestartExecutionBlocker, RestartState},
        PgVenueLedger,
    },
};

/// The single fixture order; a labelled example, never a policy restriction.
const ORDER: u64 = 417_013;
const CLIENT: &str = "order-417013";
const UPDATE_MS: i64 = 1_763_500_000_000;
const CHILD: &str = "pending_execution_restart_old_child";
const MARK: &str = "C8_RESTART_OLD_PENDING ";
const APP: &str = "robson-c8-restart-old";
const ROLE_VAR: &str = "ROBSON_TEST_C8_RESTART_ROLE";
const URL_VAR: &str = "ROBSON_TEST_C8_RESTART_URL";
const BINDING_VAR: &str = "ROBSON_TEST_C8_RESTART_BINDING";
const ROLE: &str = "old-owner";

fn sell(executed: Decimal, update_ms: i64) -> VenueOrderRecord {
    VenueOrderRecord {
        exchange_order_id: ORDER.to_string(),
        client_order_id: CLIENT.into(),
        orig_client_order_id: None,
        symbol: support::SYMBOL.into(),
        side: OrderSide::Sell,
        order_type: "MARKET".into(),
        protection_kind: None,
        status: "PARTIALLY_FILLED".into(),
        phase: OrderPhase::Live,
        orig_qty: Decimal::new(10_000, 0),
        executed_qty: executed,
        // Fixture fill price 100.
        cumulative_quote_qty: Some(executed * Decimal::new(100, 0)),
        stop_price: None,
        expiry_reason: None,
        transact_time_ms: None,
        time_ms: None,
        update_time_ms: Some(update_ms),
        working_time_ms: None,
    }
}

fn order_body(executed: &str, quote: &str) -> String {
    order_body_at(executed, quote, UPDATE_MS + 1_000)
}

fn order_body_at(executed: &str, quote: &str, updated: i64) -> String {
    format!(
        r#"{{"symbol":"BTCUSDT","orderId":417013,"orderListId":-1,"clientOrderId":"{CLIENT}","price":"0","origQty":"10000","executedQty":"{executed}","cummulativeQuoteQty":"{quote}","status":"PARTIALLY_FILLED","timeInForce":"GTC","type":"MARKET","side":"SELL","stopPrice":"0","time":{UPDATE_MS},"updateTime":{updated},"workingTime":-1,"selfTradePreventionMode":"EXPIRE_MAKER"}}"#
    )
}

fn key(scope: &str) -> Result<OrderKey> {
    Ok(OrderKey {
        venue: support::venue(),
        account_scope: scope.into(),
        symbol: support::SYMBOL.into(),
        exchange_order_id: ORDER.to_string().parse().map_err(|_| anyhow!("fixture order id"))?,
    })
}

fn link(scope: &str, position: Uuid, intent: Uuid) -> OrderLink {
    OrderLink {
        venue: support::venue(),
        account_scope: scope.into(),
        owner: OrderOwner::Leg {
            position_id: position,
            intent_id: intent,
            kind: LegOrderKind::Sell,
        },
        client_order_id: CLIENT.into(),
    }
}

async fn wait_sessions(pool: &PgPool, application: &str) -> Result<()> {
    timeout(Duration::from_secs(3), async {
        loop {
            let count: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM pg_stat_activity
                 WHERE datname = current_database() AND application_name = $1",
            )
            .bind(application)
            .fetch_one(pool)
            .await?;
            if count == 0 {
                return Ok::<_, anyhow::Error>(());
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .context("old owner PostgreSQL sessions retired")?
}

/// Only stable sanitized fields: no ticket, sample or observed amount.
struct OldReport {
    pid: u32,
    backend: i32,
}

fn parse_marker(line: &str) -> Result<OldReport> {
    let field = |name: &str| {
        line.split(' ')
            .find_map(|part| part.strip_prefix(name)?.strip_prefix('='))
            .with_context(|| format!("old marker field {name}"))
    };
    ensure!(field("uncertain")? == "1" && field("pending")? == "1" && field("dropped")? == "1");
    Ok(OldReport {
        pid: field("pid")?.parse()?,
        backend: field("backend")?.parse()?,
    })
}

/// The accepted restart prefix, shared by every successor proof: one live
/// linked SELL, an old owner whose 0.25 COMMIT landed while its reply was
/// dropped, SIGKILL and reap, retired backend and sessions, and an actual
/// unclean successor epoch whose own gate starts fenced.
struct Restart {
    f: support::Fixture,
    clock: Arc<NativeClock>,
    input: venue_budget::bootstrap::BootstrapInput,
    intent: Uuid,
    binding: JournalGeneration,
    order_link: OrderLink,
    seeded: i64,
    successor: process_epoch::EpochStart,
    journal: OrderJournal,
    upstream: std::net::SocketAddr,
    child_pid: u32,
    durable: (Decimal, Option<Decimal>, Option<i64>, Uuid),
}

async fn restart_after_lost_ack(pool: &PgPool) -> Result<Restart> {
    let pool = pool.clone();
    setup_tables(&pool).await?;
    let clock = Arc::new(NativeClock::initialize()?);
    let traces = venue_budget::Trace::default();
    let input = venue_budget::fixture::prepare(
        &pool,
        &clock,
        Instant::now() + Duration::from_secs(2),
        &traces,
    )
    .await
    .map_err(|error| anyhow!("empty C2 fixture: {error:?}"))?;
    ensure!(traces.issued() == 7);
    let f = support::Fixture::new(&pool).await;
    let intent = Uuid::now_v7();
    journal(&pool, &f, intent, CLIENT, "exit", Some(f.position), None, false).await?;
    let (binding, old) = generation_and_start(&pool, &f).await?;
    // Bound to a cutover run of the tenant (M56's foreign key).
    sqlx::query(include_str!("../fixtures/bound_venue_account.sql"))
        .bind("binance:spot")
        .bind(&f.scope)
        .bind(f.tenant)
        .bind(LOOPBACK_UID.to_string())
        .execute(&pool)
        .await?;
    let ledger = f.ledger(&pool).with_native_order_set_projection(binding.clone());
    let order_link = link(&f.scope, f.position, intent);
    // The known pre-existing live linked SELL the old owner later advances.
    write(
        &pool,
        &ledger,
        &binding,
        &sell(Decimal::new(10, 2), UPDATE_MS),
        &order_link,
        support::seen(),
    )
    .await?;
    let seeded = version(&pool, &binding).await?;
    ensure!(!f.scope.contains(char::is_whitespace));

    let options = pool.connect_options();
    let upstream = tokio::net::lookup_host((options.get_host(), options.get_port()))
        .await?
        .next()
        .context("fixture PostgreSQL endpoint")?;
    ensure!(upstream.ip().is_loopback(), "local PostgreSQL fixture");
    let url = database_url_through(
        &std::env::var("DATABASE_URL")?,
        upstream,
        options.get_database().context("macro database name")?,
    )?;
    let (_, module) = module_path!().split_once("::").context("libtest module")?;
    let fixture_binding = format!(
        "{} {} {} {} {} {} {} {}",
        binding.tenant_id,
        binding.account_scope,
        f.position,
        intent,
        binding.generation_id,
        binding.source_audit_id,
        binding.process_epoch,
        old.epoch.epoch_seq,
    );
    let mut child = Command::new(std::env::current_exe()?)
        .args([
            "--exact",
            &format!("{module}::{CHILD}"),
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env_clear()
        .env("RUST_MIN_STACK", "1048576")
        .env(ROLE_VAR, ROLE)
        .env(URL_VAR, &url)
        .env(BINDING_VAR, fixture_binding)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()?;
    let child_pid = child.id().context("old owner pid")?;
    ensure!(child_pid != std::process::id(), "separate actor processes");
    let mut lines =
        BufReader::new(child.stdout.take().context("old owner stdout")?.take(4096)).lines();
    let marker = timeout(Duration::from_secs(60), async {
        while let Some(line) = lines.next_line().await? {
            if let Some(marker) = line.strip_prefix(MARK) {
                return Ok::<_, anyhow::Error>(marker.to_owned());
            }
        }
        Err(anyhow!("old owner exited before its pending marker"))
    })
    .await;
    // Retire and reap even if the marker never arrives or is malformed.
    let kill = child.start_kill();
    let status = timeout(Duration::from_secs(5), child.wait()).await.context("bounded reap")??;
    let report = parse_marker(&marker.context("bounded old owner marker")??)?;
    kill?;
    ensure!(report.pid == child_pid);
    ensure!(status.signal() == Some(9), "old owner killed while parked, not exited");
    drop(lines);
    support::wait_for_backend_exit(&pool, report.backend).await;
    wait_sessions(&pool, APP).await?;

    // The lost reply's COMMIT landed; durable state alone shows it.
    let durable: (Decimal, Option<Decimal>, Option<i64>, Uuid) = sqlx::query_as(
        "SELECT executed_cum, cumulative_quote, venue_update_time_ms, intent_id FROM venue_orders
         WHERE tenant_id=$1 AND venue_id='binance:spot' AND account_scope=$2
           AND symbol=$3 AND exchange_order_id=$4 AND owner='leg'",
    )
    .bind(f.tenant)
    .bind(&f.scope)
    .bind(support::SYMBOL)
    .bind(ORDER.to_string())
    .fetch_one(&pool)
    .await?;
    let exact = sell(Decimal::new(25, 2), UPDATE_MS + 1_000);
    ensure!(
        durable == (exact.executed_qty, exact.cumulative_quote_qty, exact.update_time_ms, intent)
    );
    ensure!(version(&pool, &binding).await? == seeded + 1);

    let successor = process_epoch::start_epoch(&pool, f.tenant, "native-order-restart").await?;
    ensure!(successor.epoch.epoch_id != binding.process_epoch);
    ensure!(successor.predecessor_epoch_id == Some(binding.process_epoch));
    ensure!(successor.predecessor == Predecessor::NoMarker { epoch_seq: old.epoch.epoch_seq });
    ensure!(successor.unclean_generation.is_some());
    let successor_journal =
        OrderJournal::new(Arc::new(PgJournalBackend::new(&pool, f.tenant)), &order_link.venue);
    let gate = successor_journal.gate();
    gate.begin_epoch(
        EpochView {
            epoch_seq: successor.epoch.epoch_seq,
            unclean_reason: successor.predecessor.unclean_reason(),
        },
        successor.unclean_generation,
        successor.open_incidents.clone(),
    );
    ensure!(gate.fenced(), "unclean successor starts with the native fence");
    Ok(Restart {
        f,
        clock,
        input,
        intent,
        binding,
        order_link,
        seeded,
        successor,
        journal: successor_journal,
        upstream,
        child_pid,
        durable,
    })
}

/// One bounded read-only reader session through the tracing proxy.
async fn read_only(pool: &PgPool, proxy: &DbProxy) -> Result<PgPool> {
    let readonly_options = pool
        .connect_options()
        .as_ref()
        .clone()
        .host(&proxy.addr().ip().to_string())
        .port(proxy.addr().port())
        .ssl_mode(PgSslMode::Disable)
        .options([
            ("default_transaction_read_only", "on"),
            ("lock_timeout", "300ms"),
            ("statement_timeout", "1s"),
        ]);
    let readonly = PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(Duration::from_secs(2))
        .test_before_acquire(false)
        .connect_with(readonly_options)
        .await?;
    let session_readonly: String =
        sqlx::query_scalar("SHOW transaction_read_only").fetch_one(&readonly).await?;
    ensure!(session_readonly == "on");
    Ok(readonly)
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn projected_native_order_pending_execution_recovers_after_restart(
    pool: PgPool,
) -> Result<()> {
    let Restart {
        f,
        clock,
        input,
        binding,
        order_link,
        successor,
        journal: successor_journal,
        upstream,
        child_pid,
        durable,
        ..
    } = restart_after_lost_ack(&pool).await?;
    let gate = successor_journal.gate();
    let frozen = support::snapshot(&pool).await;
    let accounting = f.version(&pool).await;

    let deadline = Instant::now() + Duration::from_secs(148);
    let proxy = DbProxy::start(upstream).await?;
    proxy.enable_exact_query_trace(24);
    let readonly = read_only(&pool, &proxy).await?;
    let mut lookup = RestartLookup::new(clock, deadline);
    let outcomes = timeout_at(deadline, async {
        let foreign = RestartExecutionBlocker::reconstruct(
            &readonly,
            &binding,
            key(&f.scope)?,
            link(&f.scope, f.position, Uuid::now_v7()),
            &successor,
        )
        .await;
        ensure!(matches!(foreign, Err(LedgerError::IdentityConflict)));

        let mut blocker = RestartExecutionBlocker::reconstruct(
            &readonly,
            &binding,
            key(&f.scope)?,
            order_link.clone(),
            &successor,
        )
        .await?;
        ensure!(
            blocker.state() == RestartState::Blocked
                && blocker.key() == &key(&f.scope)?
                && gate.fenced()
        );

        eprintln!(
            "C8_RESTART_QUARANTINE predecessor=no_marker old_killed=SIGKILL backend_retired=1"
        );
        lookup
            .arm(&pool, f.tenant, &successor, &input, [
                (400, r#"{"code":-2013,"msg":"synthetic unresolved order"}"#.into()),
                (200, order_body("0.25", "25")),
                (200, order_body("0.25", "26")),
                (200, order_body("0.30", "30")),
                (200, order_body("0.25", "25")),
                (200, order_body("0.25", "25")),
            ])
            .await?;
        let checked = blocker.precheck().await?;
        let receipt = lookup.read(&proxy).await?;
        ensure!(checked.apply(receipt).await? == RestartState::Blocked, "unresolved order");
        ensure!(blocker.state() == RestartState::Blocked && gate.fenced());
        let checked = blocker.precheck().await?;
        let receipt = lookup.read(&proxy).await?;
        ensure!(checked.apply(receipt).await? == RestartState::Resolved, "exact stable match");
        ensure!(gate.fenced(), "local resolution leaves the native fence held");
        ensure!(support::snapshot(&pool).await == frozen && f.version(&pool).await == accounting);
        exact_reference(&pool, &binding).await?;

        // Separate refusal controls over the same frozen PG fixture. The
        // positive witness above is historical and never opens the gate.
        let mut conflicting = RestartExecutionBlocker::reconstruct(
            &readonly,
            &binding,
            key(&f.scope)?,
            order_link.clone(),
            &successor,
        )
        .await?;
        for (scenario, expected) in [
            ("same execution with different quote", Decimal::new(25, 2)),
            ("newer execution", Decimal::new(30, 2)),
            ("late exact record after newer execution", Decimal::new(30, 2)),
        ] {
            let checked = conflicting.precheck().await?;
            let receipt = lookup.read(&proxy).await?;
            ensure!(checked.apply(receipt).await? == RestartState::Blocked, "{scenario}");
            ensure!(conflicting.retained_execution() == Some(expected) && gate.fenced());
        }

        // An exact fresh response cannot resolve after the durable basis moves.
        let mut second = RestartExecutionBlocker::reconstruct(
            &readonly,
            &binding,
            key(&f.scope)?,
            order_link,
            &successor,
        )
        .await?;
        let checked = second.precheck().await?;
        let receipt = lookup.read(&proxy).await?;
        process_epoch::start_epoch(&pool, f.tenant, "native-order-interleaved").await?;
        ensure!(checked.apply(receipt).await? == RestartState::Blocked, "moved epoch");
        ensure!(second.precheck().await.is_err() && gate.fenced());
        ensure!(support::snapshot(&pool).await == frozen && f.version(&pool).await == accounting);
        lookup.verify_weight()?;
        Ok::<_, anyhow::Error>(())
    })
    .await
    .context("148s restart rehearsal ceiling")
    .and_then(|inner| inner);
    let retired = lookup.finish(&successor, outcomes.is_err()).await;
    let readonly_stopped = timeout(Duration::from_secs(2), readonly.close()).await;
    retired?;
    readonly_stopped.context("reader pool retirement")?;
    outcomes?;
    let trace = proxy.query_trace();
    ensure!(
        proxy.query_trace_dropped() == 0 && trace.len() == 14,
        "bounded read-only trace: {trace:?}"
    );
    ensure!(
        trace
            .iter()
            .filter(|query| query.trim_start().starts_with("SELECT o.client_order_id"))
            .count()
            == 13
            && trace.iter().all(|query| query.trim_start().starts_with("SELECT ")
                || query == "SHOW transaction_read_only")
    );
    eprintln!(
        "C8_PENDING_RESTART old_pending=1 old_uncertain=1 commit_acks_dropped=1 old_killed=SIGKILL \
         backend_retired=1 old_pid={child_pid} successor_pid={} durable_executed={} version=seeded+1 \
         predecessor=no_marker recovery=blocked(unresolved,wrong_quote,newer_execution,late_old),resolved_exact \
         interleaved_epoch=blocked quarantine_seconds=120 time_handoffs=1 get_handoffs=6 total_weight=25 \
         get_sql=0 recovery_selects=13 ledger_writes=0 native_fence=held",
        std::process::id(), durable.0
    );
    Ok(())
}

#[test]
#[ignore = "old owner process of projected_native_order_pending_execution_recovers_after_restart"]
fn pending_execution_restart_old_child() -> Result<()> {
    ensure!(std::env::var(ROLE_VAR)? == ROLE);
    let url = std::env::var(URL_VAR)?;
    let fixture = std::env::var(BINDING_VAR)?;
    let fields: Vec<&str> = fixture.split(' ').collect();
    let [tenant, scope, position, intent, generation, audit, epoch, seq] = fields.as_slice() else {
        return Err(anyhow!("old owner fixture binding"));
    };
    let binding = JournalGeneration {
        tenant_id: tenant.parse()?,
        account_scope: (*scope).to_owned(),
        generation_id: generation.parse()?,
        source_audit_id: audit.parse()?,
        process_epoch: epoch.parse()?,
    };
    let (position, intent, seq) = (position.parse()?, intent.parse()?, seq.parse()?);
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(old_owner(&url, binding, position, intent, seq))
}

async fn old_owner(
    url: &str,
    binding: JournalGeneration,
    position: Uuid,
    intent: Uuid,
    epoch_seq: i64,
) -> Result<()> {
    let direct =
        PgConnectOptions::from_str(url).map_err(|_| anyhow!("fixture PostgreSQL options"))?;
    let host: std::net::IpAddr = direct.get_host().parse()?;
    ensure!(
        host.is_loopback()
            && direct.get_port() == 15671
            && direct.get_database().is_some_and(|name| name.starts_with("_sqlx_test_")),
        "local macro-owned PostgreSQL fixture"
    );
    let proxy = DbProxy::start(std::net::SocketAddr::new(host, direct.get_port())).await?;
    let proxied =
        database_url_through(url, proxy.addr(), direct.get_database().context("database")?)?;
    let options = PgConnectOptions::from_str(&proxied)
        .map_err(|_| anyhow!("proxied PostgreSQL options"))?
        .application_name(APP);
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .test_before_acquire(false)
        .connect_with(options)
        .await?;
    let backend: i32 = sqlx::query_scalar("SELECT pg_backend_pid()").fetch_one(&pool).await?;
    let ledger = PgVenueLedger::new(&pool, binding.tenant_id)
        .with_native_order_set_projection(binding.clone());
    let order_link = link(&binding.account_scope, position, intent);
    let journal = OrderJournal::new(
        Arc::new(PgJournalBackend::new(&pool, binding.tenant_id)),
        &order_link.venue,
    );
    let gate = journal.gate();
    gate.begin_epoch(
        EpochView {
            epoch_seq,
            unclean_reason: Predecessor::None.unclean_reason(),
        },
        None,
        Vec::new(),
    );
    let receiver =
        OrderExecutionReceiver::bind(gate, &ledger, key(&binding.account_scope)?, order_link)?;
    let clock = NativeClock::initialize()?;
    let evidence = PendingExecutionEvidence {
        sample: Uuid::now_v7(),
        send: clock.wall_bracket(binding.process_epoch)?,
        receive: clock.wall_bracket(binding.process_epoch)?,
        executed: Decimal::new(25, 2),
    };
    let work = receiver
        .register(sell(evidence.executed, UPDATE_MS + 1_000), evidence)?
        .context("old owner ticket")?;
    ensure!(receiver.pending() == Some(evidence), "registered before any persistence await");
    proxy.enable_exact_query_trace(16);
    proxy.set_mode(ProxyMode::DropCommitAckOf("UPDATE venue_orders"));
    let before = ledger.statements_total();
    ensure!(matches!(work.persist().await, Err(LedgerError::Uncertain(_))));
    ensure!(ledger.statements_total() - before == 9 && proxy.stats().commit_acks_dropped == 1);
    ensure!(
        receiver.pending() == Some(evidence),
        "lost native reply keeps the observation pending"
    );
    println!(
        "\n{MARK}pid={} backend={backend} uncertain=1 pending=1 dropped=1",
        std::process::id()
    );
    // Park for the parent's SIGKILL; a normal return here is a failure.
    tokio::time::sleep(Duration::from_secs(60)).await;
    Err(anyhow!("old owner was not killed"))
}

#[path = "advance.rs"]
mod advance;
