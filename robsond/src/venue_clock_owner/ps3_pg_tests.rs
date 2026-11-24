//! PS-3 on PG16 and a loopback TIME server (crash-points harness URL): the
//! production construction, C8 durability before publication, on-demand
//! samples under their caller's deadline, clock evidence across a restart,
//! the stop's join, the store's statements per branch, and the metrics.
//! Budgets start 61 s in the past (injected ticks); no limiter window is
//! waited for. A fresh tenant's start is unclean, so its first scheduled
//! cycle is 60 s away and never interferes.
use std::{
    collections::BTreeSet,
    sync::atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicUsize, Ordering},
};

use anyhow::{anyhow, ensure, Context, Result};
use chrono::Utc;
use robson_testkit::db_proxy::{DbProxy, ProxyMode};
use sqlx::{
    postgres::{PgPoolOptions, PgSslMode},
    PgPool,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

use super::*;
use crate::{
    process_epoch,
    stop::{run_stop_with_clock, MarkerWrite, StopResult, StopTimings},
    venue_budget::state::SharedBudget,
};

/// A loopback TIME venue: one answer per connection, after `delay_ms` and
/// once not held, with `serverTime` shifted by `offset_ms`; connections
/// counted, each answer's instant kept.
struct TimeServer {
    base_url: String,
    connections: Arc<AtomicUsize>,
    delay_ms: Arc<AtomicU64>,
    offset_ms: Arc<AtomicI64>,
    hold: tokio::sync::watch::Sender<bool>,
    answered: Arc<Mutex<Vec<Instant>>>,
    task: JoinHandle<()>,
}

impl TimeServer {
    async fn start() -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let base_url = format!("http://{}", listener.local_addr()?);
        let connections = Arc::new(AtomicUsize::new(0));
        let delay_ms = Arc::new(AtomicU64::new(0));
        let offset_ms = Arc::new(AtomicI64::new(0));
        let (hold, held) = tokio::sync::watch::channel(false);
        let answered = Arc::new(Mutex::new(Vec::new()));
        let (count, delay, offset, answers) = (
            Arc::clone(&connections),
            Arc::clone(&delay_ms),
            Arc::clone(&offset_ms),
            Arc::clone(&answered),
        );
        let task = tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                count.fetch_add(1, Ordering::SeqCst);
                let (delay, offset) = (delay.load(Ordering::SeqCst), offset.load(Ordering::SeqCst));
                let (mut held, answers) = (held.clone(), Arc::clone(&answers));
                tokio::spawn(async move {
                    let mut head = Vec::new();
                    while !head.ends_with(b"\r\n\r\n") && head.len() < 16_384 {
                        match socket.read_u8().await {
                            Ok(byte) => head.push(byte),
                            Err(_) => return,
                        }
                    }
                    tokio::time::sleep(Duration::from_millis(delay)).await;
                    let _ = held.wait_for(|held| !*held).await;
                    let body =
                        format!("{{\"serverTime\":{}}}", Utc::now().timestamp_millis() + offset);
                    let answer = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: \
                         {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    answers.lock().unwrap().push(Instant::now());
                    let _ = socket.write_all(answer.as_bytes()).await;
                    let _ = socket.shutdown().await;
                });
            }
        });
        Ok(Self {
            base_url,
            connections,
            delay_ms,
            offset_ms,
            hold,
            answered,
            task,
        })
    }

    fn connections(&self) -> usize {
        self.connections.load(Ordering::SeqCst)
    }

    /// Answers wait, from now until [`Self::release`], however long that is.
    fn hold(&self) {
        self.hold.send_replace(true);
    }

    fn release(&self) {
        self.hold.send_replace(false);
    }

    /// When the venue wrote each answer, in order.
    fn answered(&self) -> Vec<Instant> {
        self.answered.lock().unwrap().clone()
    }
}

impl Drop for TimeServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// A capture `seconds` before now, in the clock's domain.
fn before(clock: &NativeClock, seconds: i64) -> Result<TickCapture> {
    let now = clock.capture()?.nanoseconds();
    Ok(TickCapture::new(
        clock.capture()?.domain(),
        i64::try_from(now / 1_000_000_000)? - seconds,
        i64::try_from(now % 1_000_000_000)?,
    )?)
}

struct Rig {
    clock: Arc<NativeClock>,
    tenant: Uuid,
    start: process_epoch::EpochStart,
    budget: NonTradingBudget,
    server: TimeServer,
}

/// An epoch, a non-trading budget past its grant window and a TIME server.
async fn rig(pool: &PgPool) -> Result<Rig> {
    let clock = Arc::new(NativeClock::initialize()?);
    let tenant = Uuid::now_v7();
    let start = process_epoch::start_epoch(pool, tenant, "ps-3-test").await?;
    let budget = SharedBudget::non_trading(before(&clock, 61)?, None, false)
        .map_err(|e| anyhow!("budget: {e:?}"))?;
    Ok(Rig {
        clock,
        tenant,
        start,
        budget,
        server: TimeServer::start().await?,
    })
}

impl Rig {
    fn owner_on(&self, pool: &PgPool, start: &process_epoch::EpochStart) -> Result<ClockOwner> {
        let client = Arc::new(BinanceRestClient::with_harness_base_url(
            self.server.base_url.clone(),
            String::new(),
            String::new(),
        ));
        ClockOwner::non_trading(
            Arc::clone(&self.clock),
            client,
            pool,
            self.tenant,
            start,
            &self.budget,
        )
        .map_err(|e| anyhow!("TIME owner: {e:?}"))
    }

    fn owner(&self, pool: &PgPool) -> Result<ClockOwner> {
        self.owner_on(pool, &self.start)
    }

    fn handoffs(&self) -> Result<u64> {
        self.budget.time_owner().handoffs().map_err(|e| anyhow!("{e:?}"))
    }
}

/// A pool whose connections go through a statement-counting proxy.
async fn wire(pool: &PgPool) -> Result<(DbProxy, PgPool)> {
    let options = pool.connect_options();
    let upstream = tokio::net::lookup_host((options.get_host(), options.get_port()))
        .await?
        .next()
        .ok_or_else(|| anyhow!("no upstream address"))?;
    let proxy = DbProxy::start(upstream).await?;
    let wired = PgPoolOptions::new().max_connections(1).connect_lazy_with(
        (*options)
            .clone()
            .host("127.0.0.1")
            .port(proxy.addr().port())
            .ssl_mode(PgSslMode::Disable),
    );
    Ok((proxy, wired))
}

async fn sample_row(pool: &PgPool, sample: Uuid) -> Result<Option<(Uuid, String)>> {
    Ok(sqlx::query_as(
        "SELECT process_epoch, numeric_verdict FROM clock_samples WHERE sample_id = $1",
    )
    .bind(sample)
    .fetch_optional(pool)
    .await?)
}

async fn epoch_samples(pool: &PgPool, epoch: Uuid) -> Result<i64> {
    Ok(
        sqlx::query_scalar("SELECT count(*) FROM clock_samples WHERE process_epoch = $1")
            .bind(epoch)
            .fetch_one(pool)
            .await?,
    )
}

fn in_ms(ms: u64) -> Instant {
    Instant::now() + Duration::from_millis(ms)
}

/// C8 durability (PLAN §6.6): a sample whose row cannot land is never
/// published, though its TIME request went out; a landed one is published
/// only with its row of this epoch already stored.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_sample_is_published_only_after_it_lands(pool: PgPool) -> Result<()> {
    let rig = rig(&pool).await?;
    let (proxy, wired) = wire(&pool).await?;
    let mut owner = rig.owner(&wired)?;
    let handle = owner.handle();
    // The database refuses at once: the store fails before the cycle's
    // persistence bound, so nothing but the failure stops the publication.
    // The refusal is the severed connect's I/O error (Backend), which no
    // bound's expiry yields (Expired, or Unpublished after an attempt); no
    // fixed window over the cycle's transport preparation and connects.
    proxy.set_mode(ProxyMode::Sever);
    let lost = owner.run_cycle_kind(CycleKind::OnDemand { deadline: in_ms(5_000) }).await;
    ensure!(
        matches!(lost, Err(TimeError::Storage(crate::venue_clock_store::StoreError::Backend))),
        "{lost:?}"
    );
    ensure!(rig.server.connections() == 1 && rig.handoffs()? == 1, "the request went out");
    ensure!(handle.clock_evidence().is_err(), "published without a landed row");
    ensure!(epoch_samples(&pool, rig.start.epoch.epoch_id).await? == 0);
    proxy.set_mode(ProxyMode::Pass);
    let landed = owner.run_cycle().await.map_err(|e| anyhow!("scheduled cycle: {e:?}"))?;
    let row = sample_row(&pool, landed).await?;
    ensure!(row == Some((rig.start.epoch.epoch_id, "valid".into())), "{row:?}");
    let verdict = handle.clock_evidence().map_err(|e| anyhow!("evidence: {e:?}"))?;
    ensure!(verdict.sample == landed && verdict.valid && verdict.u2 <= 2_000_000_000);
    ensure!(verdict.age < Duration::from_secs(60), "{verdict:?}");
    Ok(())
}

/// A sample of the TIME request's answer that fails the numeric checks
/// (u2 > 2 s) is stored invalid and never published; every clock metric is
/// registered, zero-valued, when the owner is built (PLAN §6.11).
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn an_invalid_sample_is_stored_and_never_published(pool: PgPool) -> Result<()> {
    let rig = rig(&pool).await?;
    rig.server.offset_ms.store(5_000, Ordering::SeqCst);
    let mut owner = rig.owner(&pool)?;
    let names: BTreeSet<String> = prometheus::gather()
        .iter()
        .map(|family| family.get_name().to_string())
        .collect();
    let cycles = prometheus::gather()
        .into_iter()
        .find(|family| family.get_name() == "robsond_spot_time_cycles_total")
        .map(|family| {
            let labels =
                family.get_metric().iter().map(|m| m.get_label()[0].get_value().to_string());
            labels.collect::<BTreeSet<_>>()
        })
        .unwrap_or_default();
    for outcome in [
        "published",
        "deferred",
        "budget",
        "expired",
        "closing",
        "transport",
        "native",
        "resource",
        "payload",
        "venue",
        "storage",
    ] {
        ensure!(cycles.contains(outcome), "cycle outcome {outcome} not registered: {cycles:?}");
    }
    for name in [
        "robsond_spot_clock_fault",
        "robsond_spot_clock_faults_total",
        "robsond_spot_clock_uncertainty_seconds",
        "robsond_spot_time_cycles_total",
        "robsond_spot_response_restriction_level",
        "robsond_clock_evidence_valid",
        "robsond_venue_clock_bound_seconds",
    ] {
        ensure!(names.contains(name), "{name} not registered at start: {names:?}");
    }
    let refused = owner.run_cycle().await;
    ensure!(matches!(refused, Err(TimeError::Expired)), "{refused:?}");
    let stored: Vec<(String, String)> = sqlx::query_as(
        "SELECT numeric_verdict, u2_ns::text FROM clock_samples WHERE process_epoch = $1",
    )
    .bind(rig.start.epoch.epoch_id)
    .fetch_all(&pool)
    .await?;
    ensure!(stored.len() == 1 && stored[0].0 == "invalid", "{stored:?}");
    ensure!(stored[0].1.parse::<i128>()? > 2_000_000_000);
    ensure!(owner.handle().clock_evidence().is_err(), "an invalid sample was published");
    Ok(())
}

/// A restart's owner never validates on a sample of the previous epoch: it
/// has no evidence until its own sample, of its own epoch, lands.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn clock_evidence_never_crosses_an_epoch(pool: PgPool) -> Result<()> {
    let rig = rig(&pool).await?;
    let mut first = rig.owner(&pool)?;
    let old = first.run_cycle().await.map_err(|e| anyhow!("first epoch: {e:?}"))?;
    ensure!(first.handle().clock_evidence().map_err(|e| anyhow!("{e:?}"))?.sample == old);
    let restart = process_epoch::start_epoch(&pool, rig.tenant, "ps-3-test").await?;
    ensure!(restart.predecessor_epoch_id == Some(rig.start.epoch.epoch_id));
    let mut second = rig.owner_on(&pool, &restart)?;
    let handle = second.handle();
    ensure!(handle.clock_evidence().is_err(), "the restart validated on the old sample");
    let new = second.run_cycle().await.map_err(|e| anyhow!("restart: {e:?}"))?;
    let verdict = handle.clock_evidence().map_err(|e| anyhow!("{e:?}"))?;
    ensure!(new != old && verdict.sample == new && verdict.valid);
    ensure!(sample_row(&pool, old).await? == Some((rig.start.epoch.epoch_id, "valid".into())));
    ensure!(sample_row(&pool, new).await? == Some((restart.epoch.epoch_id, "valid".into())));
    Ok(())
}

/// A marker that records whether the TIME owner's sampler and watchdog were
/// joined when the stop wrote it.
struct JoinedAtMark(Arc<ClockClosing>, AtomicBool);

#[async_trait::async_trait]
impl crate::stop::StopMarker for JoinedAtMark {
    async fn mark_clean(&self) -> Result<(), String> {
        let sampler = self.0.worker.try_lock().is_ok_and(|task| task.is_none());
        let watchdog = self.0.watchdog.try_lock().is_ok_and(|task| task.is_none());
        self.1.store(sampler && watchdog, Ordering::SeqCst);
        Ok(())
    }
}

/// PLAN §6.10: the non-trading owner's closing is the stop's clock: the
/// stop joins its sampler and watchdog before it writes the clean marker.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn the_stop_joins_the_time_owner_before_the_marker(pool: PgPool) -> Result<()> {
    let rig = rig(&pool).await?;
    let closing = rig.owner(&pool)?.spawn();
    let backend = Arc::new(robson_exec::order_journal::StubJournalBackend::default());
    let journal =
        robson_exec::order_journal::OrderJournal::new(backend, &VenueId::new("test:venue"));
    let marker = JoinedAtMark(Arc::clone(&closing), AtomicBool::new(false));
    let report = run_stop_with_clock(
        &journal,
        Instant::now(),
        || {},
        Some(&marker),
        Some(&*closing),
        StopTimings::default(),
    )
    .await;
    ensure!(
        (report.result, report.marker) == (StopResult::Clean, MarkerWrite::Written),
        "{:?}",
        (report.result, report.marker)
    );
    ensure!(
        marker.1.load(Ordering::SeqCst),
        "the marker came before the owner's tasks ended"
    );
    ensure!(rig.server.connections() == 0, "no TIME request inside the grant");
    Ok(())
}

/// An on-demand sample is budgeted like any TIME request: refused, unsent,
/// inside the grant window; charged at its handoff (weight 1) after it.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn an_on_demand_sample_is_charged(pool: PgPool) -> Result<()> {
    let mut rig = rig(&pool).await?;
    let past = rig.budget.clone();
    rig.budget = SharedBudget::non_trading(rig.clock.capture()?, None, false)
        .map_err(|e| anyhow!("{e:?}"))?;
    let mut fresh = rig.owner(&pool)?;
    let refused = fresh.run_cycle_kind(CycleKind::OnDemand { deadline: in_ms(5_000) }).await;
    ensure!(matches!(refused, Err(TimeError::Budget(BudgetError::Deferred))), "{refused:?}");
    ensure!(rig.server.connections() == 0 && rig.handoffs()? == 0, "sent inside the grant");
    rig.budget = past;
    let mut owner = rig.owner(&pool)?;
    owner
        .run_cycle_kind(CycleKind::OnDemand { deadline: in_ms(5_000) })
        .await
        .map_err(|e| anyhow!("on-demand: {e:?}"))?;
    ensure!(rig.server.connections() == 1 && rig.handoffs()? == 1, "one charged handoff");
    // Its weight is in the minute: 19 ACCOUNTs fit beside it, a 20th does not.
    let now = rig.clock.capture()?;
    let until =
        TickCapture::new(now.domain(), i64::try_from(now.nanoseconds() / 1_000_000_000)? + 10, 0)?;
    let held: Vec<_> = (0..19)
        .map(|_| {
            rig.budget
                .try_reserve_read(crate::venue_budget::state::ReadKind::Account, now, until)
        })
        .collect::<Result<_, _>>()
        .map_err(|e| anyhow!("{e:?}"))?;
    let over =
        rig.budget
            .try_reserve_read(crate::venue_budget::state::ReadKind::Account, now, until);
    ensure!(matches!(over, Err(BudgetError::Deferred)), "the TIME weight was not charged");
    drop(held);
    Ok(())
}

/// PLAN §6.6: O1, then O2 five seconds later, each gets its own handed-off
/// cycle despite the 60 s scheduled spacing; a request inside the 5 s is
/// refused, unsent.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn o2_five_seconds_after_o1_gets_its_own_sample(pool: PgPool) -> Result<()> {
    let rig = rig(&pool).await?;
    let owner = rig.owner(&pool)?;
    let handle = owner.handle();
    let closing = owner.spawn();
    let o1 = handle.request_sample(in_ms(10_000)).await.map_err(|e| anyhow!("O1: {e:?}"))?;
    let answered = Instant::now();
    let early = handle.request_sample(in_ms(10_000)).await;
    ensure!(early == Err(TimeError::Budget(BudgetError::Deferred)), "{early:?}");
    ensure!(rig.server.connections() == 1, "the early request was sent");
    tokio::time::sleep_until(answered + Duration::from_millis(5_100)).await;
    let o2 = handle.request_sample(in_ms(10_000)).await.map_err(|e| anyhow!("O2: {e:?}"))?;
    ensure!(o1 != o2 && rig.server.connections() == 2 && rig.handoffs()? == 2);
    ensure!(handle.clock_evidence().map_err(|e| anyhow!("{e:?}"))?.sample == o2);
    closing.begin_closing();
    ensure!(closing.join(in_ms(5_000)).await, "the owner's tasks did not end");
    Ok(())
}

/// PLAN §6.6, Root's condition: an on-demand cycle ends by its caller's
/// deadline, whatever holds it, over generated cases: a venue that holds its
/// answer (the HTTP exchange), a database that answers nothing (the
/// persistence), or nothing. Each is judged against what ignoring the
/// deadline would cost (`run_cycle_kind`'s bounds): the sampler takes the
/// next request (`Deferred`, within 5 s of the handoff) before the exchange's
/// 10 s from the cycle's start; the persistence's connection is dropped
/// before 2 s from the venue's answer.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn an_on_demand_cycle_never_outlives_its_caller_deadline(pool: PgPool) -> Result<()> {
    const HTTP_BOUND: Duration = Duration::from_secs(10);
    const PERSIST_BOUND: Duration = Duration::from_secs(2);
    let mut seed = 0x9e37_79b9_u64;
    for (venue_ms, blackhole) in [
        (2_000, false),
        (0, true),
        (300, false),
        (2_000, true),
        (0, false),
        (300, true),
    ] {
        seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
        // 2-3 s: inside the 5 s on-demand spacing, so the next request is
        // `Deferred` once the sampler takes it.
        let deadline_ms = 2_000 + (seed >> 33) % 1_000;
        // 2_000: the venue holds its answer past the deadline (released only
        // after the case). A blackhole: the venue answers half a second
        // before the deadline, into a persistence that nothing answers.
        let http_held = venue_ms == 2_000;
        let persistence_held = blackhole && !http_held;
        let rig = rig(&pool).await?;
        let (proxy, wired) = wire(&pool).await?;
        let owner = rig.owner(&wired)?;
        let handle = owner.handle();
        let closing = owner.spawn();
        if http_held || persistence_held {
            rig.server.hold();
        } else {
            rig.server.delay_ms.store(venue_ms, Ordering::SeqCst);
        }
        if blackhole {
            proxy.set_mode(ProxyMode::Blackhole);
        }
        let began = Instant::now();
        let deadline = began + Duration::from_millis(deadline_ms);
        let release = async {
            if persistence_held {
                while rig.server.connections() == 0 && Instant::now() < deadline {
                    tokio::time::sleep(Duration::from_millis(2)).await;
                }
                tokio::time::sleep_until(deadline - Duration::from_millis(500)).await;
                rig.server.release();
            }
        };
        // The persistence's connection, as the proxy sees it: dropped at the
        // caller's deadline by a correct cycle, kept to 2 s after the answer
        // (at the earliest) by one that ignores it. Its close is judged, not
        // the sampler's release, which also waits for the blocking parser.
        let connections = proxy.stats().connections;
        let closed = proxy.closed_relays();
        let done = AtomicBool::new(false);
        let close = async {
            loop {
                if !persistence_held {
                    break None;
                }
                if proxy.closed_relays() > closed {
                    break Some(Instant::now());
                }
                if done.load(Ordering::SeqCst) && proxy.stats().connections == connections {
                    break None;
                }
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        };
        let cycle = async {
            let (first, ()) = tokio::join!(handle.request_sample(deadline), release);
            // A held exchange runs to its 10 s bound; a cycle that ends by
            // itself, or behind the blackhole, ends by its own 10 s exchange
            // and 2 s persistence.
            let free_by = began
                + HTTP_BOUND
                + if http_held {
                    Duration::ZERO
                } else {
                    PERSIST_BOUND
                };
            let next = handle.request_sample(free_by).await;
            let free = Instant::now();
            done.store(true, Ordering::SeqCst);
            (first, next, free)
        };
        let close = async { tokio::time::timeout(HTTP_BOUND + PERSIST_BOUND, close).await };
        let ((first, next, free), close) = tokio::join!(cycle, close);
        let case = format!("venue {venue_ms} ms, blackhole {blackhole}, deadline {deadline_ms} ms");
        if http_held {
            ensure!(free < began + HTTP_BOUND, "{case}: free {:?} after the start", free - began);
        }
        // A persistence case whose request never went out (its preparation
        // outlasted the deadline) has no answer and no connection: not judged.
        if persistence_held && proxy.stats().connections > connections {
            let answered =
                *rig.server.answered().first().context("a persistence without an answer")?;
            let close = close.map_err(|_| anyhow!("{case}: the persistence was never dropped"))?;
            let close = close.context("a connection that never closed")?;
            ensure!(
                close < answered + PERSIST_BOUND,
                "{case}: the persistence was dropped {:?} after the answer",
                close - answered
            );
        }
        ensure!(
            next == Err(TimeError::Budget(BudgetError::Deferred)),
            "{case}: {first:?} {next:?}"
        );
        if http_held || persistence_held {
            ensure!(first.is_err() && handle.clock_evidence().is_err(), "{case}: {first:?}");
        }
        rig.server.release();
        proxy.set_mode(ProxyMode::Pass);
        closing.begin_closing();
        ensure!(closing.join(in_ms(5_000)).await, "{case}: the owner's tasks did not end");
    }
    Ok(())
}

/// PLAN §6.6: a request whose deadline passed while it waited behind the
/// cycle in flight is refused when the sampler takes it: unsent, uncharged,
/// and spending no on-demand spacing (the next request is admitted at once).
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn an_expired_queued_sample_request_is_refused_unsent(pool: PgPool) -> Result<()> {
    let clock = Arc::new(NativeClock::initialize()?);
    let tenant = Uuid::now_v7();
    // A clean predecessor: the first scheduled cycle comes at once (start +
    // 61 s has passed for this budget) and holds the sampler while the venue
    // holds its answer.
    sqlx::query(
        "INSERT INTO process_epochs (epoch_id, tenant_id, binary_version, clean_stop_at, \
         marker_version) VALUES ($1, $2, 'ps-3-predecessor', now(), $3)",
    )
    .bind(Uuid::now_v7())
    .bind(tenant)
    .bind(process_epoch::MARKER_VERSION)
    .execute(&pool)
    .await?;
    let start = process_epoch::start_epoch(&pool, tenant, "ps-3-test").await?;
    ensure!(start.predecessor.unclean_reason().is_none());
    let budget = SharedBudget::non_trading(before(&clock, 61)?, None, false)
        .map_err(|e| anyhow!("{e:?}"))?;
    let rig = Rig {
        clock,
        tenant,
        start,
        budget,
        server: TimeServer::start().await?,
    };
    rig.server.hold();
    let owner = rig.owner(&pool)?;
    let handle = owner.handle();
    let closing = owner.spawn();
    // The first cycle prepares its transport on a blocking permit before it
    // connects, which a loaded module run can delay past any fixed sleep:
    // wait (bounded) for its connection; the server then holds its answer
    // until the test releases it, so the queued request's 100 ms pass behind
    // the cycle in flight whatever the scheduling.
    let connected = in_ms(5_000);
    while rig.server.connections() == 0 && Instant::now() < connected {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    ensure!(rig.server.connections() == 1, "the scheduled cycle is in flight");
    // The test also waits on the queued request itself: the sampler answers
    // it when it takes it, after the held cycle, and only then may a later
    // request be made (one made before would join it and lend it a live
    // deadline).
    let deadline = in_ms(100);
    let taken = handle.requests.join(deadline).map_err(|e| anyhow!("join: {e:?}"))?;
    let queued = handle.request_sample(deadline).await;
    ensure!(queued == Err(TimeError::Expired), "{queued:?}");
    ensure!(rig.server.connections() == 1 && rig.handoffs()? == 1);
    rig.server.release();
    let taken = tokio::time::timeout(Duration::from_secs(10), taken).await;
    ensure!(
        matches!(taken, Ok(Ok(Err(TimeError::Expired)))),
        "the sampler sent the expired request: {taken:?}"
    );
    // Refused, unsent and spending no spacing: the next request goes out at
    // once as the second exchange.
    let next = handle.request_sample(in_ms(5_000)).await;
    ensure!(next.is_ok(), "the expired request spent the spacing: {next:?}");
    ensure!(
        rig.server.connections() == 2 && rig.handoffs()? == 2,
        "the expired request went out"
    );
    closing.begin_closing();
    ensure!(closing.join(in_ms(5_000)).await);
    Ok(())
}

/// One sample's raw captures, of `epoch`, a millisecond apart.
async fn raw_sample(clock: &NativeClock, tenant: Uuid, epoch: Uuid) -> Result<RawClockSample> {
    let send = clock.wall_bracket(epoch)?;
    tokio::time::sleep(Duration::from_millis(1)).await;
    let server_ms = Utc::now().timestamp_millis();
    let receive = clock.wall_bracket(epoch)?;
    Ok(RawClockSample {
        id: Uuid::now_v7(),
        tenant,
        venue: VenueId::new("binance:spot"),
        send,
        receive,
        server_ms,
    })
}

/// PLAN §7.2 (as built): the store's wire statements per branch: warm
/// success 1 (INSERT), cold success 2 (session bounds + INSERT), warm
/// ambiguity 3 (INSERT whose ACK is lost, then bounds + one confirmation on
/// a fresh session), cold ambiguity 4; a lost ACK is confirmed once and
/// every sample lands exactly once.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn the_clock_store_sends_one_to_four_statements_per_branch(pool: PgPool) -> Result<()> {
    let rig = rig(&pool).await?;
    let (proxy, wired) = wire(&pool).await?;
    let epoch = rig.start.epoch.epoch_id;
    let mut store = ClockSampleStore::from_committed_epoch(&wired, rig.tenant, &rig.start);
    let mut cold = ClockSampleStore::from_committed_epoch(&wired, rig.tenant, &rig.start);
    let lose_ack = || proxy.set_mode(ProxyMode::DropAutocommitAckOf("INSERT INTO clock_samples"));
    let mut counts = Vec::new();
    for (branch, ambiguous) in [
        ("cold", false),
        ("warm", false),
        ("warm", true),
        ("cold", true),
    ] {
        let raw = raw_sample(&rig.clock, rig.tenant, epoch).await?;
        if ambiguous {
            lose_ack();
        }
        let before = proxy.stats().statements;
        let target = if branch == "cold" && ambiguous {
            &mut cold
        } else {
            &mut store
        };
        let cycle = counts.len() as u64 + 1;
        target
            .persist(cycle, raw, Some(in_ms(2_000)))
            .await
            .map_err(|e| anyhow!("{branch} ambiguous={ambiguous}: {e:?}"))?;
        counts.push(proxy.stats().statements - before);
    }
    ensure!(counts == [2, 1, 3, 4], "statements per branch: {counts:?}");
    ensure!(proxy.autocommit_acks_dropped() == 2, "two lost INSERT ACKs");
    ensure!(epoch_samples(&pool, epoch).await? == 4, "each sample landed once");
    Ok(())
}
